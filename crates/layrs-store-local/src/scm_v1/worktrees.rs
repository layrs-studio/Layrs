//! Local checkout locators are hints until their complete binding is validated.
use super::{NativeScmReader, NativeStoreOpen};
use crate::secure_fs::PinnedDirectory;
use layrs_core::scm::{LayerId, OperationId, StepId, StoreId, Worktree, WorktreeId};
use layrs_store::scm::{ScmReader, StoreError};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const LOCATOR_SCHEMA: &str = "layrs.worktree-locator.v1";
const MAX_LOCATOR_BYTES: u64 = 32 * 1024;
const RESERVATION: &str = ".layrs.reservation";

#[path = "worktree_creation.rs"]
mod creation;
pub use creation::{WorktreeCreationLease, WorktreeRegistrationGate};

pub trait LocatorPublicationFaults {
    fn write_staged(&self, output: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
        output.write_all(bytes)
    }
    fn before_publication(&self) -> io::Result<()> {
        Ok(())
    }
}
pub struct NoLocatorPublicationFaults;
impl LocatorPublicationFaults for NoLocatorPublicationFaults {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeLocator {
    schema: String,
    pub store_root: PathBuf,
    pub store_id: StoreId,
    pub worktree_id: WorktreeId,
    pub checkout_root: PathBuf,
    pub filesystem_identity: String,
    pub store_filesystem_identity: String,
    pub store_metadata_identity: String,
    pub creation: WorktreeCreation,
}

/// Durable intent is sufficient to retry registration without inventing a new
/// identity or selecting a newer Step after a crash or optimistic conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeCreation {
    pub worktree: Worktree,
    pub source_worktree_id: WorktreeId,
    pub source_operation_id: OperationId,
    pub layer_id: LayerId,
    pub step_id: StepId,
}

/// Bounded listing from durable rows; every payload and scalar projection is
/// verified by the normal typed reader. No path or ID is inferred from a name.
pub fn registered_worktrees(store: &NativeStoreOpen) -> layrs_store::scm::Result<Vec<Worktree>> {
    let ids = {
        let connection = store.connection().map_err(backend)?;
        let mut query = connection
            .prepare("SELECT worktree_id FROM worktrees ORDER BY worktree_id LIMIT 16385")
            .map_err(backend)?;
        let ids = query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(backend)?;
        ids.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(backend)?
    };
    if ids.len() > 16384 {
        return Err(backend("Worktree listing limit exceeded"));
    }
    let reader = NativeScmReader::new(store)?;
    ids.into_iter()
        .map(|id| {
            let id = WorktreeId::parse(id).map_err(backend)?;
            reader
                .get_worktree(&id)?
                .ok_or_else(|| backend("registered Worktree disappeared"))
        })
        .collect()
}

/// Reserve the existing empty directory without replacing any entry. All
/// writes are directory-relative and flushed; failure leaves evidence intact.
pub fn write_locator(
    root: &Path,
    store_root: &Path,
    creation: &WorktreeCreation,
) -> io::Result<()> {
    let identity = PinnedDirectory::open(store_root)?.identity_token();
    write_locator_bound(
        root,
        store_root,
        &identity,
        creation,
        &NoLocatorPublicationFaults,
    )
}

pub fn write_locator_bound(
    root: &Path,
    store_root: &Path,
    store_identity: &str,
    creation: &WorktreeCreation,
    faults: &dyn LocatorPublicationFaults,
) -> io::Result<()> {
    let metadata_identity = PinnedDirectory::open(&store_root.join("metadata"))?.identity_token();
    write_locator_bound_metadata(
        root,
        store_root,
        store_identity,
        &metadata_identity,
        creation,
        faults,
    )
}

pub fn write_locator_bound_metadata(
    root: &Path,
    store_root: &Path,
    store_identity: &str,
    metadata_identity: &str,
    creation: &WorktreeCreation,
    faults: &dyn LocatorPublicationFaults,
) -> io::Result<()> {
    let worktree = &creation.worktree;
    let capability = super::repository::open_read_retry(store_root)
        .map_err(io::Error::other)?
        .with_filesystem_identity(store_identity)?
        .with_metadata_identity(metadata_identity)?;
    if capability.store_id() != worktree.store_id() {
        return Err(invalid("Worktree reservation Store ID changed"));
    }
    let directory = PinnedDirectory::open(root)?;
    if directory.identity_token() != worktree.filesystem_identity() {
        return Err(invalid("checkout filesystem identity changed"));
    }
    if std::fs::read_dir(root)?.next().transpose()?.is_some() {
        return Err(invalid("secondary Worktree directory must be empty"));
    }
    let store = PinnedDirectory::open(store_root)?;
    if store.identity_token() != store_identity || !directory.same_volume(&store) {
        return Err(invalid(
            "Worktree requires the original Store on the same filesystem volume",
        ));
    }
    let locator = WorktreeLocator {
        schema: LOCATOR_SCHEMA.into(),
        store_root: std::path::absolute(store_root)?,
        store_id: worktree.store_id().clone(),
        worktree_id: worktree.id().clone(),
        checkout_root: std::path::absolute(root)?,
        filesystem_identity: worktree.filesystem_identity().into(),
        store_filesystem_identity: store.identity_token(),
        store_metadata_identity: metadata_identity.to_owned(),
        creation: creation.clone(),
    };
    let bytes = serde_json::to_vec(&locator).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_LOCATOR_BYTES {
        return Err(invalid("Worktree locator too large"));
    }
    directory.verify_path_identity()?;
    capability.verify_filesystem_identity()?;
    // A short write stays outside the checkout. The existing empty directory
    // remains retryable and no incomplete JSON can become checkout authority.
    let staging = store
        .open_child_for_traversal("metadata".as_ref())?
        .ensure_child("worktree-intents".as_ref())?;
    let name = format!(
        "{}.json",
        blake3::hash(worktree.id().as_str().as_bytes()).to_hex()
    );
    let mut file = staging.create_file(name.as_ref())?;
    faults.write_staged(file.file_mut(), &bytes)?;
    file.file().sync_all()?;
    staging.sync()?;
    directory.verify_path_identity()?;
    store.verify_path_identity()?;
    capability.verify_filesystem_identity()?;
    if std::fs::read_dir(root)?.next().transpose()?.is_some() {
        return Err(invalid(
            "secondary Worktree directory changed during reservation",
        ));
    }
    if file.rename_to(&directory, RESERVATION.as_ref())? {
        return Err(invalid("Worktree reservation already exists"));
    }
    directory.sync()?;
    staging.sync()?;
    faults.before_publication()?;
    capability.verify_filesystem_identity()?;
    publish_reservation(root)?;
    directory.verify_path_identity()?;
    store.verify_path_identity()?;
    capability.verify_filesystem_identity()?;
    Ok(())
}

pub fn read_locator(root: &Path) -> io::Result<WorktreeLocator> {
    read_locator_named(root, ".layrs")
}

/// Explicit recovery accepts only a complete, identity-bound staged intent.
/// A colliding final marker is never replaced, even if it is malformed.
pub fn resume_locator(root: &Path) -> io::Result<WorktreeLocator> {
    match std::fs::symlink_metadata(root.join(".layrs")) {
        Ok(_) => read_locator(root),
        Err(error) if error.kind() == io::ErrorKind::NotFound => publish_reservation(root),
        Err(error) => Err(error),
    }
}

/// Read the exact pending intent without publishing it. Acquire its creation
/// lease before calling resume_locator or accepting any registration.
pub fn read_creation_locator(root: &Path) -> io::Result<WorktreeLocator> {
    match read_locator(root) {
        Ok(locator) => Ok(locator),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match read_locator_named(root, RESERVATION) {
                Ok(locator) => Ok(locator),
                // Another resumer may have atomically promoted the reservation
                // before this caller acquires its per-checkout creation lease.
                Err(error) if error.kind() == io::ErrorKind::NotFound => read_locator(root),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn publish_reservation(root: &Path) -> io::Result<WorktreeLocator> {
    let locator = read_locator_named(root, RESERVATION)?;
    let directory = PinnedDirectory::open(root)?;
    if directory.identity_token() != locator.filesystem_identity {
        return Err(invalid("Worktree reservation root was replaced"));
    }
    let file = directory.open_file(RESERVATION.as_ref())?;
    // Re-read through the pinned handle used for publication, so replacement
    // between the first validation and opening this handle cannot be promoted.
    let checked = read_locator_file(&directory, &file, RESERVATION)?;
    if checked.worktree_id != locator.worktree_id {
        return Err(invalid("Worktree reservation changed"));
    }
    if file.rename_to(&directory, ".layrs".as_ref())? {
        return Err(invalid(
            "Worktree locator already exists; reservation preserved",
        ));
    }
    directory.sync()?;
    read_locator(root)
}

fn read_locator_named(root: &Path, name: &str) -> io::Result<WorktreeLocator> {
    let root = std::path::absolute(root)?;
    let directory = PinnedDirectory::open(&root)?;
    let file = directory.open_file(name.as_ref())?;
    read_locator_file(&directory, &file, name)
}

fn read_locator_file(
    directory: &PinnedDirectory,
    file: &crate::secure_fs::PinnedFile,
    name: &str,
) -> io::Result<WorktreeLocator> {
    let mut bytes = Vec::new();
    file.file()
        .take(MAX_LOCATOR_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LOCATOR_BYTES {
        return Err(invalid("Worktree locator too large"));
    }
    let locator: WorktreeLocator = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if locator.schema != LOCATOR_SCHEMA
        || !locator.checkout_root.is_absolute()
        || locator.filesystem_identity != directory.identity_token()
        || !locator.store_root.is_absolute()
    {
        return Err(invalid("Worktree locator does not identify this checkout"));
    }
    if locator.creation.worktree.id() != &locator.worktree_id
        || locator.creation.worktree.store_id() != &locator.store_id
        || Path::new(locator.creation.worktree.checkout_root()) != locator.checkout_root
        || locator.creation.worktree.filesystem_identity() != locator.filesystem_identity
    {
        return Err(invalid("Worktree creation intent does not match locator"));
    }
    // The caller may use another spelling of the same directory. Both its
    // pinned handle and the unchanged recorded path must identify this root.
    let recorded = PinnedDirectory::open(&locator.checkout_root)?;
    if recorded.identity_token() != locator.filesystem_identity {
        return Err(invalid(
            "Worktree locator recorded checkout identity changed",
        ));
    }
    let store = PinnedDirectory::open(&locator.store_root)?;
    if store.identity_token() != locator.store_filesystem_identity {
        return Err(invalid("Worktree locator Store directory identity changed"));
    }
    if store
        .open_child_for_traversal("metadata".as_ref())?
        .identity_token()
        != locator.store_metadata_identity
    {
        return Err(invalid(
            "Worktree locator Store metadata directory identity changed",
        ));
    }
    directory.verify_path_identity()?;
    recorded.verify_path_identity()?;
    if !file.same_file(&directory.open_file(name.as_ref())?) {
        return Err(invalid("Worktree locator replaced while opening"));
    }
    Ok(locator)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend {
        message: error.to_string(),
    }
}

/// Filesystem identities, rather than lexical case/spelling, decide ancestry.
/// Missing registered roots remain protected by their recorded path as well.
pub fn validate_secondary_root(store: &NativeStoreOpen, root: &Path) -> io::Result<()> {
    validate_secondary_root_with_vfs(store, root, |_| {
        Err(invalid(
            "registered VFS root requires verified provider registration",
        ))
    })
}

/// The callback must verify the durable provider ticket against the actual root
/// without connecting/mutating the provider. A backend name alone is no proof.
pub fn validate_secondary_root_with_vfs(
    store: &NativeStoreOpen,
    root: &Path,
    mut verify_vfs: impl FnMut(&Worktree) -> io::Result<String>,
) -> io::Result<()> {
    let root_path = std::path::absolute(root)?;
    let root = PinnedDirectory::open(&root_path)?;
    let store_directory = PinnedDirectory::open(store.layout().root())?;
    if !root.same_volume(&store_directory) {
        return Err(invalid(
            "Worktree and Store must share a filesystem volume for journaled materialization",
        ));
    }
    let root_identity = root.identity_token();
    let mut ancestors = Vec::new();
    // The root is already absolute through CheckoutReader at the client boundary.
    for path in root_path.ancestors() {
        ancestors.push(PinnedDirectory::open(path)?.identity_token());
    }
    for registered in registered_worktrees(store).map_err(io::Error::other)? {
        let existing = Path::new(registered.checkout_root());
        if root_path.starts_with(existing)
            || existing.starts_with(&root_path)
            || ancestors
                .iter()
                .any(|identity| identity == registered.filesystem_identity())
        {
            return Err(invalid("Worktree roots must not overlap"));
        }
        let backend = registered
            .metadata()
            .entries()
            .iter()
            .find(|entry| entry.key() == "layrs.worktree.backend");
        let ancestry_start = match backend.map(|entry| entry.value()) {
            Some("windows-cfapi-v1") => {
                if verify_vfs(&registered)? != registered.filesystem_identity() {
                    return Err(invalid(
                        "verified VFS root identity differs from registered Worktree",
                    ));
                }
                existing
                    .parent()
                    .ok_or_else(|| invalid("VFS root has no parent"))?
            }
            Some(_) => return Err(invalid("unsupported registered Worktree backend")),
            None => existing,
        };
        for parent in ancestry_start.ancestors() {
            match PinnedDirectory::open(parent) {
                Ok(directory) => {
                    let identity = directory.identity_token();
                    // A removed registered root may have been recreated with a
                    // different identity. Its recorded namespace stays reserved,
                    // including when the caller spells Windows components differently.
                    if identity == root_identity
                        || (parent == existing && ancestors.contains(&identity))
                    {
                        return Err(invalid("Worktree roots must not overlap"));
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }
    root.verify_path_identity()
}
