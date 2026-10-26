//! Explicit, source-preserving provider recovery export. This is portable byte
//! evidence, never a capture grant, managed conversion or invented SCM tree.
use super::*;
use crate::projection::{
    provider::{
        capture_lock::{CaptureAttemptLock, CaptureAttemptLockError},
        records::{capture::ProviderCaptureState, Receipt},
    },
    shared::provider_access::FamilyAccess,
};
use crate::scm_v1::vfs_recovery::VfsExportWriter;
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{canonical_json, FileId, RepoPath, TreeEntryTarget},
};
use layrs_vfs::{Cancellation, CaptureEvidence, SnapshotSource};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    fs::OpenOptions,
    io::{self, Read, Write},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
};

const FILE_LIMIT: u64 = 1024 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 4 * FILE_LIMIT;
pub(super) const PATH_LIMIT: usize = 100_000;
pub(super) const PATH_BYTES_LIMIT: usize = 8 * 1024 * 1024;
const MANIFEST_LIMIT: usize = 32 * 1024 * 1024;
const READ_BYTES: usize = 64 * 1024;
const IMAGE_DOMAIN: &str = "layrs-provider-recovery-image-v1";
const MANIFEST_SCHEMA: &str = "layrs.provider-recovery-export.v1";

#[derive(Debug)]
pub enum ProviderRecoveryExportCause {
    Store(Cause),
    ActiveValidation(ProviderActiveOpenFailure),
    Native(layrs_vfs::Error),
    Io(io::Error),
}
impl fmt::Display for ProviderRecoveryExportCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => e.fmt(f),
            Self::ActiveValidation(e) => e.fmt(f),
            Self::Native(e) => e.fmt(f),
            Self::Io(e) => e.fmt(f),
        }
    }
}
impl Error for ProviderRecoveryExportCause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            Self::ActiveValidation(e) => Some(e),
            Self::Native(e) => Some(e),
            Self::Io(e) => Some(e),
        }
    }
}
impl From<Cause> for ProviderRecoveryExportCause {
    fn from(value: Cause) -> Self {
        Self::Store(value)
    }
}
impl From<crate::projection::ProjectionStoreError> for ProviderRecoveryExportCause {
    fn from(value: crate::projection::ProjectionStoreError) -> Self {
        Self::Store(value.into())
    }
}
impl From<layrs_vfs::Error> for ProviderRecoveryExportCause {
    fn from(value: layrs_vfs::Error) -> Self {
        Self::Native(value)
    }
}
impl From<io::Error> for ProviderRecoveryExportCause {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRecoveryExportStep {
    Admission,
    Destination,
    Checkpoint,
    Preflight,
    Hash,
    Copy,
    Publish,
}
#[derive(Debug)]
pub struct ProviderRecoveryExportFailure {
    pub step: ProviderRecoveryExportStep,
    pub cause: ProviderRecoveryExportCause,
    /// Present once a create-new container exists. It is retained on failure.
    pub container_root: Option<PathBuf>,
    /// Planned create-new container path. Presence does not prove creation or
    /// ownership and grants no cleanup authority.
    pub attempted_container_root: Option<PathBuf>,
}
impl fmt::Display for ProviderRecoveryExportFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider recovery export at {:?}: {}",
            self.step, self.cause
        )
    }
}
impl Error for ProviderRecoveryExportFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderRecoveryExportClass {
    ManagedCandidate,
    RecoveryOnlyUnknown,
    RecoveryOnlyExcluded,
    RecoveryOnlyProtected,
    RecoveryOnlyRelocated,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRecoveryExportFile {
    pub path: RepoPath,
    pub file_id: FileId,
    pub length: u64,
    /// None when no fixed provider entry supplies a trustworthy mode.
    pub executable: Option<bool>,
    pub class: ProviderRecoveryExportClass,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRecoveryExportDeletion {
    pub path: RepoPath,
    pub base: FileId,
    pub expected_executable: Option<bool>,
    pub class: ProviderRecoveryExportClass,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderRecoveryExportGapKind {
    MissingSourceBytes,
}
#[derive(Debug)]
pub struct ProviderRecoveryExportGap {
    pub path: RepoPath,
    pub backing: Option<FileId>,
    pub kind: ProviderRecoveryExportGapKind,
    pub cause: layrs_vfs::Error,
}
#[derive(Debug)]
pub struct ProviderRecoveryExportReport {
    pub container_root: PathBuf,
    pub tree_root: PathBuf,
    pub manifest_path: PathBuf,
    pub files: Vec<ProviderRecoveryExportFile>,
    pub deletions: Vec<ProviderRecoveryExportDeletion>,
    pub gaps: Vec<ProviderRecoveryExportGap>,
    pub verified_bytes: u64,
    pub observed_image_digest: String,
    pub complete_selected: bool,
}
#[derive(Debug)]
pub enum ProviderRecoveryExportOutcome {
    Complete(ProviderRecoveryExportReport),
    Incomplete(ProviderRecoveryExportReport),
}

#[derive(Clone)]
pub(super) struct Observed {
    pub(super) path: RepoPath,
    pub(super) evidence: CaptureEvidence,
    pub(super) original_path: Option<RepoPath>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReaderKind {
    ImmutableSource,
    Native,
}
pub(super) struct PlannedFile {
    pub(super) path: RepoPath,
    pub(super) backing: Option<FileId>,
    pub(super) size: u64,
    pub(super) executable: Option<bool>,
    pub(super) class: ProviderRecoveryExportClass,
    pub(super) expected: Option<(FileId, bool)>,
    pub(super) writable: bool,
    pub(super) reader: ReaderKind,
}
pub(super) struct Plan {
    pub(super) files: Vec<PlannedFile>,
    pub(super) deletions: Vec<ProviderRecoveryExportDeletion>,
}

fn fail(
    step: ProviderRecoveryExportStep,
    cause: impl Into<ProviderRecoveryExportCause>,
    container_root: Option<&Path>,
) -> ProviderRecoveryExportFailure {
    ProviderRecoveryExportFailure {
        step,
        cause: cause.into(),
        container_root: container_root.map(Path::to_owned),
        attempted_container_root: container_root.map(Path::to_owned),
    }
}

fn checked_path(
    path: &RepoPath,
    aliases: &mut BTreeSet<String>,
    bytes: &mut usize,
) -> CauseResult<()> {
    let raw = path.as_str();
    *bytes = bytes
        .checked_add(raw.len())
        .filter(|n| *n <= PATH_BYTES_LIMIT)
        .ok_or_else(|| invalid("recovery export path metadata exceeds bound"))?;
    if raw.is_empty() || raw.len() > 4096 || !aliases.insert(raw.to_ascii_lowercase()) {
        return Err(invalid("recovery export duplicate/aliased/oversize path"));
    }
    Ok(())
}

/// Classifies the entire observed namespace before any source/native content
/// read or output creation. Unknown, excluded and protected edits remain
/// recovery-only evidence. A managed candidate is still never admitted here.
pub(super) fn plan(
    observations: &[Observed],
    source: &ProviderBootstrapSource,
    activated: &ProjectionSnapshot,
) -> CauseResult<Plan> {
    if observations.len() > PATH_LIMIT {
        return Err(invalid("recovery export path count exceeds bound"));
    }
    let mut entries = BTreeMap::new();
    for entry in &activated.entries {
        entries.insert(&entry.path, entry);
    }
    let mut descriptors = BTreeMap::new();
    for descriptor in source.descriptors() {
        descriptors.insert(&descriptor.path, descriptor);
    }
    let mut files = Vec::new();
    let mut deletions = Vec::new();
    let mut aliases = BTreeSet::new();
    let mut path_bytes = 0;
    let mut total = 0;
    let mut previous: Option<&RepoPath> = None;
    for observation in observations {
        if previous.is_some_and(|last| last >= &observation.path) {
            return Err(invalid("recovery export observations not sorted/unique"));
        }
        previous = Some(&observation.path);
        checked_path(&observation.path, &mut aliases, &mut path_bytes)?;
        let entry = entries.get(&observation.path).copied();
        let selected = entry.is_some_and(|e| source.selection().includes(&e.path));
        let descriptor = descriptors.get(&observation.path).copied();
        let class = match entry {
            None => ProviderRecoveryExportClass::RecoveryOnlyUnknown,
            Some(_) if !selected => ProviderRecoveryExportClass::RecoveryOnlyExcluded,
            Some(_) => ProviderRecoveryExportClass::ManagedCandidate,
        };
        let expected = entry.and_then(|e| match &e.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id.clone(), e.executable)),
            _ => None,
        });
        let writable = entry.is_some_and(|e| e.writable);
        match &observation.evidence {
            CaptureEvidence::Deleted { base } => {
                if observation.original_path.is_some() {
                    return Err(invalid("recovery export deleted entry has native origin"));
                }
                if descriptor.is_some_and(|d| d.file_id != *base) {
                    return Err(invalid(
                        "recovery export deleted backing differs from catalog",
                    ));
                }
                deletions.push(ProviderRecoveryExportDeletion {
                    path: observation.path.clone(),
                    base: base.clone(),
                    expected_executable: entry.map(|e| e.executable),
                    class: if selected && !writable && expected.is_some() {
                        ProviderRecoveryExportClass::RecoveryOnlyProtected
                    } else {
                        class
                    },
                });
            }
            CaptureEvidence::Immutable {
                file_id,
                size,
                original_path,
            } => {
                let d = descriptor
                    .ok_or_else(|| invalid("recovery export immutable source has no descriptor"))?;
                if file_id != &d.file_id
                    || *size != d.bytes
                    || original_path != &d.path
                    || observation.original_path.as_ref() != Some(original_path)
                {
                    return Err(invalid("recovery export immutable source binding differs"));
                }
                total = add_size(total, *size)?;
                files.push(PlannedFile {
                    path: observation.path.clone(),
                    backing: Some(file_id.clone()),
                    size: *size,
                    executable: Some(entry.map_or(d.executable, |e| e.executable)),
                    class,
                    expected: expected.clone(),
                    writable,
                    reader: ReaderKind::ImmutableSource,
                });
            }
            CaptureEvidence::NeedsHash { base, size } => {
                total = add_size(total, *size)?;
                let class = match (base, &observation.original_path) {
                    (Some(id), Some(origin)) => {
                        let original = descriptors.get(origin).ok_or_else(|| {
                            invalid("recovery export native origin is outside fixed catalog")
                        })?;
                        if &original.file_id != id {
                            return Err(invalid(
                                "recovery export native backing differs from catalog",
                            ));
                        }
                        if origin != &observation.path {
                            ProviderRecoveryExportClass::RecoveryOnlyRelocated
                        } else if descriptor.is_some_and(|d| &d.file_id == id) {
                            class
                        } else {
                            return Err(invalid("recovery export native origin binding differs"));
                        }
                    }
                    (Some(id), None) if descriptor.is_some_and(|d| &d.file_id == id) => class,
                    (None, None) => class,
                    _ => return Err(invalid("recovery export native origin evidence differs")),
                };
                files.push(PlannedFile {
                    path: observation.path.clone(),
                    backing: base.clone(),
                    size: *size,
                    executable: entry.map(|e| e.executable),
                    class,
                    expected: expected.clone(),
                    writable,
                    reader: ReaderKind::Native,
                });
            }
        }
    }
    // The VFS includes all catalog paths, even when absent. A vanished fixed
    // descriptor is not silently inferred to be a deletion.
    if descriptors
        .keys()
        .any(|path| observations.binary_search_by(|o| o.path.cmp(path)).is_err())
    {
        return Err(invalid("recovery export omitted fixed catalog path"));
    }
    Ok(Plan { files, deletions })
}

fn add_size(total: u64, size: u64) -> CauseResult<u64> {
    if size > FILE_LIMIT {
        return Err(invalid("recovery export file exceeds 1 GiB"));
    }
    total
        .checked_add(size)
        .filter(|n| *n <= TOTAL_LIMIT)
        .ok_or_else(|| invalid("recovery export exceeds 4 GiB aggregate"))
}

pub(super) fn verified_class(
    planned: &PlannedFile,
    actual: &FileId,
) -> ProviderRecoveryExportClass {
    if planned.class == ProviderRecoveryExportClass::ManagedCandidate
        && !planned.writable
        && !planned
            .expected
            .as_ref()
            .is_some_and(|(id, mode)| id == actual && Some(*mode) == planned.executable)
    {
        ProviderRecoveryExportClass::RecoveryOnlyProtected
    } else {
        planned.class
    }
}

fn source_missing(error: &layrs_vfs::Error) -> bool {
    match error {
        layrs_vfs::Error::MissingBytes { .. } => true,
        layrs_vfs::Error::ExportRead { cause, .. } => source_missing(cause),
        _ => false,
    }
}

fn hash_reader(
    reader: &mut dyn Read,
    size: u64,
    cancel: &Cancellation,
) -> std::result::Result<FileId, layrs_vfs::Error> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut remaining = size;
    let mut buffer = [0; READ_BYTES];
    while remaining > 0 {
        cancel.check()?;
        let n = (remaining as usize).min(buffer.len());
        reader.read_exact(&mut buffer[..n])?;
        hash.update(&buffer[..n]);
        remaining -= n as u64;
    }
    cancel.check()?;
    FileId::parse(format!("blake3:{}", hash.finalize()))
        .map_err(|e| layrs_vfs::Error::InvalidCatalog(e.to_string()))
}

fn hash_source(
    source: &dyn SnapshotSource,
    id: &FileId,
    size: u64,
    cancel: &Cancellation,
) -> layrs_vfs::Result<FileId> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut buffer = [0; READ_BYTES];
    let mut offset = 0;
    while offset < size {
        cancel.check()?;
        let count = ((size - offset) as usize).min(buffer.len());
        source.read_exact_at(id, offset, &mut buffer[..count], cancel)?;
        hash.update(&buffer[..count]);
        offset += count as u64;
    }
    cancel.check()?;
    let actual = FileId::parse(format!("blake3:{}", hash.finalize()))
        .map_err(|e| layrs_vfs::Error::InvalidCatalog(e.to_string()))?;
    if &actual != id {
        return Err(layrs_vfs::Error::IdentityMismatch);
    }
    Ok(actual)
}

fn same_capture_state(
    a: &validation::PrivateActiveCaptureValidation,
    b: &validation::PrivateActiveCaptureValidation,
) -> bool {
    a.active.mount == b.active.mount
        && a.active.activation == b.active.activation
        && a.active.source.commitment() == b.active.source.commitment()
        && a.active.activated.snapshot == b.active.activated.snapshot
        && a.capture.transactions == b.capture.transactions
        && a.capture.receipts == b.capture.receipts
        && a.capture.patches == b.capture.patches
        && a.capture
            .content
            .iter()
            .map(|c| (&c.file_id, &c.manifest_id, c.bytes))
            .eq(b
                .capture
                .content
                .iter()
                .map(|c| (&c.file_id, &c.manifest_id, c.bytes)))
        && a.capture
            .claims
            .iter()
            .map(|c| (&c.root_id, &c.digest, &c.kind, &c.owner))
            .eq(b
                .capture
                .claims
                .iter()
                .map(|c| (&c.root_id, &c.digest, &c.kind, &c.owner)))
}

fn admitted(
    held: &outcome::PublicationPins,
    native: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    cancel: &Cancellation,
) -> std::result::Result<validation::PrivateActiveCaptureValidation, ProviderRecoveryExportCause> {
    cancel.check().map_err(Cause::Vfs)?;
    let _locks = held.family.locks(
        &held.artifacts.allocation.format.identity.worktree_id,
        false,
    )?;
    attempt.verify(&held.artifacts, &held.family)?;
    let state = capture_state::read(held, Some(native), Some(cancel)).map_err(|e| {
        ProviderRecoveryExportCause::ActiveValidation(ProviderActiveOpenFailure::from_failure(
            "validate active capture for recovery export",
            e,
        ))
    })?;
    if state.active.mount != *mount || state.active.activation != *activation {
        return Err(invalid("recovery export active map changed").into());
    }
    if state.capture.transactions.iter().any(|record| {
        matches!(
            record.state,
            ProviderCaptureState::Hashing | ProviderCaptureState::Staging { .. }
        )
    }) {
        return Err(invalid("recovery export requires no pending capture hashing/staging").into());
    }
    attempt.verify(&held.artifacts, &held.family)?;
    Ok(state)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestGap<'a> {
    path: &'a RepoPath,
    backing: &'a Option<FileId>,
    kind: ProviderRecoveryExportGapKind,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    schema: &'static str,
    allocation: &'a crate::projection::provider::records::ReceiptRef,
    prepared: &'a crate::projection::provider::records::ReceiptRef,
    mount_digest: &'a str,
    activation_digest: &'a str,
    source: &'a crate::projection::provider::records::SourceCommitment,
    capture_sequence: u64,
    capture_digest: Option<&'a str>,
    observed_image_digest: &'a str,
    complete_selected: bool,
    files: &'a [ProviderRecoveryExportFile],
    deletions: &'a [ProviderRecoveryExportDeletion],
    gaps: Vec<ManifestGap<'a>>,
}

pub(super) fn image_digest(
    files: &[ProviderRecoveryExportFile],
    deletions: &[ProviderRecoveryExportDeletion],
    gaps: &[ProviderRecoveryExportGap],
) -> CauseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    enum Item<'a> {
        File {
            file_id: &'a FileId,
            length: u64,
            executable: Option<bool>,
        },
        Deleted {
            base: &'a FileId,
        },
        Gap {
            backing: &'a Option<FileId>,
        },
    }
    let mut map = BTreeMap::new();
    for file in files {
        map.insert(
            &file.path,
            Item::File {
                file_id: &file.file_id,
                length: file.length,
                executable: file.executable,
            },
        );
    }
    for deletion in deletions {
        map.insert(
            &deletion.path,
            Item::Deleted {
                base: &deletion.base,
            },
        );
    }
    for gap in gaps {
        map.insert(
            &gap.path,
            Item::Gap {
                backing: &gap.backing,
            },
        );
    }
    let bytes = canonical_json(&map).map_err(|e| invalid(e.to_string()))?;
    if bytes.len() > MANIFEST_LIMIT {
        return Err(invalid("recovery image document exceeds bound"));
    }
    Ok(crate::projection::provider::records::codec::hash(
        IMAGE_DOMAIN,
        &bytes,
    ))
}

/// Existing ordinary export parent plus native identity exclusions for family,
/// private metadata and control. The VFS guard retains every ancestor without
/// DELETE and revalidates the exact path after our identity-only probes.
struct Destination<'a> {
    guard: layrs_vfs::ExportParentGuard<'a>,
}
impl Destination<'_> {
    fn open<'a>(
        session: &'a layrs_vfs::Session,
        parent: &Path,
        family: &FamilyAccess,
        held: &outcome::PublicationPins,
    ) -> std::result::Result<Destination<'a>, ProviderRecoveryExportCause> {
        let absolute = std::path::absolute(parent)?;
        let guard = session.guard_export_parent(&absolute)?;
        let excluded = [
            family.root().to_owned(),
            PathBuf::from(&held.artifacts.allocation.private.metadata_path),
            PathBuf::from(&held.artifacts.allocation.private.metadata_path)
                .join("provider-control"),
        ];
        let excluded_ids = [
            &held.artifacts.allocation.family.root_identity,
            &held.artifacts.allocation.private.metadata_identity,
            &held.artifacts.allocation.private.provider_control_identity,
        ];
        for ancestor in absolute.ancestors() {
            if session.is_owned_root_directory(ancestor)? {
                return Err(invalid("recovery export parent overlaps provider checkout").into());
            }
            let id = directory_identity(ancestor)?;
            if excluded_ids.contains(&&id) || excluded.iter().any(|path| path == ancestor) {
                return Err(
                    invalid("recovery export parent overlaps family/control metadata").into(),
                );
            }
        }
        guard.revalidate()?;
        Ok(Destination { guard })
    }
    fn verify(&self) -> layrs_vfs::Result<()> {
        self.guard.revalidate()
    }
    fn root(&self) -> &Path {
        self.guard.root()
    }
}

fn export_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-recovery-export-id-v1\0");
    hash.update(&std::process::id().to_le_bytes());
    hash.update(&NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hash.update(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    let hex = hash.finalize().to_hex().to_string();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn directory_identity(path: &Path) -> io::Result<String> {
    let file = OpenOptions::new()
        .access_mode(0x0000_0080 | 0x0000_0001)
        .share_mode(0x0000_0003)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: file remains open and info writable for the duration of call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(format!(
        "fs:v1:{:016x}:{:016x}",
        info.dwVolumeSerialNumber,
        ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64
    ))
}

pub(super) fn run(
    session: &layrs_vfs::Session,
    source: &dyn SnapshotSource,
    held: &outcome::PublicationPins,
    native: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    parent: &Path,
    cancel: &Cancellation,
) -> std::result::Result<ProviderRecoveryExportOutcome, ProviderRecoveryExportFailure> {
    if !session.is_connected() {
        return Err(fail(
            ProviderRecoveryExportStep::Admission,
            invalid("recovery export requires connected session"),
            None,
        ));
    }
    let attempt = CaptureAttemptLock::acquire(&held.artifacts, &held.family).map_err(|e| {
        let cause = match e {
            CaptureAttemptLockError::Contended => ProviderRecoveryExportCause::Io(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another provider attempt owns allocation",
            )),
            CaptureAttemptLockError::Cause(e) => e.into(),
        };
        fail(ProviderRecoveryExportStep::Admission, cause, None)
    })?;
    let initial = admitted(held, native, mount, activation, &attempt, cancel)
        .map_err(|e| fail(ProviderRecoveryExportStep::Admission, e, None))?;
    let destination = Destination::open(session, parent, &held.family, held)
        .map_err(|e| fail(ProviderRecoveryExportStep::Destination, e, None))?;
    let mut checkpoint = session
        .export_checkpoint()
        .map_err(|e| fail(ProviderRecoveryExportStep::Checkpoint, e, None))?;
    let mut observations = Vec::with_capacity(checkpoint.entries().len());
    for entry in checkpoint.entries() {
        cancel
            .check()
            .map_err(|e| fail(ProviderRecoveryExportStep::Preflight, e, None))?;
        observations.push(Observed {
            path: entry.path.clone(),
            evidence: entry.evidence.clone(),
            original_path: checkpoint
                .original_path(&entry.path)
                .map_err(|e| fail(ProviderRecoveryExportStep::Preflight, e, None))?,
        });
    }
    let preflight = plan(
        &observations,
        &initial.active.source,
        &initial.active.activated.snapshot,
    )
    .map_err(|e| fail(ProviderRecoveryExportStep::Preflight, e, None))?;
    checkpoint
        .revalidate_namespace()
        .map_err(|e| fail(ProviderRecoveryExportStep::Preflight, e, None))?;
    destination
        .verify()
        .map_err(|e| fail(ProviderRecoveryExportStep::Preflight, e, None))?;
    let mut files = Vec::new();
    let mut gaps = Vec::new();
    let mut verified_bytes = 0;
    let mut available = Vec::new();
    for planned in &preflight.files {
        cancel
            .check()
            .map_err(|e| fail(ProviderRecoveryExportStep::Hash, e, None))?;
        let result = match planned.reader {
            ReaderKind::ImmutableSource => hash_source(
                source,
                planned
                    .backing
                    .as_ref()
                    .expect("preflight immutable backing"),
                planned.size,
                cancel,
            ),
            ReaderKind::Native => checkpoint.with_reader(&planned.path, |reader| {
                hash_reader(reader, planned.size, cancel)
            }),
        };
        match result {
            Ok(file_id) => {
                verified_bytes = add_size(verified_bytes, planned.size)
                    .map_err(|e| fail(ProviderRecoveryExportStep::Hash, e, None))?;
                let class = verified_class(planned, &file_id);
                files.push(ProviderRecoveryExportFile {
                    path: planned.path.clone(),
                    file_id,
                    length: planned.size,
                    executable: planned.executable,
                    class,
                });
                available.push(planned);
            }
            Err(cause) if source_missing(&cause) => gaps.push(ProviderRecoveryExportGap {
                path: planned.path.clone(),
                backing: planned.backing.clone(),
                kind: ProviderRecoveryExportGapKind::MissingSourceBytes,
                cause,
            }),
            Err(cause) => return Err(fail(ProviderRecoveryExportStep::Hash, cause, None)),
        }
    }
    checkpoint
        .revalidate_namespace()
        .map_err(|e| fail(ProviderRecoveryExportStep::Hash, e, None))?;
    let current = admitted(held, native, mount, activation, &attempt, cancel)
        .map_err(|e| fail(ProviderRecoveryExportStep::Hash, e, None))?;
    if !same_capture_state(&initial, &current) {
        return Err(fail(
            ProviderRecoveryExportStep::Hash,
            invalid("recovery export capture state changed"),
            None,
        ));
    }
    let observed_image_digest = image_digest(&files, &preflight.deletions, &gaps)
        .map_err(|e| fail(ProviderRecoveryExportStep::Hash, e, None))?;
    destination
        .verify()
        .map_err(|e| fail(ProviderRecoveryExportStep::Destination, e, None))?;
    let id = export_id();
    let attempted = destination.root().join(format!("layrs-vfs-export-{id}"));
    let mut writer = VfsExportWriter::create(destination.root(), &id).map_err(|e| {
        ProviderRecoveryExportFailure {
            step: ProviderRecoveryExportStep::Destination,
            cause: e.into(),
            container_root: None,
            attempted_container_root: Some(attempted),
        }
    })?;
    let container_root = writer.container_root().to_owned();
    for (planned, record) in available.into_iter().zip(&files) {
        checkpoint
            .revalidate_namespace()
            .map_err(|e| fail(ProviderRecoveryExportStep::Copy, e, Some(&container_root)))?;
        destination
            .verify()
            .map_err(|e| fail(ProviderRecoveryExportStep::Copy, e, Some(&container_root)))?;
        let mut copy_cause = None;
        let mut cancelled = false;
        let mut check = || {
            if cancel.is_cancelled() {
                cancelled = true;
                Err(io::Error::other(layrs_vfs::Error::Cancelled))
            } else {
                Ok(())
            }
        };
        let written = writer.write_file_with_check(
            &record.path,
            &record.file_id,
            record.length,
            &mut |output| {
                match planned.reader {
                    ReaderKind::ImmutableSource => copy_source(
                        source,
                        planned
                            .backing
                            .as_ref()
                            .expect("preflight immutable backing"),
                        planned.size,
                        cancel,
                        output,
                    ),
                    ReaderKind::Native => checkpoint
                        .with_reader(&planned.path, |reader| {
                            let mut buffer = [0; READ_BYTES];
                            loop {
                                cancel.check()?;
                                let count = reader.read(&mut buffer)?;
                                if count == 0 {
                                    break;
                                }
                                output.write_all(&buffer[..count])?;
                            }
                            Ok(())
                        })
                        .map_err(Into::into),
                }
                .map_err(|e: ProviderRecoveryExportCause| {
                    copy_cause = Some(e);
                    io::Error::other("provider recovery export copy failed; typed cause retained")
                })
            },
            &mut check,
        );
        if let Err(error) = written {
            let cause = copy_cause.unwrap_or_else(|| {
                if cancelled {
                    ProviderRecoveryExportCause::Native(layrs_vfs::Error::Cancelled)
                } else {
                    error.into()
                }
            });
            return Err(fail(
                ProviderRecoveryExportStep::Copy,
                cause,
                Some(&container_root),
            ));
        }
    }
    checkpoint.revalidate_namespace().map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            e,
            Some(&container_root),
        )
    })?;
    destination.verify().map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            e,
            Some(&container_root),
        )
    })?;
    let final_state = admitted(held, native, mount, activation, &attempt, cancel).map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            e,
            Some(&container_root),
        )
    })?;
    if !same_capture_state(&initial, &final_state) {
        return Err(fail(
            ProviderRecoveryExportStep::Publish,
            invalid("recovery export capture state changed"),
            Some(&container_root),
        ));
    }
    let head = final_state.capture.patches.last();
    let allocation = held.artifacts.allocation.data_reference().map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            e,
            Some(&container_root),
        )
    })?;
    let prepared = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| {
            fail(
                ProviderRecoveryExportStep::Publish,
                invalid("recovery export P absent"),
                Some(&container_root),
            )
        })?
        .data_reference()
        .map_err(|e| {
            fail(
                ProviderRecoveryExportStep::Publish,
                e,
                Some(&container_root),
            )
        })?;
    let mount_digest = mount.digest().map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            e,
            Some(&container_root),
        )
    })?;
    let activation_digest = activation.digest().map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            e,
            Some(&container_root),
        )
    })?;
    let complete_selected = gaps.is_empty();
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        allocation: &allocation,
        prepared: &prepared,
        mount_digest: &mount_digest,
        activation_digest: &activation_digest,
        source: initial.active.source.commitment(),
        capture_sequence: head.map_or(0, |p| p.sequence),
        capture_digest: head.map(|p| p.digest.as_str()),
        observed_image_digest: &observed_image_digest,
        complete_selected,
        files: &files,
        deletions: &preflight.deletions,
        gaps: gaps
            .iter()
            .map(|g| ManifestGap {
                path: &g.path,
                backing: &g.backing,
                kind: g.kind,
            })
            .collect(),
    };
    let bytes = canonical_json(&manifest).map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            invalid(e.to_string()),
            Some(&container_root),
        )
    })?;
    if bytes.len() > MANIFEST_LIMIT {
        return Err(fail(
            ProviderRecoveryExportStep::Publish,
            invalid("recovery export manifest exceeds bound"),
            Some(&container_root),
        ));
    }
    destination.verify().map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            e,
            Some(&container_root),
        )
    })?;
    let mut cancelled = false;
    let mut check = || {
        if cancel.is_cancelled() {
            cancelled = true;
            Err(io::Error::other(layrs_vfs::Error::Cancelled))
        } else {
            Ok(())
        }
    };
    let tree_root = writer.finish_with_check(&bytes, &mut check).map_err(|e| {
        fail(
            ProviderRecoveryExportStep::Publish,
            if cancelled {
                ProviderRecoveryExportCause::Native(layrs_vfs::Error::Cancelled)
            } else {
                e.into()
            },
            Some(&container_root),
        )
    })?;
    let report = ProviderRecoveryExportReport {
        container_root: container_root.clone(),
        tree_root,
        manifest_path: container_root.join("manifest.json"),
        files,
        deletions: preflight.deletions,
        gaps,
        verified_bytes,
        observed_image_digest,
        complete_selected,
    };
    Ok(if complete_selected {
        ProviderRecoveryExportOutcome::Complete(report)
    } else {
        ProviderRecoveryExportOutcome::Incomplete(report)
    })
}

fn copy_source(
    source: &dyn SnapshotSource,
    id: &FileId,
    size: u64,
    cancel: &Cancellation,
    output: &mut dyn Write,
) -> std::result::Result<(), ProviderRecoveryExportCause> {
    let mut buffer = [0; READ_BYTES];
    let mut offset = 0;
    while offset < size {
        cancel.check()?;
        let count = ((size - offset) as usize).min(buffer.len());
        source.read_exact_at(id, offset, &mut buffer[..count], cancel)?;
        output.write_all(&buffer[..count])?;
        offset += count as u64;
    }
    Ok(())
}

#[cfg(test)]
#[path = "recovery_export/tests.rs"]
mod tests;
