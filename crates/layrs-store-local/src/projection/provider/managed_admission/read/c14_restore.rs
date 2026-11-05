//! Local Restore suffix on the immutable C14 Ready generation: bounded raw
//! rows, DATA-only journal interpretation and forced-append funding. This
//! grants no writer, family, content, checkout or provenance authority.
use super::c14_capture::{bounded_count, MAX_CONTENT_ROWS, MAX_LOCAL_CAPTURE_PAYLOAD, MAX_LOCAL_ROWS};
use super::*;
use crate::projection::{
    capture::CapturedPatch,
    restore::{LocalRestorePlan, RestoreDestination, RestorePreview, RestoreSource},
    ProjectionSnapshot, VersionedProjection,
};
use layrs_core::scm::{FileId, FileManifestId};
use rusqlite::OptionalExtension;
use std::collections::{BTreeMap, BTreeSet};

/// Aggregate canonical bytes of local history plus the one pending journal.
pub(super) const MAX_LOCAL_RESTORE_PAYLOAD: u64 = 64 * 1024 * 1024;

/// Bound local history rows and journal bytes before any payload is read.
/// Imported history (at or below the Ready capture) is outside this suffix.
pub(super) fn local_restore_inventory(c: &Connection, ready_sequence: u64) -> Result<(u64, u64)> {
    let (count, history, pending): (u64, u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(payload)),0),(SELECT coalesce(sum(length(payload)),0) FROM projection_restore_pending) FROM projection_restore_history WHERE capture_sequence>?1",
        [ready_sequence],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    match history.checked_add(pending) {
        Some(bytes) if count <= MAX_LOCAL_ROWS && bytes <= MAX_LOCAL_RESTORE_PAYLOAD => {
            Ok((count, bytes))
        }
        _ => Err(invalid("local denial Restore history exceeds aggregate bound")),
    }
}

/// `<u128 nanos>-<u32 pid>` exactly as Restore preview mints it.
pub(super) fn canonical_operation(id: &str) -> bool {
    fn canonical<T: std::str::FromStr + std::fmt::Display>(value: &str) -> bool {
        !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
            && value
                .parse::<T>()
                .ok()
                .is_some_and(|n| n.to_string() == value)
    }
    id.split_once('-')
        .is_some_and(|(nanos, pid)| canonical::<u128>(nanos) && canonical::<u32>(pid))
}

/// Raw bounded local Restore rows of one retained snapshot.
pub(super) struct LocalRestoreRows {
    pub(super) history: BTreeMap<String, (u64, Vec<u8>)>,
    pub(super) pending: Option<Vec<u8>>,
    pub(super) prejournal: Option<String>,
}

pub(super) fn read_local_restore_rows(
    c: &Connection,
    ready_sequence: u64,
    cancel: Option<&Cancellation>,
) -> Result<LocalRestoreRows> {
    let (count, _) = local_restore_inventory(c, ready_sequence)?;
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_restore_history WHERE capture_sequence>?1 AND (length(CAST(operation_id AS BLOB))=0 OR length(CAST(operation_id AS BLOB))>96 OR length(payload)=0))",
        [ready_sequence],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("local denial Restore history row exceeds bound"));
    }
    let mut statement = c.prepare(
        "SELECT operation_id,capture_sequence,payload FROM projection_restore_history WHERE capture_sequence>?1 ORDER BY operation_id",
    )?;
    let mut rows = statement.query([ready_sequence])?;
    let mut history = BTreeMap::new();
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        history.insert(
            row.get::<_, String>(0)?,
            (row.get::<_, u64>(1)?, row.get::<_, Vec<u8>>(2)?),
        );
    }
    if history.len() as u64 != count {
        return Err(invalid("local denial Restore history inventory differs"));
    }
    let pending = c
        .query_row(
            "SELECT payload FROM projection_restore_pending WHERE singleton=1",
            [],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    let prejournal = match bounded_count(c, "projection_prejournal", 1)? {
        0 => None,
        _ => Some(c.query_row(
            "SELECT operation FROM projection_prejournal WHERE length(CAST(operation AS BLOB))<=128",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| invalid("local denial Restore prejournal exceeds bound"))?),
    };
    cancellation::check(cancel)?;
    Ok(LocalRestoreRows {
        history,
        pending,
        prejournal,
    })
}

impl VerifiedAdmittedImport<'_> {
    fn local_destination(&self) -> RestoreDestination {
        RestoreDestination {
            identity: self.marker.identity.clone(),
            checkout_identity: self.marker.checkout_identity().to_owned(),
        }
    }

    /// A local journal's source is itself a later local fact: a post-Ready
    /// Capture or a local inverse, strictly older than `before`.
    /// Versions a C14 store may restore: its own fixed Ready generation, or a
    /// post-Ready local Capture strictly older than `before`.
    pub(super) fn is_local_version(
        &self,
        version: &layrs_core::projection::ProjectionVersion,
        before: u64,
    ) -> bool {
        use layrs_core::projection::ProjectionVersion;
        let own = &self.marker.identity.worktree_id;
        match version {
            ProjectionVersion::Generation {
                worktree_id,
                generation,
                digest,
            } => {
                worktree_id == own
                    && *generation == self.record.boundaries.ready_generation
                    && *digest == self.ready().plan.placement_digest
            }
            ProjectionVersion::Capture {
                worktree_id,
                sequence,
                ..
            } => {
                worktree_id == own
                    && *sequence > self.record.boundaries.ready_capture_sequence
                    && *sequence < before
            }
            ProjectionVersion::Remote { .. } => false,
        }
    }

    fn require_local_source(
        &self,
        source: &RestoreSource,
        before: u64,
        history: &BTreeMap<String, (u64, Vec<u8>)>,
    ) -> Result<()> {
        let ready = self.record.boundaries.ready_capture_sequence;
        let valid = match source {
            RestoreSource::Version { version, .. } => self.is_local_version(version, before),
            RestoreSource::Capture { sequence, .. } => *sequence > ready && *sequence < before,
            RestoreSource::Inverse { operation_id } => history
                .get(operation_id)
                .is_some_and(|(sequence, _)| *sequence < before),
        };
        if !valid {
            return Err(invalid("local denial Restore source escaped Capture suffix"));
        }
        Ok(())
    }

    /// Interpret every local history journal against its own forced capture
    /// and the one pending journal against the fixed Ready snapshot. Every
    /// before/after file must be retained: by an exact captured-content claim
    /// or, for Ready-image bytes, by the frozen import mapping. Claimed files
    /// become references, so uncaptured preimages stay protected.
    /// Returns the pending operation, if any.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn require_local_restore_journals(
        &self,
        rows: &LocalRestoreRows,
        patches: &BTreeMap<u64, CapturedPatch>,
        base: Option<&ProjectionSnapshot>,
        last: u64,
        mappings: &BTreeMap<FileId, (FileManifestId, u64)>,
        frozen: &BTreeMap<FileId, (FileManifestId, u64)>,
        captured: &BTreeSet<FileId>,
        referenced: &mut BTreeSet<FileId>,
        cancel: Option<&Cancellation>,
    ) -> Result<Option<String>> {
        if rows.history.is_empty() && rows.pending.is_none() {
            return Ok(None);
        }
        let base = base.ok_or_else(|| invalid("local denial Restore has no local Capture base"))?;
        let ready = self.record.boundaries.ready_capture_sequence;
        let destination = self.local_destination();
        let mut sequences = BTreeSet::new();
        let mut previews: Vec<(u64, RestorePreview)> = Vec::new();
        for (id, (sequence, bytes)) in &rows.history {
            cancellation::check(cancel)?;
            if *sequence <= ready
                || *sequence > last
                || !sequences.insert(*sequence)
                || !canonical_operation(id)
            {
                return Err(invalid("local denial Restore history escaped Capture suffix"));
            }
            let capture = patches
                .get(sequence)
                .ok_or_else(|| invalid("local denial Restore history Capture absent"))?;
            let preview = crate::projection::restore::local_history_preview(
                bytes,
                id,
                capture,
                base,
                &destination,
            )?;
            previews.push((*sequence, preview));
        }
        for (sequence, preview) in &previews {
            self.require_local_source(&preview.source, *sequence, &rows.history)?;
        }
        let pending = match &rows.pending {
            None => None,
            Some(bytes) => {
                let preview = crate::projection::restore::local_pending_preview(
                    bytes,
                    base,
                    self.record.boundaries.ready_generation,
                    &self.ready().plan.placement_digest,
                    &destination,
                )?;
                if rows.history.contains_key(&preview.operation_id)
                    || !canonical_operation(&preview.operation_id)
                {
                    return Err(invalid("local denial pending Restore is not new"));
                }
                self.require_local_source(&preview.source, last.saturating_add(1), &rows.history)?;
                Some(preview)
            }
        };
        for preview in previews.iter().map(|(_, preview)| preview).chain(pending.iter()) {
            cancellation::check(cancel)?;
            for file in preview.before.values().chain(preview.after.values()).flatten() {
                let claimed = captured.contains(&file.file_id);
                if !(claimed || frozen.contains_key(&file.file_id))
                    || mappings
                        .get(&file.file_id)
                        .is_none_or(|(_, bytes)| *bytes != file.bytes)
                {
                    return Err(invalid("local denial Restore journal file is not retained"));
                }
                if claimed {
                    referenced.insert(file.file_id.clone());
                }
            }
        }
        Ok(pending.map(|preview| preview.operation_id))
    }

    /// Prove one local Restore's forced capture, journal, history row, claims
    /// and mappings fit every C14 inventory bound before its first durable
    /// reservation. Restore never takes Capture's same-image dedup branch.
    pub(in crate::projection::provider) fn preflight_local_restore(
        &self,
        state: &VersionedProjection,
        plan: &LocalRestorePlan,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if self.ready().plan.denial_coverage.is_none() {
            return Ok(());
        }
        cancellation::check(cancel)?;
        let c = self.connection;
        let ready = self.record.boundaries.ready_capture_sequence;
        let source_valid = match plan.source() {
            RestoreSource::Version { version, .. } => self.is_local_version(version, u64::MAX),
            RestoreSource::Capture { sequence, .. } => *sequence > ready,
            RestoreSource::Inverse { operation_id } => c
                .query_row(
                    "SELECT capture_sequence FROM projection_restore_history WHERE operation_id=?1",
                    [operation_id],
                    |r| r.get::<_, u64>(0),
                )
                .optional()?
                .is_some_and(|sequence| sequence > ready),
        };
        let budget: Option<String> = c
            .query_row(
                "SELECT operation FROM projection_budgets WHERE singleton=1 AND state='active' AND length(CAST(operation AS BLOB))<=128",
                [],
                |r| r.get(0),
            )
            .optional()?;
        // The reader accepts only a canonical `restore:<u128>-<u32>` budget; a
        // caller-recomputed preview digest must never mint an unreadable one.
        if c.is_autocommit()
            || self.private_schema != 9
            || state.generation != self.record.boundaries.ready_generation
            || state.digest != self.ready().plan.placement_digest
            || !source_valid
            || !plan
                .operation()
                .strip_prefix("restore:")
                .is_some_and(canonical_operation)
            || bounded_count(c, "projection_restore_pending", 1)? != 0
            || bounded_count(c, "projection_prejournal", 1)? != 0
        {
            return Err(invalid("local denial Restore preflight escaped Ready scope"));
        }
        if budget.as_deref().is_some_and(|op| op != plan.operation())
            || bounded_count(c, "projection_budgets", 1)? != u64::from(budget.is_some())
        {
            return Err(invalid(
                "shared quota reservation pending; explicit recovery required",
            ));
        }
        let payload = crate::projection::capture::storage::local_restore_append_payload(
            c,
            state,
            &self.marker.identity,
            plan.changes(),
        )?;
        let (capture_count, capture_bytes): (u64, u64) = c.query_row(
            "SELECT count(*),coalesce(sum(length(payload)),0) FROM projection_captures WHERE sequence>?1",
            [ready],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (history_count, history_bytes) = local_restore_inventory(c, ready)?;
        let counts = LocalRestoreCounts {
            captures: capture_count,
            capture_bytes,
            all_captures: bounded_count(c, "projection_captures", MAX_CONTENT_ROWS)?,
            history: history_count,
            history_bytes,
            claims: bounded_count(c, "projection_claims", MAX_LOCAL_ROWS + 1)?,
            content: bounded_count(c, "projection_content", MAX_CONTENT_ROWS)?,
        };
        let mut query = c.prepare(
            "SELECT owner FROM projection_claims WHERE kind='captured-content' ORDER BY owner",
        )?;
        let owners = query
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        let mut new_claims = 0u64;
        let mut new_mappings = 0u64;
        for file in plan.files() {
            cancellation::check(cancel)?;
            if !owners.contains(file.file_id.as_str()) {
                new_claims += 1;
            }
            if !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM projection_content WHERE file_id=?1)",
                [file.file_id.as_str()],
                |r| r.get::<_, bool>(0),
            )? {
                new_mappings += 1;
            }
        }
        require_restore_fits(
            &counts,
            &LocalRestoreCost {
                capture_payload: payload.len() as u64,
                journal: plan.journal_bound(),
                claims: new_claims,
                mappings: new_mappings,
            },
        )?;
        cancellation::check(cancel)
    }
}

pub(super) struct LocalRestoreCounts {
    pub(super) captures: u64,
    pub(super) capture_bytes: u64,
    pub(super) all_captures: u64,
    pub(super) history: u64,
    pub(super) history_bytes: u64,
    pub(super) claims: u64,
    pub(super) content: u64,
}

pub(super) struct LocalRestoreCost {
    pub(super) capture_payload: u64,
    pub(super) journal: u64,
    /// Captured-content claims to add; the one `restore` claim is implied.
    pub(super) claims: u64,
    pub(super) mappings: u64,
}

/// Pure arithmetic: one forced capture row, one history row (whose payload
/// is first the pending journal), new claims plus the `restore` claim, and
/// new content mappings. Each bound is the one the C14 reader enforces.
pub(super) fn require_restore_fits(counts: &LocalRestoreCounts, cost: &LocalRestoreCost) -> Result<()> {
    let fits = |value: u64, add: u64, maximum: u64| value.checked_add(add).is_some_and(|n| n <= maximum);
    if !fits(counts.captures, 1, MAX_LOCAL_ROWS)
        || !fits(counts.capture_bytes, cost.capture_payload, MAX_LOCAL_CAPTURE_PAYLOAD)
        || !fits(counts.all_captures, 1, MAX_CONTENT_ROWS)
        || !fits(counts.history, 1, MAX_LOCAL_ROWS)
        || !fits(counts.history_bytes, cost.journal, MAX_LOCAL_RESTORE_PAYLOAD)
    {
        return Err(invalid("local denial Restore append exceeds history bound"));
    }
    if !cost
        .claims
        .checked_add(1)
        .is_some_and(|add| fits(counts.claims, add, MAX_LOCAL_ROWS + 1))
        || !fits(counts.content, cost.mappings, MAX_CONTENT_ROWS)
    {
        return Err(invalid("local denial Restore append exceeds content bound"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::provider::process_profile::tests::child;

    #[test]
    #[ignore = "UNQUALIFIED Restore suffix inventory SQL bounds; ROOT review/release child120 required"]
    fn local_restore_inventory_bounds_history_and_pending_before_decoding() {
        child::run(
            concat!(module_path!(), "::local_restore_inventory_bounds_history_and_pending_before_decoding"),
            120,
            || {
                child::declare();
                let c = Connection::open_in_memory().unwrap();
                c.execute_batch("CREATE TABLE projection_restore_history(operation_id TEXT PRIMARY KEY,capture_sequence INTEGER NOT NULL,payload BLOB NOT NULL) STRICT; CREATE TABLE projection_restore_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL) STRICT;").unwrap();
                // Imported history at or below the Ready capture is excluded.
                c.execute("INSERT INTO projection_restore_history VALUES('1-1',1,zeroblob(?1))", [MAX_LOCAL_RESTORE_PAYLOAD + 1]).unwrap();
                assert_eq!(local_restore_inventory(&c, 1).unwrap(), (0, 0));
                // Aggregation rejects before any malformed payload is decoded.
                c.execute("INSERT INTO projection_restore_history VALUES('2-1',2,zeroblob(?1))", [MAX_LOCAL_RESTORE_PAYLOAD - 1]).unwrap();
                assert_eq!(local_restore_inventory(&c, 1).unwrap(), (1, MAX_LOCAL_RESTORE_PAYLOAD - 1));
                c.execute("INSERT INTO projection_restore_pending VALUES(1,x'00')", []).unwrap();
                assert_eq!(local_restore_inventory(&c, 1).unwrap(), (1, MAX_LOCAL_RESTORE_PAYLOAD));
                c.execute("UPDATE projection_restore_pending SET payload=x'0000'", []).unwrap();
                let error = local_restore_inventory(&c, 1).unwrap_err();
                assert!(error.to_string().contains("Restore history exceeds aggregate bound"));
                c.execute_batch("DELETE FROM projection_restore_history; DELETE FROM projection_restore_pending; WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<100000) INSERT INTO projection_restore_history SELECT i||'-1',i,x'00' FROM n;").unwrap();
                assert_eq!(local_restore_inventory(&c, 0).unwrap(), (MAX_LOCAL_ROWS, MAX_LOCAL_ROWS));
                c.execute("INSERT INTO projection_restore_history VALUES('100001-1',100001,x'00')", []).unwrap();
                let error = local_restore_inventory(&c, 0).unwrap_err();
                assert!(error.to_string().contains("Restore history exceeds aggregate bound"));
            },
        );
    }

    #[test]
    #[ignore = "UNQUALIFIED Restore funding arithmetic; ROOT review/release child120 required"]
    fn restore_funding_arithmetic_is_exact_at_each_bound() {
        child::run(
            concat!(module_path!(), "::restore_funding_arithmetic_is_exact_at_each_bound"),
            120,
            || {
                child::declare();
                let at_limit = || LocalRestoreCounts {
                    captures: MAX_LOCAL_ROWS - 1,
                    capture_bytes: MAX_LOCAL_CAPTURE_PAYLOAD - 10,
                    all_captures: MAX_CONTENT_ROWS - 1,
                    history: MAX_LOCAL_ROWS - 1,
                    history_bytes: MAX_LOCAL_RESTORE_PAYLOAD - 20,
                    claims: MAX_LOCAL_ROWS + 1 - 3,
                    content: MAX_CONTENT_ROWS - 4,
                };
                let cost = LocalRestoreCost { capture_payload: 10, journal: 20, claims: 2, mappings: 4 };
                require_restore_fits(&at_limit(), &cost).unwrap();
                let history = "Restore append exceeds history bound";
                let content = "Restore append exceeds content bound";
                for (mutate, expected) in [
                    (Box::new(|c: &mut LocalRestoreCounts| c.captures += 1) as Box<dyn Fn(&mut LocalRestoreCounts)>, history),
                    (Box::new(|c: &mut LocalRestoreCounts| c.capture_bytes += 1), history),
                    (Box::new(|c: &mut LocalRestoreCounts| c.all_captures += 1), history),
                    (Box::new(|c: &mut LocalRestoreCounts| c.history += 1), history),
                    (Box::new(|c: &mut LocalRestoreCounts| c.history_bytes += 1), history),
                    (Box::new(|c: &mut LocalRestoreCounts| c.claims += 1), content),
                    (Box::new(|c: &mut LocalRestoreCounts| c.content += 1), content),
                ] {
                    let mut counts = at_limit();
                    mutate(&mut counts);
                    let error = require_restore_fits(&counts, &cost).unwrap_err();
                    assert!(error.to_string().contains(expected), "{error}");
                }
                // The forced capture row is funded even with zero new files.
                let mut counts = at_limit();
                counts.captures = MAX_LOCAL_ROWS;
                let empty = LocalRestoreCost { capture_payload: 0, journal: 0, claims: 0, mappings: 0 };
                assert!(require_restore_fits(&counts, &empty).is_err());
                assert!(canonical_operation("1793664458797530400-78428"));
                assert!(!canonical_operation("01-2") && !canonical_operation("1-") && !canonical_operation("1-4294967296"));
            },
        );
    }
}
