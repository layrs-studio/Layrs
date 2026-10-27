//! Exact immutable row inventory after copying a verified source prefix. The
//! caller separately validates destination ingress, head, journal, live tables
//! and physical ownership. This result never admits an ordinary worktree.
use super::super::records::Record;
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
    scoped::read_scoped_closure(c, proof.metadata.kind, prefix, &proof.metadata, cancel)?;
    super::super::cancellation::check(cancel)?;
    Ok(())
}
