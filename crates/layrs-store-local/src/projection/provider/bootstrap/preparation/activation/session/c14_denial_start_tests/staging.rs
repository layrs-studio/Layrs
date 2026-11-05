//! Same-owner native staging after real begin/allocation/FORMAT/Copied writers.
//! Only the named negative cases corrupt a fresh fixture, never a success path.
use super::super::super::{
    managed_allocation::ManagedDestinationAllocation,
    managed_journal::denial_start::PendingDenialStart, managed_staging,
};
use super::*;
use crate::projection::{
    provider::{
        bootstrap::outcome::PublicationPins,
        denied_coverage_storage,
        destination_sql::{self, CopiedStagingEffect},
        destination_validation,
        records::{
            denied_coverage::TransitionDeniedCoverageRecord, import::ImportSource,
            transition::TransitionState,
        },
    },
    shared::source_denial::SourceDeniedCoverageReservation,
    SharedQuota,
};
use rusqlite::{params, Connection};

#[derive(Clone, Copy)]
pub(super) enum Case {
    Success,
    SourceAck,
    DestinationAck,
    Capacity,
    DestinationAhead,
    MissingBody,
    SubstitutedBody,
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    source_file: (String, u64, String),
    destination_file: (String, u64, String),
    family_file: (String, u64, String),
    format_file: (String, u64, String),
    source: CatalogSnapshot,
    destination: CatalogSnapshot,
    family: CatalogSnapshot,
}
impl Snapshot {
    fn read(
        f: &Fixture,
        held: &PublicationPins,
        destination: &ManagedDestinationAllocation<'_>,
        cancel: &Cancellation,
    ) -> Self {
        let read = |file: &database_io::DatabaseFile| {
            super::super::super::managed_destination::valid(
                database_io::read_cancellable_strict(file, cancel, table_snapshot).unwrap(),
            )
            .unwrap()
        };
        Self {
            source_file: database_io::tests::support::witness(held.artifacts.database.path()),
            destination_file: database_io::tests::support::witness(
                destination.destination_database().path(),
            ),
            family_file: database_io::tests::support::witness(
                &f.family.root().join("metadata/catalog.sqlite3"),
            ),
            format_file: database_io::tests::support::witness(
                &destination.root().join(".layrs/FORMAT"),
            ),
            source: read(&held.artifacts.database),
            destination: read(destination.destination_database()),
            family: catalog_snapshot(f, cancel),
        }
    }
    fn require_only_intent_changes(&self, after: &Self) {
        let without_intent = |tables: &CatalogSnapshot| {
            let mut rows = tables.clone();
            assert_eq!(
                rows.remove("projection_provider_transitions")
                    .unwrap()
                    .len(),
                1
            );
            rows
        };
        assert_eq!(without_intent(&self.source), without_intent(&after.source));
        assert_eq!(
            without_intent(&self.destination),
            without_intent(&after.destination)
        );
        assert_eq!(self.source_file.0, after.source_file.0);
        assert_eq!(self.destination_file.0, after.destination_file.0);
        assert_eq!(self.family_file, after.family_file);
        assert_eq!(self.family, after.family);
        assert_eq!(self.format_file, after.format_file);
    }
}

#[allow(clippy::too_many_arguments)]
fn require_state(
    held: &PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
    destination: &ManagedDestinationAllocation<'_>,
    coverage: &DerivedDeniedCoverage,
    source_staging: bool,
    destination_staging: bool,
    cancel: &Cancellation,
) {
    let _locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .unwrap();
    super::super::super::managed_destination::valid(database_io::read_cancellable_strict(
        &held.artifacts.database, cancel, |source| {
            let a = &held.artifacts.allocation;
            let p = held.artifacts.prepared.as_ref().unwrap();
            let expected = if source_staging { staging } else { copying };
            let full = validation::private_active_transition_with_source(source, a, p, &held.binding,
                Some(root.registration().root_id()), Some(cancel))?;
            assert_eq!(full.transitions.as_slice(), std::slice::from_ref(expected));
            let funding = SourceDeniedCoverageReservation::new(a, copying, coverage.record(), Some(cancel))?;
            assert_eq!(full.capture.denial_reservations(), std::slice::from_ref(&funding));
            let proof = PrivateActiveCaptureValidation { active: full.active, capture: full.capture };
            let prefix = &copying.plan.source_prefix;
            let ImportSource::Provider { version, .. } = &prefix.source else { panic!("not provider"); };
            let closure = import_validation::verify_source(source, &a.format, ImportDatabaseKind::Provider,
                prefix, ImportSourceReferences::Provider { allocation: a, prepared: p, activation: &proof.active.activation },
                Some(&proof), &version.effective_image_digest, Some(cancel))?;
            let body = denied_coverage_storage::read_required(source, expected.plan.denial_coverage.as_ref().unwrap(), Some(cancel))?;
            assert_eq!(&body, coverage.record());
            body.require_intent(expected, Some(cancel))?;
            super::super::super::managed_destination::valid(database_io::read_cancellable_strict(
                &held.family.catalog, cancel, |c| {
                    if source_staging {
                        held.family.validate_import_staging_unreserved(c, a, p, &proof.active.activation,
                            &proof.capture, staging, Some(cancel))?;
                    } else {
                        assert_eq!(held.family.validate_import_initializing(c, a, p, &proof.active.activation,
                            &proof.capture, copying, Some(cancel))?,
                            crate::projection::shared::provider_access::ImportRegistration::Initializing);
                    }
                    funding.require_retained(c, Some(cancel))?;
                    let reserved: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 OR operation=?1)",
                        [staging.reservation.as_ref().unwrap()], |row| row.get(0))?;
                    assert!(!reserved, "Staging created a C reservation");
                    Ok(())
                },
            )?)?;
            let (dest_copying, dest_staging) = managed_staging::destination_intents(copying, staging)?;
            let intent = if destination_staging { &dest_staging } else { &dest_copying };
            super::super::super::managed_destination::valid(database_io::read_cancellable_strict(
                destination.destination_database(), cancel, |c| {
                    let marker = &destination.proof().format;
                    closure.require_copied_prefix_scope(source, c, marker, prefix, Some(cancel))?;
                    if destination_staging {
                        destination_validation::read_staging(c, marker, &proof.active.source, a, p,
                            &proof.active.activation, prefix, intent, &closure, Some(cancel))?;
                    } else {
                        destination_validation::read_copied(c, marker, &proof.active.source, a, p,
                            &proof.active.activation, prefix, intent, &closure, Some(cancel))?;
                    }
                    let current = denied_coverage_storage::read_required(c, intent.plan.denial_coverage.as_ref().unwrap(), Some(cancel))?;
                    assert_eq!(current, body);
                    current.require_intent(intent, Some(cancel))?;
                    Ok(())
                },
            )?)?;
            Ok(())
        },
    ).unwrap()).unwrap();
    destination.require_published(&copying.plan).unwrap();
}

// Explicit test-only corruption: the real C was copied and validated first.
// A retained physical owner is real; this helper does not invent any proof.
fn corrupt_fixture_destination(
    f: &Fixture,
    destination: &ManagedDestinationAllocation<'_>,
    mutate: impl FnOnce(&Connection),
) {
    let file = destination.destination_database();
    assert!(file.path().starts_with(&f.outer));
    let admission = database_io::clean_admission(file, false, |_| Ok(())).unwrap();
    let mut close = Close::default();
    let bare = admission.open(file, &mut close).unwrap();
    admission.require_geometry(&bare).unwrap();
    bare.connection()
        .execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
        .unwrap();
    mutate(bare.connection());
    bare.connection().execute_batch("COMMIT").unwrap();
    bare.finish_writer(Ok(()), &mut close).unwrap();
    assert_eq!((close.opened, close.closed), (1, 1));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    f: &Fixture,
    held: &PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    owner: &mut PendingDenialStart<'_, '_, '_, '_>,
    copying: &ProviderTransitionIntentRecord,
    destination: &ManagedDestinationAllocation<'_>,
    coverage: &DerivedDeniedCoverage,
    case: Case,
    cancel: &Cancellation,
) {
    let mut expected = ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
    expected.state = TransitionState::Staging;
    expected.reservation = Some(expected.reservation_operation().unwrap());
    expected.require_transition(copying).unwrap();
    require_state(
        held,
        root,
        copying,
        &expected,
        destination,
        coverage,
        false,
        false,
        cancel,
    );
    let mut before = Snapshot::read(f, held, destination, cancel);
    if matches!(
        case,
        Case::DestinationAhead | Case::MissingBody | Case::SubstitutedBody
    ) {
        let message = if matches!(case, Case::DestinationAhead) {
            // Deliberately bypass the driver B-before-C protocol using the real
            // SQL CAS primitive, with the actual retained source/closure/owner.
            let _locks = held
                .family
                .locks(&held.artifacts.allocation.format.identity.worktree_id, true)
                .unwrap();
            owner
                .with_copying_destination_source(
                    held,
                    copying,
                    destination,
                    false,
                    cancel,
                    |source, proof, closure, _| {
                        let (dest_copying, dest_staging) =
                            managed_staging::destination_intents(copying, &expected)?;
                        corrupt_fixture_destination(f, destination, |c| {
                            assert_eq!(
                                destination_sql::stage_copied(
                                    source,
                                    c,
                                    &destination.proof().format,
                                    &proof.active.source,
                                    &held.artifacts.allocation,
                                    held.artifacts.prepared.as_ref().unwrap(),
                                    &proof.active.activation,
                                    &copying.plan.source_prefix,
                                    &dest_copying,
                                    &dest_staging,
                                    closure,
                                    Some(cancel)
                                )
                                .unwrap(),
                                CopiedStagingEffect::Updated
                            );
                        });
                        Ok(())
                    },
                )
                .unwrap();
            drop(_locks);
            require_state(
                held,
                root,
                copying,
                &expected,
                destination,
                coverage,
                false,
                true,
                cancel,
            );
            "denial source Copying requires destination CopiedPrefix"
        } else {
            let reference = copying.plan.denial_coverage.as_ref().unwrap();
            corrupt_fixture_destination(f, destination, |c| {
                if matches!(case, Case::MissingBody) {
                    assert_eq!(
                        c.execute(
                            "DELETE FROM projection_observations WHERE digest=?1",
                            [&reference.id]
                        )
                        .unwrap(),
                        1
                    );
                } else {
                    let mut substitute = TransitionDeniedCoverageRecord::decode(
                        &coverage.record().encode().unwrap(),
                    )
                    .unwrap();
                    substitute.transaction_id = "ffffffffffffffffffffffffffffffff".into();
                    assert_ne!(substitute.transaction_id, copying.transaction_id);
                    let payload = substitute.encode().unwrap();
                    assert_ne!(substitute.reference().unwrap(), *reference);
                    assert_eq!(
                        c.execute(
                            "UPDATE projection_observations SET payload=?1 WHERE digest=?2",
                            params![payload, reference.id]
                        )
                        .unwrap(),
                        1
                    );
                }
            });
            if matches!(case, Case::MissingBody) {
                "required denied coverage document is missing"
            } else {
                "denied coverage actual body differs from reference"
            }
        };
        let corrupted = Snapshot::read(f, held, destination, cancel);
        assert_eq!(before.source_file, corrupted.source_file);
        assert_eq!(before.family_file, corrupted.family_file);
        let mut close = Close::default();
        let error = owner
            .stage_denial(destination, &mut close, cancel)
            .unwrap_err();
        require_validation_message(error, message);
        assert_eq!((close.opened, close.closed), (0, 0));
        assert_eq!(Snapshot::read(f, held, destination, cancel), corrupted);
        destination.verify(&copying.plan).unwrap();
        return;
    }
    if matches!(case, Case::Capacity) {
        let usage = f.family.usage().unwrap();
        let current = usage.content_bytes + usage.reserved_bytes;
        let low = current + copying.plan.quota_bytes - 1;
        assert!(low >= current);
        f.family.set_quota(SharedQuota { bytes: low }).unwrap();
        let low_snapshot = Snapshot::read(f, held, destination, cancel);
        let mut close = Close::default();
        let error = owner
            .stage_denial(destination, &mut close, cancel)
            .unwrap_err();
        require_validation_message(
            error,
            "partial_store_quota: denial source and future destination unfunded",
        );
        assert_eq!((close.opened, close.closed), (0, 0));
        assert_eq!(Snapshot::read(f, held, destination, cancel), low_snapshot);
        require_state(
            held,
            root,
            copying,
            &expected,
            destination,
            coverage,
            false,
            false,
            cancel,
        );
        f.family
            .set_quota(SharedQuota {
                bytes: usage.limit_bytes,
            })
            .unwrap();
        let restored = Snapshot::read(f, held, destination, cancel);
        assert_eq!(restored.source_file, before.source_file);
        assert_eq!(restored.destination_file, before.destination_file);
        assert_eq!(restored.family, before.family);
        before = restored;
    }
    let mut close = Close::default();
    let result = if matches!(case, Case::SourceAck | Case::DestinationAck) {
        let refused = Rc::new(Cell::new(false));
        let probes = Rc::new(Cell::new(0));
        let ordinal = if matches!(case, Case::SourceAck) {
            1
        } else {
            2
        };
        close.refused = Some(refused.clone());
        close.refuse_at = Some(ordinal);
        let watch_refused = refused.clone();
        let watch_probes = probes.clone();
        let error = with_observer(
            move |_, _| {
                if watch_refused.get() {
                    watch_probes.set(watch_probes.get() + 1);
                    return Err(invalid("forbidden DB probe after Staging ACK"));
                }
                Ok(())
            },
            || owner.stage_denial(destination, &mut close, cancel),
        )
        .unwrap_err();
        require_validation_message(error, "C14 start SQLite close ACK withheld");
        assert!(refused.get());
        assert_eq!(probes.get(), 0);
        assert_eq!((close.opened, close.closed), (ordinal, ordinal));
        let c_staged = matches!(case, Case::DestinationAck);
        require_state(
            held,
            root,
            copying,
            &expected,
            destination,
            coverage,
            true,
            c_staged,
            cancel,
        );
        let gap = Snapshot::read(f, held, destination, cancel);
        if !c_staged {
            assert_eq!(gap.destination_file, before.destination_file);
        }
        let reduced_quota = if !c_staged {
            let usage = f.family.usage().unwrap();
            let current = usage.content_bytes + usage.reserved_bytes;
            let low = current + copying.plan.quota_bytes - 1;
            assert!(low >= current);
            f.family.set_quota(SharedQuota { bytes: low }).unwrap();
            Some((
                usage.limit_bytes,
                Snapshot::read(f, held, destination, cancel),
            ))
        } else {
            None
        };
        let mut retry = Close::default();
        let result = owner.stage_denial(destination, &mut retry, cancel).unwrap();
        assert_eq!(result.1.source, SourceIntentEffect::AlreadyPresent);
        if c_staged {
            assert_eq!(result.1.destination, CopiedStagingEffect::AlreadyPresent);
            assert_eq!((retry.opened, retry.closed), (0, 0));
            assert_eq!(Snapshot::read(f, held, destination, cancel), gap);
        } else {
            assert_eq!(result.1.destination, CopiedStagingEffect::Updated);
            assert_eq!((retry.opened, retry.closed), (1, 1));
            assert_eq!(
                Snapshot::read(f, held, destination, cancel).source_file,
                gap.source_file
            );
        }
        if let Some((limit, lowered)) = reduced_quota {
            let finished = Snapshot::read(f, held, destination, cancel);
            assert_eq!(finished.family_file, lowered.family_file);
            assert_eq!(finished.family, lowered.family);
            assert_eq!(finished.source_file, lowered.source_file);
            require_state(
                held,
                root,
                copying,
                &expected,
                destination,
                coverage,
                true,
                true,
                cancel,
            );
            f.family.set_quota(SharedQuota { bytes: limit }).unwrap();
            let restored = Snapshot::read(f, held, destination, cancel);
            assert_eq!(restored.family, before.family);
            // Only the explicit quota roundtrip changed the catalog header.
            // Stage itself was required to preserve the lowered file exactly.
            before.family_file = restored.family_file;
        }
        result
    } else {
        let result = owner.stage_denial(destination, &mut close, cancel).unwrap();
        assert_eq!((close.opened, close.closed), (2, 2));
        assert_eq!(result.1.source, SourceIntentEffect::Updated);
        assert_eq!(result.1.destination, CopiedStagingEffect::Updated);
        result
    };
    assert_eq!(result.0, expected);
    require_state(
        held,
        root,
        copying,
        &expected,
        destination,
        coverage,
        true,
        true,
        cancel,
    );
    let staged = Snapshot::read(f, held, destination, cancel);
    before.require_only_intent_changes(&staged);
    let mut replay = Close::default();
    let result = owner
        .stage_denial(destination, &mut replay, cancel)
        .unwrap();
    assert_eq!(result.0, expected);
    assert_eq!(result.1.source, SourceIntentEffect::AlreadyPresent);
    assert_eq!(result.1.destination, CopiedStagingEffect::AlreadyPresent);
    assert_eq!((replay.opened, replay.closed), (0, 0));
    require_state(
        held,
        root,
        copying,
        &expected,
        destination,
        coverage,
        true,
        true,
        cancel,
    );
    assert_eq!(Snapshot::read(f, held, destination, cancel), staged);
    if matches!(case, Case::Success) {
        // Receipt construction is DATA only: use genuine durable Staging and
        // its sealed document, but do not claim or persist Ready/publication.
        let receipt_before = Snapshot::read(f, held, destination, cancel);
        require_validation_message(
            crate::projection::provider::source_companion::planned_receipt(&expected).unwrap_err(),
            "source companion denial coverage requires matching closed mode",
        );
        let receipt = crate::projection::provider::source_companion::planned_receipt_with_coverage(
            &expected,
            coverage,
            Some(cancel),
        )
        .unwrap();
        assert_eq!(
            receipt.schema_version,
            "layrs.projection-provider-transition-receipt.v2"
        );
        assert_eq!(receipt.denial_coverage, expected.plan.denial_coverage);
        let mut ready_data =
            ProviderTransitionIntentRecord::decode(&expected.encode().unwrap()).unwrap();
        ready_data.state = TransitionState::Ready;
        ready_data.receipt_id = Some(receipt.digest().unwrap());
        ready_data.require_transition(&expected).unwrap();
        ready_data.require_receipt(&receipt).unwrap();
        let mut different =
            ProviderTransitionIntentRecord::decode(&expected.encode().unwrap()).unwrap();
        different.plan.denial_coverage.as_mut().unwrap().id = format!("blake3:{}", "f".repeat(64));
        assert_ne!(
            different.plan.denial_coverage,
            expected.plan.denial_coverage
        );
        require_validation_message(
            crate::projection::provider::source_companion::planned_receipt_with_coverage(
                &different,
                coverage,
                Some(cancel),
            )
            .unwrap_err(),
            "denied coverage body differs from transition link",
        );
        assert_eq!(Snapshot::read(f, held, destination, cancel), receipt_before);
        // Existing dual Staging is retained DATA, not new capacity admission.
        let usage = f.family.usage().unwrap();
        let current = usage.content_bytes + usage.reserved_bytes;
        let low = current + copying.plan.quota_bytes - 1;
        assert!(low >= current);
        f.family.set_quota(SharedQuota { bytes: low }).unwrap();
        let low_snapshot = Snapshot::read(f, held, destination, cancel);
        let mut close = Close::default();
        let result = owner.stage_denial(destination, &mut close, cancel).unwrap();
        assert_eq!(result.0, expected);
        assert_eq!(result.1.source, SourceIntentEffect::AlreadyPresent);
        assert_eq!(result.1.destination, CopiedStagingEffect::AlreadyPresent);
        assert_eq!((close.opened, close.closed), (0, 0));
        require_state(
            held,
            root,
            copying,
            &expected,
            destination,
            coverage,
            true,
            true,
            cancel,
        );
        assert_eq!(Snapshot::read(f, held, destination, cancel), low_snapshot);
        f.family
            .set_quota(SharedQuota {
                bytes: usage.limit_bytes,
            })
            .unwrap();
        let restored = Snapshot::read(f, held, destination, cancel);
        assert_eq!(restored.source_file, staged.source_file);
        assert_eq!(restored.destination_file, staged.destination_file);
        assert_eq!(restored.family, staged.family);
    }
}

macro_rules! staging_recipe {
    ($name:ident, $case:ident) => {
        #[test]
        #[ignore = "owned native C14 Staging only; no C reservation, companion or Ready claim"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                1200,
                || {
                    child::declare();
                    run_start_configuration(
                        false,
                        false,
                        None,
                        PreimageCase::DescriptorOnly,
                        Some(AllocationCase::Stage(Case::$case)),
                    );
                },
            );
        }
    };
}
staging_recipe!(
    denial_staging_commits_exact_phases_and_replays_without_future_capacity,
    Success
);
staging_recipe!(
    denial_staging_retries_source_ack_under_reduced_quota_with_only_destination_writer,
    SourceAck
);
staging_recipe!(
    denial_staging_retries_destination_ack_without_writers,
    DestinationAck
);
staging_recipe!(
    denial_staging_refuses_insufficient_future_capacity_before_source_writer,
    Capacity
);
staging_recipe!(
    denial_staging_refuses_destination_ahead_of_source_after_fixture_corruption,
    DestinationAhead
);
staging_recipe!(
    denial_staging_refuses_missing_current_body_without_repair,
    MissingBody
);
staging_recipe!(
    denial_staging_refuses_substituted_current_body_without_repair,
    SubstitutedBody
);
