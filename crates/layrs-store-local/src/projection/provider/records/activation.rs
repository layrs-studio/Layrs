//! Pure source-to-capture generation placement DATA. SQL, native READY, and
//! family registry authority belong to later validators and consuming holders.
use super::{
    AllocationRecord, PreparedRecord, ProviderBootstrapBinding, ProviderBootstrapSource, Record,
    SourceCommitment, codec, invalid,
    preparation::{MountRowData, PreparationState, ProviderPreparationRecord},
};
use crate::projection::{Result, database};
use layrs_core::{
    projection::{EntryState, ProjectionSnapshot},
    scm::{TreeEntryTarget, canonical_json},
};
use serde::{Deserialize, Serialize};

const ACTIVATION_SCHEMA: &str = "layrs.projection-provider-activation.v1";
const ACTIVATION_DOMAIN: &str = "layrs-projection-provider-activation-v1";
const ACTIVE_MOUNT_SCHEMA: &str = "layrs.projection-provider-mount.v3";
const ACTIVE_MOUNT_DOMAIN: &str = "layrs-projection-provider-mount-v3";
const OWNER_DOMAIN: &str = "layrs-projection-provider-owner-identity-v1";

/// One exact SQL-shaped activation row. DATA comparison does not check rowset
/// cardinality, foreign keys, a family registry or native READY.
pub(in crate::projection) struct ActivationRowData<'a> {
    pub id: &'a str,
    pub owner_identity_digest: &'a str,
    pub mount_id: &'a str,
    pub catalog_id: &'a str,
    pub source_generation: u64,
    pub source_digest: &'a str,
    pub activated_generation: u64,
    pub activated_digest: &'a str,
    pub payload: &'a [u8],
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderActivationRecord {
    pub schema_version: String,
    pub owner_identity_digest: String,
    pub mount_id: String,
    pub catalog_id: String,
    pub binding: ProviderBootstrapBinding,
    pub source: SourceCommitment,
    pub prepared_digest: String,
    pub source_generation: u64,
    pub source_digest: String,
    pub selection_digest: String,
    pub activated_generation: u64,
    pub activated_digest: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderActiveMountRecord {
    pub schema_version: String,
    pub prepared: ProviderPreparationRecord,
    pub activation_digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnerIdentity<'a> {
    family_root: &'a str,
    family_incarnation: &'a str,
    family_marker_digest: &'a str,
    family_root_identity: &'a str,
    format_identity: &'a crate::projection::ProjectionIdentity,
    format_file_id: &'a str,
    checkout_identity: &'a str,
    private_metadata_path: &'a str,
    private_metadata_identity: &'a str,
    private_database_file_identity: &'a str,
    private_control_identity: &'a str,
}

/// Stable for this exact FORMAT/family/private member; never a SQL proof.
pub(in crate::projection) fn owner_identity_digest(a: &AllocationRecord) -> Result<String> {
    a.validate_fields()?;
    let identity = OwnerIdentity {
        family_root: &a.family.root,
        family_incarnation: &a.family.incarnation,
        family_marker_digest: &a.family.marker_digest,
        family_root_identity: &a.family.root_identity,
        format_identity: &a.format.identity,
        format_file_id: a.format_file_id.as_str(),
        checkout_identity: a.format.checkout_identity(),
        private_metadata_path: &a.private.metadata_path,
        private_metadata_identity: &a.private.metadata_identity,
        private_database_file_identity: &a.private.database_file_identity,
        private_control_identity: &a.private.provider_control_identity,
    };
    codec::preflight(&identity, 16 * 1024)?;
    Ok(codec::hash(
        OWNER_DOMAIN,
        &canonical_json(&identity).map_err(invalid)?,
    ))
}

/// Placement changes only the selected catalog files from Unavailable to
/// Materialized. Here Materialized means native placeholder installation, not
/// resident bytes. No content, filesystem, SQL or native API is consulted.
pub(in crate::projection) fn activation_snapshot(
    source: &ProviderBootstrapSource,
) -> Result<ProjectionSnapshot> {
    source.initial().validate()?;
    source.selection().validate_snapshot(source.initial())?;
    source.commitment().validate_fields()?;
    if source.commitment().source_generation != 0 {
        return Err(invalid("activation requires source generation zero"));
    }
    let mut after = source.initial().clone();
    for descriptor in source.descriptors() {
        if !source.selection().includes(&descriptor.path) {
            return Err(invalid("activation descriptor is outside selection"));
        }
        let index = after
            .entries
            .binary_search_by(|entry| entry.path.cmp(&descriptor.path))
            .map_err(|_| invalid("activation descriptor has no initial entry"))?;
        let entry = &mut after.entries[index];
        if entry.state != EntryState::Unavailable
            || entry.executable != descriptor.executable
            || !matches!(&entry.target, Some(TreeEntryTarget::File { file_id }) if file_id == &descriptor.file_id)
        {
            return Err(invalid(
                "activation descriptor differs from cold file entry",
            ));
        }
        entry.state = EntryState::Materialized;
    }
    after.validate()?;
    Ok(after)
}

impl Record for ProviderActivationRecord {
    const MAX_BYTES: usize = 1024 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, ACTIVATION_SCHEMA)?;
        codec::digest_token(&self.owner_identity_digest)?;
        codec::transaction(&self.mount_id)?;
        codec::digest_token(&self.catalog_id)?;
        codec::digest_token(&self.prepared_digest)?;
        codec::raw_digest(&self.source_digest)?;
        codec::raw_digest(&self.selection_digest)?;
        codec::raw_digest(&self.activated_digest)?;
        self.binding.validate_fields()?;
        self.source.validate_fields()?;
        if self.binding.prepared.is_none()
            || self.binding.abandoned.is_some()
            || self.mount_id != self.binding.transaction_id
            || self.catalog_id != self.source.catalog_id
            || self.source_generation != 0
            || self.source_generation != self.source.source_generation
            || self.source_digest != self.source.snapshot_digest
            || self.selection_digest != self.source.selection_digest
            || self.activated_generation != 1
        {
            return Err(invalid("activation source/placement shape differs"));
        }
        Ok(())
    }
}
impl ProviderActivationRecord {
    pub fn derive(
        a: &AllocationRecord,
        p: &PreparedRecord,
        prepared: &ProviderPreparationRecord,
        source: &ProviderBootstrapSource,
    ) -> Result<(Self, ProjectionSnapshot)> {
        prepared.require_references(a, p, source, prepared.root_id()?)?;
        if !matches!(prepared.state, PreparationState::ProviderPrepared { .. }) {
            return Err(invalid(
                "activation requires v2 ProviderPrepared predecessor",
            ));
        }
        let snapshot = activation_snapshot(source)?;
        let payload = snapshot.canonical_bytes()?;
        let source_digest = source.commitment().snapshot_digest.clone();
        let activation = Self {
            schema_version: ACTIVATION_SCHEMA.into(),
            owner_identity_digest: owner_identity_digest(a)?,
            mount_id: prepared.bootstrap.transaction_id.clone(),
            catalog_id: source.commitment().catalog_id.clone(),
            binding: ProviderBootstrapBinding::decode(&prepared.binding.encode()?)?,
            source: serde_json::from_slice(
                &serde_json::to_vec(source.commitment()).map_err(invalid)?,
            )
            .map_err(invalid)?,
            prepared_digest: prepared.digest()?,
            source_generation: 0,
            source_digest: source_digest.clone(),
            selection_digest: source.commitment().selection_digest.clone(),
            activated_generation: 1,
            activated_digest: database::digest(1, Some(&source_digest), &payload),
        };
        activation.validate_fields()?;
        Ok((activation, snapshot))
    }

    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(ACTIVATION_DOMAIN, &self.encode()?))
    }

    pub fn require_references(
        &self,
        a: &AllocationRecord,
        p: &PreparedRecord,
        prepared: &ProviderPreparationRecord,
        source: &ProviderBootstrapSource,
        activated: &ProjectionSnapshot,
    ) -> Result<()> {
        self.validate_fields()?;
        let (expected, snapshot) = Self::derive(a, p, prepared, source)?;
        if self != &expected || activated != &snapshot {
            return Err(invalid(
                "activation source/selection/placement binding differs",
            ));
        }
        Ok(())
    }

    pub fn require_row(&self, row: ActivationRowData<'_>) -> Result<()> {
        self.validate_fields()?;
        if row.id != self.digest()?
            || row.owner_identity_digest != self.owner_identity_digest
            || row.mount_id != self.mount_id
            || row.catalog_id != self.catalog_id
            || row.source_generation != self.source_generation
            || row.source_digest != self.source_digest
            || row.activated_generation != self.activated_generation
            || row.activated_digest != self.activated_digest
            || row.payload != self.encode()?
        {
            return Err(invalid("activation SQL row differs"));
        }
        Ok(())
    }
}

impl Record for ProviderActiveMountRecord {
    const MAX_BYTES: usize = 256 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, ACTIVE_MOUNT_SCHEMA)?;
        self.prepared.validate_fields()?;
        if !matches!(
            self.prepared.state,
            PreparationState::ProviderPrepared { .. }
        ) {
            return Err(invalid("active mount requires prepared v2 predecessor"));
        }
        codec::digest_token(&self.activation_digest)
    }
}
impl ProviderActiveMountRecord {
    pub fn derive(
        prepared: &ProviderPreparationRecord,
        activation: &ProviderActivationRecord,
    ) -> Result<Self> {
        prepared.validate_fields()?;
        activation.validate_fields()?;
        if !matches!(prepared.state, PreparationState::ProviderPrepared { .. })
            || activation.prepared_digest != prepared.digest()?
            || activation.mount_id != prepared.bootstrap.transaction_id
            || activation.catalog_id != prepared.source.catalog_id
            || activation.binding != prepared.binding
            || activation.source != prepared.source
        {
            return Err(invalid("active mount predecessor/activation differs"));
        }
        let record = Self {
            schema_version: ACTIVE_MOUNT_SCHEMA.into(),
            prepared: ProviderPreparationRecord::decode(&prepared.encode()?)?,
            activation_digest: activation.digest()?,
        };
        record.validate_fields()?;
        Ok(record)
    }

    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(ACTIVE_MOUNT_DOMAIN, &self.encode()?))
    }
    pub fn sql_phase(&self) -> &'static str {
        "active"
    }

    pub fn require_references(
        &self,
        a: &AllocationRecord,
        p: &PreparedRecord,
        source: &ProviderBootstrapSource,
        activation: &ProviderActivationRecord,
        activated: &ProjectionSnapshot,
    ) -> Result<()> {
        self.validate_fields()?;
        activation.require_references(a, p, &self.prepared, source, activated)?;
        if self != &Self::derive(&self.prepared, activation)? {
            return Err(invalid("active mount activation link differs"));
        }
        Ok(())
    }

    pub fn require_predecessor_row(&self, row: MountRowData<'_>) -> Result<()> {
        self.validate_fields()?;
        self.prepared.require_row(row)
    }

    pub fn require_row(&self, row: MountRowData<'_>) -> Result<()> {
        self.validate_fields()?;
        if row.id != self.prepared.bootstrap.transaction_id
            || row.catalog_id != self.prepared.source.catalog_id
            || row.phase != "active"
            || row.payload != self.encode()?
            || row.digest != self.digest()?
        {
            return Err(invalid("active mount SQL row differs"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "activation/tests.rs"]
mod tests;
