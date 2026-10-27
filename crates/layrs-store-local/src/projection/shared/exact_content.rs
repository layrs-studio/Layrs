//! Exact managed representation installation. Catalog scopes contain metadata
//! only; all byte I/O and no-replace publication happen with those scopes shut.
use crate::{
    lock::StoreLock,
    projection::{
        file,
        provider::recovery::error::{invalid, Result},
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_core::scm::{canonical_json, FileId, FileManifest, FileManifestId, WorktreeId};
use layrs_vfs::Cancellation;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

mod journal;
mod publish;
mod published_cas;
mod spool;
mod sql;
#[cfg(test)]
mod tests;

pub(in crate::projection) use journal::{ExactRecord, Phase};
pub(in crate::projection) use publish::publish;
pub(in crate::projection) use published_cas::{verify_published_cas, PublishedCasProof};
pub(in crate::projection) use spool::{finish_piece, prepare_piece, verify_staged, OwnedPiece};
pub(in crate::projection) use sql::{
    validate_installations, validate_protected_installations, ExactInstallState,
};

pub(in crate::projection) fn canonical_manifest_bytes(manifest: &FileManifest) -> Result<Vec<u8>> {
    journal::manifest_bytes(manifest)
}

pub(in crate::projection) const PREPARE_CHARGE: u64 = 8192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ExactBinding {
    pub family_incarnation: String,
    pub family_root_identity: String,
    pub family_marker_digest: String,
    pub family_catalog_identity: String,
    pub plan_digest: String,
    pub prefix_id: String,
    pub destination_proof_digest: String,
    pub transaction_id: String,
    pub owner: WorktreeId,
    pub database_identity: String,
    pub reservation: String,
    pub operation: String,
    pub quota_bytes: u64,
}
impl ExactBinding {
    pub(in crate::projection) fn validate(&self) -> Result<()> {
        for value in [
            &self.family_marker_digest,
            &self.plan_digest,
            &self.prefix_id,
            &self.destination_proof_digest,
        ] {
            if !token(value) {
                return Err(invalid("exact installation binding digest invalid"));
            }
        }
        for value in [
            &self.family_incarnation,
            &self.family_root_identity,
            &self.family_catalog_identity,
            &self.database_identity,
        ] {
            if value.is_empty() || value.len() > 512 || value.contains('\0') {
                return Err(invalid("exact installation binding text exceeds bound"));
            }
        }
        if self.owner.as_str().len() > 512
            || !hex(&self.transaction_id, 32)
            || self.reservation != self.operation
            || self.operation.len() != 84
            || !self.operation.starts_with("provider-transition:")
            || !hex(&self.operation[20..], 64)
            || self.quota_bytes > i64::MAX as u64
        {
            return Err(invalid("exact installation reservation binding differs"));
        }
        Ok(())
    }
}

/// Owned pins plus an independent shared lifecycle guard prevent reclamation
/// while an attempt is outside its catalog scope. The caller admits this exact
/// physical family; this constructor does not grant source or SQL authority.
pub(in crate::projection) struct ExactStorage {
    root_path: PathBuf,
    root: PinnedDirectory,
    staging: PinnedDirectory,
    chunks: PinnedDirectory,
    manifests: PinnedDirectory,
    _lifecycle: StoreLock,
}
impl ExactStorage {
    pub(in crate::projection) fn open(
        root: &Path,
        root_identity: &str,
        lifecycle: StoreLock,
    ) -> Result<Self> {
        let pin = PinnedDirectory::open(root)?;
        if pin.identity_token() != root_identity {
            return Err(invalid("exact storage root identity differs"));
        }
        let staging = pin.open_private_child_for_write(OsStr::new("staging"))?;
        let objects = pin.open_child_for_traversal(OsStr::new("objects"))?;
        let chunks = objects.open_child(OsStr::new("chunks"))?;
        let manifests = objects.open_child(OsStr::new("manifests"))?;
        let value = Self {
            root_path: root.to_owned(),
            root: pin,
            staging,
            chunks,
            manifests,
            _lifecycle: lifecycle,
        };
        value.verify()?;
        Ok(value)
    }
    fn verify(&self) -> Result<()> {
        self.root.verify_path_identity()?;
        self.staging.verify_path_identity()?;
        self.chunks.verify_path_identity()?;
        self.manifests.verify_path_identity()?;
        Ok(())
    }
}

/// Closed crate-internal scope implemented by the managed provider adapter.
/// `action` receives an active caller transaction (FK ON for writes). A scope
/// returns only after COMMIT and every observable family/source close succeeds.
/// No ordinary Lease, opener, cleanup or arbitrary budget is exposed here.
pub(in crate::projection) trait ExactCatalog {
    fn expected_manifest(&self, id: &FileManifestId) -> Result<FileManifest>;
    fn binding(&self) -> Result<ExactBinding>;
    fn pin_storage(&self) -> Result<ExactStorage>;
    fn boundary(&self, cancel: &Cancellation) -> Result<()>;
    fn with_catalog<T>(
        &mut self,
        write: bool,
        cancel: &Cancellation,
        action: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T>;
}

pub(in crate::projection) struct ExactAttempt {
    manifest: FileManifest,
    manifest_bytes: Vec<u8>,
    record: ExactRecord,
    storage: ExactStorage,
    directory: PinnedDirectory,
}
impl ExactAttempt {
    pub(in crate::projection) fn manifest(&self) -> &FileManifest {
        &self.manifest
    }
    pub(in crate::projection) fn published(&self) -> bool {
        self.record.phase == Phase::Published
    }
    fn require_context<C: ExactCatalog>(&self, context: &C, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        if context.binding()? != self.record.binding
            || context.expected_manifest(self.manifest.id())? != self.manifest
        {
            return Err(invalid("exact attempt context or representation differs"));
        }
        self.storage.verify()?;
        self.directory.verify_path_identity()?;
        if self.record.directory_identity.as_deref()
            != Some(self.directory.identity_token().as_str())
        {
            return Err(invalid("exact attempt staging directory identity differs"));
        }
        context.boundary(cancel)
    }
}

pub(in crate::projection) fn prepare<C: ExactCatalog>(
    context: &mut C,
    id: &FileManifestId,
    cancel: &Cancellation,
) -> Result<ExactAttempt> {
    cancel.check()?;
    context.boundary(cancel)?;
    let binding = context.binding()?;
    binding.validate()?;
    let manifest = context.expected_manifest(id)?;
    if manifest.id() != id {
        return Err(invalid("exact expected ManifestId differs"));
    }
    let manifest_bytes = journal::manifest_bytes(&manifest)?;
    let storage = context.pin_storage()?;
    if storage.root.identity_token() != binding.family_root_identity {
        return Err(invalid("exact storage binding differs"));
    }
    let existing = context.with_catalog(false, cancel, |c| {
        sql::require_file_length(c, &manifest)?;
        sql::manifest_indexed(c, &manifest)?;
        for chunk in manifest.chunks() {
            cancel.check()?;
            sql::chunk_indexed(c, chunk.chunk_id().as_str(), chunk.byte_length())?;
        }
        sql::load(c, &binding, &manifest)
    })?;
    let mut record = if let Some(record) = existing {
        record
    } else {
        let record = ExactRecord::declared(
            binding,
            &manifest,
            super::unique_id().map_err(|error| invalid(error.to_string()))?,
        );
        context.with_catalog(true, cancel, |c| sql::insert(c, &record, &manifest))?;
        record
    };
    context.boundary(cancel)?;
    storage.verify()?;
    let directory = if record.phase == Phase::Declared {
        // A colliding name after an unacknowledged create is withheld. Do not
        // infer ownership from a nonce, rotate attempts, or remove that object.
        let directory = storage
            .staging
            .create_new_private_child(OsStr::new(&record.directory))?;
        let mut next = record.clone();
        next.phase = Phase::Allocated;
        next.directory_identity = Some(directory.identity_token());
        context.with_catalog(true, cancel, |c| sql::replace(c, &record, &next, &manifest))?;
        record = next;
        directory
    } else {
        let directory = storage
            .staging
            .open_private_child_for_write(OsStr::new(&record.directory))?;
        if record.directory_identity.as_deref() != Some(directory.identity_token().as_str()) {
            return Err(invalid("exact staging directory was replaced"));
        }
        directory
    };
    let attempt = ExactAttempt {
        manifest,
        manifest_bytes,
        record,
        storage,
        directory,
    };
    attempt.require_context(context, cancel)?;
    spool::require_namespace(&attempt)?;
    Ok(attempt)
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn token(value: &str) -> bool {
    value
        .strip_prefix("blake3:")
        .is_some_and(|value| hex(value, 64))
}
fn file_identity(pin: &PinnedFile) -> Result<String> {
    Ok(file::identity(pin.file())?)
}
fn object_name(id: &str) -> Result<&str> {
    if !token(id) {
        return Err(invalid("exact immutable object key invalid"));
    }
    Ok(&id[7..])
}
fn converted(
    error: impl std::fmt::Display,
) -> crate::projection::provider::recovery::error::ProviderBootstrapCause {
    invalid(error.to_string())
}
