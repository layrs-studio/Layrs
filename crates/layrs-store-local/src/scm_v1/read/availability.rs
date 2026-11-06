//! Typed closure and bounded File availability reads.
use super::*;
impl NativeScmReader<'_> {
    pub(super) fn object_exists_typed(&self, id: &StoreObjectId) -> Result<bool> {
        match id {
            // A verified object row: bytes were checked on installation and
            // are checked again at every read (`chunk_present_metadata`).
            StoreObjectId::ChunkRaw(id) => {
                <Self as layrs_store::scm::ScmReader>::chunk_present_metadata(self, id)
            }
            StoreObjectId::FileManifest(id) => {
                self.get_file_manifest(id).map(|value| value.is_some())
            }
            StoreObjectId::Tree(id) => self.get_tree(id).map(|value| value.is_some()),
            StoreObjectId::Symlink(id) => self.get_symlink(id).map(|value| value.is_some()),
            StoreObjectId::Tombstone(id) => self.get_tombstone(id).map(|value| value.is_some()),
            StoreObjectId::Step(id) => self.get_step(id).map(|value| value.is_some()),
            StoreObjectId::RepositoryView(id) => {
                self.get_repository_view(id).map(|value| value.is_some())
            }
            StoreObjectId::Conflict(id) => self.get_conflict(id).map(|value| value.is_some()),
            StoreObjectId::Product(id) => self.get_product(id).map(|value| value.is_some()),
        }
    }

    /// Reads only the bounded metadata row needed to account for a Chunk
    /// before its CAS payload can be opened or allocated.
    pub(super) fn preflight_chunk_bytes(&self, id: &ChunkId) -> Result<usize> {
        if self.compact() {
            return self.preflight_chunk_bytes_compact(id);
        }
        let object_id = StoreObjectId::ChunkRaw(id.clone());
        let connection = self.connection()?;
        let row: Option<(String, String, i64, i64)> = connection
            .query_row(
                "SELECT object_kind, storage_path, byte_length, verified \
                 FROM objects WHERE object_id = ?1",
                [object_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sqlite_backend)?;
        drop(connection);
        let Some((kind, storage_path, byte_length, verified)) = row else {
            return Err(corruption(format!(
                "FileManifest references Chunk {id} without an objects row"
            )));
        };
        if kind != kind_name(StoreObjectKind::ChunkRaw) || verified != 1 {
            return Err(corruption(format!(
                "objects row for Chunk {id} has the wrong kind or verification state"
            )));
        }
        let expected_path = self.cas.object_path(&object_id).map_err(cas_read_error)?;
        let expected_relative = expected_path
            .strip_prefix(self.root())
            .map_err(|_| corruption(format!("CAS path for Chunk {id} escapes Store root")))?;
        if normalize_storage_path(expected_relative) != storage_path.replace('\\', "/") {
            return Err(corruption(format!(
                "objects.storage_path for Chunk {id} does not match its ID-derived CAS path"
            )));
        }
        positive_usize(byte_length, "Chunk", id)
    }

    pub(super) fn require_step(&self, id: &StepId, context: &str) -> Result<Step> {
        self.get_step(id)?.ok_or_else(|| {
            corruption(format!(
                "{context} references missing or unverified Step {id}"
            ))
        })
    }

    pub(super) fn step_reaches(&self, head: &StepId, target: &StepId) -> Result<bool> {
        let mut pending = vec![head.clone()];
        let mut visited = BTreeSet::new();
        while let Some(candidate) = pending.pop() {
            if !visited.insert(candidate.clone()) {
                continue;
            }
            let step = self.require_step(&candidate, "Step DAG")?;
            if &candidate == target {
                return Ok(true);
            }
            pending.extend(step.parent_step_ids().iter().cloned());
        }
        Ok(false)
    }

    pub(super) fn exists(&self, sql: &str, first: &str, second: Option<&str>) -> Result<bool> {
        let connection = self.connection()?;
        let present = match second {
            Some(second) => connection
                .query_row(sql, params![first, second], |_| Ok(()))
                .optional(),
            None => connection.query_row(sql, [first], |_| Ok(())).optional(),
        }
        .map_err(sqlite_backend)?
        .is_some();
        Ok(present)
    }

    /// Byte length of a File recorded `present`, from its validated
    /// FileManifest only. Chunk bytes are NOT read or re-hashed: callers that
    /// later consume content verify it at use (e.g. `VerifiedWriter` during
    /// materialization). Use for checkout projections and diffs, never as
    /// proof that content is intact. `None` when the File is not present.
    pub fn file_length_metadata(&self, id: &FileId) -> Result<Option<u64>> {
        let _span = crate::perf::span("read.file_length_metadata");
        if self.compact() {
            return self.file_length_metadata_compact(id);
        }
        let connection = self.connection()?;
        let state: Option<String> = connection
            .prepare_cached("SELECT availability FROM file_availability WHERE file_id = ?1")
            .map_err(sqlite_backend)?
            .query_row([id.as_str()], |row| row.get(0))
            .optional()
            .map_err(sqlite_backend)?;
        let Some(state) = state else {
            return Ok(None);
        };
        if state != "present" {
            return Err(corruption(format!(
                "file_availability for {id} has invalid state {state}"
            )));
        }
        // `file_byte_length` is written from the validated FileManifest in the
        // same admission transaction that records it (repository.rs); one
        // indexed query instead of reading the manifest object from disk.
        // Every manifest of a File records the same validated length; a
        // manifest without its objects row is corruption, as in the full read.
        let (length, orphaned): (Option<i64>, i64) = connection
            .prepare_cached(
                "SELECT min(fm.file_byte_length), \
                        coalesce(sum(o.object_id IS NULL), 0) \
                 FROM file_manifests AS fm \
                 LEFT JOIN objects AS o ON o.object_id = fm.manifest_id \
                 WHERE fm.file_id = ?1",
            )
            .map_err(sqlite_backend)?
            .query_row([id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sqlite_backend)?;
        if orphaned != 0 {
            return Err(corruption(format!(
                "file {id} references a manifest without an objects row"
            )));
        }
        let length = length
            .ok_or_else(|| corruption(format!("present file {id} has no verified manifest")))?;
        Ok(Some(u64::try_from(length).map_err(|_| {
            corruption(format!("file {id} has a negative recorded byte length"))
        })?))
    }

    /// `file_length_metadata` for many Files at once: the same checks, in
    /// batches of FileIds per query instead of two queries per File. Absent
    /// Files are missing from the map.
    pub fn file_lengths_metadata<'f>(
        &self,
        files: impl IntoIterator<Item = &'f FileId>,
    ) -> Result<std::collections::BTreeMap<FileId, u64>> {
        let _span = crate::perf::span("read.file_lengths_metadata");
        if self.compact() {
            return self.file_lengths_metadata_compact(files);
        }
        const BATCH: usize = 256;
        let connection = self.connection()?;
        let files: std::collections::BTreeSet<&FileId> = files.into_iter().collect();
        let files: Vec<&FileId> = files.into_iter().collect();
        let mut lengths = std::collections::BTreeMap::new();
        for batch in files.chunks(BATCH) {
            let placeholders = (0..batch.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = connection
                .prepare_cached(&format!(
                    "SELECT fa.file_id, fa.availability, min(fm.file_byte_length),                             coalesce(sum(fm.manifest_id IS NOT NULL AND o.object_id IS NULL), 0)                      FROM file_availability AS fa                      LEFT JOIN file_manifests AS fm ON fm.file_id = fa.file_id                      LEFT JOIN objects AS o ON o.object_id = fm.manifest_id                      WHERE fa.file_id IN ({placeholders}) GROUP BY fa.file_id"
                ))
                .map_err(sqlite_backend)?;
            let names: Vec<&str> = batch.iter().map(|file| file.as_str()).collect();
            let parameters: Vec<&dyn rusqlite::ToSql> = names
                .iter()
                .map(|name| name as &dyn rusqlite::ToSql)
                .collect();
            let mut rows = statement
                .query(parameters.as_slice())
                .map_err(sqlite_backend)?;
            while let Some(row) = rows.next().map_err(sqlite_backend)? {
                let id: String = row.get(0).map_err(sqlite_backend)?;
                let state: String = row.get(1).map_err(sqlite_backend)?;
                let length: Option<i64> = row.get(2).map_err(sqlite_backend)?;
                let orphaned: i64 = row.get(3).map_err(sqlite_backend)?;
                if state != "present" {
                    return Err(corruption(format!(
                        "file_availability for {id} has invalid state {state}"
                    )));
                }
                if orphaned != 0 {
                    return Err(corruption(format!(
                        "file {id} references a manifest without an objects row"
                    )));
                }
                let length = length.ok_or_else(|| {
                    corruption(format!("present file {id} has no verified manifest"))
                })?;
                let length = u64::try_from(length).map_err(|_| {
                    corruption(format!("file {id} has a negative recorded byte length"))
                })?;
                lengths.insert(FileId::parse(id).map_err(core_corruption)?, length);
            }
        }
        Ok(lengths)
    }

    /// Batched `ScmReader::admitted_manifests_metadata`: only Files whose
    /// metadata is entirely sound (present, every manifest with its objects
    /// row, at most 1,024 manifests) are answered; any other is left to the
    /// per-File path, which reports its corruption exactly as before.
    pub fn admitted_manifests_metadata<'f>(
        &self,
        files: impl IntoIterator<Item = &'f FileId>,
        space: &layrs_core::scm::SpaceId,
    ) -> Result<std::collections::BTreeMap<FileId, FileManifestId>> {
        let _span = crate::perf::span("read.admitted_manifests_metadata");
        if self.compact() {
            return self.admitted_manifests_metadata_compact(files, space);
        }
        const BATCH: usize = 256;
        let connection = self.connection()?;
        let files: std::collections::BTreeSet<&FileId> = files.into_iter().collect();
        let files: Vec<&FileId> = files.into_iter().collect();
        let mut admitted = std::collections::BTreeMap::new();
        for batch in files.chunks(BATCH) {
            let placeholders = (0..batch.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = connection
                .prepare_cached(&format!(
                    "SELECT fm.file_id,                             min(CASE WHEN a.object_id IS NOT NULL AND o.object_id IS NOT NULL                                      THEN fm.manifest_id END),                             coalesce(sum(o.object_id IS NULL), 0),                             coalesce(sum(fa.availability <> 'present'), 0),                             count(*)                      FROM file_manifests AS fm                      JOIN file_availability AS fa ON fa.file_id = fm.file_id                      LEFT JOIN objects AS o ON o.object_id = fm.manifest_id                      LEFT JOIN space_object_admissions AS a                        ON a.object_id = fm.manifest_id AND a.space_id = ?1                      WHERE fm.file_id IN ({placeholders}) GROUP BY fm.file_id"
                ))
                .map_err(sqlite_backend)?;
            let space = space.as_str();
            let names: Vec<&str> = batch.iter().map(|file| file.as_str()).collect();
            let mut parameters: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(batch.len() + 1);
            parameters.push(&space);
            for name in &names {
                parameters.push(name);
            }
            let mut rows = statement
                .query(parameters.as_slice())
                .map_err(sqlite_backend)?;
            while let Some(row) = rows.next().map_err(sqlite_backend)? {
                let file: String = row.get(0).map_err(sqlite_backend)?;
                let selected: Option<String> = row.get(1).map_err(sqlite_backend)?;
                let orphaned: i64 = row.get(2).map_err(sqlite_backend)?;
                let absent: i64 = row.get(3).map_err(sqlite_backend)?;
                let manifests: i64 = row.get(4).map_err(sqlite_backend)?;
                let (Some(selected), 0, 0, 1..=1024) = (selected, orphaned, absent, manifests)
                else {
                    continue;
                };
                admitted.insert(
                    FileId::parse(file).map_err(core_corruption)?,
                    FileManifestId::parse(selected).map_err(core_corruption)?,
                );
            }
        }
        Ok(admitted)
    }

    /// ChunkRaw ids of `files` (normally the capture base) already admitted
    /// to `space`, or `None` above `limit`. Lets a capture spool and carry only
    /// chunks this Space lacks; admitted chunks are durable, verified on
    /// installation and closed. Memory scales with the base checkout, never
    /// with the Space history.
    pub fn admitted_chunk_ids_of_files<'f>(
        &self,
        space: &layrs_core::scm::SpaceId,
        files: impl IntoIterator<Item = &'f FileId>,
        limit: usize,
    ) -> Result<Option<std::collections::BTreeSet<layrs_core::scm::ChunkId>>> {
        let _span = crate::perf::span("read.admitted_chunk_ids_of_files");
        if self.compact() {
            return self.admitted_chunk_ids_of_files_compact(space, files, limit);
        }
        // Batches of FileIds per query (bounded below SQLite's parameter limit).
        const BATCH: usize = 256;
        let connection = self.connection()?;
        let files: std::collections::BTreeSet<&FileId> = files.into_iter().collect();
        let files: Vec<&FileId> = files.into_iter().collect();
        let mut ids = std::collections::BTreeSet::new();
        for batch in files.chunks(BATCH) {
            let placeholders = (0..batch.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = connection
                .prepare_cached(&format!(
                    "SELECT DISTINCT fmc.chunk_id FROM file_manifests AS fm \
                     JOIN file_manifest_chunks AS fmc ON fmc.manifest_id = fm.manifest_id \
                     JOIN space_object_admissions AS a \
                       ON a.object_id = fmc.chunk_id AND a.space_id = ?1 \
                     JOIN objects AS o ON o.object_id = fmc.chunk_id \
                     WHERE fm.file_id IN ({placeholders}) AND o.object_kind = 'chunk_raw'"
                ))
                .map_err(sqlite_backend)?;
            let mut parameters: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(batch.len() + 1);
            let space = space.as_str();
            parameters.push(&space);
            let names: Vec<&str> = batch.iter().map(|file| file.as_str()).collect();
            for name in &names {
                parameters.push(name);
            }
            let mut rows = statement
                .query(parameters.as_slice())
                .map_err(sqlite_backend)?;
            while let Some(row) = rows.next().map_err(sqlite_backend)? {
                let id: String = row.get(0).map_err(sqlite_backend)?;
                ids.insert(layrs_core::scm::ChunkId::parse(id).map_err(core_corruption)?);
                if ids.len() > limit {
                    return Ok(None);
                }
            }
        }
        Ok(Some(ids))
    }

    /// Manifest ids of a File recorded `present`, from metadata only (no
    /// manifest object read, no chunk re-hash). Empty when not present.
    pub fn file_manifest_ids_metadata(&self, id: &FileId) -> Result<Vec<FileManifestId>> {
        if self.compact() {
            return self.file_manifest_ids_metadata_compact(id);
        }
        let connection = self.connection()?;
        let state: Option<String> = connection
            .prepare_cached("SELECT availability FROM file_availability WHERE file_id = ?1")
            .map_err(sqlite_backend)?
            .query_row([id.as_str()], |row| row.get(0))
            .optional()
            .map_err(sqlite_backend)?;
        match state.as_deref() {
            None => return Ok(Vec::new()),
            Some("present") => {}
            Some(other) => {
                return Err(corruption(format!(
                    "file_availability for {id} has invalid state {other}"
                )));
            }
        }
        // Same row set and corruption checks as `file_availability_with_budget`
        // (LEFT JOIN: a manifest without its objects row is corruption, not
        // silently skipped), bounded like the full read's 1024-manifest limit.
        let mut statement = connection
            .prepare_cached(
                "SELECT fm.manifest_id, o.object_id IS NOT NULL FROM file_manifests AS fm \
                 LEFT JOIN objects AS o ON o.object_id = fm.manifest_id \
                 WHERE fm.file_id = ?1 ORDER BY fm.manifest_id LIMIT 1025",
            )
            .map_err(sqlite_backend)?;
        let rows = statement
            .query_map([id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
            })
            .map_err(sqlite_backend)?;
        let mut ids = Vec::new();
        for row in rows {
            let (manifest, stored) = row.map_err(sqlite_backend)?;
            let manifest = FileManifestId::parse(manifest).map_err(core_corruption)?;
            if !stored {
                return Err(corruption(format!(
                    "file {id} references manifest {manifest} without an objects row"
                )));
            }
            ids.push(manifest);
        }
        if ids.is_empty() {
            return Err(corruption(format!(
                "present file {id} has no verified manifest"
            )));
        }
        if ids.len() > 1024 {
            return Err(corruption(format!(
                "present file {id} exceeds the 1024 verified manifest bound"
            )));
        }
        Ok(ids)
    }

    /// Resolves and verifies File availability under explicit deterministic
    /// work limits. Limits are charged before every manifest/chunk CAS read,
    /// so an adversarial FileId cannot cause an unbounded pre-admission scan.
    pub fn file_availability_with_budget(
        &self,
        id: &FileId,
        max_rows: usize,
        max_bytes: usize,
        max_manifests: usize,
        max_chunks: usize,
    ) -> Result<FileAvailability> {
        let _span = crate::perf::span("read.file_availability");
        let limits = FileAvailabilityLimits {
            rows: max_rows,
            bytes: max_bytes,
            manifests: max_manifests,
            chunks: max_chunks,
        };
        let mut consumed = FileAvailabilityConsumption::default();
        consumed.charge_rows(1, limits.rows)?;

        let manifest_rows = if self.compact() {
            match self.file_availability_rows_compact(id, limits, &mut consumed)? {
                Some(rows) => rows,
                None => return Ok(FileAvailability::Absent),
            }
        } else {
            match self.file_availability_rows(id, limits, &mut consumed)? {
                Some(rows) => rows,
                None => return Ok(FileAvailability::Absent),
            }
        };
        self.verify_file_availability(id, limits, &mut consumed, manifest_rows)
    }

    /// The manifest rows of a File recorded `present`, charged; `None` when
    /// the File is absent.
    fn file_availability_rows(
        &self,
        id: &FileId,
        limits: FileAvailabilityLimits,
        consumed: &mut FileAvailabilityConsumption,
    ) -> Result<Option<Vec<FileManifestId>>> {
        let connection = self.connection()?;
        let state: Option<String> = connection
            .query_row(
                "SELECT availability FROM file_availability WHERE file_id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_backend)?;
        let Some(state) = state else {
            return Ok(None);
        };
        if state != "present" {
            return Err(corruption(format!(
                "file_availability for {id} has invalid state {state}"
            )));
        }

        let mut statement = connection
            .prepare(
                "SELECT fm.manifest_id, o.byte_length \
                 FROM file_manifests AS fm \
                 LEFT JOIN objects AS o ON o.object_id = fm.manifest_id \
                 WHERE fm.file_id = ?1 ORDER BY fm.manifest_id",
            )
            .map_err(sqlite_backend)?;
        let mut rows = statement.query([id.as_str()]).map_err(sqlite_backend)?;
        let mut manifest_rows = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite_backend)? {
            consumed.charge_rows(1, limits.rows)?;
            consumed.charge_manifests(1, limits.manifests)?;
            let manifest_id =
                FileManifestId::parse(row.get::<_, String>(0).map_err(sqlite_backend)?)
                    .map_err(core_corruption)?;
            let stored_bytes: Option<i64> = row.get(1).map_err(sqlite_backend)?;
            let stored_bytes = stored_bytes.ok_or_else(|| {
                corruption(format!(
                    "file {id} references manifest {manifest_id} without an objects row"
                ))
            })?;
            consumed.charge_bytes(
                positive_usize(stored_bytes, "manifest", &manifest_id)?,
                limits.bytes,
            )?;
            manifest_rows.push(manifest_id);
        }
        drop(rows);
        drop(statement);
        drop(connection);
        if manifest_rows.is_empty() {
            return Err(corruption(format!(
                "present file {id} has no verified manifest"
            )));
        }
        Ok(Some(manifest_rows))
    }

    /// Reads every listed manifest and verifies its chunks, charging the
    /// budget before each read.
    fn verify_file_availability(
        &self,
        id: &FileId,
        limits: FileAvailabilityLimits,
        consumed: &mut FileAvailabilityConsumption,
        manifest_rows: Vec<FileManifestId>,
    ) -> Result<FileAvailability> {
        let mut manifests = Vec::with_capacity(manifest_rows.len());
        for manifest_id in manifest_rows {
            let manifest = self.get_file_manifest(&manifest_id)?.ok_or_else(|| {
                corruption(format!(
                    "file {id} references missing or unverified manifest {manifest_id}"
                ))
            })?;
            if manifest.file_id() != id {
                return Err(corruption(format!(
                    "manifest {manifest_id} reconstructs another FileId"
                )));
            }
            consumed.charge_rows(manifest.chunks().len(), limits.rows)?;
            consumed.charge_chunks(manifest.chunks().len(), limits.chunks)?;
            for entry in manifest.chunks() {
                let chunk_bytes = self.preflight_chunk_bytes(entry.chunk_id())?;
                if chunk_bytes > 64 * 1024 * 1024 {
                    return Err(file_budget_exceeded(
                        "individual Chunk bytes",
                        64 * 1024 * 1024,
                    ));
                }
                consumed.charge_bytes(chunk_bytes, limits.bytes)?;
                if u64::try_from(chunk_bytes).ok() != Some(entry.byte_length()) {
                    return Err(corruption(format!(
                        "objects.byte_length for Chunk {} disagrees with FileManifest {manifest_id}",
                        entry.chunk_id()
                    )));
                }
            }

            let mut verifier = manifest.verifier();
            for entry in manifest.chunks() {
                let chunk = self.get_chunk(entry.chunk_id())?.ok_or_else(|| {
                    corruption(format!(
                        "manifest {manifest_id} references missing Chunk {}",
                        entry.chunk_id()
                    ))
                })?;
                verifier.push(chunk.bytes()).map_err(core_corruption)?;
            }
            verifier.finish().map_err(core_corruption)?;
            manifests.push(manifest_id);
        }
        FileAvailability::present(id.clone(), manifests)
            .map_err(|error| corruption(error.to_string()))
    }
}
