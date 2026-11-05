//! Entity payload projections and recovery separation.
use super::*;
impl Context<'_> {
    pub(super) fn check_payload_projections(&mut self) -> Result<(), FsckError> {
        const TABLES: &[(&str, &str, &str)] = &[
            ("spaces", "space_id", "spaceId"),
            ("layers", "layer_id", "layerId"),
            ("step_observations", "observation_id", "observationId"),
            ("operations", "operation_id", "operationId"),
            ("worktrees", "worktree_id", "worktreeId"),
            ("markers", "marker_id", "markerId"),
            ("releases", "release_id", "releaseId"),
            (
                "release_publications",
                "release_publication_id",
                "releasePublicationId",
            ),
        ];
        let mut processed = 0_usize;
        for (table, id_column, payload_id) in TABLES {
            let remaining = self.row_limit.saturating_sub(processed);
            if remaining == 0 {
                self.work_truncated = true;
                self.report.truncated_count = self.report.truncated_count.saturating_add(1);
                return Ok(());
            }
            let sql = format!(
                "SELECT rowid, {id_column}, length(CAST(payload_json AS BLOB)) \
                 FROM {table} ORDER BY {id_column} LIMIT ?1"
            );
            let rows = {
                let mut statement = self.connection.prepare(&sql)?;
                let rows = statement.query_map([remaining.saturating_add(1) as u64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            if rows.len() > remaining {
                self.work_truncated = true;
                self.report.truncated_count = self.report.truncated_count.saturating_add(1);
            }
            for (rowid, id, payload_length) in rows.into_iter().take(remaining) {
                processed = processed.saturating_add(1);
                self.report.checked_rows = self.report.checked_rows.saturating_add(1);
                if !self.reserve_bytes(
                    payload_length,
                    MAX_ENTITY_PAYLOAD_BYTES,
                    &format!("{table}.{id_column}={id} payload_json"),
                ) {
                    return Ok(());
                }
                let payload = self.connection.query_row(
                    &format!("SELECT payload_json FROM {table} WHERE rowid=?1"),
                    [rowid],
                    |row| row.get::<_, String>(0),
                )?;
                let value = match decode_core_projection(table, &payload) {
                    Ok(value) => value,
                    Err(error) => {
                        // A rebuild action is safe only when Core accepted the
                        // source payload and recomputed all of its invariants.
                        self.finding(
                            FsckFindingCode::ProjectionDivergence,
                            FsckSeverity::Error,
                            None,
                            None,
                            Some(id.clone()),
                            format!("{table}: payload rejected by Core: {error}"),
                        );
                        continue;
                    }
                };
                let canonical =
                    canonical_json(&value).map_err(|error| FsckError::Native(error.to_string()))?;
                if canonical != payload.as_bytes() {
                    self.projection_divergence(table, &id, "payload_json is not exact JCS");
                }
                if value.get(*payload_id).and_then(serde_json::Value::as_str) != Some(id.as_str()) {
                    self.projection_divergence(
                        table,
                        &id,
                        "payload identity differs from primary key",
                    );
                }
                self.compare_common_projection(table, &id, &value)?;
                self.compare_array_projection(table, &id, &value)?;
            }
            if self.work_truncated {
                return Ok(());
            }
        }
        Ok(())
    }

    pub(super) fn compare_array_projection(
        &mut self,
        table: &str,
        id: &str,
        payload: &serde_json::Value,
    ) -> Result<(), FsckError> {
        let specification = match table {
            "layers" => Some((
                "layer_parents",
                "parent_layer_id",
                "layer_id",
                "parentLayerIds",
            )),
            "operations" => Some((
                "operation_parents",
                "parent_operation_id",
                "operation_id",
                "parentOperationIds",
            )),
            "markers" => Some(("marker_proofs", "proof_id", "marker_id", "proofIds")),
            "releases" => Some(("release_proofs", "proof_id", "release_id", "proofIds")),
            _ => None,
        };
        let Some((projection_table, value_column, id_column, field)) = specification else {
            return Ok(());
        };
        let expected: Vec<_> = payload
            .get(field)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        let sql = format!(
            "SELECT {value_column} FROM {projection_table} WHERE {id_column}=?1 ORDER BY ordinal"
        );
        if query_strings(self.connection, &sql, id)? != expected {
            self.projection_divergence(
                projection_table,
                id,
                &format!("projection differs from canonical {field}"),
            );
        }
        if table == "operations" {
            self.compare_operation_rewrites(id, payload)?;
        }
        Ok(())
    }

    pub(super) fn compare_operation_rewrites(
        &mut self,
        id: &str,
        payload: &serde_json::Value,
    ) -> Result<(), FsckError> {
        let rewrites = payload
            .get("rewriteMap")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten();
        let expected_sources: Vec<_> = rewrites
            .clone()
            .filter_map(|rewrite| rewrite.get("oldStepId")?.as_str())
            .map(str::to_owned)
            .collect();
        let expected_targets: Vec<_> = rewrites
            .flat_map(|rewrite| {
                let old = rewrite
                    .get("oldStepId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                rewrite
                    .get("newStepIds")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(move |new| format!("{old}:{new}"))
            })
            .collect();
        let projected_sources = query_strings(
            self.connection,
            "SELECT old_step_id FROM operation_rewrites WHERE operation_id=?1 ORDER BY ordinal",
            id,
        )?;
        let projected_targets = query_strings(
            self.connection,
            "SELECT r.old_step_id || ':' || t.new_step_id
             FROM operation_rewrites r JOIN operation_rewrite_targets t
               ON t.operation_id=r.operation_id AND t.old_step_id=r.old_step_id
             WHERE r.operation_id=?1 ORDER BY r.ordinal, t.ordinal",
            id,
        )?;
        if projected_sources != expected_sources {
            self.projection_divergence(
                "operation_rewrites",
                id,
                "projection differs from canonical rewriteMap sources",
            );
        }
        if projected_targets != expected_targets {
            self.projection_divergence(
                "operation_rewrite_targets",
                id,
                "projection differs from canonical rewriteMap targets",
            );
        }
        Ok(())
    }

    pub(super) fn compare_common_projection(
        &mut self,
        table: &str,
        id: &str,
        payload: &serde_json::Value,
    ) -> Result<(), FsckError> {
        let comparisons: &[(&str, &str)] = match table {
            "layers" => &[
                ("space_id", "spaceId"),
                ("layer_kind", "kind"),
                ("layer_status", "status"),
            ],
            "operations" => &[
                ("space_id", "spaceId"),
                ("resulting_view_id", "resultingViewId"),
                ("operation_kind", "kind"),
                ("actor_id", "actorId"),
                ("device_id", "deviceId"),
                ("created_at", "createdAt"),
            ],
            "worktrees" => &[
                ("store_id", "storeId"),
                ("space_id", "spaceId"),
                ("active_operation_id", "activeOperationId"),
                ("checkout_root", "checkoutRoot"),
                ("filesystem_identity", "filesystemIdentity"),
            ],
            "markers" => &[
                ("space_id", "spaceId"),
                ("target_step_id", "targetStepId"),
                ("marker_kind", "kind"),
                ("created_at", "createdAt"),
            ],
            "releases" => &[
                ("space_id", "spaceId"),
                ("target_step_id", "targetStepId"),
                ("version", "version"),
                ("created_at", "createdAt"),
            ],
            "release_publications" => &[
                ("release_id", "releaseId"),
                ("space_id", "spaceId"),
                ("operation_id", "operationId"),
                ("published_at", "publishedAt"),
            ],
            "step_observations" => &[
                ("step_id", "stepId"),
                ("space_id", "spaceId"),
                ("operation_id", "operationId"),
                ("worktree_id", "worktreeId"),
                ("actor_id", "actorId"),
                ("device_id", "deviceId"),
                ("observed_at", "observedAt"),
                ("capture_reason", "captureReason"),
            ],
            _ => &[],
        };
        let id_column = match table {
            "spaces" => "space_id",
            "layers" => "layer_id",
            "operations" => "operation_id",
            "worktrees" => "worktree_id",
            "markers" => "marker_id",
            "releases" => "release_id",
            "release_publications" => "release_publication_id",
            "step_observations" => "observation_id",
            _ => return Ok(()),
        };
        for (column, field) in comparisons {
            let sql = format!("SELECT CAST({column} AS TEXT) FROM {table} WHERE {id_column}=?1");
            let projected: Option<Option<String>> = self
                .connection
                .query_row(&sql, [id], |row| row.get::<_, Option<String>>(0))
                .optional()?;
            let canonical = payload.get(*field).and_then(value_as_projection_text);
            if projected.as_ref() != Some(&canonical) {
                self.projection_divergence(
                    table,
                    id,
                    &format!("column {column} differs from payload field {field}"),
                );
            }
        }
        if matches!(table, "spaces" | "layers" | "worktrees") {
            let timestamp = payload
                .get("createdAt")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| FsckError::Native(format!("{table} {id} has no createdAt")))?;
            let expected_ms = sqlite::rfc3339_to_epoch_millis(timestamp)
                .map_err(|reason| FsckError::Native(format!("{table} {id}: {reason}")))?;
            let projected_ms: Option<i64> = self
                .connection
                .query_row(
                    &format!("SELECT created_at_ms FROM {table} WHERE {id_column}=?1"),
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            if projected_ms != Some(expected_ms) {
                self.projection_divergence(
                    table,
                    id,
                    "column created_at_ms differs from the safely converted createdAt timestamp",
                );
            }
        }
        Ok(())
    }

    pub(super) fn check_recovery_separation(&mut self) -> Result<(), FsckError> {
        let mixed = query_strings_no_param(
            self.connection,
            "SELECT r.recovery_operation_id FROM recovery_operations r \
             JOIN operations o ON o.operation_id=r.recovery_operation_id",
        )?;
        for id in mixed {
            self.error(
                FsckFindingCode::RecoveryOperationMixing,
                Some(id),
                "technical RecoveryOperation identity is also a user-visible Operation",
            );
        }
        Ok(())
    }
}
