//! Pure provider capture transaction and acknowledgement DATA. These records
//! never prove a checkpoint, staged bytes, SQL history, or native READY.
use super::{
    AllocationRecord, Receipt, Record,
    activation::{ProviderActivationRecord, ProviderActiveMountRecord, owner_identity_digest},
    codec, invalid,
};
use crate::projection::{
    Result,
    capture::{CapturedChange, CapturedPatch, MAX_CAPTURE_BYTES, MAX_CAPTURE_FILE_BYTES},
};
use layrs_core::scm::canonical_json;
use serde::{Deserialize, Serialize};

const TRANSACTION_SCHEMA: &str = "layrs.projection-provider-capture-transaction.v1";
const TRANSACTION_DOMAIN: &str = "layrs-projection-provider-capture-transaction-v1";
const RECEIPT_SCHEMA: &str = "layrs.projection-provider-capture-receipt.v1";
const RECEIPT_DOMAIN: &str = "layrs-projection-provider-capture-receipt-v1";
const CAPTURE_DOMAIN: &str = "layrs-projection-capture-v1";
const MAX_IMAGE_PATHS: u64 = 100_000;
const MAX_PATCH_CHANGES: usize = 1024; // Existing CapturedPatch storage contract.
const MAX_CAPTURE_DOCUMENT: usize = 8 * 1024 * 1024;

/// A whole-image commitment and the exact base-relative proposed diff. The
/// digest does not attest that a native checkpoint or its bytes were observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderCaptureStage {
    pub image_digest: String,
    pub image_path_count: u64,
    pub changes: Vec<CapturedChange>,
    /// An owned reservation identifier, not a content/admission capability.
    pub reservation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) enum ProviderCaptureState {
    Hashing,
    Staging {
        stage: ProviderCaptureStage,
    },
    Accepted {
        stage: ProviderCaptureStage,
        patch: CapturedPatch,
    },
    Cancelled {
        stage: Option<ProviderCaptureStage>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderCaptureTransactionRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub owner_identity_digest: String,
    pub mount_id: String,
    pub mount_digest: String,
    pub activation_digest: String,
    pub capture_generation: u64,
    pub capture_digest: String,
    pub expected_capture_sequence: u64,
    pub expected_capture_digest: Option<String>,
    pub state: ProviderCaptureState,
}

pub(in crate::projection) struct ProviderCaptureTransactionRowData<'a> {
    pub id: &'a str,
    pub mount_id: &'a str,
    pub capture_generation: u64,
    pub digest: &'a str,
    pub expected_capture_sequence: u64,
    pub expected_capture_digest: Option<&'a str>,
    pub state: &'a str,
    pub reservation: Option<&'a str>,
    pub payload: &'a [u8],
    pub record_digest: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderCaptureReceiptRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub transaction_digest: String,
    pub owner_identity_digest: String,
    pub mount_digest: String,
    pub activation_digest: String,
    pub capture_generation: u64,
    pub generation_digest: String,
    pub capture_sequence: u64,
    pub capture_digest: String,
}

pub(in crate::projection) struct ProviderCaptureReceiptRowData<'a> {
    pub transaction_id: &'a str,
    pub capture_sequence: u64,
    pub capture_digest: &'a str,
    pub payload: &'a [u8],
    pub digest: &'a str,
}

fn identifier(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || value.bytes().any(|byte| byte == 0) {
        return Err(invalid(
            "provider capture identifier must be 1..128 UTF-8 bytes",
        ));
    }
    Ok(())
}

fn changes(changes: &[CapturedChange]) -> Result<()> {
    if changes.len() > MAX_PATCH_CHANGES {
        return Err(invalid(
            "provider capture diff exceeds existing patch bound",
        ));
    }
    let mut previous: Option<&str> = None;
    let mut total = 0u64;
    for change in changes {
        let path = change.path.as_str();
        if path.is_empty()
            || path.len() > codec::PATH_MAX
            || previous.is_some_and(|prior| prior >= path)
        {
            return Err(invalid(
                "provider capture paths must be bounded, sorted and unique",
            ));
        }
        previous = Some(path);
        if change
            .replacement
            .as_ref()
            .map(|file| (&file.file_id, file.executable))
            == change
                .expected
                .as_ref()
                .map(|file| (&file.file_id, file.executable))
        {
            return Err(invalid(
                "provider capture change has no target/mode difference",
            ));
        }
        if let Some(file) = &change.replacement {
            if file.bytes > MAX_CAPTURE_FILE_BYTES {
                return Err(invalid("provider capture file exceeds bound"));
            }
            total = total
                .checked_add(file.bytes)
                .filter(|bytes| *bytes <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("provider capture replacement bytes exceed bound"))?;
        }
    }
    Ok(())
}

fn validate_stage(stage: &ProviderCaptureStage) -> Result<()> {
    codec::raw_digest(&stage.image_digest)?;
    if stage.image_path_count > MAX_IMAGE_PATHS {
        return Err(invalid(
            "provider capture whole image path count exceeds bound",
        ));
    }
    identifier(&stage.reservation)?;
    changes(&stage.changes)
}

fn patch_digest(patch: &CapturedPatch) -> Result<String> {
    let mut unhashed = patch.clone();
    unhashed.digest.clear();
    codec::preflight(&unhashed, MAX_CAPTURE_DOCUMENT)?;
    let bytes = canonical_json(&unhashed).map_err(invalid)?;
    if bytes.len() > MAX_CAPTURE_DOCUMENT {
        return Err(invalid(
            "provider captured patch exceeds existing document bound",
        ));
    }
    Ok(codec::hash(CAPTURE_DOMAIN, &bytes))
}

fn validate_patch(
    patch: &CapturedPatch,
    transaction: &ProviderCaptureTransactionRecord,
) -> Result<()> {
    if patch.schema_version != "layrs.projection-capture.v1"
        || patch.sequence == 0
        || patch.sequence > i64::MAX as u64
        || patch.projection_generation != transaction.capture_generation
        || patch.projection_digest != transaction.capture_digest
        || patch.digest != patch_digest(patch)?
    {
        return Err(invalid("provider accepted patch anchor or digest differs"));
    }
    changes(&patch.changes)?;
    let expected = transaction.expected_capture_sequence;
    if patch.sequence == expected {
        if transaction.expected_capture_digest.as_deref() != Some(patch.digest.as_str()) {
            return Err(invalid(
                "provider deduplicated patch differs from capture head",
            ));
        }
    } else if Some(patch.sequence) == expected.checked_add(1)
        && patch.previous_patch_digest == transaction.expected_capture_digest
    {
        // New capture extends exactly the head witnessed by this attempt.
    } else {
        return Err(invalid("provider accepted patch ancestry differs"));
    }
    if patch.sequence == 1 && patch.previous_patch_digest.is_some() {
        return Err(invalid("first provider capture has an unexpected ancestor"));
    }
    if patch.sequence > 1 {
        codec::digest_token(
            patch
                .previous_patch_digest
                .as_deref()
                .ok_or_else(|| invalid("provider patch ancestor absent"))?,
        )?;
    }
    Ok(())
}

impl Record for ProviderCaptureTransactionRecord {
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, TRANSACTION_SCHEMA)?;
        identifier(&self.transaction_id)?;
        identifier(&self.mount_id)?;
        codec::digest_token(&self.owner_identity_digest)?;
        codec::digest_token(&self.mount_digest)?;
        codec::digest_token(&self.activation_digest)?;
        codec::raw_digest(&self.capture_digest)?;
        if self.capture_generation != 1 || self.expected_capture_sequence > i64::MAX as u64 {
            return Err(invalid("provider capture generation/head is out of range"));
        }
        match (
            self.expected_capture_sequence,
            self.expected_capture_digest.as_deref(),
        ) {
            (0, None) => {}
            (1.., Some(digest)) => codec::digest_token(digest)?,
            _ => return Err(invalid("provider capture expected head pair differs")),
        }
        match &self.state {
            ProviderCaptureState::Hashing => {}
            ProviderCaptureState::Staging { stage }
            | ProviderCaptureState::Cancelled { stage: Some(stage) } => validate_stage(stage)?,
            ProviderCaptureState::Cancelled { stage: None } => {}
            ProviderCaptureState::Accepted { stage, patch } => {
                validate_stage(stage)?;
                validate_patch(patch, self)?;
                if patch.changes != stage.changes {
                    return Err(invalid(
                        "provider accepted patch differs from staged changes",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl ProviderCaptureTransactionRecord {
    pub fn begin(
        allocation: &AllocationRecord,
        mount: &ProviderActiveMountRecord,
        activation: &ProviderActivationRecord,
        transaction_id: String,
        expected_capture_sequence: u64,
        expected_capture_digest: Option<String>,
    ) -> Result<Self> {
        let record = Self {
            schema_version: TRANSACTION_SCHEMA.into(),
            transaction_id,
            owner_identity_digest: owner_identity_digest(allocation)?,
            mount_id: activation.mount_id.clone(),
            mount_digest: mount.digest()?,
            activation_digest: activation.digest()?,
            capture_generation: activation.activated_generation,
            capture_digest: activation.activated_digest.clone(),
            expected_capture_sequence,
            expected_capture_digest,
            state: ProviderCaptureState::Hashing,
        };
        record.require_references(allocation, mount, activation)?;
        Ok(record)
    }

    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(TRANSACTION_DOMAIN, &self.encode()?))
    }

    pub fn sql_state(&self) -> &'static str {
        match self.state {
            ProviderCaptureState::Hashing => "hashing",
            ProviderCaptureState::Staging { .. } => "staging",
            ProviderCaptureState::Accepted { .. } => "accepted",
            ProviderCaptureState::Cancelled { .. } => "cancelled",
        }
    }

    pub fn reservation(&self) -> Option<&str> {
        match &self.state {
            ProviderCaptureState::Hashing | ProviderCaptureState::Cancelled { stage: None } => None,
            ProviderCaptureState::Staging { stage }
            | ProviderCaptureState::Accepted { stage, .. }
            | ProviderCaptureState::Cancelled { stage: Some(stage) } => Some(&stage.reservation),
        }
    }

    pub fn require_references(
        &self,
        allocation: &AllocationRecord,
        mount: &ProviderActiveMountRecord,
        activation: &ProviderActivationRecord,
    ) -> Result<()> {
        self.validate_fields()?;
        mount.validate_fields()?;
        activation.validate_fields()?;
        let bootstrap = &mount.prepared.bootstrap;
        let binding = &mount.prepared.binding;
        let allocation_ref = allocation.data_reference()?;
        // A mount/map can be internally self-consistent while belonging to a
        // different allocation. Compare the actual A receipt and recovery
        // binding, rather than accepting a copied owner digest as authority.
        if bootstrap.transaction_id != allocation.transaction_id
            || bootstrap.allocation != allocation_ref
            || binding.transaction_id != allocation.transaction_id
            || binding.allocation != allocation_ref
            || bootstrap.prepared.as_ref() != binding.prepared.as_ref().map(|p| &p.receipt)
            || binding.recovery.identity != allocation.format.identity
            || binding.recovery.checkout_identity != allocation.format.checkout_identity()
            || binding.recovery.database_identity != allocation.private.database_file_identity
            || binding.recovery.marker_digest
                != codec::hash(
                    "layrs-projection-recovery-marker-v1",
                    &allocation.format.bytes()?,
                )
            || binding.recovery.family_marker_digest.as_ref()
                != Some(&allocation.family.marker_digest)
            || mount.prepared.source != allocation.source
            || activation.source != allocation.source
        {
            return Err(invalid(
                "provider capture allocation reference/binding differs",
            ));
        }
        if self.owner_identity_digest != owner_identity_digest(allocation)?
            || self.mount_id != activation.mount_id
            || self.mount_digest != mount.digest()?
            || self.activation_digest != activation.digest()?
            || mount.activation_digest != self.activation_digest
            || mount.prepared.bootstrap.transaction_id != self.mount_id
            || activation.owner_identity_digest != self.owner_identity_digest
            || self.capture_generation != activation.activated_generation
            || self.capture_digest != activation.activated_digest
            || ProviderActiveMountRecord::derive(&mount.prepared, activation)? != *mount
        {
            return Err(invalid(
                "provider capture owner/mount/activation binding differs",
            ));
        }
        if let ProviderCaptureState::Accepted { patch, .. } = &self.state {
            if patch.identity != allocation.format.identity {
                return Err(invalid("provider accepted patch identity differs"));
            }
        }
        Ok(())
    }

    /// Exact predecessor for a SQL compare-and-swap. A retry must compare the
    /// entire old row/payload; the transaction ID alone is never sufficient.
    pub fn require_transition(&self, previous: &Self) -> Result<()> {
        self.validate_fields()?;
        previous.validate_fields()?;
        if self.schema_version != previous.schema_version
            || self.transaction_id != previous.transaction_id
            || self.owner_identity_digest != previous.owner_identity_digest
            || self.mount_id != previous.mount_id
            || self.mount_digest != previous.mount_digest
            || self.activation_digest != previous.activation_digest
            || self.capture_generation != previous.capture_generation
            || self.capture_digest != previous.capture_digest
            || self.expected_capture_sequence != previous.expected_capture_sequence
            || self.expected_capture_digest != previous.expected_capture_digest
        {
            return Err(invalid(
                "provider capture transition changed attempt identity",
            ));
        }
        match (&previous.state, &self.state) {
            (ProviderCaptureState::Hashing, ProviderCaptureState::Staging { .. })
            | (ProviderCaptureState::Hashing, ProviderCaptureState::Cancelled { stage: None }) => {}
            (
                ProviderCaptureState::Staging { stage: old },
                ProviderCaptureState::Accepted { stage, .. },
            )
            | (
                ProviderCaptureState::Staging { stage: old },
                ProviderCaptureState::Cancelled { stage: Some(stage) },
            ) if stage == old => {}
            _ => return Err(invalid("provider capture state transition differs")),
        }
        Ok(())
    }

    pub fn require_row(&self, row: ProviderCaptureTransactionRowData<'_>) -> Result<()> {
        self.validate_fields()?;
        if row.id != self.transaction_id
            || row.mount_id != self.mount_id
            || row.capture_generation != self.capture_generation
            || row.digest != self.capture_digest
            || row.expected_capture_sequence != self.expected_capture_sequence
            || row.expected_capture_digest != self.expected_capture_digest.as_deref()
            || row.state != self.sql_state()
            || row.reservation != self.reservation()
            || row.payload != self.encode()?
            || row.record_digest != self.digest()?
        {
            return Err(invalid("provider capture transaction SQL row differs"));
        }
        Ok(())
    }
}

impl Record for ProviderCaptureReceiptRecord {
    const MAX_BYTES: usize = 1024 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, RECEIPT_SCHEMA)?;
        identifier(&self.transaction_id)?;
        codec::digest_token(&self.transaction_digest)?;
        codec::digest_token(&self.owner_identity_digest)?;
        codec::digest_token(&self.mount_digest)?;
        codec::digest_token(&self.activation_digest)?;
        codec::raw_digest(&self.generation_digest)?;
        codec::digest_token(&self.capture_digest)?;
        if self.capture_generation != 1
            || self.capture_sequence == 0
            || self.capture_sequence > i64::MAX as u64
        {
            return Err(invalid(
                "provider capture receipt generation/sequence differs",
            ));
        }
        Ok(())
    }
}

impl ProviderCaptureReceiptRecord {
    pub fn derive(transaction: &ProviderCaptureTransactionRecord) -> Result<Self> {
        transaction.validate_fields()?;
        let ProviderCaptureState::Accepted { patch, .. } = &transaction.state else {
            return Err(invalid(
                "provider capture receipt requires accepted transaction",
            ));
        };
        let record = Self {
            schema_version: RECEIPT_SCHEMA.into(),
            transaction_id: transaction.transaction_id.clone(),
            transaction_digest: transaction.digest()?,
            owner_identity_digest: transaction.owner_identity_digest.clone(),
            mount_digest: transaction.mount_digest.clone(),
            activation_digest: transaction.activation_digest.clone(),
            capture_generation: transaction.capture_generation,
            generation_digest: transaction.capture_digest.clone(),
            capture_sequence: patch.sequence,
            capture_digest: patch.digest.clone(),
        };
        record.validate_fields()?;
        Ok(record)
    }

    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(RECEIPT_DOMAIN, &self.encode()?))
    }

    pub fn require_references(&self, transaction: &ProviderCaptureTransactionRecord) -> Result<()> {
        if self != &Self::derive(transaction)? {
            return Err(invalid(
                "provider capture receipt transaction binding differs",
            ));
        }
        Ok(())
    }

    pub fn require_row(&self, row: ProviderCaptureReceiptRowData<'_>) -> Result<()> {
        self.validate_fields()?;
        if row.transaction_id != self.transaction_id
            || row.capture_sequence != self.capture_sequence
            || row.capture_digest != self.capture_digest
            || row.payload != self.encode()?
            || row.digest != self.digest()?
        {
            return Err(invalid("provider capture receipt SQL row differs"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "capture/tests.rs"]
mod tests;
