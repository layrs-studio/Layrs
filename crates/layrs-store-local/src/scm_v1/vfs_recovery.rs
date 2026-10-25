//! Private, append-only VFS lifecycle evidence. This journal never holds the
//! native Store lock across provider callbacks or a repository acceptance.
//!
//! A pending capture is resolved from the repository's committed receipt. If no
//! receipt exists, recovery abandons that *attempt*, preserving every user file.
//! It never reconstructs an unaccepted capture from a later filesystem state.
use super::{sqlite::validate_canonical_receipt_json, write_batch::ApplyBatchReceipt};
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use layrs_core::scm::{OperationId, StepId, StoreId, WorktreeId};
use layrs_store::scm::{StoreMutation, WriteBatch};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

const SCHEMA: &str = "layrs.vfs-journal.v1";
const MAX_RECORD_BYTES: u64 = 256 * 1024;
const MAX_RECORDS: usize = 65_536;

fn require_append_capacity(sequence: usize, limit: usize, event: &VfsEvent) -> io::Result<()> {
    if matches!(
        event,
        VfsEvent::CapturePrepared { .. } | VfsEvent::ActivationPrepared { .. }
    ) {
        // The accepted transaction must always have room for its acknowledgment.
        require_capture_slots(sequence, limit)?;
    }
    require_replay_capacity(sequence, limit, event)
}

fn require_capture_slots(sequence: usize, limit: usize) -> io::Result<()> {
    if sequence.checked_add(2).is_none_or(|end| end > limit) {
        return Err(invalid("VFS journal capacity exhausted; capture is disabled, preserve work with recovery export"));
    }
    Ok(())
}

fn require_replay_capacity(sequence: usize, limit: usize, event: &VfsEvent) -> io::Result<()> {
    // A previous reader could accept a final-slot CapturePrepared. Permit one
    // terminal resolution only; transition() still requires its exact pending
    // transaction, and resolve_capture() verifies the committed receipt first.
    if sequence > limit
        || (sequence == limit && !matches!(event, VfsEvent::CaptureResolved { accepted: true, .. }))
    {
        return Err(invalid("VFS journal record limit exceeded"));
    }
    Ok(())
}

/// Keep the native failure available to callers while identifying the durable
/// boundary that failed. Context never includes journal or endpoint contents.
#[derive(Debug)]
struct JournalIoError {
    stage: &'static str,
    source: io::Error,
}

impl std::fmt::Display for JournalIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VFS journal {}: {}", self.stage, self.source)
    }
}

impl std::error::Error for JournalIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

trait JournalIoContext<T> {
    fn journal_context(self, stage: &'static str) -> io::Result<T>;
}

impl<T> JournalIoContext<T> for io::Result<T> {
    fn journal_context(self, stage: &'static str) -> io::Result<T> {
        self.map_err(|source| io::Error::new(source.kind(), JournalIoError { stage, source }))
    }
}

/// Retry only typed transient contention. Identity checks are performed on the
/// acquired capability before it can expose metadata; no lock survives a retry.
fn open_bound_store(
    root: &Path,
    identity: &str,
    metadata_identity: &str,
) -> io::Result<super::NativeStoreOpen> {
    open_bound_store_wait(root, identity, metadata_identity, &mut || {
        std::thread::sleep(Duration::from_millis(2));
    })
}

fn open_bound_store_wait(
    root: &Path,
    identity: &str,
    metadata_identity: &str,
    wait_for_retry: &mut impl FnMut(),
) -> io::Result<super::NativeStoreOpen> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match super::open(root) {
            Ok(store) => {
                return store
                    .with_filesystem_identity(identity)
                    .journal_context("bind Store directory")?
                    .with_metadata_identity(metadata_identity)
                    .journal_context("bind Store metadata");
            }
            Err(super::NativeStoreError::Lock(
                crate::StoreError::LockBusy(_) | crate::StoreError::ReentrantLock(_),
            )) if Instant::now() < deadline => {
                wait_for_retry();
            }
            Err(error) => return Err(io::Error::other(error)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VfsCaptureIntent {
    pub transaction_id: String,
    pub request_digest: String,
    pub operation_id: OperationId,
    pub step_id: StepId,
    receipt: ApplyBatchReceipt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VfsActivationIntent {
    pub transaction_id: String,
    pub request_digest: String,
    pub operation_id: OperationId,
    pub step_id: StepId,
    pub expected_registration_operation_id: OperationId,
    receipt: ApplyBatchReceipt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum VfsEvent {
    /// The caller validates this versioned locator before using its paths/IDs.
    Reserved {
        locator: Vec<u8>,
    },
    Registered,
    /// Written by the provider's pre-registration persistence hook.
    ProviderPrepared {
        ticket: Vec<u8>,
    },
    ProviderPopulated,
    /// Exact flushed staged-file identity, before publication into the checkout.
    ControlPrepared {
        control: Vec<u8>,
    },
    LocatorPublished {
        control: Vec<u8>,
    },
    ActivationPrepared {
        intent: VfsActivationIntent,
    },
    ActivationResolved {
        transaction_id: String,
        accepted: bool,
    },
    /// Replay-only legacy readiness has no provider-owned activation proof.
    /// Status/export remain available; a later exact accepted capture receipt
    /// is required before the legacy history can establish a capture baseline.
    Ready,
    CapturePrepared {
        intent: VfsCaptureIntent,
    },
    CaptureResolved {
        transaction_id: String,
        accepted: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfsPhase {
    Reserved,
    Registered,
    ProviderPrepared,
    ProviderPopulated,
    ControlPrepared,
    LocatorPublished,
    ActivationPrepared,
    Ready,
}

#[derive(Clone, Debug)]
pub struct VfsRecoveryState {
    pub phase: VfsPhase,
    pub locator: Vec<u8>,
    pub ticket: Option<Vec<u8>>,
    pub control: Option<Vec<u8>>,
    pub pending_activation: Option<VfsActivationIntent>,
    pub activation: Option<VfsActivationIntent>,
    pub pending_capture: Option<VfsCaptureIntent>,
    pub last_accepted: Option<VfsCaptureIntent>,
}

impl VfsRecoveryState {
    /// Replayed accepted receipts establish the last provider-owned acceptance,
    /// including for legacy Ready journals. Pending intents and current Store
    /// state never confer lineage authority.
    pub fn provider_capture_baseline(&self) -> Option<(&OperationId, &StepId)> {
        self.last_accepted
            .as_ref()
            .map(|intent| (&intent.operation_id, &intent.step_id))
            .or_else(|| {
                self.activation
                    .as_ref()
                    .map(|intent| (&intent.operation_id, &intent.step_id))
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema: String,
    sequence: usize,
    previous_digest: String,
    store_id: StoreId,
    worktree_id: WorktreeId,
    event: VfsEvent,
}

/// Failure seams leave flushed evidence in place; none authorizes rollback.
pub trait VfsJournalFaults {
    fn write_staged(&self, file: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
        file.write_all(bytes)
    }
    fn after_event(&self, _event: &VfsEvent) -> io::Result<()> {
        Ok(())
    }
}
pub struct NoVfsJournalFaults;
impl VfsJournalFaults for NoVfsJournalFaults {}

/// Holds only pinned directories and a per-Worktree process lease. Dropping it
/// releases the lease; it never deletes the journal, registration or checkout.
pub struct VfsRecovery {
    store_root: PathBuf,
    store_identity: String,
    metadata_identity: String,
    store_id: StoreId,
    worktree_id: WorktreeId,
    store_directory: PinnedDirectory,
    directory: PinnedDirectory,
    _lease: PinnedFile,
    next_sequence: usize,
    record_limit: usize,
    previous_digest: String,
    state: Option<VfsRecoveryState>,
}

impl VfsRecovery {
    pub fn create(
        store_root: &Path,
        store_identity: &str,
        metadata_identity: &str,
        store_id: &StoreId,
        worktree_id: &WorktreeId,
        locator: Vec<u8>,
        faults: &dyn VfsJournalFaults,
    ) -> io::Result<Self> {
        let mut journal = Self::acquire(
            store_root,
            store_identity,
            metadata_identity,
            store_id,
            worktree_id,
            true,
        )?;
        if journal.state.is_some() {
            return Err(invalid("VFS journal already exists; use explicit recovery"));
        }
        journal.append(VfsEvent::Reserved { locator }, faults)?;
        Ok(journal)
    }

    pub fn open(
        store_root: &Path,
        store_identity: &str,
        metadata_identity: &str,
        store_id: &StoreId,
        worktree_id: &WorktreeId,
    ) -> io::Result<Self> {
        let journal = Self::acquire(
            store_root,
            store_identity,
            metadata_identity,
            store_id,
            worktree_id,
            false,
        )?;
        if journal.state.is_none() {
            return Err(invalid("VFS journal has no complete reservation"));
        }
        Ok(journal)
    }

    fn acquire(
        store_root: &Path,
        store_identity: &str,
        metadata_identity: &str,
        store_id: &StoreId,
        worktree_id: &WorktreeId,
        create: bool,
    ) -> io::Result<Self> {
        Self::acquire_bounded(
            store_root,
            store_identity,
            metadata_identity,
            store_id,
            worktree_id,
            create,
            MAX_RECORDS,
        )
    }

    // Production always supplies MAX_RECORDS. A smaller private limit lets fault
    // tests exercise the same durable boundary without creating 65,536 files.
    fn acquire_bounded(
        store_root: &Path,
        store_identity: &str,
        metadata_identity: &str,
        store_id: &StoreId,
        worktree_id: &WorktreeId,
        create: bool,
        record_limit: usize,
    ) -> io::Result<Self> {
        if record_limit == 0 || record_limit > MAX_RECORDS {
            return Err(invalid("invalid VFS journal record bound"));
        }
        let store = open_bound_store(store_root, store_identity, metadata_identity)
            .journal_context("acquire Store")?;
        if store.store_id() != store_id {
            return Err(invalid("VFS journal Store identity changed"));
        }
        let store_directory =
            PinnedDirectory::open(store_root).journal_context("pin Store directory")?;
        if store_directory.identity_token() != store_identity {
            return Err(invalid("VFS journal Store directory replaced"));
        }
        let metadata = store_directory
            .open_child_for_traversal("metadata".as_ref())
            .journal_context("open Store metadata")?;
        if metadata.identity_token() != metadata_identity {
            return Err(invalid("VFS metadata directory changed"));
        }
        let private = if create {
            metadata
                .ensure_private_child("vfs-v1".as_ref())
                .journal_context("create private container")?
        } else {
            metadata
                .open_private_child("vfs-v1".as_ref())
                .journal_context("reopen private container")?
        };
        let key = blake3::hash(worktree_id.as_str().as_bytes())
            .to_hex()
            .to_string();
        let directory = if create {
            private
                .ensure_private_child(key.as_ref())
                .journal_context("create private Worktree directory")?
        } else {
            private
                .open_private_child_for_write(key.as_ref())
                .journal_context("reopen private Worktree directory")?
        };
        let lease = match directory.create_file("lease".as_ref()) {
            Ok(file) => {
                file.file()
                    .sync_all()
                    .journal_context("flush new host lease")?;
                directory
                    .sync()
                    .journal_context("flush host lease directory")?;
                file
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => directory
                .open_file("lease".as_ref())
                .journal_context("reopen host lease")?,
            Err(error) => return Err(error).journal_context("create host lease"),
        };
        fs2::FileExt::try_lock_exclusive(lease.file())
            .journal_context("acquire exclusive host lease")?;
        drop(store);
        let path = store_root.join("metadata").join("vfs-v1").join(&key);
        let (state, next_sequence, previous_digest) = read_records_bounded(
            &directory,
            &path,
            store_id,
            worktree_id,
            record_limit,
            &mut |transaction_id, request_digest, receipt| {
                let store = open_bound_store(store_root, store_identity, metadata_identity)?;
                if store.store_id() != store_id
                    || !acceptance_receipt_matches(&store, transaction_id, request_digest, receipt)?
                {
                    return Err(invalid(
                        "VFS replay lacks its exact committed acceptance receipt",
                    ));
                }
                Ok(())
            },
        )
        .journal_context("replay durable records")?;
        let journal = Self {
            store_root: store_root.into(),
            store_identity: store_identity.into(),
            metadata_identity: metadata_identity.into(),
            store_id: store_id.clone(),
            worktree_id: worktree_id.clone(),
            store_directory,
            directory,
            _lease: lease,
            next_sequence,
            record_limit,
            previous_digest,
            state,
        };
        journal.verify()?;
        Ok(journal)
    }

    /// Non-owning read of a published immutable journal prefix. It acquires no
    /// host lease and performs no writes, including while a host is connected.
    pub fn read_registration(
        store: &super::NativeStoreOpen,
        worktree: &layrs_core::scm::Worktree,
    ) -> io::Result<VfsRecoveryState> {
        store.verify_filesystem_identity()?;
        if store.store_id() != worktree.store_id() {
            return Err(invalid("foreign VFS registration Store"));
        }
        Self::read_reservation(store, worktree.id())
    }

    /// Reads even a pre-registration reservation, without its host lease. Use
    /// the result only after validating its locator and Store identity binding.
    pub fn read_reservation(
        store: &super::NativeStoreOpen,
        worktree_id: &WorktreeId,
    ) -> io::Result<VfsRecoveryState> {
        store.verify_filesystem_identity()?;
        let root = PinnedDirectory::open(store.layout().root())?;
        let metadata = root.open_child_for_traversal("metadata".as_ref())?;
        let private = metadata
            .open_private_child("vfs-v1".as_ref())
            .journal_context("inspect registration private container")?;
        let key = blake3::hash(worktree_id.as_str().as_bytes())
            .to_hex()
            .to_string();
        let directory = private
            .open_private_child(key.as_ref())
            .journal_context("inspect registration private Worktree directory")?;
        let path = store.layout().metadata_dir().join("vfs-v1").join(key);
        let (state, _, _) = read_records(&directory, &path, store, worktree_id)
            .journal_context("inspect registration durable records")?;
        directory.verify_path_identity()?;
        metadata.verify_path_identity()?;
        root.verify_path_identity()?;
        store.verify_filesystem_identity()?;
        state.ok_or_else(|| invalid("VFS registration has no complete reservation"))
    }

    /// Namespace metadata only; never content-availability or reference-reuse
    /// authority. NativeScmReader validates the canonical manifest/projections.
    pub fn file_length_metadata(
        store: &super::NativeStoreOpen,
        id: &layrs_core::scm::FileId,
    ) -> io::Result<u64> {
        use layrs_store::scm::ScmReader;
        let manifest_id: Option<String> = store.connection().map_err(io::Error::other)?
            .query_row("SELECT manifest_id FROM file_manifests WHERE file_id=?1 ORDER BY manifest_id LIMIT 1",
                [id.as_str()], |row| row.get(0)).optional().map_err(io::Error::other)?;
        let manifest_id = layrs_core::scm::FileManifestId::parse(
            manifest_id.ok_or_else(|| invalid("VFS file manifest metadata missing"))?,
        )
        .map_err(io::Error::other)?;
        let reader = super::NativeScmReader::new(store).map_err(io::Error::other)?;
        let manifest = reader
            .get_file_manifest(&manifest_id)
            .map_err(io::Error::other)?
            .ok_or_else(|| invalid("VFS file manifest missing"))?;
        if manifest.file_id() != id {
            return Err(invalid("VFS file manifest identity differs"));
        }
        Ok(manifest.byte_length())
    }

    pub fn state(&self) -> &VfsRecoveryState {
        self.state.as_ref().expect("public journal always reserved")
    }

    /// Check before hashing/checkpoint work as well as before writing the intent.
    /// An exhausted legacy pending attempt stays visible for recovery/export.
    pub fn ensure_capture_capacity(&self) -> io::Result<()> {
        self.verify()?;
        require_capture_slots(self.next_sequence, self.record_limit)
    }

    pub fn append(&mut self, event: VfsEvent, faults: &dyn VfsJournalFaults) -> io::Result<()> {
        if matches!(event, VfsEvent::Ready) {
            return Err(invalid(
                "legacy VFS Ready is replay-only; use receipt-backed activation",
            ));
        }
        if matches!(
            event,
            VfsEvent::CapturePrepared { .. }
                | VfsEvent::CaptureResolved { .. }
                | VfsEvent::ActivationPrepared { .. }
                | VfsEvent::ActivationResolved { .. }
        ) {
            return Err(invalid(
                "capture transitions require prepared batch and verified receipt",
            ));
        }
        self.append_checked(event, faults)
    }

    fn append_checked(&mut self, event: VfsEvent, faults: &dyn VfsJournalFaults) -> io::Result<()> {
        self.verify()?;
        if matches!(event, VfsEvent::LocatorPublished { .. })
            && self.state().phase != VfsPhase::ControlPrepared
        {
            return Err(invalid(
                "VFS locator publication requires durable control preparation",
            ));
        }
        require_append_capacity(self.next_sequence, self.record_limit, &event)?;
        let next = transition(self.state.as_ref(), &event)?;
        let record = Record {
            schema: SCHEMA.into(),
            sequence: self.next_sequence,
            previous_digest: self.previous_digest.clone(),
            store_id: self.store_id.clone(),
            worktree_id: self.worktree_id.clone(),
            event,
        };
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        self.publish(&self.record_name(), &bytes, faults)?;
        self.previous_digest = blake3::hash(&bytes).to_hex().to_string();
        self.next_sequence += 1;
        self.state = Some(next);
        faults
            .after_event(&record.event)
            .journal_context("after durable event")
    }

    pub fn prepare_capture(
        &mut self,
        batch: &WriteBatch,
        step_id: &StepId,
        faults: &dyn VfsJournalFaults,
    ) -> io::Result<VfsCaptureIntent> {
        let advances: Vec<_> = batch
            .mutations()
            .iter()
            .filter_map(|mutation| match mutation {
                StoreMutation::WorktreeActiveOperation(cas) => Some(cas),
                _ => None,
            })
            .collect();
        let [advance] = advances.as_slice() else {
            return Err(invalid("VFS capture must advance exactly one Worktree"));
        };
        if advance.worktree_id() != &self.worktree_id {
            return Err(invalid("VFS capture advances a foreign Worktree"));
        }
        let intent = VfsCaptureIntent {
            transaction_id: batch.transaction_id().to_string(),
            request_digest: batch.request_digest().to_string(),
            operation_id: advance.new_operation_id().clone(),
            step_id: step_id.clone(),
            receipt: ApplyBatchReceipt::new(batch),
        };
        self.append_checked(
            VfsEvent::CapturePrepared {
                intent: intent.clone(),
            },
            faults,
        )?;
        Ok(intent)
    }

    pub fn prepare_activation(
        &mut self,
        batch: &WriteBatch,
        step_id: &StepId,
        expected_registration_operation_id: &OperationId,
        faults: &dyn VfsJournalFaults,
    ) -> io::Result<VfsActivationIntent> {
        let advances: Vec<_> = batch
            .mutations()
            .iter()
            .filter_map(|mutation| match mutation {
                StoreMutation::WorktreeActiveOperation(cas) => Some(cas),
                _ => None,
            })
            .collect();
        let [advance] = advances.as_slice() else {
            return Err(invalid("VFS activation must advance exactly one Worktree"));
        };
        if advance.worktree_id() != &self.worktree_id
            || advance.expected_operation_id() != expected_registration_operation_id
        {
            return Err(invalid(
                "VFS activation differs from its original registration",
            ));
        }
        let intent = VfsActivationIntent {
            transaction_id: batch.transaction_id().to_string(),
            request_digest: batch.request_digest().to_string(),
            operation_id: advance.new_operation_id().clone(),
            step_id: step_id.clone(),
            expected_registration_operation_id: expected_registration_operation_id.clone(),
            receipt: ApplyBatchReceipt::new(batch),
        };
        self.append_checked(
            VfsEvent::ActivationPrepared {
                intent: intent.clone(),
            },
            faults,
        )?;
        Ok(intent)
    }

    /// A committed receipt is the only authority for the initial provider-owned
    /// Operation. Absence permits a retry against the original registration;
    /// it never adopts whichever Operation happens to be current on restart.
    pub fn resolve_activation(
        &mut self,
        faults: &dyn VfsJournalFaults,
    ) -> io::Result<Option<bool>> {
        let Some(intent) = self.state().pending_activation.clone() else {
            return Ok(None);
        };
        self.verify()?;
        let store = open_bound_store(
            &self.store_root,
            &self.store_identity,
            &self.metadata_identity,
        )?;
        if store.store_id() != &self.store_id {
            return Err(invalid("VFS activation Store changed"));
        }
        let accepted = acceptance_receipt_matches(
            &store,
            &intent.transaction_id,
            &intent.request_digest,
            &intent.receipt,
        )?;
        drop(store);
        self.append_checked(
            VfsEvent::ActivationResolved {
                transaction_id: intent.transaction_id,
                accepted,
            },
            faults,
        )
        .journal_context("publish activation receipt acknowledgment")?;
        Ok(Some(accepted))
    }

    /// Returns Some(true) for an accepted capture, Some(false) for an attempt
    /// without a commit, and None when no acknowledgment was pending. A legacy
    /// full journal without a receipt retains its pending intent: it can reopen
    /// and export, but ensure_capture_capacity() refuses every new capture.
    pub fn resolve_capture(&mut self, faults: &dyn VfsJournalFaults) -> io::Result<Option<bool>> {
        let Some(intent) = self.state().pending_capture.clone() else {
            return Ok(None);
        };
        self.verify()?;
        let store = open_bound_store(
            &self.store_root,
            &self.store_identity,
            &self.metadata_identity,
        )
        .journal_context("open receipt Store")?;
        if store.store_id() != &self.store_id {
            return Err(invalid("VFS Store identity changed"));
        }
        let accepted = capture_receipt_matches(&store, &intent)?;
        drop(store);
        if !accepted && self.next_sequence >= self.record_limit {
            // Do not invent an acknowledgment or consume the one repair slot
            // without evidence of acceptance. Status must keep reporting pending.
            return Ok(Some(false));
        }
        self.append_checked(
            VfsEvent::CaptureResolved {
                transaction_id: intent.transaction_id,
                accepted,
            },
            faults,
        )
        .journal_context("publish capture receipt acknowledgment")?;
        Ok(Some(accepted))
    }

    /// Bounded control endpoint data inherits the journal's owner-only ACL.
    /// Names are immutable and supplied as random UUIDs, never file paths.
    pub fn publish_endpoint(&self, id: &str, bytes: &[u8]) -> io::Result<()> {
        if id.len() != 36 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
            return Err(invalid("invalid VFS endpoint ID"));
        }
        self.publish(&format!("endpoint-{id}"), bytes, &NoVfsJournalFaults)
    }

    pub fn endpoint_path(&self, id: &str) -> io::Result<PathBuf> {
        if id.len() != 36 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
            return Err(invalid("invalid VFS endpoint ID"));
        }
        Ok(self
            .store_root
            .join("metadata/vfs-v1")
            .join(
                blake3::hash(self.worktree_id.as_str().as_bytes())
                    .to_hex()
                    .as_str(),
            )
            .join(format!("endpoint-{id}")))
    }

    /// The provider must independently pin this existing private directory and
    /// require its volume to match the checkout before staging any control file.
    pub fn staging_root(&self) -> io::Result<PathBuf> {
        self.verify()
            .journal_context("verify control staging directory")?;
        Ok(self.store_root.join("metadata/vfs-v1").join(
            blake3::hash(self.worktree_id.as_str().as_bytes())
                .to_hex()
                .as_str(),
        ))
    }

    /// Read a host endpoint without acquiring its live host lease. The caller
    /// supplies the complete Store/Worktree binding, never an arbitrary path.
    pub fn read_endpoint(
        store_root: &Path,
        store_identity: &str,
        metadata_identity: &str,
        store_id: &StoreId,
        worktree_id: &WorktreeId,
        id: &str,
    ) -> io::Result<Vec<u8>> {
        if id.len() != 36 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
            return Err(invalid("invalid VFS endpoint ID"));
        }
        let store = open_bound_store(store_root, store_identity, metadata_identity)
            .journal_context("open endpoint Store")?;
        if store.store_id() != store_id {
            return Err(invalid("VFS endpoint Store changed"));
        }
        let root = PinnedDirectory::open(store_root)?;
        if root.identity_token() != store_identity {
            return Err(invalid("VFS endpoint directory changed"));
        }
        let metadata = root.open_child_for_traversal("metadata".as_ref())?;
        if metadata.identity_token() != metadata_identity {
            return Err(invalid("VFS metadata replaced"));
        }
        let private = metadata
            .open_private_child("vfs-v1".as_ref())
            .journal_context("read endpoint private container")?;
        let key = blake3::hash(worktree_id.as_str().as_bytes())
            .to_hex()
            .to_string();
        let directory = private
            .open_private_child(key.as_ref())
            .journal_context("read endpoint private Worktree directory")?;
        let name = format!("endpoint-{id}");
        let file = directory
            .open_file(name.as_ref())
            .journal_context("open endpoint file")?;
        let bytes =
            read_bounded(file.file(), 16 * 1024).journal_context("read bounded endpoint")?;
        directory.verify_path_identity()?;
        root.verify_path_identity()?;
        Ok(bytes)
    }

    fn record_name(&self) -> String {
        format!("{:011}.json", self.next_sequence)
    }

    fn verify(&self) -> io::Result<()> {
        self.store_directory.verify_path_identity()?;
        self.directory.verify_path_identity()?;
        let metadata = self
            .store_directory
            .open_child_for_traversal("metadata".as_ref())?;
        if metadata.identity_token() != self.metadata_identity
            || !self
                ._lease
                .same_file(&self.directory.open_file("lease".as_ref())?)
        {
            return Err(invalid("VFS metadata or host lease replaced"));
        }
        if self.store_directory.identity_token() != self.store_identity {
            return Err(invalid("VFS journal Store replaced"));
        }
        Ok(())
    }

    fn publish(&self, name: &str, bytes: &[u8], faults: &dyn VfsJournalFaults) -> io::Result<()> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        self.verify()?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid("VFS record too large"));
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let staged = format!(
            "staged-{}-{nonce}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = self
            .directory
            .create_file(staged.as_ref())
            .journal_context("create staged record")?;
        faults
            .write_staged(file.file_mut(), bytes)
            .journal_context("write staged record")?;
        file.file()
            .sync_all()
            .journal_context("flush staged record")?;
        self.directory
            .sync()
            .journal_context("flush staged record directory")?;
        self.verify()?;
        if file
            .rename_to(&self.directory, name.as_ref())
            .journal_context("publish immutable record")?
        {
            return Err(invalid("VFS journal publication collision"));
        }
        self.directory
            .sync()
            .journal_context("flush published record directory")?;
        self.verify()
    }
}

fn read_records(
    directory: &PinnedDirectory,
    path: &Path,
    store: &super::NativeStoreOpen,
    worktree_id: &WorktreeId,
) -> io::Result<(Option<VfsRecoveryState>, usize, String)> {
    read_records_bounded(
        directory,
        path,
        store.store_id(),
        worktree_id,
        MAX_RECORDS,
        &mut |transaction_id, request_digest, receipt| {
            if !acceptance_receipt_matches(store, transaction_id, request_digest, receipt)? {
                return Err(invalid(
                    "VFS replay lacks its exact committed acceptance receipt",
                ));
            }
            Ok(())
        },
    )
}

fn read_records_bounded(
    directory: &PinnedDirectory,
    path: &Path,
    store_id: &StoreId,
    worktree_id: &WorktreeId,
    record_limit: usize,
    verify_receipt: &mut impl FnMut(&str, &str, &ApplyBatchReceipt) -> io::Result<()>,
) -> io::Result<(Option<VfsRecoveryState>, usize, String)> {
    let mut names = Vec::new();
    for (count, item) in std::fs::read_dir(path)?.enumerate() {
        if count > MAX_RECORDS * 3 {
            return Err(invalid("VFS journal directory exceeds limit"));
        }
        let name = item?
            .file_name()
            .into_string()
            .map_err(|_| invalid("non-Unicode VFS journal entry"))?;
        if name == "lease" || name.starts_with("staged-") || name.starts_with("endpoint-") {
            continue;
        }
        if name.len() != 16
            || !name.ends_with(".json")
            || !name[..11].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(invalid("unrecognized VFS journal entry"));
        }
        names.push(name);
        if names.len() > record_limit + 1 {
            return Err(invalid("VFS journal record limit exceeded"));
        }
    }
    names.sort();
    let mut state = None;
    let mut previous_digest = String::new();
    let count = names.len();
    for (sequence, name) in names.into_iter().enumerate() {
        if name != format!("{sequence:011}.json") {
            return Err(invalid("VFS journal sequence gap"));
        }
        let file = directory.open_file(name.as_ref())?;
        let bytes = read_bounded(file.file(), MAX_RECORD_BYTES)?;
        let record: Record = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        require_replay_capacity(sequence, record_limit, &record.event)?;
        if record.schema != SCHEMA
            || record.sequence != sequence
            || record.previous_digest != previous_digest
            || &record.store_id != store_id
            || &record.worktree_id != worktree_id
        {
            return Err(invalid("VFS journal binding or digest chain mismatch"));
        }
        let next = transition(state.as_ref(), &record.event)?;
        if matches!(
            &record.event,
            VfsEvent::CaptureResolved { accepted: true, .. }
        ) {
            // Every accepted capture, including legacy records below capacity,
            // must retain its exact receipt before it can confer lineage. This
            // also validates the single bounded terminal repair exception.
            let pending = state
                .as_ref()
                .and_then(|state| state.pending_capture.as_ref())
                .ok_or_else(|| invalid("VFS capture acknowledgment has no pending intent"))?;
            verify_receipt(
                &pending.transaction_id,
                &pending.request_digest,
                &pending.receipt,
            )?;
        }
        if matches!(
            &record.event,
            VfsEvent::ActivationResolved { accepted: true, .. }
        ) {
            let pending = state
                .as_ref()
                .and_then(|state| state.pending_activation.as_ref())
                .ok_or_else(|| invalid("VFS activation acknowledgment has no pending intent"))?;
            verify_receipt(
                &pending.transaction_id,
                &pending.request_digest,
                &pending.receipt,
            )?;
        }
        state = Some(next);
        previous_digest = blake3::hash(&bytes).to_hex().to_string();
    }
    Ok((state, count, previous_digest))
}

fn capture_receipt_matches(
    store: &super::NativeStoreOpen,
    intent: &VfsCaptureIntent,
) -> io::Result<bool> {
    acceptance_receipt_matches(
        store,
        &intent.transaction_id,
        &intent.request_digest,
        &intent.receipt,
    )
}

fn acceptance_receipt_matches(
    store: &super::NativeStoreOpen,
    transaction_id: &str,
    request_digest: &str,
    expected_receipt: &ApplyBatchReceipt,
) -> io::Result<bool> {
    store.verify_filesystem_identity()?;
    let row: Option<(String, String)> = store
        .connection()
        .map_err(io::Error::other)?
        .query_row(
            "SELECT request_digest,receipt_json FROM applied_transactions WHERE transaction_id=?1",
            [transaction_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(io::Error::other)
        .journal_context("read committed capture receipt")?;
    let Some((digest, receipt)) = row else {
        return Ok(false);
    };
    let value = validate_canonical_receipt_json(&receipt).map_err(io::Error::other)?;
    let receipt: ApplyBatchReceipt = serde_json::from_value(value).map_err(io::Error::other)?;
    if digest != request_digest || &receipt != expected_receipt {
        return Err(invalid(
            "VFS acceptance receipt does not match prepared transaction",
        ));
    }
    store.verify_filesystem_identity()?;
    Ok(true)
}

fn transition(state: Option<&VfsRecoveryState>, event: &VfsEvent) -> io::Result<VfsRecoveryState> {
    let Some(previous) = state else {
        return match event {
            VfsEvent::Reserved { locator } if !locator.is_empty() && locator.len() <= 32 * 1024 => {
                Ok(VfsRecoveryState {
                    phase: VfsPhase::Reserved,
                    locator: locator.clone(),
                    ticket: None,
                    control: None,
                    pending_activation: None,
                    activation: None,
                    pending_capture: None,
                    last_accepted: None,
                })
            }
            _ => Err(invalid("VFS journal must begin with a bounded reservation")),
        };
    };
    let mut next = previous.clone();
    match (previous.phase, event) {
        (VfsPhase::Reserved, VfsEvent::Registered) => next.phase = VfsPhase::Registered,
        (VfsPhase::Registered, VfsEvent::ProviderPrepared { ticket })
            if !ticket.is_empty() && ticket.len() <= 32_768 =>
        {
            next.phase = VfsPhase::ProviderPrepared;
            next.ticket = Some(ticket.clone());
        }
        (VfsPhase::ProviderPrepared, VfsEvent::ProviderPopulated) => {
            next.phase = VfsPhase::ProviderPopulated
        }
        (VfsPhase::ProviderPopulated, VfsEvent::ControlPrepared { control })
            if !control.is_empty() && control.len() <= 32_768 =>
        {
            next.phase = VfsPhase::ControlPrepared;
            next.control = Some(control.clone());
        }
        (VfsPhase::ControlPrepared, VfsEvent::LocatorPublished { control })
            if previous.control.as_ref() == Some(control) =>
        {
            next.phase = VfsPhase::LocatorPublished;
        }
        // Existing prototype journals already bind the published file's native
        // identity. Keep those receipts readable; append_checked forbids newly
        // publishing without ControlPrepared, and resume never adopts by bytes.
        (VfsPhase::ProviderPopulated, VfsEvent::LocatorPublished { control })
            if !control.is_empty() && control.len() <= 32_768 =>
        {
            next.phase = VfsPhase::LocatorPublished;
            next.control = Some(control.clone());
        }
        (VfsPhase::LocatorPublished, VfsEvent::Ready) => next.phase = VfsPhase::Ready,
        (VfsPhase::LocatorPublished, VfsEvent::ActivationPrepared { intent })
            if previous.pending_activation.is_none() && previous.activation.is_none() =>
        {
            next.phase = VfsPhase::ActivationPrepared;
            next.pending_activation = Some(intent.clone());
        }
        (
            VfsPhase::ActivationPrepared,
            VfsEvent::ActivationResolved {
                transaction_id,
                accepted,
            },
        ) => {
            let pending = previous
                .pending_activation
                .as_ref()
                .ok_or_else(|| invalid("VFS activation resolution without intent"))?;
            if &pending.transaction_id != transaction_id {
                return Err(invalid("VFS activation resolution mismatch"));
            }
            if *accepted {
                next.activation = Some(pending.clone());
                next.phase = VfsPhase::Ready;
            } else {
                next.phase = VfsPhase::LocatorPublished;
            }
            next.pending_activation = None;
        }
        (VfsPhase::Ready, VfsEvent::CapturePrepared { intent })
            if previous.pending_capture.is_none() =>
        {
            next.pending_capture = Some(intent.clone());
        }
        (
            VfsPhase::Ready,
            VfsEvent::CaptureResolved {
                transaction_id,
                accepted,
            },
        ) => {
            let pending = previous
                .pending_capture
                .as_ref()
                .ok_or_else(|| invalid("VFS capture resolution without intent"))?;
            if &pending.transaction_id != transaction_id {
                return Err(invalid("VFS capture resolution mismatch"));
            }
            if *accepted {
                next.last_accepted = Some(pending.clone());
            }
            next.pending_capture = None;
        }
        _ => return Err(invalid("invalid VFS durable transition")),
    }
    Ok(next)
}

fn read_bounded(file: &std::fs::File, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("VFS record exceeds limit"));
    }
    Ok(bytes)
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// An ordinary, private recovery artifact. Files are create-new copies, never
/// links to the CAS or virtual checkout. Failure leaves the incomplete artifact
/// intact without a completion manifest.
pub struct VfsExportWriter {
    container: PinnedDirectory,
    tree: PinnedDirectory,
    root: PathBuf,
    files: std::collections::BTreeMap<layrs_core::scm::RepoPath, (layrs_core::scm::FileId, u64)>,
}
impl VfsExportWriter {
    pub fn create(parent: &Path, id: &str) -> io::Result<Self> {
        if id.len() != 36 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
            return Err(invalid("invalid VFS export ID"));
        }
        let parent_path = std::path::absolute(parent)?;
        let parent = PinnedDirectory::open(&parent_path)?;
        let name = format!("layrs-vfs-export-{id}");
        // create_file reserves the name first; the actual container has a
        // different suffix and owner-only ACL from its initial creation.
        let reservation = parent.create_file(format!("{name}.reservation").as_ref())?;
        reservation.file().sync_all()?;
        parent.sync()?;
        let container = parent.ensure_private_child(name.as_ref())?;
        let root = parent_path.join(&name);
        if std::fs::read_dir(&root)?.next().transpose()?.is_some() {
            return Err(invalid("VFS export directory already has entries"));
        }
        let tree = container.ensure_private_child("tree".as_ref())?;
        Ok(Self {
            container,
            tree,
            root,
            files: Default::default(),
        })
    }

    pub fn container_root(&self) -> &Path {
        &self.root
    }
    pub fn tree_root(&self) -> PathBuf {
        self.root.join("tree")
    }

    pub fn write_file(
        &mut self,
        path: &layrs_core::scm::RepoPath,
        expected: &layrs_core::scm::FileId,
        length: u64,
        produce: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.files.contains_key(path) || self.files.len() >= 100_000 {
            return Err(invalid("duplicate or excessive export paths"));
        }
        self.container.verify_path_identity()?;
        let (parent, name) = self.tree.components_to_parent(Path::new(path.as_str()))?;
        let mut file = parent.create_file(&name)?;
        let mut writer = ExportHasher::new(file.file_mut(), expected, length);
        produce(&mut writer)?;
        writer.finish()?;
        file.file().sync_all()?;
        parent.sync()?;
        // Re-read the published ordinary file and hash all its bytes before it
        // becomes eligible for a completed recovery manifest.
        let checked = parent.open_file(&name)?;
        if !checked.same_file(&file) {
            return Err(invalid("export file replaced"));
        }
        let mut sink = io::sink();
        let mut verifier = ExportHasher::new(&mut sink, expected, length);
        io::copy(&mut checked.file(), &mut verifier)?;
        verifier.finish()?;
        self.files.insert(path.clone(), (expected.clone(), length));
        Ok(())
    }

    pub fn finish(self, manifest: &[u8]) -> io::Result<PathBuf> {
        if manifest.len() > 32 * 1024 * 1024 {
            return Err(invalid("export manifest exceeds limit"));
        }
        self.container.verify_path_identity()?;
        self.tree.verify_path_identity()?;
        // Compare the whole ordinary namespace, including unexpected leaves.
        let mut pending = vec![(
            String::new(),
            self.container.open_child_for_traversal("tree".as_ref())?,
        )];
        let mut found = std::collections::BTreeSet::new();
        let mut visited = 0usize;
        while let Some((relative, directory)) = pending.pop() {
            for entry in std::fs::read_dir(self.tree_root().join(&relative))? {
                visited += 1;
                if visited > 200_000 {
                    return Err(invalid("export namespace exceeds limit"));
                }
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("export path is not Unicode"))?;
                let path = if relative.is_empty() {
                    name.clone()
                } else {
                    format!("{relative}/{name}")
                };
                if entry.file_type()?.is_dir() {
                    pending.push((path, directory.open_child_for_traversal(name.as_ref())?));
                } else {
                    let path = layrs_core::scm::RepoPath::parse(path).map_err(io::Error::other)?;
                    let (expected, length) = self
                        .files
                        .get(&path)
                        .ok_or_else(|| invalid("unexpected export file"))?;
                    let file = directory.open_file(name.as_ref())?;
                    let mut sink = io::sink();
                    let mut verifier = ExportHasher::new(&mut sink, expected, *length);
                    io::copy(&mut file.file(), &mut verifier)?;
                    verifier.finish()?;
                    found.insert(path);
                }
            }
        }
        if found.len() != self.files.len() {
            return Err(invalid("export file disappeared"));
        }
        let mut file = self.container.create_file("manifest.pending".as_ref())?;
        file.file_mut().write_all(manifest)?;
        file.file().sync_all()?;
        self.container.sync()?;
        if file.rename_to(&self.container, "manifest.json".as_ref())? {
            return Err(invalid("export completion manifest collision"));
        }
        self.container.sync()?;
        self.container.verify_path_identity()?;
        Ok(self.tree_root())
    }
}

struct ExportHasher<'a> {
    output: &'a mut dyn Write,
    hash: blake3::Hasher,
    expected: &'a layrs_core::scm::FileId,
    length: u64,
    written: u64,
    failed: bool,
}
impl<'a> ExportHasher<'a> {
    fn new(output: &'a mut dyn Write, expected: &'a layrs_core::scm::FileId, length: u64) -> Self {
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-file-v1\0");
        Self {
            output,
            hash,
            expected,
            length,
            written: 0,
            failed: false,
        }
    }
    fn finish(self) -> io::Result<()> {
        if self.failed
            || self.written != self.length
            || self.expected.as_str() != format!("blake3:{}", self.hash.finalize().to_hex())
        {
            return Err(invalid(
                "export file does not match complete FileId and length",
            ));
        }
        Ok(())
    }
}
impl Write for ExportHasher<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed || bytes.len() as u64 > self.length.saturating_sub(self.written) {
            self.failed = true;
            return Err(invalid("export file exceeds expected length"));
        }
        match self.output.write(bytes) {
            Ok(n) => {
                self.hash.update(&bytes[..n]);
                self.written += n as u64;
                Ok(n)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush().inspect_err(|_| self.failed = true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture_batch(id: &WorktreeId) -> WriteBatch {
        WriteBatch::new(
            layrs_store::scm::TransactionId::new("transaction:journal-boundary").unwrap(),
            vec![],
            vec![],
            vec![],
            vec![StoreMutation::WorktreeActiveOperation(
                layrs_store::scm::WorktreeActiveOperationCas::new(
                    id.clone(),
                    OperationId::parse("operation:before").unwrap(),
                    OperationId::parse("operation:after").unwrap(),
                )
                .unwrap(),
            )],
        )
        .unwrap()
    }

    fn capture_intent(id: &WorktreeId) -> VfsCaptureIntent {
        let batch = capture_batch(id);
        VfsCaptureIntent {
            transaction_id: batch.transaction_id().to_string(),
            request_digest: batch.request_digest().to_string(),
            operation_id: OperationId::parse("operation:after").unwrap(),
            step_id: StepId::parse(format!("blake3:{}", "0".repeat(64))).unwrap(),
            receipt: ApplyBatchReceipt::new(&batch),
        }
    }

    fn activation_intent(id: &WorktreeId) -> VfsActivationIntent {
        let capture = capture_intent(id);
        VfsActivationIntent {
            transaction_id: capture.transaction_id,
            request_digest: capture.request_digest,
            operation_id: capture.operation_id,
            step_id: capture.step_id,
            expected_registration_operation_id: OperationId::parse("operation:before").unwrap(),
            receipt: capture.receipt,
        }
    }

    #[test]
    fn production_capacity_reserves_ack_and_allows_only_one_legacy_terminal_event() {
        let intent = capture_intent(&WorktreeId::parse("worktree:capacity").unwrap());
        let prepared = VfsEvent::CapturePrepared {
            intent: intent.clone(),
        };
        let resolved = VfsEvent::CaptureResolved {
            transaction_id: intent.transaction_id,
            accepted: true,
        };
        assert!(require_append_capacity(65_534, MAX_RECORDS, &prepared).is_ok());
        assert!(require_append_capacity(65_535, MAX_RECORDS, &prepared).is_err());
        assert!(require_append_capacity(65_535, MAX_RECORDS, &resolved).is_ok());
        assert!(require_append_capacity(65_536, MAX_RECORDS, &resolved).is_ok());
        assert!(require_append_capacity(
            65_536,
            MAX_RECORDS,
            &VfsEvent::CaptureResolved {
                transaction_id: "absent".into(),
                accepted: false,
            }
        )
        .is_err());
        assert!(require_append_capacity(65_536, MAX_RECORDS, &VfsEvent::Ready).is_err());
        assert!(require_append_capacity(65_537, MAX_RECORDS, &resolved).is_err());
        let activation = activation_intent(&WorktreeId::parse("worktree:capacity").unwrap());
        let prepared = VfsEvent::ActivationPrepared {
            intent: activation.clone(),
        };
        assert!(require_append_capacity(65_534, MAX_RECORDS, &prepared).is_ok());
        assert!(require_append_capacity(65_535, MAX_RECORDS, &prepared).is_err());
        for accepted in [false, true] {
            let resolved = VfsEvent::ActivationResolved {
                transaction_id: activation.transaction_id.clone(),
                accepted,
            };
            assert!(require_append_capacity(65_535, MAX_RECORDS, &resolved).is_ok());
            assert!(require_append_capacity(65_536, MAX_RECORDS, &resolved).is_err());
            assert!(require_replay_capacity(65_536, MAX_RECORDS, &resolved).is_err());
        }
    }

    #[test]
    fn activation_replay_requires_exact_receipt_and_never_infers_legacy_baseline() {
        let root = std::env::temp_dir().join(format!(
            "layrs-vfs-activation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let store_root = root.join("store");
        let store_id = StoreId::parse("store:activation").unwrap();
        let worktree_id = WorktreeId::parse("worktree:activation").unwrap();
        drop(super::super::init(&store_root, store_id.clone()).unwrap());
        let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
        let metadata = PinnedDirectory::open(&store_root.join("metadata"))
            .unwrap()
            .identity_token();
        let open = |create| {
            VfsRecovery::acquire_bounded(
                &store_root,
                &identity,
                &metadata,
                &store_id,
                &worktree_id,
                create,
                12,
            )
        };
        let mut journal = open(true).unwrap();
        for event in [
            VfsEvent::Reserved { locator: vec![1] },
            VfsEvent::Registered,
            VfsEvent::ProviderPrepared { ticket: vec![2] },
            VfsEvent::ProviderPopulated,
            VfsEvent::ControlPrepared { control: vec![3] },
            VfsEvent::LocatorPublished { control: vec![3] },
        ] {
            journal.append(event, &NoVfsJournalFaults).unwrap();
        }
        let legacy = transition(Some(journal.state()), &VfsEvent::Ready).unwrap();
        assert!(legacy.provider_capture_baseline().is_none());
        let batch = capture_batch(&worktree_id);
        let expected = activation_intent(&worktree_id);
        assert!(journal
            .prepare_activation(
                &batch,
                &expected.step_id,
                &OperationId::parse("operation:foreign").unwrap(),
                &NoVfsJournalFaults
            )
            .is_err());
        assert_eq!(journal.next_sequence, 6);
        journal.record_limit = 7;
        assert!(journal
            .prepare_activation(
                &batch,
                &expected.step_id,
                &expected.expected_registration_operation_id,
                &NoVfsJournalFaults
            )
            .is_err());
        assert_eq!(journal.next_sequence, 6);
        journal.record_limit = 12;
        journal
            .prepare_activation(
                &batch,
                &expected.step_id,
                &expected.expected_registration_operation_id,
                &NoVfsJournalFaults,
            )
            .unwrap();
        drop(journal); // Before acceptance: replay may only return to the original registration.
        let mut journal = open(false).unwrap();
        assert_eq!(
            journal.resolve_activation(&NoVfsJournalFaults).unwrap(),
            Some(false)
        );
        assert_eq!(journal.state().phase, VfsPhase::LocatorPublished);
        assert!(journal.state().provider_capture_baseline().is_none());
        journal
            .prepare_activation(
                &batch,
                &expected.step_id,
                &expected.expected_registration_operation_id,
                &NoVfsJournalFaults,
            )
            .unwrap();
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        connection.execute("INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,0)", rusqlite::params![
            expected.transaction_id, expected.request_digest,
            String::from_utf8(layrs_core::scm::canonical_json(&expected.receipt).unwrap()).unwrap(),
        ]).unwrap();
        drop(connection);
        drop(committed);
        drop(journal); // After acceptance, before acknowledgment: exact Operation survives.
        let mut journal = open(false).unwrap();
        assert_eq!(
            journal.resolve_activation(&NoVfsJournalFaults).unwrap(),
            Some(true)
        );
        assert_eq!(
            journal.state().provider_capture_baseline(),
            Some((&expected.operation_id, &expected.step_id))
        );
        let sequence = journal.next_sequence;
        assert_eq!(
            journal.resolve_activation(&NoVfsJournalFaults).unwrap(),
            None
        );
        assert_eq!(journal.next_sequence, sequence);
        drop(journal);
        let journal = open(false).unwrap();
        assert_eq!(
            journal.state().provider_capture_baseline(),
            Some((&expected.operation_id, &expected.step_id))
        );
        drop(journal);
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        assert_ne!(
            expected.request_digest,
            format!("blake3:{}", "0".repeat(64))
        );
        connection
            .execute(
                "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
                rusqlite::params![
                    format!("blake3:{}", "0".repeat(64)),
                    expected.transaction_id,
                ],
            )
            .unwrap();
        drop(connection);
        drop(committed);
        assert!(
            open(false).is_err(),
            "acknowledged activation still requires the exact receipt on reopen"
        );
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_capture_below_capacity_requires_receipt_for_replayed_lineage() {
        let root = std::env::temp_dir().join(format!(
            "layrs-vfs-legacy-receipt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let store_root = root.join("store");
        let store_id = StoreId::parse("store:legacy-receipt").unwrap();
        let worktree_id = WorktreeId::parse("worktree:legacy-receipt").unwrap();
        drop(super::super::init(&store_root, store_id.clone()).unwrap());
        let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
        let metadata = PinnedDirectory::open(&store_root.join("metadata"))
            .unwrap()
            .identity_token();
        let open = |create| {
            VfsRecovery::acquire_bounded(
                &store_root,
                &identity,
                &metadata,
                &store_id,
                &worktree_id,
                create,
                12,
            )
        };
        let mut journal = open(true).unwrap();
        for event in [
            VfsEvent::Reserved { locator: vec![1] },
            VfsEvent::Registered,
            VfsEvent::ProviderPrepared { ticket: vec![2] },
            VfsEvent::ProviderPopulated,
            VfsEvent::ControlPrepared { control: vec![3] },
            VfsEvent::LocatorPublished { control: vec![3] },
        ] {
            journal.append(event, &NoVfsJournalFaults).unwrap();
        }
        assert!(journal
            .append(VfsEvent::Ready, &NoVfsJournalFaults)
            .is_err());
        journal
            .append_checked(VfsEvent::Ready, &NoVfsJournalFaults)
            .unwrap();
        assert!(journal.state().provider_capture_baseline().is_none());
        let expected = capture_intent(&worktree_id);
        journal
            .prepare_capture(
                &capture_batch(&worktree_id),
                &expected.step_id,
                &NoVfsJournalFaults,
            )
            .unwrap();
        assert!(journal.state().provider_capture_baseline().is_none());
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        connection.execute("INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,0)", rusqlite::params![
            expected.transaction_id, expected.request_digest,
            String::from_utf8(layrs_core::scm::canonical_json(&expected.receipt).unwrap()).unwrap(),
        ]).unwrap();
        drop(connection);
        drop(committed);
        assert_eq!(
            journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
            Some(true)
        );
        assert_eq!(journal.next_sequence, 9);
        drop(journal);
        let journal = open(false).unwrap();
        assert!(journal.state().activation.is_none());
        assert_eq!(
            journal.state().provider_capture_baseline(),
            Some((&expected.operation_id, &expected.step_id))
        );
        drop(journal);
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        assert_ne!(
            expected.request_digest,
            format!("blake3:{}", "0".repeat(64))
        );
        connection
            .execute(
                "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
                rusqlite::params![
                    format!("blake3:{}", "0".repeat(64)),
                    expected.transaction_id
                ],
            )
            .unwrap();
        drop(connection);
        drop(committed);
        assert!(
            open(false).is_err(),
            "legacy capture below capacity needs its exact accepted receipt"
        );
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        connection
            .execute(
                "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
                rusqlite::params![expected.request_digest, expected.transaction_id],
            )
            .unwrap();
        drop(connection);
        drop(committed);
        assert_eq!(
            open(false).unwrap().state().provider_capture_baseline(),
            Some((&expected.operation_id, &expected.step_id))
        );
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exhausted_legacy_pending_journal_reopens_and_resolves_exact_receipt_once() {
        let root = std::env::temp_dir().join(format!(
            "layrs-vfs-terminal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let store_root = root.join("store");
        let store_id = StoreId::parse("store:vfs-terminal").unwrap();
        let worktree_id = WorktreeId::parse("worktree:vfs-terminal").unwrap();
        drop(super::super::init(&store_root, store_id.clone()).unwrap());
        let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
        let metadata = PinnedDirectory::open(&store_root.join("metadata"))
            .unwrap()
            .identity_token();
        // Same parser/writer at a smaller physical limit: seven creation records
        // leave exactly one slot, which is insufficient for a new capture.
        let open = |create| {
            VfsRecovery::acquire_bounded(
                &store_root,
                &identity,
                &metadata,
                &store_id,
                &worktree_id,
                create,
                8,
            )
        };
        let mut journal = open(true).unwrap();
        for event in [
            VfsEvent::Reserved { locator: vec![1] },
            VfsEvent::Registered,
            VfsEvent::ProviderPrepared { ticket: vec![2] },
            VfsEvent::ProviderPopulated,
            VfsEvent::ControlPrepared { control: vec![3] },
            VfsEvent::LocatorPublished { control: vec![3] },
            VfsEvent::Ready,
        ] {
            if matches!(event, VfsEvent::Ready) {
                assert!(journal.append(event.clone(), &NoVfsJournalFaults).is_err());
                // Test-only old-producer fixture; public writers cannot emit Ready.
                journal.append_checked(event, &NoVfsJournalFaults).unwrap();
            } else {
                journal.append(event, &NoVfsJournalFaults).unwrap();
            }
        }
        let intent = capture_intent(&worktree_id);
        assert!(journal
            .prepare_capture(
                &capture_batch(&worktree_id),
                &intent.step_id,
                &NoVfsJournalFaults
            )
            .is_err());
        assert_eq!(journal.next_sequence, 7);
        assert!(journal.state().pending_capture.is_none());
        // Reproduce an old writer's durable final-slot intent without weakening
        // the current append path. All later replay/resolution uses real methods.
        let record = Record {
            schema: SCHEMA.into(),
            sequence: 7,
            previous_digest: journal.previous_digest.clone(),
            store_id: store_id.clone(),
            worktree_id: worktree_id.clone(),
            event: VfsEvent::CapturePrepared {
                intent: intent.clone(),
            },
        };
        journal
            .publish(
                "00000000007.json",
                &serde_json::to_vec(&record).unwrap(),
                &NoVfsJournalFaults,
            )
            .unwrap();
        drop(journal);
        let mut journal = open(false).unwrap();
        assert_eq!(journal.next_sequence, 8);
        assert_eq!(
            journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
            Some(false)
        );
        assert_eq!(journal.next_sequence, 8);
        assert!(journal.state().pending_capture.is_some());
        assert!(journal.state().last_accepted.is_none());
        assert!(journal
            .ensure_capture_capacity()
            .unwrap_err()
            .to_string()
            .contains("recovery export"));
        assert!(journal
            .append(VfsEvent::Ready, &NoVfsJournalFaults)
            .is_err());
        journal
            .publish_endpoint("00000000-0000-0000-0000-000000000001", b"recovery endpoint")
            .unwrap();
        drop(journal);
        let mut journal = open(false).unwrap();
        assert_eq!(
            journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
            Some(false)
        );
        assert!(journal.state().pending_capture.is_some());
        // Exhaustion never prevents the independent verified-copy export path
        // or changes unaccepted dirty bytes. Native tests separately cover the
        // provider checkpoint feeding this same export writer.
        let dirty = b"unaccepted dirty bytes must survive a full journal";
        let dirty_path = root.join("dirty.txt");
        std::fs::write(&dirty_path, dirty).unwrap();
        let mut export =
            VfsExportWriter::create(&root, "00000000-0000-0000-0000-000000000003").unwrap();
        export
            .write_file(
                &layrs_core::scm::RepoPath::parse("dirty.txt").unwrap(),
                &layrs_core::scm::FileId::from_bytes(dirty),
                dirty.len() as u64,
                &mut |output| output.write_all(&std::fs::read(&dirty_path)?),
            )
            .unwrap();
        let exported = export.finish(br#"{"complete":true}"#).unwrap();
        assert_eq!(std::fs::read(exported.join("dirty.txt")).unwrap(), dirty);
        assert_eq!(std::fs::read(&dirty_path).unwrap(), dirty);
        // Emulate the independently committed acceptance receipt. An altered
        // receipt must refuse resolution and leave the pending intent untouched.
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        let mut bad_receipt = intent.receipt.clone();
        let mut bad_value = serde_json::to_value(&bad_receipt).unwrap();
        bad_value["mutationCount"] = serde_json::json!(99);
        bad_receipt = serde_json::from_value(bad_value).unwrap();
        connection.execute("INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,0)", rusqlite::params![
            intent.transaction_id, intent.request_digest,
            String::from_utf8(layrs_core::scm::canonical_json(&bad_receipt).unwrap()).unwrap(),
        ]).unwrap();
        drop(connection);
        drop(committed);
        assert!(journal.resolve_capture(&NoVfsJournalFaults).is_err());
        assert_eq!(journal.next_sequence, 8);
        assert!(journal.state().pending_capture.is_some());
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        connection
            .execute(
                "UPDATE applied_transactions SET receipt_json=?1 WHERE transaction_id=?2",
                rusqlite::params![
                    String::from_utf8(layrs_core::scm::canonical_json(&intent.receipt).unwrap())
                        .unwrap(),
                    intent.transaction_id,
                ],
            )
            .unwrap();
        drop(connection);
        drop(committed);
        assert_eq!(
            journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
            Some(true)
        );
        assert_eq!(journal.next_sequence, 9);
        assert!(journal
            .append(VfsEvent::Ready, &NoVfsJournalFaults)
            .is_err());
        assert!(journal
            .prepare_capture(
                &capture_batch(&worktree_id),
                &intent.step_id,
                &NoVfsJournalFaults
            )
            .is_err());
        // Endpoint renewal is not an event and must remain available to recovery.
        journal
            .publish_endpoint("00000000-0000-0000-0000-000000000002", b"endpoint")
            .unwrap();
        drop(journal);
        let mut reopened = open(false).unwrap();
        assert_eq!(reopened.next_sequence, 9);
        assert!(reopened.state().pending_capture.is_none());
        assert_eq!(
            reopened
                .state()
                .last_accepted
                .as_ref()
                .unwrap()
                .operation_id,
            intent.operation_id
        );
        assert_eq!(reopened.resolve_capture(&NoVfsJournalFaults).unwrap(), None);
        drop(reopened);
        // Replay of the exceptional terminal slot rechecks the committed receipt,
        // rather than trusting only a self-consistent journal digest chain.
        let committed = super::super::open_exclusive(&store_root).unwrap();
        let connection = committed.begin_write_connection().unwrap();
        assert_ne!(intent.request_digest, format!("blake3:{}", "0".repeat(64)));
        connection
            .execute(
                "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
                rusqlite::params![format!("blake3:{}", "0".repeat(64)), intent.transaction_id,],
            )
            .unwrap();
        drop(connection);
        drop(committed);
        assert!(open(false).is_err());
        assert!(VfsRecovery::acquire_bounded(
            &store_root,
            "foreign-root",
            &metadata,
            &store_id,
            &worktree_id,
            false,
            8
        )
        .is_err());
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_context_preserves_native_error_kind_and_source() {
        let error = Err::<(), _>(io::Error::from_raw_os_error(5))
            .journal_context("reopen private container")
            .unwrap_err();
        assert_eq!(error.kind(), io::Error::from_raw_os_error(5).kind());
        assert!(error.to_string().contains("reopen private container"));
        let context = error
            .get_ref()
            .unwrap()
            .downcast_ref::<JournalIoError>()
            .unwrap();
        assert_eq!(context.source.raw_os_error(), Some(5));
    }

    #[test]
    fn metadata_read_waits_for_typed_writer_contention_and_rechecks_identity() {
        let root = std::env::temp_dir().join(format!(
            "layrs-vfs-contention-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let store_root = root.join("store");
        let store_id = StoreId::parse("store:vfs-contention-test").unwrap();
        drop(super::super::init(&store_root, store_id.clone()).unwrap());
        let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
        let metadata = PinnedDirectory::open(&store_root.join("metadata"))
            .unwrap()
            .identity_token();
        let writer = super::super::open_exclusive(&store_root).unwrap();
        let (contended_tx, contended_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let (reader_root, reader_identity, reader_metadata, reader_store_id) =
                (&store_root, &identity, &metadata, &store_id);
            let reader = scope.spawn(move || {
                let store = open_bound_store_wait(
                    reader_root,
                    reader_identity,
                    reader_metadata,
                    &mut || {
                        contended_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                    },
                )
                .unwrap();
                assert_eq!(store.store_id(), reader_store_id);
                store.verify_filesystem_identity().unwrap();
            });
            // This handshake fires only after the actual typed failed open.
            contended_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            drop(writer);
            release_tx.send(()).unwrap();
            reader.join().unwrap();
        });
        let mut retried = false;
        assert!(
            open_bound_store_wait(&store_root, "wrong-root", &metadata, &mut || {
                retried = true;
            })
            .is_err()
        );
        assert!(!retried, "identity failures must not be retried");
        assert!(
            open_bound_store_wait(&store_root, &identity, "wrong-metadata", &mut || {
                retried = true;
            })
            .is_err()
        );
        assert!(!retried);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creation_cannot_skip_population_or_publish_ready_early() {
        assert!(transition(None, &VfsEvent::Ready).is_err());
        let mut state = transition(None, &VfsEvent::Reserved { locator: vec![1] }).unwrap();
        assert!(transition(Some(&state), &VfsEvent::Ready).is_err());
        for event in [
            VfsEvent::Registered,
            VfsEvent::ProviderPrepared { ticket: vec![2] },
            VfsEvent::ProviderPopulated,
            VfsEvent::ControlPrepared { control: vec![3] },
            VfsEvent::LocatorPublished { control: vec![3] },
            VfsEvent::Ready,
        ] {
            state = transition(Some(&state), &event).unwrap();
            if state.phase == VfsPhase::ControlPrepared {
                assert!(
                    transition(
                        Some(&state),
                        &VfsEvent::LocatorPublished { control: vec![4] }
                    )
                    .is_err(),
                    "publication must retain the prepared native identity"
                );
            }
        }
        assert_eq!(state.phase, VfsPhase::Ready);
        assert_eq!(state.ticket, Some(vec![2]));
        assert!(transition(Some(&state), &VfsEvent::ProviderPopulated).is_err());
        assert!(transition(
            Some(&state),
            &VfsEvent::CaptureResolved {
                transaction_id: "foreign".into(),
                accepted: true
            }
        )
        .is_err());
    }

    #[test]
    fn short_staged_record_never_becomes_authority_and_host_lease_is_exclusive() {
        struct Short;
        impl VfsJournalFaults for Short {
            fn write_staged(&self, output: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
                output.write_all(&bytes[..bytes.len() / 2])?;
                Err(io::Error::other("injected short journal write"))
            }
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("layrs-vfs-journal-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let store_root = root.join("store");
        let store_id = StoreId::parse(format!("store:journal-{nonce}")).unwrap();
        let worktree_id = WorktreeId::parse(format!("worktree:journal-{nonce}")).unwrap();
        drop(super::super::init(&store_root, store_id.clone()).unwrap());
        let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
        let metadata = PinnedDirectory::open(&store_root.join("metadata"))
            .unwrap()
            .identity_token();
        assert!(VfsRecovery::create(
            &store_root,
            &identity,
            &metadata,
            &store_id,
            &worktree_id,
            vec![1, 2],
            &Short
        )
        .is_err());
        assert!(
            VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).is_err()
        );
        let journal = VfsRecovery::create(
            &store_root,
            &identity,
            &metadata,
            &store_id,
            &worktree_id,
            vec![3, 4],
            &NoVfsJournalFaults,
        )
        .unwrap();
        assert!(
            VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).is_err()
        );
        drop(journal);
        let mut journal =
            VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).unwrap();
        assert_eq!(journal.state().locator, vec![3, 4]);
        journal
            .append(VfsEvent::Registered, &NoVfsJournalFaults)
            .unwrap();
        let endpoint_id = "00000000-0000-0000-0000-000000000001";
        journal
            .publish_endpoint(endpoint_id, b"private endpoint test")
            .unwrap();
        assert_eq!(
            VfsRecovery::read_endpoint(
                &store_root,
                &identity,
                &metadata,
                &store_id,
                &worktree_id,
                endpoint_id,
            )
            .unwrap(),
            b"private endpoint test"
        );
        assert!(journal
            .append(
                VfsEvent::CaptureResolved {
                    transaction_id: "forged".into(),
                    accepted: true
                },
                &NoVfsJournalFaults
            )
            .is_err());
        drop(journal);
        let record = store_root
            .join("metadata/vfs-v1")
            .join(
                blake3::hash(worktree_id.as_str().as_bytes())
                    .to_hex()
                    .as_str(),
            )
            .join("00000000000.json");
        std::fs::write(record, b"broken journal").unwrap();
        assert!(
            VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).is_err()
        );
        // Only this test's unique disposable root is removed; never a provider root.
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
