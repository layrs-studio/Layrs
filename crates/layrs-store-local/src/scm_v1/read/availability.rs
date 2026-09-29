//! Typed closure and bounded File availability reads.
use super::*;
impl NativeScmReader<'_> {
    pub(super) fn object_exists_typed(&self, id: &StoreObjectId) -> Result<bool> {
        match id {
            StoreObjectId::ChunkRaw(id) => self.get_chunk(id).map(|value| value.is_some()),
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
        let limits = FileAvailabilityLimits {
            rows: max_rows,
            bytes: max_bytes,
            manifests: max_manifests,
            chunks: max_chunks,
        };
        let mut consumed = FileAvailabilityConsumption::default();
        consumed.charge_rows(1, limits.rows)?;

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
            return Ok(FileAvailability::Absent);
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
