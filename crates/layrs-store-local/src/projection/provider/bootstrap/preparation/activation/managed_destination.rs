//! Short SQL-only managed destination publication. Both physical owners and
//! the native source remain with the caller on every error. This ends at a
//! copied prefix plus family Initializing, never Ready or ordinary admission.
use super::*;
use crate::projection::{
    provider::{
        capture_lock::CaptureAttemptLock,
        database_io::{self, WriterCloseObserver},
        destination_sql, destination_validation,
        import_validation::{self, ImportDatabaseKind, VerifiedImportClosure},
        records::{
            import::{ImportSource, ImportSourceReferences},
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
            Record,
        },
        validation::{self, PrivateActiveCaptureValidation},
    },
    shared::provider_access::{ImportRegistration, ImportRegistrationEffect},
};
use layrs_vfs::Cancellation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DestinationSqlState {
    FreshCopied,
    EmptyCopied,
    ReplayedCopied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ManagedDestinationSqlEffect {
    pub family: ImportRegistrationEffect,
    pub destination: DestinationSqlState,
}

pub(super) fn valid<T>(state: database_io::ReadState<T>) -> CauseResult<T> {
    match state {
        database_io::ReadState::Valid(value) => Ok(value),
        database_io::ReadState::Uninitialized => Err(invalid("managed database uninitialized")),
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

pub(super) fn boundary(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    attempt: &CaptureAttemptLock,
    copying: &ProviderTransitionIntentRecord,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    cancel: &Cancellation,
) -> CauseResult<()> {
    cancel.check()?;
    attempt.verify(&held.artifacts, &held.family)?;
    super::boundary(held, root)?;
    destination.require_published(&copying.plan)?;
    if copying.destination.as_ref() != Some(destination.proof()) {
        return Err(invalid(
            "managed destination published proof differs from Copying",
        ));
    }
    Ok(())
}

/// Reopen the source only for a bounded SQL step. The issued closure never
/// escapes its source connection. A failed observable close ends the step.
#[allow(clippy::too_many_arguments)]
pub(super) fn with_source<T>(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    copying: &ProviderTransitionIntentRecord,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    cancel: &Cancellation,
    action: impl FnOnce(
        &Connection,
        &PrivateActiveCaptureValidation,
        &VerifiedImportClosure,
    ) -> CauseResult<T>,
) -> CauseResult<T> {
    boundary(held, root, attempt, copying, destination, cancel)?;
    let read = database_io::read_cancellable(&held.artifacts.database, cancel, |c| {
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
        if state.active.mount != *mount
            || state.active.activation != *activation
            || state
                .transitions
                .iter()
                .find(|row| row.transaction_id == copying.transaction_id)
                != Some(copying)
        {
            return Err(invalid("managed source durable Copying differs"));
        }
        let proof = PrivateActiveCaptureValidation {
            active: state.active,
            capture: state.capture,
        };
        let ImportSource::Provider { version, .. } = &copying.plan.source_prefix.source else {
            return Err(invalid("managed source Copying is not provider"));
        };
        let closure = import_validation::verify_source(
            c,
            &held.artifacts.allocation.format,
            ImportDatabaseKind::Provider,
            &copying.plan.source_prefix,
            ImportSourceReferences::Provider {
                allocation: &held.artifacts.allocation,
                prepared: held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("managed source P absent"))?,
                activation: &proof.active.activation,
            },
            Some(&proof),
            &version.effective_image_digest,
            Some(cancel),
        )?;
        boundary(held, root, attempt, copying, destination, cancel)?;
        let value = action(c, &proof, &closure)?;
        boundary(held, root, attempt, copying, destination, cancel)?;
        Ok(value)
    })?;
    valid(read)
}

#[allow(clippy::too_many_arguments)]
fn family_register(
    held: &outcome::PublicationPins,
    copying: &ProviderTransitionIntentRecord,
    proof: &PrivateActiveCaptureValidation,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<ImportRegistrationEffect> {
    let a = &held.artifacts.allocation;
    let p = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("managed source P absent"))?;
    let activation = &proof.active.activation;
    let check = |c: &Connection| {
        held.family.validate_import_initializing(
            c,
            a,
            p,
            activation,
            &proof.capture,
            copying,
            Some(cancel),
        )
    };
    let before = valid(database_io::read_cancellable(
        &held.family.catalog,
        cancel,
        check,
    )?)?;
    if before == ImportRegistration::Initializing {
        return Ok(ImportRegistrationEffect::AlreadyPresent);
    }
    let admission =
        database_io::clean_admission(&held.family.catalog, false, |c| check(c).map(|_| ()))?;
    let bare = admission.open(&held.family.catalog, close)?;
    let result = (|| -> CauseResult<ImportRegistrationEffect> {
        admission.require_geometry(&bare)?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
        if check(c)? != before {
            return Err(invalid("managed family registration changed before write"));
        }
        let effect = held.family.register_import_initializing(
            c,
            a,
            p,
            activation,
            &proof.capture,
            copying,
            Some(cancel),
        )?;
        c.execute_batch("COMMIT")?;
        Ok(effect)
    })();
    let effect = bare.finish_writer(result, close)?;
    let durable = valid(database_io::read_cancellable(
        &held.family.catalog,
        cancel,
        check,
    )?)?;
    if durable != ImportRegistration::Initializing {
        return Err(invalid("managed family Initializing readback differs"));
    }
    Ok(effect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ingress {
    Empty,
    Copied,
}

fn inspect_destination(
    c: &Connection,
    held: &outcome::PublicationPins,
    proof: &PrivateActiveCaptureValidation,
    closure: &VerifiedImportClosure,
    copying: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<Ingress> {
    let marker = &copying
        .destination
        .as_ref()
        .ok_or_else(|| invalid("managed destination absent"))?
        .format;
    let prefix = &copying.plan.source_prefix;
    import_validation::require_schema(c, marker, ImportDatabaseKind::Provider, Some(cancel))?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_head LIMIT 2)",
        [],
        |r| r.get(0),
    )?;
    if count != 1 {
        return Err(invalid("managed destination head cardinality differs"));
    }
    let head: u64 = c.query_row(
        "SELECT generation FROM projection_head WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let generation = match &prefix.source {
        ImportSource::Provider { version, .. } => version.generation,
        _ => return Err(invalid("managed destination source is not provider")),
    };
    let source = &proof.active.source;
    let a = &held.artifacts.allocation;
    let p = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("managed source P absent"))?;
    let activation = &proof.active.activation;
    match head {
        0 => {
            destination_validation::read_empty(
                c,
                marker,
                source,
                a,
                p,
                activation,
                prefix,
                &destination_intent(copying)?,
                Some(cancel),
            )?;
            Ok(Ingress::Empty)
        }
        value if value == generation => {
            destination_validation::read_copied(
                c,
                marker,
                source,
                a,
                p,
                activation,
                prefix,
                &destination_intent(copying)?,
                closure,
                Some(cancel),
            )?;
            Ok(Ingress::Copied)
        }
        _ => Err(invalid(
            "managed destination head outside Empty/Copied boundary",
        )),
    }
}

pub(super) fn destination_intent(
    source: &ProviderTransitionIntentRecord,
) -> CauseResult<ProviderTransitionIntentRecord> {
    let mut intent = ProviderTransitionIntentRecord::decode(&source.encode()?)?;
    intent.role = TransitionRole::Destination;
    intent.local_generation = 0;
    intent.validate_fields()?;
    Ok(intent)
}

#[allow(clippy::too_many_arguments)]
fn copy_destination(
    held: &outcome::PublicationPins,
    copying: &ProviderTransitionIntentRecord,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    source_c: &Connection,
    proof: &PrivateActiveCaptureValidation,
    closure: &VerifiedImportClosure,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<DestinationSqlState> {
    let file = destination.destination_database();
    database_io::validate_native_filename(file.path())?;
    let prior = database_io::read_cancellable(file, cancel, |c| {
        inspect_destination(c, held, proof, closure, copying, cancel)
    })?;
    let ingress = match prior {
        database_io::ReadState::Uninitialized => None,
        other => Some(valid(other)?),
    };
    if ingress == Some(Ingress::Copied) {
        return Ok(DestinationSqlState::ReplayedCopied);
    }
    let fresh = ingress.is_none();
    let admission = database_io::clean_admission(file, fresh, |c| {
        if fresh {
            return Err(invalid(
                "managed destination fresh admission became nonempty",
            ));
        }
        if inspect_destination(c, held, proof, closure, copying, cancel)? != Ingress::Empty {
            return Err(invalid("managed destination EmptyIngress changed"));
        }
        Ok(())
    })?;
    let bare = admission.open(file, close)?;
    let result = (|| -> CauseResult<DestinationSqlState> {
        admission.require_geometry(&bare)?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
        let marker = &copying
            .destination
            .as_ref()
            .ok_or_else(|| invalid("managed destination absent"))?
            .format;
        let prefix = &copying.plan.source_prefix;
        let intent = destination_intent(copying)?;
        let a = &held.artifacts.allocation;
        let p = held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("managed source P absent"))?;
        let activation = &proof.active.activation;
        if fresh {
            destination_sql::initialize_empty(
                c,
                marker,
                &proof.active.source,
                a,
                p,
                activation,
                prefix,
                &intent,
                Some(cancel),
            )?;
        } else if inspect_destination(c, held, proof, closure, copying, cancel)? != Ingress::Empty {
            return Err(invalid(
                "managed destination EmptyIngress changed before copy",
            ));
        }
        destination_sql::promote_or_replay_copied(
            source_c,
            c,
            marker,
            &proof.active.source,
            a,
            p,
            activation,
            prefix,
            &intent,
            closure,
            Some(cancel),
        )?;
        c.execute_batch("COMMIT")?;
        Ok(if fresh {
            DestinationSqlState::FreshCopied
        } else {
            DestinationSqlState::EmptyCopied
        })
    })();
    bare.finish_writer(result, close)
}

/// Borrowed, retained owners ensure every error leaves the exact source and
/// destination capabilities with the caller. No later effect follows an
/// uncertain COMMIT or observable close.
#[allow(clippy::too_many_arguments)]
pub(super) fn register_and_copy(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    attempt: &CaptureAttemptLock,
    copying: &ProviderTransitionIntentRecord,
    destination: &managed_allocation::ManagedDestinationAllocation<'_>,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<ManagedDestinationSqlEffect> {
    if copying.role != TransitionRole::Source
        || copying.state != TransitionState::Copying
        || copying.reservation.is_some()
        || copying.receipt_id.is_some()
        || copying.destination.as_ref() != Some(destination.proof())
    {
        return Err(invalid(
            "managed destination requires exact unreserved source Copying",
        ));
    }
    let _locks = held
        .family
        .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
    boundary(held, root, attempt, copying, destination, cancel)?;
    let family = with_source(
        held,
        root,
        mount,
        activation,
        attempt,
        copying,
        destination,
        cancel,
        |_source_c, proof, _closure| family_register(held, copying, proof, close, cancel),
    )?;
    let destination_state = with_source(
        held,
        root,
        mount,
        activation,
        attempt,
        copying,
        destination,
        cancel,
        |source_c, proof, closure| {
            copy_destination(
                held,
                copying,
                destination,
                source_c,
                proof,
                closure,
                close,
                cancel,
            )
        },
    )?;
    with_source(
        held,
        root,
        mount,
        activation,
        attempt,
        copying,
        destination,
        cancel,
        |_source_c, proof, closure| {
            let catalog = valid(database_io::read_cancellable(
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
            if catalog != ImportRegistration::Initializing {
                return Err(invalid("managed destination family readback differs"));
            }
            let copied = valid(database_io::read_cancellable(
                destination.destination_database(),
                cancel,
                |c| inspect_destination(c, held, proof, closure, copying, cancel),
            )?)?;
            if copied != Ingress::Copied {
                return Err(invalid("managed destination CopiedPrefix readback differs"));
            }
            Ok(())
        },
    )?;
    boundary(held, root, attempt, copying, destination, cancel)?;
    Ok(ManagedDestinationSqlEffect {
        family,
        destination: destination_state,
    })
}
