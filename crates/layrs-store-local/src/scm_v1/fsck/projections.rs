//! Manifest, object-edge and View projection verification.
use super::*;
impl Context<'_> {
    pub(super) fn check_manifest_projections(&mut self) -> Result<(), FsckError> {
        let manifests: Vec<_> = self
            .objects
            .iter()
            .filter_map(|(id, object)| match object {
                CanonicalObject::FileManifest(value) => Some((id.clone(), value.clone())),
                _ => None,
            })
            .collect();
        for (id, manifest) in manifests {
            if let Err(error) = self.reconstruct_manifest_streaming(&manifest) {
                self.finding(
                    FsckFindingCode::ManifestReconstructionFailure,
                    FsckSeverity::Error,
                    None,
                    Some(id.clone()),
                    None,
                    error,
                );
            }
            let row = self
                .connection
                .query_row(
                    "SELECT file_id, file_byte_length FROM file_manifests WHERE manifest_id=?1",
                    [&id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
                )
                .optional()?;
            if row.as_ref().is_none_or(|(file_id, length)| {
                file_id != manifest.file_id().as_str() || *length != manifest.byte_length()
            }) {
                self.projection_divergence(
                    "file_manifests",
                    &id,
                    "manifest identity/length projection differs from canonical CAS",
                );
            }
            let projected = query_strings(
                self.connection,
                "SELECT chunk_id || ':' || byte_offset || ':' || byte_length FROM file_manifest_chunks WHERE manifest_id=?1 ORDER BY ordinal",
                &id,
            )?;
            let expected: Vec<_> = manifest
                .chunks()
                .iter()
                .map(|chunk| {
                    format!(
                        "{}:{}:{}",
                        chunk.chunk_id().as_str(),
                        chunk.offset(),
                        chunk.byte_length()
                    )
                })
                .collect();
            if projected != expected {
                self.projection_divergence(
                    "file_manifest_chunks",
                    &id,
                    "chunk projection differs from canonical manifest",
                );
            }
        }
        Ok(())
    }

    pub(super) fn reconstruct_manifest_streaming(
        &mut self,
        manifest: &layrs_core::scm::FileManifest,
    ) -> Result<(), String> {
        if manifest.byte_length() > 1024 * 1024 * 1024
            || manifest.byte_length() > self.file_byte_budget_remaining
        {
            self.exhaust_bytes("logical File verification");
            return Err("File verification exceeds1GiB per file or4GiB total".into());
        }
        self.file_byte_budget_remaining -= manifest.byte_length();
        let cas = NativeCas::open_audit(self.audit).map_err(|error| error.to_string())?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs-file-v1\0");
        let mut reconstructed_length = 0_u64;
        for entry in manifest.chunks() {
            if !self.object_ids.contains_key(entry.chunk_id().as_str()) {
                return Err(format!("missing verified Chunk {}", entry.chunk_id()));
            }
            let chunk_length = cas
                .stream_verified_chunk(entry.chunk_id(), |bytes| {
                    hasher.update(bytes);
                })
                .map_err(|error| error.to_string())?;
            if chunk_length != entry.byte_length() {
                return Err(format!(
                    "Chunk {} length is {}, expected {}",
                    entry.chunk_id(),
                    chunk_length,
                    entry.byte_length()
                ));
            }
            reconstructed_length = reconstructed_length.saturating_add(chunk_length);
        }
        if reconstructed_length != manifest.byte_length() {
            return Err(format!(
                "reconstructed length is {reconstructed_length}, expected {}",
                manifest.byte_length()
            ));
        }
        let actual = format!("blake3:{}", hasher.finalize().to_hex());
        if actual != manifest.file_id().as_str() {
            return Err(format!(
                "reconstructed FileId is {actual}, expected {}",
                manifest.file_id()
            ));
        }
        Ok(())
    }

    pub(super) fn check_object_edges(&mut self) -> Result<(), FsckError> {
        let actual = {
            let mut statement = self.connection.prepare(
                "SELECT source_object_id, edge_kind, ordinal, target_object_id FROM object_edges ORDER BY source_object_id, edge_kind, ordinal",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<BTreeSet<_>>>()?
        };
        if !self.consume_cpu(actual.len()) {
            return Ok(());
        }
        let mut expected = BTreeSet::new();
        let objects: Vec<_> = self
            .objects
            .iter()
            .map(|(id, object)| (id.clone(), object.clone()))
            .collect();
        for (source, object) in objects {
            if !self.consume_cpu(1) {
                return Ok(());
            }
            match object {
                CanonicalObject::Product(value) => {
                    for (ordinal, dependency) in value.direct_dependencies().iter().enumerate() {
                        expected.insert((
                            source.clone(),
                            "product_dependency".to_owned(),
                            ordinal as u64,
                            dependency.to_string(),
                        ));
                    }
                    let projected: Option<String> = self
                        .connection
                        .query_row(
                            "SELECT product_id FROM products WHERE product_object_id=?1",
                            [value.id().as_str()],
                            |row| row.get(0),
                        )
                        .optional()?;
                    if projected.as_deref() != Some(value.envelope().logical_id()) {
                        self.projection_divergence(
                            "products",
                            value.id().as_str(),
                            "logical ID differs from canonical Product",
                        );
                    }
                }
                CanonicalObject::FileManifest(value) => {
                    for (ordinal, chunk) in value.chunks().iter().enumerate() {
                        if !self.consume_cpu(1) {
                            return Ok(());
                        }
                        expected.insert((
                            source.clone(),
                            "manifest_chunk".to_owned(),
                            ordinal as u64,
                            chunk.chunk_id().to_string(),
                        ));
                    }
                }
                CanonicalObject::Tree(value) => {
                    for (ordinal, entry) in value.entries().iter().enumerate() {
                        if !self.consume_cpu(1) {
                            return Ok(());
                        }
                        let target = entry.target();
                        let target_id = target
                            .tree_id()
                            .map(ToString::to_string)
                            .or_else(|| target.conflict_id().map(ToString::to_string))
                            .or_else(|| target.symlink_id().map(ToString::to_string))
                            .or_else(|| target.tombstone_id().map(ToString::to_string));
                        if let Some(target_id) = target_id {
                            if !self.object_ids.contains_key(&target_id) {
                                self.finding(
                                    FsckFindingCode::TreeClosureBroken,
                                    FsckSeverity::Error,
                                    None,
                                    Some(source.clone()),
                                    None,
                                    format!("Tree target object {target_id} is missing"),
                                );
                            }
                            expected.insert((
                                source.clone(),
                                "tree_entry".to_owned(),
                                ordinal as u64,
                                target_id,
                            ));
                        } else if let Some(file_id) = target.file_id() {
                            let present: bool = self.connection.query_row(
                                "SELECT EXISTS(SELECT 1 FROM file_manifests WHERE file_id=?1)",
                                [file_id.as_str()],
                                |row| row.get(0),
                            )?;
                            if !present {
                                self.finding(
                                    FsckFindingCode::TreeClosureBroken,
                                    FsckSeverity::Error,
                                    None,
                                    Some(source.clone()),
                                    None,
                                    format!("Tree references unavailable FileId {file_id}"),
                                );
                            }
                        }
                    }
                }
                CanonicalObject::Step(value) => {
                    for (ordinal, parent) in value.parent_step_ids().iter().enumerate() {
                        if !self.consume_cpu(1) {
                            return Ok(());
                        }
                        expected.insert((
                            source.clone(),
                            "step_parent".to_owned(),
                            ordinal as u64,
                            parent.to_string(),
                        ));
                    }
                    expected.insert((
                        source,
                        "step_tree".to_owned(),
                        0,
                        value.root_tree_id().to_string(),
                    ));
                }
                CanonicalObject::RepositoryView(value) => {
                    let mut ordinal = 0_u64;
                    for heads in value.layer_heads() {
                        for step in heads.step_ids() {
                            if !self.consume_cpu(1) {
                                return Ok(());
                            }
                            expected.insert((
                                source.clone(),
                                "view_step".to_owned(),
                                ordinal,
                                step.to_string(),
                            ));
                            ordinal += 1;
                        }
                    }
                    self.check_view_projection(&source, &value)?;
                }
                _ => {}
            }
        }
        for edge in expected.symmetric_difference(&actual) {
            if !self.consume_cpu(1) {
                return Ok(());
            }
            self.finding(
                FsckFindingCode::ObjectEdgeMismatch,
                FsckSeverity::Error,
                None,
                Some(edge.0.clone()),
                None,
                format!(
                    "edge projection mismatch: {}[{}] -> {}",
                    edge.1, edge.2, edge.3
                ),
            );
        }
        Ok(())
    }

    pub(super) fn check_view_projection(
        &mut self,
        id: &str,
        view: &layrs_core::scm::RepositoryView,
    ) -> Result<(), FsckError> {
        let space: Option<String> = self
            .connection
            .query_row(
                "SELECT space_id FROM repository_views WHERE repository_view_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if space.as_deref() != Some(view.space_id().as_str()) {
            self.projection_divergence(
                "repository_views",
                id,
                "Space projection differs from canonical View",
            );
        }
        let projected_heads = query_strings(
            self.connection,
            "SELECT layer_id || ':' || step_id FROM repository_view_layer_heads WHERE repository_view_id=?1 ORDER BY layer_id, ordinal",
            id,
        )?;
        let expected_heads: Vec<_> = view
            .layer_heads()
            .iter()
            .flat_map(|heads| {
                heads
                    .step_ids()
                    .iter()
                    .map(|step| format!("{}:{}", heads.layer_id(), step))
            })
            .collect();
        if projected_heads != expected_heads {
            self.projection_divergence(
                "repository_view_layer_heads",
                id,
                "head projection differs from canonical View",
            );
        }
        for (table, column, expected) in [
            (
                "repository_view_markers",
                "marker_id",
                view.marker_ids()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            ),
            (
                "repository_view_releases",
                "release_id",
                view.release_ids()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            ),
        ] {
            let sql = format!(
                "SELECT {column} FROM {table} WHERE repository_view_id=?1 ORDER BY ordinal"
            );
            if query_strings(self.connection, &sql, id)? != expected {
                self.projection_divergence(table, id, "projection differs from canonical View");
            }
        }
        let payload =
            serde_json::to_value(view).map_err(|error| FsckError::Native(error.to_string()))?;
        let expected_worktrees: Vec<_> = payload
            .get("worktrees")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        let projected_worktrees = query_strings(
            self.connection,
            "SELECT json_object(
                 'worktreeId',worktree_id,'activeLayerId',active_layer_id,
                 'baseStepId',base_step_id,'materializedTreeId',materialized_tree_id,'status',status
             ) FROM repository_view_worktrees WHERE repository_view_id=?1 ORDER BY ordinal",
            id,
        )?;
        let projected_worktrees: Vec<serde_json::Value> = projected_worktrees
            .into_iter()
            .map(|value| serde_json::from_str(&value))
            .collect::<Result<_, _>>()
            .map_err(|error| FsckError::Native(error.to_string()))?;
        if projected_worktrees != expected_worktrees {
            self.projection_divergence(
                "repository_view_worktrees",
                id,
                "Worktree projection differs from canonical View",
            );
        }
        Ok(())
    }

    pub(super) fn projection_divergence(&mut self, table: &str, entity_id: &str, reason: &str) {
        self.finding(
            FsckFindingCode::ProjectionDivergence,
            FsckSeverity::Error,
            None,
            None,
            Some(entity_id.to_owned()),
            format!("{table}: {reason}"),
        );
        if self.report.projection_rebuild_plan.actions.len() < self.max_findings {
            self.report
                .projection_rebuild_plan
                .actions
                .push(ProjectionAction {
                    table: table.to_owned(),
                    entity_id: entity_id.to_owned(),
                    reason: reason.to_owned(),
                });
        }
    }
}
