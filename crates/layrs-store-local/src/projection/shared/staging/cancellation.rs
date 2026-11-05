//! Admission-scoped cancellation before a restore journal is accepted. The
//! first pass is entirely read-only; the second repeats that complete pass
//! before publishing or removing any installation.
use super::*;
use layrs_core::scm::Chunk;
use std::{collections::BTreeSet, ffi::OsStr};

impl Lease {
    pub(in crate::projection) fn preflight_restore_cancellation(
        &self,
        marker: &format::Marker,
        reservation: &str,
        operation: &str,
        bytes: u64,
        committed: bool,
    ) -> Result<()> {
        self.cancellation_stages(marker, reservation, operation, bytes, committed, false)?;
        Ok(())
    }

    #[cfg(windows)]
    pub(in crate::projection) fn preflight_capture_cancellation(
        &self,
        marker: &format::Marker,
        reservation: &str,
        capture: &crate::projection::retention::LocalCaptureCancellation,
        bytes: u64,
        committed: bool,
    ) -> Result<()> {
        self.cancellation_stages(
            marker,
            reservation,
            capture.operation(),
            bytes,
            committed,
            true,
        )?;
        Ok(())
    }

    pub(in crate::projection) fn cancel_restore_installations(
        &self,
        marker: &format::Marker,
        reservation: &str,
        operation: &str,
        bytes: u64,
        committed: bool,
    ) -> Result<()> {
        let stages =
            self.cancellation_stages(marker, reservation, operation, bytes, committed, false)?;
        self.finish_cancellation_stages(stages)
    }

    #[cfg(windows)]
    pub(in crate::projection) fn cancel_capture_installations(
        &self,
        marker: &format::Marker,
        reservation: &str,
        capture: &crate::projection::retention::LocalCaptureCancellation,
        bytes: u64,
        committed: bool,
    ) -> Result<()> {
        let stages = self.cancellation_stages(
            marker,
            reservation,
            capture.operation(),
            bytes,
            committed,
            true,
        )?;
        self.finish_cancellation_stages(stages)
    }

    fn finish_cancellation_stages(&self, stages: Vec<Stage>) -> Result<()> {
        for mut stage in stages {
            if let Some(manifest) = &stage.manifest {
                // Never choose another representation by FileId. A catalog
                // commit may already have succeeded before staging cleanup.
                if self.manifest_optional(manifest.id())?.as_ref() == Some(manifest) {
                    self.verify_manifest(manifest)?;
                    stage.published = true;
                    stage.finish(self)?;
                } else {
                    self.publish_installation(stage, &mut |_| Ok(()))?;
                }
            } else {
                stage.finish(self)?;
            }
        }
        Ok(())
    }

    fn cancellation_stages(
        &self,
        marker: &format::Marker,
        reservation: &str,
        operation: &str,
        bytes: u64,
        committed: bool,
        capture: bool,
    ) -> Result<Vec<Stage>> {
        let operation_matches = if capture {
            operation.strip_prefix("capture:").is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        } else {
            operation.strip_prefix("restore:").is_some_and(|id| {
                !id.is_empty()
                    && id.len() <= 96
                    && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            })
        };
        if !self.is_admitted()
            || reservation.is_empty()
            || reservation.len() > 128
            || reservation.contains('\0')
            || !operation_matches
            || !(65536..=i64::MAX as u64).contains(&bytes)
        {
            return Err(invalid(
                "restore cancellation requires bounded admitted ownership",
            ));
        }
        self.authorize_content_protection(marker, operation)?;
        let binding = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("restore family missing"))?;
        let (stages, remaining) = self.with_content_catalog(false, |c| {
            self.authorize_content_reservation(c, reservation)?;
            let payload = marker.bytes()?;
            let path = binding.metadata_path.to_string_lossy();
            let valid: bool = c.query_row(
                "SELECT typeof(metadata_path)='text' AND length(CAST(metadata_path AS BLOB))=?2 AND typeof(database_identity)='text' AND length(CAST(database_identity AS BLOB))=?3 AND typeof(marker)='blob' AND length(marker)=?4 AND state='ready' FROM worktrees WHERE id=?1",
                params![marker.identity.worktree_id.as_str(), path.len(), binding.database_identity.len(), payload.len()],
                |r| r.get(0),
            ).optional()?.unwrap_or(false);
            if !valid {
                return Err(invalid("restore cancellation registration missing or exceeds bounds"));
            }
            let (actual_path, database, actual_marker): (String,String,Vec<u8>) = c.query_row(
                "SELECT metadata_path,database_identity,marker FROM worktrees WHERE id=?1",
                [marker.identity.worktree_id.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            )?;
            if actual_path != path.as_ref() || database != binding.database_identity || actual_marker != payload {
                return Err(invalid("restore cancellation ready registration differs"));
            }
            let bounded: Option<bool> = c.query_row(
                "SELECT typeof(owner)='text' AND length(CAST(owner AS BLOB)) BETWEEN 1 AND 512 AND typeof(operation)='text' AND length(CAST(operation AS BLOB)) BETWEEN 1 AND 128 AND typeof(remaining)='integer' AND remaining>=0 FROM reservations WHERE id=?1",
                [reservation], |r| r.get(0),
            ).optional()?;
            let remaining = match bounded {
                Some(true) => {
                    let (owner, actual_operation, remaining): (String,String,u64) = c.query_row(
                        "SELECT owner,operation,remaining FROM reservations WHERE id=?1", [reservation],
                        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
                    )?;
                    if owner != marker.identity.worktree_id.as_str() || actual_operation != operation || remaining > bytes {
                        return Err(invalid("restore cancellation reservation identity or charge differs"));
                    }
                    Some(remaining)
                }
                None if committed => None,
                _ => return Err(invalid("restore cancellation reservation missing or exceeds bounds")),
            };
            let (count, aggregate, malformed): (usize,u64,bool) = c.query_row(
                "SELECT count(*),coalesce(sum(length(payload)+coalesce(length(manifest),0)),0),coalesce(max(typeof(file_id)!='text' OR length(CAST(file_id AS BLOB))!=71 OR typeof(payload)!='blob' OR length(payload) NOT BETWEEN 1 AND 8192 OR (manifest IS NOT NULL AND (typeof(manifest)!='blob' OR length(manifest) NOT BETWEEN 1 AND 262144)) OR typeof(published)!='integer' OR published NOT IN (0,1)),0) FROM (SELECT file_id,payload,manifest,published FROM installations WHERE reservation=?1 LIMIT 1025)",
                [reservation], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            )?;
            if count > 1024 || aggregate > 64 * 1024 * 1024 || malformed || (count != 0 && (committed || remaining.is_none())) {
                return Err(invalid("restore cancellation installation inventory differs or exceeds bounds"));
            }
            let mut query = c.prepare("SELECT file_id FROM installations WHERE reservation=?1 ORDER BY file_id LIMIT 1025")?;
            let mut rows = query.query([reservation])?;
            let mut stages = Vec::with_capacity(count);
            let mut directories = BTreeSet::new();
            let mut identities = BTreeSet::new();
            while let Some(row) = rows.next()? {
                let file = FileId::parse(row.get::<_,String>(0)?).map_err(invalid)?;
                let stage = load(c, reservation, &file)?.ok_or_else(|| invalid("restore installation disappeared"))?;
                let expected = CapturedFile { file_id: file, bytes: stage.intent.bytes, executable: false };
                stage.validate_identity(self, reservation, &expected, marker.identity.worktree_id.as_str(), operation, &binding.database_identity)?;
                if !bounded_directory(&stage.intent.directory)
                    || stage.intent.directory_identity.is_empty()
                    || stage.intent.directory_identity.len() > 512
                    || stage.intent.bytes > bytes
                    || !directories.insert(stage.intent.directory.clone())
                    || !identities.insert(stage.intent.directory_identity.clone())
                {
                    return Err(invalid("restore installation directory or source bound differs"));
                }
                self.authorize_content_directory(c, &stage.intent.directory, &stage.intent.directory_identity)?;
                stages.push(stage);
            }
            require_exclusive_installation_directories(c, &stages)?;
            Ok((stages,remaining.unwrap_or(0)))
        })?;
        let mut chunks = BTreeSet::new();
        let mut manifests = BTreeSet::new();
        let mut cost = 0u64;
        for stage in &stages {
            self.validate_cancellation_bytes(stage)?;
            if let Some(manifest) = &stage.manifest {
                self.with_content_catalog(false, |c| {
                    if catalog_schema_version(c)? == CatalogSchema::V2 {
                        let stored: Option<u64> = c
                            .query_row(
                                "SELECT bytes FROM files WHERE file_id=?1",
                                [manifest.file_id().as_str()],
                                |r| r.get(0),
                            )
                            .optional()?;
                        if stored.is_some_and(|n| n != manifest.byte_length()) {
                            return Err(invalid(
                                "restore installation FileId catalog length differs",
                            ));
                        }
                    }
                    for part in manifest.chunks() {
                        let stored: Option<u64> = c
                            .query_row(
                                "SELECT bytes FROM chunks WHERE id=?1",
                                [part.chunk_id().as_str()],
                                |r| r.get(0),
                            )
                            .optional()?;
                        if stored.is_some_and(|n| n != part.byte_length()) {
                            return Err(invalid(
                                "restore installation chunk catalog length differs",
                            ));
                        }
                        if stored.is_none() && chunks.insert(part.chunk_id().clone()) {
                            cost = cost
                                .checked_add(part.byte_length())
                                .ok_or_else(|| invalid("restore installation cost overflow"))?;
                        }
                    }
                    let exists: bool = c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM manifests WHERE id=?1)",
                        [manifest.id().as_str()],
                        |r| r.get(0),
                    )?;
                    if !exists && manifests.insert(manifest.id().clone()) {
                        cost = cost
                            .checked_add(canonical_json(manifest).map_err(invalid)?.len() as u64)
                            .ok_or_else(|| invalid("restore installation cost overflow"))?;
                    }
                    Ok(())
                })?;
            }
        }
        if cost > remaining {
            return Err(invalid(
                "restore installations exceed remaining reservation",
            ));
        }
        self.verify()?;
        Ok(stages)
    }

    fn validate_cancellation_bytes(&self, stage: &Stage) -> Result<()> {
        let resident = match &stage.manifest {
            Some(manifest) => match self.manifest_optional(manifest.id())? {
                Some(actual) if actual == *manifest => {
                    self.verify_manifest(manifest)?;
                    true
                }
                Some(_) => {
                    return Err(invalid("restore installation exact representation differs"));
                }
                None if stage.published => {
                    return Err(invalid(
                        "published restore installation lacks exact catalog representation",
                    ));
                }
                None => false,
            },
            None => false,
        };
        let parent =
            PinnedDirectory::open(&self.family.root)?.open_private_child("staging".as_ref())?;
        let directory = match parent.open_private_child(stage.intent.directory.as_ref()) {
            Ok(directory) => {
                if directory.identity_token() != stage.intent.directory_identity {
                    return Err(invalid("restore installation directory replaced"));
                }
                self.with_content_catalog(false, |c| {
                    self.authorize_content_directory(
                        c,
                        &stage.intent.directory,
                        &directory.identity_token(),
                    )
                })?;
                let names = stage.inventory(self, &directory)?;
                let mut staged_bytes = 0u64;
                for name in names {
                    let pin = directory.open_readonly_file(&name)?;
                    let length = pin.file().metadata()?.len();
                    staged_bytes = staged_bytes
                        .checked_add(length)
                        .ok_or_else(|| invalid("restore staged byte count overflow"))?;
                    if let Some(index) = name
                        .to_str()
                        .and_then(|n| n.strip_prefix("chunk-"))
                        .and_then(|n| n.parse::<usize>().ok())
                    {
                        let limit = match &stage.manifest {
                            Some(manifest) => manifest.chunks().get(index).map(|p| p.byte_length()),
                            None => stage
                                .intent
                                .bytes
                                .checked_sub(index as u64 * CAPTURE_CHUNK_BYTES as u64)
                                .filter(|n| *n > 0)
                                .map(|n| n.min(CAPTURE_CHUNK_BYTES as u64)),
                        };
                        if limit.is_none_or(|limit| length > limit) {
                            return Err(invalid(
                                "restore installation has a chunk outside its source bounds",
                            ));
                        }
                    }
                    pin.verify_parent()?;
                }
                if staged_bytes > stage.intent.bytes + 262144 {
                    return Err(invalid(
                        "restore installation staged bytes exceed source bound",
                    ));
                }
                Some(directory)
            }
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && (stage.manifest.is_none() || resident) =>
            {
                None
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(manifest) = &stage.manifest {
            let objects = PinnedDirectory::open(&self.family.root.join("objects"))?;
            let chunks = objects.open_child_for_traversal("chunks".as_ref())?;
            let manifests = objects.open_child_for_traversal("manifests".as_ref())?;
            let expected = canonical_json(manifest).map_err(invalid)?;
            let staged = directory
                .as_ref()
                .map(|d| read_piece(d, "manifest".as_ref(), 262144))
                .transpose()?
                .flatten();
            let stored = read_piece(
                &manifests,
                object_name(manifest.id().as_str())?.as_ref(),
                262144,
            )?;
            if staged.is_none() && stored.is_none()
                || staged.as_ref().is_some_and(|b| b != &expected)
                || stored.as_ref().is_some_and(|b| b != &expected)
            {
                return Err(invalid(
                    "restore installation exact manifest bytes differ or are absent",
                ));
            }
            let mut verifier = manifest.verifier();
            for (ordinal, part) in manifest.chunks().iter().enumerate() {
                let staged = directory
                    .as_ref()
                    .map(|d| {
                        read_piece(
                            d,
                            format!("chunk-{ordinal}").as_ref(),
                            CAPTURE_CHUNK_BYTES as u64,
                        )
                    })
                    .transpose()?
                    .flatten();
                let stored = read_piece(
                    &chunks,
                    object_name(part.chunk_id().as_str())?.as_ref(),
                    CAPTURE_CHUNK_BYTES as u64,
                )?;
                for bytes in staged.iter().chain(stored.iter()) {
                    let chunk = Chunk::new(bytes.clone()).map_err(invalid)?;
                    if chunk.id() != part.chunk_id() || chunk.byte_length() != part.byte_length() {
                        return Err(invalid("restore installation chunk bytes differ"));
                    }
                }
                let bytes = staged
                    .as_ref()
                    .or(stored.as_ref())
                    .ok_or_else(|| invalid("restore installation chunk bytes absent"))?;
                verifier.push(bytes).map_err(invalid)?;
            }
            verifier.finish().map_err(invalid)?;
            chunks.verify_path_identity()?;
            manifests.verify_path_identity()?;
        }
        if let Some(directory) = directory {
            directory.verify_path_identity()?;
        }
        parent.verify_path_identity()?;
        Ok(())
    }
}

/// Catalog corruption must not let this reservation clean another ordinary
/// installation's directory. Exact-import aliases are checked separately.
pub(super) fn require_exclusive_installation_directories(
    c: &Connection,
    stages: &[Stage],
) -> Result<()> {
    if stages.is_empty() {
        return Ok(());
    }
    let directories = stages
        .iter()
        .map(|s| {
            (
                s.intent.directory.as_str(),
                (s.intent.reservation.as_str(), s.intent.file_id.as_str()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let identities = stages
        .iter()
        .map(|s| {
            (
                s.intent.directory_identity.as_str(),
                (s.intent.reservation.as_str(), s.intent.file_id.as_str()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let (count, bytes, malformed): (u64, u64, bool) = c.query_row(
        "SELECT count(*),coalesce(sum(length(payload)),0),coalesce(max(typeof(reservation)!='text' OR length(CAST(reservation AS BLOB)) NOT BETWEEN 1 AND 128 OR typeof(file_id)!='text' OR length(CAST(file_id AS BLOB))!=71 OR typeof(payload)!='blob' OR length(payload) NOT BETWEEN 1 AND 8192),0) FROM (SELECT reservation,file_id,payload FROM installations LIMIT 100001)",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if count > 100000 || bytes > 64 * 1024 * 1024 || malformed {
        return Err(invalid(
            "ordinary installation alias inventory exceeds bounds",
        ));
    }
    let mut query =
        c.prepare("SELECT reservation,file_id,payload FROM installations LIMIT 100001")?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let reservation: String = row.get(0)?;
        let file: String = row.get(1)?;
        let payload: Vec<u8> = row.get(2)?;
        let intent: Intent = serde_json::from_slice(&payload).map_err(invalid)?;
        if canonical_json(&intent).map_err(invalid)? != payload
            || intent.reservation != reservation
            || intent.file_id.as_str() != file
            || !bounded_directory(&intent.directory)
            || intent.directory_identity.is_empty()
            || intent.directory_identity.len() > 512
        {
            return Err(invalid(
                "ordinary installation alias inventory is not canonical",
            ));
        }
        let actual = (reservation.as_str(), file.as_str());
        if directories
            .get(intent.directory.as_str())
            .is_some_and(|owner| *owner != actual)
            || identities
                .get(intent.directory_identity.as_str())
                .is_some_and(|owner| *owner != actual)
        {
            return Err(invalid(
                "ordinary staging alias belongs to another installation",
            ));
        }
    }
    Ok(())
}

fn bounded_directory(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

fn object_name(id: &str) -> Result<&str> {
    id.strip_prefix("blake3:")
        .filter(|name| {
            name.len() == 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(|| invalid("restore installation content key differs"))
}

fn read_piece(directory: &PinnedDirectory, name: &OsStr, limit: u64) -> Result<Option<Vec<u8>>> {
    let pin = match directory.open_readonly_file(name) {
        Ok(pin) => pin,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    crate::projection::file::require_single_link(pin.file())?;
    if pin.file().metadata()?.len() > limit {
        return Err(invalid("restore installation piece exceeds bound"));
    }
    let mut bytes = Vec::new();
    pin.file().take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("restore installation piece grew beyond bound"));
    }
    pin.verify_parent()?;
    crate::projection::file::require_single_link(pin.file())?;
    if !pin.same_file(&directory.open_readonly_file(name)?) {
        return Err(invalid("restore installation piece replaced"));
    }
    directory.verify_path_identity()?;
    Ok(Some(bytes))
}
