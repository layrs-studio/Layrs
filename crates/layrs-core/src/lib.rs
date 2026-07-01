use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::str::FromStr;

pub type Metadata = BTreeMap<String, String>;

const MAX_ID_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    Empty,
    TooLong { max: usize, actual: usize },
    InvalidChar { ch: char, index: usize },
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "id cannot be empty"),
            Self::TooLong { max, actual } => {
                write!(f, "id is too long: {actual} bytes, maximum is {max}")
            }
            Self::InvalidChar { ch, index } => {
                write!(f, "id contains invalid character `{ch}` at byte {index}")
            }
        }
    }
}

impl Error for IdError {}

fn validate_id(value: &str) -> std::result::Result<(), IdError> {
    if value.is_empty() {
        return Err(IdError::Empty);
    }

    if value.len() > MAX_ID_LEN {
        return Err(IdError::TooLong {
            max: MAX_ID_LEN,
            actual: value.len(),
        });
    }

    for (index, ch) in value.char_indices() {
        let valid = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':');
        if !valid {
            return Err(IdError::InvalidChar { ch, index });
        }
    }

    Ok(())
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> std::result::Result<Self, IdError> {
                let value = value.into();
                validate_id(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;

            fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
                Self::new(value)
            }
        }
    };
}

id_type!(WorkspaceId);
id_type!(TeamId);
id_type!(SpaceId);
id_type!(LayerId);
id_type!(ViewId);
id_type!(ArtifactId);
id_type!(ArtifactVersionId);
id_type!(StepId);
id_type!(FlowId);
id_type!(WeaveId);
id_type!(LensId);
id_type!(ProofId);
id_type!(GateId);
id_type!(PolicyId);
id_type!(TimelineEventId);
id_type!(ChunkId);
id_type!(ActorId);
id_type!(GraphEdgeId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp {
    unix_millis: u64,
}

impl Timestamp {
    pub const fn from_unix_millis(unix_millis: u64) -> Self {
        Self { unix_millis }
    }

    pub const fn as_unix_millis(self) -> u64 {
        self.unix_millis
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn new(value: impl Into<String>) -> std::result::Result<Self, IdError> {
        let value = value.into();
        validate_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ObjectKind {
    Workspace,
    Team,
    Space,
    Layer,
    View,
    Artifact,
    ArtifactVersion,
    Step,
    Flow,
    Weave,
    Lens,
    Proof,
    Gate,
    Policy,
    TimelineEvent,
    Chunk,
    Actor,
    GraphEdge,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectRef {
    pub kind: ObjectKind,
    pub id: String,
}

impl ObjectRef {
    pub fn new(kind: ObjectKind, id: impl Into<String>) -> std::result::Result<Self, IdError> {
        let id = id.into();
        validate_id(&id)?;
        Ok(Self { kind, id })
    }

    pub fn workspace(id: WorkspaceId) -> Self {
        Self::from_validated(ObjectKind::Workspace, id.into_string())
    }

    pub fn team(id: TeamId) -> Self {
        Self::from_validated(ObjectKind::Team, id.into_string())
    }

    pub fn space(id: SpaceId) -> Self {
        Self::from_validated(ObjectKind::Space, id.into_string())
    }

    pub fn layer(id: LayerId) -> Self {
        Self::from_validated(ObjectKind::Layer, id.into_string())
    }

    pub fn artifact(id: ArtifactId) -> Self {
        Self::from_validated(ObjectKind::Artifact, id.into_string())
    }

    pub fn step(id: StepId) -> Self {
        Self::from_validated(ObjectKind::Step, id.into_string())
    }

    pub fn proof(id: ProofId) -> Self {
        Self::from_validated(ObjectKind::Proof, id.into_string())
    }

    pub fn gate(id: GateId) -> Self {
        Self::from_validated(ObjectKind::Gate, id.into_string())
    }

    pub fn policy(id: PolicyId) -> Self {
        Self::from_validated(ObjectKind::Policy, id.into_string())
    }

    fn from_validated(kind: ObjectKind, id: String) -> Self {
        Self { kind, id }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub description: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub responsibilities: Vec<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: SpaceId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub description: Option<String>,
    pub default_layer_id: Option<LayerId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    Base,
    Exploration,
    Proposal,
    AutomatedResult,
    ReleaseCandidate,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerStatus {
    Draft,
    Active,
    Archived,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub space_id: SpaceId,
    pub name: String,
    pub kind: LayerKind,
    pub status: LayerStatus,
    pub base_layer_ids: Vec<LayerId>,
    pub created_by: Option<ActorId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    File,
    Note,
    Report,
    Image,
    Proof,
    StepOutput,
    DecisionCapture,
    Dataset,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub space_id: SpaceId,
    pub layer_id: Option<LayerId>,
    pub kind: ArtifactKind,
    pub title: String,
    pub path: Option<String>,
    pub latest_version_id: Option<ArtifactVersionId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactVersion {
    pub id: ArtifactVersionId,
    pub artifact_id: ArtifactId,
    pub ordinal: u64,
    pub content_hash: ContentHash,
    pub chunk_ids: Vec<ChunkId>,
    pub created_by: Option<ActorId>,
    pub created_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub id: ChunkId,
    pub content_hash: ContentHash,
    pub byte_len: u64,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepTrigger {
    Manual,
    OnArtifactChanged,
    OnLayerCreated,
    OnGateRequested,
    OnFlowStarted,
    OnSchedule { schedule: String },
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepAction {
    TransformState,
    VerifyState,
    ProduceArtifact,
    EvaluateGate,
    AttachProof,
    Notify,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
    RequiresInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub id: StepId,
    pub space_id: SpaceId,
    pub flow_id: Option<FlowId>,
    pub name: String,
    pub trigger: StepTrigger,
    pub action: StepAction,
    pub status: StepStatus,
    pub input_refs: Vec<ObjectRef>,
    pub output_refs: Vec<ObjectRef>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowStatus {
    Pending,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flow {
    pub id: FlowId,
    pub space_id: SpaceId,
    pub name: String,
    pub trigger: StepTrigger,
    pub step_ids: Vec<StepId>,
    pub status: FlowStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeaveStatus {
    Open,
    Resolved,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Weave {
    pub id: WeaveId,
    pub space_id: SpaceId,
    pub title: String,
    pub status: WeaveStatus,
    pub related_refs: Vec<ObjectRef>,
    pub created_by: Option<ActorId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LensFocus {
    Security,
    Product,
    TechnicalDebt,
    Dependencies,
    Ownership,
    Quality,
    Delivery,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lens {
    pub id: LensId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub focus: LensFocus,
    pub filter: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub id: ViewId,
    pub space_id: SpaceId,
    pub name: String,
    pub target: ObjectRef,
    pub lens_id: Option<LensId>,
    pub query: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofKind {
    Test,
    Review,
    Decision,
    Audit,
    External,
    Durability,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofStatus {
    Pending,
    Verified,
    Rejected,
    Expired,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    pub id: ProofId,
    pub space_id: SpaceId,
    pub target: ObjectRef,
    pub kind: ProofKind,
    pub status: ProofStatus,
    pub summary: String,
    pub artifact_id: Option<ArtifactId>,
    pub created_by: Option<ActorId>,
    pub created_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateStatus {
    Open,
    Passing,
    Blocked,
    NeedsProof,
    Waived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateDecision {
    Allowed,
    Blocked,
    NeedsProof,
    Waived,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub id: GateId,
    pub space_id: SpaceId,
    pub name: String,
    pub target: ObjectRef,
    pub status: GateStatus,
    pub required_policy_ids: Vec<PolicyId>,
    pub required_proof_ids: Vec<ProofId>,
    pub decision: Option<GateDecision>,
    pub updated_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RelationKind {
    Contains,
    MemberOf,
    Owns,
    DependsOn,
    Produces,
    Verifies,
    Requires,
    Blocks,
    Approves,
    Supersedes,
    DerivesFrom,
    Triggers,
    Annotates,
    References,
    Governs,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyScope {
    Global,
    Workspace(WorkspaceId),
    Team(TeamId),
    Space(SpaceId),
    Layer(LayerId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicySubject {
    Everyone,
    Actor(ActorId),
    Team(TeamId),
    Step(StepId),
    Automation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyEffect {
    Allow,
    Deny,
    RequireProof,
    RequireGate,
    Notify,
    TriggerStep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyAction {
    ReadWorkspace,
    ManageWorkspace,
    CreateSpace,
    ReadSpace,
    WriteSpace,
    CreateLayer,
    PromoteLayer,
    CreateArtifact,
    AttachProof,
    ApproveGate,
    RunStep,
    ManagePolicy,
    Custom(String),
}

impl PolicyAction {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ReadWorkspace => "read_workspace",
            Self::ManageWorkspace => "manage_workspace",
            Self::CreateSpace => "create_space",
            Self::ReadSpace => "read_space",
            Self::WriteSpace => "write_space",
            Self::CreateLayer => "create_layer",
            Self::PromoteLayer => "promote_layer",
            Self::CreateArtifact => "create_artifact",
            Self::AttachProof => "attach_proof",
            Self::ApproveGate => "approve_gate",
            Self::RunStep => "run_step",
            Self::ManagePolicy => "manage_policy",
            Self::Custom(value) => value,
        }
    }
}

impl fmt::Display for PolicyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Custom(value) => write!(f, "custom:{value}"),
            _ => f.write_str(self.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsePolicyActionError {
    value: String,
}

impl fmt::Display for ParsePolicyActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown policy action `{}`", self.value)
    }
}

impl Error for ParsePolicyActionError {}

impl FromStr for PolicyAction {
    type Err = ParsePolicyActionError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let action = match value {
            "read_workspace" => Self::ReadWorkspace,
            "manage_workspace" => Self::ManageWorkspace,
            "create_space" => Self::CreateSpace,
            "read_space" => Self::ReadSpace,
            "write_space" => Self::WriteSpace,
            "create_layer" => Self::CreateLayer,
            "promote_layer" => Self::PromoteLayer,
            "create_artifact" => Self::CreateArtifact,
            "attach_proof" => Self::AttachProof,
            "approve_gate" => Self::ApproveGate,
            "run_step" => Self::RunStep,
            "manage_policy" => Self::ManagePolicy,
            custom if custom.starts_with("custom:") && custom.len() > "custom:".len() => {
                Self::Custom(custom["custom:".len()..].to_owned())
            }
            _ => {
                return Err(ParsePolicyActionError {
                    value: value.to_owned(),
                });
            }
        };

        Ok(action)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyCondition {
    pub target_kind: Option<ObjectKind>,
    pub relation: Option<RelationKind>,
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub id: PolicyId,
    pub scope: PolicyScope,
    pub name: String,
    pub subjects: Vec<PolicySubject>,
    pub action: PolicyAction,
    pub effect: PolicyEffect,
    pub conditions: Vec<PolicyCondition>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineAction {
    Created,
    Updated,
    Deleted,
    Linked,
    Unlinked,
    StepStarted,
    StepFinished,
    GateEvaluated,
    PolicyEvaluated,
    ProofAttached,
    Commented,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub id: TimelineEventId,
    pub workspace_id: WorkspaceId,
    pub space_id: Option<SpaceId>,
    pub actor_id: Option<ActorId>,
    pub action: TimelineAction,
    pub target: ObjectRef,
    pub summary: String,
    pub occurred_at: Timestamp,
    pub metadata: Metadata,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_validate_stable_slugs() {
        let id = WorkspaceId::new("workspace:alpha-1").expect("valid id");

        assert_eq!(id.as_str(), "workspace:alpha-1");
        assert!(WorkspaceId::new("").is_err());
        assert!(WorkspaceId::new("workspace alpha").is_err());
    }

    #[test]
    fn policy_action_roundtrips_through_display() {
        let actions = [
            PolicyAction::ReadWorkspace,
            PolicyAction::PromoteLayer,
            PolicyAction::ApproveGate,
            PolicyAction::Custom("run_internal_check".to_owned()),
        ];

        for action in actions {
            let encoded = action.to_string();
            let decoded = encoded.parse::<PolicyAction>().expect("roundtrip");
            assert_eq!(decoded, action);
        }
    }
}
