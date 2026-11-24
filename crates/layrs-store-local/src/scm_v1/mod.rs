//! Fresh local-store baseline for the native SCM model.
//!
//! This module is intentionally independent from the development V1/V2
//! layout. Detection is read-only and initialization never imports, migrates,
//! deletes, or rewrites an unsupported store.

mod acquisition;
pub use acquisition::{open_for_read_with_retry, open_for_registration_with_retry};
mod admission;
pub mod cas;
pub mod change_index;
pub mod checkout;
pub mod checkout_ignore;
mod checkout_paths;
pub mod compaction;
mod consolidation;
mod derived_cache;
mod format;
pub mod fsck;
mod guarded_vfs;
mod identity;
mod layout;
pub use identity::{LocalIdentity, configure_local_identity, local_identity};
pub mod materialize;
pub mod projection_cache;
mod read;
pub mod recovery;
mod references;
mod repository;
pub mod schema;
mod sqlite;
pub mod status_view;
pub mod vfs_recovery;
pub mod worktree_audit;
pub mod worktrees;
mod write_batch;

#[cfg(any(test, feature = "test-support"))]
pub use admission::existence_proofs_on_this_thread;
pub use admission::{CompositeAdmissionResolver, preflight_batch_budgets};
pub use cas::{
    CasError, CasFaultInjector, CasFaultPoint, CasLocation, CasResult, CasScrubIssue,
    CasScrubReport, InstallOutcome, NativeCas, NativeCasAudit, NativeCasWriter, PackFileHold,
    PreparedObject, PutItem,
};
#[cfg(any(test, feature = "test-support"))]
pub use cas::{
    blocks_read_on_this_thread, forget_loaded_packs, held_pack_files, set_cluster_max_raw,
    set_consolidation_budget, set_inline_pack_limit,
};
pub use consolidation::{
    MaintenanceOutcome, MetadataRewrite, maintain, maintenance_paused, maintenance_running,
    rewrite_metadata, take_maintenance_due,
};
#[cfg(any(test, feature = "test-support"))]
pub use consolidation::{maintain_with_pause, set_vacuum_min_pages};
pub use fsck::{
    FsckError, FsckFinding, FsckFindingCode, FsckOptions, FsckReport, FsckSeverity, GcReachability,
    NativeFsck, ProjectionAction, ProjectionRebuildPlan, fsck, fsck_existing,
};

pub use format::{
    COMPACT_FORMAT_VERSION, FORMAT_MAGIC, FORMAT_SCHEMA, FORMAT_VERSION, FormatMarker, FormatState,
    HISTORY_FORMAT_VERSION, NEWEST_FORMAT_VERSION, NativeStoreError, NativeStoreInspection,
    PACKED_FORMAT_VERSION, SharedReadLock, SharedReadLocks, WHOLE_FILES_FORMAT_VERSION,
    creation_format_version, init, init_with_format, inspect, keeps_whole_files, open, open_audit,
    open_exclusive, remove_owned_folder, supported_format,
};
pub use layout::{
    FORMAT_FILE_NAME, LOCK_FILE_NAME, METADATA_DIR_NAME, NativeStoreAudit, NativeStoreExclusive,
    NativeStoreLayout, NativeStoreOpen, OBJECTS_DIR_NAME, REPAIR_BACKUPS_DIR_NAME,
    SQLITE_FILE_NAME, STAGING_DIR_NAME, STAGING_LEASE_FILE_NAME,
};
pub use read::{
    FileRecordRefs, NativeScmReader, NativeStoreReading, NativeTransactionalScmReader,
    ObservationProjection, ObservationSnapshot,
};
pub use recovery::{
    ClaimOutcome, MutationOutcome, NativeRecovery, PathEvidence, RecoveryDecision, RecoveryError,
    RecoveryFaultInjector, RecoveryFaultPoint, RecoveryFinding, RecoveryFindingCode,
    RecoveryHandler, RecoveryId, RecoveryPathPlan, RecoveryPathRecord, RecoveryPathResult,
    RecoveryPathRole, RecoveryPathState, RecoveryPlan, RecoveryPreparation, RecoveryRecord,
    RecoveryReport, RecoveryResult, RecoveryState, RecoveryTransition, RecoveryType, ResourceLease,
};
pub use repository::NativeScmRepository;
pub use schema::{
    BASELINE_NAME, REQUIRED_TABLES, SCHEMA_CHECKSUM, SCHEMA_FINGERPRINT, SCHEMA_MAGIC, SCHEMA_SQL,
    SCHEMA_VERSION, schema_checksum,
};
pub use sqlite::ScmSchemaError;
pub use write_batch::{RepositoryFaultInjector, RepositoryFaultPoint};

/// Asks the next Workspace discovery of this process to load its Store's
/// catalog on a thread (`warm_catalog_if_requested`): for a command that
/// will read projections, so the packs' indexes load while it discovers.
pub fn request_catalog_warmup() {
    CATALOG_WARMUP.store(true, std::sync::atomic::Ordering::Release);
}

static CATALOG_WARMUP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `warm_catalog`, once, if `request_catalog_warmup` asked for it.
pub fn warm_catalog_if_requested(root: &std::path::Path) {
    if CATALOG_WARMUP.swap(false, std::sync::atomic::Ordering::AcqRel) {
        warm_catalog(root);
    }
}

/// Loads the catalog of the Store at `root` (its registered packs' indexes,
/// cached for this process and checked against their registration at every
/// catalog load) on a detached thread, through its own shared session.
/// Best effort: a failure is the reader's to find and report.
pub fn warm_catalog(root: &std::path::Path) {
    let root = root.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("layrs-catalog".into())
        .spawn(move || {
            let _span = crate::perf::span("warm.catalog");
            if let Ok(store) = open(&root) {
                if let Ok(reader) = NativeScmReader::new(&store) {
                    let _ = reader.catalog_damaged();
                }
            }
        });
}
