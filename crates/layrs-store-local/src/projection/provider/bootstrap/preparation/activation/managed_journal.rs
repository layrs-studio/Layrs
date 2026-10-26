//! Durable source intent under the retained Active owner. This layer only
//! records Planned/Cancelled: it creates no destination and authorizes no
//! reservation, installation, protection, or Ready publication.
use super::*;
use crate::projection::provider::{
    capture_lock::CaptureAttemptLock,
    database_io::WriterCloseObserver,
    records::{
        import::ImportSource,
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    transition_sql::{self, SourceIntentEffect},
    validation::PrivateActiveTransitionValidation,
};
use layrs_vfs::Cancellation;

fn valid<T>(state: database_io::ReadState<T>) -> CauseResult<T> {
    match state {
        database_io::ReadState::Valid(value) => Ok(value),
        database_io::ReadState::Uninitialized => {
            Err(invalid("managed source database uninitialized"))
        }
        database_io::ReadState::Hot(hot) => Err(Cause::ReadonlyRollback(hot.trigger)),
        database_io::ReadState::UnboundRollback { trigger, cause } => Err(Cause::Cleanup {
            primary: Box::new(Cause::ReadonlyRollback(trigger)),
            close: Box::new(cause),
        }),
        database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::ReadButUnavailable(cause)
        | database_io::ReadState::Unavailable(cause) => Err(cause),
    }
}

fn private(
    c: &Connection,
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    cancel: &Cancellation,
) -> CauseResult<PrivateActiveTransitionValidation> {
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
        return Err(invalid("managed source retained active map differs"));
    }
    Ok(state)
}

fn family(
    held: &outcome::PublicationPins,
    state: &PrivateActiveTransitionValidation,
    cancel: &Cancellation,
) -> CauseResult<()> {
    let registration = valid(database_io::read_cancellable(
        &held.family.catalog,
        cancel,
        |c| {
            held.family.validate_capture_catalog(
                c,
                &held.artifacts.allocation,
                &state.capture,
                Some(cancel),
            )
        },
    )?)?;
    if registration != ActivationRegistration::Ready {
        return Err(invalid("managed source family owner is not Ready"));
    }
    Ok(())
}

/// Read-only allocation admission. Call before and between fresh filesystem
/// effects; the returned unit retains no locks and grants no destination pins.
pub(super) fn observe_planned(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    planned: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<()> {
    if planned.role != TransitionRole::Source
        || planned.state != TransitionState::Planned
        || planned.destination.is_some()
        || planned.reservation.is_some()
        || planned.receipt_id.is_some()
    {
        return Err(invalid(
            "fresh allocation requires exact unreserved source Planned",
        ));
    }
    let _locks = held.family.locks(
        &held.artifacts.allocation.format.identity.worktree_id,
        false,
    )?;
    cancel.check()?;
    attempt.verify(&held.artifacts, &held.family)?;
    boundary(held, root)?;
    let state = valid(database_io::read_cancellable(
        &held.artifacts.database,
        cancel,
        |c| {
            let state = private(c, held, root, mount, activation, cancel)?;
            require_next(&state, planned)?;
            let proof = validation::PrivateActiveCaptureValidation {
                active: state.active,
                capture: state.capture,
            };
            let ImportSource::Provider { version, .. } = &planned.plan.source_prefix.source else {
                return Err(invalid("source allocation requires provider prefix"));
            };
            crate::projection::provider::import_validation::verify_source(
                c,
                &held.artifacts.allocation.format,
                crate::projection::provider::import_validation::ImportDatabaseKind::Provider,
                &planned.plan.source_prefix,
                crate::projection::provider::records::import::ImportSourceReferences::Provider {
                    allocation: &held.artifacts.allocation,
                    prepared: held
                        .artifacts
                        .prepared
                        .as_ref()
                        .ok_or_else(|| invalid("source allocation P absent"))?,
                    activation: &proof.active.activation,
                },
                Some(&proof),
                &version.effective_image_digest,
                Some(cancel),
            )?;
            Ok(PrivateActiveTransitionValidation {
                active: proof.active,
                capture: proof.capture,
                transitions: state.transitions,
            })
        },
    )?)?;
    require_next(&state, planned)?;
    family(held, &state, cancel)?;
    let registered = valid(database_io::read_cancellable(
        &held.family.catalog,
        cancel,
        |c| {
            let present: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1)",
                [planned
                    .plan
                    .source_prefix
                    .destination_identity
                    .worktree_id
                    .as_str()],
                |row| row.get(0),
            )?;
            Ok(present)
        },
    )?)?;
    if registered {
        return Err(invalid(
            "fresh managed destination worktree is already registered",
        ));
    }
    attempt.verify(&held.artifacts, &held.family)?;
    boundary(held, root)?;
    cancel.check()?;
    Ok(())
}

/// Hash before taking the exclusive owner/catalog locks: immutable reads may
/// acquire those gates through LocalSource. The persisted image digest is a
/// declaration at this boundary, never permission for subsequent byte effects.
/// A destination driver must retain and reverify the image before its effects.
pub(super) fn begin(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    image: &mut managed_image::VerifiedManagedImage<'_, '_>,
    next: &ProviderTransitionIntentRecord,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<SourceIntentEffect> {
    if next.role != TransitionRole::Source
        || next.state != TransitionState::Planned
        || next.destination.is_some()
        || next.reservation.is_some()
        || next.receipt_id.is_some()
    {
        return Err(invalid(
            "managed source begin requires a fresh Planned intent",
        ));
    }
    attempt.verify(&held.artifacts, &held.family)?;
    let ImportSource::Provider { version, .. } = &next.plan.source_prefix.source else {
        return Err(invalid("managed source begin requires provider source"));
    };
    image.revalidate_and_match_digest(&version.effective_image_digest, cancel)?;
    write(
        held,
        root,
        mount,
        activation,
        attempt,
        None,
        next,
        Some(image),
        None,
        close,
        cancel,
    )
}

/// Cancellation retains every acquired fact. It neither releases a reservation
/// nor deletes a destination. Those effects require independent retained proofs.
pub(super) fn cancel(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    previous: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<SourceIntentEffect> {
    if next.state != TransitionState::Cancelled || next.role != TransitionRole::Source {
        return Err(invalid("managed source cancellation phase differs"));
    }
    next.require_transition(previous)?;
    write(
        held,
        root,
        mount,
        activation,
        attempt,
        Some(previous),
        next,
        None,
        None,
        close,
        cancel,
    )
}

/// Persist actual fresh destination identities before FORMAT publication or
/// SQL initialization. This does not register, reserve, protect or publish it.
pub(super) fn bind_allocation(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    previous: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<SourceIntentEffect> {
    if previous.role != TransitionRole::Source
        || previous.state != TransitionState::Planned
        || previous.destination.is_some()
        || previous.reservation.is_some()
        || next.role != TransitionRole::Source
        || next.state != TransitionState::Copying
        || next.reservation.is_some()
        || next.receipt_id.is_some()
        || next.destination.as_ref() != Some(destination.proof())
    {
        return Err(invalid(
            "managed allocation binding requires exact fresh destination",
        ));
    }
    next.require_transition(previous)?;
    destination.verify(&next.plan)?;
    write(
        held,
        root,
        mount,
        activation,
        attempt,
        Some(previous),
        next,
        None,
        Some(destination),
        close,
        cancel,
    )
}

fn write(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    previous: Option<&ProviderTransitionIntentRecord>,
    next: &ProviderTransitionIntentRecord,
    image: Option<&managed_image::VerifiedManagedImage<'_, '_>>,
    destination: Option<&managed_allocation::ManagedDestinationAllocation<'_>>,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<SourceIntentEffect> {
    let _locks = held
        .family
        .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
    let boundary = |state: &PrivateActiveTransitionValidation| -> CauseResult<()> {
        cancel.check()?;
        attempt.verify(&held.artifacts, &held.family)?;
        super::boundary(held, root)?;
        if let Some(image) = image {
            image.require_owner(root, &state.active.source, &state.active.activated.snapshot)?;
        }
        if let Some(destination) = destination {
            destination.verify(&next.plan)?;
            if next.destination.as_ref() != Some(destination.proof()) {
                return Err(invalid("managed allocation proof changed"));
            }
        }
        Ok(())
    };
    let inspect = |c: &Connection| private(c, held, root, mount, activation, cancel);
    let initial = valid(database_io::read_cancellable(
        &held.artifacts.database,
        cancel,
        inspect,
    )?)?;
    boundary(&initial)?;
    family(held, &initial, cancel)?;
    let admission =
        database_io::clean_admission(&held.artifacts.database, false, |c| inspect(c).map(|_| ()))?;
    boundary(&initial)?;
    let bare = admission.open(&held.artifacts.database, close)?;
    let result = (|| -> CauseResult<SourceIntentEffect> {
        admission.require_geometry(&bare)?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        let before = inspect(c)?;
        boundary(&before)?;
        family(held, &before, cancel)?;
        let effect = transition_sql::apply_source_intent(
            c,
            &held.artifacts.allocation,
            held.artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("managed source P absent"))?,
            &held.binding,
            root.registration().root_id(),
            previous,
            next,
            Some(cancel),
        )?;
        let after = inspect(c)?;
        require_next(&after, next)?;
        family(held, &after, cancel)?;
        boundary(&after)?;
        c.execute_batch("COMMIT")?;
        Ok(effect)
    })();
    // Any uncertain commit/close ends this attempt. No readback or subsequent
    // effect is permitted until a separate retry obtains fresh read admission.
    let effect = bare.finish_writer(result, close)?;
    let durable = valid(database_io::read_cancellable(
        &held.artifacts.database,
        cancel,
        inspect,
    )?)?;
    require_next(&durable, next)?;
    family(held, &durable, cancel)?;
    boundary(&durable)?;
    Ok(effect)
}

fn require_next(
    state: &PrivateActiveTransitionValidation,
    next: &ProviderTransitionIntentRecord,
) -> CauseResult<()> {
    if state
        .transitions
        .iter()
        .find(|row| row.transaction_id == next.transaction_id)
        != Some(next)
    {
        return Err(invalid("managed source durable intent differs"));
    }
    Ok(())
}
