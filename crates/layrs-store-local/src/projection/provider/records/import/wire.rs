//! Closed versioned wire shapes. Shared DATA Options are never optional fields
//! on an older wire version. No SQL, authority or body authentication is supplied.
use super::*;
use serde::ser::SerializeStruct;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClosureV1 {
    snapshots: ImportRowset,
    selections: ImportRowset,
    captures: ImportRowset,
    publications: ImportRowset,
    catalogs: ImportRowset,
    descriptors: ImportRowset,
    activation_maps: ImportRowset,
    transition_receipts: ImportRowset,
    companions: ImportRowset,
    companion_files: ImportRowset,
    companion_deletions: ImportRowset,
    content_mappings: ImportRowset,
}
impl From<ClosureV1> for ImportClosure {
    fn from(value: ClosureV1) -> Self {
        Self {
            snapshots: value.snapshots,
            selections: value.selections,
            captures: value.captures,
            publications: value.publications,
            catalogs: value.catalogs,
            descriptors: value.descriptors,
            activation_maps: value.activation_maps,
            transition_receipts: value.transition_receipts,
            companions: value.companions,
            companion_files: value.companion_files,
            companion_deletions: value.companion_deletions,
            content_mappings: value.content_mappings,
            restore_history: None,
            denial_observations: None,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClosureV2 {
    snapshots: ImportRowset,
    selections: ImportRowset,
    captures: ImportRowset,
    publications: ImportRowset,
    catalogs: ImportRowset,
    descriptors: ImportRowset,
    activation_maps: ImportRowset,
    transition_receipts: ImportRowset,
    companions: ImportRowset,
    companion_files: ImportRowset,
    companion_deletions: ImportRowset,
    content_mappings: ImportRowset,
    restore_history: ImportRowset,
}
impl From<ClosureV2> for ImportClosure {
    fn from(value: ClosureV2) -> Self {
        Self {
            snapshots: value.snapshots,
            selections: value.selections,
            captures: value.captures,
            publications: value.publications,
            catalogs: value.catalogs,
            descriptors: value.descriptors,
            activation_maps: value.activation_maps,
            transition_receipts: value.transition_receipts,
            companions: value.companions,
            companion_files: value.companion_files,
            companion_deletions: value.companion_deletions,
            content_mappings: value.content_mappings,
            restore_history: Some(value.restore_history),
            denial_observations: None,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClosureV3 {
    snapshots: ImportRowset,
    selections: ImportRowset,
    captures: ImportRowset,
    publications: ImportRowset,
    catalogs: ImportRowset,
    descriptors: ImportRowset,
    activation_maps: ImportRowset,
    transition_receipts: ImportRowset,
    companions: ImportRowset,
    companion_files: ImportRowset,
    companion_deletions: ImportRowset,
    content_mappings: ImportRowset,
    restore_history: ImportRowset,
    denial_observations: ImportRowset,
}
impl From<ClosureV3> for ImportClosure {
    fn from(value: ClosureV3) -> Self {
        Self {
            snapshots: value.snapshots,
            selections: value.selections,
            captures: value.captures,
            publications: value.publications,
            catalogs: value.catalogs,
            descriptors: value.descriptors,
            activation_maps: value.activation_maps,
            transition_receipts: value.transition_receipts,
            companions: value.companions,
            companion_files: value.companion_files,
            companion_deletions: value.companion_deletions,
            content_mappings: value.content_mappings,
            restore_history: Some(value.restore_history),
            denial_observations: Some(value.denial_observations),
        }
    }
}
impl Serialize for ImportClosure {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if self.denial_observations.is_some() && self.restore_history.is_none() {
            return Err(serde::ser::Error::custom(
                "denial observations require restore history",
            ));
        }
        let mut state = serializer.serialize_struct(
            "ImportClosure",
            12 + usize::from(self.restore_history.is_some())
                + usize::from(self.denial_observations.is_some()),
        )?;
        state.serialize_field("snapshots", &self.snapshots)?;
        state.serialize_field("selections", &self.selections)?;
        state.serialize_field("captures", &self.captures)?;
        state.serialize_field("publications", &self.publications)?;
        state.serialize_field("catalogs", &self.catalogs)?;
        state.serialize_field("descriptors", &self.descriptors)?;
        state.serialize_field("activationMaps", &self.activation_maps)?;
        state.serialize_field("transitionReceipts", &self.transition_receipts)?;
        state.serialize_field("companions", &self.companions)?;
        state.serialize_field("companionFiles", &self.companion_files)?;
        state.serialize_field("companionDeletions", &self.companion_deletions)?;
        state.serialize_field("contentMappings", &self.content_mappings)?;
        if let Some(value) = &self.restore_history {
            state.serialize_field("restoreHistory", value)?;
        }
        if let Some(value) = &self.denial_observations {
            state.serialize_field("denialObservations", value)?;
        }
        state.end()
    }
}
#[derive(Deserialize)]
#[serde(untagged)]
enum StandaloneClosure {
    V3(ClosureV3),
    V2(ClosureV2),
    V1(ClosureV1),
}
impl<'de> Deserialize<'de> for ImportClosure {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(match StandaloneClosure::deserialize(deserializer)? {
            StandaloneClosure::V1(value) => value.into(),
            StandaloneClosure::V2(value) => value.into(),
            StandaloneClosure::V3(value) => value.into(),
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrefixBody<C> {
    kind: ProviderImportKind,
    source: ImportSource,
    destination_identity: ProjectionIdentity,
    inherited_segments: ImportRowset,
    ancestry_root_receipt: Option<String>,
    closure: C,
}
#[derive(Deserialize)]
#[serde(tag = "schemaVersion")]
enum PrefixWire {
    #[serde(rename = "layrs.projection-provider-import-prefix.v1")]
    V1(PrefixBody<ClosureV1>),
    #[serde(rename = "layrs.projection-provider-import-prefix.v2")]
    V2(PrefixBody<ClosureV2>),
    #[serde(rename = "layrs.projection-provider-import-prefix.v3")]
    V3(PrefixBody<ClosureV3>),
}
impl<C: Into<ImportClosure>> PrefixBody<C> {
    fn into_record(self, schema: &str) -> ProviderImportPrefixRecord {
        ProviderImportPrefixRecord {
            schema_version: schema.into(),
            kind: self.kind,
            source: self.source,
            destination_identity: self.destination_identity,
            inherited_segments: self.inherited_segments,
            ancestry_root_receipt: self.ancestry_root_receipt,
            closure: self.closure.into(),
        }
    }
}
impl<'de> Deserialize<'de> for ProviderImportPrefixRecord {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(match PrefixWire::deserialize(deserializer)? {
            PrefixWire::V1(value) => value.into_record(PREFIX_SCHEMA),
            PrefixWire::V2(value) => value.into_record(PREFIX_SCHEMA_V2),
            PrefixWire::V3(value) => value.into_record(PREFIX_SCHEMA_V3),
        })
    }
}
impl Serialize for ProviderImportPrefixRecord {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if !matches!(
            (
                self.schema_version.as_str(),
                self.closure.restore_history.is_some(),
                self.closure.denial_observations.is_some()
            ),
            (PREFIX_SCHEMA, false, false)
                | (PREFIX_SCHEMA_V2, true, false)
                | (PREFIX_SCHEMA_V3, true, true)
        ) {
            return Err(serde::ser::Error::custom(
                "import prefix version/closure differs",
            ));
        }
        let mut state = serializer.serialize_struct("ProviderImportPrefixRecord", 7)?;
        state.serialize_field("schemaVersion", &self.schema_version)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("source", &self.source)?;
        state.serialize_field("destinationIdentity", &self.destination_identity)?;
        state.serialize_field("inheritedSegments", &self.inherited_segments)?;
        state.serialize_field("ancestryRootReceipt", &self.ancestry_root_receipt)?;
        state.serialize_field("closure", &self.closure)?;
        state.end()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptV1 {
    owner_identity_digest: String,
    transaction_id: String,
    transition_plan_digest: String,
    destination_proof_digest: String,
    source_prefix_id: String,
    source_prefix: ProviderImportPrefixRecord,
    parent_receipt_id: Option<String>,
    source_identity: ProjectionIdentity,
    destination_identity: ProjectionIdentity,
    imported_max_generation: u64,
    destination_observation_generation: u64,
    destination_observation_digest: String,
    destination_placement_generation: u64,
    destination_placement_digest: String,
    imported_capture_count: u64,
    destination_capture_sequence: u64,
    destination_capture_digest: String,
    protected_manifests: ImportRowset,
    content_mappings: ImportRowset,
    companion_id: Option<String>,
    companion_files: ImportRowset,
    companion_deletions: ImportRowset,
}
impl From<ReceiptV1> for ProviderTransitionReceiptRecord {
    fn from(value: ReceiptV1) -> Self {
        Self {
            owner_identity_digest: value.owner_identity_digest,
            transaction_id: value.transaction_id,
            transition_plan_digest: value.transition_plan_digest,
            destination_proof_digest: value.destination_proof_digest,
            source_prefix_id: value.source_prefix_id,
            source_prefix: value.source_prefix,
            parent_receipt_id: value.parent_receipt_id,
            source_identity: value.source_identity,
            destination_identity: value.destination_identity,
            imported_max_generation: value.imported_max_generation,
            destination_observation_generation: value.destination_observation_generation,
            destination_observation_digest: value.destination_observation_digest,
            destination_placement_generation: value.destination_placement_generation,
            destination_placement_digest: value.destination_placement_digest,
            imported_capture_count: value.imported_capture_count,
            destination_capture_sequence: value.destination_capture_sequence,
            destination_capture_digest: value.destination_capture_digest,
            protected_manifests: value.protected_manifests,
            content_mappings: value.content_mappings,
            companion_id: value.companion_id,
            companion_files: value.companion_files,
            companion_deletions: value.companion_deletions,
            schema_version: RECEIPT_SCHEMA.into(),
            denial_coverage: None,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptV2 {
    owner_identity_digest: String,
    transaction_id: String,
    transition_plan_digest: String,
    destination_proof_digest: String,
    source_prefix_id: String,
    source_prefix: ProviderImportPrefixRecord,
    parent_receipt_id: Option<String>,
    source_identity: ProjectionIdentity,
    destination_identity: ProjectionIdentity,
    imported_max_generation: u64,
    destination_observation_generation: u64,
    destination_observation_digest: String,
    destination_placement_generation: u64,
    destination_placement_digest: String,
    imported_capture_count: u64,
    destination_capture_sequence: u64,
    destination_capture_digest: String,
    protected_manifests: ImportRowset,
    content_mappings: ImportRowset,
    companion_id: Option<String>,
    companion_files: ImportRowset,
    companion_deletions: ImportRowset,
    denial_coverage: DeniedCoverageRef,
}
impl From<ReceiptV2> for ProviderTransitionReceiptRecord {
    fn from(value: ReceiptV2) -> Self {
        Self {
            owner_identity_digest: value.owner_identity_digest,
            transaction_id: value.transaction_id,
            transition_plan_digest: value.transition_plan_digest,
            destination_proof_digest: value.destination_proof_digest,
            source_prefix_id: value.source_prefix_id,
            source_prefix: value.source_prefix,
            parent_receipt_id: value.parent_receipt_id,
            source_identity: value.source_identity,
            destination_identity: value.destination_identity,
            imported_max_generation: value.imported_max_generation,
            destination_observation_generation: value.destination_observation_generation,
            destination_observation_digest: value.destination_observation_digest,
            destination_placement_generation: value.destination_placement_generation,
            destination_placement_digest: value.destination_placement_digest,
            imported_capture_count: value.imported_capture_count,
            destination_capture_sequence: value.destination_capture_sequence,
            destination_capture_digest: value.destination_capture_digest,
            protected_manifests: value.protected_manifests,
            content_mappings: value.content_mappings,
            companion_id: value.companion_id,
            companion_files: value.companion_files,
            companion_deletions: value.companion_deletions,
            schema_version: RECEIPT_SCHEMA_V2.into(),
            denial_coverage: Some(value.denial_coverage),
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "schemaVersion")]
enum ReceiptWire {
    #[serde(rename = "layrs.projection-provider-transition-receipt.v1")]
    V1(ReceiptV1),
    #[serde(rename = "layrs.projection-provider-transition-receipt.v2")]
    V2(ReceiptV2),
}
impl<'de> Deserialize<'de> for ProviderTransitionReceiptRecord {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(match ReceiptWire::deserialize(deserializer)? {
            ReceiptWire::V1(value) => value.into(),
            ReceiptWire::V2(value) => value.into(),
        })
    }
}
impl Serialize for ProviderTransitionReceiptRecord {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if !matches!(
            (self.schema_version.as_str(), self.denial_coverage.is_some()),
            (RECEIPT_SCHEMA, false) | (RECEIPT_SCHEMA_V2, true)
        ) {
            return Err(serde::ser::Error::custom(
                "transition receipt version/denial coverage differs",
            ));
        }
        let mut state = serializer.serialize_struct(
            "ProviderTransitionReceiptRecord",
            23 + usize::from(self.denial_coverage.is_some()),
        )?;
        state.serialize_field("schemaVersion", &self.schema_version)?;
        state.serialize_field("ownerIdentityDigest", &self.owner_identity_digest)?;
        state.serialize_field("transactionId", &self.transaction_id)?;
        state.serialize_field("transitionPlanDigest", &self.transition_plan_digest)?;
        state.serialize_field("destinationProofDigest", &self.destination_proof_digest)?;
        state.serialize_field("sourcePrefixId", &self.source_prefix_id)?;
        state.serialize_field("sourcePrefix", &self.source_prefix)?;
        state.serialize_field("parentReceiptId", &self.parent_receipt_id)?;
        state.serialize_field("sourceIdentity", &self.source_identity)?;
        state.serialize_field("destinationIdentity", &self.destination_identity)?;
        state.serialize_field("importedMaxGeneration", &self.imported_max_generation)?;
        state.serialize_field(
            "destinationObservationGeneration",
            &self.destination_observation_generation,
        )?;
        state.serialize_field(
            "destinationObservationDigest",
            &self.destination_observation_digest,
        )?;
        state.serialize_field(
            "destinationPlacementGeneration",
            &self.destination_placement_generation,
        )?;
        state.serialize_field(
            "destinationPlacementDigest",
            &self.destination_placement_digest,
        )?;
        state.serialize_field("importedCaptureCount", &self.imported_capture_count)?;
        state.serialize_field(
            "destinationCaptureSequence",
            &self.destination_capture_sequence,
        )?;
        state.serialize_field("destinationCaptureDigest", &self.destination_capture_digest)?;
        state.serialize_field("protectedManifests", &self.protected_manifests)?;
        state.serialize_field("contentMappings", &self.content_mappings)?;
        state.serialize_field("companionId", &self.companion_id)?;
        state.serialize_field("companionFiles", &self.companion_files)?;
        state.serialize_field("companionDeletions", &self.companion_deletions)?;
        if let Some(value) = &self.denial_coverage {
            state.serialize_field("denialCoverage", value)?;
        }
        state.end()
    }
}
