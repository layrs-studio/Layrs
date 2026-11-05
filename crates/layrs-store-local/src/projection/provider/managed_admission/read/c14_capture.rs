//! Closed local Capture suffix on the immutable C14 Ready generation.
//! No source/family opener, byte authority or provenance-dispatching loader.
use super::*;
use crate::projection::{VersionedProjection, capture::CapturedChange};
use layrs_core::scm::{FileId, FileManifestId};
use rusqlite::{DatabaseName, OptionalExtension, TransactionState};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const MAX_LOCAL_ROWS: u64 = 100_000;
// Revalidation is bounded by aggregate canonical payload, rather than by a
// 100k-by-8MiB product. The immutable Ready snapshot is read once per walk.
pub(super) const MAX_LOCAL_CAPTURE_PAYLOAD: u64 = 64 * 1024 * 1024;
pub(super) const MAX_CONTENT_ROWS: u64 = 1_100_010;

fn local_capture_inventory(c: &Connection, ready_sequence: u64) -> Result<(u64, u64)> {
    let (count, bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(payload)),0) FROM projection_captures WHERE sequence>?1",
        [ready_sequence],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count > MAX_LOCAL_ROWS || bytes > MAX_LOCAL_CAPTURE_PAYLOAD {
        return Err(invalid(
            "local denial Capture history exceeds aggregate bound",
        ));
    }
    Ok((count, bytes))
}

pub(super) fn bounded_count(c: &Connection, table: &str, limit: u64) -> Result<u64> {
    let count = c.query_row(
        &format!("SELECT count(*) FROM (SELECT 1 FROM \"{table}\" LIMIT ?1)"),
        [limit + 1],
        |r| r.get(0),
    )?;
    if count > limit {
        return Err(invalid("local denial Capture inventory exceeds bound"));
    }
    Ok(count)
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl VerifiedAdmittedImport<'_> {
    /// Prove the exact ordinary append fits every local inventory bound before
    /// Capture's first durable reservation. Existing same-image deduplication
    /// needs no row/payload/claim margin and remains usable at the limit.
    pub(in crate::projection::provider) fn preflight_local_capture(
        &self,
        state: &VersionedProjection,
        changes: &[CapturedChange],
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if self.ready().plan.denial_coverage.is_none() {
            return Ok(());
        }
        cancellation::check(cancel)?;
        if self.connection.is_autocommit()
            || self.private_schema != 9
            || state.generation != self.record.boundaries.ready_generation
            || state.digest != self.ready().plan.placement_digest
        {
            return Err(invalid(
                "local denial Capture preflight escaped Ready scope",
            ));
        }
        let c = self.connection;
        let Some(payload) = crate::projection::capture::storage::local_capture_append_payload(
            c,
            state,
            &self.marker.identity,
            changes,
        )?
        else {
            return Ok(());
        };
        let (local_count, local_bytes) =
            local_capture_inventory(c, self.record.boundaries.ready_capture_sequence)?;
        if local_count >= MAX_LOCAL_ROWS
            || local_bytes
                .checked_add(payload.len() as u64)
                .is_none_or(|n| n > MAX_LOCAL_CAPTURE_PAYLOAD)
            || bounded_count(c, "projection_captures", MAX_CONTENT_ROWS)? >= MAX_CONTENT_ROWS
        {
            return Err(invalid("local denial Capture append exceeds history bound"));
        }
        let replacements = changes
            .iter()
            .filter_map(|change| change.replacement.as_ref())
            .map(|file| file.file_id.clone())
            .collect::<BTreeSet<_>>();
        let mut query = c.prepare(
            "SELECT owner FROM projection_claims WHERE kind='captured-content' ORDER BY owner",
        )?;
        let owners = query
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        let new_claims = replacements
            .iter()
            .filter(|file| !owners.contains(file.as_str()))
            .count() as u64;
        let mut new_mappings = 0u64;
        for file in &replacements {
            cancellation::check(cancel)?;
            if !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM projection_content WHERE file_id=?1)",
                [file.as_str()],
                |r| r.get::<_, bool>(0),
            )? {
                new_mappings += 1;
            }
        }
        if bounded_count(c, "projection_claims", MAX_LOCAL_ROWS + 1)?
            .checked_add(new_claims)
            .is_none_or(|n| n > MAX_LOCAL_ROWS + 1)
            || bounded_count(c, "projection_content", MAX_CONTENT_ROWS)?
                .checked_add(new_mappings)
                .is_none_or(|n| n > MAX_CONTENT_ROWS)
        {
            return Err(invalid("local denial Capture append exceeds content bound"));
        }
        cancellation::check(cancel)
    }

    /// Reinterpret only the permitted local Capture tail. The immutable import
    /// proof is already complete; this grants no writer, checkout or family
    /// capability. Terminal callers additionally require the exact empty tail.
    pub(in crate::projection::provider) fn require_local_capture_suffix(
        &self,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        let c = self.connection;
        if c.is_autocommit()
            || self.private_schema != 9
            || self.ready().plan.denial_coverage.is_none()
            || self.receipt().denial_coverage.is_none()
            || !self.outgoing.intents().is_empty()
            || self.outgoing.receipt_count() != 0
        {
            return Err(invalid("local denial Capture requires Provider9 C14 scope"));
        }
        let boundaries = &self.record.boundaries;
        let (singleton, head, last): (u64, u64, u64) = c.query_row(
            "SELECT singleton,generation,(SELECT coalesce(max(sequence),0) FROM projection_captures) FROM projection_head",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if singleton != 1
            || head != boundaries.ready_generation
            || last < boundaries.ready_capture_sequence
            || last - boundaries.ready_capture_sequence > MAX_LOCAL_ROWS
        {
            return Err(invalid("local denial Capture escaped fixed Ready frontier"));
        }
        let (local_count, _) = local_capture_inventory(c, boundaries.ready_capture_sequence)?;
        if local_count != last - boundaries.ready_capture_sequence {
            return Err(invalid("local denial Capture sequence inventory differs"));
        }
        // Local Restore rows are bounded before any journal is decoded.
        let restore_rows = super::c14_restore::read_local_restore_rows(
            c,
            boundaries.ready_capture_sequence,
            cancel,
        )?;

        // Incoming mappings and Ready additions remain exact. New mappings
        // belong only to same-file captured-content claims; the family reader
        // independently authenticates every actual protection and its bytes.
        let mut mappings = self.prefix.copied_content_mappings(c, cancel)?;
        for row in &self.frozen.new_content {
            if mappings
                .insert(row.file_id.clone(), (row.manifest_id.clone(), row.bytes))
                .is_some_and(|old| old != (row.manifest_id.clone(), row.bytes))
            {
                return Err(invalid("local denial Capture changed frozen mapping"));
            }
        }
        let frozen_mappings = mappings;
        let content_count = bounded_count(c, "projection_content", MAX_CONTENT_ROWS)?;
        if c.query_row("SELECT EXISTS(SELECT 1 FROM projection_content WHERE length(CAST(file_id AS BLOB))!=71 OR length(CAST(manifest_id AS BLOB))!=71 OR bytes<0 OR bytes>1073741824)", [], |r| r.get::<_,bool>(0))? {
            return Err(invalid("local denial Capture mapping exceeds bound"));
        }
        let mut statement =
            c.prepare("SELECT file_id,manifest_id,bytes FROM projection_content ORDER BY file_id")?;
        let mut rows = statement.query([])?;
        let mut actual = BTreeMap::new();
        while let Some(row) = rows.next()? {
            cancellation::check(cancel)?;
            let file =
                FileId::parse(row.get::<_, String>(0)?).map_err(|e| invalid(e.to_string()))?;
            let manifest = FileManifestId::parse(row.get::<_, String>(1)?)
                .map_err(|e| invalid(e.to_string()))?;
            let bytes = row.get::<_, u64>(2)?;
            actual.insert(file, (manifest, bytes));
        }
        for (file, expected) in &frozen_mappings {
            if actual.get(file) != Some(expected) {
                return Err(invalid("local denial Capture lost frozen mapping"));
            }
        }

        let claim_count = bounded_count(c, "projection_claims", MAX_LOCAL_ROWS + 1)?;
        if c.query_row("SELECT EXISTS(SELECT 1 FROM projection_claims WHERE length(CAST(root_id AS BLOB))!=64 OR length(CAST(digest AS BLOB))!=64 OR length(CAST(kind AS BLOB))>32 OR length(CAST(owner AS BLOB))>128)", [], |r| r.get::<_,bool>(0))? {
            return Err(invalid("local denial Capture claim exceeds bound"));
        }
        let mut claims =
            c.prepare("SELECT root_id,digest,kind,owner FROM projection_claims ORDER BY root_id")?;
        let mut rows = claims.query([])?;
        let mut captured = BTreeSet::new();
        let mut imported = 0u64;
        // Local Restore protections: one `restore` claim per pending journal,
        // one `restore-history` claim per local history row. Ownership is
        // checked once the journals are interpreted below.
        let mut restore_claims = Vec::new();
        let mut history_claims = BTreeSet::new();
        while let Some(row) = rows.next()? {
            cancellation::check(cancel)?;
            let root: String = row.get(0)?;
            let hash: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let owner: String = row.get(3)?;
            if root == self.record.root_id {
                if hash != root || kind != "provider-import" || owner != self.record.transaction_id
                {
                    return Err(invalid("local denial Capture changed import claim"));
                }
                imported += 1;
            } else if kind == "restore" || kind == "restore-history" {
                if root != hash
                    || !digest(&root)
                    || !super::c14_restore::canonical_operation(&owner)
                    || (kind == "restore-history"
                        && (!restore_rows.history.contains_key(&owner)
                            || !history_claims.insert(owner.clone())))
                {
                    return Err(invalid("local denial Restore claim is not local history"));
                }
                if kind == "restore" {
                    restore_claims.push(owner);
                }
            } else {
                let owner = FileId::parse(owner).map_err(|e| invalid(e.to_string()))?;
                if kind != "captured-content"
                    || root != hash
                    || !digest(&root)
                    || !actual.contains_key(&owner)
                    || !captured.insert(owner)
                {
                    return Err(invalid(
                        "local denial Capture claim is not captured content",
                    ));
                }
            }
        }
        if imported != 1 {
            return Err(invalid("local denial Capture import claim absent"));
        }
        for file in actual.keys() {
            if !frozen_mappings.contains_key(file) && !captured.contains(file) {
                return Err(invalid("local denial Capture mapping has no content claim"));
            }
        }

        // Origin-only interpretation validates canonical payloads, predecessor
        // links and writable/Reserved permissions against the fixed snapshot.
        // It never dispatches back through admitted capture provenance.
        let mut referenced = BTreeSet::new();
        let history_sequences = restore_rows
            .history
            .values()
            .map(|(sequence, _)| *sequence)
            .collect::<BTreeSet<_>>();
        let mut history_patches = BTreeMap::new();
        let mut base = None;
        let journals = !restore_rows.history.is_empty() || restore_rows.pending.is_some();
        if last > boundaries.ready_capture_sequence {
            let first = boundaries
                .ready_capture_sequence
                .checked_add(1)
                .ok_or_else(|| invalid("local denial Capture sequence overflow"))?;
            crate::projection::capture::storage::visit_metadata_range_for_origin_with_base(
                c,
                first,
                last,
                &self.marker.identity,
                boundaries.ready_generation,
                &self.ready().plan.placement_digest,
                || cancellation::check(cancel).map_err(Into::into),
                |patch, snapshot| {
                    // Plain Capture suffixes never pay for a snapshot copy.
                    if journals && base.is_none() {
                        base = Some(snapshot.clone());
                    }
                    if history_sequences.contains(&patch.sequence) {
                        history_patches.insert(patch.sequence, patch.clone());
                    }
                    for file in patch
                        .changes
                        .iter()
                        .filter_map(|change| change.replacement.as_ref())
                    {
                        referenced.insert(file.file_id.clone());
                        if !captured.contains(&file.file_id)
                            || actual
                                .get(&file.file_id)
                                .is_none_or(|(_, bytes)| *bytes != file.bytes)
                        {
                            return Err(crate::projection::invalid(
                                "local denial Capture replacement is not retained",
                            ));
                        }
                    }
                    Ok(())
                },
            )?;
        }
        let pending = self.require_local_restore_journals(
            &restore_rows,
            &history_patches,
            base.as_ref(),
            last,
            &actual,
            &frozen_mappings,
            &captured,
            &mut referenced,
            cancel,
        )?;

        // Capture reserves before its private transaction and can leave an
        // active budget for an explicit retry. Files/claims may also precede
        // the final Capture INSERT inside that same writer transaction.
        let budget_count = bounded_count(c, "projection_budgets", 1)?;
        let mut active_budget = false;
        let mut restore_budget = None;
        let write = c.transaction_state(Some(DatabaseName::Main))? == TransactionState::Write;
        if budget_count == 1 {
            let (operation,reservation,bytes,witness,state):(String,String,u64,String,String) = c.query_row(
                "SELECT operation,reservation,bytes,witness,state FROM projection_budgets WHERE singleton=1 AND length(CAST(operation AS BLOB))<=128 AND length(CAST(reservation AS BLOB))<=128 AND length(CAST(witness AS BLOB))=64",
                [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
            ).optional()?.ok_or_else(|| invalid("local denial Capture budget exceeds bound"))?;
            // budget_for adds 2048 per distinct file and 256 per rounded-up
            // chunk. Rounding each of at most1024 files can add one extra
            // chunk per file beyond rounding their combined byte length.
            let maximum = crate::projection::capture::MAX_CAPTURE_BYTES
                + 1024 * (2048 + 256)
                + crate::projection::capture::MAX_CAPTURE_BYTES
                    .div_ceil(crate::projection::capture::CAPTURE_CHUNK_BYTES as u64)
                    * 256
                + 65536;
            let restore = operation
                .strip_prefix("restore:")
                .filter(|id| super::c14_restore::canonical_operation(id));
            let within = if restore.is_some() {
                bytes <= crate::projection::retention::MAX_RESTORE_BUDGET_BYTES
            } else {
                operation == format!("capture:{}", self.ready().plan.placement_digest)
                    && bytes <= maximum
            };
            if !within
                || reservation.is_empty()
                || reservation.contains('\0')
                || bytes < 65536
                || !digest(&witness)
                || !matches!(state.as_str(), "active" | "committed")
            {
                return Err(invalid(
                    "local denial Capture budget is not a Capture reservation",
                ));
            }
            active_budget = state == "active";
            restore_budget = restore.map(|id| (id.to_owned(), active_budget));
        }
        // Restore journals and checkout staging are owned by exactly one
        // restore budget: reserved, then staged (prejournal), then accepted
        // (pending), then committed by finish or cancellation.
        let owned = match &restore_budget {
            None => pending.is_none() && restore_rows.prejournal.is_none(),
            Some((id, true)) => {
                !restore_rows.history.contains_key(id)
                    && match (&pending, &restore_rows.prejournal) {
                        (None, None) => true,
                        (None, Some(owner)) => {
                            *owner == format!("restore:{id}")
                                && crate::projection::retention::require_restore_staging(c, owner)?
                        }
                        (Some(operation), None) => operation == id,
                        (Some(_), Some(_)) => false,
                    }
            }
            // Finish commits the budget before its history row, inside one
            // writer transaction; otherwise a committed budget owns nothing.
            Some((id, false)) => {
                restore_rows.prejournal.is_none()
                    && pending
                        .as_ref()
                        .is_none_or(|operation| write && operation == id)
            }
        };
        if !owned {
            return Err(invalid(
                "local denial Restore journal is not owned by its budget",
            ));
        }
        // Accept writes the pending journal before its exact protection claim
        // in the same writer transaction; history transfers it atomically.
        let claim_owned = match (&pending, restore_claims.as_slice()) {
            (None, []) => true,
            (Some(operation), [owner]) => operation == owner,
            (Some(_), []) => write && active_budget,
            _ => false,
        };
        if !claim_owned || history_claims.len() != restore_rows.history.len() {
            return Err(invalid(
                "local denial Restore protection claim differs from journals",
            ));
        }
        if captured.iter().any(|file| !referenced.contains(file)) && (!active_budget || !write) {
            return Err(invalid(
                "local denial Capture has unreferenced content claims",
            ));
        }

        let mut counts = self.denial_terminal_counts()?;
        for (table, count) in &mut counts {
            *count = match *table {
                "projection_captures" => count
                    .checked_add(last - boundaries.ready_capture_sequence)
                    .filter(|n| *n <= 1_100_010)
                    .ok_or_else(|| invalid("local denial Capture count overflow"))?,
                "projection_content" => content_count,
                "projection_claims" => claim_count,
                "projection_restore_history" => count
                    .checked_add(restore_rows.history.len() as u64)
                    .filter(|n| *n <= 1_100_010)
                    .ok_or_else(|| invalid("local denial Restore count overflow"))?,
                _ => *count,
            };
        }
        counts.push(("projection_budgets", budget_count));
        counts.push(("projection_restore_pending", u64::from(pending.is_some())));
        counts.push((
            "projection_prejournal",
            u64::from(restore_rows.prejournal.is_some()),
        ));
        for (kind, name, _) in super::super::super::database_io::schema_inventory(c)? {
            cancellation::check(cancel)?;
            if kind != "table" {
                continue;
            }
            let expected = counts
                .iter()
                .find(|(table, _)| *table == name)
                .map_or(0, |(_, n)| *n);
            if bounded_count(c, &name, expected)? != expected {
                return Err(invalid(
                    "local denial Capture table has missing or extra rows",
                ));
            }
        }
        if !c.query_row("SELECT backend='ordinary' AND mount_id IS NULL FROM projection_backend WHERE singleton=1", [], |r| r.get::<_,bool>(0))?
            || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_,u64>(0))? != 1
            || c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)", [], |r| r.get::<_,bool>(0))?
        {
            return Err(invalid("local denial Capture backend/foreign keys differ"));
        }
        cancellation::check(cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::provider::process_profile::tests::child;

    #[test]
    #[ignore = "UNQUALIFIED Capture suffix inventory SQL bounds; ROOT review/release child120 required"]
    fn local_capture_inventory_bounds_payload_before_loading_and_excludes_imported_prefix() {
        child::run(
            concat!(
                module_path!(),
                "::local_capture_inventory_bounds_payload_before_loading_and_excludes_imported_prefix"
            ),
            120,
            || {
                child::declare();
                let c = Connection::open_in_memory().unwrap();
                c.execute_batch("CREATE TABLE projection_captures(sequence INTEGER PRIMARY KEY,payload BLOB NOT NULL) STRICT;").unwrap();
                // The prefix is outside this fixed-Ready suffix allowance.
                c.execute(
                    "INSERT INTO projection_captures VALUES(1,zeroblob(?1))",
                    [MAX_LOCAL_CAPTURE_PAYLOAD + 1],
                )
                .unwrap();
                assert_eq!(local_capture_inventory(&c, 1).unwrap(), (0, 0));
                // Length aggregation must reject before any malformed payload
                // can be read or parsed. These rows carry no admission proof.
                c.execute(
                    "INSERT INTO projection_captures VALUES(2,zeroblob(?1))",
                    [MAX_LOCAL_CAPTURE_PAYLOAD],
                )
                .unwrap();
                assert_eq!(
                    local_capture_inventory(&c, 1).unwrap(),
                    (1, MAX_LOCAL_CAPTURE_PAYLOAD)
                );
                c.execute("INSERT INTO projection_captures VALUES(3,x'00')", [])
                    .unwrap();
                let error = local_capture_inventory(&c, 1).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("history exceeds aggregate bound")
                );
                c.execute("DELETE FROM projection_captures", []).unwrap();
                c.execute_batch("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<100000) INSERT INTO projection_captures SELECT i,x'00' FROM n;").unwrap();
                assert_eq!(
                    local_capture_inventory(&c, 0).unwrap(),
                    (MAX_LOCAL_ROWS, MAX_LOCAL_ROWS)
                );
                c.execute("INSERT INTO projection_captures VALUES(100001,x'00')", [])
                    .unwrap();
                let error = local_capture_inventory(&c, 0).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("history exceeds aggregate bound")
                );
            },
        );
    }
}
