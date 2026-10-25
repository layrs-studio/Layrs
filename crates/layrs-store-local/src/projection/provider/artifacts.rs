//! Authenticate the three fixed receipts without SQLite or checkout readers.
use super::{
    database_io::DatabaseFile,
    records::{self, *},
    recovery::error::{Result, invalid},
};
use crate::projection::shared::provider_access::{FamilyAccess, canonical_path, exact_path};
use crate::{
    projection::{RecoveryBinding, file},
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::WorktreeId;
use std::{
    error::Error,
    ffi::OsStr,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) struct Artifacts {
    pub(super) allocation: AllocationRecord,
    pub(super) prepared: Option<PreparedRecord>,
    pub(super) abandoned: Option<AbandonedRecord>,
    pub(super) database: DatabaseFile,
    metadata: PinnedDirectory,
    control: PinnedDirectory,
    receipts: Vec<(PathBuf, File, Vec<u8>)>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderControlLocation {
    AllocatedStage,
    FinalDirectory,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFormatState {
    Complete,
    Empty,
    IncompletePrefix,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ControlObservation {
    pub(super) location: ProviderControlLocation,
    pub(super) format: ProviderFormatState,
    pub(super) root_nonempty: bool,
    pub(super) unregistered_verified: bool,
}

/// No StoreLock, lease, connection or source capability is retained here.
pub(super) enum RetainedControl {
    Final(layrs_vfs::ControlDirectoryGuard),
    Stage(layrs_vfs::ObservedAllocatedControlStage),
}

impl Artifacts {
    pub(super) fn open(
        family: &FamilyAccess,
        worktree: &WorktreeId,
        transaction: &str,
    ) -> Result<Self> {
        records::codec::transaction(transaction)?;
        let member = family.member(worktree);
        let parent = PinnedDirectory::open(
            member
                .parent()
                .ok_or_else(|| invalid("private member parent absent"))?,
        )?;
        let metadata = parent.open_private_child(OsStr::new("metadata"))?;
        let control = metadata.open_private_child(OsStr::new("provider-control"))?;
        let directory = member.join("provider-control");
        let mut receipts = Vec::new();
        let allocation = read_receipt::<AllocationRecord>(
            &directory,
            "allocation.json",
            &control,
            &mut receipts,
        )?
        .ok_or_else(|| invalid("provider allocation receipt absent; no restart adoption"))?;
        if allocation.transaction_id != transaction
            || &allocation.format.identity.worktree_id != worktree
            || allocation.private.metadata_path != exact_path(&member)?
            || allocation.private.canonical_metadata_path != canonical_path(&member)?
            || allocation.private.metadata_identity != metadata.identity_token()
            || allocation.private.provider_control_identity != control.identity_token()
        {
            return Err(invalid(
                "provider allocation private/transaction binding differs",
            ));
        }
        family.require_allocation(&allocation)?;
        require_recorded_separation(&allocation)?;
        let prepared =
            read_receipt::<PreparedRecord>(&directory, "prepared.json", &control, &mut receipts)?;
        if let Some(p) = &prepared {
            p.require_allocation(&allocation)?;
        }
        let abandoned =
            read_receipt::<AbandonedRecord>(&directory, "abandoned.json", &control, &mut receipts)?;
        if let Some(t) = &abandoned {
            t.require_receipt_references(&allocation, prepared.as_ref())?;
        }
        let database = DatabaseFile::open(
            member.join("projection.sqlite3"),
            &allocation.private.database_file_identity,
        )?;
        let bound = Self {
            allocation,
            prepared,
            abandoned,
            database,
            metadata,
            control,
            receipts,
        };
        bound.verify(family)?;
        Ok(bound)
    }
    pub(super) fn binding(&self) -> Result<ProviderBootstrapBinding> {
        let a = &self.allocation;
        let binding = ProviderBootstrapBinding {
            schema_version: "layrs.projection-provider-bootstrap-binding.v1".into(),
            recovery: RecoveryBinding {
                identity: a.format.identity.clone(),
                checkout_identity: a.format.checkout_identity().to_owned(),
                marker_digest: records::codec::hash(
                    "layrs-projection-recovery-marker-v1",
                    &a.format.bytes()?,
                ),
                database_identity: a.private.database_file_identity.clone(),
                family_marker_digest: Some(a.family.marker_digest.clone()),
            },
            transaction_id: a.transaction_id.clone(),
            allocation: a.data_reference()?,
            prepared: self
                .prepared
                .as_ref()
                .map(|p| -> crate::projection::Result<_> {
                    Ok(PreparedBinding {
                        receipt: p.data_reference()?,
                        control_identity_hex: p.control_identity_hex.clone(),
                    })
                })
                .transpose()?,
            abandoned: self
                .abandoned
                .as_ref()
                .map(Receipt::data_reference)
                .transpose()?,
        };
        match binding.require_references(a, self.prepared.as_ref(), self.abandoned.as_ref())? {
            ReceiptBindingCheck::NoTerminal | ReceiptBindingCheck::TerminalPendingValidation => {}
        }
        Ok(binding)
    }
    pub(super) fn verify(&self, family: &FamilyAccess) -> Result<()> {
        family.require_allocation(&self.allocation)?;
        self.metadata.verify_path_identity()?;
        self.control.verify_path_identity()?;
        self.database.verify()?;
        for (path, file, expected) in &self.receipts {
            let current = crate::layout::open_read_no_follow(path)?;
            if file::identity(&current)? != file::identity(file)?
                || read_bounded(current, expected.len())? != *expected
            {
                return Err(invalid("provider authoritative receipt changed"));
            }
        }
        let directory = Path::new(&self.allocation.private.metadata_path).join("provider-control");
        for (name, absent) in [
            ("prepared.json", self.prepared.is_none()),
            ("abandoned.json", self.abandoned.is_none()),
        ] {
            if absent {
                match crate::layout::open_read_no_follow(&directory.join(name)) {
                    Err(e) if e.raw_os_error() == Some(2) => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {
                        return Err(invalid("provider receipt appeared; fresh binding required"));
                    }
                }
            }
        }
        self.control.verify_path_identity()?;
        Ok(())
    }
    pub(super) fn observe(&self) -> Result<ControlObservation> {
        let intended = self.allocation.format.bytes()?;
        let stage = self.allocation.stage_identity()?;
        if let Some(p) = &self.prepared {
            let expected = p.control_identity()?;
            match layrs_vfs::discover_control_directory(stage.root()) {
                Ok(observed) => {
                    // VFS final Path comparison intentionally accepts drive case;
                    // Store requires the exact durable canonical string separately.
                    if observed.root().as_os_str() != stage.root().as_os_str()
                        || !observed.identity().matches_expected(&expected)
                        || observed.format_bytes() != intended
                    {
                        return Err(invalid(
                            "provider final observation differs from prepared binding",
                        ));
                    }
                    return Ok(ControlObservation {
                        location: ProviderControlLocation::FinalDirectory,
                        format: ProviderFormatState::Complete,
                        root_nonempty: false,
                        unregistered_verified: false,
                    });
                }
                Err(e) if exact_missing(&e) => {} // A below independently proves final absence.
                Err(e) => return Err(e.into()),
            }
        }
        let observed = layrs_vfs::observe_allocated_control_stage(&stage)?;
        check_stage(&observed, &stage, &intended)
    }
    pub(super) fn retain_control(&self) -> Result<RetainedControl> {
        let observation = self.observe()?;
        let stage = self.allocation.stage_identity()?;
        match observation.location {
            ProviderControlLocation::FinalDirectory => {
                let p = self
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("final provider control lacks prepared receipt"))?;
                let guard =
                    layrs_vfs::ControlDirectoryGuard::open(stage.root(), &p.control_identity()?)?;
                if guard.root().as_os_str() != stage.root().as_os_str()
                    || guard.format_bytes() != self.allocation.format.bytes()?
                {
                    return Err(invalid(
                        "retained provider control differs from exact receipt root/FORMAT",
                    ));
                }
                Ok(RetainedControl::Final(guard))
            }
            ProviderControlLocation::AllocatedStage => Ok(RetainedControl::Stage(
                layrs_vfs::observe_allocated_control_stage(&stage)?,
            )),
        }
    }
}

fn require_recorded_separation(allocation: &AllocationRecord) -> Result<()> {
    let stage = allocation.stage_identity()?;
    let family = Path::new(&allocation.family.canonical_root);
    let metadata = Path::new(&allocation.private.canonical_metadata_path);
    // Refusal against the exact canonical locators, in addition to the native
    // bindings verified by FamilyAccess, the private pins and A/final observation.
    // Do not reopen the historical stage/parent in a final-present branch: their
    // later absence is legitimate. This is not a substitute for the future
    // allocator's retained native ancestry proof before it writes allocation A.
    if family.starts_with(stage.root())
        || stage.root().starts_with(family)
        || stage.stage_parent().starts_with(stage.root())
        || stage.stage_parent().starts_with(family)
        || stage.stage_parent().starts_with(metadata)
        || stage.root_filesystem_identity() == allocation.family.root_identity
        || stage.stage_parent_filesystem_identity() == allocation.family.root_identity
        || stage.stage_parent_filesystem_identity() == allocation.private.metadata_identity
    {
        return Err(invalid(
            "provider allocation root/stage/family separation differs",
        ));
    }
    Ok(())
}

impl RetainedControl {
    pub(super) fn revalidate(&self, artifacts: &Artifacts) -> Result<ControlObservation> {
        let intended = artifacts.allocation.format.bytes()?;
        let stage = artifacts.allocation.stage_identity()?;
        match self {
            Self::Final(guard) => {
                guard.revalidate()?;
                let p = artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("retained final control lost prepared receipt"))?;
                if guard.root().as_os_str() != stage.root().as_os_str()
                    || guard.identity() != &p.control_identity()?
                    || guard.format_bytes() != intended
                {
                    return Err(invalid("retained provider final control binding changed"));
                }
                // Required only for terminal eligibility; no namespace traversal
                // and no claim about registrations in unrelated descendants.
                let unregistered = if artifacts.abandoned.is_some() {
                    guard.revalidate_unregistered_root()?;
                    true
                } else {
                    false
                };
                Ok(ControlObservation {
                    location: ProviderControlLocation::FinalDirectory,
                    format: ProviderFormatState::Complete,
                    root_nonempty: false,
                    unregistered_verified: unregistered,
                })
            }
            Self::Stage(held) => {
                check_stage(held, &stage, &intended)?;
                // The original strong reader remains retained. Fresh A checks
                // registration/path/bytes; neither object becomes a guard.
                let fresh = layrs_vfs::observe_allocated_control_stage(&stage)?;
                check_stage(&fresh, &stage, &intended)
            }
        }
    }
}

fn check_stage(
    observed: &layrs_vfs::ObservedAllocatedControlStage,
    expected: &layrs_vfs::ControlDirectoryStageIdentity,
    intended: &[u8],
) -> Result<ControlObservation> {
    if !observed.identity().matches_expected(expected) {
        return Err(invalid("allocated stage observation identity differs"));
    }
    let bytes = observed.format_bytes();
    let format = if bytes == intended {
        ProviderFormatState::Complete
    } else if bytes.is_empty() {
        ProviderFormatState::Empty
    } else if intended.starts_with(bytes) {
        ProviderFormatState::IncompletePrefix
    } else {
        return Err(invalid(
            "allocated FORMAT diverges from intended canonical bytes",
        ));
    };
    Ok(ControlObservation {
        location: ProviderControlLocation::AllocatedStage,
        format,
        root_nonempty: observed.root_namespace() == layrs_vfs::ObservedRootNamespace::Nonempty,
        unregistered_verified: true,
    })
}

fn read_receipt<R: Receipt>(
    directory: &Path,
    name: &str,
    pin: &PinnedDirectory,
    held: &mut Vec<(PathBuf, File, Vec<u8>)>,
) -> Result<Option<R>> {
    pin.verify_path_identity()?;
    let path = directory.join(name);
    let file = match crate::layout::open_read_no_follow(&path) {
        Ok(file) => file,
        Err(e) if e.raw_os_error() == Some(2) => {
            pin.verify_path_identity()?;
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    let identity = file::identity(&file)?;
    let bytes = read_bounded(file.try_clone()?, R::MAX_BYTES)?;
    let record = R::decode(&bytes)?;
    if record.file_identity() != identity {
        return Err(invalid(
            "provider receipt self-identity differs from native file",
        ));
    }
    let current = crate::layout::open_read_no_follow(&path)?;
    if file::identity(&current)? != identity || read_bounded(current, R::MAX_BYTES)? != bytes {
        return Err(invalid("provider receipt changed during read"));
    }
    pin.verify_path_identity()?;
    held.push((path, file, bytes));
    Ok(Some(record))
}
fn read_bounded(mut file: File, limit: usize) -> Result<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    if file.metadata()?.len() > limit as u64 {
        return Err(invalid("provider receipt exceeds bound"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("provider receipt grew beyond bound"));
    }
    Ok(bytes)
}
fn exact_missing(error: &layrs_vfs::Error) -> bool {
    let mut next: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(e) = next {
        if e.downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.raw_os_error() == Some(2))
        {
            return true;
        }
        if let Some(layrs_vfs::Error::Windows { hresult, .. }) =
            e.downcast_ref::<layrs_vfs::Error>()
        {
            if *hresult as u32 == 0x80070002 {
                return true;
            }
        }
        next = e.source();
    }
    false
}

impl Artifacts {
    pub(super) fn writer_control(&self) -> Result<PinnedDirectory> {
        self.metadata.verify_path_identity()?;
        let control = self
            .metadata
            .open_private_child_for_write(OsStr::new("provider-control"))?;
        if control.identity_token() != self.allocation.private.provider_control_identity {
            return Err(invalid("provider receipt directory identity changed"));
        }
        Ok(control)
    }
    pub(super) fn writer_refresh_prepared(
        &mut self,
        family: &FamilyAccess,
        prepared: &PreparedRecord,
    ) -> Result<()> {
        if self.prepared.is_some() || self.abandoned.is_some() {
            return Err(invalid(
                "prepared refresh requires known previous receipt absence",
            ));
        }
        let next = Self::open(
            family,
            &self.allocation.format.identity.worktree_id,
            &self.allocation.transaction_id,
        )?;
        if next.allocation != self.allocation
            || next.prepared.as_ref() != Some(prepared)
            || next.abandoned.is_some()
        {
            return Err(invalid(
                "published prepared receipt readback/binding differs",
            ));
        }
        *self = next;
        Ok(())
    }
    pub(super) fn writer_refresh_terminal(
        &mut self,
        family: &FamilyAccess,
        terminal: &AbandonedRecord,
    ) -> Result<()> {
        if self.abandoned.is_some() {
            return Err(invalid("terminal refresh requires known previous absence"));
        }
        let next = Self::open(
            family,
            &self.allocation.format.identity.worktree_id,
            &self.allocation.transaction_id,
        )?;
        if next.allocation != self.allocation
            || next.prepared != self.prepared
            || next.abandoned.as_ref() != Some(terminal)
        {
            return Err(invalid(
                "published terminal receipt readback/binding differs",
            ));
        }
        *self = next;
        Ok(())
    }
}

impl Artifacts {
    pub(super) fn writer_require_guard(
        &self,
        guard: &layrs_vfs::ControlDirectoryGuard,
    ) -> Result<()> {
        let stage = self.allocation.stage_identity()?;
        let prepared = self
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("published guard requires authenticated prepared receipt"))?;
        if guard.root().as_os_str() != stage.root().as_os_str()
            || guard.identity().encode() != prepared.control_identity()?.encode()
            || guard.format_bytes() != self.allocation.format.bytes()?
        {
            return Err(invalid(
                "retained writer guard differs from exact Store binding",
            ));
        }
        Ok(())
    }
}
