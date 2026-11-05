use super::{ClientId, Cursor, IdempotencyKey, MutationId, PolicySet, ProductEnvelope};
use layrs_core::scm::{
    LayerId, ObjectId, RepositoryView, RepositoryViewId, SpaceId, Step, StepObservation,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVersionExpectationError {
    DuplicateCapturedPolicyProduct(ObjectId),
    AuthorizationEpochChanged {
        expected: Vec<ObjectId>,
        captured: Vec<ObjectId>,
    },
}
impl PolicyVersionExpectationError {
    pub const fn code(&self) -> &'static str {
        "authorization_epoch_changed"
    }
}
impl fmt::Display for PolicyVersionExpectationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateCapturedPolicyProduct(id) => write!(f, "captured PolicySet repeats Policy Product {id}"),
            Self::AuthorizationEpochChanged { .. } => f.write_str("expected Policy Product set differs from the PolicySet captured at transaction start"),
        }
    }
}
impl Error for PolicyVersionExpectationError {}

pub fn captured_policy_version_object_ids(
    policy_set: &PolicySet,
) -> Result<Vec<ObjectId>, PolicyVersionExpectationError> {
    let mut ids = policy_set
        .policy_versions
        .iter()
        .map(|entry| entry.policy_product_object_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    if let Some(duplicate) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(
            PolicyVersionExpectationError::DuplicateCapturedPolicyProduct(duplicate[0].clone()),
        );
    }
    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncV3Protocol {
    #[serde(rename = "layrs.sync.v3")]
    V3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionRequestType {
    #[serde(rename = "syncTransactionRequest")]
    Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerMutationMode {
    Advance,
    Integrate,
    Choose,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerMutationIntent {
    pub layer_id: LayerId,
    pub mode: LayerMutationMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_product_object_id: Option<ObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ProposedRepositoryView {
    #[serde(rename = "inline")]
    Inline { view: RepositoryView },
    #[serde(rename = "stored")]
    Stored {
        #[serde(rename = "repositoryViewId")]
        repository_view_id: RepositoryViewId,
    },
}

impl ProposedRepositoryView {
    pub fn id(&self) -> &RepositoryViewId {
        match self {
            Self::Inline { view } => view.id(),
            Self::Stored { repository_view_id } => repository_view_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryViewMutation {
    pub mutation_id: MutationId,
    pub expected_current_view_id: RepositoryViewId,
    pub expected_generation: u64,
    pub proposed_view: ProposedRepositoryView,
    pub layer_mutations: Vec<LayerMutationIntent>,
    pub expected_policy_version_object_ids: Vec<ObjectId>,
}

impl RepositoryViewMutation {
    /// Enforces the exact, sorted Policy Product object-ID set from the
    /// PolicySet locked at transaction start. It is never a subset check.
    pub fn verify_captured_policy_versions(
        &self,
        captured_policy_set: &PolicySet,
    ) -> Result<(), PolicyVersionExpectationError> {
        let captured = captured_policy_version_object_ids(captured_policy_set)?;
        if self.expected_policy_version_object_ids != captured {
            return Err(PolicyVersionExpectationError::AuthorizationEpochChanged {
                expected: self.expected_policy_version_object_ids.clone(),
                captured,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ProposedPolicySet {
    #[serde(rename = "inline")]
    Inline {
        #[serde(rename = "policySet")]
        policy_set: PolicySet,
    },
    #[serde(rename = "stored")]
    Stored {
        #[serde(rename = "policySetId")]
        policy_set_id: ObjectId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRootMutation {
    pub mutation_id: MutationId,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
    pub proposed_policy_set: ProposedPolicySet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTransactionRequest {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: TransactionRequestType,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: ObjectId,
    pub known_event_cursor: Option<Cursor>,
    pub steps: Vec<Step>,
    pub observations: Vec<StepObservation>,
    pub products: Vec<ProductEnvelope>,
    pub view_mutation: Option<RepositoryViewMutation>,
    pub policy_root_mutation: Option<PolicyRootMutation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransactionWire {
    protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    message_type: TransactionRequestType,
    space_id: SpaceId,
    client_id: ClientId,
    idempotency_key: IdempotencyKey,
    request_digest: ObjectId,
    #[serde(default, deserialize_with = "present_nullable")]
    known_event_cursor: Option<Option<Cursor>>,
    steps: Vec<Step>,
    observations: Vec<StepObservation>,
    products: Vec<ProductEnvelope>,
    #[serde(default, deserialize_with = "present_nullable")]
    view_mutation: Option<Option<RepositoryViewMutation>>,
    #[serde(default, deserialize_with = "present_nullable")]
    policy_root_mutation: Option<Option<PolicyRootMutation>>,
}

fn present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for SyncTransactionRequest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = TransactionWire::deserialize(d)?;
        Ok(Self {
            protocol: wire.protocol,
            message_type: wire.message_type,
            space_id: wire.space_id,
            client_id: wire.client_id,
            idempotency_key: wire.idempotency_key,
            request_digest: wire.request_digest,
            known_event_cursor: wire
                .known_event_cursor
                .ok_or_else(|| serde::de::Error::missing_field("knownEventCursor"))?,
            steps: wire.steps,
            observations: wire.observations,
            products: wire.products,
            view_mutation: wire
                .view_mutation
                .ok_or_else(|| serde::de::Error::missing_field("viewMutation"))?,
            policy_root_mutation: wire
                .policy_root_mutation
                .ok_or_else(|| serde::de::Error::missing_field("policyRootMutation"))?,
        })
    }
}

impl SyncTransactionRequest {
    pub fn computed_request_digest(&self) -> Result<ObjectId, serde_json::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SemanticEnvelope<'a> {
            protocol: SyncV3Protocol,
            #[serde(rename = "type")]
            message_type: TransactionRequestType,
            space_id: &'a SpaceId,
            client_id: &'a ClientId,
            idempotency_key: &'a IdempotencyKey,
            known_event_cursor: &'a Option<Cursor>,
            steps: &'a [Step],
            observations: &'a [StepObservation],
            products: &'a [ProductEnvelope],
            view_mutation: &'a Option<RepositoryViewMutation>,
            policy_root_mutation: &'a Option<PolicyRootMutation>,
        }
        let bytes = serde_jcs::to_vec(&SemanticEnvelope {
            protocol: self.protocol,
            message_type: self.message_type,
            space_id: &self.space_id,
            client_id: &self.client_id,
            idempotency_key: &self.idempotency_key,
            known_event_cursor: &self.known_event_cursor,
            steps: &self.steps,
            observations: &self.observations,
            products: &self.products,
            view_mutation: &self.view_mutation,
            policy_root_mutation: &self.policy_root_mutation,
        })?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs.sync.v3/request-digest\0");
        hasher.update(&bytes);
        Ok(ObjectId::parse(format!("blake3:{}", hasher.finalize()))
            .expect("BLAKE3 is a valid ObjectId"))
    }

    /// Returns the closed HTTP 409 DTO for a malformed request digest before
    /// semantic evaluation or idempotency-row creation.
    pub fn presemantic_digest_error(
        &self,
    ) -> Result<Option<super::SyncTransactionError>, serde_json::Error> {
        let computed = self.computed_request_digest()?;
        Ok((computed != self.request_digest).then(|| {
            super::SyncTransactionError::request_digest_mismatch(
                self.space_id.clone(),
                self.client_id.clone(),
                self.idempotency_key.clone(),
                self.request_digest.clone(),
                computed,
            )
        }))
    }
}
