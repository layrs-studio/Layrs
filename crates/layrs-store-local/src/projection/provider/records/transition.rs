//! Pure schema-8 transition intent DATA. This codec reads no SQL or filesystem,
//! and proves neither permissions, resident bytes, native READY nor publication.
use super::{
    codec,
    import::{
        ImportRowset, ImportSource, ProviderImportKind, ProviderImportPrefixRecord,
        ProviderTransitionReceiptRecord,
    },
    invalid, Record, Result,
};
use crate::projection::{format, ProjectionIdentity};
use layrs_core::scm::{ObjectId, StepId, TreeId};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SCHEMA: &str = "layrs.projection-provider-transition-intent.v1";
const DOMAIN: &str = "layrs-projection-provider-transition-intent-v1";
const PLAN_DOMAIN: &str = "layrs-projection-provider-transition-plan-v1";
const DESTINATION_DOMAIN: &str = "layrs-projection-provider-transition-destination-v1";
const OWNER_DOMAIN: &str = "layrs-projection-provider-transition-owner-v1";
const RESERVATION_DOMAIN: &str = "layrs-projection-provider-transition-reservation-v1";
const MAX_ROWS: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::projection) enum TransitionRole {
    Source,
    Destination,
}
impl TransitionRole {
    fn sql(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Destination => "destination",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::projection) enum TransitionState {
    Planned,
    Copying,
    Staging,
    Ready,
    Published,
    Cancelled,
}
impl TransitionState {
    fn sql(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Copying => "copying",
            Self::Staging => "staging",
            Self::Ready => "ready",
            Self::Published => "published",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A separately stored, ordered rowset. No history, namespace or path array is
/// embedded in a transition row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct TransitionRowset {
    pub count: u64,
    pub digest: String,
}
impl TransitionRowset {
    fn validate(&self) -> Result<()> {
        if self.count > MAX_ROWS {
            return Err(invalid("transition rowset count exceeds bound"));
        }
        codec::digest_token(&self.digest)
    }
    fn matches_import(&self, rowset: &ImportRowset) -> bool {
        self.count == rowset.count && self.digest == rowset.digest
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct TransitionObservation {
    pub step_id: StepId,
    pub root_tree_id: TreeId,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub generation: u64,
    pub snapshot_digest: String,
    pub selection_digest: String,
}
impl TransitionObservation {
    fn validate(&self) -> Result<()> {
        if self.authorization_epoch == 0 || self.generation > i64::MAX as u64 {
            return Err(invalid("transition observation epoch/generation invalid"));
        }
        codec::raw_digest(&self.snapshot_digest)?;
        codec::raw_digest(&self.selection_digest)
    }
}

/// Immutable intent. `source_prefix` is a compact historical header, not the
/// imported rows. Its digest is bound before any destination directory exists.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct TransitionPlan {
    pub kind: ProviderImportKind,
    pub source_prefix_id: String,
    pub source_prefix: ProviderImportPrefixRecord,
    pub destination_parent: String,
    pub destination_parent_identity: String,
    pub destination_leaf_nonce: String,
    pub observation: TransitionObservation,
    pub placement_generation: u64,
    pub placement_digest: String,
    pub capture_sequence: u64,
    pub capture_digest: String,
    pub working_overlay: TransitionRowset,
    pub companion_id: Option<String>,
    pub companion_files: TransitionRowset,
    pub companion_deletions: TransitionRowset,
    pub protected_manifests: TransitionRowset,
    pub quota_bytes: u64,
}
impl TransitionPlan {
    /// Includes the embedded source prefix and every immutable transition
    /// commitment. It excludes mutable phase, destination pins and receipt ID.
    pub fn digest(&self) -> Result<String> {
        codec::preflight(self, ProviderTransitionIntentRecord::MAX_BYTES)?;
        self.validate()?;
        Ok(codec::hash(
            PLAN_DOMAIN,
            &codec::encode(self, ProviderTransitionIntentRecord::MAX_BYTES)?,
        ))
    }
    fn source_identity(&self) -> &ProjectionIdentity {
        match &self.source_prefix.source {
            ImportSource::Ordinary { identity, .. } | ImportSource::Provider { identity, .. } => {
                identity
            }
        }
    }
    fn source_generation(&self) -> u64 {
        match &self.source_prefix.source {
            ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => {
                version.generation
            }
        }
    }
    fn source_capture_count(&self) -> u64 {
        match &self.source_prefix.source {
            ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => {
                version.capture_count
            }
        }
    }
    fn validate(&self) -> Result<()> {
        self.source_prefix.encode()?;
        format::validate_identity(self.source_identity())?;
        codec::digest_token(&self.source_prefix_id)?;
        if self.source_prefix_id != self.source_prefix.digest()?
            || self.kind != self.source_prefix.kind
        {
            return Err(invalid("transition source prefix differs"));
        }
        codec::locator(&self.destination_parent)?;
        codec::directory_identity(&self.destination_parent_identity)?;
        if !codec::lower_hex(&self.destination_leaf_nonce, 32)
            || Path::new(&self.destination_leaf_nonce).components().count() != 1
        {
            return Err(invalid("transition destination leaf nonce invalid"));
        }
        self.observation.validate()?;
        if self.observation.generation <= self.source_generation()
            || self.placement_generation <= self.observation.generation
            || self.placement_generation > i64::MAX as u64
            || self.capture_sequence <= self.source_capture_count()
            || self.capture_sequence > i64::MAX as u64
            || self.quota_bytes > i64::MAX as u64
        {
            return Err(invalid(
                "transition destination boundary exceeds source or SQLite",
            ));
        }
        codec::raw_digest(&self.placement_digest)?;
        codec::digest_token(&self.capture_digest)?;
        for rowset in [
            &self.working_overlay,
            &self.companion_files,
            &self.companion_deletions,
            &self.protected_manifests,
        ] {
            rowset.validate()?;
        }
        let total = [
            &self.working_overlay,
            &self.companion_files,
            &self.companion_deletions,
            &self.protected_manifests,
        ]
        .iter()
        .try_fold(0u64, |sum, rowset| codec::add(sum, rowset.count))?;
        if total > MAX_ROWS {
            return Err(invalid("transition aggregate row count exceeds bound"));
        }
        if self.companion_id.is_none()
            && (self.companion_files.count != 0 || self.companion_deletions.count != 0)
        {
            return Err(invalid(
                "transition companion rows without companion header",
            ));
        }
        if let Some(id) = &self.companion_id {
            codec::digest_token(id)?;
        }
        Ok(())
    }
}

/// Actual destination FORMAT and filesystem identities, admitted only after
/// creation. They are expected DATA until an external pinned reader proves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::projection) enum TransitionBackendKind {
    Ordinary,
    Provider,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct TransitionDestinationProof {
    pub backend: TransitionBackendKind,
    pub format: format::Marker,
    /// Identity of the exclusively allocated FORMAT file, retained across its
    /// temporary-name publication. Required before resuming a partial write.
    pub format_file_identity: String,
    pub private_database_identity: String,
    pub family_marker_digest: String,
    pub native_control_identity_hex: Option<String>,
    pub native_root_identity: Option<String>,
}
impl TransitionDestinationProof {
    pub const MAX_FORMAT_BYTES: usize = codec::FORMAT_MAX;

    /// Physical owner commitment from FORMAT, family marker and private file
    /// identity. Native registration identity is excluded so renewal cannot
    /// silently change the durable owner.
    pub fn owner_digest(&self, plan: &TransitionPlan) -> Result<String> {
        self.validate(plan)?;
        let data = (
            &self.format,
            &self.format_file_identity,
            &self.private_database_identity,
            &self.family_marker_digest,
            &self.native_control_identity_hex,
        );
        codec::preflight(&data, ProviderTransitionIntentRecord::MAX_BYTES)?;
        Ok(codec::hash(
            OWNER_DOMAIN,
            &layrs_core::scm::canonical_json(&data).map_err(invalid)?,
        ))
    }

    /// Full post-creation commitment. The reader must independently verify
    /// these expected DATA against pinned files and native registration.
    pub fn digest(&self, plan: &TransitionPlan) -> Result<String> {
        self.validate(plan)?;
        Ok(codec::hash(
            DESTINATION_DOMAIN,
            &codec::encode(self, ProviderTransitionIntentRecord::MAX_BYTES)?,
        ))
    }

    fn validate(&self, plan: &TransitionPlan) -> Result<()> {
        self.format.validate()?;
        if self.format.bytes()?.len() > Self::MAX_FORMAT_BYTES
            || self.format.identity != plan.source_prefix.destination_identity
            || self.format.import_digest.as_deref() != Some(&plan.source_prefix_id)
            || self.format.checkout_root
                != Path::new(&plan.destination_parent)
                    .join(&plan.destination_leaf_nonce)
                    .to_str()
                    .ok_or_else(|| invalid("non-UTF8 destination root"))?
            || self
                .format
                .shared
                .as_ref()
                .map(|binding| binding.database_identity.as_str())
                != Some(self.private_database_identity.as_str())
        {
            return Err(invalid("transition destination FORMAT differs"));
        }
        codec::file_identity(&self.private_database_identity)?;
        codec::file_identity(&self.format_file_identity)?;
        if self.format_file_identity == self.private_database_identity {
            return Err(invalid("transition FORMAT aliases private database"));
        }
        codec::digest_token(&self.family_marker_digest)?;
        if let Some(hex) = &self.native_control_identity_hex {
            let bytes = codec::decode_hex(hex, layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES)?;
            let control = layrs_vfs::ControlDirectoryIdentity::decode(&bytes).map_err(invalid)?;
            if control.encode() != bytes
                || control.stage_identity().format_binding_identity() != self.format_file_identity
            {
                return Err(invalid("transition destination control identity differs"));
            }
        }
        if let Some(identity) = &self.native_root_identity {
            if identity.is_empty() || identity.len() > 512 || identity.contains('\0') {
                return Err(invalid("transition native root identity invalid"));
            }
        }
        if self.native_root_identity.is_some()
            != (plan.kind == ProviderImportKind::ExpandedProvider)
            || self.native_control_identity_hex.is_some()
                != (plan.kind == ProviderImportKind::ExpandedProvider)
        {
            return Err(invalid("transition destination native backend differs"));
        }
        if !matches!(
            (self.backend, plan.kind),
            (
                TransitionBackendKind::Ordinary,
                ProviderImportKind::ManagedOrdinary
            ) | (
                TransitionBackendKind::Provider,
                ProviderImportKind::ExpandedProvider
            )
        ) {
            return Err(invalid("transition destination backend differs"));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderTransitionIntentRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub role: TransitionRole,
    pub state: TransitionState,
    pub local_generation: u64,
    pub plan: TransitionPlan,
    #[serde(deserialize_with = "codec::required_option")]
    pub destination: Option<TransitionDestinationProof>,
    #[serde(deserialize_with = "codec::required_option")]
    pub reservation: Option<String>,
    #[serde(deserialize_with = "codec::required_option")]
    pub receipt_id: Option<String>,
}
impl Record for ProviderTransitionIntentRecord {
    const MAX_BYTES: usize = 256 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, SCHEMA)?;
        codec::transaction(&self.transaction_id)?;
        self.plan.validate()?;
        if self.local_generation > i64::MAX as u64
            || self.local_generation
                != match self.role {
                    TransitionRole::Source => self.plan.source_generation(),
                    TransitionRole::Destination => 0,
                }
            || (self.role == TransitionRole::Destination && self.state == TransitionState::Planned)
            || (self.role == TransitionRole::Destination && self.destination.is_none())
            || (self.state == TransitionState::Planned && self.destination.is_some())
            || (self.state != TransitionState::Planned
                && self.state != TransitionState::Cancelled
                && self.destination.is_none())
            || (matches!(
                self.state,
                TransitionState::Ready | TransitionState::Published
            ) != self.receipt_id.is_some())
            || (matches!(
                self.state,
                TransitionState::Staging | TransitionState::Ready | TransitionState::Published
            ) && self.reservation.is_none())
            || (self.receipt_id.is_some() && self.destination.is_none())
        {
            return Err(invalid("transition role/state/anchor differs"));
        }
        if let Some(proof) = &self.destination {
            proof.validate(&self.plan)?;
        }
        if let Some(reservation) = &self.reservation {
            let expected = self.reservation_operation()?;
            if reservation != &expected || reservation.len() > 128 {
                return Err(invalid("transition reservation operation differs"));
            }
        }
        if let Some(id) = &self.receipt_id {
            codec::digest_token(id)?;
        }
        Ok(())
    }
}
impl ProviderTransitionIntentRecord {
    pub fn reservation_operation(&self) -> Result<String> {
        self.plan.validate()?;
        codec::transaction(&self.transaction_id)?;
        let key = (
            &self.transaction_id,
            &self.plan.source_prefix.destination_identity,
            &self.plan.source_prefix_id,
        );
        codec::preflight(&key, 16 * 1024)?;
        let digest = codec::hash(
            RESERVATION_DOMAIN,
            &layrs_core::scm::canonical_json(&key).map_err(invalid)?,
        );
        Ok(format!("provider-transition:{}", &digest[7..]))
    }

    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(DOMAIN, &self.encode()?))
    }
    pub fn require_row(&self, row: TransitionRowData<'_>) -> Result<()> {
        if row.id != self.transaction_id
            || row.role != self.role.sql()
            || row.state != self.state.sql()
            || row.local_generation != self.local_generation
            || row.reservation != self.reservation.as_deref()
            || row.receipt_id != self.receipt_id.as_deref()
            || row.payload != self.encode()?
            || row.digest != self.digest()?
        {
            return Err(invalid("transition SQL row projection differs"));
        }
        Ok(())
    }
    pub fn require_prefix(&self, prefix: &ProviderImportPrefixRecord) -> Result<()> {
        self.validate_fields()?;
        prefix.validate_fields()?;
        if &self.plan.source_prefix != prefix || self.plan.source_prefix_id != prefix.digest()? {
            return Err(invalid("transition import prefix differs"));
        }
        Ok(())
    }
    pub fn require_receipt(&self, receipt: &ProviderTransitionReceiptRecord) -> Result<()> {
        self.validate_fields()?;
        receipt.validate_fields()?;
        receipt.require_prefix(&self.plan.source_prefix)?;
        if self.receipt_id.as_deref() != Some(receipt.digest()?.as_str())
            || receipt.transition_plan_digest != self.plan.digest()?
            || receipt.transaction_id != self.transaction_id
            || receipt.owner_identity_digest
                != self
                    .destination
                    .as_ref()
                    .ok_or_else(|| invalid("transition receipt has no destination"))?
                    .owner_digest(&self.plan)?
            || receipt.destination_proof_digest
                != self
                    .destination
                    .as_ref()
                    .ok_or_else(|| invalid("transition receipt has no destination"))?
                    .digest(&self.plan)?
            || receipt.destination_observation_generation != self.plan.observation.generation
            || receipt.destination_observation_digest != self.plan.observation.snapshot_digest
            || receipt.destination_placement_generation != self.plan.placement_generation
            || receipt.destination_placement_digest != self.plan.placement_digest
            || receipt.destination_capture_sequence != self.plan.capture_sequence
            || receipt.destination_capture_digest != self.plan.capture_digest
            || !self
                .plan
                .protected_manifests
                .matches_import(&receipt.protected_manifests)
            || receipt.companion_id != self.plan.companion_id
            || !self
                .plan
                .companion_files
                .matches_import(&receipt.companion_files)
            || !self
                .plan
                .companion_deletions
                .matches_import(&receipt.companion_deletions)
        {
            return Err(invalid("transition receipt differs from planned closure"));
        }
        Ok(())
    }
    /// Pure CAS preflight. The writer must still compare every SQL column and
    /// validate all external effects before issuing a transaction.
    pub fn require_transition(&self, previous: &Self) -> Result<()> {
        self.validate_fields()?;
        previous.validate_fields()?;
        let legal = match (previous.state, self.state) {
            (TransitionState::Planned, TransitionState::Copying)
            | (TransitionState::Copying, TransitionState::Staging)
            | (TransitionState::Staging, TransitionState::Ready)
            | (TransitionState::Ready, TransitionState::Published)
            | (
                TransitionState::Planned | TransitionState::Copying | TransitionState::Staging,
                TransitionState::Cancelled,
            ) => true,
            (a, b) if a == b => {
                self == previous
                    || matches!(a, TransitionState::Planned | TransitionState::Copying)
                        && previous.reservation.is_none()
                        && self.reservation.is_some()
                        && self.destination == previous.destination
                        && self.receipt_id == previous.receipt_id
            }
            _ => false,
        };
        if !legal
            || self.transaction_id != previous.transaction_id
            || self.role != previous.role
            || self.local_generation != previous.local_generation
            || self.plan != previous.plan
            || (previous.destination.is_some() && self.destination != previous.destination)
            || (previous.reservation.is_some() && self.reservation != previous.reservation)
            || (previous.receipt_id.is_some() && self.receipt_id != previous.receipt_id)
        {
            return Err(invalid("transition intent or phase changed"));
        }
        Ok(())
    }
}

pub(in crate::projection) struct TransitionRowData<'a> {
    pub id: &'a str,
    pub role: &'a str,
    pub state: &'a str,
    pub local_generation: u64,
    pub reservation: Option<&'a str>,
    pub receipt_id: Option<&'a str>,
    pub payload: &'a [u8],
    pub digest: &'a str,
}

#[cfg(test)]
#[path = "transition/tests.rs"]
mod tests;
