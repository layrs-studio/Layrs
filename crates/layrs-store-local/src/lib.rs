//! Durable local metadata and content-addressed storage for Layrs.
//!
//! This crate owns the on-disk `.layrs` foundation. The client core will adopt
//! its transactional repository in a later delivery; no core layout migration
//! is performed here.

mod cas;
mod error;
mod fault;
mod fsck;
mod journal;
mod layout;
mod lock;
mod recovery;
mod repository;
mod schema;
mod session;
mod sqlite;

use std::io;

pub use cas::{
    ContentHash, HashAlgorithm, HashError, HashVerification, ObjectWriteOutcome,
    ObjectWriteOutcomeV2, PendingObject, ScrubReport, StoredChunkObject, StoredFileObject,
    StoredLocalStepRef, StoredTreeObject, cas_path, cas_path_for_digest, verify_content_hash,
};
pub use error::{StoreError, StoreResult};
pub use fsck::{
    FsckCode, FsckIssue, FsckReport, RepairAction, RepairReport, fsck_existing, repair,
};
pub use journal::{
    JournalOperation, JournalPath, JournalPathInput, JournalState, NewJournalOperation,
    OperationKind,
};
pub use layout::{
    DEFAULT_CAS_DIR, DEFAULT_LOCK_FILE, DEFAULT_METADATA_DIR, DEFAULT_SQLITE_FILE,
    DEFAULT_STORE_DIR, DEFAULT_TMP_DIR, LocalStoreConfig, PathIdentity, StoreLayout,
    contains_reparse_component, normalize_path_key, path_has_reparse_point,
};
pub use lock::SessionMode;
pub use recovery::{
    RecoveryAction, RecoveryHandler, RecoveryReport, RecoveryResult, recover_operations,
};
pub use repository::{
    LayerRecord, MetadataRepository, ObjectRecord, PendingPublishRecord, RefRecord, RefUpdate,
    SpaceRecord, StepPathRecord, StepRecord, SyncCursorRecord,
};
pub use schema::{CURRENT_SCHEMA_VERSION, MigrationInfo, migration_info};
pub use session::LocalStoreSession;
pub use sqlite::PragmaSnapshot;

use layrs_sync::{Chunker, LocalStepRef, ObjectDigest, TreeEntry};

#[derive(Debug, Clone)]
pub struct LocalStore {
    config: LocalStoreConfig,
    layout: StoreLayout,
}

impl LocalStore {
    /// Creates the store layout and initializes or upgrades the real SQLite DB.
    pub fn init(config: LocalStoreConfig) -> io::Result<Self> {
        let layout = config.validated_layout()?;
        layout::ensure_layout(&layout)?;
        let store = Self { config, layout };
        session::initialize_store(&store).map_err(StoreError::into_io_error)?;
        Ok(store)
    }

    /// Opens an existing root, creating missing subordinate layout directories.
    pub fn open(config: LocalStoreConfig) -> io::Result<Self> {
        if !config.root.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("store root does not exist: {}", config.root.display()),
            ));
        }

        let layout = config.validated_layout()?;
        layout::ensure_layout(&layout)?;
        let store = Self { config, layout };
        session::initialize_store(&store).map_err(StoreError::into_io_error)?;
        Ok(store)
    }

    pub fn init_or_open(config: LocalStoreConfig) -> io::Result<Self> {
        if config.root.exists() {
            Self::open(config)
        } else {
            Self::init(config)
        }
    }

    pub fn config(&self) -> &LocalStoreConfig {
        &self.config
    }

    pub fn layout(&self) -> &StoreLayout {
        &self.layout
    }

    pub fn session(&self, mode: SessionMode) -> StoreResult<LocalStoreSession> {
        LocalStoreSession::open(self, mode)
    }

    /// Runs consistency checks without creating files or applying migrations.
    pub fn fsck(&self) -> StoreResult<FsckReport> {
        fsck::fsck_read_only(&self.layout, self.config.sqlite_options())
    }

    pub fn repair(&self, findings: &FsckReport, updated_at_ms: i64) -> StoreResult<RepairReport> {
        fsck::repair(self, findings, updated_at_ms)
    }

    pub fn cas_path(&self, hash: &ContentHash) -> std::path::PathBuf {
        cas_path(&self.layout.cas_dir, hash)
    }

    pub fn cas_path_v2(&self, digest: &ObjectDigest) -> std::path::PathBuf {
        cas_path_for_digest(&self.layout.cas_dir, digest)
    }

    pub fn write_temp_object(&self, bytes: &[u8]) -> io::Result<PendingObject> {
        cas::write_temp_object(&self.layout, bytes)
    }

    pub fn write_object_prehashed(
        &self,
        bytes: &[u8],
        hash: &ContentHash,
    ) -> io::Result<ObjectWriteOutcome> {
        cas::write_object_prehashed(&self.layout, bytes, hash)
    }

    pub fn write_object_v2(
        &self,
        bytes: &[u8],
        digest: &ObjectDigest,
    ) -> io::Result<ObjectWriteOutcomeV2> {
        cas::write_object_v2(&self.layout, bytes, digest)
    }

    pub fn write_chunk_v2(&self, bytes: &[u8]) -> io::Result<StoredChunkObject> {
        cas::write_chunk_v2(&self.layout, bytes)
    }

    pub fn build_file_v2(
        &self,
        bytes: &[u8],
        chunker: &dyn Chunker,
    ) -> io::Result<StoredFileObject> {
        cas::build_file_v2(&self.layout, bytes, chunker)
    }

    pub fn write_tree_v2(&self, entries: Vec<TreeEntry>) -> io::Result<StoredTreeObject> {
        cas::write_tree_v2(&self.layout, entries)
    }

    pub fn write_local_step_ref_v2(&self, step: LocalStepRef) -> io::Result<StoredLocalStepRef> {
        cas::write_local_step_ref_v2(&self.layout, step)
    }

    pub fn scrub(&self) -> io::Result<ScrubReport> {
        cas::scrub(&self.layout)
    }
}
