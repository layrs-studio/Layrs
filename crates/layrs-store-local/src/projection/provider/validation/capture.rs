//! Bounded private SQL interpretation for an Active provider's capture state.
//! This is DATA for the separate family validator, never native authority.
use super::*;
use crate::projection::{
    capture::{CapturedChange, CapturedPatch, MAX_CAPTURE_FILE_BYTES},
    provider::records::capture::{
        ProviderCaptureReceiptRecord, ProviderCaptureReceiptRowData, ProviderCaptureState,
        ProviderCaptureTransactionRecord, ProviderCaptureTransactionRowData,
    },
};
use layrs_core::{
    projection::{EntryState, ReservedPaths},
    scm::{FileId, FileManifestId, TreeEntryTarget, canonical_json},
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_TRANSACTIONS: u64 = 4096;
const MAX_CAPTURES: u64 = 4096;
const MAX_CONTENT: u64 = 100_000;
const MAX_CLAIMS: u64 = 4096;
const MAX_PAYLOAD_TOTAL: u64 = 128 * 1024 * 1024;

pub(in crate::projection) struct PrivateCaptureProof {
    pub transactions: Vec<ProviderCaptureTransactionRecord>,
    pub receipts: Vec<ProviderCaptureReceiptRecord>,
    pub patches: Vec<CapturedPatch>,
    pub content: Vec<CaptureContentProof>,
    pub claims: Vec<CaptureClaimProof>,
}

pub(in crate::projection) struct CaptureContentProof {
    pub file_id: String,
    pub manifest_id: String,
    pub bytes: u64,
}

pub(in crate::projection) struct CaptureClaimProof {
    pub root_id: String,
    pub digest: String,
    pub kind: String,
    pub owner: String,
}

fn fence(c: &Connection, table: &str, limit: u64, predicate: &str) -> Result<()> {
    let count: u64 = c.query_row(
        &format!(
            "SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT {})",
            limit + 1
        ),
        [],
        |r| r.get(0),
    )?;
    if count > limit {
        return Err(invalid(format!(
            "provider capture {table} row count exceeds bound"
        )));
    }
    let bad: bool = c.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {predicate})"),
        [],
        |r| r.get(0),
    )?;
    if bad {
        return Err(invalid(format!(
            "provider capture {table} projection differs"
        )));
    }
    Ok(())
}

fn captured_change_authority(
    changes: &[CapturedChange],
    activated: &VersionedProjection,
    reserved: &ReservedPaths,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let mut previous = None;
    for change in changes {
        super::super::cancellation::check(cancel)?;
        let path = change.path.as_str();
        if path.len() > 4096 || previous.is_some_and(|prior: &str| prior >= path) {
            return Err(invalid(
                "provider capture paths are not bounded, sorted and unique",
            ));
        }
        previous = Some(path);
        reserved
            .check_path(path)
            .map_err(|e| invalid(e.to_string()))?;
        let index = activated
            .snapshot
            .entries
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .map_err(|_| invalid("provider capture references an unknown path"))?;
        let entry = &activated.snapshot.entries[index];
        if !entry.writable || !matches!(&entry.state, EntryState::Materialized | EntryState::Absent)
        {
            return Err(invalid("provider capture references a protected entry"));
        }
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => return Err(invalid("provider capture expected target is not a file")),
        };
        if change
            .expected
            .as_ref()
            .map(|file| (&file.file_id, file.executable))
            != expected
            || change
                .replacement
                .as_ref()
                .map(|file| (&file.file_id, file.executable))
                == expected
        {
            return Err(invalid(
                "provider capture expected/replacement target differs",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate(
    c: &Connection,
    allocation: &AllocationRecord,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
    cancel: Option<&Cancellation>,
) -> Result<PrivateCaptureProof> {
    super::super::cancellation::check(cancel)?;
    fence(
        c,
        "projection_provider_capture_transactions",
        MAX_TRANSACTIONS,
        "length(CAST(id AS BLOB))=0 OR length(CAST(id AS BLOB))>128 OR length(CAST(mount_id AS BLOB))=0 OR length(CAST(mount_id AS BLOB))>128 OR capture_generation!=1 OR length(CAST(digest AS BLOB))!=64 OR expected_capture_sequence<0 OR length(CAST(expected_capture_digest AS BLOB))>71 OR state NOT IN ('hashing','staging','accepted','cancelled') OR length(CAST(reservation AS BLOB))>128 OR length(payload)>16777216 OR length(CAST(record_digest AS BLOB))!=71",
    )?;
    fence(
        c,
        "projection_provider_capture_receipts",
        MAX_TRANSACTIONS,
        "length(CAST(transaction_id AS BLOB))=0 OR length(CAST(transaction_id AS BLOB))>128 OR capture_sequence<=0 OR length(CAST(capture_digest AS BLOB))!=71 OR length(payload)>1048576 OR length(CAST(digest AS BLOB))!=71",
    )?;
    fence(
        c,
        "projection_captures",
        MAX_CAPTURES,
        "sequence<=0 OR length(CAST(digest AS BLOB))!=71 OR projection_generation!=1 OR length(payload)>8388608",
    )?;
    fence(
        c,
        "projection_content",
        MAX_CONTENT,
        "length(CAST(file_id AS BLOB))!=71 OR length(CAST(manifest_id AS BLOB))!=71 OR bytes<0 OR bytes>1073741824",
    )?;
    fence(
        c,
        "projection_claims",
        MAX_CLAIMS,
        "length(CAST(root_id AS BLOB))!=64 OR length(CAST(digest AS BLOB))!=64 OR length(CAST(kind AS BLOB))>64 OR length(CAST(owner AS BLOB))=0 OR length(CAST(owner AS BLOB))>128",
    )?;
    let total: u64 = c.query_row(
        "SELECT (SELECT coalesce(sum(length(payload)),0) FROM projection_provider_capture_transactions)+(SELECT coalesce(sum(length(payload)),0) FROM projection_provider_capture_receipts)+(SELECT coalesce(sum(length(payload)),0) FROM projection_captures)",
        [],
        |r| r.get(0),
    )?;
    if total > MAX_PAYLOAD_TOTAL {
        return Err(invalid("provider capture aggregate payload exceeds bound"));
    }
    super::super::cancellation::check(cancel)?;
    let reserved = activated
        .snapshot
        .reserved_paths()
        .map_err(|e| invalid(e.to_string()))?;
    super::super::cancellation::check(cancel)?;

    let mut transactions = Vec::new();
    let mut tx_by_id = BTreeMap::new();
    let mut staged_files = BTreeMap::<String, u64>::new();
    let mut stage_by_id = BTreeMap::<String, (String, BTreeSet<String>)>::new();
    let mut active_count = 0u64;
    {
        let mut statement = c.prepare("SELECT id,mount_id,capture_generation,digest,expected_capture_sequence,expected_capture_digest,state,reservation,payload,record_digest FROM projection_provider_capture_transactions ORDER BY id COLLATE BINARY")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            super::super::cancellation::check(cancel)?;
            let id: String = row.get(0)?;
            let mount_id: String = row.get(1)?;
            let generation: u64 = row.get(2)?;
            let digest: String = row.get(3)?;
            let expected_sequence: u64 = row.get(4)?;
            let expected_digest: Option<String> = row.get(5)?;
            let state: String = row.get(6)?;
            let reservation: Option<String> = row.get(7)?;
            let payload: Vec<u8> = row.get(8)?;
            let record_digest: String = row.get(9)?;
            let record = ProviderCaptureTransactionRecord::decode(&payload)?;
            record.require_row(ProviderCaptureTransactionRowData {
                id: &id,
                mount_id: &mount_id,
                capture_generation: generation,
                digest: &digest,
                expected_capture_sequence: expected_sequence,
                expected_capture_digest: expected_digest.as_deref(),
                state: &state,
                reservation: reservation.as_deref(),
                payload: &payload,
                record_digest: &record_digest,
            })?;
            record.require_references(allocation, mount, activation)?;
            if matches!(
                &record.state,
                ProviderCaptureState::Hashing | ProviderCaptureState::Staging { .. }
            ) {
                active_count += 1;
            }
            let stage = match &record.state {
                ProviderCaptureState::Staging { stage }
                | ProviderCaptureState::Accepted { stage, .. }
                | ProviderCaptureState::Cancelled { stage: Some(stage) } => Some(stage),
                _ => None,
            };
            if let Some(stage) = stage {
                captured_change_authority(&stage.changes, activated, &reserved, cancel)?;
                let mut files = BTreeSet::new();
                for change in &stage.changes {
                    super::super::cancellation::check(cancel)?;
                    if let Some(file) = &change.replacement {
                        let name = file.file_id.as_str().to_owned();
                        if staged_files
                            .insert(name.clone(), file.bytes)
                            .is_some_and(|old| old != file.bytes)
                        {
                            return Err(invalid("provider capture staged file size differs"));
                        }
                        files.insert(name);
                    }
                }
                stage_by_id.insert(
                    record.transaction_id.clone(),
                    (stage.reservation.clone(), files),
                );
            }
            tx_by_id.insert(record.transaction_id.clone(), transactions.len());
            transactions.push(record);
        }
    }
    if active_count > 1 {
        return Err(invalid("provider capture has multiple active transactions"));
    }
    super::super::cancellation::check(cancel)?;
    let mut patches = Vec::new();
    {
        let mut statement = c.prepare("SELECT sequence,digest,projection_generation,payload FROM projection_captures ORDER BY sequence")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            super::super::cancellation::check(cancel)?;
            let sequence: u64 = row.get(0)?;
            let digest: String = row.get(1)?;
            let generation: u64 = row.get(2)?;
            let payload: Vec<u8> = row.get(3)?;
            let patch: CapturedPatch =
                serde_json::from_slice(&payload).map_err(|e| invalid(e.to_string()))?;
            if patch.sequence != sequence
                || sequence != patches.len() as u64 + 1
                || patch.digest != digest
                || generation != activated.generation
                || patch.identity != allocation.format.identity
                || patch.previous_patch_digest
                    != patches
                        .last()
                        .map(|prior: &CapturedPatch| prior.digest.clone())
                || canonical_json(&patch).map_err(|e| invalid(e.to_string()))? != payload
            {
                return Err(invalid("provider capture SQL history differs"));
            }
            if patch.projection_generation != activated.generation
                || patch.projection_digest != activated.digest
            {
                return Err(invalid("provider capture base projection differs"));
            }
            captured_change_authority(&patch.changes, activated, &reserved, cancel)?;
            patches.push(patch);
        }
    }
    for tx in &transactions {
        super::super::cancellation::check(cancel)?;
        let prior = if tx.expected_capture_sequence == 0 {
            None
        } else {
            patches
                .get(tx.expected_capture_sequence as usize - 1)
                .map(|p| p.digest.as_str())
        };
        if prior != tx.expected_capture_digest.as_deref() {
            return Err(invalid("provider capture expected head is absent"));
        }
        if matches!(
            &tx.state,
            ProviderCaptureState::Hashing | ProviderCaptureState::Staging { .. }
        ) && (tx.expected_capture_sequence != patches.len() as u64
            || tx.expected_capture_digest.as_deref()
                != patches.last().map(|patch| patch.digest.as_str()))
        {
            return Err(invalid("active provider capture expected head is stale"));
        }
        if let ProviderCaptureState::Accepted { patch, .. } = &tx.state {
            if patches.get(patch.sequence as usize - 1) != Some(patch) {
                return Err(invalid(
                    "provider accepted transaction has no exact capture row",
                ));
            }
        }
    }
    let mut receipts = Vec::new();
    let mut receipt_ids = BTreeSet::new();
    {
        let mut statement = c.prepare("SELECT transaction_id,capture_sequence,capture_digest,payload,digest FROM projection_provider_capture_receipts ORDER BY transaction_id COLLATE BINARY")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            super::super::cancellation::check(cancel)?;
            let id: String = row.get(0)?;
            let sequence: u64 = row.get(1)?;
            let digest: String = row.get(2)?;
            let payload: Vec<u8> = row.get(3)?;
            let row_digest: String = row.get(4)?;
            let receipt = ProviderCaptureReceiptRecord::decode(&payload)?;
            receipt.require_row(ProviderCaptureReceiptRowData {
                transaction_id: &id,
                capture_sequence: sequence,
                capture_digest: &digest,
                payload: &payload,
                digest: &row_digest,
            })?;
            let tx = tx_by_id
                .get(&id)
                .map(|index| &transactions[*index])
                .ok_or_else(|| invalid("provider capture receipt transaction absent"))?;
            receipt.require_references(tx)?;
            receipt_ids.insert(id);
            receipts.push(receipt);
        }
    }
    for tx in &transactions {
        if matches!(&tx.state, ProviderCaptureState::Accepted { .. })
            != receipt_ids.contains(&tx.transaction_id)
        {
            return Err(invalid(
                "provider accepted transaction/receipt pair differs",
            ));
        }
    }
    for patch in &patches {
        if !transactions.iter().any(|tx| {
            matches!(&tx.state,
            ProviderCaptureState::Accepted { patch: accepted, .. } if accepted == patch)
        }) {
            return Err(invalid("provider capture row lacks accepted transaction"));
        }
    }
    let mut content = Vec::new();
    let mut represented = BTreeSet::new();
    {
        let mut statement = c.prepare("SELECT file_id,manifest_id,bytes FROM projection_content ORDER BY file_id COLLATE BINARY")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            super::super::cancellation::check(cancel)?;
            let file_id: String = row.get(0)?;
            let manifest_id: String = row.get(1)?;
            let bytes: u64 = row.get(2)?;
            FileId::parse(file_id.clone()).map_err(|e| invalid(e.to_string()))?;
            FileManifestId::parse(manifest_id.clone()).map_err(|e| invalid(e.to_string()))?;
            if bytes > MAX_CAPTURE_FILE_BYTES || staged_files.get(&file_id) != Some(&bytes) {
                return Err(invalid("provider content lacks a staged replacement"));
            }
            represented.insert(file_id.clone());
            content.push(CaptureContentProof {
                file_id,
                manifest_id,
                bytes,
            });
        }
    }
    for tx in &transactions {
        if let ProviderCaptureState::Accepted { stage, .. } = &tx.state {
            if !stage
                .changes
                .iter()
                .filter_map(|change| change.replacement.as_ref())
                .all(|file| represented.contains(file.file_id.as_str()))
            {
                return Err(invalid("provider accepted capture lacks private content"));
            }
        }
    }
    let mut claims = Vec::new();
    let mut claimed_owners = BTreeSet::new();
    {
        let mut statement = c.prepare("SELECT root_id,digest,kind,owner FROM projection_claims ORDER BY root_id COLLATE BINARY")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            super::super::cancellation::check(cancel)?;
            let root_id: String = row.get(0)?;
            let digest: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let owner: String = row.get(3)?;
            if kind != "provider-capture"
                || root_id != digest
                || !stage_by_id.contains_key(&owner)
                || !claimed_owners.insert(owner.clone())
            {
                return Err(invalid(
                    "provider capture claim lacks a unique staged owner",
                ));
            }
            let (_, files) = &stage_by_id[&owner];
            if !files.iter().all(|file| represented.contains(file)) {
                return Err(invalid("provider capture claim lacks private content"));
            }
            claims.push(CaptureClaimProof {
                root_id,
                digest,
                kind,
                owner,
            });
        }
    }
    for tx in &transactions {
        if matches!(&tx.state, ProviderCaptureState::Accepted { .. })
            && !claimed_owners.contains(&tx.transaction_id)
        {
            return Err(invalid(
                "provider accepted capture lacks private protection claim",
            ));
        }
    }
    // Every SQL relation is bounded before the global traversal. The progress
    // handler on cancellable reads interrupts an unbounded corrupt FK walk.
    super::super::cancellation::check(cancel)?;
    let bad_fk: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |r| r.get(0),
    )?;
    if bad_fk {
        return Err(invalid("provider capture private foreign-key corruption"));
    }
    super::super::cancellation::check(cancel)?;
    Ok(PrivateCaptureProof {
        transactions,
        receipts,
        patches,
        content,
        claims,
    })
}

#[cfg(test)]
#[path = "capture/tests.rs"]
pub(in crate::projection::provider) mod tests;
