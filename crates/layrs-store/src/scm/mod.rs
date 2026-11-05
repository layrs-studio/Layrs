//! Synchronous storage contract for the native Layrs SCM model.
//!
//! A user-visible [`layrs_core::scm::Operation`] records an intentional SCM state transition.
//! It is domain history and may be inspected, retained, or published. A
//! backend's recovery journal is different: it is private technical evidence
//! used to make [`ScmRepository::apply_batch`] atomic and idempotent. Recovery
//! must never synthesize user Operations, and pruning a technical journal must
//! never alter the user Operation DAG.
//!
//! This module describes contracts only. Filesystem layout, SQLite, locking,
//! asynchronous execution, and recovery policy belong to backend crates.

mod batch;
mod error;
mod maintenance;
mod product;
mod repository;
mod types;
pub use product::CanonicalProduct;

pub use batch::{
    OperationHeadsCas, RepositoryViewCas, StoreMutation, WorktreeActiveOperationCas, WriteBatch,
};
pub use error::{Result, StoreError};
pub use maintenance::{
    FsckFinding, FsckFindingCode, FsckReport, Maintenance, RecoveryFinding, RecoveryFindingCode,
    RecoveryReport, ReportSeverity,
};
pub use repository::{ApplyBatchOutcome, CurrentRepositoryView, ScmReader, ScmRepository};
pub use types::{
    AvailableFile, CanonicalConflict, CanonicalObject, EntityKind, EntityRecord, FileAvailability,
    RequestDigest, SpaceObjectAdmission, StoreObjectId, StoreObjectKind, TransactionId,
};

#[cfg(test)]
mod tests;
