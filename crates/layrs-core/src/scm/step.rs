use super::canonical::{domain_hash, domain_separated_bytes};
use super::error::{Result, ScmError};
use super::ids::{
    ActorId, DeviceId, ObservationId, OperationId, SpaceId, StepId, TreeId, WorktreeId,
};
use super::repository::{CaptureReason, Metadata, Timestamp};
use super::serde_support::deserialize_present_nullable;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;

const STEP_DOMAIN: &str = "layrs-step-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    id: StepId,
    parent_step_ids: Vec<StepId>,
    root_tree_id: TreeId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StepRef<'a> {
    schema_version: &'static str,
    step_id: &'a StepId,
    parent_step_ids: &'a [StepId],
    root_tree_id: &'a TreeId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StepWire {
    schema_version: String,
    step_id: StepId,
    parent_step_ids: Vec<StepId>,
    root_tree_id: TreeId,
}

impl Step {
    pub fn new(mut parent_step_ids: Vec<StepId>, root_tree_id: TreeId) -> Result<Self> {
        let mut seen = HashSet::with_capacity(parent_step_ids.len());
        for parent in &parent_step_ids {
            if !seen.insert(parent.as_str().to_owned()) {
                return Err(ScmError::DuplicateStepParent(parent.to_string()));
            }
        }
        parent_step_ids.sort();
        let payload = canonical_step_payload(&parent_step_ids, &root_tree_id);
        let id = StepId::from_digest(domain_hash(STEP_DOMAIN, &payload));
        if parent_step_ids.binary_search(&id).is_ok() {
            return Err(ScmError::InvalidField {
                field: "parentStepIds",
                reason: "a Step cannot be its own parent".into(),
            });
        }
        Ok(Self {
            id,
            parent_step_ids,
            root_tree_id,
        })
    }

    pub fn id(&self) -> &StepId {
        &self.id
    }

    pub fn parent_step_ids(&self) -> &[StepId] {
        &self.parent_step_ids
    }

    pub fn root_tree_id(&self) -> &TreeId {
        &self.root_tree_id
    }

    pub fn canonical_payload(&self) -> Vec<u8> {
        canonical_step_payload(&self.parent_step_ids, &self.root_tree_id)
    }

    pub fn identity_bytes(&self) -> Vec<u8> {
        domain_separated_bytes(STEP_DOMAIN, &self.canonical_payload())
    }
}

fn canonical_step_payload(parents: &[StepId], root: &TreeId) -> Vec<u8> {
    let mut payload = format!("parent-count\t{}\n", parents.len());
    for parent in parents {
        payload.push_str("parent\t");
        payload.push_str(parent.as_str());
        payload.push('\n');
    }
    payload.push_str("root-tree\t");
    payload.push_str(root.as_str());
    payload.push('\n');
    payload.into_bytes()
}

impl Serialize for Step {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        StepRef {
            schema_version: "layrs.step.v1",
            step_id: &self.id,
            parent_step_ids: &self.parent_step_ids,
            root_tree_id: &self.root_tree_id,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = StepWire::deserialize(deserializer)?;
        if wire.schema_version != "layrs.step.v1" {
            return Err(serde::de::Error::custom("unsupported Step schemaVersion"));
        }
        let value =
            Self::new(wire.parent_step_ids, wire.root_tree_id).map_err(serde::de::Error::custom)?;
        if value.id != wire.step_id {
            return Err(serde::de::Error::custom(ScmError::IdentityMismatch {
                claimed: wire.step_id.to_string(),
                computed: value.id.to_string(),
            }));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepObservation {
    id: ObservationId,
    step_id: StepId,
    space_id: Option<SpaceId>,
    operation_id: Option<OperationId>,
    worktree_id: Option<WorktreeId>,
    actor_id: Option<ActorId>,
    device_id: Option<DeviceId>,
    observed_at: Timestamp,
    capture_reason: CaptureReason,
    metadata: Metadata,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StepObservationRef<'a> {
    schema_version: &'static str,
    observation_id: &'a ObservationId,
    step_id: &'a StepId,
    #[serde(skip_serializing_if = "Option::is_none")]
    space_id: &'a Option<SpaceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: &'a Option<OperationId>,
    worktree_id: &'a Option<WorktreeId>,
    actor_id: &'a Option<ActorId>,
    device_id: &'a Option<DeviceId>,
    observed_at: &'a Timestamp,
    capture_reason: CaptureReason,
    metadata: &'a Metadata,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StepObservationWire {
    schema_version: String,
    observation_id: ObservationId,
    step_id: StepId,
    #[serde(default)]
    space_id: Option<SpaceId>,
    #[serde(default)]
    operation_id: Option<OperationId>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    worktree_id: Option<Option<WorktreeId>>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    actor_id: Option<Option<ActorId>>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    device_id: Option<Option<DeviceId>>,
    observed_at: Timestamp,
    capture_reason: CaptureReason,
    metadata: Metadata,
}

impl StepObservation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ObservationId,
        step_id: StepId,
        space_id: Option<SpaceId>,
        operation_id: Option<OperationId>,
        worktree_id: Option<WorktreeId>,
        actor_id: Option<ActorId>,
        device_id: Option<DeviceId>,
        observed_at: Timestamp,
        capture_reason: CaptureReason,
        metadata: Metadata,
    ) -> Self {
        Self {
            id,
            step_id,
            space_id,
            operation_id,
            worktree_id,
            actor_id,
            device_id,
            observed_at,
            capture_reason,
            metadata,
        }
    }

    pub fn id(&self) -> &ObservationId {
        &self.id
    }
    pub fn step_id(&self) -> &StepId {
        &self.step_id
    }
    pub fn space_id(&self) -> Option<&SpaceId> {
        self.space_id.as_ref()
    }
    pub fn operation_id(&self) -> Option<&OperationId> {
        self.operation_id.as_ref()
    }
    pub fn worktree_id(&self) -> Option<&WorktreeId> {
        self.worktree_id.as_ref()
    }
    pub fn actor_id(&self) -> Option<&ActorId> {
        self.actor_id.as_ref()
    }
    pub fn device_id(&self) -> Option<&DeviceId> {
        self.device_id.as_ref()
    }
    pub fn observed_at(&self) -> &Timestamp {
        &self.observed_at
    }
    pub fn capture_reason(&self) -> CaptureReason {
        self.capture_reason
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

impl Serialize for StepObservation {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        StepObservationRef {
            schema_version: "layrs.step-observation.v1",
            observation_id: &self.id,
            step_id: &self.step_id,
            space_id: &self.space_id,
            operation_id: &self.operation_id,
            worktree_id: &self.worktree_id,
            actor_id: &self.actor_id,
            device_id: &self.device_id,
            observed_at: &self.observed_at,
            capture_reason: self.capture_reason,
            metadata: &self.metadata,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StepObservation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = StepObservationWire::deserialize(deserializer)?;
        if wire.schema_version != "layrs.step-observation.v1" {
            return Err(serde::de::Error::custom(
                "unsupported StepObservation schemaVersion",
            ));
        }
        Ok(Self::new(
            wire.observation_id,
            wire.step_id,
            wire.space_id,
            wire.operation_id,
            wire.worktree_id
                .ok_or_else(|| serde::de::Error::missing_field("worktreeId"))?,
            wire.actor_id
                .ok_or_else(|| serde::de::Error::missing_field("actorId"))?,
            wire.device_id
                .ok_or_else(|| serde::de::Error::missing_field("deviceId"))?,
            wire.observed_at,
            wire.capture_reason,
            wire.metadata,
        ))
    }
}
