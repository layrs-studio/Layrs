//! Bounded exact incoming/outgoing partition. The incoming interpreter supplies
//! canonical ancestry and the actual ingress receipt, never caller-picked IDs.
use super::*;
use crate::projection::provider::{
    import_validation::VerifiedDestinationPrefix, records::import::TransitionReceiptRowData,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ANCESTORS: u64 = 4096;
const MAX_OUTGOING: u64 = MAX_TRANSITIONS - 1;
pub(super) const MAX_OUTGOING_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECEIPT_BYTES: u64 = ProviderTransitionReceiptRecord::MAX_BYTES as u64;
const MAX_RECEIPT_SCALARS: u64 = 71 + 71 + 128 + 71;

/// Before any receipt decode or global FK walk, bound the complete inventory.
/// Incoming ancestry keeps its independent 4096-row/16MiB budget, PLUS its own
/// incoming receipt. Outgoing terminals use a separate 4095-row/16MiB
/// normalized five-column budget (not merely payload bytes).
/// The authenticated partition below tightens these aggregate upper bounds.
pub(in crate::projection::provider) fn preflight(
    c: &Connection,
    cancel: Option<&Cancellation>,
) -> CauseResult<(u64, u64)> {
    cancellation::check(cancel)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transition_receipts LIMIT 8193)",
        [],
        |row| row.get(0),
    )?;
    if count == 0 || count > MAX_ANCESTORS + 1 + MAX_OUTGOING {
        return Err(invalid("ordinary receipt inventory count exceeds bound"));
    }
    let (payload, normalized, oversized): (u64, u64, bool) = c.query_row(
        "SELECT coalesce(sum(length(payload)),0),coalesce(sum(length(CAST(id AS BLOB))+length(CAST(owner_identity_digest AS BLOB))+length(CAST(transaction_id AS BLOB))+coalesce(length(CAST(companion_id AS BLOB)),0)+length(payload)),0),coalesce(max(length(CAST(id AS BLOB))!=71 OR length(CAST(owner_identity_digest AS BLOB))!=71 OR length(CAST(transaction_id AS BLOB))=0 OR length(CAST(transaction_id AS BLOB))>128 OR coalesce(length(CAST(companion_id AS BLOB)),71)!=71 OR length(payload)=0 OR length(payload)>65536),0) FROM projection_provider_transition_receipts",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let payload_bound = 2 * MAX_OUTGOING_BYTES + MAX_RECEIPT_BYTES;
    if oversized
        || payload > payload_bound
        || normalized > payload_bound + (MAX_ANCESTORS + 1) * MAX_RECEIPT_SCALARS
    {
        return Err(invalid("ordinary receipt SQL columns exceed envelope"));
    }
    cancellation::check(cancel)?;
    Ok((count, payload))
}

pub(super) fn read_partition(
    c: &Connection,
    incoming: &VerifiedDestinationPrefix<'_>,
    intents: &[ProviderTransitionIntentRecord],
    cancel: Option<&Cancellation>,
) -> CauseResult<Vec<ProviderTransitionReceiptRecord>> {
    incoming.require_destination_connection(c)?;
    let (count, _) = preflight(c, cancel)?;
    let ancestors = incoming.ancestor_receipts();
    if ancestors.len() as u64 > MAX_ANCESTORS
        || incoming.prefix().closure.transition_receipts.count != ancestors.len() as u64
    {
        return Err(invalid(
            "ordinary incoming receipt closure differs from ancestry",
        ));
    }
    let mut ingress = BTreeMap::new();
    for receipt in ancestors.iter().chain(std::iter::once(incoming.receipt())) {
        if ingress.insert(receipt.digest()?, receipt).is_some() {
            return Err(invalid("ordinary incoming receipt domains overlap"));
        }
    }
    let mut owners = BTreeMap::new();
    for intent in intents {
        require_shape(intent)?;
        if let Some(id) = &intent.receipt_id {
            if ingress.contains_key(id) || owners.insert(id.clone(), intent).is_some() {
                return Err(invalid(
                    "ordinary outgoing receipt reference overlaps or is reused",
                ));
            }
        }
    }
    if owners.len() as u64 > MAX_OUTGOING || count != ingress.len() as u64 + owners.len() as u64 {
        return Err(invalid("ordinary receipts have missing or unowned rows"));
    }
    let mut statement = c.prepare(
        "SELECT id,owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts ORDER BY id COLLATE BINARY",
    )?;
    let mut rows = statement.query([])?;
    let mut output = Vec::with_capacity(owners.len());
    let mut keys = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut outgoing_bytes = 0u64;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let (id, owner, transaction, companion, payload): (
            String,
            String,
            String,
            Option<String>,
            Vec<u8>,
        ) = (
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        );
        let receipt = ProviderTransitionReceiptRecord::decode(&payload)?;
        receipt.require_row(TransitionReceiptRowData {
            id: &id,
            owner_identity_digest: &owner,
            transaction_id: &transaction,
            companion_id: companion.as_deref(),
            payload: &payload,
        })?;
        let normalized = id.len() as u64
            + owner.len() as u64
            + transaction.len() as u64
            + companion.as_ref().map_or(0, |id| id.len() as u64)
            + payload.len() as u64;
        // SQL's unique key is (owner,transaction), not owner alone: one C owner
        // can have several legitimate historical transactions.
        if !keys.insert((owner, transaction)) || !seen.insert(id.clone()) {
            return Err(invalid(
                "ordinary receipt owner/transaction or ID is duplicated",
            ));
        }
        if let Some(expected) = ingress.get(&id) {
            if *expected != &receipt {
                return Err(invalid("ordinary authenticated incoming receipt changed"));
            }
        } else {
            if receipt.denial_coverage.is_some() {
                return Err(invalid(
                    "ordinary outgoing denial receipt reader is not integrated",
                ));
            }
            owners
                .get(&id)
                .ok_or_else(|| invalid("ordinary receipt has no exact terminal owner"))?
                .require_receipt(&receipt)?;
            outgoing_bytes = outgoing_bytes
                .checked_add(normalized)
                .filter(|bytes| *bytes <= MAX_OUTGOING_BYTES)
                .ok_or_else(|| {
                    invalid("ordinary outgoing receipt normalized columns exceed bound")
                })?;
            output.push(receipt);
        }
    }
    if seen.len() as u64 != count || output.len() != owners.len() {
        return Err(invalid(
            "ordinary receipt partition changed after preflight",
        ));
    }
    cancellation::check(cancel)?;
    Ok(output)
}
