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
