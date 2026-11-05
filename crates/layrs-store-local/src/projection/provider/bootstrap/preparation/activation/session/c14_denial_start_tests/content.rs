//! One actual C14 begin-to-publication recipe. No fixture SQL is forged.
//! The observation is local DATA, not a continuous remote-authority lease.
use super::super::super::{
    managed_allocation::{self, ManagedDestinationAllocation},
    managed_content::{self, CompanionPins},
    managed_destination,
    managed_image::complete::VerifiedCompleteManagedImage,
    managed_journal::denial_start::PendingDenialStart,
};
use super::*;
use crate::projection::{
    provider::{
        bootstrap::outcome::PublicationPins,
        denied_coverage_storage, destination_ready,
        destination_sql::CopiedStagingEffect,
        records::{
            import::{ImportSource, ProviderTransitionReceiptRecord},
            transition::{TransitionRole, TransitionState},
        },
        source_companion::SourceCompanionPublication,
        transition_reconcile::source_sql,
    },
    shared::{
        exact_content::ExactBinding,
        source_companion::{SourceCompanionFamilyContext, SourceCompanionPresence},
        source_denial::SourceDeniedCoverageReservation,
    },
};

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
            database_io::read_cancellable_strict(file, cancel, table_snapshot)
                .and_then(managed_destination::valid)
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
}

#[allow(clippy::too_many_arguments)]
fn require_terminal(
    session: &ProviderActiveSession,
    destination: &ManagedDestinationAllocation<'_>,
    staging: &ProviderTransitionIntentRecord,
    publication: &SourceCompanionPublication<'_>,
    coverage: &DerivedDeniedCoverage,
    receipt: &ProviderTransitionReceiptRecord,
    cancel: &Cancellation,
) {
    let held = &session.source.held;
    let a = &held.artifacts.allocation;
    let _locks = held
        .family
        .locks(&a.format.identity.worktree_id, false)
        .unwrap();
    managed_destination::valid(database_io::read_cancellable_strict(
        &held.artifacts.database,
        cancel,
        |source| {
            let full = validation::private_active_publication_with_source(
                source, a, held.artifacts.prepared.as_ref().unwrap(), &held.binding,
                Some(session.source.root.registration().root_id()), Some(cancel),
            )?;
            let actual = full.transitions.iter()
                .find(|row| row.transaction_id == staging.transaction_id)
                .ok_or_else(|| invalid("C14 recipe actual Published source absent"))?;
            assert_eq!(actual.role, TransitionRole::Source);
            assert_eq!(actual.state, TransitionState::Published);
            assert_eq!(actual.plan, staging.plan);
            actual.require_receipt(receipt)?;
            publication.rows().require_new_rows(source, Some(cancel))?;
            let source_receipt: Vec<u8> = source.query_row(
                "SELECT payload FROM projection_provider_transition_receipts WHERE transaction_id=?1",
                [staging.transaction_id.as_str()], |row| row.get(0),
            )?;
            assert_eq!(source_receipt, receipt.encode()?);
            let source_body = denied_coverage_storage::read_required(
                source, actual.plan.denial_coverage.as_ref().unwrap(), Some(cancel),
            )?;
            assert_eq!(&source_body, coverage.record());
            source_body.require_intent(actual, Some(cancel))?;
            let funding = SourceDeniedCoverageReservation::new(a, staging, &source_body, Some(cancel))?;
            assert!(full.capture.denial_reservations().contains(&funding));
            assert_ne!(funding.operation(), publication.protection().operation());
            managed_destination::valid(database_io::read_cancellable_strict(
                &held.family.catalog, cancel, |family| {
                    held.family.validate_source_companion_catalog(family, a,
                        &SourceCompanionFamilyContext {
                            publication: &full,
                            pending: Some(publication.protection()),
                        }, Some(cancel))?;
                    funding.require_retained(family, Some(cancel))?;
                    assert_eq!(publication.protection().inspect(family, Some(cancel))?,
                        SourceCompanionPresence::Protected);
                    let (member_state, database_identity): (String, String) = family.query_row(
                        "SELECT state,database_identity FROM worktrees WHERE id=?1",
                        [destination.proof().format.identity.worktree_id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    assert_eq!(member_state, "ready");
                    assert_eq!(database_identity, destination.proof().private_database_identity);
                    let distinct: u64 = family.query_row(
                        "SELECT count(*) FROM reservations WHERE operation IN (?1,?2)",
                        rusqlite::params![funding.operation(), publication.protection().operation()],
                        |row| row.get(0),
                    )?;
                    assert_eq!(distinct, 2, "document d and source companion s must both remain funded");
                    managed_destination::valid(database_io::read_cancellable_strict(
                        destination.destination_database(), cancel, |c| {
                            // Full SQL Ready interpretation of actual C and family;
                            // no detached DATA is promoted to physical/admission authority.
                            let ready = destination_ready::read_managed_ready(
                                c, &destination.proof().format, family, Some(cancel),
                            )?;
                            assert_eq!(ready.receipt(), receipt);
                            assert_eq!(ready.ready().state, TransitionState::Ready);
                            assert_eq!(ready.ready().role, TransitionRole::Destination);
                            assert_eq!(ready.ready().plan, staging.plan);
                            publication.rows().require_new_rows(c, Some(cancel))?;
                            let body = denied_coverage_storage::read_required(c,
                                ready.ready().plan.denial_coverage.as_ref().unwrap(), Some(cancel))?;
                            assert_eq!(body, source_body);
                            body.require_intent(ready.ready(), Some(cancel))?;
                            let published = crate::projection::provider::published_handoff::read_published_for_ready(
                                source,
                                crate::projection::provider::published_handoff::PublicationSource {
                                    allocation: a,
                                    prepared: held.artifacts.prepared.as_ref().unwrap(),
                                    binding: &held.binding,
                                    root_id: session.source.root.registration().root_id(),
                                }, ready.ready(), ready.receipt(), Some(cancel),
                            )?;
                            let changes = c.total_changes();
                            let error = crate::projection::provider::managed_admission::admit_ready(
                                c, &destination.proof().format, family, source,
                                &ready, &published, Some(cancel),
                            ).unwrap_err();
                            require_validation_message(error, "admitted denial coverage writer is not integrated");
                            assert_eq!(c.total_changes(), changes);
                            assert_eq!(c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?, 8);
                            Ok(())
                        },
                    )?)
                },
            )?)
        },
    ).unwrap()).unwrap();
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    f: &Fixture,
    session: &ProviderActiveSession,
    attempt: &CaptureAttemptLock,
    planned: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    observed: &TransitionObservationData,
    image: &mut VerifiedCompleteManagedImage<'_, '_>,
    required: &[FileManifest],
    metadata: u64,
    candidate_cost: &crate::projection::provider::source_companion::ProspectiveCompanionCost,
    ranges: &RangeWatch,
    cancel: &Cancellation,
) -> admission::Published {
    let held = &session.source.held;
    let a = &held.artifacts.allocation;
    let prepared = held.artifacts.prepared.as_ref().unwrap();
    // This real connected Store anchor must precede Planned. Reconstructing it
    // from a pending plan or the complete image would lose predecessor history.
    let anchor = session.transition_source(cancel).unwrap();
    candidate_cost.require_plan(planned).unwrap();
    let usage = f.family.usage().unwrap();
    let funding =
        SourceDeniedCoverageReservation::new(a, planned, coverage.record(), Some(cancel)).unwrap();
    let threshold = usage
        .content_bytes
        .checked_add(usage.reserved_bytes)
        .unwrap()
        .checked_add(funding.reservation_bytes())
        .unwrap()
        .checked_add(planned.plan.quota_bytes)
        .unwrap()
        .checked_add(candidate_cost.reservation_bytes())
        .unwrap();
    assert!(candidate_cost.reservation_bytes() > 0);
    f.family
        .set_quota(crate::projection::SharedQuota {
            bytes: threshold - 1,
        })
        .unwrap();
    let before = (
        database_io::tests::support::witness(held.artifacts.database.path()),
        database_io::tests::support::witness(held.family.catalog.path()),
    );
    let parent = session
        .session
        .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
        .unwrap();
    let error = PendingDenialStart::prepare(
        held,
        parent,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        planned,
        coverage,
        observed,
        image,
        required,
        cancel,
    )
    .err()
    .expect("d+q fits but missing one byte of future s must refuse before B");
    require_validation_message(
        error,
        "partial_store_quota: denial source and future destination unfunded",
    );
    assert_eq!(
        (
            database_io::tests::support::witness(held.artifacts.database.path()),
            database_io::tests::support::witness(held.family.catalog.path()),
        ),
        before
    );
    assert!(
        denial_receipt::reopen(a, prepared, Some(cancel))
            .unwrap()
            .is_none()
    );
    assert!(!ranges.funded.load(Ordering::SeqCst));
    assert!(
        !std::path::Path::new(&planned.plan.destination_parent)
            .join(&planned.plan.destination_leaf_nonce)
            .exists()
    );
    f.family
        .set_quota(crate::projection::SharedQuota { bytes: threshold })
        .unwrap();
    let parent = session
        .session
        .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
        .unwrap();
    let mut pending = PendingDenialStart::prepare(
        held,
        parent,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        planned,
        coverage,
        observed,
        image,
        required,
        cancel,
    )
    .unwrap();
    let prospective = pending
        .prospective_companion_cost()
        .unwrap()
        .cloned()
        .expect("Content must retain the pre-funding source companion bound");
    prospective.require_plan(planned).unwrap();
    assert_eq!(&prospective, candidate_cost);
    let refused = Rc::new(Cell::new(false));
    let mut family_close = Close {
        funded: Some(ranges.funded.clone()),
        refused: Some(refused.clone()),
        ..Close::default()
    };
    let mut source_close = Close::default();
    let error = pending
        .begin(&mut family_close, &mut source_close, cancel)
        .unwrap_err();
    require_validation_message(error, "C14 start SQLite close ACK withheld");
    assert!(refused.get());
    assert_eq!((family_close.opened, family_close.closed), (1, 1));
    assert_eq!((source_close.opened, source_close.closed), (0, 0));
    assert_eq!(
        f.family.usage().unwrap().reserved_bytes,
        usage.reserved_bytes + funding.reservation_bytes()
    );
    let mut family_close = Close::default();
    assert_eq!(
        pending
            .begin(&mut family_close, &mut source_close, cancel)
            .unwrap(),
        SourceIntentEffect::Inserted
    );
    assert_eq!((family_close.opened, family_close.closed), (0, 0));
    assert_eq!((source_close.opened, source_close.closed), (1, 1));
    f.family
        .set_quota(crate::projection::SharedQuota {
            bytes: threshold - 1,
        })
        .unwrap();
    let before_replay = (
        database_io::tests::support::witness(held.artifacts.database.path()),
        database_io::tests::support::witness(held.family.catalog.path()),
    );
    let mut source_replay = Close::default();
    assert_eq!(
        pending
            .begin(&mut family_close, &mut source_replay, cancel)
            .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert_eq!(
        (
            family_close.opened,
            family_close.closed,
            source_replay.opened,
            source_replay.closed
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        (
            database_io::tests::support::witness(held.artifacts.database.path()),
            database_io::tests::support::witness(held.family.catalog.path())
        ),
        before_replay
    );
    f.family
        .set_quota(crate::projection::SharedQuota {
            bytes: usage.limit_bytes,
        })
        .unwrap();
    let mut destination =
        managed_allocation::allocate_denial(&session.session, held, &mut pending, planned, cancel)
            .unwrap_or_else(|error| panic!("C14 content allocation: {error:?}"));
    let mut close = Close::default();
    let (copying, effect) = pending
        .bind_denial_allocation(&destination, &mut close, cancel)
        .unwrap();
    assert_eq!(effect, SourceIntentEffect::Updated);
    assert_eq!((close.opened, close.closed), (1, 1));
    destination
        .publish_format_denial(&mut pending, &copying, cancel)
        .unwrap();
    destination.require_published(&copying.plan).unwrap();
    let before = (close.opened, close.closed);
    let copied = managed_destination::register_and_copy_denial(
        held,
        &mut pending,
        &copying,
        &destination,
        &mut close,
        cancel,
    )
    .unwrap();
    assert_eq!(
        copied.family,
        crate::projection::shared::provider_access::ImportRegistrationEffect::Inserted
    );
    assert_eq!(
        copied.destination,
        managed_destination::DestinationSqlState::FreshCopied
    );
    assert_eq!((close.opened - before.0, close.closed - before.1), (2, 2));
    require_denial_copy_state(
        held,
        &pending,
        &copying,
        &destination,
        coverage,
        true,
        cancel,
    );
    let before = (close.opened, close.closed);
    let (staging, effect) = pending
        .stage_denial(&destination, &mut close, cancel)
        .unwrap();
    assert_eq!(effect.source, SourceIntentEffect::Updated);
    assert_eq!(effect.destination, CopiedStagingEffect::Updated);
    assert_eq!((close.opened - before.0, close.closed - before.1), (2, 2));
    drop(pending);
    assert_eq!(ranges.late.load(Ordering::SeqCst), 0);

    let rows = {
        let _locks = held
            .family
            .locks(&a.format.identity.worktree_id, false)
            .unwrap();
        database_io::read_cancellable_strict(&held.artifacts.database, cancel, |c| {
            let full = validation::private_active_publication_with_source(
                c,
                a,
                prepared,
                &held.binding,
                Some(session.source.root.registration().root_id()),
                Some(cancel),
            )?;
            anchor.require_transition(held, &session.source.root, &full, &staging, cancel)?;
            let ImportSource::Provider { version, .. } = &staging.plan.source_prefix.source else {
                panic!("C14 content fixture requires Provider prefix");
            };
            let closure = import_validation::verify_source_publication(
                c,
                &a.format,
                &staging.plan.source_prefix,
                ImportSourceReferences::Provider {
                    allocation: a,
                    prepared,
                    activation: &full.active.activation,
                },
                &full,
                &version.effective_image_digest,
                Some(cancel),
            )?;
            source_sql::with_source(
                c,
                &a.format,
                &staging.plan.source_prefix,
                &closure,
                &full,
                observed,
                Some(cancel),
                |source| {
                    image.reconcile_denial_publication_retained(
                        source, observed, &full, &staging, coverage, required, metadata, cancel,
                    )
                },
            )
        })
        .and_then(managed_destination::valid)
        .unwrap()
    };
    assert!(
        rows.header().is_some(),
        "Reserved modified README must derive a real companion header"
    );
    assert!(
        rows.authorized_files()
            .iter()
            .all(|row| row.path.as_str() != "README.md")
    );
    assert_eq!(rows.manifests(), required);
    let derived = super::super::super::managed_plan::derive_reconciled_denial_data(
        &staging.plan.source_prefix,
        &rows,
        observed,
        staging.plan.source_prefix.destination_identity.clone(),
        &staging.plan.destination_parent,
        &staging.plan.destination_parent_identity,
        &staging.plan.destination_leaf_nonce,
        &staging.transaction_id,
        coverage,
    )
    .unwrap();
    assert_eq!(derived.plan, staging.plan);
    let binding = {
        let proof = destination.proof();
        let shared = proof.format.shared.as_ref().unwrap();
        let operation = staging.reservation_operation().unwrap();
        ExactBinding {
            family_incarnation: a.family.incarnation.clone(),
            family_root_identity: a.family.root_identity.clone(),
            family_marker_digest: a.family.marker_digest.clone(),
            family_catalog_identity: a.family.catalog_file_identity.clone(),
            plan_digest: staging.plan.digest().unwrap(),
            prefix_id: staging.plan.source_prefix_id.clone(),
            destination_proof_digest: proof.digest(&staging.plan).unwrap(),
            transaction_id: staging.transaction_id.clone(),
            owner: proof.format.identity.worktree_id.clone(),
            database_identity: shared.database_identity.clone(),
            reservation: operation.clone(),
            operation,
            quota_bytes: staging.plan.quota_bytes,
        }
    };
    let publication = SourceCompanionPublication::derive_with_coverage(
        a,
        &staging,
        &binding,
        &rows,
        coverage,
        Some(cancel),
    )
    .unwrap();
    prospective
        .require_actual(&publication, Some(cancel))
        .unwrap();
    // Exercise the public boundary with a real publication and independently
    // invalid cost DATA. A refusal must precede every writer and byte request.
    let cost_refusal_before = Snapshot::read(f, held, &destination, cancel);
    let cost_refusal_requests = session.hydration_requests().len();
    for (wrong_cost, expected_error) in prospective.negative_test_cases() {
        let mut refused_pins = CompanionPins::default();
        let mut refused_close = Close::default();
        let error = managed_content::commit_denial_companion(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            attempt,
            &destination,
            image,
            &staging,
            &publication,
            &anchor,
            observed,
            metadata,
            coverage,
            &wrong_cost,
            &mut refused_pins,
            &mut refused_close,
            cancel,
        )
        .unwrap_err();
        require_validation_message(error, expected_error);
        assert_eq!((refused_close.opened, refused_close.closed), (0, 0));
        assert_eq!(
            Snapshot::read(f, held, &destination, cancel),
            cost_refusal_before
        );
        assert_eq!(session.hydration_requests().len(), cost_refusal_requests);
    }
    let mut pins = CompanionPins::default();
    let before_legacy = Snapshot::read(f, held, &destination, cancel);
    let requests = session.hydration_requests().len();
    let mut legacy = Close::default();
    let error = managed_content::commit_companion(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        &destination,
        image,
        &staging,
        &publication,
        &anchor,
        observed,
        metadata,
        &mut pins,
        &mut legacy,
        cancel,
    )
    .unwrap_err();
    require_validation_message(error, "companion opener denial coverage mode differs");
    assert_eq!((legacy.opened, legacy.closed), (0, 0));
    assert_eq!(Snapshot::read(f, held, &destination, cancel), before_legacy);
    let mut commit = Close::default();
    let receipt = managed_content::commit_denial_companion(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        &destination,
        image,
        &staging,
        &publication,
        &anchor,
        observed,
        metadata,
        coverage,
        &prospective,
        &mut pins,
        &mut commit,
        cancel,
    )
    .unwrap();
    assert!(commit.opened > 0);
    assert_eq!(commit.opened, commit.closed);
    assert_eq!(
        receipt.schema_version,
        "layrs.projection-provider-transition-receipt.v2"
    );
    assert_eq!(receipt.denial_coverage, staging.plan.denial_coverage);
    assert_eq!(&receipt, publication.receipt());
    require_terminal(
        session,
        &destination,
        &staging,
        &publication,
        coverage,
        &receipt,
        cancel,
    );
    assert!(
        !destination.root().join("README.md").exists(),
        "Reserved source bytes escaped the companion"
    );
    assert_eq!(
        fs::read(destination.root().join("unchanged.txt")).unwrap(),
        b"C14 distinct immutable bytes"
    );
    assert_eq!(
        fs::read(f.checkout.join("README.md")).unwrap(),
        b"C14 current native image"
    );
    let published = Snapshot::read(f, held, &destination, cancel);
    let mut replay = Close::default();
    let repeated = managed_content::commit_denial_companion(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        &destination,
        image,
        &staging,
        &publication,
        &anchor,
        observed,
        metadata,
        coverage,
        &prospective,
        &mut pins,
        &mut replay,
        cancel,
    )
    .unwrap();
    assert_eq!(repeated, receipt);
    assert_eq!((replay.opened, replay.closed), (0, 0));
    assert_eq!(Snapshot::read(f, held, &destination, cancel), published);
    require_terminal(
        session,
        &destination,
        &staging,
        &publication,
        coverage,
        &receipt,
        cancel,
    );
    assert_eq!(session.hydration_requests().len(), requests);
    assert_eq!(ranges.late.load(Ordering::SeqCst), 0);
    require_ready_body_refusals(held, &destination, coverage, cancel);
    require_terminal(
        session,
        &destination,
        &staging,
        &publication,
        coverage,
        &receipt,
        cancel,
    );
    let destination_root = destination.root().to_path_buf();
    let destination_database_path = destination.destination_database().path().to_path_buf();
    let witness = || {
        (
            database_io::tests::support::witness(&destination_database_path),
            database_io::tests::support::witness(held.artifacts.database.path()),
            database_io::tests::support::witness(held.family.catalog.path()),
        )
    };
    let before_admission = witness();
    drop(pins);
    drop(publication);
    drop(destination);
    drop(anchor);
    let admission = crate::projection::provider::managed_ready_reopen::ManagedReadyAdmission::open(
        &destination_root,
        cancel,
    )
    .unwrap();
    let mut admission_close = Close::default();
    let failure = session
        .admit_ready_destination(admission, &mut admission_close, cancel)
        .expect_err("C14 admission must refuse before its first writer");
    assert_eq!(
        failure.step,
        crate::projection::provider::managed_ready_reopen::AdmissionStep::Validating
    );
    require_validation_message(
        failure.cause,
        "admitted denial coverage writer is not integrated",
    );
    assert_eq!((admission_close.opened, admission_close.closed), (0, 0));
    assert_eq!(witness(), before_admission);
    drop(failure.held);
    admission::Published {
        root: destination_root,
        receipt,
    }
}

/// Explicit corruption inside rolled-back test transactions on an already
/// genuine Ready C. No forged state is committed or used as native authority.
fn require_ready_body_refusals(
    held: &PublicationPins,
    destination: &ManagedDestinationAllocation<'_>,
    coverage: &DerivedDeniedCoverage,
    cancel: &Cancellation,
) {
    let file = destination.destination_database();
    let _locks = held
        .family
        .locks(&held.artifacts.allocation.format.identity.worktree_id, true)
        .unwrap();
    let reference = coverage.record().reference().unwrap();
    let mut substitute = crate::projection::provider::records::denied_coverage::TransitionDeniedCoverageRecord::decode(
        &coverage.record().encode().unwrap(),
    ).unwrap();
    substitute.transaction_id = "ffffffffffffffffffffffffffffffff".into();
    assert_ne!(substitute.reference().unwrap(), reference);
    let payload = substitute.encode().unwrap();
    managed_destination::valid(
        database_io::read_cancellable_strict(&held.family.catalog, cancel, |family| {
            let check = |c: &Connection| {
                destination_ready::read_managed_ready(
                    c,
                    &destination.proof().format,
                    family,
                    Some(cancel),
                )
                .map(|_| ())
            };
            let admission = database_io::clean_admission(file, false, check)?;
            let mut close = Close::default();
            let bare = admission.open(file, &mut close)?;
            admission.require_geometry(&bare)?;
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            for case in 0..3 {
                c.execute_batch("SAVEPOINT corrupt_current_body")?;
                let expected = match case {
                    0 => {
                        assert_eq!(
                            c.execute(
                                "DELETE FROM projection_observations WHERE digest=?1",
                                [&reference.id]
                            )?,
                            1
                        );
                        "required denied coverage document is missing"
                    }
                    1 => {
                        assert_eq!(
                            c.execute(
                                "UPDATE projection_observations SET payload=?1 WHERE digest=?2",
                                rusqlite::params![payload, reference.id]
                            )?,
                            1
                        );
                        "denied coverage actual body differs from reference"
                    }
                    _ => {
                        assert_eq!(
                            c.execute(
                                "INSERT INTO projection_observations(digest,payload) VALUES(?1,?2)",
                                rusqlite::params![substitute.reference()?.id, payload]
                            )?,
                            1
                        );
                        "destination Ready table has missing or extra rows"
                    }
                };
                let changes = c.total_changes();
                require_validation_message(check(c).unwrap_err(), expected);
                assert_eq!(c.total_changes(), changes, "refusal must never repair C");
                c.execute_batch("ROLLBACK TO corrupt_current_body; RELEASE corrupt_current_body")?;
                check(c)?;
            }
            c.execute_batch("ROLLBACK")?;
            bare.finish_writer(Ok(()), &mut close)?;
            assert_eq!((close.opened, close.closed), (1, 1));
            Ok(())
        })
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "owned native C14 content/publication and same-owner replay; no admission/autonomy claim"]
fn c14_denial_companion_commits_real_ready_and_published_then_replays_without_writers() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_companion_commits_real_ready_and_published_then_replays_without_writers"
        ),
        1800,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::Content),
            );
        },
    );
}
