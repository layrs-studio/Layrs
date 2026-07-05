//! Server-side store abstraction for Layrs.
//!
//! The concrete database schema and object storage backend are intentionally
//! deferred. This crate defines the boundary that server handlers can target.

use layrs_sync::{
    ChunkId, IdempotencyKey, PublishReceipt, ReceiveRequest, ReceiveResponse, SyncDecision,
    SyncManifest,
};
use std::collections::BTreeMap;
use std::fmt;

pub mod schema {
    pub const ACCOUNTS: &str = "accounts";
    pub const ACCOUNT_PASSWORDS: &str = "account_passwords";
    pub const WEB_SESSIONS: &str = "web_sessions";
    pub const DESKTOP_DEVICES: &str = "desktop_devices";
    pub const DESKTOP_DEVICE_TOKENS: &str = "desktop_device_tokens";
    pub const DEVICE_AUTHORIZATION_FLOWS: &str = "device_authorization_flows";
    pub const WORKSPACES: &str = "workspaces";
    pub const WORKSPACE_MEMBERSHIPS: &str = "workspace_memberships";
    pub const TEAMS: &str = "teams";
    pub const TEAM_MEMBERSHIPS: &str = "team_memberships";
    pub const SPACES: &str = "spaces";
    pub const SPACE_MEMBERSHIPS: &str = "space_memberships";
    pub const LAYERS: &str = "layers";
    pub const LAYER_MEMBERSHIPS: &str = "layer_memberships";
    pub const ARTIFACTS: &str = "artifacts";
    pub const LAYER_ACCESS_POLICIES: &str = "layer_access_policies";
    pub const LAYER_ACCESS_POLICY_RULES: &str = "layer_access_policy_rules";
    pub const ARTIFACT_CONTENT_OBJECTS: &str = "artifact_content_objects";
    pub const INVITATIONS: &str = "invitations";
    pub const AUDIT_EVENTS: &str = "audit_events";
    pub const CHUNKS: &str = "chunks";
    pub const OBJECT_CHUNKS: &str = "object_chunks";
    pub const FILE_OBJECTS: &str = "file_objects";
    pub const FILE_OBJECT_CHUNKS: &str = "file_object_chunks";
    pub const TREE_OBJECTS: &str = "tree_objects";
    pub const TREE_ENTRIES: &str = "tree_entries";
    pub const LAYER_STATES: &str = "layer_states";
    pub const LAYER_HEADS: &str = "layer_heads";
    pub const LAYER_STEPS: &str = "layer_steps";
    pub const SYNC_MANIFESTS: &str = "sync_manifests";
    pub const SYNC_IDEMPOTENCY: &str = "sync_idempotency";
    pub const SYNC_BATCHES: &str = "sync_batches";
    pub const SYNC_BATCH_CHANGES: &str = "sync_batch_changes";
    pub const WEAVES: &str = "weaves";
    pub const WEAVE_REQUESTS: &str = "weave_requests";
    pub const WEAVE_SESSIONS: &str = "weave_sessions";
    pub const WEAVE_CONFLICTS: &str = "weave_conflicts";
    pub const WEAVE_RESOLUTIONS: &str = "weave_resolutions";
    pub const PROOFS: &str = "proofs";
    pub const POLICIES: &str = "policies";
    pub const TIMELINE_EVENTS: &str = "timeline_events";

    pub const TABLES: &[&str] = &[
        ACCOUNTS,
        ACCOUNT_PASSWORDS,
        WEB_SESSIONS,
        DESKTOP_DEVICES,
        DESKTOP_DEVICE_TOKENS,
        DEVICE_AUTHORIZATION_FLOWS,
        WORKSPACES,
        WORKSPACE_MEMBERSHIPS,
        TEAMS,
        TEAM_MEMBERSHIPS,
        SPACES,
        SPACE_MEMBERSHIPS,
        LAYERS,
        LAYER_MEMBERSHIPS,
        ARTIFACTS,
        LAYER_ACCESS_POLICIES,
        LAYER_ACCESS_POLICY_RULES,
        ARTIFACT_CONTENT_OBJECTS,
        INVITATIONS,
        AUDIT_EVENTS,
        CHUNKS,
        OBJECT_CHUNKS,
        FILE_OBJECTS,
        FILE_OBJECT_CHUNKS,
        TREE_OBJECTS,
        TREE_ENTRIES,
        LAYER_STATES,
        LAYER_HEADS,
        LAYER_STEPS,
        SYNC_MANIFESTS,
        SYNC_IDEMPOTENCY,
        SYNC_BATCHES,
        SYNC_BATCH_CHANGES,
        WEAVES,
        WEAVE_REQUESTS,
        WEAVE_SESSIONS,
        WEAVE_CONFLICTS,
        WEAVE_RESOLUTIONS,
        PROOFS,
        POLICIES,
        TIMELINE_EVENTS,
    ];

    pub const SCHEMA_NOTES: &[&str] = &[
        "server is the source of truth for workspace membership and permissions",
        "desktop tokens must be stored as digests and bound to a registered device key",
        "layer access registries live at .layrs/layers/<layer_id>/access.json",
        "layer access policy_epoch must be compared before replacing a registry",
        "sync_idempotency must have a unique key over workspace_id and idempotency_key",
        "chunk rows should be content-addressed and object-store backed",
        "V2 file objects are assembled from ordered object_chunks",
        "layer_heads advance atomically after policy_epoch and object availability checks",
        "layer_steps store synchronizable anonymous Layer snapshots separate from Layer heads",
        "weave sessions must preserve the target layer pre-weave tree until applied or aborted",
        "timeline events should be append-only and cursor-addressable",
        "policies should be evaluated before publish decisions are committed",
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerStoreConfig {
    pub deployment_id: String,
    pub database_url: Option<String>,
    pub object_store: ObjectStoreConfig,
    pub max_chunk_bytes: u64,
    pub require_policy_epoch_match: bool,
}

impl Default for ServerStoreConfig {
    fn default() -> Self {
        Self {
            deployment_id: "local-dev".into(),
            database_url: None,
            object_store: ObjectStoreConfig::InMemory,
            max_chunk_bytes: 64 * 1024 * 1024,
            require_policy_epoch_match: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectStoreConfig {
    InMemory,
    FileSystem {
        root: String,
    },
    External {
        provider: String,
        bucket: String,
        prefix: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn for_chunk(chunk_id: &ChunkId) -> Self {
        Self(format!("chunks/{}", chunk_id.as_str()))
    }

    pub fn new(value: impl Into<String>) -> StoreResult<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(StoreError::invalid("object key must not be empty"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreError {
    pub code: StoreErrorCode,
    pub message: String,
}

impl StoreError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: StoreErrorCode::InvalidRequest,
            message: message.into(),
        }
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self {
            code: StoreErrorCode::NotImplemented,
            message: message.into(),
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for StoreError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum StoreErrorCode {
    InvalidRequest,
    PermissionDenied,
    Conflict,
    NotFound,
    NotImplemented,
    BackendUnavailable,
}

pub trait ObjectStore {
    fn put(&mut self, key: &ObjectKey, bytes: &[u8]) -> StoreResult<()>;
    fn get(&self, key: &ObjectKey) -> StoreResult<Option<Vec<u8>>>;
    fn exists(&self, key: &ObjectKey) -> StoreResult<bool>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMemoryObjectStore {
    objects: BTreeMap<ObjectKey, Vec<u8>>,
}

impl InMemoryObjectStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObjectStore for InMemoryObjectStore {
    fn put(&mut self, key: &ObjectKey, bytes: &[u8]) -> StoreResult<()> {
        self.objects.insert(key.clone(), bytes.to_vec());
        Ok(())
    }

    fn get(&self, key: &ObjectKey) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.objects.get(key).cloned())
    }

    fn exists(&self, key: &ObjectKey) -> StoreResult<bool> {
        Ok(self.objects.contains_key(key))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishReservation {
    pub idempotency_key: IdempotencyKey,
    pub reservation_id: String,
    pub already_applied: bool,
    pub server_cursor: Option<String>,
}

pub trait ServerStore {
    fn config(&self) -> &ServerStoreConfig;
    fn object_store(&self) -> &dyn ObjectStore;
    fn object_store_mut(&mut self) -> &mut dyn ObjectStore;

    fn reserve_publish(
        &mut self,
        idempotency_key: &IdempotencyKey,
        manifest: &SyncManifest,
    ) -> StoreResult<PublishReservation>;

    fn record_publish_decision(
        &mut self,
        idempotency_key: &IdempotencyKey,
        decision: SyncDecision,
    ) -> StoreResult<()>;

    fn load_publish_receipt(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> StoreResult<Option<PublishReceipt>>;

    fn receive_since(&self, request: &ReceiveRequest) -> StoreResult<ReceiveResponse>;
}

pub struct NoopServerStore {
    config: ServerStoreConfig,
    object_store: InMemoryObjectStore,
}

impl NoopServerStore {
    pub fn new(config: ServerStoreConfig) -> Self {
        Self {
            config,
            object_store: InMemoryObjectStore::new(),
        }
    }
}

impl ServerStore for NoopServerStore {
    fn config(&self) -> &ServerStoreConfig {
        &self.config
    }

    fn object_store(&self) -> &dyn ObjectStore {
        &self.object_store
    }

    fn object_store_mut(&mut self) -> &mut dyn ObjectStore {
        &mut self.object_store
    }

    fn reserve_publish(
        &mut self,
        idempotency_key: &IdempotencyKey,
        _manifest: &SyncManifest,
    ) -> StoreResult<PublishReservation> {
        Err(StoreError::not_implemented(format!(
            "reserve_publish for {} is not wired to a database yet",
            idempotency_key
        )))
    }

    fn record_publish_decision(
        &mut self,
        _idempotency_key: &IdempotencyKey,
        _decision: SyncDecision,
    ) -> StoreResult<()> {
        Err(StoreError::not_implemented(
            "record_publish_decision is not wired to a database yet",
        ))
    }

    fn load_publish_receipt(
        &self,
        _idempotency_key: &IdempotencyKey,
    ) -> StoreResult<Option<PublishReceipt>> {
        Err(StoreError::not_implemented(
            "load_publish_receipt is not wired to a database yet",
        ))
    }

    fn receive_since(&self, _request: &ReceiveRequest) -> StoreResult<ReceiveResponse> {
        Err(StoreError::not_implemented(
            "receive_since is not wired to a database yet",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_exposes_idempotency_table() {
        assert!(schema::TABLES.contains(&schema::SYNC_IDEMPOTENCY));
    }

    #[test]
    fn schema_exposes_merkle_v2_tables() {
        assert!(schema::TABLES.contains(&schema::OBJECT_CHUNKS));
        assert!(schema::TABLES.contains(&schema::FILE_OBJECTS));
        assert!(schema::TABLES.contains(&schema::TREE_OBJECTS));
        assert!(schema::TABLES.contains(&schema::LAYER_HEADS));
        assert!(schema::TABLES.contains(&schema::SYNC_BATCHES));
    }

    #[test]
    fn in_memory_object_store_round_trips_bytes() {
        let mut store = InMemoryObjectStore::new();
        let key = ObjectKey::new("chunks/chunk-1").unwrap();

        store.put(&key, b"hello").unwrap();

        assert_eq!(store.exists(&key), Ok(true));
        assert_eq!(store.get(&key), Ok(Some(b"hello".to_vec())));
    }
}
