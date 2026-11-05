//! Creation coordination without a long-lived Store capability. The per-root
//! lease precedes the short registration gate, which in turn precedes Store IO.
use crate::{
    scm_v1::{NativeStoreError, NativeStoreOpen},
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_core::scm::StoreId;
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const PRIVATE_DIRECTORY: &str = "worktree-creation-v1";
// A root reservation spans physical population, unlike the short namespace
// transaction. Even a small journaled Windows checkout can exceed five seconds.
const CREATION_WAIT: Duration = Duration::from_secs(60);
const REGISTRATION_WAIT: Duration = Duration::from_secs(5);

/// Serializes reservation and physical creation of one filesystem directory.
/// Dropping it releases the OS lock, retaining all lock files and recovery data.
pub struct WorktreeCreationLease {
    store_id: StoreId,
    store: PinnedDirectory,
    metadata: PinnedDirectory,
    private: PinnedDirectory,
    checkout_root: PathBuf,
    checkout_identity: String,
    name: String,
    file: PinnedFile,
}

impl WorktreeCreationLease {
    /// The callback must verify the actual root using its backend's read-only
    /// identity guard. It runs before and after waiting, with no Store capability
    /// held. Callers must also release their own Store capabilities before entry.
    pub fn acquire(
        store_root: &Path,
        store_identity: &str,
        metadata_identity: &str,
        store_id: &StoreId,
        checkout_root: &Path,
        checkout_identity: &str,
        mut verify_checkout: impl FnMut(&Path) -> io::Result<String>,
    ) -> io::Result<Self> {
        let checkout_root = std::path::absolute(checkout_root)?;
        if verify_checkout(&checkout_root)? != checkout_identity {
            return Err(invalid("creation checkout identity changed"));
        }
        let (store, metadata, private) = {
            let capability = open_store(store_root)?
                .with_filesystem_identity(store_identity)?
                .with_metadata_identity(metadata_identity)?;
            if capability.store_id() != store_id {
                return Err(invalid("creation lease Store ID changed"));
            }
            let store = PinnedDirectory::open(store_root)?;
            let metadata = store.open_child_for_traversal("metadata".as_ref())?;
            if store.identity_token() != store_identity
                || metadata.identity_token() != metadata_identity
            {
                return Err(invalid("creation lease Store directory changed"));
            }
            let private = metadata.ensure_private_child(PRIVATE_DIRECTORY.as_ref())?;
            capability.verify_filesystem_identity()?;
            (store, metadata, private)
        }; // Release the Store before waiting for the per-root lock.
        let name = format!(
            "{}.lease",
            blake3::hash(checkout_identity.as_bytes()).to_hex()
        );
        let file = lock_file(&private, &name, CREATION_WAIT)?;
        let lease = Self {
            store_id: store_id.clone(),
            store,
            metadata,
            private,
            checkout_root,
            checkout_identity: checkout_identity.into(),
            name,
            file,
        };
        lease.verify()?;
        if verify_checkout(&lease.checkout_root)? != lease.checkout_identity {
            return Err(invalid("creation checkout identity changed while waiting"));
        }
        Ok(lease)
    }

    /// Acquire BEFORE opening the Store snapshot used for overlap and CAS reads.
    /// Retain through acceptance, then drop before physical materialization.
    pub fn registration_gate(&self) -> io::Result<WorktreeRegistrationGate<'_>> {
        self.verify()?;
        let file = lock_file(&self.private, "namespace.lease", REGISTRATION_WAIT)?;
        let gate = WorktreeRegistrationGate {
            creation: self,
            file,
        };
        gate.verify()?;
        Ok(gate)
    }

    /// Filesystem-only Store/metadata/private-lock binding check, safe during
    /// guarded acceptance. This does NOT prove the live checkout: callers retain
    /// and revalidate CheckoutReader or their phase-appropriate provider guard.
    pub fn verify(&self) -> io::Result<()> {
        self.store.verify_path_identity()?;
        self.metadata.verify_path_identity()?;
        self.private.verify_path_identity()?;
        if !self
            .file
            .same_file(&self.private.open_file(self.name.as_ref())?)
        {
            return Err(invalid("Worktree creation lease file was replaced"));
        }
        Ok(())
    }

    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }
    pub fn checkout_root(&self) -> &Path {
        &self.checkout_root
    }
    pub fn checkout_identity(&self) -> &str {
        &self.checkout_identity
    }
}

/// A short Store-wide namespace gate shared across Spaces and checkout backends.
/// It holds no SQLite connection or Store lock. Lock order is creation lease,
/// optional per-Worktree journal/checkout lease, this gate, then Store capability.
pub struct WorktreeRegistrationGate<'a> {
    creation: &'a WorktreeCreationLease,
    file: PinnedFile,
}

impl WorktreeRegistrationGate<'_> {
    /// Like creation.verify(), this excludes live checkout/provider proof.
    pub fn verify(&self) -> io::Result<()> {
        self.creation.verify()?;
        if !self.file.same_file(
            &self
                .creation
                .private
                .open_file("namespace.lease".as_ref())?,
        ) {
            return Err(invalid("Worktree registration gate file was replaced"));
        }
        Ok(())
    }
}

fn lock_file(directory: &PinnedDirectory, name: &str, wait: Duration) -> io::Result<PinnedFile> {
    directory.verify_path_identity()?;
    let file = match directory.create_file(name.as_ref()) {
        Ok(file) => {
            file.file().sync_all()?;
            directory.sync()?;
            file
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            directory.open_file(name.as_ref())?
        }
        Err(error) => return Err(error),
    };
    let deadline = Instant::now() + wait;
    loop {
        match fs2::FileExt::try_lock_exclusive(file.file()) {
            Ok(()) => break,
            Err(error) if contended(&error) => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "Worktree lease {name} remained busy for {} seconds: {error}",
                            wait.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    }
    directory.verify_path_identity()?;
    if !file.same_file(&directory.open_file(name.as_ref())?) {
        return Err(invalid("creation lock file changed while waiting"));
    }
    Ok(file)
}

fn contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)))
}

fn open_store(root: &Path) -> io::Result<NativeStoreOpen> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match crate::scm_v1::open(root) {
            Ok(store) => return Ok(store),
            Err(NativeStoreError::Lock(
                crate::StoreError::LockBusy(_) | crate::StoreError::ReentrantLock(_),
            )) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            Err(error) => return Err(io::Error::other(error)),
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
