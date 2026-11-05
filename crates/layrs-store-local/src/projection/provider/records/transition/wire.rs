//! Closed versioned wire shapes. Shared DATA Options are never optional fields
//! on an older wire version. No SQL, authority or body authentication is supplied.
use super::*;
use serde::ser::SerializeStruct;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanV1 {
    kind: ProviderImportKind,
    source_prefix_id: String,
    source_prefix: ProviderImportPrefixRecord,
    destination_parent: String,
    destination_parent_identity: String,
    destination_leaf_nonce: String,
    observation: TransitionObservation,
    placement_generation: u64,
    placement_digest: String,
    capture_sequence: u64,
    capture_digest: String,
    working_overlay: TransitionRowset,
    companion_id: Option<String>,
    companion_files: TransitionRowset,
    companion_deletions: TransitionRowset,
    protected_manifests: TransitionRowset,
    quota_bytes: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanV2 {
    kind: ProviderImportKind,
    source_prefix_id: String,
    source_prefix: ProviderImportPrefixRecord,
    destination_parent: String,
    destination_parent_identity: String,
    destination_leaf_nonce: String,
    observation: TransitionObservation,
    placement_generation: u64,
    placement_digest: String,
    capture_sequence: u64,
    capture_digest: String,
    working_overlay: TransitionRowset,
    companion_id: Option<String>,
    companion_files: TransitionRowset,
    companion_deletions: TransitionRowset,
    protected_manifests: TransitionRowset,
    quota_bytes: u64,
    denial_coverage: DeniedCoverageRef,
}
impl From<PlanV1> for TransitionPlan {
    fn from(value: PlanV1) -> Self {
        Self {
            kind: value.kind,
            source_prefix_id: value.source_prefix_id,
            source_prefix: value.source_prefix,
            destination_parent: value.destination_parent,
            destination_parent_identity: value.destination_parent_identity,
            destination_leaf_nonce: value.destination_leaf_nonce,
            observation: value.observation,
            placement_generation: value.placement_generation,
            placement_digest: value.placement_digest,
            capture_sequence: value.capture_sequence,
            capture_digest: value.capture_digest,
            working_overlay: value.working_overlay,
            companion_id: value.companion_id,
            companion_files: value.companion_files,
            companion_deletions: value.companion_deletions,
            protected_manifests: value.protected_manifests,
            quota_bytes: value.quota_bytes,
            denial_coverage: None,
        }
    }
}
impl From<PlanV2> for TransitionPlan {
    fn from(value: PlanV2) -> Self {
        Self {
            kind: value.kind,
            source_prefix_id: value.source_prefix_id,
            source_prefix: value.source_prefix,
            destination_parent: value.destination_parent,
            destination_parent_identity: value.destination_parent_identity,
            destination_leaf_nonce: value.destination_leaf_nonce,
            observation: value.observation,
            placement_generation: value.placement_generation,
            placement_digest: value.placement_digest,
            capture_sequence: value.capture_sequence,
            capture_digest: value.capture_digest,
            working_overlay: value.working_overlay,
            companion_id: value.companion_id,
            companion_files: value.companion_files,
            companion_deletions: value.companion_deletions,
            protected_manifests: value.protected_manifests,
            quota_bytes: value.quota_bytes,
            denial_coverage: Some(value.denial_coverage),
        }
    }
}
impl Serialize for TransitionPlan {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct(
            "TransitionPlan",
            17 + usize::from(self.denial_coverage.is_some()),
        )?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("sourcePrefixId", &self.source_prefix_id)?;
        state.serialize_field("sourcePrefix", &self.source_prefix)?;
        state.serialize_field("destinationParent", &self.destination_parent)?;
        state.serialize_field(
            "destinationParentIdentity",
            &self.destination_parent_identity,
        )?;
        state.serialize_field("destinationLeafNonce", &self.destination_leaf_nonce)?;
        state.serialize_field("observation", &self.observation)?;
        state.serialize_field("placementGeneration", &self.placement_generation)?;
        state.serialize_field("placementDigest", &self.placement_digest)?;
        state.serialize_field("captureSequence", &self.capture_sequence)?;
        state.serialize_field("captureDigest", &self.capture_digest)?;
        state.serialize_field("workingOverlay", &self.working_overlay)?;
        state.serialize_field("companionId", &self.companion_id)?;
        state.serialize_field("companionFiles", &self.companion_files)?;
        state.serialize_field("companionDeletions", &self.companion_deletions)?;
        state.serialize_field("protectedManifests", &self.protected_manifests)?;
        state.serialize_field("quotaBytes", &self.quota_bytes)?;
        if let Some(value) = &self.denial_coverage {
            state.serialize_field("denialCoverage", value)?;
        }
        state.end()
    }
}
#[derive(Deserialize)]
#[serde(untagged)]
enum StandalonePlan {
    V2(PlanV2),
    V1(PlanV1),
}
impl<'de> Deserialize<'de> for TransitionPlan {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(match StandalonePlan::deserialize(deserializer)? {
            StandalonePlan::V1(value) => value.into(),
            StandalonePlan::V2(value) => value.into(),
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IntentBody<P> {
    transaction_id: String,
    role: TransitionRole,
    state: TransitionState,
    local_generation: u64,
    plan: P,
    #[serde(deserialize_with = "codec::required_option")]
    destination: Option<TransitionDestinationProof>,
    #[serde(deserialize_with = "codec::required_option")]
    reservation: Option<String>,
    #[serde(deserialize_with = "codec::required_option")]
    receipt_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "schemaVersion")]
enum IntentWire {
    #[serde(rename = "layrs.projection-provider-transition-intent.v1")]
    V1(IntentBody<PlanV1>),
    #[serde(rename = "layrs.projection-provider-transition-intent.v2")]
    V2(IntentBody<PlanV2>),
}
impl<P: Into<TransitionPlan>> IntentBody<P> {
    fn into_record(self, schema: &str) -> ProviderTransitionIntentRecord {
        ProviderTransitionIntentRecord {
            schema_version: schema.into(),
            transaction_id: self.transaction_id,
            role: self.role,
            state: self.state,
            local_generation: self.local_generation,
            plan: self.plan.into(),
            destination: self.destination,
            reservation: self.reservation,
            receipt_id: self.receipt_id,
        }
    }
}
impl<'de> Deserialize<'de> for ProviderTransitionIntentRecord {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(match IntentWire::deserialize(deserializer)? {
            IntentWire::V1(value) => value.into_record(SCHEMA),
            IntentWire::V2(value) => value.into_record(SCHEMA_V2),
        })
    }
}
impl Serialize for ProviderTransitionIntentRecord {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if !matches!(
            (
                self.schema_version.as_str(),
                self.plan.denial_coverage.is_some()
            ),
            (SCHEMA, false) | (SCHEMA_V2, true)
        ) {
            return Err(serde::ser::Error::custom(
                "transition intent version/denial coverage differs",
            ));
        }
        let mut state = serializer.serialize_struct("ProviderTransitionIntentRecord", 9)?;
        state.serialize_field("schemaVersion", &self.schema_version)?;
        state.serialize_field("transactionId", &self.transaction_id)?;
        state.serialize_field("role", &self.role)?;
        state.serialize_field("state", &self.state)?;
        state.serialize_field("localGeneration", &self.local_generation)?;
        state.serialize_field("plan", &self.plan)?;
        state.serialize_field("destination", &self.destination)?;
        state.serialize_field("reservation", &self.reservation)?;
        state.serialize_field("receiptId", &self.receipt_id)?;
        state.end()
    }
}
