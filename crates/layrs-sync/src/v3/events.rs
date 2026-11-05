use super::{
    ClientId, Cursor, EventId, LayerMutationResult, MutationId, Nullable, PrincipalId, ProductRoot,
    SyncV3Protocol, TransactionId, ViewOutcome,
};
use layrs_core::scm::{
    ActorId, DeviceId, LayerId, ObjectId, ObservationId, RepositoryViewId, SpaceId, StepId,
    Timestamp,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActorKind {
    Account,
    Device,
    Automation,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventActor {
    pub kind: ActorKind,
    pub actor_id: ActorId,
    pub device_id: Nullable<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SpaceEvent {
    #[serde(rename = "steps.accepted")]
    StepsAccepted {
        sequence: u64,
        #[serde(rename = "eventId")]
        event_id: EventId,
        actor: EventActor,
        #[serde(rename = "transactionId")]
        transaction_id: TransactionId,
        payload: StepsAcceptedPayload,
        #[serde(rename = "createdAt")]
        created_at: Timestamp,
    },
    #[serde(rename = "products.accepted")]
    ProductsAccepted {
        sequence: u64,
        #[serde(rename = "eventId")]
        event_id: EventId,
        actor: EventActor,
        #[serde(rename = "transactionId")]
        transaction_id: TransactionId,
        payload: ProductsAcceptedPayload,
        #[serde(rename = "createdAt")]
        created_at: Timestamp,
    },
    #[serde(rename = "layer.definition_changed")]
    LayerDefinitionChanged {
        sequence: u64,
        #[serde(rename = "eventId")]
        event_id: EventId,
        actor: EventActor,
        #[serde(rename = "transactionId")]
        transaction_id: TransactionId,
        payload: LayerDefinitionChangedPayload,
        #[serde(rename = "createdAt")]
        created_at: Timestamp,
    },
    #[serde(rename = "repository_view.changed")]
    RepositoryViewChanged {
        sequence: u64,
        #[serde(rename = "eventId")]
        event_id: EventId,
        actor: EventActor,
        #[serde(rename = "transactionId")]
        transaction_id: TransactionId,
        payload: RepositoryViewChangedPayload,
        #[serde(rename = "createdAt")]
        created_at: Timestamp,
    },
    #[serde(rename = "policy.root_changed")]
    PolicyRootChanged {
        sequence: u64,
        #[serde(rename = "eventId")]
        event_id: EventId,
        actor: EventActor,
        #[serde(rename = "transactionId")]
        transaction_id: TransactionId,
        payload: PolicyRootChangedPayload,
        #[serde(rename = "createdAt")]
        created_at: Timestamp,
    },
}

impl SpaceEvent {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::StepsAccepted { sequence, .. }
            | Self::ProductsAccepted { sequence, .. }
            | Self::LayerDefinitionChanged { sequence, .. }
            | Self::RepositoryViewChanged { sequence, .. }
            | Self::PolicyRootChanged { sequence, .. } => *sequence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepsAcceptedPayload {
    pub step_ids: Vec<StepId>,
    pub observation_ids: Vec<ObservationId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductsAcceptedPayload {
    pub products: Vec<ProductRoot>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerDefinitionChangedPayload {
    pub layer_id: LayerId,
    pub previous_product_object_id: Nullable<ObjectId>,
    pub product_object_id: ObjectId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryViewChangedPayload {
    pub mutation_id: MutationId,
    pub outcome: ViewOutcome,
    pub old_repository_view_id: RepositoryViewId,
    pub proposed_repository_view_id: RepositoryViewId,
    pub new_repository_view_id: RepositoryViewId,
    pub old_generation: u64,
    pub new_generation: u64,
    pub layer_results: Vec<LayerMutationResult>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRootChangedPayload {
    pub mutation_id: MutationId,
    pub old_policy_set_id: ObjectId,
    pub new_policy_set_id: ObjectId,
    pub old_authorization_epoch: u64,
    pub new_authorization_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventPageRequestType {
    #[serde(rename = "spaceEventPageRequest")]
    Request,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventPageResponseType {
    #[serde(rename = "spaceEventPage")]
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceEventPageRequest {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: EventPageRequestType,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub cursor: Cursor,
    pub max_events: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceEventPage {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: EventPageResponseType,
    pub principal_id: PrincipalId,
    pub authorization_epoch: u64,
    pub current_policy_set_id: ObjectId,
    pub space_id: SpaceId,
    pub events: Vec<SpaceEvent>,
    pub scanned_through_sequence: u64,
    pub next_cursor: Cursor,
    pub has_more: bool,
}
