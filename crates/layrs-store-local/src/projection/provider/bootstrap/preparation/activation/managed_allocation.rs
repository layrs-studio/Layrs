//! Exclusive filesystem allocation for a fresh managed destination. The
//! source Planned row already exists; no FORMAT bytes, SQLite rows, family
//! registration, or imported content are written by this module.
use super::*;
use crate::{
    projection::{
        file,
        provider::{
            capture_lock::CaptureAttemptLock,
            database_io::DatabaseFile,
            records::{
                import::ProviderImportKind,
                transition::{
                    ProviderTransitionIntentRecord, TransitionBackendKind,
                    TransitionDestinationProof, TransitionPlan, TransitionRole, TransitionState,
                },
                Record,
            },
        },
        shared::provider_access::exact_path,
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_core::scm::canonical_json;
use layrs_vfs::{Cancellation, ExportParentGuard, Session};
use std::{
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedAllocationStep {
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
pub(super) struct ManagedAllocationEffect {
    pub(super) step: ManagedAllocationStep,
    pub(super) path: PathBuf,
    pub(super) confirmed: bool,
}

#[derive(Default)]
struct PartialPins<'a> {
    lifecycle: Option<crate::lock::StoreLock>,
    parent_guard: Option<ExportParentGuard<'a>>,
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
pub(super) struct ManagedAllocationFailure<'a> {
    pub(super) cause: Cause,
    pub(super) attempted_root: PathBuf,
    pub(super) effects: Vec<ManagedAllocationEffect>,
    pins: PartialPins<'a>,
}
impl fmt::Debug for ManagedAllocationFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedAllocationFailure")
            .field("cause", &self.cause)
            .field("attempted_root", &self.attempted_root)
            .field("effects", &self.effects)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ManagedAllocationFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.cause.fmt(f)
    }
}
impl std::error::Error for ManagedAllocationFailure<'_> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
impl ManagedAllocationFailure<'_> {
    /// Pins are deliberately retained until the caller consumes/drops this
    /// failure; this also proves we do not silently clean up an orphan.
    pub(super) fn retained_pin_count(&self) -> usize {
        let p = &self.pins;
        [
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
/// after a crash requires a new nonce and logical destination identity; its
/// pathname alone can never be used as a resume capability.
pub(super) struct ManagedDestinationAllocation<'a> {
    held: &'a outcome::PublicationPins,
    parent_guard: ExportParentGuard<'a>,
    parent: PinnedDirectory,
    root: PinnedDirectory,
    control: PinnedDirectory,
    worktrees: PinnedDirectory,
    member: PinnedDirectory,
    metadata: PinnedDirectory,
    lock: PinnedFile,
    database: DatabaseFile,
    temporary_format: PinnedFile,
    temporary_format_path: PathBuf,
    root_path: PathBuf,
    proof: TransitionDestinationProof,
    plan_digest: String,
    _lifecycle: crate::lock::StoreLock,
}
impl ManagedDestinationAllocation<'_> {
    pub(super) fn root(&self) -> &Path {
        &self.root_path
    }
    pub(super) fn proof(&self) -> &TransitionDestinationProof {
        &self.proof
    }
    pub(super) fn destination_database(&self) -> &DatabaseFile {
        &self.database
    }

    /// Pure physical/binding check; no SQL lock or Planned re-observation.
    /// The Copying writer calls this while it owns source catalog gates.
    pub(super) fn verify(&self, plan: &TransitionPlan) -> CauseResult<()> {
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
        self.held.family.verify()?;
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
        self.lock.verify_parent()?;
        let lock = self.metadata.open_file(OsStr::new("layrs.lock"))?;
        if !self.lock.same_file(&lock) {
            return Err(invalid("managed destination owner lock changed"));
        }
        self.temporary_format.verify_parent()?;
        let name = self
            .temporary_format_path
            .file_name()
            .ok_or_else(|| invalid("managed temporary FORMAT leaf absent"))?;
        let format = self.control.open_file(name)?;
        if !self.temporary_format.same_file(&format)
            || file::identity(self.temporary_format.file())? != self.proof.format_file_identity
        {
            return Err(invalid("managed temporary FORMAT identity changed"));
        }
        self.database.verify()?;
        let db = crate::layout::open_read_no_follow(self.database.path())?;
        if file::identity(&db)? != self.proof.private_database_identity {
            return Err(invalid("managed destination database identity changed"));
        }
        let rebuilt = marker(
            self.held,
            plan,
            &self.root,
            &self.control,
            &self.metadata,
            &self.proof.private_database_identity,
        )?;
        if rebuilt != self.proof.format
            || self.proof.family_marker_digest != self.held.family.marker_digest()
        {
            return Err(invalid("managed destination FORMAT pins differ"));
        }
        self.parent_guard.revalidate()?;
        Ok(())
    }
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

#[allow(clippy::too_many_arguments)]
pub(super) fn allocate<'a>(
    session: &'a Session,
    held: &'a outcome::PublicationPins,
    source_root: &'a layrs_vfs::ProviderRootGuard,
    mount: &'a ProviderActiveMountRecord,
    activation: &'a ProviderActivationRecord,
    attempt: &'a CaptureAttemptLock,
    planned: &'a ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> std::result::Result<ManagedDestinationAllocation<'a>, ManagedAllocationFailure<'a>> {
    let target =
        PathBuf::from(&planned.plan.destination_parent).join(&planned.plan.destination_leaf_nonce);
    let mut pins = PartialPins::default();
    let mut effects = Vec::with_capacity(7);
    let result = (|| -> CauseResult<(TransitionDestinationProof, String, PathBuf)> {
        cancel.check()?;
        planned.validate_fields()?;
        if planned.role != TransitionRole::Source
            || planned.state != TransitionState::Planned
            || planned.plan.kind != ProviderImportKind::ManagedOrdinary
            || planned.destination.is_some()
            || planned.reservation.is_some()
            || planned.receipt_id.is_some()
            || held
                .family
                .writer_identity(&planned.plan.source_prefix.destination_identity.worktree_id)
                != planned.plan.source_prefix.destination_identity
        {
            return Err(invalid(
                "managed destination requires fresh exact source Planned",
            ));
        }
        exact_path(&target)?;
        let plan_digest = planned.plan.digest()?;
        // Retain lifecycle before the first source observation, closing the
        // gap between source admission and the first filesystem effect.
        pins.lifecycle = Some(held.family.lifecycle_lease()?);
        managed_journal::observe_planned(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            cancel,
        )?;
        let parent_path = Path::new(&planned.plan.destination_parent);
        let guard = session.guard_export_parent(parent_path)?;
        if guard.root() != parent_path
            || guard.filesystem_identity() != planned.plan.destination_parent_identity
        {
            return Err(invalid("managed destination parent identity differs"));
        }
        let excluded = [
            held.family.root().to_owned(),
            PathBuf::from(&held.artifacts.allocation.private.metadata_path),
            PathBuf::from(&held.artifacts.allocation.private.metadata_path)
                .join("provider-control"),
        ];
        for ancestor in parent_path.ancestors() {
            if session.is_owned_root_directory(ancestor)? {
                return Err(invalid(
                    "managed destination parent overlaps source checkout",
                ));
            }
            let id = PinnedDirectory::open(ancestor)?.identity_token();
            if excluded.iter().any(|path| path == ancestor)
                || id == held.artifacts.allocation.family.root_identity
                || id == held.artifacts.allocation.private.metadata_identity
                || id == held.artifacts.allocation.private.provider_control_identity
            {
                return Err(invalid(
                    "managed destination parent overlaps family/control",
                ));
            }
        }
        pins.parent_guard = Some(guard);
        let parent = PinnedDirectory::open(parent_path)?;
        if parent.identity_token() != planned.plan.destination_parent_identity {
            return Err(invalid("managed destination pinned parent differs"));
        }
        pins.parent = Some(parent);
        pins.worktrees = Some(held.family.writer_worktrees()?);
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

        let root_index = arm(&mut effects, ManagedAllocationStep::Root, target.clone());
        pins.root = Some(
            pins.parent
                .as_ref()
                .unwrap()
                .create_new_child(OsStr::new(&planned.plan.destination_leaf_nonce))?,
        );
        confirm(&mut effects, root_index);
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

        let control_path = target.join(".layrs");
        let index = arm(&mut effects, ManagedAllocationStep::Control, control_path);
        pins.control = Some(
            pins.root
                .as_ref()
                .unwrap()
                .create_new_private_child(OsStr::new(".layrs"))?,
        );
        confirm(&mut effects, index);
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

        let member_path = held
            .family
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
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

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
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

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
        confirm(&mut effects, index);
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

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
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

        let format_name = format!("FORMAT.pending-{}", planned.plan.destination_leaf_nonce);
        let temporary_format_path = target.join(".layrs").join(&format_name);
        let index = arm(
            &mut effects,
            ManagedAllocationStep::TemporaryFormat,
            temporary_format_path.clone(),
        );
        let temporary_format = pins
            .control
            .as_ref()
            .unwrap()
            .create_new_private_file(OsStr::new(&format_name))?;
        let format_file_identity = file::identity(temporary_format.file())?;
        pins.temporary_format = Some(temporary_format);
        confirm(&mut effects, index);
        boundary(
            held,
            source_root,
            mount,
            activation,
            attempt,
            planned,
            &pins,
            cancel,
        )?;

        let format = marker(
            held,
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
            family_marker_digest: held.family.marker_digest(),
            native_control_identity_hex: None,
            native_root_identity: None,
        };
        proof.digest(&planned.plan)?;
        Ok((proof, plan_digest, temporary_format_path))
    })();
    match result {
        Ok((proof, plan_digest, temporary_format_path)) => {
            let holder = ManagedDestinationAllocation {
                held,
                parent_guard: pins.parent_guard.take().unwrap(),
                parent: pins.parent.take().unwrap(),
                root: pins.root.take().unwrap(),
                control: pins.control.take().unwrap(),
                worktrees: pins.worktrees.take().unwrap(),
                member: pins.member.take().unwrap(),
                metadata: pins.metadata.take().unwrap(),
                lock: pins.lock.take().unwrap(),
                database: pins.database.take().unwrap(),
                temporary_format: pins.temporary_format.take().unwrap(),
                temporary_format_path,
                root_path: target.clone(),
                proof,
                plan_digest,
                _lifecycle: pins.lifecycle.take().unwrap(),
            };
            if let Err(cause) = holder.verify(&planned.plan) {
                // No path cleanup. The holder owns complete pins until moved
                // into the failure; this post-create error still retains them.
                return Err(ManagedAllocationFailure {
                    cause,
                    attempted_root: target,
                    effects,
                    pins: PartialPins {
                        lifecycle: Some(holder._lifecycle),
                        parent_guard: Some(holder.parent_guard),
                        parent: Some(holder.parent),
                        root: Some(holder.root),
                        control: Some(holder.control),
                        worktrees: Some(holder.worktrees),
                        member: Some(holder.member),
                        metadata: Some(holder.metadata),
                        lock: Some(holder.lock),
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

fn boundary(
    held: &outcome::PublicationPins,
    source_root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    planned: &ProviderTransitionIntentRecord,
    pins: &PartialPins<'_>,
    cancel: &Cancellation,
) -> CauseResult<()> {
    cancel.check()?;
    pins.parent_guard
        .as_ref()
        .ok_or_else(|| invalid("managed parent guard absent"))?
        .revalidate()?;
    pins.parent
        .as_ref()
        .ok_or_else(|| invalid("managed parent pin absent"))?
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
    managed_journal::observe_planned(
        held,
        source_root,
        mount,
        activation,
        attempt,
        planned,
        cancel,
    )
}

fn marker(
    held: &outcome::PublicationPins,
    plan: &TransitionPlan,
    root: &PinnedDirectory,
    control: &PinnedDirectory,
    metadata: &PinnedDirectory,
    database_identity: &str,
) -> CauseResult<crate::projection::format::Marker> {
    let family = &held.family;
    let family_record = family.writer_record()?;
    let target = Path::new(&plan.destination_parent).join(&plan.destination_leaf_nonce);
    let source = &held.artifacts.allocation.source;
    let mut marker = serde_json::json!({
        "schemaVersion":"layrs.projection-store-format.v1", "magic":"LAYRS_NATIVE_STORE",
        "formatVersion":4, "minReaderVersion":4,
        "identity":plan.source_prefix.destination_identity,
        "checkoutRoot":exact_path(&target)?, "checkoutIdentity":root.identity_token(),
        "storeIdentity":control.identity_token(), "metadataIdentity":metadata.identity_token(),
        "initialSnapshotDigest":source.snapshot_digest,
        "importDigest":plan.source_prefix_id,
        "shared":{
            "root":family.root(), "incarnation":family_record.incarnation,
            "rootIdentity":family_record.root_identity,
            "metadataPath":family.member(&plan.source_prefix.destination_identity.worktree_id),
            "databaseIdentity":database_identity
        }
    });
    let all = crate::projection::selection::initial_digest(
        &layrs_core::projection::ProjectionSelection::All,
    )?;
    if source.selection_payload_digest != all {
        marker["initialSelectionDigest"] = source.selection_payload_digest.clone().into();
    }
    let bytes = canonical_json(&marker).map_err(|error| invalid(error.to_string()))?;
    let parsed = crate::projection::format::parse_marker_bytes(&bytes)?;
    if parsed.bytes()?.len() > 65536 {
        return Err(invalid("managed destination FORMAT exceeds bound"));
    }
    Ok(parsed)
}
