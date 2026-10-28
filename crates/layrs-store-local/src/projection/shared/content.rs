use super::*;
use layrs_core::scm::{
    Chunk, ChunkId, ChunkingDescriptor, FileId, FileManifestBuilder, FileManifestId,
};
use std::{collections::BTreeSet, io::Read};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum InstallationPoint {
    IntentDurable,
    ChunkStaged,
    ManifestDurable,
    ChunkPublished,
    BeforeCatalogCommit,
    CatalogCommitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct Protection {
    pub incarnation: String,
    pub owner: WorktreeId,
    pub database_identity: String,
    pub operation: String,
    pub manifests: Vec<FileManifest>,
}
impl Lease {
    /// Reuse the private owner's exact representation rather than choosing a
    /// different family manifest with the same FileId. The live input still
    /// has to match every chunk of the retained representation.
    pub(in crate::projection) fn retain_existing_representation(
        &self,
        input: &mut dyn Read,
        expected: &CapturedFile,
        reservation: &str,
        manifest: &FileManifest,
    ) -> Result<FileManifest> {
        if manifest.file_id() != &expected.file_id || manifest.byte_length() != expected.bytes {
            return Err(invalid(
                "retained private representation differs from input",
            ));
        }
        self.verify_manifest(manifest)?;
        verify_input(input, manifest)?;
        staging::Stage::finish_existing(self, reservation, expected)?;
        Ok(manifest.clone())
    }

    /// Caller holds this lifecycle lease until publication and private acceptance.
    /// Local fixed chunking deliberately does not preserve a remote manifest ID.
    pub(in crate::projection) fn install_file(
        &self,
        input: &mut dyn Read,
        expected: &CapturedFile,
        reservation: &str,
    ) -> Result<FileManifest> {
        self.install_file_with_faults(input, expected, reservation, &mut |_| Ok(()))
    }
    pub(in crate::projection) fn install_file_with_faults(
        &self,
        input: &mut dyn Read,
        expected: &CapturedFile,
        reservation: &str,
        faults: &mut dyn FnMut(InstallationPoint) -> io::Result<()>,
    ) -> Result<FileManifest> {
        if expected.bytes > MAX_CAPTURE_FILE_BYTES {
            return Err(invalid("shared file exceeds 1GiB"));
        }
        if let Some(manifest) = self.manifest_for(&expected.file_id)? {
            self.verify_manifest(&manifest)?;
            if manifest.byte_length() != expected.bytes {
                return Err(invalid("existing file length mismatch"));
            }
            verify_input(input, &manifest)?;
            staging::Stage::finish_existing(self, reservation, expected)?;
            return Ok(manifest);
        }
        let mut attempt = staging::Stage::begin(self, reservation, expected)?;
        faults(InstallationPoint::IntentDurable)?;
        let staging = attempt.directory(self)?;
        if let Some(manifest) = &attempt.manifest {
            verify_input(input, manifest)?;
        } else {
            attempt.reset_unverified(self)?;
            let mut builder = FileManifestBuilder::new(
                ChunkingDescriptor::deterministic_fixed(CAPTURE_CHUNK_BYTES as u64)
                    .map_err(invalid)?,
                MAX_CAPTURE_FILE_BYTES,
                1024,
            )
            .map_err(invalid)?;
            let mut remaining = expected.bytes;
            let mut ordinal = 0usize;
            while remaining > 0 {
                let size = remaining.min(CAPTURE_CHUNK_BYTES as u64) as usize;
                let mut bytes = vec![0; size];
                input.read_exact(&mut bytes)?;
                let chunk = Chunk::new(bytes).map_err(invalid)?;
                builder.push_chunk(&chunk).map_err(invalid)?;
                let name = format!("chunk-{ordinal}");
                let mut file = staging.create_file(name.as_ref())?;
                file.file_mut().write_all(chunk.bytes())?;
                file.file().sync_all()?;
                ordinal += 1;
                remaining -= size as u64;
                faults(InstallationPoint::ChunkStaged)?;
            }
            if input.read(&mut [0u8; 1])? != 0 {
                return Err(invalid("shared source grew during installation"));
            }
            let manifest = builder.finish().map_err(invalid)?;
            if manifest.file_id() != &expected.file_id || manifest.byte_length() != expected.bytes {
                return Err(invalid("shared source identity changed"));
            }
            let payload = canonical_json(&manifest).map_err(invalid)?;
            let mut mf = staging.create_file("manifest".as_ref())?;
            mf.file_mut().write_all(&payload)?;
            mf.file().sync_all()?;
            drop(mf);
            staging.sync()?;
            attempt.verified(self, &manifest)?;
        }
        drop(staging);
        faults(InstallationPoint::ManifestDurable)?;
        self.publish_installation(attempt, faults)
    }
    pub(super) fn publish_installation(
        &self,
        mut attempt: staging::Stage,
        faults: &mut dyn FnMut(InstallationPoint) -> io::Result<()>,
    ) -> Result<FileManifest> {
        let manifest = attempt
            .manifest
            .clone()
            .ok_or_else(|| invalid("installation is not verified"))?;
        let reservation = attempt.reservation().to_owned();
        let directory = PinnedDirectory::open(&self.family.root)?;
        let staging = attempt.directory(self)?;
        let payload = canonical_json(&manifest).map_err(invalid)?;
        self.with_content_catalog(true, |c| {
        self.authorize_content_reservation(c, &reservation)?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut cost = 0u64;
        let mut unique = BTreeSet::new();
        for part in manifest.chunks() {
            let id = part.chunk_id();
            if unique.insert(id.clone())
                && !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM chunks WHERE id=?1)",
                    [id.as_str()],
                    |r| r.get::<_, bool>(0),
                )?
            {
                cost += part.byte_length();
            }
        }
        let exists = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM manifests WHERE id=?1)",
            [manifest.id().as_str()],
            |r| r.get::<_, bool>(0),
        )?;
        if !exists {
            cost += payload.len() as u64;
        }
        let reserved: u64 = tx.query_row(
            "SELECT remaining FROM reservations WHERE id=?1",
            [&reservation],
            |r| r.get(0),
        )?;
        if cost > reserved {
            return Err(invalid(
                "partial_store_quota: installation exceeds reservation",
            ));
        }
        prepare_manifest_file(&tx, &manifest)?;
        let chunks = directory
            .open_child("objects".as_ref())?
            .open_child("chunks".as_ref())?;
        let mut verifier = manifest.verifier();
        for (ordinal, part) in manifest.chunks().iter().enumerate() {
            let id = part.chunk_id();
            let size = part.byte_length();
            let source = match staging.open_file(format!("chunk-{ordinal}").as_ref()) {
                Ok(source) => Some(source),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            #[cfg(windows)]
            if self.is_admitted() {
                if let Some(source) = &source {
                    crate::projection::file::require_single_link(source.file())?;
                }
            }
            let name = object_name(id.as_str())?;
            let existed = source
                .as_ref()
                .map(|source| source.rename_to(&chunks, name.as_ref()))
                .transpose()?
                .unwrap_or(false);
            #[cfg(windows)]
            if self.is_admitted() {
                let destination = chunks.open_readonly_file(name.as_ref())?;
                crate::projection::file::require_single_link(destination.file())?;
            }
            let actual = read_chunk_at(&chunks, id, size)?;
            verifier.push(actual.bytes()).map_err(invalid)?;
            if actual.byte_length() != size {
                return Err(invalid("shared chunk length changed"));
            }
            if existed {
                source
                    .as_ref()
                    .ok_or_else(|| invalid("missing duplicate stage"))?
                    .unlink()?;
            }
            tx.execute(
                "INSERT INTO chunks VALUES(?1,?2) ON CONFLICT(id) DO NOTHING",
                params![id.as_str(), size],
            )?;
            faults(InstallationPoint::ChunkPublished)?;
            // A prior sweep may have committed its intent but failed to unlink
            // this immutable file (for example, a Windows sharing violation).
            // Readmission and cancellation of that intent must be atomic.
            tx.execute(
                "DELETE FROM sweep WHERE kind='chunks' AND id=?1",
                [id.as_str()],
            )?;
        }
        verifier.finish().map_err(invalid)?;
        chunks.sync()?;
        let manifests = directory
            .open_child("objects".as_ref())?
            .open_child("manifests".as_ref())?;
        let source = match staging.open_file("manifest".as_ref()) {
            Ok(source) => Some(source),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        #[cfg(windows)]
        if self.is_admitted() {
            if let Some(source) = &source {
                crate::projection::file::require_single_link(source.file())?;
            }
        }
        let name = object_name(manifest.id().as_str())?;
        let existed = source
            .as_ref()
            .map(|source| source.rename_to(&manifests, name.as_ref()))
            .transpose()?
            .unwrap_or(false);
        #[cfg(windows)]
        if self.is_admitted() {
            let destination = manifests.open_readonly_file(name.as_ref())?;
            crate::projection::file::require_single_link(destination.file())?;
        }
        if bounded_file(
            &self.family.root.join("objects/manifests").join(&name),
            262144,
        )? != payload
        {
            return Err(invalid("shared manifest collision or corruption"));
        }
        if existed {
            source
                .as_ref()
                .ok_or_else(|| invalid("missing duplicate manifest stage"))?
                .unlink()?;
        }
        manifests.sync()?;
        staging.sync()?;
        // Flush verified immutable bytes before publishing catalog references.
        tx.execute(
            "INSERT INTO manifests VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO NOTHING",
            params![
                manifest.id().as_str(),
                manifest.file_id().as_str(),
                manifest.byte_length(),
                payload
            ],
        )?;
        tx.execute(
            "DELETE FROM sweep WHERE kind='manifests' AND id=?1",
            [manifest.id().as_str()],
        )?;
        for (ordinal, chunk) in manifest.chunks().iter().enumerate() {
            tx.execute("INSERT INTO manifest_chunks VALUES(?1,?2,?3) ON CONFLICT(manifest_id,ordinal) DO NOTHING", params![manifest.id().as_str(),ordinal as u64,chunk.chunk_id().as_str()])?;
        }
        tx.execute(
            "UPDATE reservations SET remaining=remaining-?1 WHERE id=?2",
            params![cost, reservation],
        )?;
        attempt.accept(&tx)?;
        faults(InstallationPoint::BeforeCatalogCommit)?;
        tx.commit()?;
        Ok(())
        })?;
        attempt.published = true;
        faults(InstallationPoint::CatalogCommitted)?;
        self.verify_manifest(&manifest)?;
        drop(staging);
        attempt.finish(self)?;
        Ok(manifest)
    }
    pub(in crate::projection) fn manifest_for(&self, id: &FileId) -> Result<Option<FileManifest>> {
        self.with_content_catalog(false, |c| read_manifest_for(c, id))
    }
    pub(in crate::projection) fn manifest(&self, id: &FileManifestId) -> Result<FileManifest> {
        self.manifest_optional(id)?
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows.into())
    }
    pub(in crate::projection) fn manifest_optional(
        &self,
        id: &FileManifestId,
    ) -> Result<Option<FileManifest>> {
        self.with_content_catalog(false, |c| read_manifest_optional(c, id))
    }
    pub(in crate::projection) fn verify_manifest(&self, manifest: &FileManifest) -> Result<()> {
        self.stream(manifest, &mut io::sink())
    }
    pub(in crate::projection) fn check_manifest_resident(
        &self,
        manifest: &FileManifest,
    ) -> Result<()> {
        if self.manifest(manifest.id())? != *manifest {
            return Err(invalid("shared representation changed"));
        }
        let bytes = bounded_file(
            &self
                .family
                .root
                .join("objects/manifests")
                .join(object_name(manifest.id().as_str())?),
            262144,
        )?;
        if decode_manifest(&bytes)? != *manifest {
            return Err(invalid("shared manifest bytes changed"));
        }
        let directory = PinnedDirectory::open(&self.family.root.join("objects/chunks"))?;
        for part in manifest.chunks() {
            let pin = directory.open_file(object_name(part.chunk_id().as_str())?.as_ref())?;
            if pin.file().metadata()?.len() != part.byte_length() {
                return Err(invalid("shared chunk missing or truncated"));
            }
            pin.verify_parent()?;
        }
        directory.verify_path_identity()?;
        self.verify()
    }
    pub(in crate::projection) fn stream(
        &self,
        manifest: &FileManifest,
        out: &mut dyn Write,
    ) -> Result<()> {
        // Lease is retained by self from exact representation resolution to EOF.
        let stored = self.manifest(manifest.id())?;
        if stored != *manifest {
            return Err(invalid("shared representation changed"));
        }
        let bytes = bounded_file(
            &self
                .family
                .root
                .join("objects/manifests")
                .join(object_name(manifest.id().as_str())?),
            262144,
        )?;
        if decode_manifest(&bytes)? != *manifest {
            return Err(invalid("shared manifest bytes changed"));
        }
        let directory = PinnedDirectory::open(&self.family.root.join("objects/chunks"))?;
        let mut verifier = manifest.verifier();
        for part in manifest.chunks() {
            let chunk = read_chunk_at(&directory, part.chunk_id(), part.byte_length())?;
            verifier.push(chunk.bytes()).map_err(invalid)?;
            out.write_all(chunk.bytes())?;
        }
        verifier.finish().map_err(invalid)?;
        directory.verify_path_identity()?;
        self.verify()
    }
    pub(in crate::projection) fn stream_chunk(
        &self,
        manifest: &FileManifest,
        ordinal: usize,
        out: &mut dyn Write,
    ) -> Result<()> {
        if self.manifest(manifest.id())? != *manifest {
            return Err(invalid("shared chunk representation changed"));
        }
        let part = manifest
            .chunks()
            .get(ordinal)
            .ok_or_else(|| invalid("shared chunk ordinal out of range"))?;
        let directory = PinnedDirectory::open(&self.family.root.join("objects/chunks"))?;
        out.write_all(read_chunk_at(&directory, part.chunk_id(), part.byte_length())?.bytes())?;
        self.verify()
    }
    pub(in crate::projection) fn protect(
        &self,
        marker: &format::Marker,
        operation: &str,
        manifests: &[FileManifest],
    ) -> Result<(String, String)> {
        self.authorize_content_protection(marker, operation)?;
        let binding = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("shared protection requires family"))?;
        let mut manifests = manifests.to_vec();
        manifests.sort_by(|a, b| a.id().cmp(b.id()));
        manifests.dedup_by(|a, b| a.id() == b.id());
        for manifest in &manifests {
            self.verify_manifest(manifest)?;
        }
        let protection = Protection {
            incarnation: binding.incarnation.clone(),
            owner: marker.identity.worktree_id.clone(),
            database_identity: binding.database_identity.clone(),
            operation: operation.into(),
            manifests,
        };
        let payload = canonical_json(&protection).map_err(invalid)?;
        if payload.len() > 16 * 1024 * 1024 {
            return Err(invalid("shared protection exceeds bound"));
        }
        let digest = digest(&protection)?;
        let id = digest.clone();
        self.with_content_catalog(true, |c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO roots VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO NOTHING",
                params![
                    id,
                    protection.owner.as_str(),
                    protection.database_identity,
                    operation,
                    digest,
                    payload
                ],
            )?;
            let old: Vec<u8> =
                tx.query_row("SELECT payload FROM roots WHERE id=?1", [&id], |r| r.get(0))?;
            if old != payload {
                return Err(invalid("shared protection identity collision"));
            }
            for manifest in &protection.manifests {
                tx.execute(
                    "INSERT INTO root_manifests VALUES(?1,?2) ON CONFLICT DO NOTHING",
                    params![id, manifest.id().as_str()],
                )?;
            }
            tx.commit()?;
            Ok((id, digest))
        })
    }
    pub(in crate::projection) fn verify_protection(
        &self,
        marker: &format::Marker,
        id: &str,
        expected: &str,
    ) -> Result<Vec<FileManifest>> {
        let manifests = self.verify_protection_metadata(marker, id, expected)?;
        for manifest in &manifests {
            self.verify_manifest(manifest)?;
        }
        Ok(manifests)
    }
    pub(in crate::projection) fn verify_protection_metadata(
        &self,
        marker: &format::Marker,
        id: &str,
        expected: &str,
    ) -> Result<Vec<FileManifest>> {
        self.with_content_catalog(false, |c| {
            let (claimed, payload): (String, Vec<u8>) = c.query_row(
                "SELECT digest,payload FROM roots WHERE id=?1 AND length(payload)<=16777216",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let p: Protection = serde_json::from_slice(&payload).map_err(invalid)?;
            let binding = marker
                .shared
                .as_ref()
                .ok_or_else(|| invalid("missing protection binding"))?;
            if claimed != expected
                || claimed != id
                || digest(&p)? != claimed
                || canonical_json(&p).map_err(invalid)? != payload
                || p.incarnation != binding.incarnation
                || p.owner != marker.identity.worktree_id
                || p.database_identity != binding.database_identity
            {
                return Err(invalid("shared protection owner or membership mismatch"));
            }
            let mut statement = c.prepare(
                "SELECT manifest_id FROM root_manifests WHERE root_id=?1 ORDER BY manifest_id",
            )?;
            let actual = statement
                .query_map([id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if actual
                != p.manifests
                    .iter()
                    .map(|m| m.id().to_string())
                    .collect::<Vec<_>>()
            {
                return Err(invalid("shared protection membership index mismatch"));
            }
            Ok(p.manifests)
        })
    }
}

/// The active catalog gate and caller transaction cover this check and the
/// subsequent publication. A schema-2 length collision is refused before any
/// immutable filesystem object is renamed into the family.
fn prepare_manifest_file(c: &Connection, manifest: &FileManifest) -> Result<()> {
    if c.is_autocommit() {
        return Err(invalid(
            "manifest file metadata requires caller transaction",
        ));
    }
    match catalog_schema_version(c)? {
        CatalogSchema::V1 => Ok(()),
        #[cfg(windows)]
        CatalogSchema::V2 => {
            c.execute(
                "INSERT INTO files(file_id,bytes) VALUES(?1,?2) ON CONFLICT(file_id) DO NOTHING",
                params![manifest.file_id().as_str(), manifest.byte_length()],
            )?;
            let bytes: u64 = c.query_row(
                "SELECT bytes FROM files WHERE file_id=?1",
                [manifest.file_id().as_str()],
                |r| r.get(0),
            )?;
            if bytes != manifest.byte_length() {
                return Err(invalid("shared FileId length collision"));
            }
            Ok(())
        }
    }
}

fn read_manifest_for(c: &Connection, file: &FileId) -> Result<Option<FileManifest>> {
    let key_bytes: Option<u64> = c
        .query_row(
            "SELECT length(CAST(id AS BLOB)) FROM manifests WHERE file_id=?1 ORDER BY id COLLATE BINARY LIMIT 1",
            [file.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(key_bytes) = key_bytes else {
        return Ok(None);
    };
    if key_bytes != 71 {
        return Err(invalid("shared FileId index key exceeds bound"));
    }
    let id: String = c.query_row(
        "SELECT id FROM manifests WHERE file_id=?1 ORDER BY id COLLATE BINARY LIMIT 1",
        [file.as_str()],
        |r| r.get(0),
    )?;
    let id = FileManifestId::parse(id).map_err(invalid)?;
    let manifest = read_manifest_optional(c, &id)?
        .ok_or_else(|| invalid("shared FileId index lost its representation"))?;
    if manifest.file_id() != file {
        return Err(invalid("shared FileId index mismatch"));
    }
    Ok(Some(manifest))
}

/// Exact ManifestId SQL DATA only. The caller authenticates the family and
/// checks its expected FileId/length, aggregate bound and physical bytes.
/// This never chooses another representation or creates a lease/capability.
#[cfg(windows)]
pub(in crate::projection) fn read_manifest_exact(
    c: &Connection,
    id: &FileManifestId,
    cancel: Option<&layrs_vfs::Cancellation>,
) -> Result<Option<FileManifest>> {
    crate::projection::provider::cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "exact manifest DATA requires retained caller transaction",
        ));
    }
    let manifest = read_manifest_optional(c, id)?;
    crate::projection::provider::cancellation::check(cancel)?;
    Ok(manifest)
}

fn read_manifest_optional(c: &Connection, id: &FileManifestId) -> Result<Option<FileManifest>> {
    let bounds: Option<(u64, u64, u64)> = c
        .query_row(
            "SELECT length(CAST(file_id AS BLOB)),length(payload),bytes FROM manifests WHERE id=?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((file_bytes, payload_bytes, declared_bytes)) = bounds else {
        return Ok(None);
    };
    if file_bytes != 71 || payload_bytes > 262144 || declared_bytes > MAX_CAPTURE_FILE_BYTES {
        return Err(invalid("shared manifest catalog bounds differ"));
    }
    let (file, bytes): (String, Vec<u8>) = c.query_row(
        "SELECT file_id,payload FROM manifests WHERE id=?1",
        [id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let value = decode_manifest(&bytes)?;
    if value.id() != id || value.file_id().as_str() != file || value.byte_length() != declared_bytes
    {
        return Err(invalid("shared manifest index mismatch"));
    }
    let (count, key_bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(chunk_id AS BLOB))),0) FROM (SELECT chunk_id FROM manifest_chunks WHERE manifest_id=?1 LIMIT 1025)",
        [id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != value.chunks().len() as u64 || key_bytes > 71 {
        return Err(invalid("shared manifest chunk index bound differs"));
    }
    let mut query = c.prepare("SELECT mc.ordinal,mc.chunk_id,c.bytes FROM manifest_chunks mc LEFT JOIN chunks c ON c.id=mc.chunk_id WHERE mc.manifest_id=?1 ORDER BY mc.ordinal LIMIT 1025")?;
    let mut rows = query.query([id.as_str()])?;
    for (ordinal, part) in value.chunks().iter().enumerate() {
        let row = rows
            .next()?
            .ok_or_else(|| invalid("shared manifest chunk index short"))?;
        let actual_ordinal: u64 = row.get(0)?;
        let actual_id: String = row.get(1)?;
        let actual_bytes: Option<u64> = row.get(2)?;
        if actual_ordinal != ordinal as u64
            || actual_id != part.chunk_id().as_str()
            || actual_bytes != Some(part.byte_length())
        {
            return Err(invalid("shared manifest chunk index mismatch"));
        }
    }
    if rows.next()?.is_some() {
        return Err(invalid("shared manifest chunk index has extra rows"));
    }
    Ok(Some(value))
}

fn verify_input(input: &mut dyn Read, manifest: &FileManifest) -> Result<()> {
    let mut verifier = manifest.verifier();
    for part in manifest.chunks() {
        let mut bytes = vec![0; part.byte_length() as usize];
        input.read_exact(&mut bytes)?;
        verifier.push(&bytes).map_err(invalid)?;
    }
    if input.read(&mut [0u8; 1])? != 0 {
        return Err(invalid("shared source grew"));
    }
    verifier.finish().map_err(invalid)?;
    Ok(())
}
fn object_name(id: &str) -> Result<String> {
    let name = id
        .strip_prefix("blake3:")
        .ok_or_else(|| invalid("invalid content key"))?;
    if name.len() != 64
        || !name
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(invalid("invalid content filename"));
    }
    Ok(name.to_owned())
}
fn read_chunk_at(directory: &PinnedDirectory, id: &ChunkId, length: u64) -> Result<Chunk> {
    if length == 0 || length > CAPTURE_CHUNK_BYTES as u64 {
        return Err(invalid("shared chunk size out of range"));
    }
    let name = object_name(id.as_str())?;
    let pin = directory.open_file(name.as_ref())?;
    if pin.file().metadata()?.len() != length {
        return Err(invalid("shared chunk truncated"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    pin.file().take(length + 1).read_to_end(&mut bytes)?;
    let chunk = Chunk::new(bytes).map_err(invalid)?;
    if chunk.id() != id || chunk.byte_length() != length {
        return Err(invalid("shared chunk digest mismatch"));
    }
    pin.verify_parent()?;
    Ok(chunk)
}
fn decode_manifest(bytes: &[u8]) -> Result<FileManifest> {
    let manifest: FileManifest = serde_json::from_slice(bytes).map_err(invalid)?;
    if canonical_json(&manifest).map_err(invalid)? != bytes
        || manifest.byte_length() > MAX_CAPTURE_FILE_BYTES
        || manifest.chunks().len() > 1024
    {
        return Err(invalid("shared manifest invalid"));
    }
    Ok(manifest)
}

#[cfg(all(test, windows))]
#[path = "content/exact_tests.rs"]
mod exact_tests;
