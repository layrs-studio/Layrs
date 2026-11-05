//! Refuse no-resource cancellation after a failed/unfinished physical allocation.
//! Absence is observed under the genuine B owner and pinned parent/family gates;
//! it never authorizes cleanup or adoption of an existing destination.
use crate::{
    lock::StoreLock,
    projection::{
        format,
        provider::{
            admitted_catalog_io, admitted_ordinary,
            records::transition::{ProviderTransitionIntentRecord, TransitionState},
            recovery::error::{invalid, ProviderBootstrapCause as Cause, Result},
        },
        shared::{admitted_catalog::AdmittedFamilyContext, provider_access::FamilyAccess},
        SessionMode,
    },
    secure_fs::PinnedDirectory,
};
use layrs_vfs::{Cancellation, OrdinaryRootGuard};
use rusqlite::Connection;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(super) struct Absence {
    family: FamilyAccess,
    parent_guard: OrdinaryRootGuard,
    parent: PinnedDirectory,
    worktrees: PinnedDirectory,
    root_path: PathBuf,
    member_path: PathBuf,
    _lifecycle: StoreLock,
    _catalog: StoreLock,
}

impl Absence {
    pub(super) fn open(
        c: &Connection,
        marker: &format::Marker,
        planned: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        if planned.state != TransitionState::Planned {
            return Err(invalid("no-resource cancellation requires Planned"));
        }
        let binding = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary cancellation family absent"))?;
        let family = FamilyAccess::open(&binding.root)?;
        let record = family.writer_record()?;
        if binding.incarnation != record.incarnation
            || binding.root_identity != record.root_identity
        {
            return Err(invalid("ordinary cancellation family binding differs"));
        }
        let lifecycle = family.lifecycle_lease()?;
        let parent_path = Path::new(&planned.plan.destination_parent);
        let parent_guard = OrdinaryRootGuard::open(parent_path)?;
        let parent = PinnedDirectory::open(parent_path)?;
        if parent_guard.filesystem_identity() != planned.plan.destination_parent_identity
            || parent.identity_token() != planned.plan.destination_parent_identity
        {
            return Err(invalid("ordinary cancellation destination parent changed"));
        }
        super::allocation::require_parent_exclusions(marker, &family, parent_path, cancel)?;
        let worktrees = family.writer_worktrees()?;
        let root_path = parent_path.join(&planned.plan.destination_leaf_nonce);
        let member_path = family
            .member(&planned.plan.source_prefix.destination_identity.worktree_id)
            .parent()
            .ok_or_else(|| invalid("ordinary destination member absent"))?
            .to_owned();
        let catalog_gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        let holder = Self {
            family,
            parent_guard,
            parent,
            worktrees,
            root_path,
            member_path,
            _lifecycle: lifecycle,
            _catalog: catalog_gate,
        };
        holder.verify(cancel)?;
        let actual = admitted_ordinary::inspect(c, marker, Some(cancel))?;
        let retained = AdmittedFamilyContext::new(&actual)?;
        admitted_catalog_io::with_catalog(
            &holder.family.catalog,
            false,
            |catalog| {
                holder.verify(cancel)?;
                retained.validate_catalog(catalog, holder.family.marker_bytes(), Some(cancel))?;
                let owner = planned
                    .plan
                    .source_prefix
                    .destination_identity
                    .worktree_id
                    .as_str();
                for sql in [
                    "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1)",
                    "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
                    "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1)",
                ] {
                    cancel.check()?;
                    if catalog.query_row(sql, [owner], |row| row.get::<_, bool>(0))? {
                        return Err(invalid("ordinary cancellation found destination resources"));
                    }
                }
                Ok(())
            },
            |_| Ok(()),
        )?;
        holder.verify(cancel)?;
        Ok(holder)
    }

    pub(super) fn verify(&self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.family.verify()?;
        self.parent_guard.revalidate()?;
        self.parent.verify_path_identity()?;
        self.worktrees.verify_path_identity()?;
        for path in [&self.root_path, &self.member_path] {
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(Cause::from(error)),
                Ok(_) => {
                    return Err(invalid(
                        "ordinary cancellation found allocated destination resources",
                    ))
                }
            }
        }
        self.parent_guard.revalidate()?;
        self.parent.verify_path_identity()?;
        self.worktrees.verify_path_identity()?;
        cancel.check()?;
        Ok(())
    }
}
