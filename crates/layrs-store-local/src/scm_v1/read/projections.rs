//! Canonical objects and their relational projections.
use super::*;
impl NativeScmReader<'_> {
    pub(super) fn validate_object_edges(
        &self,
        source: &StoreObjectId,
        expected: Vec<(String, u64, String)>,
    ) -> Result<()> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT edge_kind, ordinal, target_object_id FROM object_edges \
                 WHERE source_object_id = ?1 ORDER BY edge_kind, ordinal",
            )
            .map_err(sqlite_backend)?;
        let actual = statement
            .query_map([source.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(sqlite_backend)?
            .map(|row| row.map_err(sqlite_backend))
            .collect::<Result<Vec<_>>>()?;
        let mut expected = expected;
        expected.sort();
        if actual != expected {
            return Err(corruption(format!(
                "object_edges for {source} disagree with canonical CAS"
            )));
        }
        Ok(())
    }

    pub(super) fn validate_manifest_projection(&self, manifest: &FileManifest) -> Result<()> {
        let connection = self.connection()?;
        let row: Option<(String, u64, String)> = connection
            .query_row(
                "SELECT file_id, file_byte_length, chunking_json FROM file_manifests \
                 WHERE manifest_id = ?1",
                [manifest.id().as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sqlite_backend)?;
        let chunking = canonical_text(manifest.chunking())?;
        if row.as_ref().is_none_or(|(file_id, length, projected)| {
            file_id != manifest.file_id().as_str()
                || *length != manifest.byte_length()
                || projected != &chunking
        }) {
            return Err(corruption(format!(
                "file_manifests projection for {} disagrees with canonical CAS",
                manifest.id()
            )));
        }
        let mut statement = connection
            .prepare(
                "SELECT ordinal, chunk_id, byte_offset, byte_length FROM file_manifest_chunks \
                 WHERE manifest_id = ?1 ORDER BY ordinal",
            )
            .map_err(sqlite_backend)?;
        let actual = statement
            .query_map([manifest.id().as_str()], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            })
            .map_err(sqlite_backend)?
            .map(|row| row.map_err(sqlite_backend))
            .collect::<Result<Vec<_>>>()?;
        let expected: Vec<_> = manifest
            .chunks()
            .iter()
            .enumerate()
            .map(|(ordinal, chunk)| {
                (
                    ordinal as u64,
                    chunk.chunk_id().to_string(),
                    chunk.offset(),
                    chunk.byte_length(),
                )
            })
            .collect();
        drop(statement);
        drop(connection);
        if actual != expected {
            return Err(corruption(format!(
                "file_manifest_chunks projection for {} disagrees with canonical CAS",
                manifest.id()
            )));
        }
        self.validate_object_edges(
            &StoreObjectId::FileManifest(manifest.id().clone()),
            manifest
                .chunks()
                .iter()
                .enumerate()
                .map(|(ordinal, chunk)| {
                    (
                        "manifest_chunk".to_owned(),
                        ordinal as u64,
                        chunk.chunk_id().to_string(),
                    )
                })
                .collect(),
        )
    }

    pub(super) fn validate_tree_projection(&self, tree: &Tree) -> Result<()> {
        let edges = tree
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(ordinal, entry)| {
                let target = entry.target();
                target
                    .tree_id()
                    .map(ToString::to_string)
                    .or_else(|| target.conflict_id().map(ToString::to_string))
                    .or_else(|| target.symlink_id().map(ToString::to_string))
                    .or_else(|| target.tombstone_id().map(ToString::to_string))
                    .map(|target| ("tree_entry".to_owned(), ordinal as u64, target))
            })
            .collect();
        self.validate_object_edges(&StoreObjectId::Tree(tree.id().clone()), edges)
    }

    pub(super) fn validate_step_projection(&self, step: &Step) -> Result<()> {
        let connection = self.connection()?;
        let row: Option<(String, u64)> = connection
            .query_row(
                "SELECT root_tree_id, parent_count FROM steps WHERE step_id = ?1",
                [step.id().as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sqlite_backend)?;
        if row.as_ref().is_none_or(|(root, count)| {
            root != step.root_tree_id().as_str() || *count != step.parent_step_ids().len() as u64
        }) {
            return Err(corruption(format!(
                "steps projection for {} disagrees with canonical CAS",
                step.id()
            )));
        }
        drop(connection);
        let parents = self.ordered_strings(
            "SELECT ordinal, parent_step_id FROM step_parents WHERE step_id = ?1 ORDER BY ordinal",
            step.id().as_str(),
        )?;
        if parents
            != step
                .parent_step_ids()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        {
            return Err(corruption(format!(
                "step_parents projection for {} disagrees with canonical CAS",
                step.id()
            )));
        }
        let mut edges: Vec<_> = step
            .parent_step_ids()
            .iter()
            .enumerate()
            .map(|(ordinal, parent)| ("step_parent".to_owned(), ordinal as u64, parent.to_string()))
            .collect();
        edges.push(("step_tree".to_owned(), 0, step.root_tree_id().to_string()));
        self.validate_object_edges(&StoreObjectId::Step(step.id().clone()), edges)
    }

    pub(super) fn validate_view_projection(&self, view: &RepositoryView) -> Result<()> {
        let connection = self.connection()?;
        let space: Option<String> = connection
            .query_row(
                "SELECT space_id FROM repository_views WHERE repository_view_id = ?1",
                [view.id().as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_backend)?;
        if space.as_deref() != Some(view.space_id().as_str()) {
            return Err(corruption(format!(
                "repository_views projection for {} disagrees with canonical CAS",
                view.id()
            )));
        }
        let mut heads_statement = connection
            .prepare(
                "SELECT layer_id, ordinal, step_id FROM repository_view_layer_heads \
                 WHERE repository_view_id = ?1 ORDER BY layer_id, ordinal",
            )
            .map_err(sqlite_backend)?;
        let heads = heads_statement
            .query_map([view.id().as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(sqlite_backend)?
            .map(|row| row.map_err(sqlite_backend))
            .collect::<Result<Vec<_>>>()?;
        let expected_heads: Vec<_> = view
            .layer_heads()
            .iter()
            .flat_map(|head_set| {
                head_set
                    .step_ids()
                    .iter()
                    .enumerate()
                    .map(|(ordinal, step)| {
                        (
                            head_set.layer_id().to_string(),
                            ordinal as u64,
                            step.to_string(),
                        )
                    })
            })
            .collect();
        if heads != expected_heads {
            return Err(corruption(format!(
                "repository_view_layer_heads projection for {} disagrees with canonical CAS",
                view.id()
            )));
        }
        drop(heads_statement);
        let mut worktree_statement = connection
            .prepare(
                "SELECT ordinal, worktree_id, active_layer_id, base_step_id, \
                        materialized_tree_id, status FROM repository_view_worktrees \
                 WHERE repository_view_id = ?1 ORDER BY ordinal",
            )
            .map_err(sqlite_backend)?;
        let worktrees = worktree_statement
            .query_map([view.id().as_str()], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(sqlite_backend)?
            .map(|row| row.map_err(sqlite_backend))
            .collect::<Result<Vec<_>>>()?;
        let expected_worktrees: Vec<_> = view
            .worktrees()
            .iter()
            .enumerate()
            .map(|(ordinal, state)| {
                (
                    ordinal as u64,
                    state.worktree_id().to_string(),
                    state.active_layer_id().map(ToString::to_string),
                    state.base_step_id().map(ToString::to_string),
                    state.materialized_tree_id().map(ToString::to_string),
                    enum_name(state.status()),
                )
            })
            .collect();
        drop(worktree_statement);
        drop(connection);
        if worktrees != expected_worktrees {
            return Err(corruption(format!(
                "repository_view_worktrees projection for {} disagrees with canonical CAS",
                view.id()
            )));
        }
        let markers = self.ordered_strings(
            "SELECT ordinal, marker_id FROM repository_view_markers WHERE repository_view_id = ?1 ORDER BY ordinal",
            view.id().as_str(),
        )?;
        if markers
            != view
                .marker_ids()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        {
            return Err(corruption(format!(
                "repository_view_markers projection for {} disagrees with canonical CAS",
                view.id()
            )));
        }
        let releases = self.ordered_strings(
            "SELECT ordinal, release_id FROM repository_view_releases WHERE repository_view_id = ?1 ORDER BY ordinal",
            view.id().as_str(),
        )?;
        if releases
            != view
                .release_ids()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        {
            return Err(corruption(format!(
                "repository_view_releases projection for {} disagrees with canonical CAS",
                view.id()
            )));
        }
        let edges = view
            .layer_heads()
            .iter()
            .flat_map(|heads| heads.step_ids())
            .enumerate()
            .map(|(ordinal, step)| ("view_step".to_owned(), ordinal as u64, step.to_string()))
            .collect();
        self.validate_object_edges(&StoreObjectId::RepositoryView(view.id().clone()), edges)
    }

    pub(super) fn validate_layer_projection(&self, layer: &Layer) -> Result<()> {
        self.validate_scalar_projections(
            "layers",
            "layer_id",
            layer.id().as_str(),
            layer,
            &[
                ("layer_id", "layerId"),
                ("space_id", "spaceId"),
                ("layer_kind", "kind"),
                ("layer_status", "status"),
            ],
        )?;
        let parents = self.ordered_strings(
            "SELECT ordinal, parent_layer_id FROM layer_parents WHERE layer_id = ?1 ORDER BY ordinal",
            layer.id().as_str(),
        )?;
        if parents
            != layer
                .parent_layer_ids()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        {
            return Err(corruption(format!(
                "layer_parents projection for {} disagrees with payload",
                layer.id()
            )));
        }
        Ok(())
    }

    pub(super) fn validate_operation_projection(&self, operation: &Operation) -> Result<()> {
        self.validate_scalar_projections(
            "operations",
            "operation_id",
            operation.id().as_str(),
            operation,
            &[
                ("operation_id", "operationId"),
                ("space_id", "spaceId"),
                ("resulting_view_id", "resultingViewId"),
                ("operation_kind", "kind"),
                ("actor_id", "actorId"),
                ("device_id", "deviceId"),
                ("created_at", "createdAt"),
            ],
        )?;
        let parents = self.ordered_strings(
            "SELECT ordinal, parent_operation_id FROM operation_parents WHERE operation_id = ?1 ORDER BY ordinal",
            operation.id().as_str(),
        )?;
        if parents
            != operation
                .parent_operation_ids()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        {
            return Err(corruption(format!(
                "operation_parents projection for {} disagrees with payload",
                operation.id()
            )));
        }
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT r.ordinal, r.old_step_id, t.ordinal, t.new_step_id \
                 FROM operation_rewrites r LEFT JOIN operation_rewrite_targets t \
                   ON t.operation_id = r.operation_id AND t.old_step_id = r.old_step_id \
                 WHERE r.operation_id = ?1 ORDER BY r.ordinal, t.ordinal",
            )
            .map_err(sqlite_backend)?;
        let actual = statement
            .query_map([operation.id().as_str()], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<u64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(sqlite_backend)?
            .map(|row| row.map_err(sqlite_backend))
            .collect::<Result<Vec<_>>>()?;
        let mut expected = Vec::new();
        for (rewrite_ordinal, rewrite) in operation.rewrite_map().iter().enumerate() {
            if rewrite.new_step_ids().is_empty() {
                expected.push((
                    rewrite_ordinal as u64,
                    rewrite.old_step_id().to_string(),
                    None,
                    None,
                ));
            } else {
                expected.extend(rewrite.new_step_ids().iter().enumerate().map(
                    |(target_ordinal, target)| {
                        (
                            rewrite_ordinal as u64,
                            rewrite.old_step_id().to_string(),
                            Some(target_ordinal as u64),
                            Some(target.to_string()),
                        )
                    },
                ));
            }
        }
        if actual != expected {
            return Err(corruption(format!(
                "operation rewrite projection for {} disagrees with payload",
                operation.id()
            )));
        }
        Ok(())
    }

    pub(super) fn validate_proofs(
        &self,
        table: &'static str,
        id_column: &'static str,
        id: &str,
        expected: impl Iterator<Item = String>,
    ) -> Result<()> {
        let sql = format!(
            "SELECT ordinal, proof_id FROM {table} WHERE {id_column} = ?1 ORDER BY ordinal"
        );
        if self.ordered_strings(&sql, id)? != expected.collect::<Vec<_>>() {
            return Err(corruption(format!(
                "{table} projection for {id} disagrees with payload"
            )));
        }
        Ok(())
    }
}
