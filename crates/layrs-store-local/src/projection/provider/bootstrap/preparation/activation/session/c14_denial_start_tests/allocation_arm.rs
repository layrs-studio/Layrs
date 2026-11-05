//! Allocation-mode arm of `run_start_configuration`: denial allocation, copying, destination registration and FORMAT publication. Borrows the caller's state and consumes the pending start, which holds the
//! image; the caller then checks the retained preimage when told to and tears
//! the owners down.
use super::super::super::{
    managed_image::complete::VerifiedCompleteManagedImage,
    managed_journal::denial_start::PendingDenialStart,
};
use super::*;
use crate::projection::provider::records::{
    AllocationRecord, PreparedRecord, ProviderBootstrapBinding,
};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    cancel: &Cancellation,
    f: &Fixture,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    session: &ProviderActiveSession,
    attempt: &CaptureAttemptLock,
    range_watch: &RangeWatch,
    planned: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    private_path: &Path,
    family_path: &Path,
    mut pending: PendingDenialStart<'_, '_, '_, '_>,
    case: AllocationCase,
) -> bool {
    let held = &session.source.held;
    use super::super::super::managed_allocation::{ManagedFormatState, ManagedFormatStep};
    use crate::projection::provider::records::transition::TransitionState;
    let family_before = database_io::tests::support::witness(&family_path);
    let catalog_before = catalog_snapshot(&f, &cancel);
    let read_source = || {
        super::super::super::managed_destination::valid(
            database_io::read_cancellable_strict(&held.artifacts.database, &cancel, table_snapshot)
                .unwrap(),
        )
        .unwrap()
    };
    let source_before = read_source();
    let private_before = database_io::tests::support::witness(&private_path);
    if matches!(
        case,
        AllocationCase::AlteredPlanned
            | AllocationCase::LegacyRoutes
            | AllocationCase::ImageChanged
    ) {
        let mut candidate =
            ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
        if matches!(case, AllocationCase::AlteredPlanned) {
            candidate.plan.destination_leaf_nonce = "ffffffffffffffffffffffffffffffff".into();
            assert_ne!(candidate.plan, planned.plan);
            candidate.encode().unwrap();
        }
        if matches!(case, AllocationCase::ImageChanged) {
            fs::write(
                f.checkout.join("c14-after-planned.txt"),
                b"external namespace change after Planned",
            )
            .unwrap();
        }
        let target = f.outer.join(&candidate.plan.destination_leaf_nonce);
        let error = if matches!(case, AllocationCase::LegacyRoutes) {
            super::super::super::managed_allocation::allocate(
                &session.session,
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                &attempt,
                &candidate,
                &cancel,
            )
        } else {
            super::super::super::managed_allocation::allocate_denial(
                &session.session,
                held,
                &mut pending,
                &candidate,
                &cancel,
            )
        }
        .err()
        .expect("negative allocation guard created C");
        assert!(error.effects.is_empty());
        assert_eq!(error.attempted_root, target);
        match case {
            AllocationCase::ImageChanged => {
                assert!(
                    matches!(&error.cause, Cause::Vfs(layrs_vfs::Error::Busy)),
                    "{}",
                    error.cause
                )
            }
            AllocationCase::LegacyRoutes => require_validation_message_ref(
                &error.cause,
                "managed allocation requires a denial-aware driver",
            ),
            _ => require_validation_message_ref(
                &error.cause,
                "denial allocation owner or immutable plan differs",
            ),
        }
        assert!(!target.exists());
        assert!(!f.outer.join(&planned.plan.destination_leaf_nonce).exists());
        assert_eq!(read_source(), source_before);
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_eq!(catalog_snapshot(&f, &cancel), catalog_before);
        drop(error);
        if matches!(case, AllocationCase::ImageChanged) {
            drop(pending);
            assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
            return false;
        }
    }
    let mut destination = super::super::super::managed_allocation::allocate_denial(
        &session.session,
        held,
        &mut pending,
        &planned,
        &cancel,
    )
    .unwrap_or_else(|error| panic!("denial allocation failed: {error:?}"));
    destination.verify(&planned.plan).unwrap();
    assert_eq!(
        destination.root(),
        f.outer.join(&planned.plan.destination_leaf_nonce)
    );
    let proof_digest = destination.proof().digest(&planned.plan).unwrap();
    let temporary = destination.root().join(".layrs").join(format!(
        "FORMAT.pending-{}",
        planned.plan.destination_leaf_nonce,
    ));
    let final_format = destination.root().join(".layrs/FORMAT");
    assert!(!final_format.exists());
    assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
    assert_eq!(
        fs::metadata(destination.destination_database().path())
            .unwrap()
            .len(),
        0
    );
    let destination_before =
        database_io::tests::support::witness(destination.destination_database().path());
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );

    let mut bind_close = Close::default();
    let (copying, effect) = if matches!(case, AllocationCase::WithheldCopyingAck) {
        let refused = Rc::new(Cell::new(false));
        let probes = Rc::new(Cell::new(0));
        bind_close.refused = Some(refused.clone());
        let watch_refused = refused.clone();
        let watch_probes = probes.clone();
        let error = with_observer(
            move |_, _| {
                if watch_refused.get() {
                    watch_probes.set(watch_probes.get() + 1);
                    return Err(invalid("forbidden probe after SourceCopying ACK"));
                }
                Ok(())
            },
            || pending.bind_denial_allocation(&destination, &mut bind_close, &cancel),
        )
        .unwrap_err();
        require_validation_message(error, "C14 start SQLite close ACK withheld");
        assert!(refused.get());
        assert_eq!(probes.get(), 0);
        assert_eq!((bind_close.opened, bind_close.closed), (1, 1));
        assert!(
            !final_format.exists(),
            "failed Copying ACK published FORMAT"
        );
        assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_eq!(
            database_io::tests::support::witness(destination.destination_database().path()),
            destination_before
        );
        let before_retry = database_io::tests::support::witness(&private_path);
        let mut retry_close = Close::default();
        let result = pending
            .bind_denial_allocation(&destination, &mut retry_close, &cancel)
            .unwrap();
        assert_eq!((retry_close.opened, retry_close.closed), (0, 0));
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            before_retry
        );
        assert_eq!(result.1, SourceIntentEffect::AlreadyPresent);
        result
    } else {
        let result = pending
            .bind_denial_allocation(&destination, &mut bind_close, &cancel)
            .unwrap();
        assert_eq!((bind_close.opened, bind_close.closed), (1, 1));
        assert_eq!(result.1, SourceIntentEffect::Updated);
        result
    };
    assert_eq!(
        effect,
        if matches!(case, AllocationCase::WithheldCopyingAck) {
            SourceIntentEffect::AlreadyPresent
        } else {
            SourceIntentEffect::Updated
        }
    );
    assert_eq!(copying.state, TransitionState::Copying);
    assert_eq!(copying.plan, planned.plan);
    assert_eq!(copying.transaction_id, planned.transaction_id);
    assert_eq!(copying.destination.as_ref(), Some(destination.proof()));
    copying.require_transition(&planned).unwrap();
    destination.verify(&copying.plan).unwrap();
    assert!(!final_format.exists());
    assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
    let source_after = read_source();
    let mut unchanged_before = source_before;
    let mut unchanged_after = source_after.clone();
    assert_eq!(
        unchanged_before
            .remove("projection_provider_transitions")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        unchanged_after
            .remove("projection_provider_transitions")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        unchanged_after, unchanged_before,
        "Copying changed DATA outside its exact intent row"
    );
    let private_before = database_io::tests::support::witness(&private_path);
    let mut replay_close = Close::default();
    let (replay_copying, replay_effect) = pending
        .bind_denial_allocation(&destination, &mut replay_close, &cancel)
        .unwrap();
    assert_eq!(replay_copying, copying);
    assert_eq!(replay_effect, SourceIntentEffect::AlreadyPresent);
    assert_eq!((replay_close.opened, replay_close.closed), (0, 0));
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );

    if matches!(
        case,
        AllocationCase::LegacyRoutes | AllocationCase::AlteredCopying
    ) {
        let before = database_io::tests::support::witness(&private_path);
        let temporary_before = database_io::tests::support::witness(&temporary);
        let error = if matches!(case, AllocationCase::LegacyRoutes) {
            destination
                .publish_format(
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    &attempt,
                    &copying,
                    &cancel,
                )
                .unwrap_err()
        } else {
            let mut altered =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            let digest = &mut altered.destination.as_mut().unwrap().family_marker_digest;
            *digest = format!(
                "blake3:{}",
                if digest.ends_with('0') { "1" } else { "0" }.repeat(64)
            );
            altered.encode().unwrap();
            let error = pending
                .observe_copying_allocation(held, &altered, &destination, &cancel)
                .unwrap_err();
            require_validation_message(
                error,
                "denial FORMAT requires exact retained Copying proof",
            );
            destination
                .publish_format_denial(&mut pending, &altered, &cancel)
                .unwrap_err()
        };
        assert!(error.effects.is_empty());
        if matches!(case, AllocationCase::LegacyRoutes) {
            require_denial_guard(error.cause);
        } else {
            require_validation_message(
                error.cause,
                "managed FORMAT publication requires exact source Copying",
            );
        }
        assert!(!final_format.exists());
        assert_eq!(
            database_io::tests::support::witness(&temporary),
            temporary_before
        );
        assert_eq!(
            database_io::tests::support::witness(destination.destination_database().path()),
            destination_before
        );
        assert_eq!(database_io::tests::support::witness(&private_path), before);
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        destination.verify(&copying.plan).unwrap();
    }
    let publication = destination
        .publish_format_denial(&mut pending, &copying, &cancel)
        .unwrap();
    assert_eq!(publication.state, ManagedFormatState::Published);
    assert!(
        publication
            .effects
            .iter()
            .any(|effect| effect.step == ManagedFormatStep::Rename && effect.confirmed)
    );
    destination.require_published(&copying.plan).unwrap();
    assert!(!temporary.exists());
    assert_eq!(
        fs::read(&final_format).unwrap(),
        destination.proof().format.bytes().unwrap()
    );
    assert_eq!(
        destination.proof().digest(&copying.plan).unwrap(),
        proof_digest
    );
    let format_before = database_io::tests::support::witness(&final_format);
    let replay = destination
        .publish_format_denial(&mut pending, &copying, &cancel)
        .unwrap();
    assert_eq!(replay.state, ManagedFormatState::AlreadyPublished);
    assert!(!replay.effects.iter().any(|effect| matches!(
        effect.step,
        ManagedFormatStep::Append | ManagedFormatStep::Rename
    )));
    destination.require_published(&copying.plan).unwrap();
    destination.verify(&copying.plan).unwrap();
    assert_eq!(
        database_io::tests::support::witness(&final_format),
        format_before
    );
    assert_eq!(
        read_source(),
        source_after,
        "FORMAT publication changed source body or intent"
    );
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );
    assert_eq!(catalog_snapshot(&f, &cancel), catalog_before);
    let destination_owner = copying
        .plan
        .source_prefix
        .destination_identity
        .worktree_id
        .as_str();
    let registered = super::super::super::managed_destination::valid(
        database_io::read_cancellable_strict(&held.family.catalog, &cancel, |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM worktrees WHERE id=?1",
                [destination_owner],
                |row| row.get::<_, u64>(0),
            )?)
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        registered, 0,
        "allocation/FORMAT registered destination family state"
    );
    assert_eq!(
        fs::metadata(destination.destination_database().path())
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        database_io::tests::support::witness(destination.destination_database().path()),
        destination_before
    );
    super::super::super::managed_destination::valid(
        database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
            let state = validation::private_active_transition_with_source(
                c,
                &allocation,
                &prepared,
                &binding,
                Some(session.source.root.registration().root_id()),
                Some(&cancel),
            )?;
            assert_eq!(state.transitions.as_slice(), std::slice::from_ref(&copying));
            let actual = crate::projection::provider::denied_coverage_storage::read_required(
                c,
                copying.plan.denial_coverage.as_ref().unwrap(),
                Some(&cancel),
            )?;
            assert_eq!(&actual, coverage.record());
            Ok(())
        })
        .unwrap(),
    )
    .unwrap();
    if let AllocationCase::Stage(stage_case) = case {
        let mut close = Close::default();
        let copied = super::super::super::managed_destination::register_and_copy_denial(
            held,
            &mut pending,
            &copying,
            &destination,
            &mut close,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            copied.family,
            crate::projection::shared::provider_access::ImportRegistrationEffect::Inserted
        );
        assert_eq!(
            copied.destination,
            super::super::super::managed_destination::DestinationSqlState::FreshCopied
        );
        assert_eq!((close.opened, close.closed), (2, 2));
        require_denial_copy_state(
            held,
            &pending,
            &copying,
            &destination,
            &coverage,
            true,
            &cancel,
        );
        staging::run(
            &f,
            held,
            &session.source.root,
            &mut pending,
            &copying,
            &destination,
            &coverage,
            stage_case,
            &cancel,
        );
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
        drop(destination);
        drop(pending);
        return true;
    }
    if let AllocationCase::Copy(copy_case) = case {
        use super::super::super::managed_destination::DestinationSqlState;
        use crate::projection::shared::provider_access::ImportRegistrationEffect;
        let family_before_copy = database_io::tests::support::witness(&family_path);
        let catalog_before_copy = catalog_snapshot(&f, &cancel);
        let source_before_copy = read_source();
        let private_before_copy = database_io::tests::support::witness(&private_path);
        let destination_before_copy =
            database_io::tests::support::witness(destination.destination_database().path());
        // This source is genuinely Copying and FORMAT is published, so an
        // absent predecessor cannot satisfy the legacy-route assertion.
        let mut legacy_close = Close::default();
        require_denial_guard(
            super::super::super::managed_destination::register_and_copy(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                &attempt,
                &copying,
                &destination,
                &mut legacy_close,
                &cancel,
            )
            .unwrap_err(),
        );
        assert_eq!((legacy_close.opened, legacy_close.closed), (0, 0));
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before_copy
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before_copy
        );
        assert_eq!(
            database_io::tests::support::witness(destination.destination_database().path()),
            destination_before_copy
        );

        let mut copy_close = Close::default();
        let result = if matches!(copy_case, CopyCase::Publish) {
            let effect = super::super::super::managed_destination::register_and_copy_denial(
                held,
                &mut pending,
                &copying,
                &destination,
                &mut copy_close,
                &cancel,
            )
            .unwrap();
            assert_eq!((copy_close.opened, copy_close.closed), (2, 2));
            assert_eq!(effect.family, ImportRegistrationEffect::Inserted);
            assert_eq!(effect.destination, DestinationSqlState::FreshCopied);
            effect
        } else {
            let refused = Rc::new(Cell::new(false));
            let probes = Rc::new(Cell::new(0));
            copy_close.refused = Some(refused.clone());
            let ordinal = if matches!(copy_case, CopyCase::WithheldFamilyAck) {
                1
            } else {
                2
            };
            copy_close.refuse_at = Some(ordinal);
            let watch_refused = refused.clone();
            let watch_probes = probes.clone();
            let error = with_observer(
                move |_, _| {
                    if watch_refused.get() {
                        watch_probes.set(watch_probes.get() + 1);
                        return Err(invalid("forbidden probe after denial C copy ACK"));
                    }
                    Ok(())
                },
                || {
                    super::super::super::managed_destination::register_and_copy_denial(
                        held,
                        &mut pending,
                        &copying,
                        &destination,
                        &mut copy_close,
                        &cancel,
                    )
                },
            )
            .unwrap_err();
            require_validation_message(error, "C14 start SQLite close ACK withheld");
            assert!(refused.get());
            assert_eq!(probes.get(), 0);
            assert_eq!((copy_close.opened, copy_close.closed), (ordinal, ordinal));
            let copied = matches!(copy_case, CopyCase::WithheldDestinationAck);
            require_denial_copy_state(
                held,
                &pending,
                &copying,
                &destination,
                &coverage,
                copied,
                &cancel,
            );
            if !copied {
                assert_eq!(
                    database_io::tests::support::witness(destination.destination_database().path()),
                    destination_before_copy,
                    "family ACK failure wrote C SQLite"
                );
            }
            let family_before_retry = database_io::tests::support::witness(&family_path);
            let destination_before_retry =
                database_io::tests::support::witness(destination.destination_database().path());
            let mut retry_close = Close::default();
            let effect = super::super::super::managed_destination::register_and_copy_denial(
                held,
                &mut pending,
                &copying,
                &destination,
                &mut retry_close,
                &cancel,
            )
            .unwrap();
            assert_eq!(effect.family, ImportRegistrationEffect::AlreadyPresent);
            assert_eq!(
                database_io::tests::support::witness(&family_path),
                family_before_retry,
                "copy retry reopened family writer"
            );
            if copied {
                assert_eq!((retry_close.opened, retry_close.closed), (0, 0));
                assert_eq!(effect.destination, DestinationSqlState::ReplayedCopied);
                assert_eq!(
                    database_io::tests::support::witness(destination.destination_database().path()),
                    destination_before_retry
                );
            } else {
                assert_eq!((retry_close.opened, retry_close.closed), (1, 1));
                assert_eq!(effect.destination, DestinationSqlState::FreshCopied);
            }
            effect
        };
        assert!(matches!(
            result.destination,
            DestinationSqlState::FreshCopied | DestinationSqlState::ReplayedCopied
        ));
        require_denial_copy_state(
            held,
            &pending,
            &copying,
            &destination,
            &coverage,
            true,
            &cancel,
        );
        let family_before_replay = database_io::tests::support::witness(&family_path);
        let destination_before_replay =
            database_io::tests::support::witness(destination.destination_database().path());
        let mut replay_close = Close::default();
        let replay = super::super::super::managed_destination::register_and_copy_denial(
            held,
            &mut pending,
            &copying,
            &destination,
            &mut replay_close,
            &cancel,
        )
        .unwrap();
        assert_eq!(replay.family, ImportRegistrationEffect::AlreadyPresent);
        assert_eq!(replay.destination, DestinationSqlState::ReplayedCopied);
        assert_eq!((replay_close.opened, replay_close.closed), (0, 0));
        require_denial_copy_state(
            held,
            &pending,
            &copying,
            &destination,
            &coverage,
            true,
            &cancel,
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before_replay
        );
        assert_eq!(
            database_io::tests::support::witness(destination.destination_database().path()),
            destination_before_replay
        );
        assert_eq!(
            read_source(),
            source_before_copy,
            "C copy changed source intent or current denial body"
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before_copy
        );
        let catalog_after_copy = catalog_snapshot(&f, &cancel);
        assert_eq!(
            catalog_after_copy.get("reservations"),
            catalog_before_copy.get("reservations"),
            "registration or C copy changed B funding"
        );
        destination.require_published(&copying.plan).unwrap();
        assert_eq!(
            database_io::tests::support::witness(&final_format),
            format_before
        );
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
    drop(destination);
    drop(pending);
    true
}
