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
mod repository;
pub mod schema;
mod sqlite;
pub mod vfs_recovery;
pub mod worktree_audit;
pub mod worktrees;
mod write_batch;

pub use admission::CompositeAdmissionResolver;
#[cfg(any(test, feature = "test-support"))]
pub use admission::existence_proofs_on_this_thread;
pub use cas::{
    CasError, CasFaultInjector, CasFaultPoint, CasLocation, CasResult, CasScrubIssue,
    CasScrubReport, InstallOutcome, NativeCas, NativeCasAudit, NativeCasWriter, PackFileHold,
    PreparedObject, PutItem,
};
#[cfg(any(test, feature = "test-support"))]
pub use cas::{forget_loaded_packs, held_pack_files, set_inline_pack_limit};
pub use fsck::{
    FsckError, FsckFinding, FsckFindingCode, FsckOptions, FsckReport, FsckSeverity, GcReachability,
    NativeFsck, ProjectionAction, ProjectionRebuildPlan, fsck, fsck_existing,
};

pub use format::{
    COMPACT_FORMAT_VERSION, FORMAT_MAGIC, FORMAT_SCHEMA, FORMAT_VERSION, FormatMarker, FormatState,
    NEWEST_FORMAT_VERSION, NativeStoreError, NativeStoreInspection, PACKED_FORMAT_VERSION,
    SharedReadLock, SharedReadLocks, WHOLE_FILES_FORMAT_VERSION, creation_format_version, init,
    init_with_format, inspect, open, open_audit, open_exclusive, supported_format,
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
