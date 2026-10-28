//! Owned physical destination for an admitted ordinary export. The retained
//! source owner is freshly observed before and between filesystem effects.
//! This holder owns independent family/lifecycle pins; it never borrows the
//! source SQL transaction, admitted lease, image, or mutable source owner.
use super::OrdinaryManagedExport;
use crate::{
    lock::StoreLock,
    projection::{
        file, format,
        provider::{
            admitted_catalog_io, admitted_ordinary,
            database_io::DatabaseFile,
            managed_admission::read_admitted_import,
            records::{
                import::{ImportSource, ProviderImportKind},
                transition::{
                    ProviderTransitionIntentRecord, TransitionBackendKind,
                    TransitionDestinationProof, TransitionPlan, TransitionRole, TransitionState,
                },
                Record,
            },
            recovery::error::{invalid, ProviderBootstrapCause as Cause, Result as CauseResult},
        },
        shared::{
            admitted_catalog::AdmittedFamilyContext,
            provider_access::{
                exact_path, provider_import::source::OrdinarySourceInputs, FamilyAccess,
            },
        },
        SessionMode,
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_core::scm::canonical_json;
use layrs_vfs::{Cancellation, OrdinaryRootGuard};
use std::{
    ffi::OsStr,
    fmt, fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum ManagedAllocationStep {
    Root,
    Control,
    Member,
    Metadata,
    OwnerLock,
    Database,
    TemporaryFormat,
}

/// `confirmed=false` includes an attempted create whose outcome may be
/// uncertain. Neither state authorizes adopting that path on a retry.
#[derive(Debug)]
pub(in crate::projection::provider) struct ManagedAllocationEffect {
    pub(in crate::projection::provider) step: ManagedAllocationStep,
    pub(in crate::projection::provider) path: PathBuf,
    pub(in crate::projection::provider) confirmed: bool,
}

#[derive(Default)]
struct PartialPins {
    family: Option<FamilyAccess>,
    lifecycle: Option<crate::lock::StoreLock>,
    parent_guard: Option<OrdinaryRootGuard>,
    parent: Option<PinnedDirectory>,
    root: Option<PinnedDirectory>,
    control: Option<PinnedDirectory>,
    worktrees: Option<PinnedDirectory>,
    member: Option<PinnedDirectory>,
    metadata: Option<PinnedDirectory>,
    lock: Option<PinnedFile>,
    database: Option<DatabaseFile>,
    temporary_format: Option<PinnedFile>,
}

/// Retains successful pins as well as the ordered attempt log. Dropping this
/// error releases handles but never removes or repairs an uncertain artifact.
pub(in crate::projection::provider) struct ManagedAllocationFailure {
    pub(in crate::projection::provider) cause: Cause,
    pub(in crate::projection::provider) attempted_root: PathBuf,
    pub(in crate::projection::provider) effects: Vec<ManagedAllocationEffect>,
    pins: PartialPins,
}
impl fmt::Debug for ManagedAllocationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedAllocationFailure")
            .field("cause", &self.cause)
            .field("attempted_root", &self.attempted_root)
            .field("effects", &self.effects)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ManagedAllocationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.cause.fmt(f)
    }
}
impl std::error::Error for ManagedAllocationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
impl ManagedAllocationFailure {
    /// Pins are deliberately retained until the caller consumes/drops this
    /// failure; this also proves we do not silently clean up an orphan.
    pub(in crate::projection::provider) fn retained_pin_count(&self) -> usize {
        let p = &self.pins;
        [
            p.family.is_some(),
            p.parent_guard.is_some(),
            p.parent.is_some(),
            p.root.is_some(),
            p.control.is_some(),
            p.worktrees.is_some(),
            p.member.is_some(),
            p.metadata.is_some(),
            p.lock.is_some(),
            p.database.is_some(),
            p.temporary_format.is_some(),
            p.lifecycle.is_some(),
        ]
        .into_iter()
        .filter(|held| *held)
        .count()
    }
}

/// Owns fresh paths and pins. There is no public FORMAT and the empty private
/// database must not be interpreted as a registered destination. An orphan
/// after a crash cannot be adopted by pathname. Resource-aware recovery must
/// first resolve the prior live intent before a fresh nonce and destination
/// identity can start another operation; this holder implements no such recovery.
pub(in crate::projection::provider) struct ManagedDestinationAllocation {
    family: FamilyAccess,
    source_marker: format::Marker,
    parent_guard: OrdinaryRootGuard,
    parent: PinnedDirectory,
    root: PinnedDirectory,
    control: PinnedDirectory,
    worktrees: PinnedDirectory,
    member: PinnedDirectory,
    metadata: PinnedDirectory,
    lock: Option<PinnedFile>,
    lock_identity: String,
    owner_lock: Option<crate::lock::StoreLock>,
    database: DatabaseFile,
    temporary_format: PinnedFile,
    temporary_format_path: PathBuf,
    root_path: PathBuf,
    proof: TransitionDestinationProof,
    plan_digest: String,
    allocation_effects: Vec<ManagedAllocationEffect>,
    _lifecycle: crate::lock::StoreLock,
}
impl ManagedDestinationAllocation {
    pub(in crate::projection::provider) fn checkout_storage(
        &self,
        plan: &TransitionPlan,
        expected: &crate::projection::provider::checkout_journal::ExpectedCheckout,
        cancel: &Cancellation,
    ) -> CauseResult<crate::projection::provider::bootstrap::CheckoutStorage> {
        cancel.check()?;
        self.require_published(plan)?;
        if expected.marker() != &self.proof.format
            || expected.header().plan_digest != self.plan_digest
            || expected.header().destination_proof_digest != self.proof.digest(plan)?
        {
            return Err(invalid("checkout storage differs from retained allocation"));
        }
        let storage = crate::projection::provider::bootstrap::CheckoutStorage::new(
            self.root.try_clone()?,
            self.control.try_clone()?,
            expected,
            cancel,
        )?;
        self.require_published(plan)?;
        Ok(storage)
    }

    pub(in crate::projection::provider) fn family(&self) -> &FamilyAccess {
        &self.family
    }
    pub(in crate::projection::provider) fn allocation_effects(&self) -> &[ManagedAllocationEffect] {
        &self.allocation_effects
    }
    pub(in crate::projection::provider) fn root(&self) -> &Path {
        &self.root_path
    }
    pub(in crate::projection::provider) fn proof(&self) -> &TransitionDestinationProof {
        &self.proof
    }
    pub(in crate::projection::provider) fn destination_database(&self) -> &DatabaseFile {
        &self.database
    }

    /// Factual published FORMAT proof for the next destination writer. This
    /// holds the destination owner lock but grants no SQL/family readiness.
    pub(in crate::projection::provider) fn require_published(
        &self,
        plan: &TransitionPlan,
    ) -> CauseResult<()> {
        self.verify(plan)?;
        if self.owner_lock.is_none() || self.format_location()? != FormatLocation::Final {
            return Err(invalid("managed destination FORMAT is not owner-published"));
        }
        let bytes = self.proof.format.bytes()?;
        self.require_format_bytes(FormatLocation::Final, &bytes, &Cancellation::default())
    }

    /// Pure physical/binding check; no SQL lock or Planned re-observation.
    /// The Copying writer calls this while it owns source catalog gates.
    pub(in crate::projection::provider) fn verify(&self, plan: &TransitionPlan) -> CauseResult<()> {
        if plan.digest()? != self.plan_digest {
            return Err(invalid("managed destination plan/proof changed"));
        }
        self.proof.digest(plan)?;
        if self.parent_guard.root() != Path::new(&plan.destination_parent)
            || self.root_path != self.parent_guard.root().join(&plan.destination_leaf_nonce)
        {
            return Err(invalid("managed destination parent/root path differs"));
        }
        self.parent_guard.revalidate()?;
        self.family.verify()?;
        for pin in [
            &self.parent,
            &self.root,
            &self.control,
            &self.worktrees,
            &self.member,
            &self.metadata,
        ] {
            pin.verify_path_identity()?;
        }
        if let Some(creator) = &self.lock {
            creator.verify_parent()?;
        }
        let lock = self.metadata.open_readonly_file(OsStr::new("layrs.lock"))?;
        if file::identity(lock.file())? != self.lock_identity {
            return Err(invalid("managed destination owner lock changed"));
        }
        self.temporary_format.verify_parent()?;
        self.format_location()?;
        self.database.verify()?;
        let db = crate::layout::open_read_no_follow(self.database.path())?;
        if file::identity(&db)? != self.proof.private_database_identity {
            return Err(invalid("managed destination database identity changed"));
        }
        let rebuilt = marker(
            &self.family,
            &self.source_marker,
            plan,
            &self.root,
            &self.control,
            &self.metadata,
            &self.proof.private_database_identity,
        )?;
        if rebuilt != self.proof.format
            || self.proof.family_marker_digest != self.family.marker_digest()
        {
            return Err(invalid("managed destination FORMAT pins differ"));
        }
        self.parent_guard.revalidate()?;
        Ok(())
    }

    fn format_location(&self) -> CauseResult<FormatLocation> {
        let temporary_name = self
            .temporary_format_path
            .file_name()
            .ok_or_else(|| invalid("managed temporary FORMAT leaf absent"))?;
        let temporary = self.read_format_name(temporary_name)?;
        let final_file = self.read_format_name(OsStr::new("FORMAT"))?;
        let (location, file) = match (temporary, final_file) {
            (Some(file), None) => (FormatLocation::Temporary, file),
            (None, Some(file)) => (FormatLocation::Final, file),
            (None, None) => return Err(invalid("managed FORMAT inode disappeared")),
            (Some(_), Some(_)) => return Err(invalid("managed FORMAT has both names")),
        };
        if file::identity(self.temporary_format.file())? != self.proof.format_file_identity
            || file::identity(file.file())? != self.proof.format_file_identity
            || !self.temporary_format.same_file(&file)
        {
            return Err(invalid("managed FORMAT inode differs"));
        }
        Ok(location)
    }

    fn read_format_name(&self, name: &OsStr) -> CauseResult<Option<PinnedFile>> {
        match self.control.open_readonly_file(name) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatLocation {
    Temporary,
    Final,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum ManagedFormatStep {
    OwnerLock,
    Append,
    FileSync,
    Rename,
    ParentSync,
}
#[derive(Debug)]
pub(in crate::projection::provider) struct ManagedFormatEffect {
    pub(in crate::projection::provider) step: ManagedFormatStep,
    pub(in crate::projection::provider) path: PathBuf,
    pub(in crate::projection::provider) confirmed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum ManagedFormatState {
    Published,
    AlreadyPublished,
}
#[derive(Debug)]
pub(in crate::projection::provider) struct ManagedFormatPublication {
    pub(in crate::projection::provider) state: ManagedFormatState,
    pub(in crate::projection::provider) effects: Vec<ManagedFormatEffect>,
}
#[derive(Debug)]
pub(in crate::projection::provider) struct ManagedFormatPublicationFailure {
    pub(in crate::projection::provider) cause: Cause,
    pub(in crate::projection::provider) effects: Vec<ManagedFormatEffect>,
}
impl fmt::Display for ManagedFormatPublicationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.cause.fmt(f)
    }
}
impl std::error::Error for ManagedFormatPublicationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

fn publication_arm(
    effects: &mut Vec<ManagedFormatEffect>,
    step: ManagedFormatStep,
    path: &Path,
) -> usize {
    let index = effects.len();
    effects.push(ManagedFormatEffect {
        step,
        path: path.to_owned(),
        confirmed: false,
    });
    index
}

impl ManagedDestinationAllocation {
    /// Publish the same allocated FORMAT inode under Copying. The caller keeps
    /// `self` on every error, including uncertain rename/sync. This grants no
    /// source or destination Ready state and cannot adopt a foreign FORMAT.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider) fn publish_format(
        &mut self,
        source: &mut OrdinaryManagedExport<'_>,
        copying: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> std::result::Result<ManagedFormatPublication, ManagedFormatPublicationFailure> {
        let mut effects = Vec::with_capacity(5);
        let result = self.publish_format_inner(source, copying, cancel, &mut effects);
        match result {
            Ok(state) => Ok(ManagedFormatPublication { state, effects }),
            Err(cause) => Err(ManagedFormatPublicationFailure { cause, effects }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_format_inner(
        &mut self,
        source: &mut OrdinaryManagedExport<'_>,
        copying: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
        effects: &mut Vec<ManagedFormatEffect>,
    ) -> CauseResult<ManagedFormatState> {
        copying.validate_fields()?;
        if copying.role != TransitionRole::Source
            || copying.state != TransitionState::Copying
            || copying.destination.as_ref() != Some(&self.proof)
            || copying.reservation.is_some()
            || copying.receipt_id.is_some()
            || source.store.marker != self.source_marker
        {
            return Err(invalid(
                "managed FORMAT publication requires exact source Copying",
            ));
        }
        self.observed_copying(source, copying, cancel)?;
        self.verify(&copying.plan)?;
        self.require_fresh_namespace()?;
        let bytes = self.proof.format.bytes()?;
        if bytes.is_empty() || bytes.len() > 64 * 1024 {
            return Err(invalid("managed FORMAT publication byte bound differs"));
        }
        let final_path = self.root_path.join(".layrs/FORMAT");
        if self.owner_lock.is_none() {
            self.observed_copying(source, copying, cancel)?;
            let index = publication_arm(
                effects,
                ManagedFormatStep::OwnerLock,
                &self.metadata_path().join("layrs.lock"),
            );
            drop(self.lock.take()); // DELETE creator must close before StoreLock opens.
            self.owner_lock = Some(
                self.family
                    .writer_owner(&copying.plan.source_prefix.destination_identity.worktree_id)?,
            );
            effects[index].confirmed = true;
            self.verify(&copying.plan)?;
            self.require_fresh_namespace()?;
        }
        self.observed_copying(source, copying, cancel)?;
        self.require_fresh_namespace()?;
        match self.format_location()? {
            FormatLocation::Final => {
                self.require_format_bytes(FormatLocation::Final, &bytes, cancel)?;
                self.observed_copying(source, copying, cancel)?;
                let index = publication_arm(effects, ManagedFormatStep::ParentSync, &final_path);
                self.control.sync()?;
                effects[index].confirmed = true;
                self.verify(&copying.plan)?;
                self.require_fresh_namespace()?;
                self.require_format_bytes(FormatLocation::Final, &bytes, cancel)?;
                Ok(ManagedFormatState::AlreadyPublished)
            }
            FormatLocation::Temporary => {
                let prefix =
                    self.read_format_bytes(FormatLocation::Temporary, bytes.len(), cancel)?;
                if !bytes.starts_with(&prefix) {
                    return Err(invalid("managed temporary FORMAT has unknown bytes"));
                }
                if prefix.len() < bytes.len() {
                    self.observed_copying(source, copying, cancel)?;
                    self.require_fresh_namespace()?;
                    let index = publication_arm(
                        effects,
                        ManagedFormatStep::Append,
                        &self.temporary_format_path,
                    );
                    self.temporary_format.file_mut().seek(SeekFrom::End(0))?;
                    self.temporary_format
                        .file_mut()
                        .write_all(&bytes[prefix.len()..])?;
                    effects[index].confirmed = true;
                }
                self.observed_copying(source, copying, cancel)?;
                self.require_fresh_namespace()?;
                let index = publication_arm(
                    effects,
                    ManagedFormatStep::FileSync,
                    &self.temporary_format_path,
                );
                self.temporary_format.file().sync_all()?;
                effects[index].confirmed = true;
                self.require_format_bytes(FormatLocation::Temporary, &bytes, cancel)?;
                self.observed_copying(source, copying, cancel)?;
                self.require_fresh_namespace()?;
                let index = publication_arm(effects, ManagedFormatStep::Rename, &final_path);
                if self
                    .temporary_format
                    .rename_to(&self.control, OsStr::new("FORMAT"))?
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "managed FORMAT final name collides; allocated inode retained",
                    )
                    .into());
                }
                effects[index].confirmed = true;
                self.observed_copying(source, copying, cancel)?;
                let index = publication_arm(effects, ManagedFormatStep::ParentSync, &final_path);
                self.control.sync()?;
                effects[index].confirmed = true;
                self.verify(&copying.plan)?;
                self.require_fresh_namespace()?;
                self.require_format_bytes(FormatLocation::Final, &bytes, cancel)?;
                Ok(ManagedFormatState::Published)
            }
        }
    }

    fn metadata_path(&self) -> PathBuf {
        self.family.member(&self.proof.format.identity.worktree_id)
    }

    /// Refuse an unexpected sibling before publishing an ordinary FORMAT.
    /// Enumeration grants no authority; retained pins are checked around it.
    fn require_fresh_namespace(&self) -> CauseResult<()> {
        self.root.verify_path_identity()?;
        self.control.verify_path_identity()?;
        self.member.verify_path_identity()?;
        self.metadata.verify_path_identity()?;
        require_names(&self.root_path, &[".layrs"])?;
        let name = match self.format_location()? {
            FormatLocation::Temporary => self
                .temporary_format_path
                .file_name()
                .ok_or_else(|| invalid("managed temporary FORMAT leaf absent"))?,
            FormatLocation::Final => OsStr::new("FORMAT"),
        };
        require_names(
            &self.root_path.join(".layrs"),
            &[name
                .to_str()
                .ok_or_else(|| invalid("managed FORMAT name is not Unicode"))?],
        )?;
        let member = self
            .metadata_path()
            .parent()
            .ok_or_else(|| invalid("managed member path absent"))?
            .to_owned();
        require_names(&member, &["metadata"])?;
        require_names(&self.metadata_path(), &["layrs.lock", "projection.sqlite3"])?;
        self.root.verify_path_identity()?;
        self.control.verify_path_identity()?;
        self.member.verify_path_identity()?;
        self.metadata.verify_path_identity()?;
        Ok(())
    }

    fn observed_copying(
        &self,
        source: &mut OrdinaryManagedExport<'_>,
        copying: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        observe(source, &self.family, copying, cancel)
    }

    fn read_format_bytes(
        &self,
        expected_location: FormatLocation,
        limit: usize,
        cancel: &Cancellation,
    ) -> CauseResult<Vec<u8>> {
        cancel.check()?;
        if self.format_location()? != expected_location {
            return Err(invalid("managed FORMAT name moved during read"));
        }
        let name = match expected_location {
            FormatLocation::Temporary => self
                .temporary_format_path
                .file_name()
                .ok_or_else(|| invalid("managed temporary FORMAT leaf absent"))?,
            FormatLocation::Final => OsStr::new("FORMAT"),
        };
        let file = self.control.open_readonly_file(name)?;
        if file::identity(file.file())? != self.proof.format_file_identity
            || file.file().metadata()?.len() > limit as u64
        {
            return Err(invalid("managed FORMAT inode/length changed"));
        }
        let length = usize::try_from(file.file().metadata()?.len())
            .map_err(|_| invalid("managed FORMAT length does not fit memory bound"))?;
        let mut actual = Vec::with_capacity(limit.min(length));
        let mut reader = file.file().try_clone()?;
        Read::by_ref(&mut reader)
            .take(limit as u64 + 1)
            .read_to_end(&mut actual)?;
        if actual.len() > limit
            || file.file().metadata()?.len() != actual.len() as u64
            || self.format_location()? != expected_location
        {
            return Err(invalid("managed FORMAT changed during bounded read"));
        }
        cancel.check()?;
        Ok(actual)
    }

    fn require_format_bytes(
        &self,
        location: FormatLocation,
        expected: &[u8],
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        if self.read_format_bytes(location, expected.len(), cancel)? != expected {
            return Err(invalid("managed FORMAT canonical bytes differ"));
        }
        Ok(())
    }
}

fn require_names(path: &Path, expected: &[&str]) -> CauseResult<()> {
    let mut seen = vec![false; expected.len()];
    for entry in fs::read_dir(path)? {
        let name = entry?.file_name();
        let Some(index) = expected
            .iter()
            .position(|wanted| name == OsStr::new(*wanted))
        else {
            return Err(invalid("managed fresh namespace has unexpected entry"));
        };
        if seen[index] {
            return Err(invalid("managed fresh namespace has duplicate entry"));
        }
        seen[index] = true;
    }
    if seen.iter().any(|seen| !seen) {
        return Err(invalid("managed fresh namespace is missing entry"));
    }
    Ok(())
}

fn arm(
    effects: &mut Vec<ManagedAllocationEffect>,
    step: ManagedAllocationStep,
    path: PathBuf,
) -> usize {
    let index = effects.len();
    effects.push(ManagedAllocationEffect {
        step,
        path,
        confirmed: false,
    });
    index
}
fn confirm(effects: &mut [ManagedAllocationEffect], index: usize) {
    effects[index].confirmed = true;
}

/// The source Planned row must already be durable. Capacity for a canonical
/// maximum Copying successor is checked under the real source before any effect.
/// All partial/uncertain resources remain pinned in the returned failure.
/// A failure before the source's later Copying CAS may leave real artifacts
/// while SQL remains Planned; it cannot be acknowledged as resource-free rollback.
pub(in crate::projection::provider) fn allocate(
    source: &mut OrdinaryManagedExport<'_>,
    planned: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> std::result::Result<ManagedDestinationAllocation, ManagedAllocationFailure> {
    let target =
        PathBuf::from(&planned.plan.destination_parent).join(&planned.plan.destination_leaf_nonce);
    let source_marker = source.store.marker.clone();
    let mut pins = PartialPins::default();
    let mut effects = Vec::with_capacity(7);
    let result = (|| -> CauseResult<(TransitionDestinationProof, String, PathBuf, String)> {
        cancel.check()?;
        planned.validate_fields()?;
        if planned.role != TransitionRole::Source
            || planned.state != TransitionState::Planned
            || planned.plan.kind != ProviderImportKind::ManagedOrdinary
            || planned.destination.is_some()
            || planned.reservation.is_some()
            || planned.receipt_id.is_some()
        {
            return Err(invalid(
                "managed destination requires fresh exact source Planned",
            ));
        }
        exact_path(&target)?;
        let plan_digest = planned.plan.digest()?;
        let store = source.store;
        source
            .with_source(cancel, |c, _, prefix, _, _| {
                planned.require_prefix(prefix)?;
                super::super::ordinary_transition::require_allocation_capacity(
                    c,
                    &store.marker,
                    planned,
                    Some(cancel),
                )?;
                Ok(())
            })
            .map_err(Cause::from)?;
        let binding = source_marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary allocation source is not shared"))?;
        pins.family = Some(FamilyAccess::open(&binding.root)?);
        pins.lifecycle = Some(family(&pins)?.lifecycle_lease()?);
        observe(source, family(&pins)?, planned, cancel)?;
        let parent_path = Path::new(&planned.plan.destination_parent);
        let guard = OrdinaryRootGuard::open(parent_path)?;
        if guard.root() != parent_path
            || guard.filesystem_identity() != planned.plan.destination_parent_identity
        {
            return Err(invalid("ordinary destination parent identity differs"));
        }
        require_parent_exclusions(&source_marker, family(&pins)?, parent_path, cancel)?;
        pins.parent_guard = Some(guard);
        let parent = PinnedDirectory::open(parent_path)?;
        if parent.identity_token() != planned.plan.destination_parent_identity {
            return Err(invalid("managed destination pinned parent differs"));
        }
        pins.parent = Some(parent);
        pins.worktrees = Some(family(&pins)?.writer_worktrees()?);
        boundary(source, planned, &pins, cancel)?;

        let root_index = arm(&mut effects, ManagedAllocationStep::Root, target.clone());
        pins.root = Some(
            pins.parent
                .as_ref()
                .unwrap()
                .create_new_child(OsStr::new(&planned.plan.destination_leaf_nonce))?,
        );
        confirm(&mut effects, root_index);
        boundary(source, planned, &pins, cancel)?;

        let control_path = target.join(".layrs");
        let index = arm(&mut effects, ManagedAllocationStep::Control, control_path);
        pins.control = Some(
            pins.root
                .as_ref()
                .unwrap()
                .create_new_private_child(OsStr::new(".layrs"))?,
        );
        confirm(&mut effects, index);
        boundary(source, planned, &pins, cancel)?;

        let member_path = family(&pins)?
            .member(&planned.plan.source_prefix.destination_identity.worktree_id)
            .parent()
            .ok_or_else(|| invalid("managed member path absent"))?
            .to_owned();
        let member_name = member_path
            .file_name()
            .ok_or_else(|| invalid("managed member leaf absent"))?;
        let index = arm(
            &mut effects,
            ManagedAllocationStep::Member,
            member_path.clone(),
        );
        pins.member = Some(
            pins.worktrees
                .as_ref()
                .unwrap()
                .create_new_private_child(member_name)?,
        );
        confirm(&mut effects, index);
        boundary(source, planned, &pins, cancel)?;

        let metadata_path = member_path.join("metadata");
        let index = arm(
            &mut effects,
            ManagedAllocationStep::Metadata,
            metadata_path.clone(),
        );
        pins.metadata = Some(
            pins.member
                .as_ref()
                .unwrap()
                .create_new_private_child(OsStr::new("metadata"))?,
        );
        confirm(&mut effects, index);
        boundary(source, planned, &pins, cancel)?;

        let index = arm(
            &mut effects,
            ManagedAllocationStep::OwnerLock,
            metadata_path.join("layrs.lock"),
        );
        pins.lock = Some(
            pins.metadata
                .as_ref()
                .unwrap()
                .create_new_private_file(OsStr::new("layrs.lock"))?,
        );
        let lock_identity = file::identity(pins.lock.as_ref().unwrap().file())?;
        confirm(&mut effects, index);
        boundary(source, planned, &pins, cancel)?;

        let database_path = metadata_path.join("projection.sqlite3");
        let index = arm(
            &mut effects,
            ManagedAllocationStep::Database,
            database_path.clone(),
        );
        let creator = pins
            .metadata
            .as_ref()
            .unwrap()
            .create_new_private_file(OsStr::new("projection.sqlite3"))?;
        let database_identity = file::identity(creator.file())?;
        drop(creator); // No DELETE creator handle may coexist with later SQLite.
        pins.database = Some(DatabaseFile::open(database_path, &database_identity)?);
        confirm(&mut effects, index);
        boundary(source, planned, &pins, cancel)?;

        let format_name = format!("FORMAT.pending-{}", planned.plan.destination_leaf_nonce);
        let temporary_format_path = target.join(".layrs").join(&format_name);
        let index = arm(
            &mut effects,
            ManagedAllocationStep::TemporaryFormat,
            temporary_format_path.clone(),
        );
        pins.temporary_format = Some(
            pins.control
                .as_ref()
                .unwrap()
                .create_new_private_file(OsStr::new(&format_name))?,
        );
        let format_file_identity = file::identity(pins.temporary_format.as_ref().unwrap().file())?;
        confirm(&mut effects, index);
        boundary(source, planned, &pins, cancel)?;

        let format = marker(
            family(&pins)?,
            &source_marker,
            &planned.plan,
            pins.root.as_ref().unwrap(),
            pins.control.as_ref().unwrap(),
            pins.metadata.as_ref().unwrap(),
            &database_identity,
        )?;
        let proof = TransitionDestinationProof {
            backend: TransitionBackendKind::Ordinary,
            format,
            format_file_identity,
            private_database_identity: database_identity,
            family_marker_digest: family(&pins)?.marker_digest(),
            native_control_identity_hex: None,
            native_root_identity: None,
        };
        proof.digest(&planned.plan)?;
        Ok((proof, plan_digest, temporary_format_path, lock_identity))
    })();
    match result {
        Ok((proof, plan_digest, temporary_format_path, lock_identity)) => {
            let holder = ManagedDestinationAllocation {
                family: pins.family.take().unwrap(),
                source_marker,
                parent_guard: pins.parent_guard.take().unwrap(),
                parent: pins.parent.take().unwrap(),
                root: pins.root.take().unwrap(),
                control: pins.control.take().unwrap(),
                worktrees: pins.worktrees.take().unwrap(),
                member: pins.member.take().unwrap(),
                metadata: pins.metadata.take().unwrap(),
                lock: pins.lock.take(),
                lock_identity,
                owner_lock: None,
                database: pins.database.take().unwrap(),
                temporary_format: pins.temporary_format.take().unwrap(),
                temporary_format_path,
                root_path: target.clone(),
                proof,
                plan_digest,
                allocation_effects: effects,
                _lifecycle: pins.lifecycle.take().unwrap(),
            };
            if let Err(cause) = holder.verify(&planned.plan) {
                // No path cleanup. The holder owns complete pins until moved
                // into the failure; this post-create error still retains them.
                return Err(ManagedAllocationFailure {
                    cause,
                    attempted_root: target,
                    effects: holder.allocation_effects,
                    pins: PartialPins {
                        family: Some(holder.family),
                        lifecycle: Some(holder._lifecycle),
                        parent_guard: Some(holder.parent_guard),
                        parent: Some(holder.parent),
                        root: Some(holder.root),
                        control: Some(holder.control),
                        worktrees: Some(holder.worktrees),
                        member: Some(holder.member),
                        metadata: Some(holder.metadata),
                        lock: holder.lock,
                        database: Some(holder.database),
                        temporary_format: Some(holder.temporary_format),
                    },
                });
            }
            Ok(holder)
        }
        Err(cause) => Err(ManagedAllocationFailure {
            cause,
            attempted_root: target,
            effects,
            pins,
        }),
    }
}

fn family(pins: &PartialPins) -> CauseResult<&FamilyAccess> {
    pins.family
        .as_ref()
        .ok_or_else(|| invalid("ordinary destination family pin absent"))
}

fn boundary(
    source: &mut OrdinaryManagedExport<'_>,
    planned: &ProviderTransitionIntentRecord,
    pins: &PartialPins,
    cancel: &Cancellation,
) -> CauseResult<()> {
    cancel.check()?;
    pins.parent_guard
        .as_ref()
        .ok_or_else(|| invalid("ordinary destination parent guard absent"))?
        .revalidate()?;
    pins.parent
        .as_ref()
        .ok_or_else(|| invalid("ordinary destination parent pin absent"))?
        .verify_path_identity()?;
    for pin in [
        &pins.root,
        &pins.control,
        &pins.worktrees,
        &pins.member,
        &pins.metadata,
    ]
    .into_iter()
    .flatten()
    {
        pin.verify_path_identity()?;
    }
    let parent = Path::new(&planned.plan.destination_parent);
    require_parent_exclusions(&source.store.marker, family(pins)?, parent, cancel)?;
    observe(source, family(pins)?, planned, cancel)
}

/// The real source callback protects and rehashes its current image, verifies
/// its SQL closure and performs the same checks again after this callback.
/// No source connection or detached DATA escapes as an allocation capability.
fn observe(
    source: &mut OrdinaryManagedExport<'_>,
    family: &FamilyAccess,
    expected: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<()> {
    expected.validate_fields()?;
    if expected.role != TransitionRole::Source
        || !matches!(
            expected.state,
            TransitionState::Planned | TransitionState::Copying
        )
        || expected.reservation.is_some()
        || expected.receipt_id.is_some()
        || expected.plan.kind != ProviderImportKind::ManagedOrdinary
        || expected.destination.is_some() != (expected.state == TransitionState::Copying)
        || !matches!(
            &expected.plan.source_prefix.source,
            ImportSource::Ordinary {
                private_schema: 9,
                admission: Some(_),
                ..
            }
        )
    {
        return Err(invalid(
            "ordinary allocation requires unreserved Planned or Copying",
        ));
    }
    let store = source.store;
    source.with_source(cancel, |c, _, prefix, closure, _| {
        expected.require_prefix(prefix)?;
        closure.require_source_connection(c)?;
        if closure.prefix_id() != expected.plan.source_prefix_id {
            return Err(super::invalid("ordinary allocation source closure changed"));
        }
        let admitted = read_admitted_import(c, &store.marker, Some(cancel))?;
        if admitted.outgoing().iter().find(|row| row.transaction_id == expected.transaction_id)
            != Some(expected)
        {
            return Err(super::invalid("ordinary allocation source intent changed"));
        }
        let data = admitted_ordinary::inspect(c, &store.marker, Some(cancel))?;
        let provenance = AdmittedFamilyContext::new(&data)?;
        let record = family.writer_record()?;
        let binding = data.binding();
        let shared = store.marker.shared.as_ref()
            .ok_or_else(|| super::invalid("ordinary allocation lost shared source"))?;
        if shared.root != family.root()
            || shared.incarnation != record.incarnation
            || shared.root_identity != record.root_identity
            || shared.metadata_path != family.member(&store.marker.identity.worktree_id)
            || shared.database_identity != binding.database_identity
            || binding.owner != store.marker.identity.worktree_id
            || binding.family_incarnation != record.incarnation
            || binding.family_root_identity != record.root_identity
            || binding.family_catalog_identity != record.catalog_file_identity
            || binding.family_marker_digest != family.marker_digest()
            || family.writer_identity(&store.marker.identity.worktree_id) != store.marker.identity
            || family.writer_identity(&prefix.destination_identity.worktree_id)
                != prefix.destination_identity
        {
            return Err(super::invalid("ordinary allocation differs from actual admitted family"));
        }
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"), SessionMode::Shared,
        ).map_err(Cause::from)?;
        family.verify()?;
        let result = admitted_catalog_io::with_catalog(
            &family.catalog,
            false,
            |catalog| {
                cancel.check()?;
                family.verify()?;
                if expected.state == TransitionState::Copying {
                    // A retained allocation may be replayed after its exact C
                    // member was registered. The common target kernel permits
                    // only Absent/Initializing with the same FORMAT/database,
                    // never Ready or a resource-bearing later phase.
                    family.validate_ordinary_import_initializing(
                        catalog,
                        &OrdinarySourceInputs::new(
                            c, &store.marker, &admitted, closure, &provenance,
                        ),
                        expected,
                        Some(cancel),
                    )?;
                } else {
                    provenance.validate_catalog(catalog, family.marker_bytes(), Some(cancel))?;
                    if catalog.query_row(
                        "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1)",
                        [prefix.destination_identity.worktree_id.as_str()],
                        |row| row.get::<_, bool>(0),
                    )? {
                        return Err(invalid("ordinary fresh destination is already registered"));
                    }
                }
                let owner = prefix.destination_identity.worktree_id.as_str();
                for sql in [
                    "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
                    "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1)",
                    "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
                    "SELECT EXISTS(SELECT 1 FROM exact_installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
                ] {
                    cancel.check()?;
                    if catalog.query_row(sql, [owner], |row| row.get::<_, bool>(0))? {
                        return Err(invalid("ordinary fresh destination has retained family ownership"));
                    }
                }
                family.verify()?;
                cancel.check()?;
                Ok(())
            },
            |_| Ok(()),
        );
        let verified = family.verify();
        drop(gate);
        combine(result, verified)?;
        admitted.require(c, Some(cancel))?;
        Ok(())
    }).map_err(Cause::from)
}

fn combine<T>(result: CauseResult<T>, close: CauseResult<()>) -> CauseResult<T> {
    match (result, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(primary), Err(close)) => Err(Cause::Cleanup {
            primary: Box::new(primary),
            close: Box::new(close),
        }),
    }
}

pub(super) fn require_parent_exclusions(
    source: &format::Marker,
    family: &FamilyAccess,
    parent: &Path,
    cancel: &Cancellation,
) -> CauseResult<()> {
    let source_root = Path::new(&source.checkout_root);
    let source_metadata = source.metadata_path(source_root);
    let excluded = [
        source_root.to_owned(),
        source_root.join(".layrs"),
        source_metadata,
        family.root().to_owned(),
        family.root().join("metadata"),
    ];
    let mut identities = Vec::with_capacity(excluded.len());
    for path in &excluded {
        cancel.check()?;
        let pin = PinnedDirectory::open(path)?;
        pin.verify_path_identity()?;
        identities.push(pin.identity_token());
    }
    for ancestor in parent.ancestors() {
        cancel.check()?;
        let pin = PinnedDirectory::open(ancestor)?;
        let identity = pin.identity_token();
        if excluded.iter().any(|path| path == ancestor) || identities.contains(&identity) {
            return Err(invalid(
                "ordinary destination parent overlaps source/family metadata",
            ));
        }
        pin.verify_path_identity()?;
    }
    family.verify()?;
    cancel.check()?;
    Ok(())
}

fn marker(
    family: &FamilyAccess,
    source: &format::Marker,
    plan: &TransitionPlan,
    root: &PinnedDirectory,
    control: &PinnedDirectory,
    metadata: &PinnedDirectory,
    database_identity: &str,
) -> CauseResult<format::Marker> {
    let record = family.writer_record()?;
    let target = Path::new(&plan.destination_parent).join(&plan.destination_leaf_nonce);
    let source_bytes = source.bytes()?;
    let original: serde_json::Value =
        serde_json::from_slice(&source_bytes).map_err(|error| invalid(error.to_string()))?;
    let mut value = serde_json::json!({
        "schemaVersion":"layrs.projection-store-format.v1", "magic":"LAYRS_NATIVE_STORE",
        "formatVersion":4, "minReaderVersion":4,
        "identity":plan.source_prefix.destination_identity,
        "checkoutRoot":exact_path(&target)?, "checkoutIdentity":root.identity_token(),
        "storeIdentity":control.identity_token(), "metadataIdentity":metadata.identity_token(),
        "initialSnapshotDigest":source.initial_snapshot_digest,
        "importDigest":plan.source_prefix_id,
        "shared":{
            "root":family.root(), "incarnation":record.incarnation,
            "rootIdentity":record.root_identity,
            "metadataPath":family.member(&plan.source_prefix.destination_identity.worktree_id),
            "databaseIdentity":database_identity
        }
    });
    // Copy the genuine generation-zero commitment, never the source's current
    // selection frontier. FORMAT deliberately omits All's optional digest.
    if let Some(selection) = original.get("initialSelectionDigest") {
        value["initialSelectionDigest"] = selection.clone();
    }
    let bytes = canonical_json(&value).map_err(|error| invalid(error.to_string()))?;
    let parsed = format::parse_marker_bytes(&bytes)?;
    if parsed.bytes()?.len() > 65536 {
        return Err(invalid("ordinary destination FORMAT exceeds bound"));
    }
    Ok(parsed)
}
