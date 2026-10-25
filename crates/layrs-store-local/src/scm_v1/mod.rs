//! Fresh local-store baseline for the native SCM model.
//!
//! This module is intentionally independent from the development V1/V2
//! layout. Detection is read-only and initialization never imports, migrates,
//! deletes, or rewrites an unsupported store.

mod acquisition;
pub use acquisition::{open_for_read_with_retry, open_for_registration_with_retry};
mod admission;
pub mod cas;
pub mod checkout;
mod checkout_ignore;
mod checkout_paths;
pub mod compaction;
mod format;
pub mod fsck;
mod identity;
mod layout;
pub use identity::{LocalIdentity, configure_local_identity, local_identity};
pub mod materialize;
mod read;
pub mod recovery;
mod repository;
pub mod schema;
mod sqlite;
pub mod worktrees;
pub mod worktree_audit;
pub mod vfs_recovery;
mod write_batch;

pub use admission::CompositeAdmissionResolver;
pub use cas::{
    CasError, CasFaultInjector, CasFaultPoint, CasResult, CasScrubIssue, CasScrubReport,
    InstallOutcome, NativeCas, NativeCasAudit, NativeCasWriter, PreparedObject,
};
pub use fsck::{
    FsckError, FsckFinding, FsckFindingCode, FsckOptions, FsckReport, FsckSeverity, GcReachability,
    NativeFsck, ProjectionAction, ProjectionRebuildPlan, fsck, fsck_existing,
};

pub use format::{
    FORMAT_MAGIC, FORMAT_SCHEMA, FORMAT_VERSION, FormatMarker, FormatState, NativeStoreError,
    NativeStoreInspection, init, inspect, open, open_audit, open_exclusive,
};
pub use layout::{
    FORMAT_FILE_NAME, LOCK_FILE_NAME, METADATA_DIR_NAME, NativeStoreAudit, NativeStoreExclusive,
    NativeStoreLayout, NativeStoreOpen, OBJECTS_DIR_NAME, REPAIR_BACKUPS_DIR_NAME,
    SQLITE_FILE_NAME, STAGING_DIR_NAME, STAGING_LEASE_FILE_NAME,
};
pub use read::{
    NativeScmReader, NativeTransactionalScmReader, ObservationProjection, ObservationSnapshot,
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
