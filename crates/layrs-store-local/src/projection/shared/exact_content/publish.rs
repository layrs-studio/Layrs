use super::*;
use journal::{Active, Piece};
use rusqlite::params;
use std::io;

pub(in crate::projection) fn publish<C: ExactCatalog>(
    context: &mut C,
    attempt: &mut ExactAttempt,
    cancel: &Cancellation,
) -> Result<()> {
    attempt.require_context(context, cancel)?;
    spool::complete_staged(context, attempt, cancel)?;
    if attempt.record.phase == Phase::Verified {
        let mut next = attempt.record.clone();
        next.phase = Phase::Publishing;
        context.with_catalog(true, cancel, |c| {
            sql::replace(c, &attempt.record, &next, &attempt.manifest)
        })?;
        attempt.record = next;
    }
    while attempt.record.phase == Phase::Publishing
        && attempt.record.publish_cursor < attempt.manifest.chunks().len() as u32
    {
        publish_one(
            context,
            attempt,
            Piece::Chunk(attempt.record.publish_cursor),
            cancel,
        )?;
    }
    if attempt.record.phase == Phase::Publishing {
        publish_one(context, attempt, Piece::Manifest, cancel)?;
    }
    if attempt.record.phase != Phase::Published {
        return Err(invalid("exact publication did not reach Published"));
    }
    context.with_catalog(false, cancel, |c| {
        if sql::load(c, &attempt.record.binding, &attempt.manifest)?.as_ref()
            != Some(&attempt.record)
            || !sql::manifest_indexed(c, &attempt.manifest)?
        {
            return Err(invalid("exact Published readback differs"));
        }
        Ok(())
    })?;
    // Publication is still unready. Verify installed bytes after all positive
    // metadata closes; later protection/private Ready remain separate work.
    spool::verify_staged(attempt, cancel)?;
    attempt.require_context(context, cancel)
}

fn indexed(c: &Connection, attempt: &ExactAttempt, piece: Piece) -> Result<bool> {
    sql::require_file_length(c, &attempt.manifest)?;
    match piece {
        Piece::Chunk(n) => {
            let part = &attempt.manifest.chunks()[n as usize];
            sql::chunk_indexed(c, part.chunk_id().as_str(), part.byte_length())
        }
        Piece::Manifest => sql::manifest_indexed(c, &attempt.manifest),
    }
}
fn info(
    attempt: &ExactAttempt,
    piece: Piece,
) -> Result<(&PinnedDirectory, &str, u64, Option<&str>, Option<&[u8]>)> {
    match piece {
        Piece::Chunk(n) => {
            let part = attempt
                .manifest
                .chunks()
                .get(n as usize)
                .ok_or_else(|| invalid("exact publication chunk ordinal differs"))?;
            Ok((
                &attempt.storage.chunks,
                object_name(part.chunk_id().as_str())?,
                part.byte_length(),
                Some(part.chunk_id().as_str()),
                None,
            ))
        }
        Piece::Manifest => Ok((
            &attempt.storage.manifests,
            object_name(attempt.manifest.id().as_str())?,
            attempt.manifest_bytes.len() as u64,
            None,
            Some(&attempt.manifest_bytes),
        )),
    }
}

fn publish_one<C: ExactCatalog>(
    context: &mut C,
    attempt: &mut ExactAttempt,
    piece: Piece,
    cancel: &Cancellation,
) -> Result<()> {
    attempt.require_context(context, cancel)?;
    let was_indexed = context.with_catalog(false, cancel, |c| {
        if sql::load(c, &attempt.record.binding, &attempt.manifest)?.as_ref()
            != Some(&attempt.record)
        {
            return Err(invalid("exact publication predecessor changed"));
        }
        let exists = indexed(c, attempt, piece)?;
        let cost = if exists { 0 } else { info(attempt, piece)?.2 };
        if sql::remaining(c, &attempt.record.binding)? < cost {
            return Err(invalid(
                "partial_store_quota: exact publication exceeds reservation",
            ));
        }
        Ok(exists)
    })?;
    let from_cas = match piece {
        Piece::Chunk(n) => attempt.record.resident_chunk(n as usize),
        Piece::Manifest => false,
    };
    if attempt.record.active.is_none() {
        let pin = if from_cas {
            if !was_indexed {
                return Err(invalid("exact resident publication lost indexed chunk"));
            }
            let (parent, name, _, _, _) = info(attempt, piece)?;
            parent.open_file(OsStr::new(name))?
        } else {
            attempt.directory.open_file(OsStr::new(&piece.name()))?
        };
        let (_, _, size, chunk, manifest) = info(attempt, piece)?;
        spool::verify_piece(&pin, size, chunk, manifest, cancel)?;
        let mut next = attempt.record.clone();
        next.active = Some(Active {
            piece,
            inode: file_identity(&pin)?,
            from_cas,
        });
        context.with_catalog(true, cancel, |c| {
            sql::replace(c, &attempt.record, &next, &attempt.manifest)
        })?;
        attempt.record = next;
    }
    let active = attempt
        .record
        .active
        .clone()
        .ok_or_else(|| invalid("exact publication active inode absent"))?;
    if active.piece != piece || active.from_cas != from_cas {
        return Err(invalid("exact publication active object differs"));
    }
    let (parent, name, size, chunk, manifest) = info(attempt, piece)?;
    let staged = if active.from_cas {
        None
    } else {
        match attempt.directory.open_file(OsStr::new(&piece.name())) {
            Ok(pin) => {
                if file_identity(&pin)? != active.inode {
                    return Err(invalid("exact publication staged inode replaced"));
                }
                spool::verify_piece(&pin, size, chunk, manifest, cancel)?;
                Some(pin)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        }
    };
    let target = match parent.open_file(OsStr::new(name)) {
        Ok(pin) => Some(pin),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let target = match target {
        Some(pin) => {
            let same_inode = file_identity(&pin)? == active.inode;
            if !same_inode && (!was_indexed || staged.is_none()) {
                return Err(invalid("exact foreign unindexed CAS collision is withheld"));
            }
            spool::verify_piece(&pin, size, chunk, manifest, cancel)?;
            pin
        }
        None => {
            if was_indexed || active.from_cas {
                return Err(invalid("exact indexed immutable object is missing"));
            }
            let source = staged
                .as_ref()
                .ok_or_else(|| invalid("exact active inode absent from staging and CAS"))?;
            attempt.require_context(context, cancel)?;
            // true means collision; neither outcome replaces an existing leaf.
            let collision = source.rename_to(parent, OsStr::new(name))?;
            parent.sync()?;
            attempt.directory.sync()?;
            let pin = parent.open_file(OsStr::new(name))?;
            let same_inode = file_identity(&pin)? == active.inode;
            if !same_inode {
                if !collision {
                    return Err(invalid("exact rename published another inode"));
                }
                let now_indexed =
                    context.with_catalog(false, cancel, |c| indexed(c, attempt, piece))?;
                if !now_indexed {
                    return Err(invalid(
                        "exact collision has no authenticated catalog object",
                    ));
                }
            }
            spool::verify_piece(&pin, size, chunk, manifest, cancel)?;
            pin
        }
    };
    let target_inode = file_identity(&target)?;
    parent.sync()?;
    attempt.directory.sync()?;
    target.verify_parent()?;
    if !target.same_file(&parent.open_file(OsStr::new(name))?) {
        return Err(invalid("exact CAS path changed after verification"));
    }
    attempt.require_context(context, cancel)?;
    let mut next = attempt.record.clone();
    next.active = None;
    context.with_catalog(true, cancel, |c| {
        let exists = indexed(c, attempt, piece)?;
        if !exists && target_inode != active.inode { return Err(invalid("exact unindexed CAS object is not the journal active inode")); }
        let cost = if exists { 0 } else { size };
        if sql::remaining(c, &attempt.record.binding)? < cost { return Err(invalid("partial_store_quota: exact publication exceeds fixed reservation")); }
        match piece {
            Piece::Chunk(n) => {
                let part = &attempt.manifest.chunks()[n as usize];
                if !exists {
                    c.execute("INSERT INTO chunks(id,bytes) VALUES(?1,?2)", params![part.chunk_id().as_str(), part.byte_length()])?;
                    next.charge_chunk(n as usize, part.byte_length())?;
                }
                next.publish_cursor += 1;
            }
            Piece::Manifest => {
                if !exists {
                    c.execute("INSERT INTO files(file_id,bytes) VALUES(?1,?2) ON CONFLICT(file_id) DO NOTHING", params![attempt.manifest.file_id().as_str(), attempt.manifest.byte_length()])?;
                    sql::require_file_length(c, &attempt.manifest)?;
                    c.execute("INSERT INTO manifests(id,file_id,bytes,payload) VALUES(?1,?2,?3,?4)", params![attempt.manifest.id().as_str(), attempt.manifest.file_id().as_str(), attempt.manifest.byte_length(), attempt.manifest_bytes])?;
                    for (n, part) in attempt.manifest.chunks().iter().enumerate() {
                        cancel.check()?;
                        if !sql::chunk_indexed(c, part.chunk_id().as_str(), part.byte_length())? { return Err(invalid("exact manifest publication lacks completed chunk")); }
                        c.execute("INSERT INTO manifest_chunks(manifest_id,ordinal,chunk_id) VALUES(?1,?2,?3)", params![attempt.manifest.id().as_str(), n as u64, part.chunk_id().as_str()])?;
                    }
                    next.charged_manifest = true;
                    next.debit_bytes = next.debit_bytes.checked_add(attempt.manifest_bytes.len() as u64).ok_or_else(|| invalid("exact manifest publication debit overflow"))?;
                }
                next.phase = Phase::Published;
            }
        }
        sql::replace(c, &attempt.record, &next, &attempt.manifest)
    })?;
    attempt.record = next;
    // Duplicates remain in the authenticated staging directory. They are not
    // unlinked and their retained reservation is not silently released.
    attempt.require_context(context, cancel)
}
