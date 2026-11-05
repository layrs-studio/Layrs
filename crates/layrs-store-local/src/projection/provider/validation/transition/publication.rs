//! Explicit outgoing publication DATA. These receipts are not incoming import
//! ancestry and this reader confers no physical destination authority.
use super::*;
use crate::projection::provider::records::import::{
    ProviderTransitionReceiptRecord, TransitionReceiptRowData,
};
use std::collections::{BTreeMap, BTreeSet};

/// Both relations are fenced before capture validation's global FK traversal.
pub(in crate::projection::provider::validation) fn preflight_publication(
    c: &Connection,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    preflight_view(c, View::Publication, cancel)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transition_receipts LIMIT 4097)",
        [],
        |r| r.get(0),
    )?;
    if count > MAX_ROWS {
        return Err(invalid("source publication receipt count exceeds bound"));
    }
    let bad: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_provider_transition_receipts WHERE \
         typeof(id)!='text' OR length(CAST(id AS BLOB))!=71 \
         OR typeof(owner_identity_digest)!='text' OR length(CAST(owner_identity_digest AS BLOB))!=71 \
         OR typeof(transaction_id)!='text' OR length(CAST(transaction_id AS BLOB))=0 OR length(CAST(transaction_id AS BLOB))>128 \
         OR (companion_id IS NOT NULL AND (typeof(companion_id)!='text' OR length(CAST(companion_id AS BLOB))!=71)) \
         OR typeof(payload)!='blob')", [], |r| r.get(0),
    )?;
    let (total, largest): (u64, u64) = c.query_row(
        "SELECT coalesce(sum(length(payload)),0),coalesce(max(length(payload)),0) FROM projection_provider_transition_receipts",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if bad
        || total > MAX_PAYLOAD_TOTAL
        || largest > ProviderTransitionReceiptRecord::MAX_BYTES as u64
    {
        return Err(invalid(
            "source publication receipt SQL projection or payload exceeds bound",
        ));
    }
    super::super::super::cancellation::check(cancel)
}

pub(in crate::projection::provider::validation) fn validate_publication(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
    source: &ProviderBootstrapSource,
    capture: &super::super::capture::CaptureHistoryData,
    cancel: Option<&Cancellation>,
) -> Result<(
    Vec<ProviderTransitionIntentRecord>,
    Vec<ProviderTransitionReceiptRecord>,
)> {
    preflight_publication(c, cancel)?;
    let transitions = validate_view(
        c,
        allocation,
        prepared,
        activation,
        activated,
        source,
        &capture.transactions,
        &capture.patches,
        View::Publication,
        cancel,
    )?;
    let mut receipts = BTreeMap::new();
    let mut statement = c.prepare(
        "SELECT id,owner_identity_digest,transaction_id,companion_id,payload \
         FROM projection_provider_transition_receipts ORDER BY id COLLATE BINARY",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        super::super::super::cancellation::check(cancel)?;
        let id: String = row.get(0)?;
        let owner: String = row.get(1)?;
        let transaction: String = row.get(2)?;
        let companion: Option<String> = row.get(3)?;
        let payload: Vec<u8> = row.get(4)?;
        let receipt = ProviderTransitionReceiptRecord::decode(&payload)?;
        receipt.require_row(TransitionReceiptRowData {
            id: &id,
            owner_identity_digest: &owner,
            transaction_id: &transaction,
            companion_id: companion.as_deref(),
            payload: &payload,
        })?;
        receipts.insert(id, receipt);
    }
    let mut referenced = BTreeSet::new();
    for intent in &transitions {
        super::super::super::cancellation::check(cancel)?;
        if let Some(id) = &intent.receipt_id {
            let receipt = receipts
                .get(id)
                .ok_or_else(|| invalid("source publication receipt is absent"))?;
            intent.require_receipt(receipt)?;
            if !referenced.insert(id) {
                return Err(invalid("source publication receipt has multiple owners"));
            }
        }
    }
    if referenced.len() != receipts.len() {
        return Err(invalid("source publication has an unowned receipt"));
    }
    Ok((transitions, receipts.into_values().collect()))
}
