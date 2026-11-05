//! Declare a future managed import reservation in source and destination SQL.
//! No family reservation, protected root, installation, Ready state or byte
//! effect is produced. A retained image checkpoint stays with the caller.
use super::*;
use crate::projection::{
    provider::{
        capture_lock::CaptureAttemptLock,
        database_io::{self, WriterCloseObserver},
        destination_sql::{self, CopiedStagingEffect},
        destination_validation,
        import_validation::{self, ImportDatabaseKind, VerifiedImportClosure},
        records::{
            Record,
            import::ImportSource,
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        },
        transition_sql::SourceIntentEffect,
        validation::{self, PrivateActiveCaptureValidation},
    },
    shared::provider_access::ImportRegistration,
};
use layrs_core::scm::FileManifest;
use layrs_vfs::Cancellation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ManagedStagingEffect {
    pub source: SourceIntentEffect,
    pub destination: CopiedStagingEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourcePhase {
    Copying,
    Staging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DestinationPhase {
    Copying,
    Staging,
}

#[allow(clippy::too_many_arguments)]
fn current_source_phase(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<SourcePhase> {
    let state = database_io::read_cancellable(&held.artifacts.database, cancel, |c| {
        let state = validation::private_active_transition_with_source(
            c,
            &held.artifacts.allocation,
            held.artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("managed source P absent"))?,
            &held.binding,
            Some(root.registration().root_id()),
            Some(cancel),
        )?;
        if state.active.mount != *mount || state.active.activation != *activation {
            return Err(invalid("managed staging source Active map differs"));
        }
        match state
            .transitions
            .iter()
            .find(|row| row.transaction_id == copying.transaction_id)
        {
            Some(actual) if actual == copying => Ok(SourcePhase::Copying),
            Some(actual) if actual == staging => Ok(SourcePhase::Staging),
            _ => Err(invalid("managed staging source intent differs")),
        }
    })?;
    managed_destination::valid(state)
}

pub(super) fn destination_intents(
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
) -> CauseResult<(
    ProviderTransitionIntentRecord,
    ProviderTransitionIntentRecord,
)> {
    Ok((
        managed_destination::destination_intent(copying)?,
        managed_destination::destination_intent(staging)?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn family_copying(
    held: &outcome::PublicationPins,
    proof: &PrivateActiveCaptureValidation,
    copying: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<()> {
    let state = managed_destination::valid(database_io::read_cancellable(
        &held.family.catalog,
        cancel,
        |c| {
            held.family.validate_import_initializing(
                c,
                &held.artifacts.allocation,
                held.artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("managed source P absent"))?,
                &proof.active.activation,
                &proof.capture,
                copying,
                Some(cancel),
            )
        },
    )?)?;
    if state != ImportRegistration::Initializing {
        return Err(invalid("managed staging requires family Initializing"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn family_staging(
    held: &outcome::PublicationPins,
    proof: &PrivateActiveCaptureValidation,
    staging: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<()> {
    managed_destination::valid(database_io::read_cancellable(
        &held.family.catalog,
        cancel,
        |c| {
            held.family.validate_import_staging_unreserved(
                c,
                &held.artifacts.allocation,
                held.artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("managed source P absent"))?,
                &proof.active.activation,
                &proof.capture,
                staging,
                Some(cancel),
            )
        },
    )?)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_destination(
    c: &Connection,
    held: &outcome::PublicationPins,
    proof: &PrivateActiveCaptureValidation,
    closure: &VerifiedImportClosure,
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<DestinationPhase> {
    let (dest_copying, dest_staging) = destination_intents(copying, staging)?;
    let marker = &copying
        .destination
        .as_ref()
        .ok_or_else(|| invalid("managed destination absent"))?
        .format;
    import_validation::require_schema(c, marker, ImportDatabaseKind::Provider, Some(cancel))?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 2)",
        [],
        |row| row.get(0),
    )?;
    if count != 1 {
        return Err(invalid("managed staging destination intent count differs"));
    }
    let (copying_row, staging_row): (bool, bool) = c.query_row(
        "SELECT state='copying',state='staging' FROM projection_provider_transitions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let a = &held.artifacts.allocation;
    let p = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("managed source P absent"))?;
    let source = &proof.active.source;
    let activation = &proof.active.activation;
    let prefix = &copying.plan.source_prefix;
    if copying_row {
        destination_validation::read_copied(
            c,
            marker,
            source,
            a,
            p,
            activation,
            prefix,
            &dest_copying,
            closure,
            Some(cancel),
        )?;
        Ok(DestinationPhase::Copying)
    } else if staging_row {
        destination_validation::read_staging(
            c,
            marker,
            source,
            a,
            p,
            activation,
            prefix,
            &dest_staging,
            closure,
            Some(cancel),
        )?;
        Ok(DestinationPhase::Staging)
    } else {
        Err(invalid("managed staging destination phase differs"))
    }
}

#[allow(clippy::too_many_arguments)]
fn destination_stage(
    held: &outcome::PublicationPins,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    source_c: &Connection,
    proof: &PrivateActiveCaptureValidation,
    closure: &VerifiedImportClosure,
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<CopiedStagingEffect> {
    family_staging(held, proof, staging, cancel)?;
    let file = destination.destination_database();
    database_io::validate_native_filename(file.path())?;
    let prior = managed_destination::valid(database_io::read_cancellable(file, cancel, |c| {
        read_destination(c, held, proof, closure, copying, staging, cancel)
    })?)?;
    if prior == DestinationPhase::Staging {
        return Ok(CopiedStagingEffect::AlreadyPresent);
    }
    let admission = database_io::clean_admission(file, false, |c| {
        if read_destination(c, held, proof, closure, copying, staging, cancel)?
            != DestinationPhase::Copying
        {
            return Err(invalid("managed staging destination predecessor changed"));
        }
        Ok(())
    })?;
    let bare = admission.open(file, close)?;
    let result = (|| -> CauseResult<CopiedStagingEffect> {
        admission.require_geometry(&bare)?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
        family_staging(held, proof, staging, cancel)?;
        if read_destination(c, held, proof, closure, copying, staging, cancel)?
            != DestinationPhase::Copying
        {
            return Err(invalid("managed staging destination changed before CAS"));
        }
        let (dest_copying, dest_staging) = destination_intents(copying, staging)?;
        let marker = &copying
            .destination
            .as_ref()
            .ok_or_else(|| invalid("managed destination absent"))?
            .format;
        let effect = destination_sql::stage_copied(
            source_c,
            c,
            marker,
            &proof.active.source,
            &held.artifacts.allocation,
            held.artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("managed source P absent"))?,
            &proof.active.activation,
            &copying.plan.source_prefix,
            &dest_copying,
            &dest_staging,
            closure,
            Some(cancel),
        )?;
        c.execute_batch("COMMIT")?;
        Ok(effect)
    })();
    let result = managed_destination::rollback(bare.connection(), result);
    let effect = bare.finish_writer(result, close)?;
    let durable = managed_destination::valid(database_io::read_cancellable(file, cancel, |c| {
        read_destination(c, held, proof, closure, copying, staging, cancel)
    })?)?;
    if durable != DestinationPhase::Staging {
        return Err(invalid(
            "managed staging destination durable readback differs",
        ));
    }
    Ok(effect)
}

/// Source Copying/Destination Copied is the only first-write predecessor.
/// Source Staging/Destination Copying is the expected crash gap; both Staging
/// rows replay without writes. Source Copying/Destination Staging is refused.
#[allow(clippy::too_many_arguments)]
pub(super) fn stage(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    image: &mut managed_image::VerifiedManagedImage<'_, '_>,
    manifests: &[FileManifest],
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<ManagedStagingEffect> {
    managed_journal::require_legacy_allocation(copying)?;
    managed_journal::require_legacy_allocation(staging)?;
    staging.require_transition(copying)?;
    if copying.role != TransitionRole::Source
        || copying.state != TransitionState::Copying
        || staging.state != TransitionState::Staging
        || staging.reservation.as_deref() != Some(staging.reservation_operation()?.as_str())
        || copying.destination.as_ref() != Some(destination.proof())
        || staging.destination.as_ref() != Some(destination.proof())
    {
        return Err(invalid("managed staging source plan/proof differs"));
    }
    let ImportSource::Provider { version, .. } = &copying.plan.source_prefix.source else {
        return Err(invalid("managed staging requires provider source"));
    };
    image.revalidate_and_match_digest(&version.effective_image_digest, cancel)?;
    managed_plan::require_manifest_union(&copying.plan, manifests)?;
    let phase = {
        let _locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        managed_destination::boundary(held, root, attempt, copying, destination, cancel)?;
        current_source_phase(held, root, mount, activation, copying, staging, cancel)?
    };
    let source_effect = match phase {
        SourcePhase::Copying => {
            // The preflight owns short exclusive gates but reads no image bytes.
            {
                let _locks = held
                    .family
                    .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
                managed_destination::with_source(
                    held,
                    root,
                    mount,
                    activation,
                    attempt,
                    copying,
                    destination,
                    cancel,
                    |_c, proof, closure| {
                        image.require_owner(
                            root,
                            &proof.active.source,
                            &proof.active.activated.snapshot,
                        )?;
                        family_copying(held, proof, copying, cancel)?;
                        let copied = managed_destination::valid(database_io::read_cancellable(
                            destination.destination_database(),
                            cancel,
                            |c| read_destination(c, held, proof, closure, copying, staging, cancel),
                        )?)?;
                        if copied != DestinationPhase::Copying {
                            return Err(invalid(
                                "source Copying requires destination CopiedPrefix",
                            ));
                        }
                        Ok(())
                    },
                )?;
            }
            managed_journal::stage_source(
                held,
                root,
                mount,
                activation,
                attempt,
                copying,
                staging,
                image,
                destination,
                close,
                cancel,
            )?
        }
        SourcePhase::Staging => SourceIntentEffect::AlreadyPresent,
    };
    let destination_effect = {
        let _locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        managed_destination::with_source(
            held,
            root,
            mount,
            activation,
            attempt,
            staging,
            destination,
            cancel,
            |source_c, proof, closure| {
                image.require_owner(
                    root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                destination_stage(
                    held,
                    destination,
                    source_c,
                    proof,
                    closure,
                    copying,
                    staging,
                    close,
                    cancel,
                )
            },
        )?
    };
    {
        let _locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        managed_destination::with_source(
            held,
            root,
            mount,
            activation,
            attempt,
            staging,
            destination,
            cancel,
            |_c, proof, closure| {
                image.require_owner(
                    root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                family_staging(held, proof, staging, cancel)?;
                let current = managed_destination::valid(database_io::read_cancellable(
                    destination.destination_database(),
                    cancel,
                    |c| read_destination(c, held, proof, closure, copying, staging, cancel),
                )?)?;
                if current != DestinationPhase::Staging {
                    return Err(invalid("managed staging final destination differs"));
                }
                Ok(())
            },
        )?;
    }
    managed_destination::boundary(held, root, attempt, staging, destination, cancel)?;
    Ok(ManagedStagingEffect {
        source: source_effect,
        destination: destination_effect,
    })
}

/// Complete-checkpoint counterpart. Legacy stage retains its strict image guard.
pub(super) fn stage_complete(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    image: &mut managed_image::complete::VerifiedCompleteManagedImage<'_, '_>,
    manifests: &[FileManifest],
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<ManagedStagingEffect> {
    managed_journal::require_legacy_allocation(copying)?;
    managed_journal::require_legacy_allocation(staging)?;
    staging.require_transition(copying)?;
    if copying.role != TransitionRole::Source
        || copying.state != TransitionState::Copying
        || staging.state != TransitionState::Staging
        || staging.reservation.as_deref() != Some(staging.reservation_operation()?.as_str())
        || copying.destination.as_ref() != Some(destination.proof())
        || staging.destination.as_ref() != Some(destination.proof())
    {
        return Err(invalid("managed staging source plan/proof differs"));
    }
    let ImportSource::Provider { version, .. } = &copying.plan.source_prefix.source else {
        return Err(invalid("managed staging requires provider source"));
    };
    image.revalidate(cancel)?;
    image.require_retained(root, &version.effective_image_digest, cancel)?;
    managed_plan::require_manifest_union(&copying.plan, manifests)?;
    let phase = {
        let _locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        managed_destination::boundary(held, root, attempt, copying, destination, cancel)?;
        current_source_phase(held, root, mount, activation, copying, staging, cancel)?
    };
    let source_effect = match phase {
        SourcePhase::Copying => {
            // The preflight owns short exclusive gates but reads no image bytes.
            {
                let _locks = held
                    .family
                    .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
                managed_destination::with_source(
                    held,
                    root,
                    mount,
                    activation,
                    attempt,
                    copying,
                    destination,
                    cancel,
                    |_c, proof, closure| {
                        image.require_owner(
                            root,
                            &proof.active.source,
                            &proof.active.activated.snapshot,
                        )?;
                        family_copying(held, proof, copying, cancel)?;
                        let copied = managed_destination::valid(database_io::read_cancellable(
                            destination.destination_database(),
                            cancel,
                            |c| read_destination(c, held, proof, closure, copying, staging, cancel),
                        )?)?;
                        if copied != DestinationPhase::Copying {
                            return Err(invalid(
                                "source Copying requires destination CopiedPrefix",
                            ));
                        }
                        Ok(())
                    },
                )?;
            }
            managed_journal::stage_source(
                held,
                root,
                mount,
                activation,
                attempt,
                copying,
                staging,
                image.retained(),
                destination,
                close,
                cancel,
            )?
        }
        SourcePhase::Staging => SourceIntentEffect::AlreadyPresent,
    };
    let destination_effect = {
        let _locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        managed_destination::with_source(
            held,
            root,
            mount,
            activation,
            attempt,
            staging,
            destination,
            cancel,
            |source_c, proof, closure| {
                image.require_owner(
                    root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                destination_stage(
                    held,
                    destination,
                    source_c,
                    proof,
                    closure,
                    copying,
                    staging,
                    close,
                    cancel,
                )
            },
        )?
    };
    {
        let _locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        managed_destination::with_source(
            held,
            root,
            mount,
            activation,
            attempt,
            staging,
            destination,
            cancel,
            |_c, proof, closure| {
                image.require_owner(
                    root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                family_staging(held, proof, staging, cancel)?;
                let current = managed_destination::valid(database_io::read_cancellable(
                    destination.destination_database(),
                    cancel,
                    |c| read_destination(c, held, proof, closure, copying, staging, cancel),
                )?)?;
                if current != DestinationPhase::Staging {
                    return Err(invalid("managed staging final destination differs"));
                }
                Ok(())
            },
        )?;
    }
    managed_destination::boundary(held, root, attempt, staging, destination, cancel)?;
    Ok(ManagedStagingEffect {
        source: source_effect,
        destination: destination_effect,
    })
}
