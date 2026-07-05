use crate::auth::{clear_desktop_session, is_invalid_desktop_token_error};
use crate::desktop_state::{
    load_cached_bootstrap, save_cached_bootstrap, validate_desktop_bootstrap, workspace_root,
    BootstrapData, DesktopConfig, DesktopSettings, LayerSummary, LocalSpaceConfigEntry,
    SpaceSummary,
};
use crate::http_client::{
    delete_json, get_bytes_with_headers, get_json, post_json, put_bytes_json_with_headers,
};
use crate::object_store::{
    compact_loose_chunks, read_chunk_encoded, read_chunk_raw, read_file_object_bytes,
    write_file_object_manifest, write_received_encoded_chunk, CompactStoreResult, FileChunkRef,
    FileObjectFile, FILE_OBJECT_SCHEMA,
};
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
const PENDING_PUBLISH_SCHEMA: &str = "layrs.pending_publish.v1";
const PENDING_LAYER_DELETIONS_SCHEMA: &str = "layrs.pending_layer_deletions.v1";
const TREE_OBJECT_SCHEMA: &str = "layrs.tree_object.v1";
const SCAN_CACHE_SCHEMA: &str = "layrs.scan_cache.v1";
const SYNC_STATE_SCHEMA: &str = "layrs.layer_sync_state.v1";
const WEAVE_SESSION_SCHEMA: &str = "layrs.weave_session.v1";
const SYNC_PROTOCOL_V2: &str = "layrs.sync.v2";
const LOCAL_SPACE_STATE_LINKED: &str = "linked";
const LOCAL_SPACE_STATE_DRAFT: &str = "draft";
const TEXT_DIFF_DEFAULT_WINDOW_LIMIT: usize = 400;
const TEXT_DIFF_MAX_WINDOW_LIMIT: usize = 2_000;

include!("access_registry/models.rs");
include!("access_registry/api_local_spaces.rs");
include!("access_registry/api_operations.rs");
include!("access_registry/bootstrap.rs");
include!("access_registry/working_tree.rs");
include!("access_registry/weaves.rs");
include!("access_registry/diff.rs");
include!("access_registry/receive.rs");
include!("access_registry/publish.rs");
include!("access_registry/utils.rs");

include!("access_registry/tests.rs");
