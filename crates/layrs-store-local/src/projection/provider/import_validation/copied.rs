//! Exact immutable row inventory after copying a verified source prefix. The
//! caller separately validates destination ingress, head, journal, live tables
//! and physical ownership. This result never admits an ordinary worktree.
use super::super::{
    denied_coverage_storage,
    records::{Record, denied_coverage::TransitionDeniedCoverageRecord},
};
use super::*;

/// Requires the entire imported destination tables to equal the verified source
/// scope, including absence of a tail outside that scope. The source proof is
/// retained DATA; this routine does not reacquire a source or content authority.
pub(in crate::projection::provider) fn require_exact_copied_rows(
    c: &Connection,
    marker: &format::Marker,
    prefix: &ProviderImportPrefixRecord,
    proof: &VerifiedImportClosure,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    require_exact_copied_rows_with_current(c, marker, prefix, proof, None, cancel)
}

/// The current document is separate from the inherited prefix commitment. Its
/// caller authenticates the actual destination intent and full ingress phase;
/// this reader rechecks its actual retained body and exact observation inventory.
/// No missing document is copied, repaired or inferred from descendant receipts.
pub(in crate::projection::provider) fn require_exact_copied_rows_with_current(
    c: &Connection,
    marker: &format::Marker,
    prefix: &ProviderImportPrefixRecord,
    proof: &VerifiedImportClosure,
    current: Option<&TransitionDeniedCoverageRecord>,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    if c.is_autocommit() {
        return Err(invalid(
            "copied prefix reader requires a retained transaction",
        ));
    }
    require_schema(c, marker, ImportDatabaseKind::Provider, cancel)?;
    if marker.identity != prefix.destination_identity
        || proof.prefix_id() != prefix.digest()?
        || proof.closure() != &prefix.closure
    {
        return Err(invalid("copied prefix identity or proof differs"));
    }
    let rows = [
        ("projection_snapshots", &prefix.closure.snapshots),
        ("projection_selections", &prefix.closure.selections),
        ("projection_captures", &prefix.closure.captures),
        ("projection_publications", &prefix.closure.publications),
        ("projection_provider_catalogs", &prefix.closure.catalogs),
        (
            "projection_provider_descriptors",
            &prefix.closure.descriptors,
        ),
        (
            "projection_provider_activations",
            &prefix.closure.activation_maps,
        ),
        (
            "projection_provider_transition_receipts",
            &prefix.closure.transition_receipts,
        ),
        ("projection_provider_companions", &prefix.closure.companions),
        (
            "projection_provider_companion_files",
            &prefix.closure.companion_files,
        ),
        (
            "projection_provider_companion_deletions",
            &prefix.closure.companion_deletions,
        ),
        ("projection_content", &prefix.closure.content_mappings),
    ];
    for (table, expected) in rows {
        super::super::cancellation::check(cancel)?;
        // Static table names only; stop after the first unexpected row. No
        // payload is read and a hostile unrelated tail cannot force a full scan.
        let limit = expected
            .count
            .checked_add(1)
            .filter(|n| *n <= 1_000_001)
            .ok_or_else(|| invalid("copied prefix count exceeds bound"))?;
        let count: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT ?1)"),
            [limit],
            |row| row.get(0),
        )?;
        if count != expected.count {
            return Err(invalid("copied prefix table has missing or extra rows"));
        }
    }
    let expected_history = prefix
        .closure
        .restore_history
        .as_ref()
        .map_or(0, |rows| rows.count);
    let actual_history: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_restore_history LIMIT ?1)",
        [expected_history + 1],
        |row| row.get(0),
    )?;
    if actual_history != expected_history {
        return Err(invalid("copied restore history has missing or extra rows"));
    }
    scoped::with_scope(
        c,
        proof.metadata.kind,
        prefix,
        &scoped::ScopeView::from_metadata(&proof.metadata),
        cancel,
        |specs, closure| require_observation_inventory(c, prefix, specs, closure, current, cancel),
    )?;
    super::super::cancellation::check(cancel)?;
    Ok(())
}

/// Private inventory step reached only after with_scope authenticates inherited
/// bodies and computes their complete SQL commitments. Comparing that closure
/// remains mandatory: a total count alone cannot detect a substituted row.
fn require_observation_inventory(
    c: &Connection,
    prefix: &ProviderImportPrefixRecord,
    specs: &[rows::Spec<'_>],
    actual_closure: &ImportClosure,
    current: Option<&TransitionDeniedCoverageRecord>,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    super::super::cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "copied observations require a retained transaction",
        ));
    }
    if actual_closure != &prefix.closure {
        return Err(invalid(
            "copied inherited observation closure differs from prefix",
        ));
    }
    let inherited = prefix
        .closure
        .denial_observations
        .as_ref()
        .map_or(0, |rows| rows.count);
    let expected = inherited
        .checked_add(u64::from(current.is_some()))
        .filter(|count| *count <= 1_000_000)
        .ok_or_else(|| invalid("copied observation inventory exceeds row bound"))?;
    if let Some(current) = current {
        let reference = current.reference()?;
        if current.source_prefix_id != prefix.digest()?
            || current.destination_identity != prefix.destination_identity
        {
            return Err(invalid(
                "copied current observation prefix/destination differs",
            ));
        }
        let actual = denied_coverage_storage::read_required(c, &reference, cancel)?;
        if actual != *current {
            return Err(invalid("copied current observation body differs"));
        }
        // The selector is generated by with_scope from authenticated incoming
        // receipts. Never accept a caller-selected list or descendant receipt.
        if let Some(spec) = specs
            .iter()
            .find(|spec| spec.kind == rows::Kind::DenialObservations)
        {
            match spec.params {
                [ids] => {
                    let bindings: [&dyn rusqlite::ToSql; 2] = [*ids, &reference.id];
                    let collision: bool = c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM json_each(?1) WHERE value=?2)",
                        bindings.as_slice(),
                        |row| row.get(0),
                    )?;
                    if collision {
                        return Err(invalid(
                            "copied current observation aliases inherited scope",
                        ));
                    }
                }
                [] if inherited == 0 => {} // Empty ordinary6 selector.
                _ => return Err(invalid("copied inherited observation selector differs")),
            }
        } else if prefix.closure.denial_observations.is_some() {
            return Err(invalid("copied inherited observation selector is missing"));
        }
    }
    // All expected inherited rows have authenticated bodies/commitments, and
    // current (when present) is authenticated and disjoint. Exact cardinality
    // therefore proves the complete union and rejects every unrelated row.
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_observations LIMIT ?1)",
        [expected + 1],
        |row| row.get(0),
    )?;
    if count != expected {
        return Err(invalid(
            "copied observation inventory has missing or extra rows",
        ));
    }
    super::super::cancellation::check(cancel)
}

#[cfg(test)]
#[path = "copied/observations_tests.rs"]
mod observations_tests;
