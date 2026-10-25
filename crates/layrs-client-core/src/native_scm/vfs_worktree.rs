//! Complete-CAS Windows Worktrees. This is intentionally a separate capability
//! from NativeWorkspace: ordinary scanners/materializers must never receive a
//! virtual checkout. The locator has a distinct schema rejected by old readers.
use super::{
    prepare, vfs_capture::VfsCaptureCounts, worktree_operations, CapturePlan, MutationStamp,
    NativeClient, NativeClientError, NativeWorkspace, PreparedMutation, Result,
    VerifiedVfsSnapshot, VerifiedVfsSnapshotBinding,
};
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{
    self,
    checkout::CheckoutReader,
    vfs_recovery::{VfsEvent, VfsExportWriter, VfsJournalFaults, VfsPhase, VfsRecovery},
    worktrees::WorktreeCreation,
    NativeScmReader,
};
use layrs_vfs::{Cancellation, ControlFileIdentity, Registration, Session, SnapshotSource};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const VFS_BACKEND_KEY: &str = "layrs.worktree.backend";
pub const VFS_BACKEND: &str = "windows-cfapi-v1";
const LOCATOR_SCHEMA: &str = "layrs.vfs-worktree-locator.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VfsWorktreeLocator {
    schema: String,
    backend: String,
    pub creation: WorktreeCreation,
    pub source: VerifiedVfsSnapshotBinding,
    pub root_id: [u8; 16],
    registration_transaction: String,
}

impl VfsWorktreeLocator {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 32 * 1024 {
            return Err(NativeClientError::InvalidState("VFS locator exceeds bound"));
        }
        let locator: Self = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        let worktree = &locator.creation.worktree;
        if locator.schema != LOCATOR_SCHEMA
            || locator.backend != VFS_BACKEND
            || locator.root_id == [0; 16]
            || worktree.store_id() != &locator.source.store_id
            || worktree.space_id() != &locator.source.space_id
            || locator.creation.step_id != locator.source.step_id
            || locator.creation.source_operation_id != locator.source.operation_id
            || !Path::new(worktree.checkout_root()).is_absolute()
            || !is_vfs_worktree(worktree)
        {
            return Err(NativeClientError::InvalidState(
                "VFS locator binding mismatch",
            ));
        }
        TransactionId::new(&locator.registration_transaction)?;
        Ok(locator)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        Self::decode(&bytes)?;
        Ok(bytes)
    }
    pub fn checkout_root(&self) -> &Path {
        Path::new(self.creation.worktree.checkout_root())
    }
    pub fn worktree_id(&self) -> &WorktreeId {
        self.creation.worktree.id()
    }
}

pub fn is_vfs_worktree(worktree: &Worktree) -> bool {
    worktree
        .metadata()
        .entries()
        .iter()
        .any(|entry| entry.key() == VFS_BACKEND_KEY && entry.value() == VFS_BACKEND)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsWorktreeStatus {
    pub worktree_id: WorktreeId,
    pub checkout_root: PathBuf,
    pub backend: &'static str,
    pub connected: Option<bool>,
    pub source_step_id: StepId,
    pub accepted_step_id: StepId,
    pub active_operation_id: OperationId,
    pub capture_ack_pending: bool,
    pub catalog_files: usize,
    pub capture_blocked_reason: Option<VfsCaptureBlockedReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VfsCaptureBlockedReason {
    AckPending,
    UnprovenActivation,
    ExternalWorktreeAdvance,
}

impl VfsCaptureBlockedReason {
    fn error(self) -> NativeClientError {
        NativeClientError::InvalidState(match self {
            Self::AckPending => "VFS acceptance acknowledgment is pending exact receipt verification; capture lineage is not yet established",
            Self::UnprovenActivation => "VFS NeedsRepair: initial provider activation is unproven; capture disabled; status and export remain available",
            Self::ExternalWorktreeAdvance => "VFS NeedsIntegration: Worktree advanced outside provider-owned acceptance lineage; capture disabled; preserve work with export",
        })
    }
}

fn capture_blocked_reason(
    journal: &scm_v1::vfs_recovery::VfsRecoveryState,
    operation: &OperationId,
    step: &StepId,
) -> Option<VfsCaptureBlockedReason> {
    if journal
        .pending_capture
        .as_ref()
        .is_some_and(|intent| &intent.operation_id == operation && &intent.step_id == step)
        || journal
            .pending_activation
            .as_ref()
            .is_some_and(|intent| &intent.operation_id == operation && &intent.step_id == step)
    {
        return Some(VfsCaptureBlockedReason::AckPending);
    }
    match journal.provider_capture_baseline() {
        None => Some(VfsCaptureBlockedReason::UnprovenActivation),
        Some((expected_operation, expected_step))
            if expected_operation != operation || expected_step != step =>
        {
            Some(VfsCaptureBlockedReason::ExternalWorktreeAdvance)
        }
        Some(_) => None,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsCaptureReport {
    pub step_id: StepId,
    pub active_operation_id: OperationId,
    pub changed: bool,
    pub counts: VfsCaptureCounts,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsExportReport {
    pub tree_root: PathBuf,
    pub manifest_path: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub root_tree_id: TreeId,
}

/// Fault after a committed capture but before its durable acknowledgment is a
/// distinct seam from journal IO faults. Reconnect resolves the stored receipt.
pub trait VfsWorktreeFaults: VfsJournalFaults {
    /// Before any durable reservation or creation coordination is acquired.
    fn before_creation_lease(&self) -> io::Result<()> {
        Ok(())
    }
    /// After the preliminary own-Operation check, before reading the exact CAS
    /// base. No Store capability is held while this interleaving seam runs.
    fn before_capture_base(&self) -> io::Result<()> {
        Ok(())
    }
    fn after_capture_accept(&self) -> io::Result<()> {
        Ok(())
    }
    fn after_activation_accept(&self) -> io::Result<()> {
        Ok(())
    }
}
pub struct NoVfsWorktreeFaults;
impl VfsJournalFaults for NoVfsWorktreeFaults {}
impl VfsWorktreeFaults for NoVfsWorktreeFaults {}

struct JournalFaults<'a>(&'a dyn VfsWorktreeFaults);
impl VfsJournalFaults for JournalFaults<'_> {
    fn write_staged(&self, output: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
        self.0.write_staged(output, bytes)
    }
    fn after_event(&self, event: &VfsEvent) -> io::Result<()> {
        self.0.after_event(event)
    }
}

pub struct VfsWorktree {
    // Session drops first: no callback can outlive the owned immutable source.
    session: Session,
    source: Arc<VerifiedVfsSnapshot>,
    locator: VfsWorktreeLocator,
    client: NativeClient,
    journal: VfsRecovery,
}

impl VfsWorktree {
    pub fn create(
        source: &NativeWorkspace,
        root: &Path,
        layer: &LayerId,
        step: &StepId,
    ) -> Result<Self> {
        Self::create_with_faults(source, root, layer, step, &NoVfsWorktreeFaults)
    }

    pub fn create_with_faults(
        source: &NativeWorkspace,
        root: &Path,
        layer: &LayerId,
        step: &StepId,
        faults: &dyn VfsWorktreeFaults,
    ) -> Result<Self> {
        let snapshot = Arc::new(VerifiedVfsSnapshot::prepare(
            source,
            step,
            &Cancellation::default(),
        )?);
        let locator = VfsWorktreeLocator::reserve(source, root, layer, step, &snapshot)?;
        faults.before_creation_lease()?;
        let creation_lease = locator.creation_lease(&snapshot, None)?;
        {
            let store = source.open_store()?;
            scm_v1::worktrees::validate_secondary_root_with_vfs(
                &store,
                locator.checkout_root(),
                |worktree| verify_registered_vfs_root(&store, worktree).map_err(io::Error::other),
            )?;
        }
        let client = locator.open_client()?;
        let recovery_id = locator.worktree_id().clone();
        let recovery_root = locator.checkout_root().to_path_buf();
        let journal = VfsRecovery::create(
            &locator.source.store_root,
            &locator.source.store_identity,
            &locator.source.store_metadata_identity,
            &locator.source.store_id,
            locator.worktree_id(),
            locator.encode()?,
            &JournalFaults(faults),
        )
        .map_err(|error| creation_error(&recovery_id, &recovery_root, error))?;
        Self::finish_creation(locator, client, journal, snapshot, faults, &creation_lease)
            .map_err(|error| creation_error(&recovery_id, &recovery_root, error))
    }

    /// Bounded .layrs bytes are hints until compared with private Store intent,
    /// actual provider registration, root identity and the guarded control file.
    pub fn open(root: &Path) -> Result<Self> {
        let (bytes, observed) = layrs_vfs::read_control_file(root)
            .map_err(|error| provider_stage("read locator for reopen", error))?;
        let locator = VfsWorktreeLocator::decode(&bytes)?;
        if std::path::absolute(root)? != locator.checkout_root() {
            return Err(NativeClientError::InvalidState(
                "VFS locator moved to another root",
            ));
        }
        let worktree = Self::reconnect(&locator)?;
        let recorded = worktree
            .journal
            .state()
            .control
            .as_ref()
            .ok_or(NativeClientError::Missing("VFS control identity"))?;
        if observed.encode() != *recorded {
            return Err(NativeClientError::InvalidState(
                "VFS locator file identity changed",
            ));
        }
        Ok(worktree)
    }

    pub fn reconnect(locator: &VfsWorktreeLocator) -> Result<Self> {
        let client = locator.open_client()?;
        let reserved = {
            let store = client.open_store()?;
            VfsRecovery::read_reservation(&store, locator.worktree_id())?
        };
        if locator.encode()? != reserved.locator {
            return Err(NativeClientError::InvalidState(
                "VFS locator differs from private reservation",
            ));
        }
        let snapshot = Arc::new(VerifiedVfsSnapshot::reopen(
            &locator.source,
            &Cancellation::default(),
        )?);
        // Global order is root creation lease, then the private journal lease.
        // The immutable lookup above releases its Store before either wait.
        let creation_lease = locator.creation_lease(&snapshot, Some(&reserved))?;
        let journal = VfsRecovery::open(
            &locator.source.store_root,
            &locator.source.store_identity,
            &locator.source.store_metadata_identity,
            &locator.source.store_id,
            locator.worktree_id(),
        )?;
        if locator.encode()? != journal.state().locator {
            return Err(NativeClientError::InvalidState(
                "VFS locator differs from private reservation",
            ));
        }
        Self::finish_creation(
            locator.clone(),
            client,
            journal,
            snapshot,
            &NoVfsWorktreeFaults,
            &creation_lease,
        )
    }

    /// Also recovers failures before .layrs was published. The caller selects a
    /// durable registration in this Store; no path or base is inferred anew.
    pub fn resume_registered(source: &NativeWorkspace, id: &WorktreeId) -> Result<Self> {
        let store = source.open_store()?;
        let store_id = store.store_id().clone();
        let reserved = VfsRecovery::read_reservation(&store, id)?;
        drop(store);
        let locator = VfsWorktreeLocator::decode(&reserved.locator)?;
        if locator.worktree_id() != id
            || locator.source.store_id != store_id
            || locator.source.store_root != source.store_root()
            || locator.source.store_identity != source.store_identity()
            || locator.source.store_metadata_identity != source.metadata_identity()
        {
            return Err(NativeClientError::InvalidState(
                "VFS recovery reservation binding mismatch",
            ));
        }
        Self::reconnect(&locator)
    }

    fn finish_creation(
        locator: VfsWorktreeLocator,
        client: NativeClient,
        mut journal: VfsRecovery,
        source: Arc<VerifiedVfsSnapshot>,
        faults: &dyn VfsWorktreeFaults,
        creation_lease: &scm_v1::worktrees::WorktreeCreationLease,
    ) -> Result<Self> {
        let journal_faults = JournalFaults(faults);
        for intent in journal
            .state()
            .activation
            .iter()
            .chain(journal.state().pending_activation.iter())
        {
            if intent.expected_registration_operation_id
                != *locator.creation.worktree.active_operation_id()
                || intent.step_id != locator.creation.step_id
            {
                return Err(NativeClientError::InvalidState(
                    "VFS activation intent differs from its original registration",
                ));
            }
        }
        if journal.state().phase == VfsPhase::Reserved {
            // No provider mutation preceded this phase: ordinary root identity
            // validation is still applicable and mandatory here.
            CheckoutReader::open(
                locator.checkout_root(),
                Some(locator.creation.worktree.filesystem_identity()),
            )?;
            locator.register(&client, creation_lease)?;
            journal.append(VfsEvent::Registered, &journal_faults)?;
        }
        let mut session = if journal.state().phase == VfsPhase::Registered {
            let session = Session::create_with_registration_hook(
                locator.checkout_root(),
                locator.root_id,
                source.clone(),
                |ticket| {
                    validate_ticket(&locator, source.as_ref(), ticket)
                        .map_err(|e| layrs_vfs::Error::Io(io::Error::other(e)))?;
                    journal
                        .append(
                            VfsEvent::ProviderPrepared {
                                ticket: ticket.encode(),
                            },
                            &journal_faults,
                        )
                        .map_err(layrs_vfs::Error::Io)
                },
            )
            .map_err(|error| provider_stage("register and populate provider", error))?;
            journal.append(VfsEvent::ProviderPopulated, &journal_faults)?;
            session
        } else {
            let ticket = Registration::decode(
                journal
                    .state()
                    .ticket
                    .as_ref()
                    .ok_or(NativeClientError::Missing("VFS provider ticket"))?,
            )
            .map_err(|error| provider_stage("decode durable registration", error))?;
            validate_ticket(&locator, source.as_ref(), &ticket)?;
            if journal.state().phase == VfsPhase::ProviderPrepared {
                // A durable hook may have run before registration. Trying fresh
                // create is safe only because its native preflight rejects every
                // already-registered/nonempty/foreign root before mutation.
                let session = match ticket.verify(source.catalog()) {
                    Ok(()) => Session::reconnect(ticket.clone(), source.clone())
                        .map_err(|error| provider_stage("reconnect prepared provider", error))?,
                    Err(original) => {
                        let expected_ticket = ticket.encode();
                        match Session::create_with_registration_hook(
                            locator.checkout_root(),
                            locator.root_id,
                            source.clone(),
                            |candidate| {
                                if candidate.encode() != expected_ticket {
                                    return Err(layrs_vfs::Error::IdentityMismatch);
                                }
                                Ok(())
                            },
                        ) {
                            Ok(session) => session,
                            Err(_) => {
                                return Err(provider_stage(
                                    "verify prepared registration",
                                    original,
                                ))
                            }
                        }
                    }
                };
                journal.append(VfsEvent::ProviderPopulated, &journal_faults)?;
                session
            } else {
                ticket
                    .verify(source.catalog())
                    .map_err(|error| provider_stage("verify populated registration", error))?;
                Session::reconnect(ticket, source.clone())
                    .map_err(|error| provider_stage("reconnect populated provider", error))?
            }
        };
        if matches!(
            journal.state().phase,
            VfsPhase::ProviderPopulated | VfsPhase::ControlPrepared
        ) {
            let bytes = locator.encode()?;
            let expected = journal
                .state()
                .control
                .as_ref()
                .map(|bytes| ControlFileIdentity::decode(bytes))
                .transpose()
                .map_err(|error| provider_stage("decode prepared locator identity", error))?;
            let staging_root = journal.staging_root()?;
            let control = session
                .publish_control_file_recoverable(
                    &bytes,
                    &staging_root,
                    expected.as_ref(),
                    |identity| {
                        journal
                            .append(
                                VfsEvent::ControlPrepared {
                                    control: identity.encode(),
                                },
                                &journal_faults,
                            )
                            .map_err(layrs_vfs::Error::Io)
                    },
                    &layrs_vfs::NoControlPublishFaults,
                )
                .map_err(|error| provider_stage("publish recoverable locator", error))?;
            journal.append(
                VfsEvent::LocatorPublished {
                    control: control.encode(),
                },
                &journal_faults,
            )?;
        } else {
            let control = ControlFileIdentity::decode(
                journal
                    .state()
                    .control
                    .as_ref()
                    .ok_or(NativeClientError::Missing("VFS control identity"))?,
            )
            .map_err(|error| provider_stage("decode durable control identity", error))?;
            session
                .attach_control_file(&control)
                .map_err(|error| provider_stage("attach durable locator", error))?;
        }
        if matches!(
            journal.state().phase,
            VfsPhase::LocatorPublished | VfsPhase::ActivationPrepared
        ) {
            locator.mark_ready(&client, &mut journal, faults)?;
        }
        if journal.state().phase != VfsPhase::Ready {
            return Err(NativeClientError::NeedsRepair);
        }
        journal.resolve_capture(&journal_faults)?;
        let result = Self {
            session,
            source,
            locator,
            client,
            journal,
        };
        let store = result.client.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let (_, _, state) = result.locator.current(&reader)?;
        if state.status() != WorktreeStatus::Ready {
            return Err(NativeClientError::NeedsRepair);
        }
        drop(reader);
        drop(store);
        Ok(result)
    }

    /// Requires the application to stop its writers until return. Effective
    /// file handles reject busy existing writers; namespace checks detect races.
    /// This does not promise an atomic transaction across unrelated applications.
    pub fn capture_quiesced(&mut self) -> Result<VfsCaptureReport> {
        self.capture_quiesced_with_faults(&NoVfsWorktreeFaults)
    }

    pub fn capture_quiesced_with_faults(
        &mut self,
        faults: &dyn VfsWorktreeFaults,
    ) -> Result<VfsCaptureReport> {
        let journal_faults = JournalFaults(faults);
        self.journal.resolve_capture(&journal_faults)?;
        self.journal.ensure_capture_capacity()?;
        let original = self.status()?;
        if let Some(reason) = original.capture_blocked_reason {
            return Err(reason.error());
        }
        let original_operation = original.active_operation_id;
        let original_step = original.accepted_step_id;
        let mut checkpoint = self
            .session
            .checkpoint()
            .map_err(|error| provider_stage("acquire capture checkpoint", error))?;
        let entries: Vec<_> = checkpoint
            .entries()
            .iter()
            .map(|entry| (entry.path.clone(), entry.evidence.clone()))
            .collect();
        let captured = super::vfs_capture::assemble_snapshot(
            &self.source,
            entries.iter().map(|(path, evidence)| (path, evidence)),
            &mut |path, output| {
                checkpoint
                    .with_reader(path, |reader| {
                        io::copy(reader, output)?;
                        Ok(())
                    })
                    .map_err(|error| provider_stage("read capture checkpoint file", error))
            },
        )?;
        checkpoint
            .revalidate_namespace()
            .map_err(|error| provider_stage("revalidate captured namespace", error))?;
        for _ in 0..8 {
            let store = self.client.open_store()?;
            let reader = NativeScmReader::new(&store)?;
            let (current, _, current_state) = self.locator.current(&reader)?;
            if current.active_operation_id() != &original_operation
                || current_state.base_step_id() != Some(&original_step)
            {
                return Err(NativeClientError::StaleCapture);
            }
            drop(reader);
            drop(store);
            faults.before_capture_base()?;
            let expected = self.client.capture_base(self.locator.worktree_id())?;
            // Refreshing global heads/generation is allowed only while the
            // Worktree still has the exact Operation observed before scanning.
            // The earlier independent read cannot establish this CAS binding.
            if expected.operation_id() != &original_operation {
                return Err(NativeClientError::StaleCapture);
            }
            let plan = match self.client.prepare_capture(
                &expected,
                captured.snapshot.clone(),
                MutationStamp::fresh()?,
            ) {
                Err(NativeClientError::StaleCapture) => continue,
                other => other?,
            };
            checkpoint
                .revalidate_namespace()
                .map_err(|error| provider_stage("revalidate prepared capture", error))?;
            match plan {
                CapturePlan::Unchanged { step_id } => {
                    return Ok(VfsCaptureReport {
                        step_id,
                        active_operation_id: original_operation,
                        changed: false,
                        counts: captured.counts,
                    })
                }
                CapturePlan::Change(change) => {
                    let intent = self.journal.prepare_capture(
                        change.batch(),
                        change.step_id(),
                        &journal_faults,
                    )?;
                    checkpoint
                        .revalidate_namespace()
                        .map_err(|error| provider_stage("revalidate before acceptance", error))?;
                    let accepted = self.client.accept(&change);
                    if accepted.is_ok() {
                        faults.after_capture_accept()?;
                    }
                    let committed = self.journal.resolve_capture(&journal_faults)? == Some(true);
                    if committed {
                        checkpoint.revalidate_namespace().map_err(|error| {
                            provider_stage("revalidate acknowledged capture", error)
                        })?;
                        return Ok(VfsCaptureReport {
                            step_id: intent.step_id,
                            active_operation_id: intent.operation_id,
                            changed: true,
                            counts: captured.counts,
                        });
                    }
                    match accepted {
                        Err(NativeClientError::Store(StoreError::CasMismatch { .. })) => continue,
                        Err(error) => return Err(error),
                        Ok(_) => {
                            return Err(NativeClientError::InvalidState(
                                "capture acceptance has no durable receipt",
                            ))
                        }
                    }
                }
            }
        }
        Err(NativeClientError::StaleCapture)
    }

    pub fn export_quiesced(&mut self, parent: &Path) -> Result<VfsExportReport> {
        let parent = VfsExportParent::open(parent, &self.session)?;
        let mut checkpoint = self
            .session
            .export_checkpoint()
            .map_err(|error| provider_stage("acquire export checkpoint", error))?;
        let entries: Vec<_> = checkpoint
            .entries()
            .iter()
            .map(|entry| (entry.path.clone(), entry.evidence.clone()))
            .collect();
        let captured = super::vfs_capture::assemble_export_snapshot(
            self.source.catalog(),
            entries.iter().map(|(path, evidence)| (path, evidence)),
            &mut |path, output| {
                checkpoint
                    .with_reader(path, |reader| {
                        io::copy(reader, output)?;
                        Ok(())
                    })
                    .map_err(|error| provider_stage("read export checkpoint file", error))
            },
        )?;
        checkpoint
            .revalidate_namespace()
            .map_err(|error| provider_stage("revalidate export snapshot", error))?;
        parent.verify()?;
        let mut export = VfsExportWriter::create(parent.root(), &uuid::Uuid::new_v4().to_string())?;
        let manifest_path = export.container_root().join("manifest.json");
        let mut total = 0u64;
        for file in &captured.files {
            export.write_file(&file.path, &file.file_id, file.byte_length, &mut |output| {
                if file.immutable_source {
                    let cancel = Cancellation::default();
                    let mut buffer = vec![0; 64 * 1024];
                    let mut offset = 0;
                    while offset < file.byte_length {
                        let length = (file.byte_length - offset).min(buffer.len() as u64) as usize;
                        self.source
                            .read_exact_at(&file.file_id, offset, &mut buffer[..length], &cancel)
                            .map_err(io::Error::other)?;
                        output.write_all(&buffer[..length])?;
                        offset += length as u64;
                    }
                    Ok(())
                } else {
                    checkpoint
                        .with_reader(&file.path, |reader| {
                            io::copy(reader, output)?;
                            Ok(())
                        })
                        .map_err(io::Error::other)
                }
            })?;
            total = total
                .checked_add(file.byte_length)
                .ok_or(NativeClientError::InvalidState("VFS export size overflow"))?;
        }
        checkpoint
            .revalidate_namespace()
            .map_err(|error| provider_stage("revalidate export before manifest", error))?;
        parent.verify()?;
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schema": "layrs.vfs-recovery-export.v1", "worktreeId": self.locator.worktree_id(),
            "source": self.locator.source, "rootTreeId": captured.snapshot.root_tree_id(),
            "files": captured.files, "bytes": total, "complete": true,
        }))
        .map_err(io::Error::other)?;
        let tree_root = export.finish(&manifest)?;
        Ok(VfsExportReport {
            tree_root,
            manifest_path,
            files: captured.files.len(),
            bytes: total,
            root_tree_id: captured.snapshot.root_tree_id().clone(),
        })
    }

    pub fn worktree_id(&self) -> &WorktreeId {
        self.locator.worktree_id()
    }
    pub fn checkout_root(&self) -> &Path {
        self.locator.checkout_root()
    }
    pub fn store_root(&self) -> &Path {
        &self.locator.source.store_root
    }
    pub fn store_identity(&self) -> &str {
        &self.locator.source.store_identity
    }
    pub fn metadata_identity(&self) -> &str {
        &self.locator.source.store_metadata_identity
    }
    pub fn store_id(&self) -> &StoreId {
        &self.locator.source.store_id
    }
    pub fn locator(&self) -> &VfsWorktreeLocator {
        &self.locator
    }
    pub(super) fn journal(&self) -> &VfsRecovery {
        &self.journal
    }
    pub fn source_read_counts(&self) -> (u64, u64) {
        self.source.read_counts()
    }
    pub fn disconnect(&mut self) -> Result<()> {
        self.session
            .disconnect()
            .map_err(|error| provider_stage("disconnect host", error))
    }

    pub fn status(&self) -> Result<VfsWorktreeStatus> {
        let store = self.client.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let (worktree, _, state) = self.locator.current(&reader)?;
        Ok(VfsWorktreeStatus {
            worktree_id: worktree.id().clone(),
            checkout_root: self.checkout_root().into(),
            backend: VFS_BACKEND,
            connected: Some(self.session.is_connected()),
            source_step_id: self.locator.source.step_id.clone(),
            accepted_step_id: state
                .base_step_id()
                .cloned()
                .ok_or(NativeClientError::Missing("VFS base"))?,
            active_operation_id: worktree.active_operation_id().clone(),
            capture_ack_pending: self.journal.state().pending_capture.is_some(),
            catalog_files: self.source.catalog().entries().len(),
            capture_blocked_reason: capture_blocked_reason(
                self.journal.state(),
                worktree.active_operation_id(),
                state
                    .base_step_id()
                    .ok_or(NativeClientError::Missing("VFS base"))?,
            ),
        })
    }
}

impl VfsWorktreeLocator {
    fn creation_lease(
        &self,
        source: &VerifiedVfsSnapshot,
        reservation: Option<&scm_v1::vfs_recovery::VfsRecoveryState>,
    ) -> Result<scm_v1::worktrees::WorktreeCreationLease> {
        let ticket = reservation
            .and_then(|state| state.ticket.as_ref())
            .map(|bytes| Registration::decode(bytes))
            .transpose()
            .map_err(provider_error)?;
        if let Some(ticket) = &ticket {
            validate_ticket(self, source, ticket)?;
        }
        Ok(scm_v1::worktrees::WorktreeCreationLease::acquire(
            &self.source.store_root,
            &self.source.store_identity,
            &self.source.store_metadata_identity,
            &self.source.store_id,
            self.checkout_root(),
            self.creation.worktree.filesystem_identity(),
            |path| {
                if let Some(ticket) = &ticket {
                    match ticket.verify(source.catalog()) {
                        Ok(()) => return Ok(ticket.filesystem_identity()),
                        Err(error) => {
                            // The hook can be durable before the first OS call.
                            // Only that phase may still be an empty normal root.
                            if reservation.map(|state| state.phase)
                                != Some(VfsPhase::ProviderPrepared)
                                || CheckoutReader::open(
                                    path,
                                    Some(self.creation.worktree.filesystem_identity()),
                                )
                                .is_err()
                            {
                                return Err(io::Error::other(error));
                            }
                        }
                    }
                }
                let checkout =
                    CheckoutReader::open(path, Some(self.creation.worktree.filesystem_identity()))?;
                if std::fs::read_dir(checkout.root())?
                    .next()
                    .transpose()?
                    .is_some()
                {
                    return Err(io::Error::other("pending VFS creation root is not empty"));
                }
                Ok(checkout.identity())
            },
        )?)
    }

    fn reserve(
        source: &NativeWorkspace,
        root: &Path,
        layer: &LayerId,
        step: &StepId,
        snapshot: &VerifiedVfsSnapshot,
    ) -> Result<Self> {
        let checkout = CheckoutReader::open(root, None)?;
        if std::fs::read_dir(checkout.root())?
            .next()
            .transpose()?
            .is_some()
        {
            return Err(NativeClientError::InvalidState(
                "VFS creation requires an empty root",
            ));
        }
        let binding = snapshot.binding();
        let stamp = MutationStamp::fresh()?;
        let metadata = Metadata::new(vec![MetadataEntry::new(VFS_BACKEND_KEY, VFS_BACKEND)?])?;
        let worktree = Worktree::new(
            WorktreeId::parse(format!("worktree:{}", uuid::Uuid::new_v4()))?,
            binding.store_id.clone(),
            binding.space_id.clone(),
            stamp.operation_id.clone(),
            checkout
                .root()
                .to_str()
                .ok_or(NativeClientError::InvalidState("VFS root is not Unicode"))?,
            checkout.identity(),
            stamp.timestamp,
            metadata,
        )?;
        let store = source.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        if reader.get_layer(layer)?.as_ref().map(Layer::space_id) != Some(&binding.space_id)
            || &binding.step_id != step
        {
            return Err(NativeClientError::InvalidState(
                "VFS Layer or Step binding differs",
            ));
        }
        // Overlap validation is performed by create before durable reservation.
        let locator = Self {
            schema: LOCATOR_SCHEMA.into(),
            backend: VFS_BACKEND.into(),
            creation: WorktreeCreation {
                worktree,
                source_worktree_id: source.worktree_id().clone(),
                source_operation_id: binding.operation_id.clone(),
                layer_id: layer.clone(),
                step_id: step.clone(),
            },
            source: binding,
            root_id: *uuid::Uuid::new_v4().as_bytes(),
            registration_transaction: stamp.transaction_id.to_string(),
        };
        locator.encode()?;
        Ok(locator)
    }

    fn open_client(&self) -> Result<NativeClient> {
        let client = NativeClient::open(&self.source.store_root)?;
        if client.store_identity != self.source.store_identity
            || client.metadata_identity() != self.source.store_metadata_identity
        {
            return Err(NativeClientError::InvalidState(
                "VFS Store filesystem identity changed",
            ));
        }
        let store = client.open_store()?;
        if store.store_id() != &self.source.store_id {
            return Err(NativeClientError::InvalidState("VFS Store ID changed"));
        }
        drop(store);
        Ok(client)
    }

    fn current(
        &self,
        reader: &impl ScmReader,
    ) -> Result<(Worktree, RepositoryView, WorktreeViewState)> {
        let worktree = reader
            .get_worktree(self.worktree_id())?
            .ok_or(NativeClientError::Missing("VFS Worktree"))?;
        let reserved = &self.creation.worktree;
        if worktree.store_id() != reserved.store_id()
            || worktree.space_id() != reserved.space_id()
            || worktree.checkout_root() != reserved.checkout_root()
            || worktree.filesystem_identity() != reserved.filesystem_identity()
            || worktree.created_at() != reserved.created_at()
            || worktree.metadata() != reserved.metadata()
        {
            return Err(NativeClientError::InvalidState(
                "registered VFS binding changed",
            ));
        }
        let operation = reader
            .get_operation(worktree.active_operation_id())?
            .ok_or(NativeClientError::Missing("VFS Operation"))?;
        let view = reader
            .get_repository_view(operation.resulting_view_id())?
            .ok_or(NativeClientError::Missing("VFS View"))?;
        if operation.space_id() != worktree.space_id() || view.space_id() != worktree.space_id() {
            return Err(NativeClientError::InvalidState(
                "VFS Operation Space mismatch",
            ));
        }
        let state = view
            .worktrees()
            .iter()
            .find(|state| state.worktree_id() == worktree.id())
            .cloned()
            .ok_or(NativeClientError::Missing("VFS View state"))?;
        if state.active_layer_id() != Some(&self.creation.layer_id) {
            return Err(NativeClientError::InvalidState(
                "VFS backend does not support Layer switching",
            ));
        }
        Ok((worktree, view, state))
    }

    fn register(
        &self,
        client: &NativeClient,
        creation_lease: &scm_v1::worktrees::WorktreeCreationLease,
    ) -> Result<()> {
        for _ in 0..8 {
            let gate = creation_lease.registration_gate()?;
            let store = client.open_store_for_registration()?;
            let reader = NativeScmReader::new(&store)?;
            if reader.get_worktree(self.worktree_id())?.is_some() {
                self.current(&reader)?;
                return Ok(());
            }
            // Recheck the complete registered namespace for every CAS attempt.
            // All creation backends retain this gate through acceptance, even
            // across Spaces whose repository-view CAS values are independent.
            scm_v1::worktrees::validate_secondary_root_with_vfs(
                &store,
                self.checkout_root(),
                |worktree| verify_registered_vfs_root(&store, worktree).map_err(io::Error::other),
            )?;
            let parent = reader.get_operation(&self.source.operation_id)?.ok_or(
                NativeClientError::Missing("VFS registration source Operation"),
            )?;
            let source_view = reader
                .get_repository_view(parent.resulting_view_id())?
                .ok_or(NativeClientError::Missing("VFS registration source View"))?;
            if parent.space_id() != &self.source.space_id
                || source_view.space_id() != &self.source.space_id
            {
                return Err(NativeClientError::InvalidState(
                    "VFS registration source Space mismatch",
                ));
            }
            let empty = Tree::new(vec![])?;
            let mut states = source_view.worktrees().to_vec();
            states.push(WorktreeViewState::new(
                self.worktree_id().clone(),
                Some(self.creation.layer_id.clone()),
                Some(self.creation.step_id.clone()),
                Some(empty.id().clone()),
                WorktreeStatus::Stale,
            ));
            let mut layers = source_view.layer_heads().to_vec();
            if !layers
                .iter()
                .any(|layer| layer.layer_id() == &self.creation.layer_id)
            {
                layers.push(LayerHeadSet::new(
                    self.creation.layer_id.clone(),
                    vec![self.creation.step_id.clone()],
                )?);
            }
            let view = RepositoryView::new(
                self.source.space_id.clone(),
                layers,
                source_view.marker_ids().to_vec(),
                source_view.release_ids().to_vec(),
                states,
            )?;
            let mut stamp = MutationStamp::fresh()?;
            stamp.transaction_id = TransactionId::new(&self.registration_transaction)?;
            stamp.operation_id = self.creation.worktree.active_operation_id().clone();
            stamp.timestamp = self.creation.worktree.created_at().clone();
            let operation = prepare::operation(
                &stamp,
                self.source.space_id.clone(),
                vec![parent.id().clone()],
                &view,
                OperationKind::Restore,
                "Register VFS Worktree pending provider population",
            )?;
            let current = reader
                .current_repository_view(&self.source.space_id)?
                .ok_or(NativeClientError::Missing("current View"))?;
            let heads = reader.operation_heads(&self.source.space_id)?;
            let objects = vec![
                CanonicalObject::Tree(empty),
                CanonicalObject::RepositoryView(view.clone()),
            ];
            let batch = WriteBatch::new(
                stamp.transaction_id,
                objects.clone(),
                vec![
                    EntityRecord::Worktree(self.creation.worktree.clone()),
                    EntityRecord::Operation(operation.clone()),
                ],
                prepare::admissions(&self.source.space_id, &objects),
                vec![
                    StoreMutation::RepositoryView(RepositoryViewCas::new(
                        self.source.space_id.clone(),
                        current.generation(),
                        current.view_id().cloned(),
                        view.id().clone(),
                    )?),
                    StoreMutation::OperationHeads(
                        OperationHeadsCas::new(
                            self.source.space_id.clone(),
                            heads.clone(),
                            operation.id().clone(),
                            worktree_operations::advance_heads(&heads, parent.id(), operation.id()),
                        )?
                        .with_worktree_registration_source(
                            self.creation.source_worktree_id.clone(),
                        ),
                    ),
                ],
            )?;
            drop(reader);
            drop(store);
            match client.repository().apply_batch_guarded(&batch, || {
                gate.verify().map_err(|error| StoreError::Backend {
                    message: error.to_string(),
                })?;
                let root = CheckoutReader::open(
                    self.checkout_root(),
                    Some(self.creation.worktree.filesystem_identity()),
                )
                .map_err(|error| StoreError::Backend {
                    message: error.to_string(),
                })?;
                if std::fs::read_dir(root.root())
                    .map_err(|error| StoreError::Backend {
                        message: error.to_string(),
                    })?
                    .next()
                    .transpose()
                    .map_err(|error| StoreError::Backend {
                        message: error.to_string(),
                    })?
                    .is_some()
                {
                    return Err(StoreError::Backend {
                        message: "VFS root changed before registration acceptance".into(),
                    });
                }
                Ok(())
            }) {
                Err(StoreError::CasMismatch { .. }) => continue,
                other => {
                    other?;
                    return Ok(());
                }
            }
        }
        Err(NativeClientError::StaleCapture)
    }

    fn mark_ready(
        &self,
        client: &NativeClient,
        journal: &mut VfsRecovery,
        faults: &dyn VfsWorktreeFaults,
    ) -> Result<()> {
        let journal_faults = JournalFaults(faults);
        for _ in 0..8 {
            if journal.resolve_activation(&journal_faults)? == Some(true) {
                return Ok(());
            }
            if journal.state().phase != VfsPhase::LocatorPublished {
                return Err(NativeClientError::InvalidState(
                    "VFS activation lacks a published locator",
                ));
            }
            let store = client.open_store()?;
            let reader = NativeScmReader::new(&store)?;
            let (worktree, old_view, state) = self.current(&reader)?;
            let step = reader
                .get_step(&self.creation.step_id)?
                .ok_or(NativeClientError::Missing("VFS base Step"))?;
            if state.base_step_id() != Some(step.id()) {
                return Err(NativeClientError::InvalidState("pending VFS base changed"));
            }
            if state.status() != WorktreeStatus::Stale
                || worktree.active_operation_id() != self.creation.worktree.active_operation_id()
            {
                return Err(NativeClientError::InvalidState(
                    "VFS Worktree is not pending creation",
                ));
            }
            let states = old_view
                .worktrees()
                .iter()
                .map(|old| {
                    if old.worktree_id() == worktree.id() {
                        WorktreeViewState::new(
                            worktree.id().clone(),
                            Some(self.creation.layer_id.clone()),
                            Some(step.id().clone()),
                            Some(step.root_tree_id().clone()),
                            WorktreeStatus::Ready,
                        )
                    } else {
                        old.clone()
                    }
                })
                .collect();
            let view = RepositoryView::new(
                worktree.space_id().clone(),
                old_view.layer_heads().to_vec(),
                old_view.marker_ids().to_vec(),
                old_view.release_ids().to_vec(),
                states,
            )?;
            let stamp = MutationStamp::fresh()?;
            let operation = prepare::operation(
                &stamp,
                worktree.space_id().clone(),
                vec![worktree.active_operation_id().clone()],
                &view,
                OperationKind::Restore,
                "Activate populated VFS Worktree",
            )?;
            let current = reader
                .current_repository_view(worktree.space_id())?
                .ok_or(NativeClientError::Missing("current View"))?;
            let heads = reader.operation_heads(worktree.space_id())?;
            let objects = vec![CanonicalObject::RepositoryView(view.clone())];
            let batch = WriteBatch::new(
                stamp.transaction_id,
                objects.clone(),
                vec![EntityRecord::Operation(operation.clone())],
                prepare::admissions(worktree.space_id(), &objects),
                vec![
                    StoreMutation::RepositoryView(RepositoryViewCas::new(
                        worktree.space_id().clone(),
                        current.generation(),
                        current.view_id().cloned(),
                        view.id().clone(),
                    )?),
                    StoreMutation::OperationHeads(OperationHeadsCas::new(
                        worktree.space_id().clone(),
                        heads.clone(),
                        operation.id().clone(),
                        worktree_operations::advance_heads(
                            &heads,
                            worktree.active_operation_id(),
                            operation.id(),
                        ),
                    )?),
                    StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                        worktree.id().clone(),
                        worktree.active_operation_id().clone(),
                        operation.id().clone(),
                    )?),
                ],
            )?;
            drop(reader);
            drop(store);
            journal.prepare_activation(
                &batch,
                &self.creation.step_id,
                self.creation.worktree.active_operation_id(),
                &journal_faults,
            )?;
            let accepted = client.accept(&PreparedMutation {
                batch,
                step_id: self.creation.step_id.clone(),
                chunks: None,
            });
            if accepted.is_ok() {
                faults.after_activation_accept()?;
            }
            if journal.resolve_activation(&journal_faults)? == Some(true) {
                return Ok(());
            }
            match accepted {
                Err(NativeClientError::Store(StoreError::CasMismatch { .. })) => continue,
                Err(error) => return Err(error),
                Ok(_) => {
                    return Err(NativeClientError::InvalidState(
                        "VFS activation acceptance has no durable receipt",
                    ))
                }
            }
        }
        Err(NativeClientError::StaleCapture)
    }
}

/// Backend routing without owning the live provider, taking its host lease, or
/// hydrating file contents. A VFS-looking invalid locator never returns None.
pub fn inspect_vfs_checkout(root: &Path) -> Result<Option<VfsWorktreeStatus>> {
    let root = std::path::absolute(root)?;
    let metadata = match std::fs::symlink_metadata(root.join(".layrs")) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Ok(None); // Ordinary open performs its own reparse/format checks.
    }
    let (bytes, observed_control) = layrs_vfs::read_control_file(&root)
        .map_err(|error| provider_stage("inspect checkout locator", error))?;
    let header: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if header.get("schema").and_then(serde_json::Value::as_str) == Some("layrs.worktree-locator.v1")
    {
        return Ok(None);
    }
    let locator = VfsWorktreeLocator::decode(&bytes)?;
    if root != locator.checkout_root() {
        return Err(NativeClientError::InvalidState("VFS locator root differs"));
    }
    let client = locator.open_client()?;
    let store = client.open_store()?;
    let reader = NativeScmReader::new(&store)?;
    let (worktree, _, state) = locator.current(&reader)?;
    let journal = VfsRecovery::read_registration(&store, &worktree)?;
    if journal.locator != bytes || journal.control.as_ref() != Some(&observed_control.encode()) {
        return Err(NativeClientError::InvalidState(
            "VFS control file differs from private journal",
        ));
    }
    let catalog = verified_registration(&store, &worktree, &journal)?.1;
    Ok(Some(VfsWorktreeStatus {
        worktree_id: worktree.id().clone(),
        checkout_root: root,
        backend: VFS_BACKEND,
        connected: None,
        source_step_id: locator.source.step_id.clone(),
        accepted_step_id: state
            .base_step_id()
            .cloned()
            .ok_or(NativeClientError::Missing("VFS base Step"))?,
        active_operation_id: worktree.active_operation_id().clone(),
        capture_ack_pending: journal.pending_capture.is_some(),
        catalog_files: catalog.entries().len(),
        capture_blocked_reason: capture_blocked_reason(
            &journal,
            worktree.active_operation_id(),
            state
                .base_step_id()
                .ok_or(NativeClientError::Missing("VFS base Step"))?,
        ),
    }))
}

/// Callback for backend-aware native overlap validation. No connection, host
/// lease or file content reads are performed; unknown/malformed backends fail.
pub fn verify_registered_vfs_root(
    store: &scm_v1::NativeStoreOpen,
    worktree: &Worktree,
) -> Result<String> {
    let journal = VfsRecovery::read_registration(store, worktree)?;
    let (locator, ticket) = verified_registration_binding(store, worktree, &journal)?;
    ticket
        .verify_root_identity()
        .map_err(|error| provider_stage("verify registered provider identity", error))?;
    if matches!(
        journal.phase,
        VfsPhase::LocatorPublished | VfsPhase::ActivationPrepared | VfsPhase::Ready
    ) {
        let (bytes, control) = layrs_vfs::read_control_file(locator.checkout_root())
            .map_err(|error| provider_stage("verify registered locator identity", error))?;
        if bytes != journal.locator || journal.control.as_ref() != Some(&control.encode()) {
            return Err(NativeClientError::InvalidState(
                "VFS registered control differs from private journal",
            ));
        }
    }
    store.verify_filesystem_identity()?;
    Ok(ticket.filesystem_identity())
}

fn verified_registration(
    store: &scm_v1::NativeStoreOpen,
    worktree: &Worktree,
    journal: &scm_v1::vfs_recovery::VfsRecoveryState,
) -> Result<(Registration, layrs_vfs::Catalog)> {
    let (locator, ticket) = verified_registration_binding(store, worktree, journal)?;
    let catalog = metadata_catalog(store, &locator.source)?;
    ticket
        .verify(&catalog)
        .map_err(|error| provider_stage("inspect registered provider root", error))?;
    store.verify_filesystem_identity()?;
    Ok((ticket, catalog))
}

/// Identity and containment do not depend on whether this version can serve
/// every canonical source entry. Unsupported historical mounts still exclude
/// their own root while allowing unrelated Worktree creation.
fn verified_registration_binding(
    store: &scm_v1::NativeStoreOpen,
    worktree: &Worktree,
    journal: &scm_v1::vfs_recovery::VfsRecoveryState,
) -> Result<(VfsWorktreeLocator, Registration)> {
    store.verify_filesystem_identity()?;
    let locator = VfsWorktreeLocator::decode(&journal.locator)?;
    if !is_vfs_worktree(worktree)
        || locator.worktree_id() != worktree.id()
        || locator.source.store_id != *store.store_id()
        || locator.source.store_root != store.layout().root()
        || locator.creation.worktree.checkout_root() != worktree.checkout_root()
        || locator.creation.worktree.filesystem_identity() != worktree.filesystem_identity()
        || locator.creation.worktree.space_id() != worktree.space_id()
        || locator.creation.worktree.metadata() != worktree.metadata()
    {
        return Err(NativeClientError::InvalidState(
            "VFS registration locator binding mismatch",
        ));
    }
    CheckoutReader::open(store.layout().root(), Some(&locator.source.store_identity))?;
    CheckoutReader::open(
        store.layout().metadata_dir(),
        Some(&locator.source.store_metadata_identity),
    )?;
    let ticket = Registration::decode(
        journal
            .ticket
            .as_ref()
            .ok_or(NativeClientError::Missing("VFS registration ticket"))?,
    )
    .map_err(provider_error)?;
    if ticket.root() != locator.checkout_root()
        || ticket.root_id() != locator.root_id
        || ticket.filesystem_identity() != worktree.filesystem_identity()
    {
        return Err(NativeClientError::InvalidState(
            "VFS native registration binding mismatch",
        ));
    }
    store.verify_filesystem_identity()?;
    Ok((locator, ticket))
}

fn metadata_catalog(
    store: &scm_v1::NativeStoreOpen,
    binding: &VerifiedVfsSnapshotBinding,
) -> Result<layrs_vfs::Catalog> {
    let reader = NativeScmReader::new(store)?;
    if reader
        .get_operation(&binding.operation_id)?
        .as_ref()
        .map(Operation::space_id)
        != Some(&binding.space_id)
        || !reader.operation_reaches_step(&binding.operation_id, &binding.step_id)?
        || !reader.object_admitted_to_space(
            &StoreObjectId::Step(binding.step_id.clone()),
            &binding.space_id,
        )?
    {
        return Err(NativeClientError::InvalidState(
            "VFS metadata source is not retained and admitted",
        ));
    }
    let step = reader
        .get_step(&binding.step_id)?
        .ok_or(NativeClientError::Missing("VFS source Step"))?;
    let mut pending = vec![(String::new(), step.root_tree_id().clone(), 0)];
    let mut entries = Vec::new();
    let mut lengths = std::collections::BTreeMap::new();
    let mut visited = 0usize;
    let mut total = 0u64;
    while let Some((prefix, tree, depth)) = pending.pop() {
        // Provider budgets count user entries, not the implicit checkout root.
        visited += usize::from(!prefix.is_empty());
        if depth > 64 || visited + entries.len() + pending.len() > 100_000 {
            return Err(NativeClientError::InvalidState(
                "VFS catalog metadata exceeds bounds",
            ));
        }
        let tree = reader
            .get_tree(&tree)?
            .ok_or(NativeClientError::Missing("VFS source Tree"))?;
        super::vfs_source::validate_vfs_tree_placement(!prefix.is_empty(), &tree)?;
        for entry in tree.entries() {
            let path = if prefix.is_empty() {
                entry.name().as_str().to_owned()
            } else {
                format!("{prefix}/{}", entry.name().as_str())
            };
            match entry.target() {
                TreeEntryTarget::Tree { tree_id } => {
                    pending.push((path, tree_id.clone(), depth + 1))
                }
                TreeEntryTarget::File { file_id } if !entry.executable() => {
                    let size = match lengths.get(file_id) {
                        Some(size) => *size,
                        None => {
                            let size = VfsRecovery::file_length_metadata(store, file_id)?;
                            lengths.insert(file_id.clone(), size);
                            size
                        }
                    };
                    total = total
                        .checked_add(size)
                        .filter(|n| *n <= 4 * 1024 * 1024 * 1024)
                        .ok_or(NativeClientError::InvalidState("VFS catalog byte limit"))?;
                    entries.push(layrs_vfs::CatalogEntry {
                        path: RepoPath::parse(path)?,
                        file_id: file_id.clone(),
                        size,
                    });
                }
                _ => {
                    return Err(NativeClientError::InvalidState(
                        "unsupported VFS catalog entry",
                    ))
                }
            }
            if visited + entries.len() + pending.len() > 100_000 {
                return Err(NativeClientError::InvalidState("VFS catalog entry limit"));
            }
        }
    }
    layrs_vfs::Catalog::new(1, entries).map_err(provider_error)
}

/// Retain the destination and every ancestor identity through publication. A
/// case alias of the provider root is rejected by native identity, before any
/// checkpoint read, reservation or output creation.
struct VfsExportParent<'session> {
    directories: Vec<CheckoutReader>,
    guard: layrs_vfs::ExportParentGuard<'session>,
}
impl<'session> VfsExportParent<'session> {
    fn open(parent: &Path, session: &'session Session) -> Result<Self> {
        let absolute = std::path::absolute(parent)?;
        let paths: Vec<_> = absolute.ancestors().collect();
        let mut directories = Vec::with_capacity(paths.len());
        let provider_identity = session.registration().filesystem_identity();
        for path in paths.into_iter().rev() {
            if session
                .is_owned_root_directory(path)
                .map_err(|error| provider_stage("inspect export parent ancestor", error))?
            {
                return Err(NativeClientError::InvalidState(
                    "VFS recovery export parent must be outside the provider checkout",
                ));
            }
            let directory = CheckoutReader::open(path, None)?;
            if directory.identity() == provider_identity {
                return Err(NativeClientError::InvalidState(
                    "VFS recovery export parent must be outside the provider checkout",
                ));
            }
            directories.push(directory);
        }
        let guard = session
            .guard_export_parent(&absolute)
            .map_err(|error| provider_stage("retain export parent", error))?;
        Self::bind_guard(directories, guard)
    }
    fn bind_guard(
        directories: Vec<CheckoutReader>,
        guard: layrs_vfs::ExportParentGuard<'session>,
    ) -> Result<Self> {
        // CheckoutReader pins allow deletion. Bind their original identity to
        // the newly acquired no-delete guard before any checkpoint or output.
        if directories.last().map(CheckoutReader::identity) != Some(guard.filesystem_identity()) {
            return Err(NativeClientError::InvalidState(
                "VFS export parent identity changed during acquisition",
            ));
        }
        let parent = Self { directories, guard };
        parent.verify()?;
        Ok(parent)
    }
    fn root(&self) -> &Path {
        self.directories
            .last()
            .expect("absolute path has a root")
            .root()
    }
    fn verify(&self) -> Result<()> {
        self.guard
            .revalidate()
            .map_err(|error| provider_stage("revalidate retained export parent", error))?;
        for directory in &self.directories {
            CheckoutReader::open(directory.root(), Some(&directory.identity()))?;
        }
        Ok(())
    }
}

fn provider_error(error: layrs_vfs::Error) -> NativeClientError {
    provider_stage("validate provider metadata", error)
}

#[derive(Debug)]
struct VfsLifecycleError {
    stage: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for VfsLifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VFS {}: {}", self.stage, self.source)
    }
}
impl std::error::Error for VfsLifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn provider_stage(stage: &'static str, error: layrs_vfs::Error) -> NativeClientError {
    // Preserve the native I/O error and its kind instead of erasing it beneath
    // another provider wrapper. Never include locator/endpoint bytes in context.
    let (kind, source): (_, Box<dyn std::error::Error + Send + Sync>) = match error {
        layrs_vfs::Error::Io(error) => (error.kind(), Box::new(error)),
        error => (io::ErrorKind::Other, Box::new(error)),
    };
    NativeClientError::Filesystem(io::Error::new(
        kind,
        VfsLifecycleError {
            stage: stage.into(),
            source,
        },
    ))
}

fn creation_error(
    id: &WorktreeId,
    root: &Path,
    error: impl std::error::Error + Send + Sync + 'static,
) -> NativeClientError {
    NativeClientError::Filesystem(io::Error::other(VfsLifecycleError {
        stage: format!(
            "creation failed at {} (recovery Worktree ID: {id}); retained files and journal were not removed",
            root.display(),
        ),
        source: Box::new(error),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_scm::{SnapshotBuilder, SnapshotLimits};

    // Reproduce the old producer using actual Store acceptance and native
    // registration, without adding a production representability bypass.
    fn accept_test_image(
        client: &NativeClient,
        id: &WorktreeId,
        layer: &LayerId,
        step: &Step,
        mut objects: Vec<CanonicalObject>,
    ) -> OperationId {
        let store = client.open_store().unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let worktree = reader.get_worktree(id).unwrap().unwrap();
        let active = reader
            .get_operation(worktree.active_operation_id())
            .unwrap()
            .unwrap();
        let old = reader
            .get_repository_view(active.resulting_view_id())
            .unwrap()
            .unwrap();
        let view = RepositoryView::new(
            worktree.space_id().clone(),
            old.layer_heads()
                .iter()
                .map(|head| {
                    if head.layer_id() == layer {
                        LayerHeadSet::new(layer.clone(), vec![step.id().clone()]).unwrap()
                    } else {
                        head.clone()
                    }
                })
                .collect(),
            old.marker_ids().to_vec(),
            old.release_ids().to_vec(),
            old.worktrees()
                .iter()
                .map(|entry| {
                    if entry.worktree_id() == id {
                        WorktreeViewState::new(
                            id.clone(),
                            Some(layer.clone()),
                            Some(step.id().clone()),
                            Some(step.root_tree_id().clone()),
                            WorktreeStatus::Ready,
                        )
                    } else {
                        entry.clone()
                    }
                })
                .collect(),
        )
        .unwrap();
        let stamp = MutationStamp::fresh().unwrap();
        let operation = prepare::operation(
            &stamp,
            worktree.space_id().clone(),
            vec![active.id().clone()],
            &view,
            OperationKind::Restore,
            "Legacy VFS fixture canonical image",
        )
        .unwrap();
        let current = reader
            .current_repository_view(worktree.space_id())
            .unwrap()
            .unwrap();
        let heads = reader.operation_heads(worktree.space_id()).unwrap();
        objects.push(CanonicalObject::RepositoryView(view.clone()));
        let batch = WriteBatch::new(
            stamp.transaction_id,
            objects.clone(),
            vec![EntityRecord::Operation(operation.clone())],
            prepare::admissions(worktree.space_id(), &objects),
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        worktree.space_id().clone(),
                        current.generation(),
                        current.view_id().cloned(),
                        view.id().clone(),
                    )
                    .unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        worktree.space_id().clone(),
                        heads.clone(),
                        operation.id().clone(),
                        worktree_operations::advance_heads(&heads, active.id(), operation.id()),
                    )
                    .unwrap(),
                ),
                StoreMutation::WorktreeActiveOperation(
                    WorktreeActiveOperationCas::new(
                        id.clone(),
                        active.id().clone(),
                        operation.id().clone(),
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap();
        drop(reader);
        drop(store);
        client
            .accept(&PreparedMutation {
                batch,
                step_id: step.id().clone(),
                chunks: None,
            })
            .unwrap();
        operation.id().clone()
    }

    struct LegacyMount {
        parent: PathBuf,
        workspace: NativeWorkspace,
        locator: VfsWorktreeLocator,
        supported_step: StepId,
        session: Session,
        journal: VfsRecovery,
    }

    fn append_legacy_ready_fixture(
        mut journal: VfsRecovery,
        locator: &VfsWorktreeLocator,
    ) -> VfsRecovery {
        assert!(journal
            .append(VfsEvent::Ready, &scm_v1::vfs_recovery::NoVfsJournalFaults)
            .is_err());
        let directory = journal.staging_root().unwrap();
        drop(journal);
        // Construct one old-producer record only in this fresh private fixture.
        // There is deliberately no production writer for legacy readiness.
        let previous = std::fs::read(directory.join("00000000005.json")).unwrap();
        let mut record: serde_json::Value = serde_json::from_slice(&previous).unwrap();
        assert_eq!(record["sequence"], 5);
        assert_eq!(record["event"]["type"], "locator_published");
        record["sequence"] = serde_json::json!(6);
        record["previousDigest"] = serde_json::json!(blake3::hash(&previous).to_hex().to_string());
        record["event"] = serde_json::json!({ "type": "ready" });
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("00000000006.json"))
            .unwrap();
        output
            .write_all(&serde_json::to_vec(&record).unwrap())
            .unwrap();
        output.sync_all().unwrap();
        drop(output);
        VfsRecovery::open(
            &locator.source.store_root,
            &locator.source.store_identity,
            &locator.source.store_metadata_identity,
            &locator.source.store_id,
            locator.worktree_id(),
        )
        .unwrap()
    }

    fn legacy_mount(unsupported: bool) -> LegacyMount {
        let parent = PathBuf::from(
            std::env::var_os("LAYRS_VFS_TEST_PARENT")
                .expect("explicit disposable LAYRS_VFS_TEST_PARENT required"),
        );
        assert!(parent.is_absolute() && parent.is_dir());
        let parent = parent.join(format!("legacy-vfs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&parent).unwrap();
        eprintln!("retained legacy VFS fixture {}", parent.display());
        let ordinary = parent.join("ordinary");
        let root = parent.join("virtual");
        std::fs::create_dir(&ordinary).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(ordinary.join("file"), b"immutable source").unwrap();
        let workspace = NativeWorkspace::initialize(&ordinary, "Legacy VFS fixture").unwrap();
        let state = workspace.state().unwrap();
        let own = state
            .view
            .worktrees()
            .iter()
            .find(|entry| entry.worktree_id() == workspace.worktree_id())
            .unwrap();
        let layer = own.active_layer_id().unwrap().clone();
        let supported_step = own.base_step_id().unwrap().clone();
        let source = Arc::new(
            VerifiedVfsSnapshot::prepare(&workspace, &supported_step, &Cancellation::default())
                .unwrap(),
        );
        let mut locator =
            VfsWorktreeLocator::reserve(&workspace, &root, &layer, &supported_step, &source)
                .unwrap();
        let client = locator.open_client().unwrap();
        let mut step = {
            let store = workspace.open_store().unwrap();
            NativeScmReader::new(&store)
                .unwrap()
                .get_step(&supported_step)
                .unwrap()
                .unwrap()
        };
        if unsupported {
            let empty = Tree::new(vec![]).unwrap();
            let mut entries = {
                let store = workspace.open_store().unwrap();
                NativeScmReader::new(&store)
                    .unwrap()
                    .get_tree(step.root_tree_id())
                    .unwrap()
                    .unwrap()
                    .entries()
                    .to_vec()
            };
            entries.push(TreeEntry::new(
                RepoPathComponent::parse("empty").unwrap(),
                TreeEntryTarget::tree(empty.id().clone()),
                false,
            ));
            let tree = Tree::new(entries).unwrap();
            step = Step::new(vec![supported_step.clone()], tree.id().clone()).unwrap();
            let operation = accept_test_image(
                &client,
                workspace.worktree_id(),
                &layer,
                &step,
                vec![
                    CanonicalObject::Tree(empty),
                    CanonicalObject::Tree(tree),
                    CanonicalObject::Step(step.clone()),
                ],
            );
            locator.source.operation_id = operation.clone();
            locator.source.step_id = step.id().clone();
            locator.creation.source_operation_id = operation;
            locator.creation.step_id = step.id().clone();
        }
        let lease = locator.creation_lease(&source, None).unwrap();
        let mut journal = VfsRecovery::create(
            &locator.source.store_root,
            &locator.source.store_identity,
            &locator.source.store_metadata_identity,
            &locator.source.store_id,
            locator.worktree_id(),
            locator.encode().unwrap(),
            &scm_v1::vfs_recovery::NoVfsJournalFaults,
        )
        .unwrap();
        locator.register(&client, &lease).unwrap();
        let faults = scm_v1::vfs_recovery::NoVfsJournalFaults;
        journal.append(VfsEvent::Registered, &faults).unwrap();
        // The supported source has exactly the historical flattened catalog.
        // It is used solely to construct this old on-disk/native fixture.
        let mut session =
            Session::create_with_registration_hook(&root, locator.root_id, source, |ticket| {
                journal
                    .append(
                        VfsEvent::ProviderPrepared {
                            ticket: ticket.encode(),
                        },
                        &faults,
                    )
                    .map_err(layrs_vfs::Error::Io)
            })
            .unwrap();
        journal
            .append(VfsEvent::ProviderPopulated, &faults)
            .unwrap();
        let staging = journal.staging_root().unwrap();
        let control = session
            .publish_control_file_recoverable(
                &locator.encode().unwrap(),
                &staging,
                None,
                |control| {
                    journal
                        .append(
                            VfsEvent::ControlPrepared {
                                control: control.encode(),
                            },
                            &faults,
                        )
                        .map_err(layrs_vfs::Error::Io)
                },
                &layrs_vfs::NoControlPublishFaults,
            )
            .unwrap();
        journal
            .append(
                VfsEvent::LocatorPublished {
                    control: control.encode(),
                },
                &faults,
            )
            .unwrap();
        if unsupported {
            locator
                .mark_ready(&client, &mut journal, &NoVfsWorktreeFaults)
                .unwrap();
        } else {
            accept_test_image(&client, locator.worktree_id(), &layer, &step, vec![]);
            journal = append_legacy_ready_fixture(journal, &locator);
        }
        drop(lease);
        LegacyMount {
            parent,
            workspace,
            locator,
            supported_step,
            session,
            journal,
        }
    }

    #[test]
    #[ignore = "real CFAPI retained export-parent guards; explicit disposable parent required"]
    fn export_parent_retains_sharing_guards_until_drop() {
        let mount = legacy_mount(false);
        let outer = mount.parent.join("ordinary-exports");
        let parent = outer.join("parent");
        std::fs::create_dir(&outer).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let moved_parent = outer.join("moved-parent");
        let moved_outer = mount.parent.join("moved-ordinary-exports");
        let pinned = VfsExportParent::open(&parent, &mount.session).unwrap();
        let expected = CheckoutReader::open(&parent, None).unwrap().identity();
        pinned.verify().unwrap();

        // Both directories are outside the provider root: these failures must
        // come from the retained export capability, not a provider leaf guard.
        for (operation, result) in [
            ("rename parent", std::fs::rename(&parent, &moved_parent)),
            ("rename ancestor", std::fs::rename(&outer, &moved_outer)),
            ("delete empty parent", std::fs::remove_dir(&parent)),
        ] {
            let error = result.expect_err(operation);
            assert_eq!(error.raw_os_error(), Some(32), "{operation}: {error:?}");
        }
        pinned.verify().unwrap();
        assert_eq!(
            CheckoutReader::open(&parent, None).unwrap().identity(),
            expected
        );
        assert!(std::fs::read_dir(&parent).unwrap().next().is_none());
        assert!(!moved_parent.exists() && !moved_outer.exists());

        drop(pinned);
        // Positive controls prove the retained capability was the sharing
        // restriction and releases every affected handle when dropped.
        std::fs::rename(&parent, &moved_parent).unwrap();
        std::fs::rename(&moved_parent, &parent).unwrap();
        std::fs::rename(&outer, &moved_outer).unwrap();
        std::fs::rename(&moved_outer, &outer).unwrap();
        std::fs::remove_dir(&parent).unwrap();
    }

    #[test]
    #[ignore = "real CFAPI export-parent identity interleave; explicit disposable parent required"]
    fn export_parent_rejects_replacement_between_ordinary_pin_and_guard() {
        let mount = legacy_mount(false);
        let parent = mount.parent.join("replaceable-export-parent");
        let retained = mount.parent.join("retained-original-parent");
        std::fs::create_dir(&parent).unwrap();
        let absolute = std::path::absolute(&parent).unwrap();
        let paths: Vec<_> = absolute.ancestors().collect();
        let ordinary_pins: Vec<_> = paths
            .into_iter()
            .rev()
            .map(|path| CheckoutReader::open(path, None).unwrap())
            .collect();
        let original_identity = ordinary_pins.last().unwrap().identity();

        // Deterministically interleave after ordinary acquisition. These old
        // share-all pins allow replacement, so the new provider guard alone
        // proves a different directory and must not validate the old pins.
        std::fs::rename(&parent, &retained).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("replacement-sentinel"), b"unrelated bytes").unwrap();
        let guard = mount.session.guard_export_parent(&absolute).unwrap();
        assert_eq!(guard.root(), absolute);
        assert_ne!(guard.filesystem_identity(), original_identity);
        let error = match VfsExportParent::bind_guard(ordinary_pins, guard) {
            Ok(_) => panic!("new retained guard must not validate an old ordinary identity"),
            Err(error) => error,
        };
        assert!(
            matches!(
                &error,
                NativeClientError::InvalidState(
                    "VFS export parent identity changed during acquisition"
                )
            ),
            "{error:?}"
        );
        assert_eq!(
            CheckoutReader::open(&retained, None).unwrap().identity(),
            original_identity
        );
        assert!(std::fs::read_dir(&retained).unwrap().next().is_none());
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(parent.join("replacement-sentinel")).unwrap(),
            b"unrelated bytes"
        );
        // The failed pairing consumes/drops its guard instead of leaking a
        // sharing restriction over the unrelated replacement directory.
        let moved = mount.parent.join("replacement-after-refusal");
        std::fs::rename(&parent, &moved).unwrap();
        std::fs::rename(&moved, &parent).unwrap();
    }

    fn write_legacy_user_bytes(root: &Path) {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "native_scm::vfs_worktree::tests::legacy_child_write",
                "--nocapture",
            ])
            .env("LAYRS_VFS_LEGACY_WRITE_ROOT", root)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "legacy writer child exited with {status}");
                return;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("legacy writer child timed out");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    #[ignore = "helper invoked only by exact legacy activation recipe"]
    fn legacy_child_write() {
        let root = PathBuf::from(std::env::var_os("LAYRS_VFS_LEGACY_WRITE_ROOT").unwrap());
        // The actual placeholder is opened by an external process; no ordinary
        // replacement or pre-filled source substitutes for this private write.
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(root.join("file"))
            .unwrap();
        output.write_all(b"legacy unaccepted user bytes").unwrap();
        output.sync_all().unwrap();
    }

    #[test]
    #[ignore = "real CFAPI legacy activation fixture; explicit disposable parent required"]
    fn legacy_ready_without_activation_allows_reopen_export_but_refuses_capture() {
        let mount = legacy_mount(false);
        let root = mount.locator.checkout_root().to_path_buf();
        let operation = {
            let store = mount.workspace.open_store().unwrap();
            NativeScmReader::new(&store)
                .unwrap()
                .worktree_active_operation(mount.locator.worktree_id())
                .unwrap()
                .unwrap()
        };
        write_legacy_user_bytes(&root);
        drop(mount.session);
        drop(mount.journal);
        for _ in 0..2 {
            let mut reopened = VfsWorktree::open(&root).unwrap();
            let status = reopened.status().unwrap();
            assert_eq!(status.active_operation_id, operation);
            assert_eq!(
                status.capture_blocked_reason,
                Some(VfsCaptureBlockedReason::UnprovenActivation)
            );
            assert_eq!(
                inspect_vfs_checkout(&root)
                    .unwrap()
                    .unwrap()
                    .capture_blocked_reason,
                Some(VfsCaptureBlockedReason::UnprovenActivation)
            );
            assert!(
                matches!(reopened.capture_quiesced(), Err(NativeClientError::InvalidState(cause))
                if cause.contains("NeedsRepair"))
            );
            assert!(reopened.journal.state().pending_capture.is_none());
            assert!(reopened.journal.state().last_accepted.is_none());
            let exported = reopened.export_quiesced(&mount.parent).unwrap();
            assert_eq!(
                std::fs::read(exported.tree_root.join("file")).unwrap(),
                b"legacy unaccepted user bytes"
            );
        }
        assert_eq!(
            std::fs::read(root.join("file")).unwrap(),
            b"legacy unaccepted user bytes"
        );
    }

    #[test]
    #[ignore = "real CFAPI historical unsupported source; explicit disposable parent required"]
    fn unsupported_legacy_source_keeps_identity_overlap_checks_without_poisoning_creation() {
        let mount = legacy_mount(true);
        let root = mount.locator.checkout_root().to_path_buf();
        assert!(
            matches!(VerifiedVfsSnapshot::reopen(&mount.locator.source, &Cancellation::default()),
            Err(NativeClientError::InvalidState(cause)) if cause.contains("named empty directories"))
        );
        let store = mount.workspace.open_store().unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let worktree = reader
            .get_worktree(mount.locator.worktree_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            verify_registered_vfs_root(&store, &worktree).unwrap(),
            worktree.filesystem_identity()
        );
        let probe = mount.parent.join("unregistered-valid-probe");
        std::fs::create_dir(&probe).unwrap();
        let mut verified = 0;
        scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &probe, |registered| {
            if registered.id() == worktree.id() {
                verified += 1;
            }
            verify_registered_vfs_root(&store, registered).map_err(io::Error::other)
        })
        .unwrap();
        assert_eq!(
            verified, 1,
            "unrelated root must reach the VFS identity verifier"
        );
        assert!(
            scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &root, |worktree| {
                verify_registered_vfs_root(&store, worktree).map_err(io::Error::other)
            })
            .is_err()
        );
        drop(reader);
        drop(store);
        assert!(matches!(inspect_vfs_checkout(&root),
            Err(NativeClientError::InvalidState(cause)) if cause.contains("named empty directories")));
        let ordinary = mount.parent.join("unrelated-ordinary");
        std::fs::create_dir(&ordinary).unwrap();
        mount
            .workspace
            .create_worktree(
                &ordinary,
                &mount.locator.creation.layer_id,
                &mount.supported_step,
            )
            .unwrap();
        let virtual_root = mount.parent.join("unrelated-virtual");
        std::fs::create_dir(&virtual_root).unwrap();
        let other = VfsWorktree::create(
            &mount.workspace,
            &virtual_root,
            &mount.locator.creation.layer_id,
            &mount.supported_step,
        )
        .unwrap();
        assert_eq!(other.source_read_counts(), (0, 0));
        drop(other);
        drop(mount.session);
        drop(mount.journal);
        assert!(VfsWorktree::open(&root).is_err());
        // Matching bytes in a new native file are not the recorded capability.
        std::fs::rename(root.join(".layrs"), mount.parent.join("retained-control")).unwrap();
        std::fs::write(root.join(".layrs"), mount.locator.encode().unwrap()).unwrap();
        let store = mount.workspace.open_store().unwrap();
        assert!(matches!(
            verify_registered_vfs_root(&store, &worktree),
            Err(NativeClientError::InvalidState(
                "VFS registered control differs from private journal"
            ))
        ));
        let mut verified = 0;
        let error =
            scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &probe, |registered| {
                if registered.id() == worktree.id() {
                    verified += 1;
                }
                verify_registered_vfs_root(&store, registered).map_err(io::Error::other)
            })
            .unwrap_err();
        assert_eq!(verified, 1);
        assert!(
            error
                .to_string()
                .contains("VFS registered control differs from private journal"),
            "{error}"
        );
        drop(store);
        // Nor may a new directory at the recorded pathname inherit registration.
        std::fs::rename(&root, mount.parent.join("retained-provider-root")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join(".layrs"), mount.locator.encode().unwrap()).unwrap();
        let store = mount.workspace.open_store().unwrap();
        let mut verified = 0;
        let error =
            scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &probe, |registered| {
                if registered.id() == worktree.id() {
                    verified += 1;
                }
                verify_registered_vfs_root(&store, registered).map_err(io::Error::other)
            })
            .unwrap_err();
        assert_eq!(verified, 1);
        assert!(
            error
                .to_string()
                .contains("verify registered provider identity")
                && error.to_string().contains("ForeignRoot"),
            "{error}"
        );
    }

    #[test]
    fn metadata_catalog_boundary_counts_user_files_and_directories_only() {
        let root = std::env::temp_dir().join(format!("layrs-vfs-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let workspace = NativeWorkspace::initialize(&root, "Metadata catalog boundary").unwrap();
        // One shared empty FileManifest and two Trees describe the whole image;
        // no checkout leaf or provider placeholder is created by this test.
        for files in [99_999usize, 100_000] {
            let mut snapshot = SnapshotBuilder::new(SnapshotLimits::default());
            for index in 0..files {
                snapshot
                    .add_file(
                        RepoPath::parse(format!("dir/f{index:06}")).unwrap(),
                        &[],
                        false,
                    )
                    .unwrap();
            }
            let client = workspace.client();
            let base = client.capture_base(workspace.worktree_id()).unwrap();
            let CapturePlan::Change(change) = client
                .prepare_capture(
                    &base,
                    snapshot.finish().unwrap(),
                    MutationStamp::fresh().unwrap(),
                )
                .unwrap()
            else {
                panic!("new metadata image must change the Step");
            };
            client.accept(&change).unwrap();
            let state = workspace.state().unwrap();
            let binding = VerifiedVfsSnapshotBinding {
                store_root: workspace.store_root().to_path_buf(),
                store_id: state.worktree.store_id().clone(),
                store_identity: workspace.store_identity().into(),
                store_metadata_identity: workspace.metadata_identity().into(),
                space_id: state.space.id().clone(),
                operation_id: state.operation.id().clone(),
                step_id: change.step_id().clone(),
            };
            let store = workspace.open_store().unwrap();
            let catalog = metadata_catalog(&store, &binding);
            if files == 99_999 {
                assert_eq!(catalog.unwrap().entries().len(), files);
            } else {
                assert!(
                    catalog.is_err(),
                    "100,000 files plus their directory exceed the budget"
                );
            }
        }
        assert!(!root.join("dir").exists());
        drop(workspace);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn validate_ticket(
    locator: &VfsWorktreeLocator,
    source: &VerifiedVfsSnapshot,
    ticket: &Registration,
) -> Result<()> {
    if ticket.root() != locator.checkout_root()
        || ticket.root_id() != locator.root_id
        || ticket.catalog_id() != source.catalog().identity()
        || ticket.filesystem_identity() != locator.creation.worktree.filesystem_identity()
    {
        return Err(NativeClientError::InvalidState(
            "VFS provider ticket binding mismatch",
        ));
    }
    Ok(())
}
