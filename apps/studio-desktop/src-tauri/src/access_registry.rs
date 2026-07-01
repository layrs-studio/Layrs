use crate::desktop_state::{
    load_cached_bootstrap, save_cached_bootstrap, validate_desktop_bootstrap, workspace_root,
    BootstrapData, DesktopConfig, DesktopSettings, LayerSummary, LocalSpaceConfigEntry,
    SpaceSummary,
};
use crate::http_client::{delete_json, get_bytes, get_json, post_json, put_bytes_json};
use crate::secret_store::{OsSecretStore, SecretStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const LAYRS_DIR: &str = ".layrs";
const LOCAL_SPACE_SCHEMA: &str = "layrs.local_space.v1";
const ACTIVE_LAYER_SCHEMA: &str = "layrs.active_layer.v1";
const ACCESS_POINTER_SCHEMA: &str = "layrs.local_space_access.v1";
const LAYER_ACCESS_SCHEMA: &str = "layrs.layer_access.v1";
const WORKING_STATE_SCHEMA: &str = "layrs.layer_working_state.v2";
const STEP_SCHEMA: &str = "layrs.layer_step.v2";
const TREE_OBJECT_SCHEMA: &str = "layrs.tree_object.v1";
const FILE_OBJECT_SCHEMA: &str = "layrs.file_object.v1";
const SCAN_CACHE_SCHEMA: &str = "layrs.scan_cache.v1";
const SYNC_STATE_SCHEMA: &str = "layrs.layer_sync_state.v1";
const SYNC_PROTOCOL_V2: &str = "layrs.sync.v2";
const LOCAL_SPACE_STATE_LINKED: &str = "linked";
const LOCAL_SPACE_STATE_DRAFT: &str = "draft";
const CHUNK_SIZE: usize = 1024 * 1024;
const TEXT_DIFF_DEFAULT_WINDOW_LIMIT: usize = 400;
const TEXT_DIFF_MAX_WINDOW_LIMIT: usize = 2_000;

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
pub struct WorkingTreeScan {
    pub root_path: String,
    pub active_layer_id: String,
    pub changed: bool,
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub diffs: Vec<LensDiffEntry>,
    pub steps: Vec<LocalStepSummary>,
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
    pub changed_files: usize,
    pub diff_stats: DiffStats,
    pub diffs: Vec<LensDiffEntry>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<PublishStepRequest>,
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
    captured_at_unix: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<FileSnapshotEntry>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileObjectFile {
    schema: String,
    hash: String,
    size: u64,
    chunks: Vec<FileChunkRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileChunkRef {
    chunk_id: String,
    size: u64,
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

pub fn list_available_spaces() -> Result<Vec<AvailableSpaceView>, String> {
    let (bootstrap, freshness, message) = load_live_bootstrap_or_cache()?;
    let local_spaces = list_local_spaces()?;
    let mut result = Vec::with_capacity(bootstrap.spaces.len());

    for space in bootstrap.spaces {
        let layers: Vec<LayerSummary> = bootstrap
            .layers
            .iter()
            .filter(|layer| layer.space_id == space.id)
            .cloned()
            .collect();
        let matching_local_spaces = local_spaces
            .iter()
            .filter(|local| local.space_id == space.id)
            .cloned()
            .collect();

        result.push(AvailableSpaceView {
            space_id: space.id.clone(),
            workspace_id: space.workspace_id.clone(),
            name: space.name.clone(),
            current_layer_id: space.current_layer_id.clone(),
            layers: access_views(&layers, None)?,
            local_spaces: matching_local_spaces,
            freshness: freshness.clone(),
            message: message.clone(),
        });
    }

    Ok(result)
}

pub fn list_local_spaces() -> Result<Vec<LocalSpaceSummary>, String> {
    let mut config = DesktopConfig::load_or_create()?;
    let mut spaces = Vec::new();
    let mut retained_entries = Vec::new();

    for entry in config.local_spaces.iter().cloned() {
        if let Ok(handle) = open_local_space_at(PathBuf::from(&entry.root_path)) {
            spaces.push(summary_from_handle(&handle));
            retained_entries.push(entry);
        }
    }

    if retained_entries.len() != config.local_spaces.len() {
        config.local_spaces = retained_entries;
        config.save()?;
    }

    Ok(spaces)
}

pub fn create_local_space(
    space_id: String,
    target_folder: String,
    initial_layer_id: Option<String>,
) -> Result<CreateLocalSpaceResult, String> {
    create_local_space_internal(space_id, target_folder, initial_layer_id, true)
}

fn create_local_space_internal(
    space_id: String,
    target_folder: String,
    initial_layer_id: Option<String>,
    receive_from_server: bool,
) -> Result<CreateLocalSpaceResult, String> {
    let config = DesktopConfig::load_or_create()?;
    let bootstrap = load_cached_bootstrap()?.unwrap_or_default();
    let root = absolute_path(&PathBuf::from(target_folder.trim()))?;
    if root.exists() && !root.is_dir() {
        return Err(format!(
            "Layrs Desktop cannot create a Local Space in a file: {}",
            root.display()
        ));
    }

    fs::create_dir_all(&root).map_err(|error| {
        format!(
            "Layrs Desktop could not create Local Space folder {}: {error}",
            root.display()
        )
    })?;

    let (space, mut layers) = bootstrap_space(&bootstrap, &space_id, initial_layer_id.as_deref());
    let active_layer_id = initial_layer_id
        .filter(|value| !value.trim().is_empty())
        .or_else(|| space.current_layer_id.clone())
        .or_else(|| layers.first().map(|layer| layer.id.clone()))
        .unwrap_or_else(|| "main".to_string());

    if !layers.iter().any(|layer| layer.id == active_layer_id) {
        layers.push(ad_hoc_layer(&space, &active_layer_id, "Main"));
    }

    let layer_views = access_views(&layers, Some(&root))?;
    let active_access = layer_views
        .iter()
        .find(|layer| layer.layer_id == active_layer_id)
        .ok_or_else(|| "Layrs Desktop could not resolve the initial Layer.".to_string())?;
    if !active_access.can_open {
        return Err(format!(
            "Layrs Desktop cannot open initial Layer {}: {}",
            active_layer_id,
            active_access
                .reason
                .clone()
                .unwrap_or_else(|| "Layer access is blocked.".to_string())
        ));
    }

    let layrs_dir = root.join(LAYRS_DIR);
    let created = !layrs_dir.join("local-space.json").exists();
    create_local_space_directories(&layrs_dir)?;

    let now = unix_now();
    let local_space_id = format!("local-{}-{}", safe_id_fragment(&space.id), now);
    let existing_meta = read_json::<LocalSpaceFile>(&layrs_dir.join("local-space.json")).ok();
    let local_space_id = existing_meta
        .as_ref()
        .map(|meta| meta.local_space_id.clone())
        .unwrap_or(local_space_id);
    let created_at_unix = existing_meta
        .as_ref()
        .map(|meta| meta.created_at_unix)
        .unwrap_or(now);

    for view in &layer_views {
        scaffold_layer(&layrs_dir, &local_space_id, view)?;
    }

    let metadata_layers = layer_views
        .iter()
        .map(|view| LocalLayerMetadata {
            layer_id: view.layer_id.clone(),
            display_name: view.display_name.clone(),
            parent_layer_id: layers
                .iter()
                .find(|layer| layer.id == view.layer_id)
                .and_then(|layer| layer.parent_layer_id.clone()),
            access: view.access.clone(),
            can_open: view.can_open,
        })
        .collect::<Vec<_>>();
    let meta = LocalSpaceFile {
        schema: LOCAL_SPACE_SCHEMA.to_string(),
        state: LOCAL_SPACE_STATE_LINKED.to_string(),
        local_space_id: local_space_id.clone(),
        space_id: space.id.clone(),
        server_space_id: Some(space.id.clone()),
        workspace_id: space.workspace_id.clone(),
        name: space.name.clone(),
        root_path: root.display().to_string(),
        created_at_unix,
        updated_at_unix: now,
        layers: metadata_layers,
    };
    write_json(&layrs_dir.join("local-space.json"), &meta)?;

    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: active_layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(
        &layrs_dir,
        &local_space_id,
        Some(&active_layer_id),
        &layer_views,
    )?;

    remember_local_space(&meta, Some(active_layer_id.clone()))?;
    let mut handle = LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
    };

    if receive_from_server {
        if let Err(error) = receive_linked_space_state(&mut handle, &config, true) {
            if created {
                let _ = fs::remove_dir_all(&handle.layrs_dir);
            }
            let _ = remove_local_space_config_entry(&handle.meta.local_space_id, &handle.root);
            return Err(format!(
                "Layrs Desktop could not copy Space {} from Studio: {error}",
                space.name
            ));
        }
    } else {
        let state = capture_working_state(&handle.root, &active_layer_id, true)?;
        write_layer_state(&handle.layrs_dir, &active_layer_id, &state)?;
    }

    Ok(CreateLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        created,
    })
}

pub fn create_draft_local_space(
    name: String,
    target_folder: String,
) -> Result<CreateLocalSpaceResult, String> {
    let display_name = if name.trim().is_empty() {
        "Untitled Space".to_string()
    } else {
        name.trim().to_string()
    };
    let root = absolute_path(&PathBuf::from(target_folder.trim()))?;
    if root.exists() && !root.is_dir() {
        return Err(format!(
            "Layrs Desktop cannot create a Draft Local Space in a file: {}",
            root.display()
        ));
    }

    fs::create_dir_all(&root).map_err(|error| {
        format!(
            "Layrs Desktop could not create Draft Local Space folder {}: {error}",
            root.display()
        )
    })?;

    let layrs_dir = root.join(LAYRS_DIR);
    if layrs_dir.join("local-space.json").exists() {
        return Err(format!(
            "Layrs Desktop found an existing Local Space at {}.",
            root.display()
        ));
    }

    create_local_space_directories(&layrs_dir)?;
    let now = unix_now();
    let local_space_id = format!("local_space_{}", now);
    let layer_id = "local_layer_main".to_string();
    let layer_view = LayerAccessView {
        layer_id: layer_id.clone(),
        workspace_id: String::new(),
        space_id: local_space_id.clone(),
        display_name: "Main".to_string(),
        access: LayerAccessKind::Open,
        can_open: true,
        local_path: Some(layer_dir(&layrs_dir, &layer_id).display().to_string()),
        reason: None,
    };
    scaffold_layer(&layrs_dir, &local_space_id, &layer_view)?;

    let meta = LocalSpaceFile {
        schema: LOCAL_SPACE_SCHEMA.to_string(),
        state: LOCAL_SPACE_STATE_DRAFT.to_string(),
        local_space_id: local_space_id.clone(),
        space_id: local_space_id.clone(),
        server_space_id: None,
        workspace_id: String::new(),
        name: display_name,
        root_path: root.display().to_string(),
        created_at_unix: now,
        updated_at_unix: now,
        layers: vec![LocalLayerMetadata {
            layer_id: layer_id.clone(),
            display_name: "Main".to_string(),
            parent_layer_id: None,
            access: LayerAccessKind::Open,
            can_open: true,
        }],
    };
    write_json(&layrs_dir.join("local-space.json"), &meta)?;
    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(&layrs_dir, &local_space_id, Some(&layer_id), &[layer_view])?;

    let state = capture_working_state(&root, &layer_id, true)?;
    write_layer_state(&layrs_dir, &layer_id, &state)?;
    remember_local_space(&meta, Some(layer_id.clone()))?;

    let handle = LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
    };

    Ok(CreateLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        created: true,
    })
}

pub fn send_draft_local_space(
    local_space: String,
    workspace_id: String,
) -> Result<SendDraftLocalSpaceResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    if handle.meta.state != LOCAL_SPACE_STATE_DRAFT {
        return Err("Only Draft Local Spaces can be sent to Studio.".to_string());
    }
    let workspace_id = workspace_id.trim().to_string();
    if workspace_id.is_empty() {
        return Err("Choose a Workspace before sending this Draft Local Space.".to_string());
    }

    let config = DesktopConfig::load_or_create()?;
    let token = desktop_token(&config)?;
    let active_layer_id = handle.active.layer_id.clone();
    let current_state = capture_working_state(&handle.root, &active_layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &active_layer_id).ok();
    if changed_file_count(previous_state.as_ref(), &current_state) > 0 && config.auto_local_steps {
        write_step(&handle.layrs_dir, &active_layer_id, &current_state)?;
    }
    write_layer_state(&handle.layrs_dir, &active_layer_id, &current_state)?;

    let request = CreateSpaceFromLocalRequest {
        name: handle.meta.name.clone(),
        description: None,
        local_space_id: handle.meta.local_space_id.clone(),
        layers: handle
            .meta
            .layers
            .iter()
            .map(|layer| LocalLayerImportRequest {
                local_layer_id: layer.layer_id.clone(),
                name: layer.display_name.clone(),
                parent_local_layer_id: layer.parent_layer_id.clone(),
            })
            .collect(),
    };
    let import_path = format!(
        "/v1/workspaces/{}/spaces/from-local",
        url_path_segment(&workspace_id)
    );
    let created: CreateSpaceFromLocalResponse = post_json(
        &config.server_endpoint,
        &import_path,
        Some(&token),
        &request,
    )?;
    if created.layer_mappings.is_empty() {
        return Err(
            "Layrs server did not return Layer mappings for the imported Draft.".to_string(),
        );
    }

    let mut published_layers = 0usize;
    for mapping in &created.layer_mappings {
        let state = read_layer_state(&handle.layrs_dir, &mapping.local_layer_id)
            .or_else(|_| read_layer_index(&handle.layrs_dir, &mapping.local_layer_id))?;
        let publish_paths = state
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<BTreeSet<_>>();
        if !publish_paths.is_empty() {
            let publish_path = format!(
                "/v1/workspaces/{}/spaces/{}/sync/publish",
                url_path_segment(&workspace_id),
                url_path_segment(&created.space.id)
            );
            let body = build_publish_v2_request(
                &handle,
                &config,
                &mapping.server_layer_id,
                None,
                &state,
                publish_paths.iter().cloned().collect(),
                Vec::new(),
                None,
            )?;
            upload_publish_chunks(
                &handle,
                &config.server_endpoint,
                Some(&token),
                &workspace_id,
                &created.space.id,
                &body.store_objects,
            )?;
            let _: Value = post_json(&config.server_endpoint, &publish_path, Some(&token), &body)?;
        }
        published_layers += 1;
    }

    apply_server_mapping_to_draft(&mut handle, &created)?;
    if let Ok(bootstrap) = get_json::<BootstrapData>(
        &config.server_endpoint,
        "/v1/desktop/bootstrap",
        Some(&token),
    )
    .and_then(|bootstrap| validate_desktop_bootstrap(bootstrap, "/v1/desktop/bootstrap"))
    {
        let _ = save_cached_bootstrap(&bootstrap);
    }

    Ok(SendDraftLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        workspace_id,
        server_space_id: created.space.id,
        layer_mappings: created.layer_mappings,
        published_layers,
    })
}

pub fn open_local_space(local_space_id_or_path: String) -> Result<LocalSpaceSummary, String> {
    let handle = open_local_space_handle(&local_space_id_or_path)?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))?;
    Ok(summary_from_handle(&handle))
}

pub fn forget_local_space(local_space: String) -> Result<ForgetLocalSpaceResult, String> {
    let mut config = DesktopConfig::load_or_create()?;
    let selector = local_space.trim();
    let opened = open_local_space_handle(selector).ok();
    let entry = opened.as_ref().map(|handle| LocalSpaceConfigEntry {
        local_space_id: handle.meta.local_space_id.clone(),
        space_id: handle.meta.space_id.clone(),
        root_path: handle.root.display().to_string(),
        active_layer_id: Some(handle.active.layer_id.clone()),
        updated_at_unix: unix_now(),
    })
    .or_else(|| find_local_space_config_entry(&config, selector))
    .ok_or_else(|| {
        format!(
            "Layrs Desktop could not find a Local Space entry for {selector}. It may already be disconnected."
        )
    })?;

    let local_space_id = entry.local_space_id.clone();
    let root = opened
        .as_ref()
        .map(|handle| handle.root.clone())
        .unwrap_or_else(|| PathBuf::from(&entry.root_path));
    let root = absolute_path(&root).unwrap_or(root);
    let archived_layrs_path = archive_layrs_dir_at(&root, &root.join(LAYRS_DIR))?;
    let root_path = root.display().to_string();
    let root_key = path_compare_key(&root);
    config.local_spaces.retain(|entry| {
        entry.local_space_id != local_space_id
            && path_compare_key(&PathBuf::from(&entry.root_path)) != root_key
    });
    config.save()?;

    let message = if archived_layrs_path.is_some() {
        "Local Space was forgotten on this machine. Project files were kept, and the old .layrs metadata was archived."
    } else {
        "Local Space was disconnected on this machine. Project files were kept, and local .layrs metadata was already absent."
    };

    Ok(ForgetLocalSpaceResult {
        local_space_id,
        root_path,
        archived_layrs_path: archived_layrs_path.map(|path| path.display().to_string()),
        message: message.to_string(),
    })
}

pub fn switch_layer(
    local_space: String,
    target_layer_id: String,
) -> Result<LayerSwitchResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let previous_layer_id = handle.active.layer_id.clone();
    let target_layer = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == target_layer_id)
        .cloned()
        .ok_or_else(|| format!("Layrs Desktop does not know Layer {target_layer_id}."))?;

    if !target_layer.can_open {
        return Err(format!(
            "Layrs Desktop cannot switch to Layer {} because its access is {:?}.",
            target_layer.layer_id, target_layer.access
        ));
    }

    let config = DesktopConfig::load_or_create()?;
    let current_state = capture_working_state(&handle.root, &previous_layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &previous_layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step_id = if changed_files > 0 && config.auto_local_steps {
        Some(write_step(
            &handle.layrs_dir,
            &previous_layer_id,
            &current_state,
        )?)
    } else {
        None
    };
    write_working_state(&handle.layrs_dir, &previous_layer_id, &current_state)?;

    if previous_layer_id != target_layer_id {
        let target_state = read_layer_state(&handle.layrs_dir, &target_layer_id)
            .or_else(|_| read_layer_index(&handle.layrs_dir, &target_layer_id))?;
        materialize_state(&handle.root, &target_state)?;

        handle.active = ActiveLayerFile {
            schema: ACTIVE_LAYER_SCHEMA.to_string(),
            layer_id: target_layer_id.clone(),
            updated_at_unix: unix_now(),
        };
        write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
        write_access_pointer_from_meta(&handle)?;
        remember_local_space(&handle.meta, Some(target_layer_id.clone()))?;
    }

    Ok(LayerSwitchResult {
        local_space: summary_from_handle(&handle),
        previous_layer_id,
        active_layer_id: target_layer_id,
        saved_step_id,
        changed_files,
    })
}

pub fn create_layer_from_current(
    local_space: String,
    name: String,
) -> Result<LayerSwitchResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let previous_layer_id = handle.active.layer_id.clone();
    let display_name = if name.trim().is_empty() {
        "New Layer".to_string()
    } else {
        name.trim().to_string()
    };

    let current_state = capture_working_state(&handle.root, &previous_layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &previous_layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step_id = if changed_files > 0 {
        Some(write_step(
            &handle.layrs_dir,
            &previous_layer_id,
            &current_state,
        )?)
    } else {
        None
    };
    let layer_id = if is_server_linked_space(&handle) {
        create_linked_server_layer(&handle, &display_name, Some(&previous_layer_id))?
    } else {
        unique_layer_id(&handle, &display_name)
    };

    let layer = LocalLayerMetadata {
        layer_id: layer_id.clone(),
        display_name,
        parent_layer_id: Some(previous_layer_id.clone()),
        access: LayerAccessKind::Open,
        can_open: true,
    };
    handle.meta.layers.push(layer.clone());
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;

    let view = LayerAccessView {
        layer_id: layer.layer_id.clone(),
        workspace_id: handle.meta.workspace_id.clone(),
        space_id: handle.meta.space_id.clone(),
        display_name: layer.display_name.clone(),
        access: LayerAccessKind::Open,
        can_open: true,
        local_path: Some(
            layer_dir(&handle.layrs_dir, &layer.layer_id)
                .display()
                .to_string(),
        ),
        reason: None,
    };
    scaffold_layer(&handle.layrs_dir, &handle.meta.local_space_id, &view)?;
    inherit_layer_access_file(&handle, &previous_layer_id, &layer_id, &layer.display_name)?;
    let mut new_state = current_state.clone();
    new_state.layer_id = layer_id.clone();
    write_layer_state(&handle.layrs_dir, &layer_id, &new_state)?;
    if is_server_linked_space(&handle) {
        write_linked_layer_sync_state(&handle, &layer_id, Some(&previous_layer_id))?;
    }

    handle.active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: unix_now(),
    };
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(&handle)?;
    remember_local_space(&handle.meta, Some(layer_id.clone()))?;

    Ok(LayerSwitchResult {
        local_space: summary_from_handle(&handle),
        previous_layer_id,
        active_layer_id: layer_id,
        saved_step_id,
        changed_files,
    })
}

pub fn delete_layer(local_space: String, layer_id: String) -> Result<DeleteLayerResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let target_layer_id = layer_id.trim().to_string();
    if target_layer_id.is_empty() {
        return Err("Choose a Layer to delete.".to_string());
    }
    if handle.active.layer_id == target_layer_id {
        return Err("Switch to another Layer before deleting this one.".to_string());
    }
    if handle.meta.layers.len() <= 1 {
        return Err("A Local Space must keep at least one Layer.".to_string());
    }
    if handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.parent_layer_id.as_deref() == Some(target_layer_id.as_str()))
    {
        return Err("Delete child Layers before deleting their parent Layer.".to_string());
    }

    let target_index = handle
        .meta
        .layers
        .iter()
        .position(|layer| layer.layer_id == target_layer_id)
        .ok_or_else(|| format!("Layrs Desktop does not know Layer {target_layer_id}."))?;

    if is_server_linked_space(&handle) && is_probably_server_layer_id(&target_layer_id) {
        delete_linked_server_layer(&handle, &target_layer_id)?;
    }

    let layer_path = layer_dir(&handle.layrs_dir, &target_layer_id);
    if layer_path.exists() {
        fs::remove_dir_all(&layer_path).map_err(|error| {
            format!(
                "Layrs Desktop could not remove local Layer directory {}: {error}",
                layer_path.display()
            )
        })?;
    }

    handle.meta.layers.remove(target_index);
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_access_pointer_from_meta(&handle)?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))?;

    Ok(DeleteLayerResult {
        local_space: summary_from_handle(&handle),
        deleted_layer_id: target_layer_id,
        message: "Layer deleted.".to_string(),
    })
}

fn delete_linked_server_layer(handle: &LocalSpaceHandle, layer_id: &str) -> Result<(), String> {
    ensure_linked_space_ready(handle)?;
    let config = DesktopConfig::load_or_create()?;
    let token = desktop_token(&config)?;
    let delete_path = format!(
        "/v1/workspaces/{}/spaces/{}/layers/{}",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id),
        url_path_segment(layer_id)
    );
    let _: Value = delete_json(&config.server_endpoint, &delete_path, Some(&token))?;
    Ok(())
}

fn create_linked_server_layer(
    handle: &LocalSpaceHandle,
    display_name: &str,
    parent_layer_id: Option<&str>,
) -> Result<String, String> {
    ensure_linked_space_ready(handle)?;
    if let Some(parent_layer_id) = parent_layer_id {
        if !is_probably_server_layer_id(parent_layer_id) {
            return Err(unlinked_layer_message(parent_layer_id));
        }
    }

    let config = DesktopConfig::load_or_create()?;
    let token = desktop_token(&config)?;
    let create_path = format!(
        "/v1/workspaces/{}/spaces/{}/layers",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let request = CreateServerLayerRequest {
        name: display_name.to_string(),
        parent_layer_id: parent_layer_id.map(ToString::to_string),
    };
    let created: CreatedServerLayer = post_json(
        &config.server_endpoint,
        &create_path,
        Some(&token),
        &request,
    )
    .map_err(|error| {
        parent_layer_id
            .map(|parent| link_layer_error_message(parent, error.clone()))
            .unwrap_or(error)
    })?;
    let server_layer_id = created.id.trim().to_string();
    if server_layer_id.is_empty() {
        return Err("Layrs server created a Layer but did not return its id.".to_string());
    }
    if handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.layer_id == server_layer_id)
    {
        return Err(format!(
            "Layrs server returned Layer {server_layer_id}, but this Local Space already has a Layer with that id."
        ));
    }

    Ok(server_layer_id)
}

fn link_active_local_layer_for_sync(
    handle: &mut LocalSpaceHandle,
    operation: &str,
) -> Result<String, String> {
    let old_layer_id = handle.active.layer_id.clone();
    let display_name = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == old_layer_id)
        .map(|layer| layer.display_name.clone())
        .unwrap_or_else(|| "Local Layer".to_string());
    let parent_layer_id = handle
        .meta
        .layers
        .iter()
        .map(|layer| layer.layer_id.as_str())
        .find(|layer_id| *layer_id != old_layer_id && is_probably_server_layer_id(layer_id))
        .map(str::to_string);
    let current_state = capture_working_state(&handle.root, &old_layer_id, true)?;
    let server_layer_id =
        create_linked_server_layer(handle, &display_name, parent_layer_id.as_deref())?;

    let old_dir = layer_dir(&handle.layrs_dir, &old_layer_id);
    let new_dir = layer_dir(&handle.layrs_dir, &server_layer_id);
    if old_dir != new_dir && old_dir.exists() && !new_dir.exists() {
        fs::rename(&old_dir, &new_dir).map_err(|error| {
            format!(
                "Layrs Desktop could not link local Layer {old_layer_id} to server Layer {server_layer_id}: {error}"
            )
        })?;
    } else if !new_dir.exists() {
        let view = LayerAccessView {
            layer_id: server_layer_id.clone(),
            workspace_id: handle.meta.workspace_id.clone(),
            space_id: handle.meta.space_id.clone(),
            display_name: display_name.clone(),
            access: LayerAccessKind::Open,
            can_open: true,
            local_path: Some(new_dir.display().to_string()),
            reason: None,
        };
        scaffold_layer(&handle.layrs_dir, &handle.meta.local_space_id, &view)?;
    }

    rewrite_layer_files_after_mapping(
        &handle.layrs_dir,
        &old_layer_id,
        &server_layer_id,
        &handle.meta.workspace_id,
        &handle.meta.space_id,
    )?;
    let mut linked_state = current_state;
    linked_state.layer_id = server_layer_id.clone();
    write_layer_state(&handle.layrs_dir, &server_layer_id, &linked_state)?;

    for layer in &mut handle.meta.layers {
        if layer.layer_id == old_layer_id {
            layer.layer_id = server_layer_id.clone();
            layer.display_name = display_name.clone();
            layer.parent_layer_id = parent_layer_id.clone();
        }
    }
    handle.meta.updated_at_unix = unix_now();
    handle.active.layer_id = server_layer_id.clone();
    handle.active.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(handle)?;
    let _ = write_sync_state(handle, operation, "linked", 0, None)?;
    remember_local_space(&handle.meta, Some(server_layer_id.clone()))?;
    Ok(server_layer_id)
}

fn inherit_layer_access_file(
    handle: &LocalSpaceHandle,
    parent_layer_id: &str,
    layer_id: &str,
    display_name: &str,
) -> Result<(), String> {
    let parent_access_path = layer_dir(&handle.layrs_dir, parent_layer_id).join("access.json");
    if !parent_access_path.exists() {
        return Ok(());
    }

    let mut access = read_json::<LayerAccessFile>(&parent_access_path)?;
    access.workspace_id = handle.meta.workspace_id.clone();
    access.space_id = handle.meta.space_id.clone();
    access.layer_id = layer_id.to_string();
    access.display_name = display_name.to_string();
    access.generated_at_unix = unix_now();
    write_json(
        &layer_dir(&handle.layrs_dir, layer_id).join("access.json"),
        &access,
    )
}

pub fn scan_working_tree(local_space: String) -> Result<WorkingTreeScan, String> {
    let handle = open_local_space_handle(&local_space)?;
    let active_layer_id = handle.active.layer_id.clone();
    let current = capture_working_state(&handle.root, &active_layer_id, true)?;
    let previous = read_layer_state(&handle.layrs_dir, &active_layer_id).ok();
    let (added, modified, deleted) = diff_state(previous.as_ref(), &current);
    let diffs = lens_diff_entries(
        &handle,
        "workingTree",
        &active_layer_id,
        None,
        previous.as_ref(),
        &current,
        &added,
        &modified,
        &deleted,
    );
    let steps = local_step_summaries(&handle, &active_layer_id)?;

    Ok(WorkingTreeScan {
        root_path: handle.root.display().to_string(),
        active_layer_id,
        changed: !(added.is_empty() && modified.is_empty() && deleted.is_empty()),
        added,
        modified,
        deleted,
        diffs,
        steps,
        files: current.files,
    })
}

pub fn load_diff_window(
    local_space: String,
    path: String,
    source: Option<String>,
    start: usize,
    limit: usize,
) -> Result<LensDiffEntry, String> {
    let handle = open_local_space_handle(&local_space)?;
    let source = source.unwrap_or_else(|| "workingTree".to_string());

    if source == "workingTree" {
        let layer_id = handle.active.layer_id.clone();
        let current = capture_working_state(&handle.root, &layer_id, true)?;
        let previous = read_layer_state(&handle.layrs_dir, &layer_id).ok();
        return diff_window_for_path(
            &handle,
            "workingTree",
            &layer_id,
            None,
            previous.as_ref(),
            &current,
            &path,
            start,
            limit,
        );
    }

    if let Some(step_id) = source
        .strip_prefix("localStep:")
        .or_else(|| source.strip_prefix("step:"))
    {
        return load_step_diff_window(&handle, &path, step_id, start, limit);
    }

    Err(format!(
        "Layrs Desktop cannot load diff window source {source}. Supported sources: workingTree, localStep:<stepId>."
    ))
}

fn receive_linked_space_state(
    handle: &mut LocalSpaceHandle,
    config: &DesktopConfig,
    materialize_active: bool,
) -> Result<PathBuf, String> {
    ensure_linked_space_ready(handle)?;
    let layer_id = handle.active.layer_id.clone();
    if !is_probably_server_layer_id(&layer_id) {
        return Err(unlinked_layer_message(&layer_id));
    }

    let receive_path = format!(
        "/v1/workspaces/{}/spaces/{}/sync/receive",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let request = ReceiveLocalSpaceRequest {
        layer_id: layer_id.clone(),
        limit: 200,
    };
    let token = desktop_token(config)?;
    let response: ReceiveLocalSpaceResponse = post_json(
        &config.server_endpoint,
        &receive_path,
        Some(&token),
        &request,
    )?;
    apply_receive_response(
        handle,
        response,
        materialize_active,
        Some(config.server_endpoint.as_str()),
        Some(token.as_str()),
    )
}

pub fn receive_local_space(local_space: String) -> Result<SyncOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    if handle.meta.state == LOCAL_SPACE_STATE_DRAFT {
        return Err("Draft Local Spaces must be sent to Studio before receiving.".to_string());
    }
    ensure_linked_space_ready(&handle)?;

    let config = DesktopConfig::load_or_create()?;
    let layer_id = handle.active.layer_id.clone();
    if !is_probably_server_layer_id(&layer_id) {
        let path = write_sync_state(&handle, "receive", "blocked-unlinked-layer", 0, None)?;
        return Err(format!(
            "{} Sync state: {}",
            unlinked_layer_message(&layer_id),
            path.display()
        ));
    }

    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step = if changed_files > 0 && config.auto_local_steps {
        Some(write_step(&handle.layrs_dir, &layer_id, &current_state)?)
    } else {
        None
    };
    write_working_state(&handle.layrs_dir, &layer_id, &current_state)?;

    let receive_path = format!(
        "/v1/workspaces/{}/spaces/{}/sync/receive",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let request = ReceiveLocalSpaceRequest {
        layer_id: layer_id.clone(),
        limit: 200,
    };
    let token = desktop_token(&config)?;
    let response: ReceiveLocalSpaceResponse = post_json(
        &config.server_endpoint,
        &receive_path,
        Some(&token),
        &request,
    )?;
    let sync_path = apply_receive_response(
        &mut handle,
        response,
        true,
        Some(config.server_endpoint.as_str()),
        Some(token.as_str()),
    )?;
    let message = if let Some(step_id) = saved_step {
        format!("Received Studio state. Local changes were preserved in Step {step_id}.")
    } else {
        "Received Studio state.".to_string()
    };

    Ok(SyncOperationResult {
        local_space: summary_from_handle(&handle),
        status: "received".to_string(),
        message,
        sync_state_path: sync_path.display().to_string(),
    })
}

pub fn publish_local_space(local_space: String) -> Result<SyncOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    if handle.meta.state == LOCAL_SPACE_STATE_DRAFT {
        return Err("Draft Local Spaces must be sent to Studio before publishing.".to_string());
    }
    ensure_linked_space_ready(&handle)?;

    let config = DesktopConfig::load_or_create()?;
    let mut layer_id = handle.active.layer_id.clone();
    if !handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.layer_id == layer_id)
    {
        return Err(format!(
            "Layrs Desktop does not know active Layer {layer_id}. Switch to a known Layer before publishing."
        ));
    }
    let mut force_full_publish = false;
    if !is_probably_server_layer_id(&layer_id) {
        let linked_layer_id = link_active_local_layer_for_sync(&mut handle, "publish")?;
        layer_id = linked_layer_id;
        force_full_publish = true;
    }
    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let previous_state = if force_full_publish {
        None
    } else {
        read_layer_state(&handle.layrs_dir, &layer_id).ok()
    };
    let (added, modified, deleted) = diff_state(previous_state.as_ref(), &current_state);
    let changed_files = added.len() + modified.len() + deleted.len();
    if changed_files == 0 {
        let path = write_sync_state(&handle, "publish", "clean", 0, None)?;
        return Ok(SyncOperationResult {
            local_space: summary_from_handle(&handle),
            status: "clean".to_string(),
            message: "No local changes to publish.".to_string(),
            sync_state_path: path.display().to_string(),
        });
    }
    let publish_step = if config.auto_local_steps {
        let step_id = write_step(&handle.layrs_dir, &layer_id, &current_state)?;
        Some(read_step_file(&handle.layrs_dir, &layer_id, &step_id)?)
    } else {
        None
    };

    let publish_paths = added
        .iter()
        .chain(modified.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    let publish_path = format!(
        "/v1/workspaces/{}/spaces/{}/sync/publish",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let body = build_publish_v2_request(
        &handle,
        &config,
        &layer_id,
        previous_state
            .as_ref()
            .and_then(|state| state.root_tree_id.clone()),
        &current_state,
        publish_paths
            .iter()
            .chain(deleted.iter())
            .cloned()
            .collect(),
        deleted.clone(),
        publish_step.as_ref(),
    )?;
    let token = desktop_token(&config)?;
    upload_publish_chunks(
        &handle,
        &config.server_endpoint,
        Some(&token),
        &handle.meta.workspace_id,
        &handle.meta.space_id,
        &body.store_objects,
    )?;
    let response: Value =
        match post_json(&config.server_endpoint, &publish_path, Some(&token), &body) {
            Ok(response) => response,
            Err(error) if is_layer_not_found_error(&error) => {
                let path = write_sync_state(
                    &handle,
                    "publish",
                    "blocked-unlinked-layer",
                    changed_files,
                    None,
                )?;
                return Err(format!(
                    "{} Sync state: {}",
                    unlinked_layer_message(&layer_id),
                    path.display()
                ));
            }
            Err(error) => return Err(error),
        };
    write_layer_state(&handle.layrs_dir, &layer_id, &current_state)?;
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    remember_local_space(&handle.meta, Some(layer_id.clone()))?;

    let server_cursor = response
        .get("serverCursor")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let sync_path = write_sync_state(
        &handle,
        "publish",
        "published",
        changed_files,
        server_cursor,
    )?;

    Ok(SyncOperationResult {
        local_space: summary_from_handle(&handle),
        status: "published".to_string(),
        message: format!("Published {changed_files} local change(s) to Studio."),
        sync_state_path: sync_path.display().to_string(),
    })
}

pub fn load_desktop_settings() -> Result<DesktopSettings, String> {
    Ok(DesktopConfig::load_or_create()?.settings())
}

pub fn save_desktop_settings(settings: DesktopSettings) -> Result<DesktopSettings, String> {
    if !settings.server_endpoint.trim().starts_with("http://") {
        return Err(
            "Layrs Desktop currently accepts only http:// server endpoints for local development."
                .to_string(),
        );
    }

    let mut config = DesktopConfig::load_or_create()?;
    config.apply_settings(settings);
    config.save()?;
    Ok(config.settings())
}

fn load_live_bootstrap_or_cache() -> Result<(BootstrapData, String, Option<String>), String> {
    let config = DesktopConfig::load_or_create()?;
    match desktop_token(&config).and_then(|token| {
        let bootstrap = get_json::<BootstrapData>(
            &config.server_endpoint,
            "/v1/desktop/bootstrap",
            Some(&token),
        )?;
        validate_desktop_bootstrap(bootstrap, "/v1/desktop/bootstrap")
    }) {
        Ok(bootstrap) => {
            save_cached_bootstrap(&bootstrap)?;
            Ok((bootstrap, "fresh".to_string(), None))
        }
        Err(error) => {
            if let Some(bootstrap) = load_cached_bootstrap()? {
                Ok((
                    bootstrap,
                    "stale".to_string(),
                    Some(format!("Showing cached Spaces: {error}")),
                ))
            } else {
                Ok((BootstrapData::default(), "offline".to_string(), Some(error)))
            }
        }
    }
}

fn desktop_token(config: &DesktopConfig) -> Result<String, String> {
    #[cfg(test)]
    if let Ok(token) = env::var("LAYRS_DESKTOP_TEST_TOKEN") {
        return Ok(token);
    }

    let store = OsSecretStore::new();
    store
        .get_token(&config.device_id)
        .map_err(|error| format!("Layrs Desktop could not read OS secret store: {error}"))?
        .ok_or_else(|| "Layrs Desktop is not connected. Connect a device first.".to_string())
}

fn ensure_linked_space_ready(handle: &LocalSpaceHandle) -> Result<(), String> {
    if handle.meta.workspace_id.trim().is_empty() || handle.meta.space_id.trim().is_empty() {
        return Err("This Local Space is not linked to a server Space.".to_string());
    }
    Ok(())
}

fn is_server_linked_space(handle: &LocalSpaceHandle) -> bool {
    handle.meta.state == LOCAL_SPACE_STATE_LINKED
        && !handle.meta.workspace_id.trim().is_empty()
        && !handle.meta.space_id.trim().is_empty()
}

fn is_probably_server_layer_id(layer_id: &str) -> bool {
    layer_id.starts_with("layer_")
}

fn unlinked_layer_message(layer_id: &str) -> String {
    format!(
        "Layer {layer_id} exists only on this machine and is not linked to Studio yet. Create a new Layer from a linked server Layer, or refresh/recreate this Local Space before publishing."
    )
}

fn is_layer_not_found_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("http 404") && lower.contains("layer not found")
}

fn link_layer_error_message(parent_layer_id: &str, error: String) -> String {
    if is_layer_not_found_error(&error) {
        return format!(
            "{} Original server response: {error}",
            unlinked_layer_message(parent_layer_id)
        );
    }
    error
}

fn bootstrap_space(
    bootstrap: &BootstrapData,
    space_id: &str,
    initial_layer_id: Option<&str>,
) -> (SpaceSummary, Vec<LayerSummary>) {
    let space = bootstrap
        .spaces
        .iter()
        .find(|space| space.id == space_id)
        .cloned()
        .unwrap_or_else(|| SpaceSummary {
            id: space_id.to_string(),
            workspace_id: String::new(),
            name: space_id.to_string(),
            current_layer_id: initial_layer_id
                .filter(|value| !value.trim().is_empty())
                .map(ToString::to_string),
        });

    let layers = bootstrap
        .layers
        .iter()
        .filter(|layer| layer.space_id == space.id)
        .cloned()
        .collect::<Vec<_>>();

    (space, layers)
}

fn ad_hoc_layer(space: &SpaceSummary, layer_id: &str, name: &str) -> LayerSummary {
    LayerSummary {
        id: layer_id.to_string(),
        workspace_id: space.workspace_id.clone(),
        space_id: space.id.clone(),
        name: name.to_string(),
        kind: Some("local".to_string()),
        parent_layer_id: None,
        access: Some("open".to_string()),
    }
}

fn create_local_space_directories(layrs_dir: &Path) -> Result<(), String> {
    for path in [
        layrs_dir.to_path_buf(),
        layrs_dir.join("objects"),
        layrs_dir.join("objects").join("files"),
        layrs_dir.join("objects").join("trees"),
        layrs_dir.join("objects").join("chunks"),
        layrs_dir.join("layers"),
        layrs_dir.join("sync"),
        layrs_dir.join("tmp"),
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            format!(
                "Layrs Desktop could not create Local Space directory {}: {error}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn scaffold_layer(
    layrs_dir: &Path,
    local_space_id: &str,
    access: &LayerAccessView,
) -> Result<(), String> {
    let layer_path = access
        .local_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| layer_dir(layrs_dir, &access.layer_id));
    let access_path = layer_path.join("access.json");

    if access_path.exists() {
        let existing = read_json::<LayerAccessFile>(&access_path)?;
        if existing.access == LayerAccessKind::Redacted
            && access.access != LayerAccessKind::Redacted
        {
            return Err(format!(
                "Layrs Desktop refuses to replace reserved redacted Layer path {}.",
                layer_path.display()
            ));
        }
    }

    for path in [
        layer_path.clone(),
        layer_path.join("steps"),
        layer_path.join("pending-publish"),
        layer_path.join("lens-cache"),
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            format!(
                "Layrs Desktop could not create Layer directory {}: {error}",
                path.display()
            )
        })?;
    }

    let access_file = LayerAccessFile {
        schema: LAYER_ACCESS_SCHEMA.to_string(),
        local_space_id: local_space_id.to_string(),
        workspace_id: access.workspace_id.clone(),
        space_id: access.space_id.clone(),
        layer_id: access.layer_id.clone(),
        display_name: access.display_name.clone(),
        access: access.access.clone(),
        can_open: access.can_open,
        reason: access.reason.clone(),
        policy_epoch: 1,
        generated_at_unix: unix_now(),
        rules: Vec::new(),
    };
    write_json(&access_path, &access_file)?;

    let empty_tree_id = write_tree_object(layrs_dir, &[])?;
    let empty_state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: access.layer_id.clone(),
        captured_at_unix: unix_now(),
        root_tree_id: Some(empty_tree_id),
        files: Vec::new(),
    };

    for file_name in ["index.json", "working-state.json"] {
        let path = layer_path.join(file_name);
        if !path.exists() {
            write_json(&path, &empty_state)?;
        }
    }

    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": access.layer_id,
        "lastReceiveUnix": null,
        "lastPublishUnix": null,
        "pending": false
    });
    let sync_path = layer_path.join("sync-state.json");
    if !sync_path.exists() {
        write_json(&sync_path, &sync_state)?;
    }

    let timeline_path = layer_path.join("timeline-cache.json");
    if !timeline_path.exists() {
        write_json(
            &timeline_path,
            &serde_json::json!({ "schema": "layrs.timeline_cache.v1", "items": [] }),
        )?;
    }

    Ok(())
}

fn access_views(
    layers: &[LayerSummary],
    root: Option<&Path>,
) -> Result<Vec<LayerAccessView>, String> {
    let mut path_counts = BTreeMap::<String, usize>::new();
    for layer in layers {
        if let Some(path_key) = safe_layer_path_key(&layer.id) {
            *path_counts.entry(path_key).or_default() += 1;
        }
    }

    let mut emitted_paths = BTreeSet::<String>::new();
    let mut views = Vec::with_capacity(layers.len());
    for layer in layers {
        views.push(access_view(layer, &path_counts, &mut emitted_paths, root)?);
    }
    Ok(views)
}

fn access_view(
    layer: &LayerSummary,
    path_counts: &BTreeMap<String, usize>,
    emitted_paths: &mut BTreeSet<String>,
    root: Option<&Path>,
) -> Result<LayerAccessView, String> {
    let requested_access = layer.access.as_deref().unwrap_or("open");
    let redacted = matches!(
        requested_access,
        "redacted" | "denied" | "restricted" | "no-access"
    );
    let display_name = if redacted {
        "Restricted layer".to_string()
    } else {
        layer.name.clone()
    };

    let Some(path_key) = safe_layer_path_key(&layer.id) else {
        return Ok(LayerAccessView {
            layer_id: layer.id.clone(),
            workspace_id: layer.workspace_id.clone(),
            space_id: layer.space_id.clone(),
            display_name,
            access: LayerAccessKind::Blocked,
            can_open: false,
            local_path: None,
            reason: Some("Layer id cannot be represented safely as a local path.".to_string()),
        });
    };

    if path_counts.get(&path_key).copied().unwrap_or_default() > 1
        || !emitted_paths.insert(path_key.clone())
    {
        return Ok(LayerAccessView {
            layer_id: layer.id.clone(),
            workspace_id: layer.workspace_id.clone(),
            space_id: layer.space_id.clone(),
            display_name,
            access: LayerAccessKind::Blocked,
            can_open: false,
            local_path: None,
            reason: Some(
                "Layer local path collision detected; opening is blocked on this client."
                    .to_string(),
            ),
        });
    }

    let local_path = root.map(|root| {
        root.join(LAYRS_DIR)
            .join("layers")
            .join(&path_key)
            .display()
            .to_string()
    });

    if redacted {
        return Ok(LayerAccessView {
            layer_id: layer.id.clone(),
            workspace_id: layer.workspace_id.clone(),
            space_id: layer.space_id.clone(),
            display_name,
            access: LayerAccessKind::Redacted,
            can_open: false,
            local_path,
            reason: Some("Layer metadata is redacted and its local path is reserved.".to_string()),
        });
    }

    Ok(LayerAccessView {
        layer_id: layer.id.clone(),
        workspace_id: layer.workspace_id.clone(),
        space_id: layer.space_id.clone(),
        display_name,
        access: LayerAccessKind::Open,
        can_open: true,
        local_path,
        reason: None,
    })
}

fn write_access_pointer(
    layrs_dir: &Path,
    local_space_id: &str,
    active_layer_id: Option<&str>,
    layers: &[LayerAccessView],
) -> Result<(), String> {
    let pointer = LocalAccessPointer {
        schema: ACCESS_POINTER_SCHEMA.to_string(),
        local_space_id: local_space_id.to_string(),
        active_layer_id: active_layer_id.map(ToString::to_string),
        redacted_reserved_paths: layers
            .iter()
            .filter(|layer| layer.access == LayerAccessKind::Redacted)
            .filter_map(|layer| layer.local_path.clone())
            .collect(),
        layers: layers.to_vec(),
    };

    write_json(&layrs_dir.join("access.json"), &pointer)
}

fn write_access_pointer_from_meta(handle: &LocalSpaceHandle) -> Result<(), String> {
    let layers = handle
        .meta
        .layers
        .iter()
        .map(|layer| LayerAccessView {
            layer_id: layer.layer_id.clone(),
            workspace_id: handle.meta.workspace_id.clone(),
            space_id: handle.meta.space_id.clone(),
            display_name: layer.display_name.clone(),
            access: layer.access.clone(),
            can_open: layer.can_open,
            local_path: Some(
                layer_dir(&handle.layrs_dir, &layer.layer_id)
                    .display()
                    .to_string(),
            ),
            reason: if layer.access == LayerAccessKind::Redacted {
                Some("Layer metadata is redacted and its local path is reserved.".to_string())
            } else {
                None
            },
        })
        .collect::<Vec<_>>();

    write_access_pointer(
        &handle.layrs_dir,
        &handle.meta.local_space_id,
        Some(&handle.active.layer_id),
        &layers,
    )
}

fn open_local_space_handle(selector: &str) -> Result<LocalSpaceHandle, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("Layrs Desktop needs a Local Space id or path.".to_string());
    }

    let config = DesktopConfig::load_or_create()?;
    if let Some(entry) = config
        .local_spaces
        .iter()
        .find(|entry| entry.local_space_id == selector)
    {
        return open_local_space_at(PathBuf::from(&entry.root_path));
    }

    open_local_space_at(PathBuf::from(selector))
}

fn open_local_space_at(path: PathBuf) -> Result<LocalSpaceHandle, String> {
    let path = absolute_path(&path)?;
    let root = if path.file_name().and_then(|name| name.to_str()) == Some(LAYRS_DIR) {
        path.parent()
            .ok_or_else(|| "Layrs Desktop could not resolve Local Space root.".to_string())?
            .to_path_buf()
    } else if path.file_name().and_then(|name| name.to_str()) == Some("local-space.json") {
        path.parent()
            .and_then(Path::parent)
            .ok_or_else(|| "Layrs Desktop could not resolve Local Space root.".to_string())?
            .to_path_buf()
    } else {
        path
    };
    let layrs_dir = root.join(LAYRS_DIR);
    let meta = read_json::<LocalSpaceFile>(&layrs_dir.join("local-space.json"))?;
    let active = read_json::<ActiveLayerFile>(&layrs_dir.join("active-layer.json"))?;

    Ok(LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
    })
}

fn find_local_space_config_entry(
    config: &DesktopConfig,
    selector: &str,
) -> Option<LocalSpaceConfigEntry> {
    let selector_path_key = if selector.trim().is_empty() {
        None
    } else {
        Some(path_compare_key(&PathBuf::from(selector)))
    };
    config
        .local_spaces
        .iter()
        .find(|entry| {
            entry.local_space_id == selector
                || selector_path_key
                    .as_ref()
                    .is_some_and(|key| path_compare_key(&PathBuf::from(&entry.root_path)) == *key)
        })
        .cloned()
}

fn archive_layrs_dir_at(root: &Path, layrs_dir: &Path) -> Result<Option<PathBuf>, String> {
    if !layrs_dir.exists() {
        return Ok(None);
    }

    let timestamp = unix_now();
    let mut archive_path = root.join(format!(".layrs-forgotten-{timestamp}"));
    let mut suffix = 2;
    while archive_path.exists() {
        archive_path = root.join(format!(".layrs-forgotten-{timestamp}-{suffix}"));
        suffix += 1;
    }

    fs::rename(layrs_dir, &archive_path).map_err(|error| {
        format!(
            "Layrs Desktop could not archive local metadata {} to {}: {error}",
            layrs_dir.display(),
            archive_path.display()
        )
    })?;
    Ok(Some(archive_path))
}

fn path_compare_key(path: &Path) -> String {
    let absolute = absolute_path(&path.to_path_buf()).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        absolute.display().to_string().to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        absolute.display().to_string()
    }
}

fn summary_from_handle(handle: &LocalSpaceHandle) -> LocalSpaceSummary {
    LocalSpaceSummary {
        local_space_id: handle.meta.local_space_id.clone(),
        space_id: handle.meta.space_id.clone(),
        workspace_id: handle.meta.workspace_id.clone(),
        server_space_id: handle.meta.server_space_id.clone(),
        state: handle.meta.state.clone(),
        name: handle.meta.name.clone(),
        root_path: handle.root.display().to_string(),
        active_layer_id: Some(handle.active.layer_id.clone()),
        layers: handle
            .meta
            .layers
            .iter()
            .map(|layer| LocalLayerSummary {
                layer_id: layer.layer_id.clone(),
                display_name: layer.display_name.clone(),
                parent_layer_id: layer.parent_layer_id.clone(),
                access: layer.access.clone(),
                can_open: layer.can_open,
                path: layer_dir(&handle.layrs_dir, &layer.layer_id)
                    .display()
                    .to_string(),
                sync_status: layer_sync_status(&handle.meta, &layer.layer_id),
            })
            .collect(),
    }
}

fn layer_sync_status(meta: &LocalSpaceFile, layer_id: &str) -> String {
    if meta.state == LOCAL_SPACE_STATE_DRAFT {
        "local".to_string()
    } else if is_probably_server_layer_id(layer_id) {
        "linked".to_string()
    } else {
        "local-only".to_string()
    }
}

fn capture_working_state(
    root: &Path,
    layer_id: &str,
    write_objects: bool,
) -> Result<WorkingStateFile, String> {
    let layrs_dir = root.join(LAYRS_DIR);
    if write_objects {
        create_local_space_directories(&layrs_dir)?;
    }
    let previous_cache = read_scan_cache(&layrs_dir);
    let mut next_cache = BTreeMap::new();
    let mut files = Vec::new();
    collect_files(
        root,
        root,
        &layrs_dir,
        &previous_cache,
        &mut next_cache,
        &mut files,
        write_objects,
    )?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let root_tree_id = if write_objects {
        Some(write_tree_object(&layrs_dir, &files)?)
    } else {
        Some(tree_id_for_files(&files))
    };
    if write_objects {
        write_scan_cache(&layrs_dir, next_cache)?;
    }

    Ok(WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: layer_id.to_string(),
        captured_at_unix: unix_now(),
        root_tree_id,
        files,
    })
}

fn collect_files(
    root: &Path,
    current: &Path,
    layrs_dir: &Path,
    previous_cache: &BTreeMap<String, ScanCacheEntry>,
    next_cache: &mut BTreeMap<String, ScanCacheEntry>,
    files: &mut Vec<FileSnapshotEntry>,
    write_objects: bool,
) -> Result<(), String> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| {
            format!(
                "Layrs Desktop could not scan working tree {}: {error}",
                current.display()
            )
        })?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| format!("Layrs Desktop could not read working tree entry: {error}"))?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(LAYRS_DIR) {
            continue;
        }

        let file_type = entry.file_type().map_err(|error| {
            format!(
                "Layrs Desktop could not inspect working tree path {}: {error}",
                path.display()
            )
        })?;

        if file_type.is_dir() {
            collect_files(
                root,
                &path,
                layrs_dir,
                previous_cache,
                next_cache,
                files,
                write_objects,
            )?;
        } else if file_type.is_file() {
            let key = relative_key(root, &path)?;
            let metadata = entry.metadata().map_err(|error| {
                format!(
                    "Layrs Desktop could not inspect working tree file {}: {error}",
                    path.display()
                )
            })?;
            let size = metadata.len();
            let modified_at = metadata
                .modified()
                .map(system_time_marker)
                .unwrap_or_else(|_| "unknown".to_string());
            if let Some(cached) = previous_cache.get(&key) {
                if cached.size == size && cached.modified_at == modified_at {
                    files.push(cached.snapshot.clone());
                    next_cache.insert(key, cached.clone());
                    continue;
                }
            }

            let bytes = fs::read(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not read working tree file {}: {error}",
                    path.display()
                )
            })?;
            let snapshot = write_file_object(layrs_dir, &key, &bytes, write_objects)?;
            next_cache.insert(
                key,
                ScanCacheEntry {
                    path: snapshot.path.clone(),
                    size,
                    modified_at,
                    snapshot: snapshot.clone(),
                },
            );
            files.push(snapshot);
        }
    }

    Ok(())
}

fn materialize_state(root: &Path, state: &WorkingStateFile) -> Result<(), String> {
    let current = capture_working_state(root, &state.layer_id, false)?;
    let (added, modified, deleted) = diff_state(Some(&current), state);

    for path_key in deleted {
        let path = path_from_key(root, &path_key)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not remove file while switching Layer {}: {error}",
                    path.display()
                )
            })?;
        }
    }

    let target_files = file_entries(&state.files);
    for path_key in added.iter().chain(modified.iter()) {
        let file = target_files.get(path_key).ok_or_else(|| {
            format!("Layrs Desktop cannot restore missing tree entry {path_key}.")
        })?;
        let target = path_from_key(root, &file.path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Layrs Desktop could not create directory while switching Layer {}: {error}",
                    parent.display()
                )
            })?;
        }
        let bytes = read_snapshot_object_bytes(&root.join(LAYRS_DIR), file)?;
        fs::write(&target, bytes).map_err(|error| {
            format!(
                "Layrs Desktop could not restore file {}: {error}",
                target.display()
            )
        })?;
    }

    Ok(())
}

fn read_layer_state(layrs_dir: &Path, layer_id: &str) -> Result<WorkingStateFile, String> {
    read_state_file(
        layrs_dir,
        &layer_dir(layrs_dir, layer_id).join("working-state.json"),
    )
}

fn read_layer_index(layrs_dir: &Path, layer_id: &str) -> Result<WorkingStateFile, String> {
    read_state_file(
        layrs_dir,
        &layer_dir(layrs_dir, layer_id).join("index.json"),
    )
}

fn read_state_file(layrs_dir: &Path, path: &Path) -> Result<WorkingStateFile, String> {
    let mut state = read_json::<WorkingStateFile>(path)?;
    hydrate_state_files(layrs_dir, &mut state)?;
    Ok(state)
}

fn hydrate_state_files(layrs_dir: &Path, state: &mut WorkingStateFile) -> Result<(), String> {
    if state.files.is_empty() {
        if let Some(root_tree_id) = state.root_tree_id.clone() {
            state.files = read_tree_object(layrs_dir, &root_tree_id)?.files;
        }
    } else if state.root_tree_id.is_none() {
        state.root_tree_id = Some(write_tree_object(layrs_dir, &state.files)?);
    }
    Ok(())
}

fn storage_state(layrs_dir: &Path, state: &WorkingStateFile) -> Result<WorkingStateFile, String> {
    let root_tree_id = state
        .root_tree_id
        .clone()
        .map(Ok)
        .unwrap_or_else(|| write_tree_object(layrs_dir, &state.files))?;
    Ok(WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: state.layer_id.clone(),
        captured_at_unix: state.captured_at_unix,
        root_tree_id: Some(root_tree_id),
        files: Vec::new(),
    })
}

fn write_layer_state(
    layrs_dir: &Path,
    layer_id: &str,
    state: &WorkingStateFile,
) -> Result<(), String> {
    let dir = layer_dir(layrs_dir, layer_id);
    let state = storage_state(layrs_dir, state)?;
    write_json(&dir.join("working-state.json"), &state)?;
    write_json(&dir.join("index.json"), &state)
}

fn write_working_state(
    layrs_dir: &Path,
    layer_id: &str,
    state: &WorkingStateFile,
) -> Result<(), String> {
    let state = storage_state(layrs_dir, state)?;
    write_json(
        &layer_dir(layrs_dir, layer_id).join("working-state.json"),
        &state,
    )
}

fn write_step(
    layrs_dir: &Path,
    layer_id: &str,
    state: &WorkingStateFile,
) -> Result<String, String> {
    let step_id = format!("{}-{}", unix_now(), fnv1a_hex(layer_id.as_bytes()));
    let (parent_step_id, base_layer_id, base_tree_id, base_state) = step_base(layrs_dir, layer_id)?;
    let (added, modified, deleted) = diff_state(base_state.as_ref(), state);
    let changed_paths = added
        .iter()
        .chain(modified.iter())
        .chain(deleted.iter())
        .cloned()
        .collect::<Vec<_>>();
    let root_tree_id = state
        .root_tree_id
        .clone()
        .map(Ok)
        .unwrap_or_else(|| write_tree_object(layrs_dir, &state.files))?;
    let step = StepFile {
        schema: STEP_SCHEMA.to_string(),
        step_id: step_id.clone(),
        layer_id: layer_id.to_string(),
        parent_step_id,
        base_layer_id,
        base_tree_id,
        root_tree_id: Some(root_tree_id),
        changed_paths,
        captured_at_unix: state.captured_at_unix,
        files: Vec::new(),
    };
    write_json(
        &layer_dir(layrs_dir, layer_id)
            .join("steps")
            .join(format!("{step_id}.json")),
        &step,
    )?;
    Ok(step_id)
}

fn read_step_file(layrs_dir: &Path, layer_id: &str, step_id: &str) -> Result<StepFile, String> {
    read_json(
        &layer_dir(layrs_dir, layer_id)
            .join("steps")
            .join(format!("{step_id}.json")),
    )
}

fn step_base(
    layrs_dir: &Path,
    layer_id: &str,
) -> Result<
    (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<WorkingStateFile>,
    ),
    String,
> {
    let mut steps = read_step_files(layrs_dir, layer_id)?;
    steps.sort_by(|left, right| {
        left.captured_at_unix
            .cmp(&right.captured_at_unix)
            .then_with(|| left.step_id.cmp(&right.step_id))
    });
    if let Some(previous_step) = steps.last() {
        let state = state_from_step(layrs_dir, previous_step)?;
        return Ok((
            Some(previous_step.step_id.clone()),
            Some(layer_id.to_string()),
            state.root_tree_id.clone(),
            Some(state),
        ));
    }

    let parent_layer_id = read_json::<LocalSpaceFile>(&layrs_dir.join("local-space.json"))
        .ok()
        .and_then(|meta| {
            meta.layers
                .into_iter()
                .find(|layer| layer.layer_id == layer_id)
                .and_then(|layer| layer.parent_layer_id)
        });
    let base_layer_id = parent_layer_id.unwrap_or_else(|| layer_id.to_string());
    let base_state = read_layer_index(layrs_dir, &base_layer_id).ok();
    Ok((
        None,
        Some(base_layer_id),
        base_state
            .as_ref()
            .and_then(|state| state.root_tree_id.clone()),
        base_state,
    ))
}

fn state_from_step(layrs_dir: &Path, step: &StepFile) -> Result<WorkingStateFile, String> {
    let mut state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: step.layer_id.clone(),
        captured_at_unix: step.captured_at_unix,
        root_tree_id: step.root_tree_id.clone(),
        files: step.files.clone(),
    };
    hydrate_state_files(layrs_dir, &mut state)?;
    Ok(state)
}

fn recorded_base_state_for_step(layrs_dir: &Path, step: &StepFile) -> Option<WorkingStateFile> {
    let root_tree_id = step.base_tree_id.clone()?;
    let mut state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: step
            .base_layer_id
            .clone()
            .unwrap_or_else(|| step.layer_id.clone()),
        captured_at_unix: step.captured_at_unix,
        root_tree_id: Some(root_tree_id),
        files: Vec::new(),
    };
    hydrate_state_files(layrs_dir, &mut state).ok()?;
    Some(state)
}

fn tree_id_for_files(files: &[FileSnapshotEntry]) -> String {
    let mut material = String::new();
    for file in files {
        material.push_str(&file.path);
        material.push('\0');
        material.push_str(&file.hash);
        material.push('\0');
        material.push_str(&file.size.to_string());
        material.push('\n');
    }
    blake3_id(material.as_bytes())
}

fn write_tree_object(layrs_dir: &Path, files: &[FileSnapshotEntry]) -> Result<String, String> {
    let tree_id = tree_id_for_files(files);
    let path = tree_object_path(layrs_dir, &tree_id);
    if !path.exists() {
        let tree = TreeObjectFile {
            schema: TREE_OBJECT_SCHEMA.to_string(),
            tree_id: tree_id.clone(),
            files: files.to_vec(),
        };
        write_json(&path, &tree)?;
    }
    Ok(tree_id)
}

fn read_tree_object(layrs_dir: &Path, tree_id: &str) -> Result<TreeObjectFile, String> {
    read_json(&tree_object_path(layrs_dir, tree_id))
}

fn tree_object_path(layrs_dir: &Path, tree_id: &str) -> PathBuf {
    layrs_dir
        .join("objects")
        .join("trees")
        .join(format!("{}.json", object_file_stem(tree_id)))
}

fn write_file_object(
    layrs_dir: &Path,
    path: &str,
    bytes: &[u8],
    write_objects: bool,
) -> Result<FileSnapshotEntry, String> {
    validate_snapshot_key(path)?;
    let hash = blake3_id(bytes);
    let object = format!("objects/files/{}.json", object_file_stem(&hash));
    if write_objects {
        write_file_object_manifest(layrs_dir, &hash, bytes)?;
    }

    Ok(FileSnapshotEntry {
        path: path.to_string(),
        object,
        hash,
        size: bytes.len() as u64,
    })
}

fn write_file_object_manifest(layrs_dir: &Path, hash: &str, bytes: &[u8]) -> Result<(), String> {
    let object_path = layrs_dir
        .join("objects")
        .join("files")
        .join(format!("{}.json", object_file_stem(hash)));
    if object_path.exists() {
        return Ok(());
    }

    let mut chunks = Vec::new();
    for chunk in bytes.chunks(CHUNK_SIZE) {
        let chunk_id = blake3_id(chunk);
        let chunk_path = layrs_dir
            .join("objects")
            .join("chunks")
            .join(format!("{}.chunk", object_file_stem(&chunk_id)));
        if !chunk_path.exists() {
            if let Some(parent) = chunk_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "Layrs Desktop could not create chunk directory {}: {error}",
                        parent.display()
                    )
                })?;
            }
            fs::write(&chunk_path, chunk).map_err(|error| {
                format!(
                    "Layrs Desktop could not write chunk object {}: {error}",
                    chunk_path.display()
                )
            })?;
        }
        chunks.push(FileChunkRef {
            chunk_id,
            size: chunk.len() as u64,
        });
    }

    let manifest = FileObjectFile {
        schema: FILE_OBJECT_SCHEMA.to_string(),
        hash: hash.to_string(),
        size: bytes.len() as u64,
        chunks,
    };
    write_json(&object_path, &manifest)
}

fn read_snapshot_object_bytes(
    layrs_dir: &Path,
    file: &FileSnapshotEntry,
) -> Result<Vec<u8>, String> {
    let object_path = layrs_dir.join(&file.object);
    if file.object.starts_with("objects/files/") {
        let manifest = read_json::<FileObjectFile>(&object_path)?;
        let mut bytes = Vec::with_capacity(manifest.size as usize);
        for chunk in manifest.chunks {
            let chunk_path = layrs_dir
                .join("objects")
                .join("chunks")
                .join(format!("{}.chunk", object_file_stem(&chunk.chunk_id)));
            let chunk_bytes = fs::read(&chunk_path).map_err(|error| {
                format!(
                    "Layrs Desktop could not read chunk object {}: {error}",
                    chunk_path.display()
                )
            })?;
            bytes.extend_from_slice(&chunk_bytes);
        }
        if blake3_id(&bytes) != file.hash {
            return Err(format!(
                "Layrs Desktop object hash mismatch while reading {}.",
                file.path
            ));
        }
        return Ok(bytes);
    }

    fs::read(&object_path).map_err(|error| {
        format!(
            "Layrs Desktop could not read snapshot object {}: {error}",
            object_path.display()
        )
    })
}

fn read_scan_cache(layrs_dir: &Path) -> BTreeMap<String, ScanCacheEntry> {
    read_json::<ScanCacheFile>(&layrs_dir.join("scan-cache.json"))
        .map(|cache| {
            cache
                .entries
                .into_iter()
                .map(|entry| (entry.path.clone(), entry))
                .collect()
        })
        .unwrap_or_default()
}

fn write_scan_cache(
    layrs_dir: &Path,
    entries: BTreeMap<String, ScanCacheEntry>,
) -> Result<(), String> {
    let cache = ScanCacheFile {
        schema: SCAN_CACHE_SCHEMA.to_string(),
        entries: entries.into_values().collect(),
    };
    write_json(&layrs_dir.join("scan-cache.json"), &cache)
}

fn system_time_marker(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| format!("{}.{}", duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn diff_state(
    previous: Option<&WorkingStateFile>,
    current: &WorkingStateFile,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let previous_files = previous
        .map(|state| file_hashes(&state.files))
        .unwrap_or_default();
    let current_files = file_hashes(&current.files);

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for (path, hash) in &current_files {
        match previous_files.get(path) {
            None => added.push(path.clone()),
            Some(previous_hash) if previous_hash != hash => modified.push(path.clone()),
            _ => {}
        }
    }

    for path in previous_files.keys() {
        if !current_files.contains_key(path) {
            deleted.push(path.clone());
        }
    }

    (added, modified, deleted)
}

fn changed_file_count(previous: Option<&WorkingStateFile>, current: &WorkingStateFile) -> usize {
    let (added, modified, deleted) = diff_state(previous, current);
    added.len() + modified.len() + deleted.len()
}

fn lens_diff_entries(
    handle: &LocalSpaceHandle,
    source: &str,
    layer_id: &str,
    step_id: Option<&str>,
    previous: Option<&WorkingStateFile>,
    current: &WorkingStateFile,
    added: &[String],
    modified: &[String],
    deleted: &[String],
) -> Vec<LensDiffEntry> {
    let previous_files = previous
        .map(|state| file_entries(&state.files))
        .unwrap_or_default();
    let current_files = file_entries(&current.files);
    let mut diffs = Vec::with_capacity(added.len() + modified.len() + deleted.len());

    for path in added {
        if let Some(diff) = lens_diff_entry(
            handle,
            path,
            "added",
            "Added locally",
            None,
            current_files.get(path),
            DiffWindowOptions::scan(source, layer_id, step_id),
        ) {
            diffs.push(diff);
        }
    }

    for path in modified {
        if let Some(diff) = lens_diff_entry(
            handle,
            path,
            "modified",
            "Modified locally",
            previous_files.get(path),
            current_files.get(path),
            DiffWindowOptions::scan(source, layer_id, step_id),
        ) {
            diffs.push(diff);
        }
    }

    for path in deleted {
        if let Some(diff) = lens_diff_entry(
            handle,
            path,
            "deleted",
            "Deleted locally",
            previous_files.get(path),
            None,
            DiffWindowOptions::scan(source, layer_id, step_id),
        ) {
            diffs.push(diff);
        }
    }

    diffs
}

fn load_step_diff_window(
    handle: &LocalSpaceHandle,
    path: &str,
    step_id: &str,
    start: usize,
    limit: usize,
) -> Result<LensDiffEntry, String> {
    let layer_id = handle.active.layer_id.clone();
    let mut steps = read_step_files(&handle.layrs_dir, &layer_id)?;
    steps.sort_by(|left, right| {
        left.captured_at_unix
            .cmp(&right.captured_at_unix)
            .then_with(|| left.step_id.cmp(&right.step_id))
    });

    let mut previous_state: Option<WorkingStateFile> = None;
    for step in steps {
        let current_state = state_from_step(&handle.layrs_dir, &step)?;
        let base_state = previous_state
            .clone()
            .or_else(|| recorded_base_state_for_step(&handle.layrs_dir, &step))
            .or_else(|| initial_step_base_state(handle, &layer_id));
        if step.step_id == step_id {
            return diff_window_for_path(
                handle,
                "localStep",
                &step.layer_id,
                Some(&step.step_id),
                base_state.as_ref(),
                &current_state,
                path,
                start,
                limit,
            );
        }
        previous_state = Some(current_state);
    }

    Err(format!(
        "Layrs Desktop could not find Local Step {step_id} on the active Layer."
    ))
}

fn diff_window_for_path(
    handle: &LocalSpaceHandle,
    source: &str,
    layer_id: &str,
    step_id: Option<&str>,
    previous: Option<&WorkingStateFile>,
    current: &WorkingStateFile,
    path: &str,
    start: usize,
    limit: usize,
) -> Result<LensDiffEntry, String> {
    let previous_files = previous
        .map(|state| file_entries(&state.files))
        .unwrap_or_default();
    let current_files = file_entries(&current.files);
    let old_file = previous_files.get(path);
    let new_file = current_files.get(path);
    let (state, title) = match (old_file, new_file) {
        (None, Some(_)) => ("added", "Added locally"),
        (Some(_), None) => ("deleted", "Deleted locally"),
        (Some(old_file), Some(new_file)) if old_file.hash != new_file.hash => {
            ("modified", "Modified locally")
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "Layrs Desktop did not find text changes for {path} in source {source}."
            ));
        }
        (None, None) => {
            return Err(format!(
                "Layrs Desktop could not find {path} in source {source}."
            ));
        }
    };

    lens_diff_entry(
        handle,
        path,
        state,
        title,
        old_file,
        new_file,
        DiffWindowOptions::requested(source, layer_id, step_id, start, limit),
    )
    .ok_or_else(|| format!("Layrs Desktop could not build a diff for {path}."))
}

fn local_step_summaries(
    handle: &LocalSpaceHandle,
    layer_id: &str,
) -> Result<Vec<LocalStepSummary>, String> {
    let mut steps = read_step_files(&handle.layrs_dir, layer_id)?;
    steps.sort_by(|left, right| {
        left.captured_at_unix
            .cmp(&right.captured_at_unix)
            .then_with(|| left.step_id.cmp(&right.step_id))
    });

    let mut summaries = Vec::with_capacity(steps.len());
    let mut previous_state: Option<WorkingStateFile> = None;

    for step in steps {
        let current_state = state_from_step(&handle.layrs_dir, &step)?;
        let base_state = previous_state
            .clone()
            .or_else(|| recorded_base_state_for_step(&handle.layrs_dir, &step))
            .or_else(|| initial_step_base_state(handle, layer_id));
        let (added, modified, deleted) = diff_state(base_state.as_ref(), &current_state);
        let diffs = lens_diff_entries(
            handle,
            "localStep",
            &step.layer_id,
            Some(&step.step_id),
            base_state.as_ref(),
            &current_state,
            &added,
            &modified,
            &deleted,
        );
        let changed_files = added.len() + modified.len() + deleted.len();
        let diff_stats = diff_stats(&diffs);

        summaries.push(LocalStepSummary {
            step_id: step.step_id,
            layer_id: step.layer_id,
            captured_at: step.captured_at_unix,
            changed_files,
            diff_stats,
            diffs,
        });
        previous_state = Some(current_state);
    }

    Ok(summaries)
}

fn initial_step_base_state(handle: &LocalSpaceHandle, layer_id: &str) -> Option<WorkingStateFile> {
    let parent_layer_id = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)
        .and_then(|layer| layer.parent_layer_id.as_deref());

    if let Some(parent_layer_id) = parent_layer_id {
        read_layer_state(&handle.layrs_dir, parent_layer_id)
            .or_else(|_| read_layer_index(&handle.layrs_dir, parent_layer_id))
            .ok()
    } else {
        read_layer_index(&handle.layrs_dir, layer_id).ok()
    }
}

fn read_step_files(layrs_dir: &Path, layer_id: &str) -> Result<Vec<StepFile>, String> {
    let steps_dir = layer_dir(layrs_dir, layer_id).join("steps");
    if !steps_dir.exists() {
        return Ok(Vec::new());
    }

    let mut steps = Vec::new();
    let entries = fs::read_dir(&steps_dir).map_err(|error| {
        format!(
            "Layrs Desktop could not read Layer steps {}: {error}",
            steps_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Layrs Desktop could not read Layer step entry {}: {error}",
                steps_dir.display()
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            steps.push(read_json::<StepFile>(&path)?);
        }
    }

    Ok(steps)
}

fn lens_diff_entry(
    handle: &LocalSpaceHandle,
    path: &str,
    state: &str,
    title: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    options: DiffWindowOptions<'_>,
) -> Option<LensDiffEntry> {
    let lens_id = lens_id_for_path(path).to_string();
    let (diff, message) = if lens_id == "layrs.code" || lens_id == "layrs.text" {
        let old_text = old_file
            .map(|file| read_text_object_for_diff(handle, file))
            .unwrap_or_else(TextDiffSource::absent);
        let new_text = new_file
            .map(|file| read_text_object_for_diff(handle, file))
            .unwrap_or_else(TextDiffSource::absent);
        let message = text_diff_message(
            old_file,
            new_file,
            old_text.readable,
            new_text.readable,
            old_text.truncated || new_text.truncated,
        );
        (
            text_diff_model(
                path,
                state,
                &lens_id,
                old_text.text.as_deref().unwrap_or_default(),
                new_text.text.as_deref().unwrap_or_default(),
                old_file,
                new_file,
                old_text.truncated,
                new_text.truncated,
                &options,
            ),
            message,
        )
    } else if lens_id == "layrs.image" {
        (
            metadata_diff_model("imageMetadata", path, state, &lens_id, old_file, new_file),
            Some(
                "Image metadata diff is available; visual pixel diff is not available yet."
                    .to_string(),
            ),
        )
    } else {
        (
            metadata_diff_model("binary", path, state, &lens_id, old_file, new_file),
            Some(
                "Binary diff is represented as metadata because visual diff is not available."
                    .to_string(),
            ),
        )
    };

    Some(LensDiffEntry {
        path: path.to_string(),
        state: state.to_string(),
        lens_id,
        title: title.to_string(),
        diff,
        message,
    })
}

fn text_diff_model(
    path: &str,
    state: &str,
    lens_id: &str,
    old_text: &str,
    new_text: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    old_truncated: bool,
    new_truncated: bool,
    options: &DiffWindowOptions<'_>,
) -> DiffModel {
    let old_lines = split_text_lines(old_text);
    let new_lines = split_text_lines(new_text);
    let mut all_lines = diff_text_lines(&old_lines, &new_lines);
    let old_hash = old_file.map(|file| file.hash.as_str());
    let new_hash = new_file.map(|file| file.hash.as_str());
    if line_stats(&all_lines).additions == 0
        && line_stats(&all_lines).deletions == 0
        && old_hash != new_hash
    {
        all_lines = synthetic_text_change_lines(old_file, new_file, old_truncated || new_truncated);
    }
    let stats = line_stats(&all_lines);
    let total_diff_lines = all_lines.len();
    let window_start = options.start.min(total_diff_lines);
    let window_limit = clamp_diff_window_limit(options.limit);
    let window_end = window_start
        .saturating_add(window_limit)
        .min(total_diff_lines);
    let lines = all_lines[window_start..window_end].to_vec();
    let has_more = window_end < total_diff_lines;
    let large_diff = old_truncated
        || new_truncated
        || has_more
        || total_diff_lines > window_limit;
    let mut fields = common_diff_fields(path, state, lens_id, old_file, new_file);
    fields.insert(
        "source".to_string(),
        Value::String(options.source.to_string()),
    );
    fields.insert(
        "layerId".to_string(),
        Value::String(options.layer_id.to_string()),
    );
    if let Some(step_id) = options.step_id {
        fields.insert("stepId".to_string(), Value::String(step_id.to_string()));
    }
    fields.insert("additions".to_string(), Value::from(stats.additions as u64));
    fields.insert("deletions".to_string(), Value::from(stats.deletions as u64));
    fields.insert("unchanged".to_string(), Value::from(stats.unchanged as u64));
    fields.insert(
        "totalOldLines".to_string(),
        Value::from(old_lines.len() as u64),
    );
    fields.insert(
        "totalNewLines".to_string(),
        Value::from(new_lines.len() as u64),
    );
    fields.insert(
        "totalDiffLines".to_string(),
        Value::from(total_diff_lines as u64),
    );
    fields.insert("windowStart".to_string(), Value::from(window_start as u64));
    fields.insert("windowLimit".to_string(), Value::from(window_limit as u64));
    fields.insert("windowEnd".to_string(), Value::from(window_end as u64));
    fields.insert("hasMore".to_string(), Value::Bool(has_more));
    fields.insert("largeDiff".to_string(), Value::Bool(large_diff));
    fields.insert("preview".to_string(), Value::Bool(options.preview));
    fields.insert(
        "windowUnit".to_string(),
        Value::String("diffLines".to_string()),
    );
    fields.insert("oldTruncated".to_string(), Value::Bool(old_truncated));
    fields.insert("newTruncated".to_string(), Value::Bool(new_truncated));
    if old_truncated || new_truncated {
        fields.insert("truncated".to_string(), Value::Bool(true));
    }

    DiffModel {
        kind: "textLines".to_string(),
        summary: summarize_line_diff(&stats, large_diff),
        hunks: vec![DiffHunk {
            old_start: first_old_line(&lines).unwrap_or(1),
            old_lines: lines.iter().filter(|line| line.old_line.is_some()).count(),
            new_start: first_new_line(&lines).unwrap_or(1),
            new_lines: lines.iter().filter(|line| line.new_line.is_some()).count(),
            lines,
        }],
        fields,
    }
}

fn metadata_diff_model(
    kind: &str,
    path: &str,
    state: &str,
    lens_id: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
) -> DiffModel {
    DiffModel {
        kind: kind.to_string(),
        summary: metadata_diff_summary(kind, state),
        hunks: Vec::new(),
        fields: common_diff_fields(path, state, lens_id, old_file, new_file),
    }
}

fn metadata_diff_summary(kind: &str, state: &str) -> String {
    let label = if kind == "imageMetadata" {
        "image metadata"
    } else {
        "binary metadata"
    };

    match state {
        "added" => format!("Added {label}"),
        "deleted" => format!("Deleted {label}"),
        _ => format!("Changed {label}"),
    }
}

fn common_diff_fields(
    path: &str,
    state: &str,
    lens_id: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    fields.insert("path".to_string(), Value::String(path.to_string()));
    fields.insert("state".to_string(), Value::String(state.to_string()));
    fields.insert("lensId".to_string(), Value::String(lens_id.to_string()));
    fields.insert(
        "mediaType".to_string(),
        Value::String(media_type_for_path(path).to_string()),
    );
    if let Some(file) = old_file {
        fields.insert("oldHash".to_string(), Value::String(file.hash.clone()));
        fields.insert("oldSize".to_string(), Value::from(file.size));
    }
    if let Some(file) = new_file {
        fields.insert("newHash".to_string(), Value::String(file.hash.clone()));
        fields.insert("newSize".to_string(), Value::from(file.size));
    }
    fields
}

fn split_text_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(ToString::to_string)
        .collect()
}

fn clamp_diff_window_limit(limit: usize) -> usize {
    if limit == 0 {
        TEXT_DIFF_DEFAULT_WINDOW_LIMIT
    } else {
        limit.min(TEXT_DIFF_MAX_WINDOW_LIMIT)
    }
}

fn first_old_line(lines: &[DiffLine]) -> Option<usize> {
    lines.iter().find_map(|line| line.old_line)
}

fn first_new_line(lines: &[DiffLine]) -> Option<usize> {
    lines.iter().find_map(|line| line.new_line)
}

fn diff_text_lines(old_lines: &[String], new_lines: &[String]) -> Vec<DiffLine> {
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix + prefix < old_lines.len()
        && suffix + prefix < new_lines.len()
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut lines = Vec::with_capacity(old_lines.len().max(new_lines.len()));

    for index in 0..prefix {
        lines.push(DiffLine {
            op: "equal".to_string(),
            old_line: Some(index + 1),
            new_line: Some(index + 1),
            text: old_lines[index].clone(),
        });
    }

    for old_index in prefix..old_lines.len().saturating_sub(suffix) {
        lines.push(DiffLine {
            op: "delete".to_string(),
            old_line: Some(old_index + 1),
            new_line: None,
            text: old_lines[old_index].clone(),
        });
    }

    for new_index in prefix..new_lines.len().saturating_sub(suffix) {
        lines.push(DiffLine {
            op: "insert".to_string(),
            old_line: None,
            new_line: Some(new_index + 1),
            text: new_lines[new_index].clone(),
        });
    }

    for offset in 0..suffix {
        let old_index = old_lines.len() - suffix + offset;
        let new_index = new_lines.len() - suffix + offset;
        lines.push(DiffLine {
            op: "equal".to_string(),
            old_line: Some(old_index + 1),
            new_line: Some(new_index + 1),
            text: old_lines[old_index].clone(),
        });
    }

    lines
}

fn synthetic_text_change_lines(
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    truncated: bool,
) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    if let Some(file) = old_file {
        lines.push(DiffLine {
            op: "delete".to_string(),
            old_line: Some(1),
            new_line: None,
            text: synthetic_text_change_label("Old", file, truncated),
        });
    }
    if let Some(file) = new_file {
        lines.push(DiffLine {
            op: "insert".to_string(),
            old_line: None,
            new_line: Some(1),
            text: synthetic_text_change_label("New", file, truncated),
        });
    }
    lines
}

fn synthetic_text_change_label(side: &str, file: &FileSnapshotEntry, truncated: bool) -> String {
    let detail = if truncated {
        "content changed outside available text content"
    } else {
        "content changed but text preview is unavailable"
    };
    format!(
        "{side} {detail}; size: {}; object: {}",
        format_bytes(file.size),
        file.hash
    )
}

fn line_stats(lines: &[DiffLine]) -> TextLineStats {
    let mut stats = TextLineStats::default();
    for line in lines {
        match line.op.as_str() {
            "insert" => stats.additions += 1,
            "delete" => stats.deletions += 1,
            _ => stats.unchanged += 1,
        }
    }
    stats
}

fn diff_stats(diffs: &[LensDiffEntry]) -> DiffStats {
    let mut stats = DiffStats {
        files: diffs.len(),
        ..DiffStats::default()
    };

    for diff in diffs {
        for hunk in &diff.diff.hunks {
            for line in &hunk.lines {
                match line.op.as_str() {
                    "insert" => stats.additions += 1,
                    "delete" => stats.deletions += 1,
                    _ => {}
                }
            }
        }
    }

    stats
}

#[derive(Clone, Debug, Default)]
struct TextLineStats {
    additions: usize,
    deletions: usize,
    unchanged: usize,
}

fn summarize_line_diff(stats: &TextLineStats, truncated: bool) -> String {
    if stats.additions == 0 && stats.deletions == 0 {
        "No text changes".to_string()
    } else if truncated {
        format!(
            "Large text changed: {} additions, {} deletions",
            stats.additions, stats.deletions
        )
    } else {
        format!(
            "{} additions, {} deletions",
            stats.additions, stats.deletions
        )
    }
}

fn file_entries(files: &[FileSnapshotEntry]) -> BTreeMap<String, FileSnapshotEntry> {
    files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect()
}

fn read_text_object_for_diff(
    handle: &LocalSpaceHandle,
    file: &FileSnapshotEntry,
) -> TextDiffSource {
    if !(is_text_path(&file.path) || is_code_path(&file.path)) {
        return TextDiffSource {
            text: None,
            readable: false,
            truncated: false,
        };
    }

    match read_snapshot_object_bytes(&handle.layrs_dir, file)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        Some(text) => TextDiffSource {
            text: Some(text),
            readable: true,
            truncated: false,
        },
        None => TextDiffSource {
            text: None,
            readable: false,
            truncated: false,
        },
    }
}

fn text_diff_message(
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    old_readable: bool,
    new_readable: bool,
    truncated: bool,
) -> Option<String> {
    if !old_readable || !new_readable {
        return Some(
            "Text diff preview is unavailable because this file could not be read as UTF-8."
                .to_string(),
        );
    }

    if truncated {
        let old_size = old_file.map(|file| format_bytes(file.size));
        let new_size = new_file.map(|file| format_bytes(file.size));
        return Some(format!(
            "Large text diff is available in line windows. Old size: {}; New size: {}.",
            old_size.as_deref().unwrap_or("none"),
            new_size.as_deref().unwrap_or("none")
        ));
    }

    None
}

fn format_bytes(size: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / MIB)
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / KIB)
    } else {
        format!("{size} B")
    }
}

fn file_hashes(files: &[FileSnapshotEntry]) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|file| (file.path.clone(), file.hash.clone()))
        .collect()
}

fn write_sync_state(
    handle: &LocalSpaceHandle,
    operation: &str,
    status: &str,
    changed_files: usize,
    server_cursor: Option<String>,
) -> Result<PathBuf, String> {
    let path = layer_dir(&handle.layrs_dir, &handle.active.layer_id).join("sync-state.json");
    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": handle.active.layer_id,
        "lastManualOperation": operation,
        "lastManualOperationUnix": unix_now(),
        "changedFiles": changed_files,
        "serverCursor": server_cursor,
        "pending": false,
        "status": status
    });
    write_json(&path, &sync_state)?;
    Ok(path)
}

fn write_linked_layer_sync_state(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    parent_layer_id: Option<&str>,
) -> Result<PathBuf, String> {
    let path = layer_dir(&handle.layrs_dir, layer_id).join("sync-state.json");
    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": layer_id,
        "parentLayerId": parent_layer_id,
        "lastReceiveUnix": null,
        "lastPublishUnix": null,
        "pending": false,
        "status": "linked"
    });
    write_json(&path, &sync_state)?;
    Ok(path)
}

fn apply_receive_response(
    handle: &mut LocalSpaceHandle,
    response: ReceiveLocalSpaceResponse,
    materialize_active: bool,
    endpoint: Option<&str>,
    token: Option<&str>,
) -> Result<PathBuf, String> {
    if response.workspace_id != handle.meta.workspace_id
        || response.space_id != handle.meta.space_id
    {
        return Err("Layrs Desktop received sync data for a different Space.".to_string());
    }

    let active_layer_id = handle.active.layer_id.clone();
    if response.layer_id != active_layer_id {
        return Err(format!(
            "Layrs Desktop received Layer {} while {} is active.",
            response.layer_id, active_layer_id
        ));
    }
    let response_uses_v2 = response.protocol.as_deref() == Some(SYNC_PROTOCOL_V2)
        || response.content_objects.is_some();
    if !response_uses_v2 {
        return Err("Layrs Desktop requires layrs.sync.v2 receive data.".to_string());
    }
    let policy_by_layer = response
        .access_registries
        .iter()
        .filter_map(|policy| {
            policy
                .get("layer_id")
                .or_else(|| policy.get("layerId"))
                .and_then(Value::as_str)
                .map(|layer_id| (layer_id.to_string(), policy.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let content_objects = response
        .content_objects
        .as_ref()
        .ok_or_else(|| "Layrs Desktop received V2 protocol without contentObjects.".to_string())?;
    ensure_received_v2_chunks(
        &handle.layrs_dir,
        endpoint,
        token,
        &response.workspace_id,
        &response.space_id,
        &content_objects.chunks,
    )?;
    write_received_v2_file_objects(&handle.layrs_dir, &content_objects.file_objects)?;
    write_received_v2_tree_objects(&handle.layrs_dir, content_objects)?;

    let mut metadata_layers = Vec::with_capacity(response.layers.len());
    let mut active_sync_path =
        layer_dir(&handle.layrs_dir, &active_layer_id).join("sync-state.json");

    for layer in &response.layers {
        let layer_id = layer.id.clone();
        let access_kind = match layer.access.as_deref() {
            Some("redacted" | "denied" | "restricted" | "no-access") => LayerAccessKind::Redacted,
            _ => LayerAccessKind::Open,
        };
        let layer_meta = LocalLayerMetadata {
            layer_id: layer_id.clone(),
            display_name: layer.name.clone(),
            parent_layer_id: layer.parent_layer_id.clone(),
            access: access_kind.clone(),
            can_open: access_kind == LayerAccessKind::Open,
        };
        metadata_layers.push(layer_meta.clone());

        let view = LayerAccessView {
            layer_id: layer_id.clone(),
            workspace_id: layer
                .workspace_id
                .clone()
                .unwrap_or_else(|| response.workspace_id.clone()),
            space_id: layer
                .space_id
                .clone()
                .unwrap_or_else(|| response.space_id.clone()),
            display_name: layer.name.clone(),
            access: access_kind,
            can_open: layer_meta.can_open,
            local_path: Some(
                layer_dir(&handle.layrs_dir, &layer_id)
                    .display()
                    .to_string(),
            ),
            reason: if layer_meta.can_open {
                None
            } else {
                Some("Layer metadata is redacted by Studio access policy.".to_string())
            },
        };
        scaffold_layer(&handle.layrs_dir, &handle.meta.local_space_id, &view)?;
        if let Some(policy) = policy_by_layer.get(&layer_id) {
            write_received_access_file(handle, &layer_id, &layer.name, policy)?;
        }

        let layer_root_tree_id = if layer_id == active_layer_id {
            response.root_tree_id.as_deref()
        } else {
            None
        };
        let state = received_v2_state(
            &handle.layrs_dir,
            endpoint,
            token,
            &response.workspace_id,
            &response.space_id,
            &layer_id,
            layer_root_tree_id,
            content_objects,
        )?;
        if let Some(state) = state {
            write_layer_state(&handle.layrs_dir, &layer_id, &state)?;
            if materialize_active && layer_id == active_layer_id {
                materialize_state(&handle.root, &state)?;
            }
        } else if layer_id == active_layer_id {
            return Err(format!(
                "Layrs Desktop received V2 content without a tree for active Layer {layer_id}."
            ));
        }

        let sync_path = write_received_sync_state(
            handle,
            &layer_id,
            response.cursor.clone(),
            layer.parent_layer_id.as_deref(),
        )?;
        if layer_id == active_layer_id {
            active_sync_path = sync_path;
        }
        write_received_timeline_cache(handle, &layer_id, &response.timeline)?;
    }
    write_received_steps(&handle.layrs_dir, &response.steps)?;

    if !metadata_layers
        .iter()
        .any(|layer| layer.layer_id == active_layer_id)
    {
        return Err(format!(
            "Studio did not return active Layer {active_layer_id} for this Local Space."
        ));
    }

    handle.meta.layers = metadata_layers;
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(handle)?;
    remember_local_space(&handle.meta, Some(active_layer_id))?;
    Ok(active_sync_path)
}

fn write_received_access_file(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    display_name: &str,
    policy: &Value,
) -> Result<(), String> {
    let rules = policy
        .get("rules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .map(|rule| LayerAccessRuleFile {
                    id: rule
                        .get("id")
                        .or_else(|| rule.get("rule_id"))
                        .and_then(Value::as_str)
                        .unwrap_or("access_rule")
                        .to_string(),
                    path: rule
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("*")
                        .to_string(),
                    mode: rule
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("restricted")
                        .to_string(),
                    visibility: rule
                        .get("visibility")
                        .and_then(Value::as_str)
                        .unwrap_or("stub")
                        .to_string(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let access_file = LayerAccessFile {
        schema: LAYER_ACCESS_SCHEMA.to_string(),
        local_space_id: handle.meta.local_space_id.clone(),
        workspace_id: handle.meta.workspace_id.clone(),
        space_id: handle.meta.space_id.clone(),
        layer_id: layer_id.to_string(),
        display_name: display_name.to_string(),
        access: LayerAccessKind::Open,
        can_open: true,
        reason: None,
        policy_epoch: policy
            .get("policy_epoch")
            .or_else(|| policy.get("policyEpoch"))
            .and_then(Value::as_u64)
            .unwrap_or(1),
        generated_at_unix: unix_now(),
        rules,
    };
    write_json(
        &layer_dir(&handle.layrs_dir, layer_id).join("access.json"),
        &access_file,
    )
}

fn received_v2_state(
    layrs_dir: &Path,
    endpoint: Option<&str>,
    token: Option<&str>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    root_tree_id: Option<&str>,
    objects: &ReceivedContentObjects,
) -> Result<Option<WorkingStateFile>, String> {
    ensure_received_v2_chunks(
        layrs_dir,
        endpoint,
        token,
        workspace_id,
        space_id,
        &objects.chunks,
    )?;
    write_received_v2_file_objects(layrs_dir, &objects.file_objects)?;

    let tree = select_received_tree(layer_id, root_tree_id, &objects.tree_objects);
    let Some(tree) = tree else {
        return Ok(None);
    };
    let file_objects = objects
        .file_objects
        .iter()
        .map(|object| (object.file_object_id.clone(), object))
        .collect::<BTreeMap<_, _>>();
    let mut files = Vec::with_capacity(tree.entries.len());
    for entry in &tree.entries {
        validate_snapshot_key(&entry.path)?;
        let file_object_id = entry.file_object_id.clone().ok_or_else(|| {
            format!(
                "Layrs Desktop received V2 tree entry {} without fileObjectId.",
                entry.path
            )
        })?;
        validate_blake3_id(&file_object_id)?;
        let file_size = entry
            .size
            .or_else(|| {
                file_objects
                    .get(&file_object_id)
                    .and_then(|object| object.size)
            })
            .ok_or_else(|| {
                format!(
                    "Layrs Desktop received V2 tree entry {} without size.",
                    entry.path
                )
            })?;
        files.push(FileSnapshotEntry {
            path: entry.path.clone(),
            object: format!("objects/files/{}.json", object_file_stem(&file_object_id)),
            hash: file_object_id,
            size: file_size,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let tree_object = TreeObjectFile {
        schema: TREE_OBJECT_SCHEMA.to_string(),
        tree_id: tree.tree_id.clone(),
        files: files.clone(),
    };
    write_json(&tree_object_path(layrs_dir, &tree.tree_id), &tree_object)?;
    Ok(Some(WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: layer_id.to_string(),
        captured_at_unix: unix_now(),
        root_tree_id: Some(tree.tree_id.clone()),
        files,
    }))
}

fn write_received_v2_tree_objects(
    layrs_dir: &Path,
    objects: &ReceivedContentObjects,
) -> Result<(), String> {
    let file_objects = objects
        .file_objects
        .iter()
        .map(|object| (object.file_object_id.clone(), object))
        .collect::<BTreeMap<_, _>>();
    for tree in &objects.tree_objects {
        validate_blake3_id(&tree.tree_id)?;
        let mut files = Vec::with_capacity(tree.entries.len());
        for entry in &tree.entries {
            validate_snapshot_key(&entry.path)?;
            let file_object_id = entry.file_object_id.clone().ok_or_else(|| {
                format!(
                    "Layrs Desktop received V2 tree entry {} without fileObjectId.",
                    entry.path
                )
            })?;
            validate_blake3_id(&file_object_id)?;
            let file_size = entry
                .size
                .or_else(|| {
                    file_objects
                        .get(&file_object_id)
                        .and_then(|object| object.size)
                })
                .ok_or_else(|| {
                    format!(
                        "Layrs Desktop received V2 tree entry {} without size.",
                        entry.path
                    )
                })?;
            files.push(FileSnapshotEntry {
                path: entry.path.clone(),
                object: format!("objects/files/{}.json", object_file_stem(&file_object_id)),
                hash: file_object_id,
                size: file_size,
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        write_json(
            &tree_object_path(layrs_dir, &tree.tree_id),
            &TreeObjectFile {
                schema: TREE_OBJECT_SCHEMA.to_string(),
                tree_id: tree.tree_id.clone(),
                files,
            },
        )?;
    }
    Ok(())
}

fn write_received_steps(layrs_dir: &Path, steps: &[ReceivedStep]) -> Result<(), String> {
    for step in steps {
        validate_step_file_id(&step.step_id)?;
        let layer_id = step.layer_id.trim();
        if layer_id.is_empty() {
            return Err("Layrs Desktop received a step without layerId.".to_string());
        }
        if let Some(root_tree_id) = step.root_tree_id.as_deref() {
            validate_blake3_id(root_tree_id)?;
        }
        if let Some(base_tree_id) = step.base_tree_id.as_deref() {
            validate_blake3_id(base_tree_id)?;
        }
        let step_file = StepFile {
            schema: STEP_SCHEMA.to_string(),
            step_id: step.step_id.clone(),
            layer_id: layer_id.to_string(),
            parent_step_id: step.parent_step_id.clone(),
            base_layer_id: step.base_layer_id.clone(),
            base_tree_id: step.base_tree_id.clone(),
            root_tree_id: step.root_tree_id.clone(),
            changed_paths: step.changed_paths.clone(),
            captured_at_unix: step.captured_at_unix.unwrap_or_else(unix_now),
            files: Vec::new(),
        };
        write_json(
            &layer_dir(layrs_dir, layer_id)
                .join("steps")
                .join(format!("{}.json", step.step_id)),
            &step_file,
        )?;
    }
    Ok(())
}

fn validate_step_file_id(step_id: &str) -> Result<(), String> {
    let valid = !step_id.trim().is_empty()
        && step_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Layrs Desktop received an invalid stepId: {step_id}"
        ))
    }
}

fn select_received_tree<'a>(
    layer_id: &str,
    root_tree_id: Option<&str>,
    trees: &'a [ReceivedTreeObject],
) -> Option<&'a ReceivedTreeObject> {
    root_tree_id
        .and_then(|tree_id| trees.iter().find(|tree| tree.tree_id == tree_id))
        .or_else(|| {
            trees
                .iter()
                .find(|tree| tree.layer_id.as_deref() == Some(layer_id))
        })
        .or_else(|| (trees.len() == 1).then(|| &trees[0]))
}

fn ensure_received_v2_chunks(
    layrs_dir: &Path,
    endpoint: Option<&str>,
    token: Option<&str>,
    workspace_id: &str,
    space_id: &str,
    chunks: &[ReceivedChunkObject],
) -> Result<(), String> {
    for chunk in chunks {
        validate_blake3_id(&chunk.chunk_id)?;
        if let Some(digest) = chunk.digest.as_deref() {
            validate_blake3_id(digest)?;
            if digest != chunk.chunk_id {
                return Err(format!(
                    "Layrs Desktop rejected V2 chunk {} because its digest differs from chunkId.",
                    chunk.chunk_id
                ));
            }
        }
        let expected_size = chunk.size.or(chunk.size_bytes);
        let chunk_path = layrs_dir
            .join("objects")
            .join("chunks")
            .join(format!("{}.chunk", object_file_stem(&chunk.chunk_id)));
        if chunk_path.exists() {
            let bytes = fs::read(&chunk_path).map_err(|error| {
                format!(
                    "Layrs Desktop could not read local received chunk {}: {error}",
                    chunk_path.display()
                )
            })?;
            if blake3_id(&bytes) != chunk.chunk_id {
                return Err(format!(
                    "Layrs Desktop rejected local V2 chunk {} because its hash does not match.",
                    chunk.chunk_id
                ));
            }
            if let Some(expected_size) = expected_size {
                if bytes.len() as u64 != expected_size {
                    return Err(format!(
                        "Layrs Desktop rejected local V2 chunk {} because its size does not match.",
                        chunk.chunk_id
                    ));
                }
            }
            continue;
        }

        let endpoint = endpoint.ok_or_else(|| {
            format!(
                "Layrs Desktop received V2 chunk {} without local bytes and no server endpoint.",
                chunk.chunk_id
            )
        })?;
        let download_path = chunk.download_url.clone().unwrap_or_else(|| {
            format!(
                "/v1/workspaces/{}/spaces/{}/chunks/{}",
                url_path_segment(workspace_id),
                url_path_segment(space_id),
                url_path_segment(&chunk.chunk_id)
            )
        });
        let bytes = get_bytes(endpoint, &download_path, token)?;
        if blake3_id(&bytes) != chunk.chunk_id {
            return Err(format!(
                "Layrs Desktop rejected V2 chunk {} because its hash does not match.",
                chunk.chunk_id
            ));
        }
        if let Some(expected_size) = expected_size {
            if bytes.len() as u64 != expected_size {
                return Err(format!(
                    "Layrs Desktop rejected V2 chunk {} because its size does not match.",
                    chunk.chunk_id
                ));
            }
        }
        if let Some(parent) = chunk_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Layrs Desktop could not create received chunk directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&chunk_path, bytes).map_err(|error| {
            format!(
                "Layrs Desktop could not write received V2 chunk {}: {error}",
                chunk_path.display()
            )
        })?;
    }
    Ok(())
}

fn write_received_v2_file_objects(
    layrs_dir: &Path,
    file_objects: &[ReceivedFileObject],
) -> Result<(), String> {
    for object in file_objects {
        validate_blake3_id(&object.file_object_id)?;
        if let Some(hash) = object.hash.as_deref() {
            validate_blake3_id(hash)?;
            if hash != object.file_object_id {
                return Err(format!(
                    "Layrs Desktop rejected V2 fileObject {} because hash differs from fileObjectId.",
                    object.file_object_id
                ));
            }
        }
        let chunks = object
            .chunks
            .iter()
            .map(|chunk| {
                validate_blake3_id(&chunk.chunk_id)?;
                Ok(FileChunkRef {
                    chunk_id: chunk.chunk_id.clone(),
                    size: chunk.size.or(chunk.size_bytes).unwrap_or(0),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let size = object
            .size
            .unwrap_or_else(|| chunks.iter().map(|chunk| chunk.size).sum());
        let manifest = FileObjectFile {
            schema: FILE_OBJECT_SCHEMA.to_string(),
            hash: object
                .hash
                .clone()
                .unwrap_or_else(|| object.file_object_id.clone()),
            size,
            chunks,
        };
        write_json(
            &layrs_dir
                .join("objects")
                .join("files")
                .join(format!("{}.json", object_file_stem(&object.file_object_id))),
            &manifest,
        )?;
    }
    Ok(())
}

fn write_received_sync_state(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    server_cursor: Option<String>,
    parent_layer_id: Option<&str>,
) -> Result<PathBuf, String> {
    let path = layer_dir(&handle.layrs_dir, layer_id).join("sync-state.json");
    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": layer_id,
        "parentLayerId": parent_layer_id,
        "lastManualOperation": "receive",
        "lastManualOperationUnix": unix_now(),
        "lastReceiveUnix": unix_now(),
        "serverCursor": server_cursor,
        "pending": false,
        "status": "received"
    });
    write_json(&path, &sync_state)?;
    Ok(path)
}

fn write_received_timeline_cache(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    timeline: &[Value],
) -> Result<(), String> {
    let items = timeline
        .iter()
        .filter(|event| {
            event
                .get("layerId")
                .or_else(|| event.get("layer_id"))
                .and_then(Value::as_str)
                == Some(layer_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    write_json(
        &layer_dir(&handle.layrs_dir, layer_id).join("timeline-cache.json"),
        &serde_json::json!({ "schema": "layrs.timeline_cache.v1", "items": items }),
    )
}

fn build_publish_v2_request(
    handle: &LocalSpaceHandle,
    config: &DesktopConfig,
    layer_id: &str,
    base_tree_id: Option<String>,
    state: &WorkingStateFile,
    changed_paths: Vec<String>,
    deleted_paths: Vec<String>,
    step: Option<&StepFile>,
) -> Result<PublishLayerRequest, String> {
    let publish_paths = changed_paths
        .iter()
        .filter(|path| !deleted_paths.contains(path))
        .cloned()
        .collect::<BTreeSet<_>>();
    Ok(PublishLayerRequest {
        layer_id: layer_id.to_string(),
        protocol: SYNC_PROTOCOL_V2.to_string(),
        policy_epoch: layer_policy_epoch(handle, layer_id),
        idempotency_key: publish_idempotency_key(layer_id, state.root_tree_id.as_deref()),
        source_client_id: config.device_id.clone(),
        base_tree_id,
        root_tree_id: state.root_tree_id.clone(),
        changed_paths,
        store_objects: publish_store_objects_for_paths(handle, state, &publish_paths)?,
        artifacts: Vec::new(),
        deleted_paths,
        step: step.map(PublishStepRequest::from_step),
    })
}

fn upload_publish_chunks(
    handle: &LocalSpaceHandle,
    endpoint: &str,
    token: Option<&str>,
    workspace_id: &str,
    space_id: &str,
    store_objects: &PublishStoreObjectsRequest,
) -> Result<(), String> {
    let chunks_by_id = store_objects
        .chunks
        .iter()
        .map(|chunk| (chunk.chunk_id.clone(), chunk))
        .collect::<BTreeMap<_, _>>();
    if chunks_by_id.is_empty() {
        return Ok(());
    }

    let prepare_path = format!(
        "/v1/workspaces/{}/spaces/{}/chunks/prepare",
        url_path_segment(workspace_id),
        url_path_segment(space_id)
    );
    let prepare = PrepareChunkUploadRequest {
        chunks: chunks_by_id
            .values()
            .map(|chunk| PrepareChunkUploadItem {
                chunk_id: chunk.chunk_id.clone(),
                size_bytes: chunk.size,
            })
            .collect(),
    };
    let prepared: PrepareChunkUploadResponse = post_json(endpoint, &prepare_path, token, &prepare)?;

    for item in prepared.items {
        if !item.upload_required {
            continue;
        }
        let chunk = chunks_by_id.get(&item.chunk_id).ok_or_else(|| {
            format!(
                "Layrs Desktop could not match prepared upload chunk {} to the publish manifest.",
                item.chunk_id
            )
        })?;
        let bytes = read_local_chunk_object_bytes(handle, &chunk.chunk_id)?;
        if bytes.len() as u64 != chunk.size {
            return Err(format!(
                "Layrs Desktop chunk {} has size {}, expected {}.",
                chunk.chunk_id,
                bytes.len(),
                chunk.size
            ));
        }
        let actual = blake3_id(&bytes);
        if actual != chunk.chunk_id || actual != chunk.digest {
            return Err(format!(
                "Layrs Desktop chunk {} failed local hash verification before upload.",
                chunk.chunk_id
            ));
        }
        let upload_path = item.upload_url.unwrap_or_else(|| {
            format!(
                "/v1/workspaces/{}/spaces/{}/chunks/{}",
                url_path_segment(workspace_id),
                url_path_segment(space_id),
                url_path_segment(&chunk.chunk_id)
            )
        });
        let _: Value = put_bytes_json(endpoint, &upload_path, token, &bytes)?;
    }

    Ok(())
}

fn read_local_chunk_object_bytes(
    handle: &LocalSpaceHandle,
    chunk_id: &str,
) -> Result<Vec<u8>, String> {
    validate_blake3_id(chunk_id)?;
    let chunk_path = handle
        .layrs_dir
        .join("objects")
        .join("chunks")
        .join(format!("{}.chunk", object_file_stem(chunk_id)));
    fs::read(&chunk_path).map_err(|error| {
        format!(
            "Layrs Desktop could not read publish chunk {}: {error}",
            chunk_path.display()
        )
    })
}

fn layer_policy_epoch(handle: &LocalSpaceHandle, layer_id: &str) -> u64 {
    read_json::<LayerAccessFile>(&layer_dir(&handle.layrs_dir, layer_id).join("access.json"))
        .map(|access| access.policy_epoch)
        .unwrap_or(1)
}

fn publish_idempotency_key(layer_id: &str, root_tree_id: Option<&str>) -> String {
    let mut material = layer_id.as_bytes().to_vec();
    material.push(0);
    material.extend_from_slice(root_tree_id.unwrap_or("none").as_bytes());
    format!("publish-{}", object_file_stem(&blake3_id(&material)))
}

fn publish_store_objects_for_paths(
    handle: &LocalSpaceHandle,
    state: &WorkingStateFile,
    paths: &BTreeSet<String>,
) -> Result<PublishStoreObjectsRequest, String> {
    let mut objects = PublishStoreObjectsRequest::default();
    if let Some(root_tree_id) = state.root_tree_id.clone() {
        objects.tree_objects.push(PublishTreeObjectRequest {
            tree_id: root_tree_id,
            entries: state
                .files
                .iter()
                .map(|file| PublishTreeEntryRequest {
                    path: file.path.clone(),
                    file_object_id: file.hash.clone(),
                    size: file.size,
                })
                .collect(),
        });
    }

    for file in &state.files {
        if !paths.contains(&file.path) {
            continue;
        }
        let (file_object, chunks) = publish_file_object_for_file(handle, file)?;
        objects.file_objects.push(file_object);
        objects.chunks.extend(chunks);
    }
    Ok(objects)
}

fn publish_file_object_for_file(
    handle: &LocalSpaceHandle,
    file: &FileSnapshotEntry,
) -> Result<(PublishFileObjectRequest, Vec<PublishChunkObjectRequest>), String> {
    let chunks = publish_chunks_for_file(handle, file)?;
    let refs = chunks
        .iter()
        .map(|chunk| PublishChunkRefRequest {
            chunk_id: chunk.chunk_id.clone(),
            size: chunk.size,
        })
        .collect();
    Ok((
        PublishFileObjectRequest {
            file_object_id: file.hash.clone(),
            size: file.size,
            chunks: refs,
        },
        chunks,
    ))
}

fn publish_chunks_for_file(
    handle: &LocalSpaceHandle,
    file: &FileSnapshotEntry,
) -> Result<Vec<PublishChunkObjectRequest>, String> {
    if file.object.starts_with("objects/files/") {
        let manifest = read_json::<FileObjectFile>(&handle.layrs_dir.join(&file.object))?;
        let mut chunks = Vec::with_capacity(manifest.chunks.len());
        for chunk in manifest.chunks {
            chunks.push(PublishChunkObjectRequest {
                digest: chunk.chunk_id.clone(),
                chunk_id: chunk.chunk_id,
                size: chunk.size,
            });
        }
        return Ok(chunks);
    }

    let bytes = read_snapshot_object_bytes(&handle.layrs_dir, file)?;
    let mut chunks = Vec::new();
    for chunk in bytes.chunks(CHUNK_SIZE) {
        let chunk_id = blake3_id(chunk);
        let chunk_path = handle
            .layrs_dir
            .join("objects")
            .join("chunks")
            .join(format!("{}.chunk", object_file_stem(&chunk_id)));
        if !chunk_path.exists() {
            if let Some(parent) = chunk_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "Layrs Desktop could not create chunk directory {}: {error}",
                        parent.display()
                    )
                })?;
            }
            fs::write(&chunk_path, chunk).map_err(|error| {
                format!(
                    "Layrs Desktop could not write publish chunk {}: {error}",
                    chunk_path.display()
                )
            })?;
        }
        chunks.push(PublishChunkObjectRequest {
            digest: chunk_id.clone(),
            chunk_id,
            size: chunk.len() as u64,
        });
    }
    Ok(chunks)
}

fn apply_server_mapping_to_draft(
    handle: &mut LocalSpaceHandle,
    created: &CreateSpaceFromLocalResponse,
) -> Result<(), String> {
    let mapping_by_local = created
        .layer_mappings
        .iter()
        .map(|mapping| {
            (
                mapping.local_layer_id.clone(),
                mapping.server_layer_id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let old_active_layer = handle.active.layer_id.clone();
    for mapping in &created.layer_mappings {
        let old_dir = layer_dir(&handle.layrs_dir, &mapping.local_layer_id);
        let new_dir = layer_dir(&handle.layrs_dir, &mapping.server_layer_id);
        if old_dir != new_dir && old_dir.exists() && !new_dir.exists() {
            fs::rename(&old_dir, &new_dir).map_err(|error| {
                format!(
                    "Layrs Desktop could not map local Layer {} to server Layer {}: {error}",
                    mapping.local_layer_id, mapping.server_layer_id
                )
            })?;
        }

        rewrite_layer_files_after_mapping(
            &handle.layrs_dir,
            &mapping.local_layer_id,
            &mapping.server_layer_id,
            &created.space.workspace_id,
            &created.space.id,
        )?;
    }

    for layer in &mut handle.meta.layers {
        if let Some(server_layer_id) = mapping_by_local.get(&layer.layer_id) {
            layer.parent_layer_id = layer
                .parent_layer_id
                .as_ref()
                .and_then(|parent| mapping_by_local.get(parent).cloned())
                .or_else(|| layer.parent_layer_id.clone());
            layer.layer_id = server_layer_id.clone();
        }
    }
    handle.meta.state = LOCAL_SPACE_STATE_LINKED.to_string();
    handle.meta.workspace_id = created.space.workspace_id.clone();
    handle.meta.server_space_id = Some(created.space.id.clone());
    handle.meta.space_id = created.space.id.clone();
    handle.meta.updated_at_unix = unix_now();

    if let Some(server_active_layer) = mapping_by_local.get(&old_active_layer) {
        handle.active.layer_id = server_active_layer.clone();
    } else if let Some(server_active_layer) = created.space.current_layer_id.clone() {
        handle.active.layer_id = server_active_layer;
    }
    handle.active.updated_at_unix = unix_now();

    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(handle)?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))
}

fn rewrite_layer_files_after_mapping(
    layrs_dir: &Path,
    old_layer_id: &str,
    server_layer_id: &str,
    workspace_id: &str,
    space_id: &str,
) -> Result<(), String> {
    let dir = layer_dir(layrs_dir, server_layer_id);
    for file_name in ["index.json", "working-state.json"] {
        let path = dir.join(file_name);
        if path.exists() {
            let mut state = read_state_file(layrs_dir, &path)?;
            state.layer_id = server_layer_id.to_string();
            let state = storage_state(layrs_dir, &state)?;
            write_json(&path, &state)?;
        }
    }

    let access_path = dir.join("access.json");
    if access_path.exists() {
        let mut access = read_json::<LayerAccessFile>(&access_path)?;
        access.workspace_id = workspace_id.to_string();
        access.space_id = space_id.to_string();
        access.layer_id = server_layer_id.to_string();
        write_json(&access_path, &access)?;
    }

    let sync_path = dir.join("sync-state.json");
    write_json(
        &sync_path,
        &serde_json::json!({
            "schema": SYNC_STATE_SCHEMA,
            "layerId": server_layer_id,
            "previousLocalLayerId": old_layer_id,
            "lastPublishUnix": unix_now(),
            "pending": false,
            "status": "linked"
        }),
    )
}

fn media_type_for_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".json") {
        "application/json"
    } else {
        "text/plain"
    }
}

fn is_text_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".log")
        || lower.ends_with(".ini")
        || lower.ends_with(".env")
}

fn is_code_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".json")
        || lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".rs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".css")
        || lower.ends_with(".html")
        || lower.ends_with(".csv")
}

fn lens_id_for_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".svg")
    {
        "layrs.image"
    } else if is_code_path(path) {
        "layrs.code"
    } else if is_text_path(path) {
        "layrs.text"
    } else {
        "layrs.raw"
    }
}

fn url_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn remember_local_space(
    meta: &LocalSpaceFile,
    active_layer_id: Option<String>,
) -> Result<(), String> {
    let mut config = DesktopConfig::load_or_create()?;
    config.remember_local_space(LocalSpaceConfigEntry {
        local_space_id: meta.local_space_id.clone(),
        space_id: meta.space_id.clone(),
        root_path: meta.root_path.clone(),
        active_layer_id,
        updated_at_unix: unix_now(),
    });
    config.save()
}

fn remove_local_space_config_entry(local_space_id: &str, root: &Path) -> Result<(), String> {
    let mut config = DesktopConfig::load_or_create()?;
    let root_key = path_compare_key(root);
    config.local_spaces.retain(|entry| {
        entry.local_space_id != local_space_id
            && path_compare_key(&PathBuf::from(&entry.root_path)) != root_key
    });
    config.save()
}

fn layer_dir(layrs_dir: &Path, layer_id: &str) -> PathBuf {
    layrs_dir.join("layers").join(safe_id_fragment(layer_id))
}

fn unique_layer_id(handle: &LocalSpaceHandle, name: &str) -> String {
    let base = safe_id_fragment(name);
    let existing = handle
        .meta
        .layers
        .iter()
        .map(|layer| layer.layer_id.clone())
        .collect::<BTreeSet<_>>();
    let mut candidate = format!("{base}-{}", unix_now());
    let mut index = 2;
    while existing.contains(&candidate) {
        candidate = format!("{base}-{}-{index}", unix_now());
        index += 1;
    }
    candidate
}

fn safe_layer_path_key(layer_id: &str) -> Option<String> {
    if layer_id.is_empty()
        || layer_id == "."
        || layer_id == ".."
        || layer_id.contains('/')
        || layer_id.contains('\\')
        || layer_id.contains(':')
    {
        return None;
    }

    let safe = safe_id_fragment(layer_id);
    if safe.is_empty() || safe == "." || safe == ".." {
        None
    } else {
        Some(safe)
    }
}

fn safe_id_fragment(value: &str) -> String {
    let mut safe = String::with_capacity(value.len().max(4));
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            safe.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || matches!(ch, '/' | '\\' | ':') {
            safe.push('-');
        } else {
            safe.push('_');
        }
    }

    let safe = safe.trim_matches('-').to_string();
    if safe.is_empty() {
        "layer".to_string()
    } else {
        safe
    }
}

fn relative_key(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(root).map_err(|error| {
        format!(
            "Layrs Desktop could not create relative path for {}: {error}",
            path.display()
        )
    })?;
    Ok(relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn path_from_key(root: &Path, key: &str) -> Result<PathBuf, String> {
    validate_snapshot_key(key)?;
    let mut path = root.to_path_buf();
    for segment in key.split('/') {
        path.push(segment);
    }
    Ok(path)
}

fn validate_snapshot_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() || key.starts_with('/') || key.starts_with('\\') {
        return Err("Layrs Desktop snapshot path is invalid.".to_string());
    }
    for segment in key.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains('\\') {
            return Err(format!("Layrs Desktop snapshot path {key} is invalid."));
        }
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if path.is_absolute() {
                Ok(path.to_path_buf())
            } else {
                env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .map_err(|cwd_error| {
                        format!(
                            "Layrs Desktop could not resolve local path {}: {cwd_error}",
                            path.display()
                        )
                    })
            }
        }
        Err(error) => Err(format!(
            "Layrs Desktop could not resolve local path {}: {error}",
            path.display()
        )),
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let body = fs::read_to_string(path).map_err(|error| {
        format!(
            "Layrs Desktop could not read JSON file {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_str(&body).map_err(|error| {
        format!(
            "Layrs Desktop JSON file {} is invalid: {error}",
            path.display()
        )
    })
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Layrs Desktop could not create directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Layrs Desktop could not encode JSON: {error}"))?;
    fs::write(path, body).map_err(|error| {
        format!(
            "Layrs Desktop could not write JSON file {}: {error}",
            path.display()
        )
    })
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn blake3_id(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn object_file_stem(object_id: &str) -> &str {
    object_id.strip_prefix("blake3:").unwrap_or(object_id)
}

fn validate_blake3_id(object_id: &str) -> Result<(), String> {
    let Some(hex) = object_id.strip_prefix("blake3:") else {
        return Err(format!("Object id {object_id} is not a blake3 id."));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "Object id {object_id} is not a canonical blake3 id."
        ));
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn default_linked_state() -> String {
    LOCAL_SPACE_STATE_LINKED.to_string()
}

#[allow(dead_code)]
pub fn scaffold_access_registry(
    workspace_root_input: Option<String>,
    bootstrap: &BootstrapData,
) -> Result<AccessRegistryResult, String> {
    let root = workspace_root(workspace_root_input)?;
    let layers = access_views(&bootstrap.layers, Some(&root))?;
    let pointer_path = root.join(LAYRS_DIR).join("access.json");
    Ok(AccessRegistryResult {
        root: root.display().to_string(),
        pointer_path: pointer_path.display().to_string(),
        layers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_local_space(
        space_id: String,
        target_folder: String,
        initial_layer_id: Option<String>,
    ) -> Result<CreateLocalSpaceResult, String> {
        super::create_local_space_internal(space_id, target_folder, initial_layer_id, false)
    }

    #[test]
    fn switch_layer_restores_main_after_round_trip() {
        let root = unique_test_dir("round-trip");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "main").unwrap();

        let created = create_local_space(
            "space-a".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();

        fs::write(space.join("note.txt"), "feature").unwrap();
        let feature = create_layer_from_current(
            created.local_space.local_space_id.clone(),
            "Feature".to_string(),
        )
        .unwrap();

        switch_layer(
            created.local_space.local_space_id.clone(),
            "main".to_string(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(space.join("note.txt")).unwrap(), "main");

        switch_layer(created.local_space.local_space_id, feature.active_layer_id).unwrap();
        assert_eq!(
            fs::read_to_string(space.join("note.txt")).unwrap(),
            "feature"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn draft_local_space_creates_open_main_layer_offline() {
        let root = unique_test_dir("draft");
        let config = root.join("config");
        let space = root.join("draft-space");
        env::set_var("APPDATA", &config);

        let created =
            create_draft_local_space("Local Prototype".to_string(), space.display().to_string())
                .unwrap();

        assert_eq!(created.local_space.state, LOCAL_SPACE_STATE_DRAFT);
        assert_eq!(created.local_space.name, "Local Prototype");
        assert_eq!(created.local_space.layers.len(), 1);
        assert_eq!(created.local_space.layers[0].display_name, "Main");
        assert!(space.join(".layrs").join("local-space.json").exists());
        assert!(space
            .join(".layrs")
            .join("layers")
            .join("local_layer_main")
            .join("working-state.json")
            .exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forget_local_space_archives_layrs_and_keeps_project_files() {
        let root = unique_test_dir("forget-local");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "keep me").unwrap();

        let created = create_local_space(
            "space-forget".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        assert_eq!(list_local_spaces().unwrap().len(), 1);

        let forgotten = forget_local_space(created.local_space.local_space_id).unwrap();

        assert_eq!(
            path_compare_key(&PathBuf::from(&forgotten.root_path)),
            path_compare_key(&space)
        );
        assert!(!space.join(LAYRS_DIR).exists());
        assert!(PathBuf::from(forgotten.archived_layrs_path.unwrap()).exists());
        assert_eq!(
            fs::read_to_string(space.join("note.txt")).unwrap(),
            "keep me"
        );
        assert!(list_local_spaces().unwrap().is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forget_local_space_disconnects_when_layrs_metadata_is_missing() {
        let root = unique_test_dir("forget-missing");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "still here").unwrap();

        let created = create_local_space(
            "space-forget-missing".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::remove_dir_all(space.join(LAYRS_DIR)).unwrap();

        let forgotten = forget_local_space(created.local_space.local_space_id).unwrap();

        assert!(forgotten.archived_layrs_path.is_none());
        assert_eq!(
            fs::read_to_string(space.join("note.txt")).unwrap(),
            "still here"
        );
        assert!(list_local_spaces().unwrap().is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_added_text_file_returns_lens_diff_entry() {
        let root = unique_test_dir("added-diff");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);

        let created = create_local_space(
            "space-diff".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "hello from desktop\n").unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

        assert_eq!(scan.added, vec!["note.txt".to_string()]);
        assert_eq!(scan.diffs.len(), 1);
        assert_eq!(scan.diffs[0].lens_id, "layrs.text");
        assert_eq!(scan.diffs[0].diff.kind, "textLines");
        assert!(scan.diffs[0].diff.hunks[0]
            .lines
            .iter()
            .any(|line| line.op == "insert" && line.text == "hello from desktop"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_modified_text_file_returns_unified_lines() {
        let root = unique_test_dir("modified-diff");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "alpha\nbeta\nomega\n").unwrap();

        let created = create_local_space(
            "space-modified-diff".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "alpha\nbravo\nomega\n").unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        let lines = &scan.diffs[0].diff.hunks[0].lines;

        assert_eq!(scan.modified, vec!["note.txt".to_string()]);
        assert_eq!(lines[0].op, "equal");
        assert_eq!(lines[0].old_line, Some(1));
        assert_eq!(lines[0].new_line, Some(1));
        assert_eq!(lines[1].op, "delete");
        assert_eq!(lines[1].old_line, Some(2));
        assert_eq!(lines[2].op, "insert");
        assert_eq!(lines[2].new_line, Some(2));
        assert_eq!(lines[3].op, "equal");
        assert_eq!(lines[3].old_line, Some(3));
        assert_eq!(lines[3].new_line, Some(3));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_modified_large_text_file_preserves_long_diff_lines() {
        let root = unique_test_dir("modified-large-diff");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("test.txt"), "tiny\n").unwrap();

        let created = create_local_space(
            "space-large-modified-diff".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("test.txt"), "a".repeat(4 * 1024 * 1024)).unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        let diff = &scan.diffs[0];
        let lines = &diff.diff.hunks[0].lines;

        assert_eq!(scan.modified, vec!["test.txt".to_string()]);
        assert_ne!(diff.diff.summary, "No text changes");
        assert!(diff.diff.summary.contains("1 additions"));
        assert!(diff.message.is_none());
        assert!(lines.iter().any(|line| line.op == "insert"));
        let inserted = lines.iter().find(|line| line.op == "insert").unwrap();
        assert_eq!(inserted.text.chars().count(), 4 * 1024 * 1024);
        assert!(!inserted.text.contains("Layrs line truncated"));
        assert_eq!(diff.diff.fields.get("newTruncated"), Some(&Value::Bool(false)));
        assert!(diff.diff.fields.get("lineTextTruncated").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_large_text_change_after_preview_still_reports_change() {
        let root = unique_test_dir("large-diff-after-preview");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        let prefix = "a".repeat(512 * 1024);
        fs::write(space.join("test.txt"), format!("{prefix}x")).unwrap();

        let created = create_local_space(
            "space-large-tail-diff".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("test.txt"), format!("{prefix}y")).unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        let diff = &scan.diffs[0].diff;

        assert_eq!(scan.modified, vec!["test.txt".to_string()]);
        assert_ne!(diff.summary, "No text changes");
        assert!(diff.hunks[0].lines.iter().any(|line| line.op == "delete"));
        assert!(diff.hunks[0].lines.iter().any(|line| line.op == "insert"));
        assert!(diff.fields.get("lineTextTruncated").is_none());
        assert!(diff.hunks[0]
            .lines
            .iter()
            .filter(|line| line.op == "delete" || line.op == "insert")
            .all(|line| !line.text.contains("Layrs line truncated")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_large_added_text_file_returns_window_metadata() {
        let root = unique_test_dir("large-added-window");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);

        let created = create_local_space(
            "space-large-added-window".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("large.txt"), numbered_lines("line", 20_000)).unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        let diff = &scan.diffs[0].diff;

        assert_eq!(scan.added, vec!["large.txt".to_string()]);
        assert_eq!(diff.hunks[0].lines.len(), TEXT_DIFF_DEFAULT_WINDOW_LIMIT);
        assert_eq!(
            diff.fields.get("totalNewLines").and_then(Value::as_u64),
            Some(20_001)
        );
        assert_eq!(
            diff.fields.get("windowStart").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            diff.fields.get("hasMore").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            diff.fields.get("largeDiff").and_then(Value::as_bool),
            Some(true)
        );
        assert!(diff.summary.contains("20001 additions"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_large_modified_text_file_returns_window_metadata() {
        let root = unique_test_dir("large-modified-window");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("large.txt"), numbered_lines("old", 50_000)).unwrap();

        let created = create_local_space(
            "space-large-modified-window".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("large.txt"), numbered_lines("new", 50_000)).unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        let diff = &scan.diffs[0].diff;

        assert_eq!(scan.modified, vec!["large.txt".to_string()]);
        assert_eq!(diff.hunks[0].lines.len(), TEXT_DIFF_DEFAULT_WINDOW_LIMIT);
        assert_eq!(
            diff.fields.get("totalOldLines").and_then(Value::as_u64),
            Some(50_001)
        );
        assert_eq!(
            diff.fields.get("totalNewLines").and_then(Value::as_u64),
            Some(50_001)
        );
        assert_eq!(
            diff.fields.get("hasMore").and_then(Value::as_bool),
            Some(true)
        );
        assert_ne!(diff.summary, "No text changes");
        assert!(diff.summary.contains("50000 additions"));
        assert!(diff.summary.contains("50000 deletions"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_diff_window_returns_requested_window() {
        let root = unique_test_dir("load-window");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("large.txt"), numbered_lines("old", 20_000)).unwrap();

        let created = create_local_space(
            "space-load-window".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("large.txt"), numbered_lines("new", 20_000)).unwrap();

        let diff = load_diff_window(
            created.local_space.local_space_id,
            "large.txt".to_string(),
            Some("workingTree".to_string()),
            500,
            25,
        )
        .unwrap();

        assert_eq!(diff.path, "large.txt");
        assert_eq!(diff.state, "modified");
        assert_eq!(diff.diff.hunks[0].lines.len(), 25);
        assert_eq!(
            diff.diff.fields.get("windowStart").and_then(Value::as_u64),
            Some(500)
        );
        assert_eq!(
            diff.diff.fields.get("windowLimit").and_then(Value::as_u64),
            Some(25)
        );
        assert_eq!(
            diff.diff.fields.get("preview").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            diff.diff.fields.get("source").and_then(Value::as_str),
            Some("workingTree")
        );
        assert!(diff.diff.hunks[0]
            .lines
            .iter()
            .all(|line| line.op == "delete"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_includes_local_step_summaries() {
        let root = unique_test_dir("step-summary");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "base\n").unwrap();

        let created = create_local_space(
            "space-step-summary".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "step\n").unwrap();
        let state = capture_working_state(&space, "main", true).unwrap();
        write_step(&space.join(LAYRS_DIR), "main", &state).unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

        assert_eq!(scan.steps.len(), 1);
        assert_eq!(scan.steps[0].layer_id, "main");
        assert_eq!(scan.steps[0].changed_files, 1);
        assert_eq!(scan.steps[0].diff_stats.files, 1);
        assert_eq!(scan.steps[0].diffs[0].path, "note.txt");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn step_summary_uses_recorded_base_after_index_advances() {
        let root = unique_test_dir("step-base-after-publish");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);

        let created = create_local_space(
            "space-step-after-publish".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "published\n").unwrap();
        let state = capture_working_state(&space, "main", true).unwrap();
        write_step(&space.join(LAYRS_DIR), "main", &state).unwrap();
        write_layer_state(&space.join(LAYRS_DIR), "main", &state).unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

        assert_eq!(scan.steps.len(), 1);
        assert_eq!(scan.steps[0].changed_files, 1);
        assert_eq!(scan.steps[0].diff_stats.files, 1);
        assert!(scan.steps[0].diff_stats.additions > 0);
        assert_eq!(scan.steps[0].diff_stats.deletions, 0);
        assert_eq!(scan.steps[0].diffs[0].path, "note.txt");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn step_v2_persists_tree_ids_without_file_duplication() {
        let root = unique_test_dir("step-v2");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "base\n").unwrap();

        let created = create_local_space(
            "space-step-v2".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "changed\n").unwrap();
        let state = capture_working_state(&space, "main", true).unwrap();
        let step_id = write_step(&space.join(LAYRS_DIR), "main", &state).unwrap();

        let layrs_dir = space.join(LAYRS_DIR);
        let working_state: Value = read_json(
            &layrs_dir
                .join("layers")
                .join("main")
                .join("working-state.json"),
        )
        .unwrap();
        assert_eq!(working_state["schema"], WORKING_STATE_SCHEMA);
        assert!(working_state["rootTreeId"].as_str().is_some());
        assert!(working_state["rootTreeId"]
            .as_str()
            .unwrap()
            .starts_with("blake3:"));
        assert!(working_state.get("files").is_none());

        let step: Value = read_json(
            &layrs_dir
                .join("layers")
                .join("main")
                .join("steps")
                .join(format!("{step_id}.json")),
        )
        .unwrap();
        assert_eq!(step["schema"], STEP_SCHEMA);
        assert!(step["rootTreeId"].as_str().is_some());
        assert!(step["rootTreeId"].as_str().unwrap().starts_with("blake3:"));
        assert_eq!(step["changedPaths"], serde_json::json!(["note.txt"]));
        assert!(step.get("files").is_none());

        assert!(layrs_dir.join("objects").join("trees").exists());
        assert!(layrs_dir.join("objects").join("files").exists());
        assert!(layrs_dir.join("objects").join("chunks").exists());
        assert_eq!(
            scan_working_tree(created.local_space.local_space_id)
                .unwrap()
                .steps
                .len(),
            1
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_v2_payload_contains_store_objects_and_canonical_ids() {
        let root = unique_test_dir("publish-v2");
        let config_dir = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config_dir);
        fs::write(space.join("note.txt"), "base\n").unwrap();

        let created = create_local_space(
            "space-publish-v2".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
        handle.meta.workspace_id = "workspace_1".to_string();
        handle.meta.space_id = "space_1".to_string();
        write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta).unwrap();
        let base_state = read_layer_state(&handle.layrs_dir, "main").unwrap();

        fs::write(space.join("note.txt"), "changed\n").unwrap();
        let state = capture_working_state(&space, "main", true).unwrap();
        let config = DesktopConfig {
            server_endpoint: "http://127.0.0.1:8787".to_string(),
            device_id: "client_test".to_string(),
            auto_receive: false,
            auto_publish: false,
            auto_local_steps: true,
            sync_interval_seconds: 300,
            default_local_spaces_folder: root.display().to_string(),
            local_spaces: Vec::new(),
        };
        let step_id = write_step(&handle.layrs_dir, "main", &state).unwrap();
        let step = read_step_file(&handle.layrs_dir, "main", &step_id).unwrap();
        let body = build_publish_v2_request(
            &handle,
            &config,
            "main",
            base_state.root_tree_id.clone(),
            &state,
            vec!["note.txt".to_string()],
            Vec::new(),
            Some(&step),
        )
        .unwrap();
        let json = serde_json::to_value(body).unwrap();

        assert_eq!(json["protocol"], SYNC_PROTOCOL_V2);
        assert_eq!(json["layerId"], "main");
        assert_eq!(json["policyEpoch"], 1);
        assert_eq!(json["sourceClientId"], "client_test");
        assert!(json["idempotencyKey"]
            .as_str()
            .unwrap()
            .starts_with("publish-"));
        assert!(json["baseTreeId"].as_str().unwrap().starts_with("blake3:"));
        assert!(json["rootTreeId"].as_str().unwrap().starts_with("blake3:"));
        assert_eq!(json["changedPaths"], serde_json::json!(["note.txt"]));
        assert_eq!(json["deletedPaths"], serde_json::json!([]));
        assert_eq!(json["step"]["stepId"].as_str(), Some(step_id.as_str()));
        assert_eq!(
            json["step"]["changedPaths"],
            serde_json::json!(["note.txt"])
        );
        assert_eq!(
            json["step"]["rootTreeId"].as_str(),
            state.root_tree_id.as_deref()
        );
        assert!(json.get("artifacts").is_none());

        let store = &json["storeObjects"];
        assert!(store.is_object());
        let store_keys = store
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            store_keys,
            BTreeSet::from([
                "chunks".to_string(),
                "fileObjects".to_string(),
                "treeObjects".to_string()
            ])
        );
        assert_eq!(store["treeObjects"].as_array().unwrap().len(), 1);
        assert_eq!(store["fileObjects"].as_array().unwrap().len(), 1);
        assert_eq!(store["chunks"].as_array().unwrap().len(), 1);
        assert!(store["treeObjects"][0]["treeId"]
            .as_str()
            .unwrap()
            .starts_with("blake3:"));
        assert!(store["fileObjects"][0]["fileObjectId"]
            .as_str()
            .unwrap()
            .starts_with("blake3:"));
        assert!(store["chunks"][0]["chunkId"]
            .as_str()
            .unwrap()
            .starts_with("blake3:"));
        assert_eq!(store["chunks"][0]["digest"], store["chunks"][0]["chunkId"]);
        assert!(store["chunks"][0].get("data").is_none());
        assert!(store["chunks"][0].get("encoding").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn receive_v2_content_objects_materializes_chunk_bytes() {
        let root = unique_test_dir("receive-v2");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "local\n").unwrap();

        let created = create_local_space(
            "space-receive-v2".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
        let bytes = b"server bytes\n".to_vec();
        let chunk_id = blake3_id(&bytes);
        let file_object_id = blake3_id(&bytes);
        let chunk_dir = handle.layrs_dir.join("objects").join("chunks");
        fs::create_dir_all(&chunk_dir).unwrap();
        fs::write(
            chunk_dir.join(format!("{}.chunk", object_file_stem(&chunk_id))),
            &bytes,
        )
        .unwrap();
        let files = vec![FileSnapshotEntry {
            path: "note.txt".to_string(),
            object: format!("objects/files/{}.json", object_file_stem(&file_object_id)),
            hash: file_object_id.clone(),
            size: bytes.len() as u64,
        }];
        let tree_id = tree_id_for_files(&files);
        let response = ReceiveLocalSpaceResponse {
            workspace_id: handle.meta.workspace_id.clone(),
            space_id: handle.meta.space_id.clone(),
            layer_id: handle.active.layer_id.clone(),
            protocol: Some(SYNC_PROTOCOL_V2.to_string()),
            root_tree_id: Some(tree_id.clone()),
            cursor: Some("cursor_1".to_string()),
            layers: vec![
                ReceivedLayer {
                    id: handle.active.layer_id.clone(),
                    workspace_id: Some(handle.meta.workspace_id.clone()),
                    space_id: Some(handle.meta.space_id.clone()),
                    name: "Main".to_string(),
                    parent_layer_id: None,
                    access: Some("open".to_string()),
                },
                ReceivedLayer {
                    id: "layer_without_head".to_string(),
                    workspace_id: Some(handle.meta.workspace_id.clone()),
                    space_id: Some(handle.meta.space_id.clone()),
                    name: "Metadata Only".to_string(),
                    parent_layer_id: Some(handle.active.layer_id.clone()),
                    access: Some("open".to_string()),
                },
            ],
            access_registries: Vec::new(),
            content_objects: Some(ReceivedContentObjects {
                chunks: vec![ReceivedChunkObject {
                    chunk_id: chunk_id.clone(),
                    digest: Some(chunk_id.clone()),
                    download_url: None,
                    size: Some(bytes.len() as u64),
                    size_bytes: None,
                }],
                file_objects: vec![ReceivedFileObject {
                    file_object_id: file_object_id.clone(),
                    hash: Some(file_object_id.clone()),
                    size: Some(bytes.len() as u64),
                    chunks: vec![ReceivedChunkRef {
                        chunk_id,
                        size: Some(bytes.len() as u64),
                        size_bytes: None,
                    }],
                }],
                tree_objects: vec![ReceivedTreeObject {
                    tree_id: tree_id.clone(),
                    layer_id: Some(handle.active.layer_id.clone()),
                    entries: vec![ReceivedTreeEntry {
                        path: "note.txt".to_string(),
                        file_object_id: Some(file_object_id),
                        size: Some(bytes.len() as u64),
                    }],
                }],
            }),
            timeline: Vec::new(),
            steps: vec![ReceivedStep {
                step_id: "step_server_1".to_string(),
                layer_id: handle.active.layer_id.clone(),
                parent_step_id: None,
                base_layer_id: Some(handle.active.layer_id.clone()),
                base_tree_id: None,
                root_tree_id: Some(tree_id.clone()),
                changed_paths: vec!["note.txt".to_string()],
                captured_at_unix: Some(1_782_910_398),
            }],
        };

        apply_receive_response(&mut handle, response, true, None, None).unwrap();

        assert_eq!(fs::read(space.join("note.txt")).unwrap(), bytes);
        assert!(handle
            .meta
            .layers
            .iter()
            .any(|layer| layer.layer_id == "layer_without_head"));
        let received_steps = read_step_files(&handle.layrs_dir, &handle.active.layer_id).unwrap();
        assert!(received_steps
            .iter()
            .any(|step| step.step_id == "step_server_1"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn first_child_layer_step_diffs_against_parent_layer() {
        let root = unique_test_dir("child-step-parent");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "parent\n").unwrap();

        let created = create_local_space(
            "space-child-step".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        let child = create_layer_from_current(
            created.local_space.local_space_id.clone(),
            "Child".to_string(),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "child\n").unwrap();
        switch_layer(
            created.local_space.local_space_id.clone(),
            "main".to_string(),
        )
        .unwrap();
        switch_layer(
            created.local_space.local_space_id.clone(),
            child.active_layer_id.clone(),
        )
        .unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        assert_eq!(scan.steps.len(), 1);
        assert_eq!(scan.steps[0].changed_files, 1);
        let lines = &scan.steps[0].diffs[0].diff.hunks[0].lines;
        assert!(lines
            .iter()
            .any(|line| line.op == "delete" && line.text == "parent"));
        assert!(lines
            .iter()
            .any(|line| line.op == "insert" && line.text == "child"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delete_layer_removes_non_active_local_layer() {
        let root = unique_test_dir("delete-layer");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "main\n").unwrap();

        let created = create_local_space(
            "space-delete-layer".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        let child = create_layer_from_current(
            created.local_space.local_space_id.clone(),
            "Scratch".to_string(),
        )
        .unwrap();
        switch_layer(
            created.local_space.local_space_id.clone(),
            "main".to_string(),
        )
        .unwrap();

        let result = delete_layer(
            created.local_space.local_space_id.clone(),
            child.active_layer_id.clone(),
        )
        .unwrap();

        assert_eq!(result.local_space.layers.len(), 1);
        assert_eq!(result.local_space.active_layer_id.as_deref(), Some("main"));
        assert!(!space
            .join(LAYRS_DIR)
            .join("layers")
            .join(child.active_layer_id)
            .exists());

        let _ = fs::remove_dir_all(root);
    }

    fn numbered_lines(prefix: &str, count: usize) -> String {
        let mut text = String::new();
        for index in 0..count {
            text.push_str(prefix);
            text.push('-');
            text.push_str(&index.to_string());
            text.push('\n');
        }
        text
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("layrs-{name}-{}", unix_now()));
        let _ = fs::remove_dir_all(&path);
        path
    }
}
