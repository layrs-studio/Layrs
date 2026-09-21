//! Format3 partial-checkout metadata. No incomplete object is admitted into the
//! format2 SCM catalog. Selective hydration and local patches never authorize
//! network access or synthesize incomplete SCM Operations.
mod capture;
pub use capture::{
    CaptureFaults, CapturePoint, CapturedChange, CapturedFile, CapturedPatch, CapturedTarget,
    MAX_PATCH_BYTES, NoCaptureFaults,
};
mod database;
mod file;
mod format;
mod install;
mod publication;
pub use install::{InstallFaults, InstallPoint, NoInstallFaults};
pub use publication::{
    CompletedPublication, NoPublicationFaults, PendingPublication, PublicationFaults,
    PublicationPoint,
};
#[cfg(test)]
mod tests;

use crate::lock::SessionMode;
pub use format::{CreationFaults, CreationPoint, NoCreationFaults};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{SpaceId, StoreId, WorktreeId},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt, io,
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, ProjectionStoreError>;
#[derive(Debug)]
pub enum ProjectionStoreError {
    Io(io::Error),
    Database(rusqlite::Error),
    Invalid(String),
    Stale { expected: u64, actual: u64 },
}
impl fmt::Display for ProjectionStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Database(e) => e.fmt(f),
            Self::Invalid(e) => f.write_str(e),
            Self::Stale { expected, actual } => write!(
                f,
                "projection generation changed: expected {expected}, actual {actual}"
            ),
        }
    }
}
impl std::error::Error for ProjectionStoreError {}
impl From<io::Error> for ProjectionStoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for ProjectionStoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}
impl From<layrs_core::projection::ProjectionError> for ProjectionStoreError {
    fn from(e: layrs_core::projection::ProjectionError) -> Self {
        Self::Invalid(e.to_string())
    }
}
fn invalid(e: impl ToString) -> ProjectionStoreError {
    ProjectionStoreError::Invalid(e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionIdentity {
    pub store_id: StoreId,
    pub worktree_id: WorktreeId,
    pub space_id: SpaceId,
    pub origin: String,
    pub principal_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionedProjection {
    pub generation: u64,
    pub digest: String,
    pub snapshot: ProjectionSnapshot,
}

pub struct ProjectionStore {
    root: PathBuf,
    marker: format::Marker,
}
impl ProjectionStore {
    pub fn install_projection(
        &self,
        expected_generation: u64,
        target: &ProjectionSnapshot,
        write_file: &mut dyn FnMut(&str, &mut dyn io::Write) -> io::Result<()>,
    ) -> Result<VersionedProjection> {
        install::install(
            self,
            expected_generation,
            target,
            write_file,
            &NoInstallFaults,
        )
    }
    pub fn install_projection_with_faults(
        &self,
        expected_generation: u64,
        target: &ProjectionSnapshot,
        write_file: &mut dyn FnMut(&str, &mut dyn io::Write) -> io::Result<()>,
        faults: &dyn InstallFaults,
    ) -> Result<VersionedProjection> {
        install::install(self, expected_generation, target, write_file, faults)
    }
    pub fn resume_installation(checkout_root: impl AsRef<Path>) -> Result<VersionedProjection> {
        let root = std::path::absolute(checkout_root)?;
        let marker = format::read_marker(&root)?;
        install::resume(&Self { root, marker })
    }
    /// Fresh `.layrs` only. Existing complete stores require a separate explicit
    /// migration; this method never converts or overwrites them.
    pub fn create(
        checkout_root: impl AsRef<Path>,
        identity: ProjectionIdentity,
        initial: ProjectionSnapshot,
    ) -> Result<Self> {
        Self::create_with_faults(checkout_root, identity, initial, &NoCreationFaults)
    }
    pub fn create_with_faults(
        checkout_root: impl AsRef<Path>,
        identity: ProjectionIdentity,
        initial: ProjectionSnapshot,
        faults: &dyn CreationFaults,
    ) -> Result<Self> {
        format::create(checkout_root.as_ref(), identity, initial, faults)
    }
    /// Resume only a recognizable interrupted format3 bootstrap with the exact
    /// same identity and initial snapshot; never guess or erase a partial store.
    pub fn resume_create(
        checkout_root: impl AsRef<Path>,
        identity: &ProjectionIdentity,
        initial: &ProjectionSnapshot,
    ) -> Result<Self> {
        format::resume(checkout_root.as_ref(), identity, initial)
    }
    pub fn open(checkout_root: impl AsRef<Path>) -> Result<Self> {
        format::open(checkout_root.as_ref())
    }
    pub fn identity(&self) -> &ProjectionIdentity {
        &self.marker.identity
    }
    pub fn checkout_root(&self) -> &Path {
        &self.root
    }
    pub fn state(&self) -> Result<VersionedProjection> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_idle(&access.connection)?;
        let result = database::current(&access.connection)?;
        access.verify()?;
        Ok(result)
    }
    /// Atomic metadata CAS. File installation must be journaled by the partial
    /// materializer before recording a materialized state through this API.
    pub fn compare_and_swap(
        &self,
        expected_generation: u64,
        next: &ProjectionSnapshot,
    ) -> Result<VersionedProjection> {
        let bytes = next.canonical_bytes()?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current = database::current(&tx)?;
        if current.snapshot.entries != next.entries {
            return Err(invalid(
                "projection entries require journaled installation or capture",
            ));
        }
        if current.generation != expected_generation {
            return Err(ProjectionStoreError::Stale {
                expected: expected_generation,
                actual: current.generation,
            });
        }
        if current.snapshot.path_semantics != next.path_semantics {
            return Err(invalid(
                "path semantics cannot change during snapshot update",
            ));
        }
        if next.observed_authorization_epoch < current.snapshot.observed_authorization_epoch {
            return Err(invalid(
                "observed authorization epoch cannot move backwards",
            ));
        }
        let generation = expected_generation
            .checked_add(1)
            .filter(|g| *g <= i64::MAX as u64)
            .ok_or_else(|| invalid("projection generation exhausted"))?;
        let digest = database::digest(generation, Some(&current.digest), &bytes);
        tx.execute(
            "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
            rusqlite::params![generation, digest, current.digest, bytes],
        )?;
        tx.execute(
            "UPDATE projection_head SET generation=?1 WHERE singleton=1",
            [generation],
        )?;
        // Validate pinned directory/file identities before making SQL durable.
        format::verify_boundaries(&self.root, &self.marker, &access.metadata, &access.database)?;
        tx.commit()?;
        access.verify()?;
        Ok(VersionedProjection {
            generation,
            digest,
            snapshot: next.clone(),
        })
    }
}
