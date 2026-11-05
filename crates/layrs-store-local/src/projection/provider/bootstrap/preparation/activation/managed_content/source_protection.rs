//! Concrete source retention adapter. The caller keeps the complete native
//! checkpoint owner, both allocations and actual Published CAS pins borrowed.
//! DATA root construction alone never authorizes source publication.
use super::*;
use crate::projection::{
    provider::{
        import_validation, source_companion::SourceCompanionPublication, validation,
        validation::PrivateActivePublicationValidation,
    },
    shared::{
        exact_content::{
            PublishedCasProof, validate_protected_installations, verify_published_cas,
        },
        exact_protection::{ExactProtection, ProtectionPresence},
        source_companion::{SourceCompanionFamilyContext, SourceCompanionPresence},
    },
};

pub(in crate::projection::provider::bootstrap::preparation::activation) struct ManagedSourceCompanionProtection<
    's,
    'a,
> {
    data: &'s SourceCompanionPublication<'a>,
    bytes: PublishedCasProof,
}
impl<'s, 'a> ManagedSourceCompanionProtection<'s, 'a> {
    pub(super) fn data(&self) -> &SourceCompanionPublication<'a> {
        self.data
    }
    pub(super) fn require(
        &self,
        lease: &ManagedImportLease<'_, '_, '_, '_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let binding = lease.exact_binding()?;
        if binding != self.data.protection().binding.destination
            || lease.manifests != self.data.rows().manifests()
        {
            return Err(invalid(
                "retained source protection belongs to another lease or U",
            ));
        }
        let ImportSource::Provider { version, .. } = &lease.staging.plan.source_prefix.source
        else {
            return Err(invalid("source protection prefix kind differs"));
        };
        lease
            .image
            .require_retained(lease.root, &version.effective_image_digest, cancel)?;
        self.bytes.require(&binding, lease.manifests, cancel)?;
        self.data.require(
            &lease.held.artifacts.allocation,
            lease.staging,
            self.data.receipt(),
            Some(cancel),
        )?;
        lease.boundary(cancel)
    }
}

impl ManagedImportLease<'_, '_, '_, '_, '_> {
    fn read_source_protection_scope<T>(
        &self,
        file: &database_io::DatabaseFile,
        cancel: &Cancellation,
        read: impl FnOnce(&Connection) -> CauseResult<T>,
    ) -> CauseResult<database_io::ReadState<T>> {
        if self
            .companion
            .as_ref()
            .is_some_and(|data| data.coverage.is_some())
        {
            database_io::read_cancellable_strict(file, cancel, read)
        } else {
            database_io::read_cancellable(file, cancel, read)
        }
    }
    fn require_source_companion_destination_member(
        &self,
        c: &Connection,
        data: &SourceCompanionPublication<'_>,
    ) -> CauseResult<()> {
        let destination = self
            .staging
            .destination
            .as_ref()
            .ok_or_else(|| invalid("source protection C destination absent"))?;
        let shared = destination
            .format
            .shared
            .as_ref()
            .ok_or_else(|| invalid("source protection C shared identity absent"))?;
        let valid: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND database_identity=?2 AND metadata_path=?3 AND marker=?4 AND state IN ('initializing','ready'))",
            rusqlite::params![data.protection().binding.destination.owner.as_str(), destination.private_database_identity,
                crate::projection::shared::provider_access::exact_path(&shared.metadata_path)?, destination.format.bytes()?], |r| r.get(0))?;
        if !valid {
            return Err(invalid(
                "source protection C family member identity differs",
            ));
        }
        Ok(())
    }
    fn source_companion_snapshot<T>(
        &self,
        data: &SourceCompanionPublication<'_>,
        cancel: &Cancellation,
        action: impl FnOnce(&PrivateActivePublicationValidation) -> CauseResult<T>,
    ) -> CauseResult<T> {
        let prepared = self
            .held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("source protection prepared receipt absent"))?;
        managed_destination::valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |c| {
                let full = validation::private_active_publication_with_source(
                    c,
                    &self.held.artifacts.allocation,
                    prepared,
                    &self.held.binding,
                    Some(self.root.registration().root_id()),
                    Some(cancel),
                )?;
                let current = full
                    .transitions
                    .iter()
                    .find(|i| i.transaction_id == self.staging.transaction_id)
                    .ok_or_else(|| invalid("source protection original Staging absent"))?;
                if current != self.staging
                    || full.active.mount != *self.mount
                    || full.active.activation != *self.activation
                {
                    return Err(invalid(
                        "source protection actual source owner or phase changed",
                    ));
                }
                let ImportSource::Provider { version, .. } =
                    &self.staging.plan.source_prefix.source
                else {
                    return Err(invalid("source protection provider prefix absent"));
                };
                let closure = import_validation::verify_source_publication(c,&self.held.artifacts.allocation.format,&self.staging.plan.source_prefix,
                crate::projection::provider::records::import::ImportSourceReferences::Provider {allocation:&self.held.artifacts.allocation,prepared,activation:&full.active.activation},
                &full,&version.effective_image_digest,Some(cancel))?;
                self.rederive(c, &closure, &full, cancel)?;
                data.require(
                    &self.held.artifacts.allocation,
                    self.staging,
                    data.receipt(),
                    Some(cancel),
                )?;
                self.boundary(cancel)?;
                action(&full)
            },
        )?)
    }

    /// Fixed B reservation, before either receipt and before C reserve/install.
    /// No content, source private row, quota growth or release is performed.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn reserve_source_companion(
        &self,
        data: &SourceCompanionPublication<'_>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.source_companion_snapshot(data, cancel, |full| {
            let check = |c: &Connection| -> CauseResult<SourceCompanionPresence> {
                self.held.family.validate_source_companion_catalog(
                    c,
                    &self.held.artifacts.allocation,
                    &SourceCompanionFamilyContext {
                        publication: full,
                        pending: Some(data.protection()),
                    },
                    Some(cancel),
                )?;
                data.protection().require_capacity(c, Some(cancel))?;
                data.protection().inspect(c, Some(cancel))
            };
            let file = &self.held.family.catalog;
            let before = managed_destination::valid(
                self.read_source_protection_scope(file, cancel, &check)?,
            )?;
            if before != SourceCompanionPresence::Absent {
                return Ok(());
            }
            database_io::validate_native_filename(file.path())?;
            let admission = database_io::clean_admission(file, false, |c| {
                if check(c)? != before {
                    return Err(invalid("source reservation admission changed"));
                }
                Ok(())
            })?;
            self.boundary(cancel)?;
            let bare = admission.open(file, close)?;
            let result = (|| {
                admission.require_geometry(&bare)?;
                let c = bare.connection();
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let operation = (|| {
                    if check(c)? != before {
                        return Err(invalid("source reservation predecessor changed"));
                    }
                    data.protection().reserve(c, Some(cancel))?;
                    if check(c)? != SourceCompanionPresence::Reserved {
                        return Err(invalid("source reservation prospective differs"));
                    }
                    self.boundary(cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(())
                })();
                rollback(c, operation)
            })();
            bare.finish_writer(result, close)
        })?;
        // Only after BOTH positive closes may any fresh read occur.
        self.source_companion_snapshot(data, cancel, |full| {
            managed_destination::valid(self.read_source_protection_scope(
                &self.held.family.catalog,
                cancel,
                |c| {
                    self.held.family.validate_source_companion_catalog(
                        c,
                        &self.held.artifacts.allocation,
                        &SourceCompanionFamilyContext {
                            publication: full,
                            pending: Some(data.protection()),
                        },
                        Some(cancel),
                    )?;
                    if data.protection().inspect(c, Some(cancel))?
                        == SourceCompanionPresence::Absent
                    {
                        return Err(invalid("source reservation durable readback absent"));
                    }
                    Ok(())
                },
            )?)
        })
    }

    /// Acquire or reopen complete B retention AFTER actual C Published SQL and
    /// byte authentication, BEFORE C's new receipt. Root replay writes nothing.
    /// The complete image borrow remains alive through later B writers/closes.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn protect_source_companion<
        's,
        'a,
    >(
        &self,
        data: &'s SourceCompanionPublication<'a>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedSourceCompanionProtection<'s, 'a>> {
        self.image
            .require_retained(self.root, self.image.digest(), cancel)?;
        let binding = &data.protection().binding.destination;
        if self.exact_binding()? != *binding || self.manifests != data.rows().manifests() {
            return Err(invalid("source protection C union differs"));
        }
        let c_root = ExactProtection::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            self.manifests,
            Some(cancel),
        )?;
        // Every scope independently validates SQL Published; byte verification
        // is outside SQL gates and retains all manifest/chunk handles.
        let read_published = || {
            let _locks = self.held.family.locks(
                &self.held.artifacts.allocation.format.identity.worktree_id,
                true,
            )?;
            self.source_companion_snapshot(data, cancel, |full| {
                managed_destination::valid(self.read_source_protection_scope(
                    &self.held.family.catalog,
                    cancel,
                    |c| {
                        self.held.family.validate_source_companion_catalog(
                            c,
                            &self.held.artifacts.allocation,
                            &SourceCompanionFamilyContext {
                                publication: full,
                                pending: Some(data.protection()),
                            },
                            Some(cancel),
                        )?;
                        self.require_source_companion_destination_member(c, data)?;
                        let state = validate_protected_installations(
                            c,
                            binding,
                            self.manifests,
                            &c_root,
                            Some(cancel),
                        )?;
                        if c_root.inspect(c, Some(cancel))? != ProtectionPresence::Present {
                            return Err(invalid(
                                "source retention requires C Published protected union",
                            ));
                        }
                        Ok(state)
                    },
                )?)
            })
        };
        let before = read_published()?;
        let bytes =
            verify_published_cas(self.pin_exact_storage()?, binding, self.manifests, cancel)?;
        if read_published()? != before {
            return Err(invalid(
                "C Published SQL changed during source byte authentication",
            ));
        }
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.source_companion_snapshot(data, cancel, |full| {
            let check = |c: &Connection| -> CauseResult<SourceCompanionPresence> {
                self.held.family.validate_source_companion_catalog(
                    c,
                    &self.held.artifacts.allocation,
                    &SourceCompanionFamilyContext {
                        publication: full,
                        pending: Some(data.protection()),
                    },
                    Some(cancel),
                )?;
                if validate_protected_installations(
                    c,
                    binding,
                    self.manifests,
                    &c_root,
                    Some(cancel),
                )? != before
                    || c_root.inspect(c, Some(cancel))? != ProtectionPresence::Present
                {
                    return Err(invalid("source protection C Published predecessor changed"));
                }
                self.require_source_companion_destination_member(c, data)?;
                bytes.require(binding, self.manifests, cancel)?;
                data.protection().inspect(c, Some(cancel))
            };
            let file = &self.held.family.catalog;
            let previous = managed_destination::valid(
                self.read_source_protection_scope(file, cancel, &check)?,
            )?;
            if previous == SourceCompanionPresence::Protected {
                return Ok(());
            }
            if previous != SourceCompanionPresence::Reserved {
                return Err(invalid("source protection fixed B reserve absent"));
            }
            database_io::validate_native_filename(file.path())?;
            let admission = database_io::clean_admission(file, false, |c| {
                if check(c)? != previous {
                    return Err(invalid("source protection admission changed"));
                }
                Ok(())
            })?;
            self.boundary(cancel)?;
            let bare = admission.open(file, close)?;
            let result = (|| {
                admission.require_geometry(&bare)?;
                let c = bare.connection();
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let operation = (|| {
                    if check(c)? != previous {
                        return Err(invalid("source protection writer predecessor changed"));
                    }
                    data.protection().protect(c, Some(cancel))?;
                    if check(c)? != SourceCompanionPresence::Protected {
                        return Err(invalid("source protection prospective readback differs"));
                    }
                    bytes.require(binding, self.manifests, cancel)?;
                    self.boundary(cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(())
                })();
                rollback(c, operation)
            })();
            bare.finish_writer(result, close)
        })?;
        // Source/family closes have succeeded. No probe occurs on failure.
        self.source_companion_snapshot(data, cancel, |full| {
            managed_destination::valid(self.read_source_protection_scope(
                &self.held.family.catalog,
                cancel,
                |c| {
                    self.held.family.validate_source_companion_catalog(
                        c,
                        &self.held.artifacts.allocation,
                        &SourceCompanionFamilyContext {
                            publication: full,
                            pending: Some(data.protection()),
                        },
                        Some(cancel),
                    )?;
                    if data.protection().inspect(c, Some(cancel))?
                        != SourceCompanionPresence::Protected
                        || validate_protected_installations(
                            c,
                            binding,
                            self.manifests,
                            &c_root,
                            Some(cancel),
                        )? != before
                    {
                        return Err(invalid(
                            "source protection durable complete readback differs",
                        ));
                    }
                    Ok(())
                },
            )?)
        })?;
        let result = ManagedSourceCompanionProtection { data, bytes };
        result.require(self, cancel)?;
        Ok(result)
    }
}
pub(super) fn rollback<T>(c: &Connection, result: CauseResult<T>) -> CauseResult<T> {
    match result {
        Err(primary) if !c.is_autocommit() => match c.execute_batch("ROLLBACK") {
            Ok(()) => Err(primary),
            Err(e) => Err(Cause::Cleanup {
                primary: Box::new(primary),
                close: Box::new(e.into()),
            }),
        },
        other => other,
    }
}
