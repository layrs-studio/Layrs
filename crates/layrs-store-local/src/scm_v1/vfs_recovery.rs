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
        return Err(invalid(
            "VFS journal capacity exhausted; capture is disabled, preserve work with recovery export",
        ));
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
        // Format 6 has no file_manifests rows: the smallest manifest id of
        // the File comes from the registered catalog (FileId sections and
        // file records), as the query below answers it for format 5.
        let manifest_id: Option<String> = if store.compact() {
            super::NativeScmReader::new(store)
                .map_err(io::Error::other)?
                .file_manifest_ids_metadata(id)
                .map_err(io::Error::other)?
                .into_iter()
                .next()
                .map(|manifest| manifest.to_string())
        } else {
            store.connection().map_err(io::Error::other)?
                .query_row("SELECT manifest_id FROM file_manifests WHERE file_id=?1 ORDER BY manifest_id LIMIT 1",
                    [id.as_str()], |row| row.get(0)).optional().map_err(io::Error::other)?
        };
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
        self.write_file_with_check(path, expected, length, produce, &mut || Ok(()))
    }

    /// As `write_file`, with a cooperative check before effects and during the
    /// bounded verification reread. The producer remains responsible for its
    /// own checks while it supplies bytes.
    pub fn write_file_with_check(
        &mut self,
        path: &layrs_core::scm::RepoPath,
        expected: &layrs_core::scm::FileId,
        length: u64,
        produce: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        check()?;
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
        copy_export_checked(&mut checked.file(), &mut verifier, check)?;
        verifier.finish()?;
        self.files.insert(path.clone(), (expected.clone(), length));
        Ok(())
    }

    pub fn finish(self, manifest: &[u8]) -> io::Result<PathBuf> {
        self.finish_with_check(manifest, &mut || Ok(()))
    }

    /// Rechecks copied files in bounded blocks and checks once more before
    /// publishing the manifest. An error leaves the incomplete artifact intact.
    pub fn finish_with_check(
        self,
        manifest: &[u8],
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<PathBuf> {
        check()?;
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
                check()?;
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
                    copy_export_checked(&mut file.file(), &mut verifier, check)?;
                    verifier.finish()?;
                    found.insert(path);
                }
            }
        }
        if found.len() != self.files.len() {
            return Err(invalid("export file disappeared"));
        }
        check()?;
        let mut file = self.container.create_file("manifest.pending".as_ref())?;
        file.file_mut().write_all(manifest)?;
        file.file().sync_all()?;
        self.container.sync()?;
        check()?;
        if file.rename_to(&self.container, "manifest.json".as_ref())? {
            return Err(invalid("export completion manifest collision"));
        }
        self.container.sync()?;
        self.container.verify_path_identity()?;
        Ok(self.tree_root())
    }
}

fn copy_export_checked(
    input: &mut dyn Read,
    output: &mut dyn Write,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let mut buffer = [0; 64 * 1024];
    loop {
        check()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        output.write_all(&buffer[..count])?;
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
#[path = "vfs_recovery/tests.rs"]
mod tests;
