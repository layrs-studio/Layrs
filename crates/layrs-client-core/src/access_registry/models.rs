#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerAccessView {
    pub layer_id: String,
    pub workspace_id: String,
    pub space_id: String,
    pub display_name: String,
    pub access: LayerAccessKind,
    pub can_open: bool,
    pub local_path: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum LayerAccessKind {
    Open,
    Redacted,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRegistryResult {
    pub root: String,
    pub pointer_path: String,
    pub layers: Vec<LayerAccessView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableSpaceView {
    pub space_id: String,
    pub workspace_id: String,
    pub name: String,
    pub current_layer_id: Option<String>,
    pub layers: Vec<LayerAccessView>,
    pub local_spaces: Vec<LocalSpaceSummary>,
    pub freshness: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSpaceSummary {
    pub local_space_id: String,
    pub space_id: String,
    pub workspace_id: String,
    pub server_space_id: Option<String>,
    pub state: String,
    pub name: String,
    pub root_path: String,
    pub active_layer_id: Option<String>,
    pub layers: Vec<LocalLayerSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalLayerSummary {
    pub layer_id: String,
    pub display_name: String,
    pub parent_layer_id: Option<String>,
    #[serde(default = "default_layer_lineage_status")]
    pub lineage_status: String,
    pub access: LayerAccessKind,
    pub can_open: bool,
    pub path: String,
    pub sync_status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalSpaceResult {
    pub local_space: LocalSpaceSummary,
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitLocalSpaceResult {
    pub local_space: LocalSpaceSummary,
    pub created: bool,
    pub initial_step_id: Option<String>,
    pub scanned_files: usize,
    pub pending_publish_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendDraftLocalSpaceResult {
    pub local_space: LocalSpaceSummary,
    pub workspace_id: String,
    pub server_space_id: String,
    pub layer_mappings: Vec<LayerIdMapping>,
    pub published_layers: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetLocalSpaceResult {
    pub local_space_id: String,
    pub root_path: String,
    pub archived_layrs_path: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerIdMapping {
    pub local_layer_id: String,
    pub server_layer_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSwitchResult {
    pub local_space: LocalSpaceSummary,
    pub previous_layer_id: String,
    pub active_layer_id: String,
    pub saved_step_id: Option<String>,
    pub changed_files: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteLayerResult {
    pub local_space: LocalSpaceSummary,
    pub deleted_layer_id: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSettingsResult {
    pub local_space: LocalSpaceSummary,
    pub layer_id: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_steps_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeScan {
    pub root_path: String,
    pub active_layer_id: String,
    pub changed: bool,
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub diffs: Vec<LensDiffEntry>,
    pub steps: Vec<LocalStepSummary>,
    pub layer_activities: Vec<LayerStepActivity>,
    pub pending_publish_count: usize,
    pub files: Vec<FileSnapshotEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensDiffEntry {
    pub path: String,
    pub state: String,
    pub lens_id: String,
    pub title: String,
    pub diff: DiffModel,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStepSummary {
    pub step_id: String,
    pub layer_id: String,
    pub captured_at: u64,
    pub timeline_position: Option<u64>,
    pub origin_layer_id: Option<String>,
    pub origin_layer_name: Option<String>,
    pub origin_step_id: Option<String>,
    pub step_kind: Option<String>,
    pub changed_files: usize,
    pub diff_stats: DiffStats,
    pub diffs: Vec<LensDiffEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerStepActivity {
    pub layer_id: String,
    pub latest_step_at: u64,
    pub step_count: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffStats {
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffModel {
    pub kind: String,
    pub summary: String,
    pub hunks: Vec<DiffHunk>,
    pub fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug)]
struct TextDiffSource {
    text: Option<String>,
    readable: bool,
    truncated: bool,
}

impl TextDiffSource {
    fn absent() -> Self {
        Self {
            text: None,
            readable: true,
            truncated: false,
        }
    }
}

#[derive(Clone, Debug)]
struct DiffWindowOptions<'a> {
    source: &'a str,
    layer_id: &'a str,
    step_id: Option<&'a str>,
    start: usize,
    limit: usize,
    preview: bool,
}

impl<'a> DiffWindowOptions<'a> {
    fn scan(source: &'a str, layer_id: &'a str, step_id: Option<&'a str>) -> Self {
        Self {
            source,
            layer_id,
            step_id,
            start: 0,
            limit: TEXT_DIFF_DEFAULT_WINDOW_LIMIT,
            preview: true,
        }
    }

    fn requested(
        source: &'a str,
        layer_id: &'a str,
        step_id: Option<&'a str>,
        start: usize,
        limit: usize,
    ) -> Self {
        Self {
            source,
            layer_id,
            step_id,
            start,
            limit: clamp_diff_window_limit(limit),
            preview: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOperationResult {
    pub local_space: LocalSpaceSummary,
    pub status: String,
    pub message: String,
    pub sync_state_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactLocalSpaceResult {
    pub local_space: LocalSpaceSummary,
    pub packed_chunks: usize,
    pub loose_chunks_removed: usize,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub pack_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveLocalStepResult {
    pub local_space: LocalSpaceSummary,
    pub status: String,
    pub message: String,
    pub step_id: Option<String>,
    pub changed_files: usize,
    pub diff_stats: DiffStats,
    pub pending_publish_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaveOperationResult {
    pub local_space: LocalSpaceSummary,
    pub session: WeaveSessionSummary,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaveSessionSummary {
    pub weave_id: String,
    pub source_layer_id: String,
    pub target_layer_id: String,
    pub status: String,
    pub pre_weave_target_tree_id: Option<String>,
    pub pre_weave_target_step_id: Option<String>,
    pub planned_steps: Vec<String>,
    pub applied_steps: Vec<String>,
    pub conflicts: Vec<WeaveConflictSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaveConflictSummary {
    pub conflict_id: String,
    pub path: String,
    pub lens_id: String,
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub methods: Vec<String>,
    pub resolution: Option<String>,
    pub blocks: Vec<WeaveConflictBlockSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaveConflictBlockSummary {
    pub block_id: String,
    pub status: String,
    pub base: String,
    pub existing: String,
    pub incoming: String,
    pub ours: String,
    pub theirs: String,
    #[serde(default)]
    pub methods: Vec<String>,
    pub resolution: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingPublishFile {
    schema: String,
    step_id: String,
    layer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_tree_id: Option<String>,
    #[serde(default)]
    changed_paths: Vec<String>,
    created_at_unix: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingLayerDeletionsFile {
    schema: String,
    #[serde(default)]
    deleted_layers: Vec<PendingLayerDeletion>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingLayerDeletion {
    layer_id: String,
    display_name: String,
    deleted_at_unix: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSpaceFromLocalRequest {
    name: String,
    description: Option<String>,
    local_space_id: String,
    layers: Vec<LocalLayerImportRequest>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalLayerImportRequest {
    local_layer_id: String,
    name: String,
    parent_local_layer_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSpaceFromLocalResponse {
    space: CreatedServerSpace,
    #[serde(default)]
    layer_mappings: Vec<LayerIdMapping>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedServerSpace {
    id: String,
    workspace_id: String,
    #[serde(default)]
    current_layer_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateServerLayerRequest {
    name: String,
    parent_layer_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedServerLayer {
    id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishLayerRequest {
    layer_id: String,
    protocol: String,
    policy_epoch: u64,
    idempotency_key: String,
    source_client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_tree_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changed_paths: Vec<String>,
    store_objects: PublishStoreObjectsRequest,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifacts: Vec<PublishArtifactRequest>,
    deleted_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    steps: Vec<PublishStepRequest>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishStepRequest {
    step_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_layer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_tree_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changed_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeline_position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_layer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_layer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_kind: Option<String>,
    captured_at_unix: u64,
}

impl PublishStepRequest {
    fn from_step(step: &StepFile) -> Self {
        Self {
            step_id: step.step_id.clone(),
            parent_step_id: step.parent_step_id.clone(),
            base_layer_id: step.base_layer_id.clone(),
            base_tree_id: step.base_tree_id.clone(),
            root_tree_id: step.root_tree_id.clone(),
            changed_paths: step.changed_paths.clone(),
            timeline_position: step.timeline_position,
            origin_layer_id: step.origin_layer_id.clone(),
            origin_layer_name: step.origin_layer_name.clone(),
            origin_step_id: step.origin_step_id.clone(),
            step_kind: step.step_kind.clone(),
            captured_at_unix: step.captured_at_unix,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishStoreObjectsRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    chunks: Vec<PublishChunkObjectRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    file_objects: Vec<PublishFileObjectRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tree_objects: Vec<PublishTreeObjectRequest>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishChunkObjectRequest {
    chunk_id: String,
    digest: String,
    size: u64,
    raw_size: u64,
    stored_size: u64,
    compression: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareChunkUploadRequest {
    chunks: Vec<PrepareChunkUploadItem>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareChunkUploadItem {
    chunk_id: String,
    size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareChunkUploadResponse {
    items: Vec<PreparedChunkUploadItem>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreparedChunkUploadItem {
    chunk_id: String,
    upload_required: bool,
    upload_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishFileObjectRequest {
    file_object_id: String,
    size: u64,
    chunks: Vec<PublishChunkRefRequest>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishChunkRefRequest {
    chunk_id: String,
    size: u64,
    raw_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    stored_size: Option<u64>,
    compression: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishTreeObjectRequest {
    tree_id: String,
    entries: Vec<PublishTreeEntryRequest>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishTreeEntryRequest {
    path: String,
    file_object_id: String,
    size: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishArtifactRequest {
    path: String,
    kind: String,
    media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiveLocalSpaceRequest {
    layer_id: String,
    limit: i64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceiveLocalSpaceResponse {
    workspace_id: String,
    space_id: String,
    layer_id: String,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    root_tree_id: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    layers: Vec<ReceivedLayer>,
    #[serde(default, alias = "accessRegistry")]
    access_registries: Vec<Value>,
    #[serde(default, alias = "storeObjects")]
    content_objects: Option<ReceivedContentObjects>,
    #[serde(default)]
    timeline: Vec<Value>,
    #[serde(default)]
    steps: Vec<ReceivedStep>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedContentObjects {
    #[serde(default)]
    chunks: Vec<ReceivedChunkObject>,
    #[serde(default)]
    file_objects: Vec<ReceivedFileObject>,
    #[serde(default)]
    tree_objects: Vec<ReceivedTreeObject>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedChunkObject {
    chunk_id: String,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default, alias = "downloadUrl")]
    download_url: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes: Option<u64>,
    #[serde(default, alias = "rawSize")]
    raw_size: Option<u64>,
    #[serde(default, alias = "storedSize")]
    stored_size: Option<u64>,
    #[serde(default)]
    compression: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedFileObject {
    file_object_id: String,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    chunks: Vec<ReceivedChunkRef>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedChunkRef {
    chunk_id: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes: Option<u64>,
    #[serde(default, alias = "rawSize")]
    raw_size: Option<u64>,
    #[serde(default, alias = "storedSize")]
    stored_size: Option<u64>,
    #[serde(default)]
    compression: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedTreeObject {
    tree_id: String,
    #[serde(default)]
    layer_id: Option<String>,
    #[serde(default, alias = "files")]
    entries: Vec<ReceivedTreeEntry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedTreeEntry {
    path: String,
    #[serde(default, alias = "hash")]
    file_object_id: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedLayer {
    id: String,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    space_id: Option<String>,
    name: String,
    #[serde(default, alias = "parentId")]
    parent_layer_id: Option<String>,
    #[serde(default)]
    access: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedStep {
    step_id: String,
    layer_id: String,
    #[serde(default)]
    parent_step_id: Option<String>,
    #[serde(default)]
    base_layer_id: Option<String>,
    #[serde(default)]
    base_tree_id: Option<String>,
    #[serde(default)]
    root_tree_id: Option<String>,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    timeline_position: Option<u64>,
    #[serde(default)]
    origin_layer_id: Option<String>,
    #[serde(default)]
    origin_layer_name: Option<String>,
    #[serde(default)]
    origin_step_id: Option<String>,
    #[serde(default)]
    step_kind: Option<String>,
    #[serde(default)]
    captured_at_unix: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalSpaceFile {
    schema: String,
    #[serde(default = "default_linked_state")]
    state: String,
    local_space_id: String,
    space_id: String,
    #[serde(default)]
    server_space_id: Option<String>,
    #[serde(default)]
    workspace_id: String,
    name: String,
    root_path: String,
    created_at_unix: u64,
    updated_at_unix: u64,
    layers: Vec<LocalLayerMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalLayerMetadata {
    layer_id: String,
    display_name: String,
    #[serde(default)]
    parent_layer_id: Option<String>,
    #[serde(default = "default_layer_lineage_status")]
    lineage_status: String,
    access: LayerAccessKind,
    can_open: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActiveLayerFile {
    schema: String,
    layer_id: String,
    updated_at_unix: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalAccessPointer {
    schema: String,
    local_space_id: String,
    active_layer_id: Option<String>,
    layers: Vec<LayerAccessView>,
    redacted_reserved_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LayerAccessFile {
    schema: String,
    local_space_id: String,
    workspace_id: String,
    space_id: String,
    layer_id: String,
    display_name: String,
    access: LayerAccessKind,
    can_open: bool,
    reason: Option<String>,
    policy_epoch: u64,
    generated_at_unix: u64,
    rules: Vec<LayerAccessRuleFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LayerAccessRuleFile {
    id: String,
    path: String,
    mode: String,
    visibility: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkingStateFile {
    schema: String,
    layer_id: String,
    captured_at_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<FileSnapshotEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StepFile {
    schema: String,
    step_id: String,
    layer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_layer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeline_position: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin_layer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin_layer_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    step_kind: Option<String>,
    captured_at_unix: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<FileSnapshotEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WeaveSessionFile {
    schema: String,
    weave_id: String,
    source_layer_id: String,
    target_layer_id: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pre_weave_target_tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pre_weave_target_step_id: Option<String>,
    #[serde(default)]
    planned_steps: Vec<String>,
    #[serde(default)]
    applied_steps: Vec<String>,
    #[serde(default)]
    conflicts: Vec<WeaveConflictFile>,
    created_at_unix: u64,
    updated_at_unix: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WeaveConflictFile {
    conflict_id: String,
    path: String,
    lens_id: String,
    status: String,
    message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    blocks: Vec<WeaveConflictBlockFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    segments: Vec<WeaveConflictSegmentFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WeaveConflictBlockFile {
    block_id: String,
    status: String,
    base: String,
    ours: String,
    theirs: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WeaveConflictSegmentFile {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSnapshotEntry {
    pub path: String,
    pub object: String,
    pub hash: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeObjectFile {
    schema: String,
    tree_id: String,
    files: Vec<FileSnapshotEntry>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanCacheFile {
    schema: String,
    entries: Vec<ScanCacheEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanCacheEntry {
    path: String,
    size: u64,
    modified_at: String,
    snapshot: FileSnapshotEntry,
}

#[derive(Clone, Debug)]
struct LocalSpaceHandle {
    root: PathBuf,
    layrs_dir: PathBuf,
    meta: LocalSpaceFile,
    active: ActiveLayerFile,
}
