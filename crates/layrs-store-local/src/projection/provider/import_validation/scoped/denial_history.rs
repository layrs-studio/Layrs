//! Inherited denial documents selected only by authenticated incoming receipts.
//! Exact DATA transport; no remote observation capability or acceptance grant.
use super::*;
use crate::projection::{
    offline,
    provider::{denied_coverage_storage, records::denied_coverage::TransitionDeniedCoverageRecord},
};

pub(in crate::projection::provider::import_validation) const PREFLIGHT_SQL: &str = "SELECT count(*),coalesce(sum(length(CAST(digest AS BLOB))+length(payload)),0) FROM projection_observations WHERE digest IN (SELECT value FROM json_each(?1))";
pub(in crate::projection::provider::import_validation) const ROWS_SQL: &str = "SELECT digest,payload FROM projection_observations WHERE digest IN (SELECT value FROM json_each(?1)) ORDER BY digest COLLATE BINARY";

/// A receipt list is borrowed from the authenticated lineage, not supplied as a
/// selected document-ID capability. Historical scopes use its exact prefix.
pub(super) fn derive(
    c: &Connection,
    prefix: &ProviderImportPrefixRecord,
    scope: &ScopeView<'_>,
    cancel: Option<&Cancellation>,
) -> CauseResult<BTreeSet<String>> {
    super::super::super::cancellation::check(cancel)?;
    if scope.receipts.len() > scope.lineage.receipts().len()
        || scope.receipts != &scope.lineage.receipts()[..scope.receipts.len()]
        || scope.receipts.len() > scope.lineage.segments().len()
    {
        return Err(invalid(
            "import denial receipt scope is not incoming lineage",
        ));
    }
    prefix.validate_segments(&scope.lineage.segments()[..scope.receipts.len()])?;
    let mut receipt_ids = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for receipt in scope.receipts {
        super::super::super::cancellation::check(cancel)?;
        receipt.validate_fields()?;
        receipt_ids.insert(receipt.digest()?);
        if let Some(reference) = &receipt.denial_coverage {
            if prefix.closure.denial_observations.is_none() {
                return Err(invalid(
                    "legacy import prefix cannot transport inherited denial coverage",
                ));
            }
            if receipt.destination_observation_generation > prefix.source.version().generation {
                return Err(invalid(
                    "import denial observation is outside copied prefix",
                ));
            }
            ids.insert(reference.id.clone());
        }
    }
    if &receipt_ids != scope.receipt_ids || receipt_ids.len() != scope.receipts.len() {
        return Err(invalid("import denial incoming receipt IDs differ"));
    }
    if scope.kind == ImportDatabaseKind::Ordinary && !ids.is_empty() {
        return Err(invalid(
            "ordinary6 import cannot transport inherited denial documents",
        ));
    }
    // Bound the complete selected document inventory before decoding any body.
    // Extra documents can belong to another transition and are never imported.
    if !ids.is_empty() {
        let parameter = ids_json(&ids)?;
        let (count, bytes, maximum, invalid_type): (u64, u64, u64, bool) = c.query_row(
            "SELECT count(*),coalesce(sum(length(CAST(digest AS BLOB))+length(payload)),0),coalesce(max(length(payload)),0),coalesce(max(typeof(digest)!='text' OR length(CAST(digest AS BLOB))!=71 OR typeof(payload)!='blob' OR length(payload)=0),0) FROM projection_observations WHERE digest IN (SELECT value FROM json_each(?1))",
            [&parameter], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        )?;
        if count != ids.len() as u64
            || bytes > BYTE_LIMIT
            || maximum > 64 * 1024 * 1024
            || invalid_type
        {
            return Err(invalid(
                "import inherited denial document scope missing or oversized",
            ));
        }
    }
    for receipt in scope.receipts {
        let Some(reference) = &receipt.denial_coverage else {
            continue;
        };
        let body = denied_coverage_storage::read_required(c, reference, cancel)?;
        require_receipt(&body, receipt)?;
        let generation = receipt.destination_observation_generation;
        let snapshot =
            offline::snapshot_at(c, generation, Some(&receipt.destination_observation_digest))?;
        // The normalized observation stores its selection at that exact
        // generation. Do not substitute the current or a later selection.
        let selection_digest: String = c.query_row(
            "SELECT digest FROM projection_selections WHERE generation=?1",
            [generation],
            |r| r.get(0),
        )?;
        let selected = selection::at(c, generation)?;
        body.require_observation_rows(
            &snapshot.snapshot,
            &selected,
            generation,
            &snapshot.digest,
            &selection_digest,
            cancel,
        )?;
    }
    Ok(ids)
}

fn require_receipt(
    body: &TransitionDeniedCoverageRecord,
    receipt: &ProviderTransitionReceiptRecord,
) -> CauseResult<()> {
    let reference = receipt
        .denial_coverage
        .as_ref()
        .ok_or_else(|| invalid("import denial receipt has no required reference"))?;
    if !receipt.supports_denied_coverage()
        || body.reference()? != *reference
        || body.transaction_id != receipt.transaction_id
        || body.source_prefix_id != receipt.source_prefix_id
        || body.source_identity != receipt.source_identity
        || body.destination_identity != receipt.destination_identity
        || &body.source_version != receipt.source_prefix.source.version()
        || body.observation_generation != receipt.destination_observation_generation
        || body.observation_snapshot_digest != receipt.destination_observation_digest
    {
        return Err(invalid(
            "import inherited denial body differs from actual receipt",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "denial_history/tests.rs"]
mod tests;
