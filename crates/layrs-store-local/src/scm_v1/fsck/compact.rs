//! Format 6 audits (ADR 0028): content lives in registered packs v2 and
//! loose objects, not in SQL rows. Every registered copy is verified at its
//! own place; FileId sections, file records and admission bitmaps are
//! checked against the decoded content; references that may name content
//! (pins, GC roots, recovery paths) are resolved through the catalog.
use super::*;
use crate::scm_v1::cas::{AuditEvent, RegisteredCatalog, is_content};
use std::collections::HashSet;
use std::sync::Arc;

/// Format 8's derived tables: the format 7 projections it does not store,
/// rebuilt per fsck run in this connection's temporary schema (no foreign
/// keys: fsck reports what they would refuse).
const DERIVED_TABLES: &str = "
CREATE TEMP TABLE IF NOT EXISTS space_object_admissions (
    space_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    admitted_by_operation_id TEXT,
    admitted_at_ms INTEGER NOT NULL,
    PRIMARY KEY (space_id, object_id)
) WITHOUT ROWID;
CREATE TEMP TABLE IF NOT EXISTS objects (
    object_id TEXT PRIMARY KEY,
    object_kind TEXT NOT NULL,
    byte_length INTEGER NOT NULL,
    verified INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TEMP TABLE IF NOT EXISTS object_edges (
    source_object_id TEXT NOT NULL,
    target_object_id TEXT NOT NULL,
    edge_kind TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (source_object_id, edge_kind, ordinal)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS temp.object_edges_target_idx ON object_edges(target_object_id, edge_kind);
CREATE TEMP TABLE IF NOT EXISTS steps (
    step_id TEXT PRIMARY KEY,
    root_tree_id TEXT NOT NULL,
    parent_count INTEGER NOT NULL
);
CREATE TEMP TABLE IF NOT EXISTS step_parents (
    step_id TEXT NOT NULL,
    parent_step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (step_id, ordinal)
);
CREATE INDEX IF NOT EXISTS temp.step_parents_parent_idx ON step_parents(parent_step_id);
CREATE TEMP TABLE IF NOT EXISTS repository_views (
    repository_view_id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL
);
CREATE TEMP TABLE IF NOT EXISTS repository_view_layer_heads (
    repository_view_id TEXT NOT NULL,
    layer_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (repository_view_id, layer_id, ordinal)
);
CREATE TEMP TABLE IF NOT EXISTS repository_view_markers (
    repository_view_id TEXT NOT NULL,
    marker_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (repository_view_id, ordinal)
);
CREATE TEMP TABLE IF NOT EXISTS repository_view_releases (
    repository_view_id TEXT NOT NULL,
    release_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (repository_view_id, ordinal)
);
CREATE TEMP TABLE IF NOT EXISTS operations (
    operation_id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL,
    resulting_view_id TEXT NOT NULL,
    operation_kind TEXT NOT NULL,
    actor_id TEXT,
    device_id TEXT,
    created_at TEXT NOT NULL,
    payload_json TEXT NOT NULL
);
CREATE TEMP TABLE IF NOT EXISTS operation_parents (
    operation_id TEXT NOT NULL,
    parent_operation_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (operation_id, ordinal)
);
CREATE TEMP TABLE IF NOT EXISTS operation_rewrites (
    operation_id TEXT NOT NULL,
    old_step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (operation_id, ordinal)
);
CREATE TEMP TABLE IF NOT EXISTS operation_rewrite_targets (
    operation_id TEXT NOT NULL,
    old_step_id TEXT NOT NULL,
    new_step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (operation_id, old_step_id, ordinal)
);
CREATE TEMP TABLE IF NOT EXISTS step_observations (
    observation_id TEXT PRIMARY KEY,
    step_id TEXT NOT NULL,
    space_id TEXT,
    operation_id TEXT,
    worktree_id TEXT,
    actor_id TEXT,
    device_id TEXT,
    observed_at TEXT NOT NULL,
    capture_reason TEXT NOT NULL,
    payload_json TEXT NOT NULL
);
CREATE TEMP TABLE IF NOT EXISTS repository_view_worktrees (
    repository_view_id TEXT NOT NULL,
    worktree_id TEXT NOT NULL,
    active_layer_id TEXT,
    base_step_id TEXT,
    materialized_tree_id TEXT,
    status TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (repository_view_id, ordinal)
);
";

impl Context<'_> {
    /// Format 8 keeps no `space_object_admissions`: its admissions of
    /// objects that are not content are rebuilt from the bitmaps into a
    /// temporary table of this connection, so every check that reads them
    /// runs unchanged (council s25: fsck derives what the Store no longer
    /// stores).
    /// The other projections it no longer keeps (`objects`, edges, Steps,
    /// Views) start empty here and are filled from the records the scan
    /// reads (`project_history_records`).
    pub(super) fn project_bitmap_admissions(&mut self) -> Result<(), FsckError> {
        let sql = |error: rusqlite::Error| FsckError::Native(error.to_string());
        self.connection.execute_batch(DERIVED_TABLES).map_err(sql)?;
        let Some((_, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        // Charged to the row budget as it is expanded, before anything else
        // reads it (council s26 r2).
        let mut work = self.cpu_budget_remaining;
        let mut history_bits = Vec::new();
        let admitted = catalog
            .non_content_admissions(
                self.connection,
                self.row_limit,
                &mut work,
                &mut history_bits,
            )
            .map_err(|error| FsckError::Native(error.to_string()))?;
        self.cpu_budget_remaining = work;
        for (space, pack) in history_bits {
            self.error(
                FsckFindingCode::AdmissionClosureBroken,
                Some(space),
                format!("pack {pack} admits a history record (never admitted)"),
            );
        }
        let Some(admitted) = admitted.filter(|admitted| admitted.len() <= self.row_limit) else {
            self.exhaust_cpu();
            return Ok(());
        };
        let mut insert = self
            .connection
            .prepare(
                "INSERT OR IGNORE INTO temp.space_object_admissions
                 (space_id, object_id, admitted_by_operation_id, admitted_at_ms)
                 VALUES (?1, ?2, NULL, 0)",
            )
            .map_err(sql)?;
        for (space, object) in admitted {
            insert
                .execute(rusqlite::params![space, object.as_str()])
                .map_err(sql)?;
        }
        Ok(())
    }

    /// Format 8: the projection rows of every record the scan read, written
    /// by the writer's own projection code into this run's temporary tables,
    /// so the edge, graph, View and GC checks run as in formats 6 and 7.
    pub(super) fn project_history_records(&mut self) -> Result<(), FsckError> {
        let native = |error: layrs_store::scm::StoreError| FsckError::Native(error.to_string());
        let mut row = self.connection.prepare(
            "INSERT OR IGNORE INTO temp.objects
             (object_id, object_kind, byte_length, verified, created_at_ms)
             VALUES (?1, ?2, 1, 1, 0)",
        )?;
        for (id, object) in &self.objects {
            if is_content(object.object_id().kind()) {
                continue;
            }
            row.execute(rusqlite::params![
                id,
                crate::scm_v1::repository::kind_name(object.object_id().kind())
            ])?;
        }
        drop(row);
        for object in self.objects.values() {
            if is_content(object.object_id().kind()) {
                continue;
            }
            crate::scm_v1::repository::insert_history_projection(self.connection, object)
                .map_err(native)?;
        }
        self.project_history_entities()
    }

    /// Format 8: every history record (Operations, StepObservations) of the
    /// readable packs, checked (its key is its id's, its bytes canonical)
    /// and written into this run's temporary tables by the writer's own row
    /// code, so the entity, graph and GC checks run as before (council s27).
    fn project_history_entities(&mut self) -> Result<(), FsckError> {
        let native = |error: layrs_store::scm::StoreError| FsckError::Native(error.to_string());
        let Some((cas, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        // Streamed, each read authorized first (council s28).
        let mut sequences = Vec::new();
        let mut failure: Option<FsckError> = None;
        let completed = catalog.visit_history(&cas, |step| match step {
            crate::scm_v1::cas::HistoryStep::Authorize(bytes) => {
                let budget = self.byte_budget_remaining;
                self.consume_cpu(1) && self.reserve_bytes(bytes, budget, "history record")
            }
            crate::scm_v1::cas::HistoryStep::Record((code, key, sequence, space, body)) => {
                sequences.push(sequence);
                match crate::scm_v1::cas::decode_history_record(code, &key, &body, space.as_deref())
                {
                    Ok(record) => {
                        if let Err(error) = crate::scm_v1::repository::insert_history_entity(
                            self.connection,
                            &record,
                        ) {
                            failure.get_or_insert(native(error));
                        }
                    }
                    Err(message) => {
                        // Its true content is unknown, not missing.
                        self.unknown_history.insert((code, key));
                        self.finding(
                            FsckFindingCode::CasCorruption,
                            FsckSeverity::Error,
                            None,
                            None,
                            Some(crate::scm_v1::cas::pack2_digest_text(&key)),
                            message,
                        );
                    }
                }
                true
            }
            crate::scm_v1::cas::HistoryStep::Unreadable {
                code,
                key,
                sequence,
                error,
            } => {
                sequences.push(sequence);
                self.unknown_history.insert((code, key));
                self.finding(
                    FsckFindingCode::CasCorruption,
                    FsckSeverity::Error,
                    None,
                    None,
                    Some(crate::scm_v1::cas::pack2_digest_text(&key)),
                    format!("{}: {error}", error.code()),
                );
                true
            }
            crate::scm_v1::cas::HistoryStep::Duplicate { code, key } => {
                self.finding(
                    FsckFindingCode::CasCorruption,
                    FsckSeverity::Error,
                    None,
                    None,
                    Some(crate::scm_v1::cas::pack2_digest_text(&key)),
                    format!("history record of kind {code} is registered in two packs"),
                );
                true
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        if !completed {
            return Ok(());
        }
        self.check_history_clock(sequences)?;
        self.check_history_tables()
    }

    /// Format 8: sequences are unique and below the clock, and, when every
    /// pack was read, dense from 1 (a lost registration or a dropped record
    /// leaves a gap; council s27).
    fn check_history_clock(&mut self, mut sequences: Vec<u64>) -> Result<(), FsckError> {
        let next: i64 = self.connection.query_row(
            "SELECT next_sequence FROM history_clock WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let next = u64::try_from(next).unwrap_or(0);
        sequences.sort_unstable();
        let mut problems = Vec::new();
        if sequences.windows(2).any(|pair| pair[0] == pair[1]) {
            problems.push("two history records share a sequence".to_owned());
        }
        if sequences.last().is_some_and(|last| *last >= next) {
            problems.push(format!(
                "a history sequence is not below the clock ({next})"
            ));
        }
        if !self.history_incomplete()
            && (sequences.len() as u64 != next.saturating_sub(1)
                || sequences
                    .iter()
                    .enumerate()
                    .any(|(at, sequence)| *sequence != at as u64 + 1))
        {
            problems.push("history sequences are not dense below the clock".to_owned());
        }
        for problem in problems {
            self.error(FsckFindingCode::OperationHistoryInvalid, None, problem);
        }
        Ok(())
    }

    /// Format 8: `space_initializers` and `history_identities` equal what the
    /// records say (the records read; beside an unreadable pack, a row whose
    /// record may be there is unknown).
    fn check_history_tables(&mut self) -> Result<(), FsckError> {
        let missing_initializers = query_pairs(
            self.connection,
            "SELECT o.space_id, o.operation_id FROM operations o
             WHERE o.operation_kind = 'initialize'
               AND NOT EXISTS (SELECT 1 FROM space_initializers i
                               WHERE i.space_id = o.space_id AND i.operation_id = o.operation_id)",
        )?;
        for (space, operation) in missing_initializers {
            self.error(
                FsckFindingCode::OperationHistoryInvalid,
                Some(space),
                format!("Initialize Operation {operation} is not its Space's initializer"),
            );
        }
        let orphan_initializers = query_pairs(
            self.connection,
            "SELECT i.space_id, i.operation_id FROM space_initializers i
             WHERE NOT EXISTS (SELECT 1 FROM operations o
                               WHERE o.operation_id = i.operation_id
                                 AND o.space_id = i.space_id
                                 AND o.operation_kind = 'initialize')",
        )?;
        for (space, operation) in orphan_initializers {
            if self.history_unknowable(&operation) {
                continue;
            }
            self.error(
                FsckFindingCode::OperationHistoryInvalid,
                Some(space),
                format!(
                    "initializer row names {operation}, not an Initialize Operation of the Space"
                ),
            );
        }
        let missing_identities = query_pairs(
            self.connection,
            "SELECT 'actor', actor_id FROM operations WHERE actor_id IS NOT NULL
             UNION SELECT 'device', device_id FROM operations WHERE device_id IS NOT NULL
             UNION SELECT 'actor', actor_id FROM step_observations WHERE actor_id IS NOT NULL
             UNION SELECT 'device', device_id FROM step_observations WHERE device_id IS NOT NULL
             EXCEPT SELECT identity_kind, identity_id FROM history_identities",
        )?;
        for (kind, id) in missing_identities {
            self.error(
                FsckFindingCode::OperationHistoryInvalid,
                Some(id),
                format!("history {kind} has no identity row"),
            );
        }
        if !self.history_incomplete() {
            let orphan_identities = query_pairs(
                self.connection,
                "SELECT identity_kind, identity_id FROM history_identities
                 EXCEPT SELECT 'actor', actor_id FROM operations WHERE actor_id IS NOT NULL
                 EXCEPT SELECT 'device', device_id FROM operations WHERE device_id IS NOT NULL
                 EXCEPT SELECT 'actor', actor_id FROM step_observations WHERE actor_id IS NOT NULL
                 EXCEPT SELECT 'device', device_id FROM step_observations WHERE device_id IS NOT NULL",
            )?;
            for (kind, id) in orphan_identities {
                self.error(
                    FsckFindingCode::OperationHistoryInvalid,
                    Some(id),
                    format!("identity row {kind} is named by no history record"),
                );
            }
        }
        Ok(())
    }

    /// Format 8 keeps none of the foreign keys formats 5 to 7 check on their
    /// projection rows (council s26 r1): every reference they guarded is
    /// checked here, against the records this run read and the metadata
    /// rows. A record an unreadable pack may hold is unknowable, not
    /// missing (the pack is reported).
    pub(super) fn check_history_references(&mut self) -> Result<(), FsckError> {
        use crate::scm_v1::references::{self, Reference};
        let mut found: Vec<(String, Reference)> = Vec::new();
        for (id, object) in &self.objects {
            for reference in references::object_references(object) {
                found.push((id.clone(), reference));
            }
        }
        // Metadata rows that name records: (table, its key, the record
        // column, the record kind).
        const ROWS: &[(&str, &str, &str, StoreObjectKind)] = &[
            (
                "operations",
                "operation_id",
                "resulting_view_id",
                StoreObjectKind::RepositoryView,
            ),
            (
                "operation_rewrites",
                "operation_id",
                "old_step_id",
                StoreObjectKind::Step,
            ),
            (
                "operation_rewrite_targets",
                "operation_id",
                "new_step_id",
                StoreObjectKind::Step,
            ),
            (
                "step_observations",
                "observation_id",
                "step_id",
                StoreObjectKind::Step,
            ),
            (
                "markers",
                "marker_id",
                "target_step_id",
                StoreObjectKind::Step,
            ),
            (
                "releases",
                "release_id",
                "target_step_id",
                StoreObjectKind::Step,
            ),
            (
                "current_repository_views",
                "space_id",
                "repository_view_id",
                StoreObjectKind::RepositoryView,
            ),
            (
                "products",
                "product_object_id",
                "product_object_id",
                StoreObjectKind::Product,
            ),
        ];
        for (table, key, column, kind) in ROWS {
            let rows: Vec<(String, String)> = {
                let mut statement = self
                    .connection
                    .prepare(&format!("SELECT {key}, {column} FROM {table}"))?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (source, target) in rows {
                match parse_object_id(*kind, &target) {
                    Some(id) => found.push((format!("{table}:{source}"), Reference::Record(id))),
                    None => self.error(
                        FsckFindingCode::ForeignKeyViolation,
                        Some(format!("{table}:{source}")),
                        format!("{column} {target} is not a valid id"),
                    ),
                }
            }
        }
        // Rows that name an Operation (format 8: a history record), and the
        // Space and Worktree rows the history tables' keys guarded.
        const HISTORY_ROWS: &[(&str, &str, &str, &str, &str)] = &[
            (
                "worktrees",
                "worktree_id",
                "active_operation_id",
                "operations",
                "operation_id",
            ),
            (
                "operation_heads",
                "space_id",
                "operation_id",
                "operations",
                "operation_id",
            ),
            (
                "release_publications",
                "release_publication_id",
                "operation_id",
                "operations",
                "operation_id",
            ),
            (
                "recovery_operations",
                "recovery_operation_id",
                "operation_id",
                "operations",
                "operation_id",
            ),
            (
                "step_observations",
                "observation_id",
                "operation_id",
                "operations",
                "operation_id",
            ),
            (
                "operation_parents",
                "operation_id",
                "parent_operation_id",
                "operations",
                "operation_id",
            ),
            (
                "operations",
                "operation_id",
                "space_id",
                "spaces",
                "space_id",
            ),
            (
                "step_observations",
                "observation_id",
                "space_id",
                "spaces",
                "space_id",
            ),
            (
                "step_observations",
                "observation_id",
                "worktree_id",
                "worktrees",
                "worktree_id",
            ),
        ];
        for (table, key, column, target, target_column) in HISTORY_ROWS {
            let rows: Vec<(String, Option<String>)> = {
                let mut statement = self
                    .connection
                    .prepare(&format!("SELECT {key}, {column} FROM {table}"))?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (source, value) in rows {
                let Some(value) = value else {
                    continue;
                };
                if !self.consume_cpu(1) {
                    return Ok(());
                }
                if references::row_exists(self.connection, target, target_column, &value)? {
                    continue;
                }
                // An Operation an unreadable pack may hold is unknown.
                if *target == "operations" && self.history_unknowable(&value) {
                    continue;
                }
                self.error(
                    FsckFindingCode::ForeignKeyViolation,
                    Some(format!("{table}:{source}")),
                    format!("references missing {target} {value}"),
                );
            }
        }
        let catalog = self.compact_catalog().map(|(_, catalog)| catalog);
        for (source, reference) in found {
            let present = match &reference {
                Reference::Record(id) => self
                    .object_ids
                    .get(id.as_str())
                    .is_some_and(|known| known == id),
                Reference::Row(table, column, id) => {
                    references::row_exists(self.connection, table, column, id)?
                }
                // fsck checks history references from their rows, above.
                Reference::History(..) => true,
            };
            if present {
                continue;
            }
            if let Reference::Record(id) = &reference {
                let unknown = match &catalog {
                    Some(catalog) => {
                        self.compact_damaged && catalog.may_hold_unreadably(id).unwrap_or(true)
                    }
                    // No audit catalog: nothing could be read (reported).
                    None => true,
                };
                if unknown {
                    continue;
                }
            }
            let (kind, id) = match &reference {
                Reference::Record(id) => (format!("{:?}", id.kind()), id.to_string()),
                Reference::Row(table, _, id) => ((*table).to_owned(), id.clone()),
                Reference::History(_, id) => ("operations".to_owned(), id.clone()),
            };
            self.error(
                FsckFindingCode::ForeignKeyViolation,
                Some(source),
                format!("references missing {kind} {id}"),
            );
        }
        Ok(())
    }

    /// Format 8 beside an unreadable pack: its derived tables miss what that
    /// pack holds, so a check whose expectation comes from them cannot tell
    /// missing from unknown (council s18 r3, s26 r1).
    pub(super) fn history_incomplete(&self) -> bool {
        self.audit.bitmap_admissions()
            && (self.compact_damaged
                || self.compact_audit_failed
                || !self.unknown_history.is_empty())
    }

    /// Whether `id` may be a record of an unreadable pack, in format 8
    /// (formats 6 and 7 answer from `unreadable_sources`).
    pub(super) fn history_unknowable(&mut self, id: &str) -> bool {
        // A record this run read, and its admission bits, are known.
        if !self.history_incomplete() || self.object_ids.contains_key(id) {
            return false;
        }
        // A history record whose body could not be read.
        if [
            crate::scm_v1::cas::OPERATION_KIND,
            crate::scm_v1::cas::OBSERVATION_KIND,
        ]
        .into_iter()
        .any(|code| {
            self.unknown_history
                .contains(&(code, crate::scm_v1::cas::history_key(code, id)))
        }) {
            return true;
        }
        match self.compact_catalog() {
            Some((_, catalog)) => may_be_unreadable(&catalog, id),
            None => true,
        }
    }

    /// Format 8 beside an unreadable pack: whether `id`'s admission bits
    /// may be there. A copy readable elsewhere does not carry them (a
    /// record republished to another Space after its pack was lost; council
    /// s26 r3).
    pub(super) fn admission_unknowable(&mut self, id: &str) -> bool {
        if !self.history_incomplete() {
            return false;
        }
        match self.compact_catalog() {
            Some((_, catalog)) => may_be_unreadable(&catalog, id),
            None => true,
        }
    }

    /// The audit CAS view and its registered catalog, loaded once per fsck
    /// run from the metadata on disk now (never the process cache) and
    /// reused by every check.
    pub(super) fn compact_catalog(&mut self) -> Option<(NativeCas, Arc<RegisteredCatalog>)> {
        if let Some((cas, catalog)) = &self.compact_audit {
            return Some((cas.clone(), Arc::clone(catalog)));
        }
        if self.compact_audit_failed {
            return None;
        }
        let opened = NativeCas::open_audit(self.audit).and_then(|audit| {
            let cas = audit.into_inner();
            let catalog = cas
                .compact_catalog()
                .cloned()
                .ok_or(crate::scm_v1::CasError::UnsupportedPublication)?;
            Ok((cas, catalog))
        });
        match opened {
            Ok((cas, catalog)) => {
                self.compact_damaged = catalog.damage().is_some();
                self.compact_audit = Some((cas.clone(), Arc::clone(&catalog)));
                Some((cas, catalog))
            }
            Err(error) => {
                self.compact_audit_failed = true;
                self.finding(
                    FsckFindingCode::CasCorruption,
                    FsckSeverity::Error,
                    Some(self.audit.layout().objects_dir().to_path_buf()),
                    None,
                    None,
                    format!("registered CAS catalog cannot be read: {error}"),
                );
                None
            }
        }
    }

    /// Authorizes one physical read of the audit before it happens: one CPU
    /// unit, `envelope_bytes` (decoded) against the object byte budget,
    /// `chunk_bytes` (hashed) against the streamed chunk budget.
    fn authorize_audit_read(&mut self, envelope_bytes: u64, chunk_bytes: u64) -> bool {
        if !self.consume_cpu(1) {
            return false;
        }
        if chunk_bytes > self.chunk_byte_budget_remaining {
            self.exhaust_bytes("streamed pack chunks");
            return false;
        }
        self.chunk_byte_budget_remaining -= chunk_bytes;
        let budget = self.byte_budget_remaining;
        envelope_bytes == 0 || self.reserve_bytes(envelope_bytes, budget, "pack envelopes")
    }

    /// One finding for a catalog answer that failed; later ones are the same
    /// damage and are not repeated.
    fn compact_lookup_failed(&mut self, error: &crate::scm_v1::CasError) {
        if self.compact_lookup_reported {
            return;
        }
        self.compact_lookup_reported = true;
        self.finding(
            FsckFindingCode::CasCorruption,
            FsckSeverity::Error,
            None,
            None,
            None,
            format!("{}: {error}", error.code()),
        );
    }

    /// Non-content `objects` rows (their bytes in a registered pack or a
    /// loose file), then every registered content object.
    pub(super) fn scan_objects_compact(&mut self) -> Result<(), FsckError> {
        let limit = self.row_limit.saturating_add(1);
        let mut rows = {
            let mut statement = self.connection.prepare(
                "SELECT object_id, object_kind, byte_length FROM objects ORDER BY object_id LIMIT ?1",
            )?;
            let rows = statement.query_map([limit as u64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.len() > self.row_limit {
            rows.truncate(self.row_limit);
            self.work_truncated = true;
            self.report.truncated_count = self.report.truncated_count.saturating_add(1);
        }
        let Some((cas, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        // Unreadable registered packs first, once each, so the findings cap
        // keeps room for them; what they make unknowable is not reported again.
        for (path, error) in catalog.unreadable_packs(&cas) {
            self.finding(
                FsckFindingCode::CasCorruption,
                FsckSeverity::Error,
                Some(path),
                None,
                None,
                format!("{}: {error}", error.code()),
            );
        }
        // Every readable pack's keys row, against its index (council s17,
        // B1c): a wrong row would turn a lost pack into a false "absent".
        match catalog.keys_row_problems(self.connection, &cas) {
            Ok(problems) => {
                for (path, problem) in problems {
                    self.finding(
                        FsckFindingCode::CasPackKeysMismatch,
                        FsckSeverity::Error,
                        Some(path),
                        None,
                        None,
                        problem,
                    );
                }
            }
            Err(error) => self.compact_lookup_failed(&error),
        }
        for (id_text, kind_text, byte_length) in rows {
            self.report.checked_rows = self.report.checked_rows.saturating_add(1);
            self.report.checked_objects = self.report.checked_objects.saturating_add(1);
            let Some(kind) = parse_kind(&kind_text).filter(|kind| !is_content(*kind)) else {
                self.finding(
                    FsckFindingCode::ObjectKindMismatch,
                    FsckSeverity::Error,
                    None,
                    Some(id_text),
                    None,
                    format!("format 6 objects row of kind {kind_text}"),
                );
                continue;
            };
            let Some(object_id) = parse_object_id(kind, &id_text) else {
                self.finding(
                    FsckFindingCode::ObjectKindMismatch,
                    FsckSeverity::Error,
                    None,
                    Some(id_text),
                    None,
                    "object ID cannot be parsed for its declared kind",
                );
                continue;
            };
            // An object whose only copy may be in an unreadable pack is
            // unknowable, not missing: it is left out of every check that
            // would read it (and of edge comparisons), the pack being
            // reported above.
            // Only when the unreadable pack's keys row does not exclude it
            // (council s18 r1): otherwise it is missing, and reported.
            if self.compact_damaged
                && catalog
                    .placements(&object_id)
                    .is_ok_and(|placements| placements.is_empty())
                && cas.loose_length(&object_id).ok().flatten().is_none()
                && catalog.may_hold_unreadably(&object_id).unwrap_or(true)
            {
                self.unreadable_sources.insert(id_text);
                continue;
            }
            let location = match cas.locate(&object_id) {
                Ok(Some(location)) => location,
                Ok(None) => {
                    self.finding(
                        FsckFindingCode::MissingCasObject,
                        FsckSeverity::Error,
                        None,
                        Some(id_text),
                        None,
                        "indexed CAS object is missing",
                    );
                    continue;
                }
                Err(error) => {
                    self.finding(
                        FsckFindingCode::CasCorruption,
                        FsckSeverity::Error,
                        None,
                        Some(id_text),
                        None,
                        error.to_string(),
                    );
                    continue;
                }
            };
            if location.length() != byte_length {
                self.finding(
                    FsckFindingCode::ObjectLengthMismatch,
                    FsckSeverity::Error,
                    Some(location.path().to_path_buf()),
                    Some(id_text.clone()),
                    None,
                    format!("indexed length {byte_length}, actual {}", location.length()),
                );
            }
            let budget = self.byte_budget_remaining;
            if !self.reserve_bytes(
                byte_length.max(location.length()),
                budget,
                &format!("CAS object {id_text}"),
            ) {
                break;
            }
            self.read_into_objects(&cas, &catalog, object_id, id_text);
        }
        if self.work_truncated {
            return Ok(());
        }
        // Every registered physical copy, at its own place: a damaged copy is
        // reported even when a sound duplicate serves its logical reads. Each
        // block (or loose object) is authorized against the budgets before it
        // is read: one CPU unit, its envelope bytes against the object byte
        // budget and its chunk bytes against the streamed chunk budget, as
        // the format 5 scan charges objects.
        let mut damaged_copies = HashSet::new();
        let mut stopped = false;
        catalog.audit_registered(&cas, true, |event| {
            let copy = match event {
                AuditEvent::Block {
                    envelope_bytes,
                    chunk_bytes,
                } => {
                    let allowed = self.authorize_audit_read(envelope_bytes, chunk_bytes);
                    stopped = !allowed;
                    return allowed;
                }
                AuditEvent::Loose { envelope, bytes } => {
                    let allowed = if envelope {
                        self.authorize_audit_read(bytes, 0)
                    } else {
                        self.authorize_audit_read(0, bytes)
                    };
                    stopped = !allowed;
                    return allowed;
                }
                AuditEvent::Copy(copy) => copy,
            };
            self.report.checked_objects = self.report.checked_objects.saturating_add(1);
            // Reported before the envelope scan.
            if copy.pack_level {
                return true;
            }
            if let Err(error) = copy.result {
                self.finding(
                    FsckFindingCode::CasCorruption,
                    FsckSeverity::Error,
                    Some(copy.path),
                    Some(copy.object.clone()),
                    None,
                    format!("{}{}: {error}", copy.message_prefix, error.code()),
                );
                damaged_copies.insert(copy.object);
            }
            true
        });
        if stopped || self.work_truncated {
            return Ok(());
        }
        // Logical objects for closure and reachability. A chunk whose every
        // copy the audit verified is present without a second read; one with
        // a damaged copy is present only if another copy reads.
        let (mut content, mut errors) = catalog.content_objects();
        // Format 8 keeps no `objects` rows: every registered record is read.
        if self.audit.bitmap_admissions() {
            let (history, history_errors) = catalog.history_objects();
            content.extend(history);
            errors.extend(history_errors);
        }
        for error in errors {
            self.finding(
                FsckFindingCode::CasCorruption,
                FsckSeverity::Error,
                None,
                None,
                None,
                format!("{}: {error}", error.code()),
            );
        }
        for object_id in content {
            if self.object_ids.contains_key(object_id.as_str()) {
                continue;
            }
            // No CPU charge here: the audit already charged every block, and
            // a chunk it verified is only recorded present.
            let id_text = object_id.to_string();
            if matches!(object_id, StoreObjectId::ChunkRaw(_)) && !damaged_copies.contains(&id_text)
            {
                self.object_ids.insert(id_text, object_id);
                continue;
            }
            let length = match catalog.stored_length(&object_id) {
                Ok(length) => length.unwrap_or_default(),
                Err(error) => {
                    self.finding(
                        FsckFindingCode::CasCorruption,
                        FsckSeverity::Error,
                        None,
                        Some(id_text),
                        None,
                        format!("{}: {error}", error.code()),
                    );
                    continue;
                }
            };
            if matches!(object_id, StoreObjectId::ChunkRaw(_)) {
                if length > 64 * 1024 * 1024 || length > self.chunk_byte_budget_remaining {
                    self.exhaust_bytes(&format!("streamed Chunk {id_text}"));
                    break;
                }
                self.chunk_byte_budget_remaining -= length;
            } else {
                let budget = self.byte_budget_remaining;
                if !self.reserve_bytes(length, budget, &format!("CAS object {id_text}")) {
                    break;
                }
            }
            self.read_into_objects(&cas, &catalog, object_id, id_text);
        }
        Ok(())
    }

    fn read_into_objects(
        &mut self,
        cas: &NativeCas,
        catalog: &RegisteredCatalog,
        object_id: StoreObjectId,
        id_text: String,
    ) {
        match cas.read_registered(catalog, &object_id) {
            Ok(object) if object.object_id() == object_id => {
                self.object_ids.insert(id_text.clone(), object_id);
                if !matches!(object, CanonicalObject::ChunkRaw(_)) {
                    self.objects.insert(id_text, object);
                }
            }
            Ok(_) => self.finding(
                FsckFindingCode::ObjectKindMismatch,
                FsckSeverity::Error,
                None,
                Some(id_text),
                None,
                "decoded CAS kind or identity differs from its registration",
            ),
            Err(error) => self.finding(
                FsckFindingCode::CasCorruption,
                FsckSeverity::Error,
                None,
                Some(id_text),
                None,
                format!("{}: {error}", error.code()),
            ),
        }
    }

    /// The FileId-section projection of each manifest (file length, and the
    /// manifest listed for its FileId) and its streamed reconstruction.
    pub(super) fn check_manifest_projections_compact(&mut self) -> Result<(), FsckError> {
        let Some((_, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        let damaged = self.compact_damaged;
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
            // Beside an unreadable registered pack, a listing its keys row
            // does not exclude is unknown: that pack is reported once by the
            // audit, and only that listing is skipped (council s18 r1).
            let listed = match catalog.file_manifests(manifest.file_id()) {
                Ok(entries) => entries
                    .into_iter()
                    .find(|entry| &entry.id == manifest.id())
                    .map(|entry| entry.file_length),
                Err(_) if damaged && catalog.file_may_be_unreadable(manifest.file_id()) => continue,
                Err(error) => {
                    self.compact_lookup_failed(&error);
                    continue;
                }
            };
            if listed != Some(manifest.byte_length()) {
                self.projection_divergence(
                    "pack FileId section",
                    &id,
                    "manifest identity/length projection differs from canonical CAS",
                );
            }
        }
        Ok(())
    }

    /// Whether some registered manifest exists for a FileId.
    pub(super) fn file_present_compact(
        &mut self,
        file_id: &layrs_core::scm::FileId,
    ) -> Result<bool, FsckError> {
        let Some((_, catalog)) = self.compact_catalog() else {
            return Ok(true);
        };
        // Beside an unreadable registered pack, presence its keys row does
        // not exclude is unknown, not absent: that pack is reported once,
        // and Tree closure is not judged on it (council s18 r1).
        match catalog.file_manifests(file_id) {
            Ok(entries) => Ok(!entries.is_empty()),
            Err(_) if self.compact_damaged && catalog.file_may_be_unreadable(file_id) => Ok(true),
            Err(error) => {
                self.compact_lookup_failed(&error);
                Ok(true)
            }
        }
    }

    /// Content admission: every bitmap and loose admission valid, every
    /// chunk of an admitted manifest admitted to the same Space, and every
    /// reference that may name content (pins, GC roots, recovery paths)
    /// resolving to a registered or `objects` row object.
    pub(super) fn check_content_admissions(&mut self) -> Result<(), FsckError> {
        let Some((_, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        // Beside an unreadable registered pack, an admission answer its keys
        // row does not exclude is unknown: that pack is reported once by the
        // audit, and only that answer is skipped (council s18 r1).
        let damaged = self.compact_damaged;
        let spaces = query_strings_no_param(
            self.connection,
            "SELECT space_id FROM space_pack_admissions UNION SELECT space_id FROM space_loose_admissions",
        )?;
        let manifests: Vec<_> = self
            .objects
            .values()
            .filter_map(|object| match object {
                CanonicalObject::FileManifest(value) => Some(value.clone()),
                _ => None,
            })
            .collect();
        for space in spaces {
            let admissions = match catalog.space_admissions(self.connection, &space) {
                Ok(admissions) => admissions,
                Err(error) => {
                    self.finding(
                        FsckFindingCode::AdmissionClosureBroken,
                        FsckSeverity::Error,
                        None,
                        None,
                        Some(space),
                        error.to_string(),
                    );
                    continue;
                }
            };
            // Index and bitmap lookups only, bounded by the manifests already
            // budgeted by the scan: like format 5's SQL closure check, this
            // check charges no CPU units of its own.
            for manifest in &manifests {
                let object = StoreObjectId::FileManifest(manifest.id().clone());
                match catalog.admitted(&admissions, &object) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) if damaged && catalog.may_hold_unreadably(&object).unwrap_or(true) => {
                        continue;
                    }
                    Err(error) => {
                        self.compact_lookup_failed(&error);
                        continue;
                    }
                }
                for chunk in manifest.chunks() {
                    let chunk_object = StoreObjectId::ChunkRaw(chunk.chunk_id().clone());
                    let admitted = match catalog.admitted(&admissions, &chunk_object) {
                        Ok(admitted) => admitted,
                        Err(_)
                            if damaged
                                && catalog.may_hold_unreadably(&chunk_object).unwrap_or(true) =>
                        {
                            continue;
                        }
                        Err(error) => {
                            self.compact_lookup_failed(&error);
                            continue;
                        }
                    };
                    if !admitted {
                        self.finding(
                            FsckFindingCode::AdmissionClosureBroken,
                            FsckSeverity::Error,
                            None,
                            Some(manifest.id().to_string()),
                            Some(space.clone()),
                            format!(
                                "transitive object {} is not admitted to the same Space",
                                chunk.chunk_id()
                            ),
                        );
                    }
                }
            }
        }
        for (table, sql) in [
            ("object_pins", "SELECT object_id FROM object_pins"),
            ("gc_roots", "SELECT object_id FROM gc_roots"),
            (
                "recovery_paths",
                "SELECT expected_object_id FROM recovery_paths WHERE expected_object_id IS NOT NULL",
            ),
        ] {
            for id in query_strings_no_param(self.connection, sql)? {
                if !self.consume_cpu(1) {
                    return Ok(());
                }
                // Beside an unreadable pack, an object it may hold is
                // unknown, not absent (council s18 r2).
                if !self.object_ids.contains_key(&id)
                    && !(damaged
                        && (self.unreadable_sources.contains(&id)
                            || may_be_unreadable(&catalog, &id)))
                {
                    self.finding(
                        FsckFindingCode::ForeignKeyViolation,
                        FsckSeverity::Error,
                        None,
                        Some(id),
                        None,
                        format!(
                            "{table} references an object that is neither registered nor in objects"
                        ),
                    );
                }
            }
        }
        Ok(())
    }
}

/// Whether an unreadable registered pack may hold the object or history
/// record (packs v4) named `id`, whatever its kind: its keys row does not
/// exclude it as any of them.
fn may_be_unreadable(catalog: &RegisteredCatalog, id: &str) -> bool {
    use layrs_store::scm::StoreObjectKind;
    let history = [
        crate::scm_v1::cas::OPERATION_KIND,
        crate::scm_v1::cas::OBSERVATION_KIND,
    ]
    .into_iter()
    .any(|code| {
        let key = crate::scm_v1::cas::history_key(code, id);
        catalog
            .unknown_to(|| Ok(vec![crate::scm_v1::cas::keys::explicit_key(code, &key)]))
            .map_or(true, |unknown| unknown.is_some())
    });
    history
        || [
            StoreObjectKind::ChunkRaw,
            StoreObjectKind::FileManifest,
            StoreObjectKind::Tree,
            StoreObjectKind::Symlink,
            StoreObjectKind::Tombstone,
            StoreObjectKind::Step,
            StoreObjectKind::RepositoryView,
            StoreObjectKind::Conflict,
            StoreObjectKind::Product,
        ]
        .into_iter()
        .filter_map(|kind| super::parse_object_id(kind, id))
        .any(|object| catalog.may_hold_unreadably(&object).unwrap_or(true))
}
