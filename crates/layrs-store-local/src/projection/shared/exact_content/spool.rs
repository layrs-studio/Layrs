use super::*;
use journal::{Active, Piece};
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
};

pub(in crate::projection) struct OwnedPiece {
    manifest_id: FileManifestId,
    piece: Piece,
    inode: String,
    pin: PinnedFile,
    expected_bytes: u64,
    chunk_id: Option<String>,
    manifest_bytes: Option<Vec<u8>>,
    ready: bool,
    from_cas: bool,
}
impl OwnedPiece {
    pub(in crate::projection) fn ordinal(&self) -> usize {
        match self.piece {
            Piece::Chunk(n) => n as usize,
            Piece::Manifest => usize::MAX,
        }
    }
    pub(in crate::projection) fn needs_produce(&self) -> bool {
        !self.ready
    }
    pub(in crate::projection) fn produce(
        &mut self,
        produce: impl FnOnce(&mut dyn Write) -> Result<()>,
        cancel: &Cancellation,
    ) -> Result<()> {
        if self.ready {
            return Err(invalid("exact completed piece cannot be rewritten"));
        }
        cancel.check()?;
        if self.pin.file().metadata()?.len() != 0 {
            return Err(invalid(
                "exact partial piece is withheld; no append or reset",
            ));
        }
        let mut sink = PieceSink {
            output: self.pin.file_mut(),
            remaining: self.expected_bytes,
            hash: blake3::Hasher::new(),
            cancel,
            cancelled: false,
        };
        sink.hash.update(b"layrs-chunk-v1\0");
        let result = produce(&mut sink);
        if sink.cancelled {
            return Err(layrs_vfs::Error::Cancelled.into());
        }
        result?;
        if sink.remaining != 0 {
            return Err(invalid("exact producer ended before complete piece"));
        }
        if let Some(id) = &self.chunk_id {
            if format!("blake3:{}", sink.hash.finalize()) != *id {
                return Err(invalid("exact producer chunk identity differs"));
            }
        }
        drop(sink);
        verify_piece(
            &self.pin,
            self.expected_bytes,
            self.chunk_id.as_deref(),
            self.manifest_bytes.as_deref(),
            cancel,
        )?;
        if file_identity(&self.pin)? != self.inode {
            return Err(invalid("exact producer inode changed"));
        }
        self.pin.file().sync_all()?;
        self.pin.sync_parent()?;
        self.ready = true;
        Ok(())
    }
}

struct PieceSink<'a> {
    output: &'a mut dyn Write,
    remaining: u64,
    hash: blake3::Hasher,
    cancel: &'a Cancellation,
    cancelled: bool,
}
impl Write for PieceSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.cancel.is_cancelled() {
            self.cancelled = true;
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "exact producer cancelled",
            ));
        }
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "exact piece grew beyond manifest",
            ));
        }
        let count = bytes.len().min(64 * 1024).min(self.remaining as usize);
        let written = self.output.write(&bytes[..count])?;
        self.hash.update(&bytes[..written]);
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

pub(in crate::projection) fn prepare_piece<C: ExactCatalog>(
    context: &mut C,
    attempt: &mut ExactAttempt,
    cancel: &Cancellation,
) -> Result<Option<OwnedPiece>> {
    attempt.require_context(context, cancel)?;
    require_namespace(attempt)?;
    if attempt.record.phase != Phase::Allocated
        || attempt.record.spool_cursor == attempt.manifest.chunks().len() as u32
    {
        return Ok(None);
    }
    let piece = Piece::Chunk(attempt.record.spool_cursor);
    prepare_spool(context, attempt, piece, cancel).map(Some)
}

fn prepare_spool<C: ExactCatalog>(
    context: &mut C,
    attempt: &mut ExactAttempt,
    piece: Piece,
    cancel: &Cancellation,
) -> Result<OwnedPiece> {
    attempt.require_context(context, cancel)?;
    let (expected_bytes, chunk_id, manifest_bytes) = expected(attempt, piece)?;
    let (pin, inode, ready, from_cas) = if let Some(active) = &attempt.record.active {
        if active.piece != piece {
            return Err(invalid("exact active spool piece differs"));
        }
        // Existing bytes are never appended, truncated or reset. A complete
        // recorded inode can be verified and flushed through this narrow opener.
        let pin = if active.from_cas {
            attempt
                .storage
                .chunks
                .open_readonly_file(OsStr::new(object_name(
                    chunk_id
                        .as_deref()
                        .ok_or_else(|| invalid("exact resident input is not a chunk"))?,
                )?))?
        } else {
            attempt
                .directory
                .open_file_for_sync(OsStr::new(&piece.name()))?
        };
        if file_identity(&pin)? != active.inode {
            return Err(invalid("exact active spool inode replaced"));
        }
        verify_piece(
            &pin,
            expected_bytes,
            chunk_id.as_deref(),
            manifest_bytes.as_deref(),
            cancel,
        )?;
        if !active.from_cas {
            pin.file().sync_all()?;
            pin.sync_parent()?;
        }
        (pin, active.inode.clone(), true, active.from_cas)
    } else {
        let resident = context.with_catalog(false, cancel, |c| {
            if sql::load(c, &attempt.record.binding, &attempt.manifest)?.as_ref()
                != Some(&attempt.record)
            {
                return Err(invalid("exact spool predecessor changed"));
            }
            if sql::remaining(c, &attempt.record.binding)? < expected_bytes {
                return Err(invalid(
                    "partial_store_quota: exact spool exceeds retained reservation",
                ));
            }
            match &chunk_id {
                Some(id) => sql::chunk_indexed(c, id, expected_bytes),
                None => Ok(false),
            }
        })?;
        let pin = if resident {
            let pin = attempt
                .storage
                .chunks
                .open_readonly_file(OsStr::new(object_name(
                    chunk_id
                        .as_deref()
                        .ok_or_else(|| invalid("exact resident chunk id absent"))?,
                )?))?;
            verify_piece(&pin, expected_bytes, chunk_id.as_deref(), None, cancel)?;
            pin
        } else {
            if fs2::available_space(&attempt.storage.root_path)? < expected_bytes {
                return Err(invalid("insufficient space for exact spool piece"));
            }
            attempt
                .directory
                .create_new_private_file(OsStr::new(&piece.name()))?
        };
        let inode = file_identity(&pin)?;
        let mut next = attempt.record.clone();
        next.active = Some(Active {
            piece,
            inode: inode.clone(),
            from_cas: resident,
        });
        context.with_catalog(true, cancel, |c| {
            sql::replace(c, &attempt.record, &next, &attempt.manifest)
        })?;
        attempt.record = next;
        (pin, inode, resident, resident)
    };
    attempt.require_context(context, cancel)?;
    Ok(OwnedPiece {
        manifest_id: attempt.manifest.id().clone(),
        piece,
        inode,
        pin,
        expected_bytes,
        chunk_id,
        manifest_bytes,
        ready,
        from_cas,
    })
}

pub(in crate::projection) fn finish_piece<C: ExactCatalog>(
    context: &mut C,
    attempt: &mut ExactAttempt,
    piece: OwnedPiece,
    cancel: &Cancellation,
) -> Result<()> {
    attempt.require_context(context, cancel)?;
    if !piece.ready
        || piece.manifest_id != *attempt.manifest.id()
        || attempt.record.phase != Phase::Allocated
        || attempt.record.active.as_ref()
            != Some(&Active {
                piece: piece.piece,
                inode: piece.inode.clone(),
                from_cas: piece.from_cas,
            })
    {
        return Err(invalid(
            "exact finished piece differs from durable active inode",
        ));
    }
    verify_piece(
        &piece.pin,
        piece.expected_bytes,
        piece.chunk_id.as_deref(),
        piece.manifest_bytes.as_deref(),
        cancel,
    )?;
    let reopened = if piece.from_cas {
        attempt
            .storage
            .chunks
            .open_readonly_file(OsStr::new(object_name(
                piece
                    .chunk_id
                    .as_deref()
                    .ok_or_else(|| invalid("exact resident piece id absent"))?,
            )?))?
    } else {
        attempt
            .directory
            .open_file(OsStr::new(&piece.piece.name()))?
    };
    if file_identity(&piece.pin)? != piece.inode || !piece.pin.same_file(&reopened) {
        return Err(invalid("exact finished piece path was replaced"));
    }
    piece.pin.verify_parent()?;
    let mut next = attempt.record.clone();
    next.active = None;
    match piece.piece {
        Piece::Chunk(n) if n == next.spool_cursor => {
            next.spool_cursor += 1;
            if piece.from_cas {
                next.mark_resident(n as usize)?;
            }
        }
        Piece::Manifest if next.spool_cursor == attempt.manifest.chunks().len() as u32 => {
            next.phase = Phase::Verified;
            next.debit_bytes = next
                .debit_bytes
                .checked_add(attempt.manifest_bytes.len() as u64)
                .ok_or_else(|| invalid("exact manifest journal debit overflow"))?;
        }
        _ => return Err(invalid("exact finished piece cursor differs")),
    }
    context.with_catalog(true, cancel, |c| {
        sql::replace(c, &attempt.record, &next, &attempt.manifest)
    })?;
    attempt.record = next;
    attempt.require_context(context, cancel)
}

pub(in crate::projection) fn verify_staged(
    attempt: &ExactAttempt,
    cancel: &Cancellation,
) -> Result<()> {
    cancel.check()?;
    attempt.storage.verify()?;
    attempt.directory.verify_path_identity()?;
    require_namespace(attempt)?;
    if attempt.record.phase == Phase::Allocated
        && attempt.record.spool_cursor != attempt.manifest.chunks().len() as u32
    {
        return Err(invalid("exact staged file is incomplete"));
    }
    hash_file(attempt, cancel)?;
    if attempt.record.verified() {
        let (parent, pin) = manifest_pin(attempt)?;
        verify_piece(
            &pin,
            attempt.manifest_bytes.len() as u64,
            None,
            Some(&attempt.manifest_bytes),
            cancel,
        )?;
        let name = if parent.identity_token() == attempt.directory.identity_token() {
            "manifest"
        } else {
            object_name(attempt.manifest.id().as_str())?
        };
        if !pin.same_file(&parent.open_readonly_file(OsStr::new(name))?) {
            return Err(invalid("exact verified manifest path replaced"));
        }
    } else if attempt.record.phase != Phase::Allocated {
        return Err(invalid("exact staging phase cannot be verified"));
    }
    attempt.storage.verify()?;
    attempt.directory.verify_path_identity()?;
    cancel.check()?;
    Ok(())
}

pub(super) fn complete_staged<C: ExactCatalog>(
    context: &mut C,
    attempt: &mut ExactAttempt,
    cancel: &Cancellation,
) -> Result<()> {
    attempt.require_context(context, cancel)?;
    verify_staged(attempt, cancel)?;
    if attempt.record.phase == Phase::Allocated {
        let mut manifest = prepare_spool(context, attempt, Piece::Manifest, cancel)?;
        if manifest.needs_produce() {
            let bytes = attempt.manifest_bytes.clone();
            manifest.produce(
                |sink| {
                    sink.write_all(&bytes)?;
                    Ok(())
                },
                cancel,
            )?;
        }
        finish_piece(context, attempt, manifest, cancel)?;
    }
    attempt.require_context(context, cancel)
}

pub(super) fn require_namespace(attempt: &ExactAttempt) -> Result<()> {
    attempt.directory.verify_path_identity()?;
    let path = attempt
        .storage
        .root_path
        .join("staging")
        .join(&attempt.record.directory);
    let mut count = 0;
    for item in fs::read_dir(path)?.take(attempt.manifest.chunks().len() + 2) {
        count += 1;
        let name = item?.file_name();
        let allowed = name.to_str().is_some_and(|name| {
            name == "manifest"
                && (attempt.record.verified()
                    || attempt
                        .record
                        .active
                        .as_ref()
                        .is_some_and(|active| active.piece == Piece::Manifest))
                || name
                    .strip_prefix("chunk-")
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(|n| {
                        name == format!("chunk-{n}")
                            && (n < attempt.record.spool_cursor
                                && !attempt.record.resident_chunk(n as usize)
                                || attempt.record.active.as_ref().is_some_and(|active| {
                                    active.piece == Piece::Chunk(n) && !active.from_cas
                                }))
                    })
        });
        if !allowed || count > attempt.manifest.chunks().len() + 1 {
            return Err(invalid("exact staging has unrecognized entries; retained"));
        }
        attempt.directory.open_file(&name)?.verify_parent()?;
    }
    attempt.directory.verify_path_identity()?;
    Ok(())
}

fn expected(
    attempt: &ExactAttempt,
    piece: Piece,
) -> Result<(u64, Option<String>, Option<Vec<u8>>)> {
    match piece {
        Piece::Chunk(n) => {
            let chunk = attempt
                .manifest
                .chunks()
                .get(n as usize)
                .ok_or_else(|| invalid("exact chunk ordinal exceeds manifest"))?;
            Ok((
                chunk.byte_length(),
                Some(chunk.chunk_id().as_str().to_owned()),
                None,
            ))
        }
        Piece::Manifest => Ok((
            attempt.manifest_bytes.len() as u64,
            None,
            Some(attempt.manifest_bytes.clone()),
        )),
    }
}

pub(super) fn verify_piece(
    pin: &PinnedFile,
    expected: u64,
    chunk: Option<&str>,
    manifest: Option<&[u8]>,
    cancel: &Cancellation,
) -> Result<()> {
    cancel.check()?;
    if pin.file().metadata()?.len() != expected {
        return Err(invalid("exact partial or oversized piece is withheld"));
    }
    let mut reader = pin.file();
    reader.seek(SeekFrom::Start(0))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-chunk-v1\0");
    let mut offset = 0u64;
    let mut scratch = [0u8; 64 * 1024];
    while offset < expected {
        cancel.check()?;
        let count = (expected - offset).min(scratch.len() as u64) as usize;
        reader.read_exact(&mut scratch[..count])?;
        hash.update(&scratch[..count]);
        if manifest.is_some_and(|bytes| {
            bytes.get(offset as usize..offset as usize + count) != Some(&scratch[..count])
        }) {
            return Err(invalid("exact manifest staged bytes differ"));
        }
        offset += count as u64;
    }
    if reader.read(&mut [0u8; 1])? != 0 {
        return Err(invalid("exact piece grew during verification"));
    }
    if chunk.is_some_and(|id| id != format!("blake3:{}", hash.finalize())) {
        return Err(invalid("exact staged chunk hash differs"));
    }
    pin.verify_parent()?;
    cancel.check()?;
    Ok(())
}

pub(super) fn hash_file(attempt: &ExactAttempt, cancel: &Cancellation) -> Result<()> {
    let mut full = blake3::Hasher::new();
    full.update(b"layrs-file-v1\0");
    let mut offset = 0u64;
    let mut scratch = [0u8; 64 * 1024];
    for (n, chunk) in attempt.manifest.chunks().iter().enumerate() {
        cancel.check()?;
        if chunk.offset() != offset {
            return Err(invalid("exact file chunk offset differs"));
        }
        let name = format!("chunk-{n}");
        let pin = if n < attempt.record.publish_cursor as usize || attempt.record.resident_chunk(n)
        {
            attempt
                .storage
                .chunks
                .open_readonly_file(OsStr::new(object_name(chunk.chunk_id().as_str())?))?
        } else {
            match attempt.directory.open_file(OsStr::new(&name)) {
                Ok(pin) => pin,
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && attempt.record.phase == Phase::Publishing =>
                {
                    let active = attempt.record.active.as_ref().ok_or_else(|| {
                        invalid("exact missing chunk has no active publication inode")
                    })?;
                    if active.piece != Piece::Chunk(n as u32) {
                        return Err(invalid(
                            "exact missing chunk differs from active publication",
                        ));
                    }
                    let pin = attempt
                        .storage
                        .chunks
                        .open_readonly_file(OsStr::new(object_name(chunk.chunk_id().as_str())?))?;
                    if file_identity(&pin)? != active.inode {
                        return Err(invalid("exact unindexed chunk is not the active inode"));
                    }
                    pin
                }
                Err(error) => return Err(error.into()),
            }
        };
        if pin.file().metadata()?.len() != chunk.byte_length() {
            return Err(invalid("exact file chunk length differs"));
        }
        let mut reader = pin.file();
        reader.seek(SeekFrom::Start(0))?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-chunk-v1\0");
        let mut remaining = chunk.byte_length();
        while remaining != 0 {
            cancel.check()?;
            let count = remaining.min(scratch.len() as u64) as usize;
            reader.read_exact(&mut scratch[..count])?;
            hash.update(&scratch[..count]);
            full.update(&scratch[..count]);
            remaining -= count as u64;
            offset += count as u64;
        }
        if reader.read(&mut [0u8; 1])? != 0
            || format!("blake3:{}", hash.finalize()) != chunk.chunk_id().as_str()
        {
            return Err(invalid("exact file chunk bytes differ"));
        }
        pin.verify_parent()?;
    }
    if offset != attempt.manifest.byte_length()
        || format!("blake3:{}", full.finalize()) != attempt.manifest.file_id().as_str()
    {
        return Err(invalid("exact full FileId differs"));
    }
    cancel.check()?;
    Ok(())
}

fn manifest_pin(attempt: &ExactAttempt) -> Result<(&PinnedDirectory, PinnedFile)> {
    if attempt.record.phase == Phase::Published {
        let parent = &attempt.storage.manifests;
        return Ok((
            parent,
            parent.open_readonly_file(OsStr::new(object_name(attempt.manifest.id().as_str())?))?,
        ));
    }
    match attempt.directory.open_file(OsStr::new("manifest")) {
        Ok(pin) => Ok((&attempt.directory, pin)),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && attempt.record.phase == Phase::Publishing =>
        {
            let active = attempt
                .record
                .active
                .as_ref()
                .ok_or_else(|| invalid("exact missing manifest has no active inode"))?;
            let pin = attempt
                .storage
                .manifests
                .open_readonly_file(OsStr::new(object_name(attempt.manifest.id().as_str())?))?;
            if active.piece != Piece::Manifest || file_identity(&pin)? != active.inode {
                return Err(invalid(
                    "exact manifest is not the active publication inode",
                ));
            }
            Ok((&attempt.storage.manifests, pin))
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
#[path = "spool_tests.rs"]
mod tests;
