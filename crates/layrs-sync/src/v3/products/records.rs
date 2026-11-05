//! Closed typed records carried by Product envelopes.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ImmutableSubject {
    #[serde(rename = "step")]
    Step {
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "tree")]
    Tree {
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "conflict")]
    Conflict {
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "evidence")]
    Evidence {
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "product")]
    Product {
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "productRoot")]
    ProductRoot { product: ProductRoot },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProofRecord {
    pub schema_version: ProofSchemaVersion,
    pub proof_id: ProofId,
    pub space_id: SpaceId,
    pub subject: ImmutableSubject,
    pub recipe_object_id: ObjectId,
    pub evidence_object_ids: Vec<ObjectId>,
    pub policy_version_object_ids: Vec<ObjectId>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofSchemaVersion {
    #[serde(rename = "layrs.proof.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateDefinitionRecord {
    pub schema_version: GateDefinitionSchemaVersion,
    pub gate_definition_id: GateDefinitionId,
    pub space_id: SpaceId,
    pub name: String,
    pub recipe_object_ids: Vec<ObjectId>,
    pub policy_version_object_ids: Vec<ObjectId>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateDefinitionSchemaVersion {
    #[serde(rename = "layrs.gate-definition.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateEvaluationRecord {
    pub schema_version: GateEvaluationSchemaVersion,
    pub gate_evaluation_id: GateEvaluationId,
    pub space_id: SpaceId,
    pub gate_definition_object_id: ObjectId,
    pub subject: ImmutableSubject,
    pub proof_object_ids: Vec<ObjectId>,
    pub policy_version_object_ids: Vec<ObjectId>,
    pub outcome: GateOutcome,
    pub evaluated_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateEvaluationSchemaVersion {
    #[serde(rename = "layrs.gate-evaluation.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRecord {
    pub schema_version: PolicySchemaVersion,
    pub policy_id: PolicyId,
    pub version_id: PolicyVersionId,
    pub space_id: SpaceId,
    pub rules_object_id: ObjectId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicySchemaVersion {
    #[serde(rename = "layrs.policy.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionRecord {
    pub schema_version: DecisionSchemaVersion,
    pub decision_id: DecisionId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub discarded_head_step_ids: Vec<StepId>,
    pub chosen_head_step_ids: Vec<StepId>,
    pub reason: String,
    pub decided_by: ActorId,
    pub decided_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionSchemaVersion {
    #[serde(rename = "layrs.ref-decision.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256Id(String);
impl Sha256Id {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let valid = value.len() == 71
            && value.starts_with("sha256:")
            && value.as_bytes()[7..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
        valid
            .then_some(Self(value))
            .ok_or_else(|| "invalid SHA-256 ID".into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for Sha256Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for Sha256Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlgorithmVersion {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum MergeBase {
    #[serde(rename = "step")]
    Step {
        #[serde(rename = "baseId")]
        base_id: ObjectId,
        #[serde(rename = "stepId")]
        step_id: StepId,
    },
    #[serde(rename = "virtual")]
    Virtual {
        #[serde(rename = "baseId")]
        base_id: ObjectId,
        #[serde(rename = "sourceStepIds")]
        source_step_ids: Vec<StepId>,
        algorithm: AlgorithmVersion,
        result: VirtualBaseResult,
    },
}

impl MergeBase {
    pub fn base_id(&self) -> &ObjectId {
        match self {
            Self::Step { base_id, .. } | Self::Virtual { base_id, .. } => base_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VirtualBaseResultKind {
    Tree,
    Object,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VirtualBaseResult {
    pub kind: VirtualBaseResultKind,
    pub object_id: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensPin {
    pub lens_id: String,
    pub version: String,
    pub digest: Sha256Id,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermRoleKind {
    Base,
    Source,
    Target,
    Existing,
    Incoming,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TermRole {
    pub term_id: ObjectId,
    pub role: TermRoleKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceLimits {
    pub max_memory_bytes: u64,
    pub max_execution_ms: u64,
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_fuel: Nullable<u64>,
    pub max_blocks: u64,
    pub max_segments: u64,
    pub max_paths: u64,
    pub max_recursion_depth: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AmbientCapabilities {
    pub network: bool,
    pub wall_clock: bool,
    pub randomness: bool,
    pub locale: bool,
    pub host_filesystem: bool,
    pub environment: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostPolicy {
    pub id: String,
    pub version: String,
    pub digest: Sha256Id,
    pub ambient_capabilities: AmbientCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMethod {
    SelectTerm,
    CombineTerms,
    Manual,
    Rename,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionFailureAction {
    StructuralFallback,
    Reject,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombineOrder {
    TermIdAscii,
    LensDefined,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionToggle {
    Allowed,
    Forbidden,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AbsentSemantics {
    #[serde(rename = "distinct_from_empty_file")]
    DistinctFromEmptyFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolutionPolicy {
    pub id: String,
    pub version: String,
    pub digest: Sha256Id,
    pub allowed_methods: Vec<ResolutionMethod>,
    pub automatic_side_selection: bool,
    pub absent_semantics: AbsentSemantics,
    pub combine_order: CombineOrder,
    pub manual_input: PermissionToggle,
    pub rename_resolution: PermissionToggle,
    pub on_unsupported_lens: ResolutionFailureAction,
    pub on_unavailable_lens: ResolutionFailureAction,
    pub on_timeout: ResolutionFailureAction,
    pub on_quota_exceeded: ResolutionFailureAction,
    pub on_trap: ResolutionFailureAction,
    pub on_invalid_output: ResolutionFailureAction,
    pub on_invalid_signature: ResolutionFailureAction,
    pub on_digest_mismatch: ResolutionFailureAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicalizationId {
    #[serde(rename = "rfc8785")]
    Rfc8785,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicalizationVersion {
    #[serde(rename = "RFC8785")]
    Rfc8785,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalizationVersionRecord {
    pub id: CanonicalizationId,
    pub version: CanonicalizationVersion,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentHashId {
    #[serde(rename = "blake3")]
    Blake3,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentHashVersionRecord {
    pub id: ContentHashId,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeterminismProfile {
    pub profile_id: ObjectId,
    pub canonicalization: CanonicalizationVersionRecord,
    pub content_hash: ContentHashVersionRecord,
    pub merge_algorithm: AlgorithmVersion,
    pub base_algorithm: AlgorithmVersion,
    pub host_policy: HostPolicy,
    pub resolution_policy: ResolutionPolicy,
    pub limits: ResourceLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensExecutionResult {
    AutoResolved,
    Conflicted,
    StructuralFallback,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensExecution {
    pub execution_id: ObjectId,
    pub path: layrs_core::scm::RepoPath,
    pub pin: LensPin,
    pub implementation_digest: Sha256Id,
    pub determinism_profile_id: ObjectId,
    pub term_roles: Vec<TermRole>,
    pub result: LensExecutionResult,
    pub conflict_id: Nullable<ConflictId>,
}

/// Fully typed SCM V1 Weave receipt used by the V3 product union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeaveReceiptRecord {
    pub schema_version: WeaveReceiptSchemaVersion,
    pub weave_id: ObjectId,
    pub space_id: SpaceId,
    pub source_heads: Vec<StepId>,
    pub target_heads_before: Vec<StepId>,
    pub merge_bases: Vec<MergeBase>,
    pub result_step_id: StepId,
    pub outcome: WeaveOutcome,
    pub conflict_ids: Vec<ConflictId>,
    pub lens_executions: Vec<LensExecution>,
    pub determinism_profile: DeterminismProfile,
    pub operation_id: OperationId,
    pub gate_snapshot_id: Nullable<ObjectId>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeaveReceiptSchemaVersion {
    #[serde(rename = "layrs.scm.weave_receipt.v1")]
    V1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaveOutcome {
    Clean,
    Conflicted,
}
