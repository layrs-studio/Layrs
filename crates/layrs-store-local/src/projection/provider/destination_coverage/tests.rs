//! Audited scalar SQL recipes. Source closure uses the real active fixture,
//! normalizer and intent writer. Destination proof fields below are codec DATA
//! only; no native/pinned/family capability or EmptyIngress is manufactured.
use super::*;
use crate::projection::{
    provider::{
        database_io::{BareConnection, with_reference},
        owned_test as child,
        records::{
            activation::ProviderActivationRecord,
            denied_coverage::DerivedDeniedCoverage,
            transition::{TransitionBackendKind, TransitionDestinationProof, TransitionState},
        },
        recovery::error::ProviderBootstrapCause,
        transition_sql::{
            apply_source_intent_with_coverage, coverage_tests::planned_with_coverage,
        },
        validation::{active_fixture, tests::fixture::Fixture},
    },
    retention,
};

fn destination_data(
    fixture: &Fixture,
    source: &ProviderTransitionIntentRecord,
) -> ProviderTransitionIntentRecord {
    let mut intent = ProviderTransitionIntentRecord::decode(&source.encode().unwrap()).unwrap();
    let mut marker = fixture.allocation.format.clone();
    marker.identity = source.plan.source_prefix.destination_identity.clone();
    marker.checkout_root = std::path::Path::new(&source.plan.destination_parent)
        .join(&source.plan.destination_leaf_nonce)
        .to_str()
        .unwrap()
        .into();
    marker.shared.as_mut().unwrap().database_identity = "1:0:123".into();
    marker.import_digest = Some(source.plan.source_prefix_id.clone());
    intent.role = TransitionRole::Destination;
    intent.state = TransitionState::Copying;
    intent.local_generation = 0;
    intent.destination = Some(TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: marker,
        private_database_identity: "1:0:123".into(),
        family_marker_digest: fixture.allocation.family.marker_digest.clone(),
        format_file_identity: "1:0:124".into(),
        native_control_identity_hex: None,
        native_root_identity: None,
    });
    intent.validate_fields().unwrap();
    intent
}

fn with_source(
    action: impl FnOnce(
        &Connection,
        &Fixture,
        &ProviderActivationRecord,
        &ProviderTransitionIntentRecord,
        &ProviderTransitionIntentRecord,
        &DerivedDeniedCoverage,
    ),
) {
    with_source_phase(true, action);
}

fn with_source_phase(
    copying_phase: bool,
    action: impl FnOnce(
        &Connection,
        &Fixture,
        &ProviderActivationRecord,
        &ProviderTransitionIntentRecord,
        &ProviderTransitionIntentRecord,
        &DerivedDeniedCoverage,
    ),
) {
    let bare = BareConnection::reference().unwrap();
    let source = bare.connection();
    let (fixture, _, _, activation, activated) = active_fixture(source, false);
    source.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (intent, derived) = planned_with_coverage(source, &fixture, &activation, &activated);
    apply_source_intent_with_coverage(
        source,
        &fixture.allocation,
        &fixture.prepared,
        &activation.binding,
        [0x41; 16],
        None,
        &intent,
        Some(&derived),
        None,
    )
    .unwrap();
    let destination = destination_data(&fixture, &intent);
    let mut copying =
        ProviderTransitionIntentRecord::decode(&destination.encode().unwrap()).unwrap();
    copying.role = TransitionRole::Source;
    copying.local_generation = intent.local_generation;
    if copying_phase {
        apply_source_intent_with_coverage(
            source,
            &fixture.allocation,
            &fixture.prepared,
            &activation.binding,
            [0x41; 16],
            Some(&intent),
            &copying,
            Some(&derived),
            None,
        )
        .unwrap();
    }
    action(
        source,
        &fixture,
        &activation,
        if copying_phase { &copying } else { &intent },
        &destination,
        &derived,
    );
    if !source.is_autocommit() {
        source.execute_batch("ROLLBACK").unwrap();
    }
    bare.finish(Ok(())).unwrap();
}

fn with_destination(action: impl FnOnce(&Connection) -> Result<()>) {
    with_reference(retention::SQL, |destination| {
        destination.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
        action(destination)?;
        if !destination.is_autocommit() {
            destination.execute_batch("ROLLBACK")?;
        }
        Ok(())
    })
    .unwrap();
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn current_coverage_copies_actual_body_exactly_and_obeys_destination_rollback() {
    child::run(
        concat!(
            module_path!(),
            "::current_coverage_copies_actual_body_exactly_and_obeys_destination_rollback"
        ),
        120,
        || {
            child::declare();
            with_source(
                |source, f, activation, intent, destination_intent, derived| {
                    let current = read_source(
                        source,
                        &f.allocation,
                        &f.prepared,
                        &activation.binding,
                        [0x41; 16],
                        intent,
                        None,
                    )
                    .unwrap();
                    assert_eq!(current.record(), derived.record());
                    with_destination(|destination| {
                        let before = destination.total_changes();
                        assert!(read_current(destination, destination_intent, None).is_err());
                        assert_eq!(destination.total_changes(), before);
                        current.copy_fresh_into(destination, destination_intent, None)?;
                        assert_eq!(destination.total_changes(), before + 1);
                        let reference = current.record().reference()?;
                        let copied: Vec<u8> = destination.query_row(
                            "SELECT payload FROM projection_observations WHERE digest=?1",
                            [&reference.id],
                            |row| row.get(0),
                        )?;
                        let original: Vec<u8> = source.query_row(
                            "SELECT payload FROM projection_observations WHERE digest=?1",
                            [&reference.id],
                            |row| row.get(0),
                        )?;
                        assert_eq!(copied, original);
                        assert_eq!(
                            read_current(destination, destination_intent, None)?.as_ref(),
                            Some(current.record())
                        );
                        // Read-only replay never materializes a replacement body.
                        let before = destination.total_changes();
                        read_current(destination, destination_intent, None)?;
                        assert_eq!(destination.total_changes(), before);
                        destination
                            .execute_batch("ROLLBACK; PRAGMA query_only=ON; BEGIN DEFERRED")?;
                        assert!(read_current(destination, destination_intent, None).is_err());
                        let count: u64 = destination.query_row(
                            "SELECT count(*) FROM projection_observations",
                            [],
                            |row| row.get(0),
                        )?;
                        assert_eq!(count, 0);
                        assert_eq!(destination.total_changes(), before);
                        destination
                            .execute_batch("ROLLBACK; PRAGMA query_only=OFF; BEGIN IMMEDIATE")?;
                        current.copy_fresh_into(destination, destination_intent, None)?;
                        destination
                            .execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")?;
                        // This reader uses the destination body alone after
                        // the retained source transaction has actually ended.
                        source.execute_batch("ROLLBACK")?;
                        let before = destination.total_changes();
                        assert_eq!(
                            read_current(destination, destination_intent, None)?.as_ref(),
                            Some(current.record())
                        );
                        assert_eq!(destination.total_changes(), before);
                        Ok(())
                    });
                },
            );
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn current_coverage_revalidates_exact_source_intent_and_body_before_copy() {
    child::run(
        concat!(
            module_path!(),
            "::current_coverage_revalidates_exact_source_intent_and_body_before_copy"
        ),
        120,
        || {
            child::declare();
            with_source(|source, f, activation, intent, destination_intent, _| {
                let read = || {
                    read_source(
                        source,
                        &f.allocation,
                        &f.prepared,
                        &activation.binding,
                        [0x41; 16],
                        intent,
                        None,
                    )
                };
                let current = read().unwrap();
                let reference = current.record().reference().unwrap();
                for corruption in 0..4 {
                    source.execute_batch("SAVEPOINT changed_source").unwrap();
                    match corruption {
                        0 => {
                            source
                                .execute(
                                    "DELETE FROM projection_provider_transitions WHERE id=?1",
                                    [&intent.transaction_id],
                                )
                                .unwrap();
                        }
                        1 => {
                            source
                                .execute(
                                    "DELETE FROM projection_observations WHERE digest=?1",
                                    [&reference.id],
                                )
                                .unwrap();
                        }
                        2 => {
                            let mut other = TransitionDeniedCoverageRecord::decode(
                                &current.record().encode().unwrap(),
                            )
                            .unwrap();
                            other.transaction_id = "00000000000000000000000000000003".into();
                            source
                                .execute(
                                    "UPDATE projection_observations SET payload=?1 WHERE digest=?2",
                                    params![other.encode().unwrap(), reference.id],
                                )
                                .unwrap();
                        }
                        _ => {
                            let mut other =
                                ProviderTransitionIntentRecord::decode(&intent.encode().unwrap())
                                    .unwrap();
                            other.plan.quota_bytes += 1;
                            source.execute("UPDATE projection_provider_transitions SET payload=?1,digest=?2 WHERE id=?3", params![other.encode().unwrap(), other.digest().unwrap(), intent.transaction_id]).unwrap();
                        }
                    }
                    assert!(read().is_err());
                    with_destination(|destination| {
                        let before = destination.total_changes();
                        assert!(
                            current
                                .copy_fresh_into(destination, destination_intent, None)
                                .is_err()
                        );
                        assert_eq!(destination.total_changes(), before);
                        let count: u64 = destination.query_row(
                            "SELECT count(*) FROM projection_observations",
                            [],
                            |row| row.get(0),
                        )?;
                        assert_eq!(count, 0);
                        Ok(())
                    });
                    source
                        .execute_batch("ROLLBACK TO changed_source; RELEASE changed_source")
                        .unwrap();
                }
                assert!(read().is_ok());
                source.execute_batch("ROLLBACK").unwrap();
                assert!(read().is_err());
                assert!(
                    current
                        .require_destination(destination_intent, None)
                        .is_err()
                );
                with_destination(|destination| {
                    let before = destination.total_changes();
                    assert!(
                        current
                            .copy_fresh_into(destination, destination_intent, None)
                            .is_err()
                    );
                    assert_eq!(destination.total_changes(), before);
                    Ok(())
                });
            });
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn current_coverage_refuses_foreign_destination_and_any_preexisting_id_without_repair() {
    child::run(
        concat!(
            module_path!(),
            "::current_coverage_refuses_foreign_destination_and_any_preexisting_id_without_repair"
        ),
        120,
        || {
            child::declare();
            with_source(|source, f, activation, intent, destination_intent, _| {
                let current = read_source(
                    source,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    [0x41; 16],
                    intent,
                    None,
                )
                .unwrap();
                with_destination(|destination| {
                    for field in 0..2 {
                        let mut wrong =
                            ProviderTransitionIntentRecord::decode(&destination_intent.encode()?)?;
                        if field == 0 {
                            wrong.transaction_id = "00000000000000000000000000000003".into();
                        } else {
                            wrong.plan.quota_bytes += 1;
                        }
                        wrong.validate_fields()?;
                        let before = destination.total_changes();
                        assert!(current.require_destination(&wrong, None).is_err());
                        assert!(current.copy_fresh_into(destination, &wrong, None).is_err());
                        assert!(read_current(destination, &wrong, None).is_err());
                        assert_eq!(destination.total_changes(), before);
                    }
                    assert!(current.require_destination(intent, None).is_err());
                    assert!(read_current(destination, intent, None).is_err());
                    let reference = current.record().reference()?;
                    for payload in [current.record().encode()?, b"{}".to_vec()] {
                        destination.execute_batch("SAVEPOINT preexisting_body")?;
                        destination.execute(
                            "INSERT INTO projection_observations(digest,payload) VALUES(?1,?2)",
                            params![reference.id, payload],
                        )?;
                        let before = destination.total_changes();
                        assert!(
                            current
                                .copy_fresh_into(destination, destination_intent, None)
                                .is_err()
                        );
                        assert_eq!(destination.total_changes(), before);
                        let retained: Vec<u8> = destination.query_row(
                            "SELECT payload FROM projection_observations WHERE digest=?1",
                            [&reference.id],
                            |row| row.get(0),
                        )?;
                        assert_eq!(retained, payload);
                        destination.execute_batch(
                            "ROLLBACK TO preexisting_body; RELEASE preexisting_body",
                        )?;
                    }
                    let before = destination.total_changes();
                    assert!(read_current(destination, destination_intent, None).is_err());
                    assert_eq!(destination.total_changes(), before); // missing replay stays missing
                    let mut legacy =
                        ProviderTransitionIntentRecord::decode(&destination_intent.encode()?)?;
                    legacy.schema_version = "layrs.projection-provider-transition-intent.v1".into();
                    legacy.plan.denial_coverage = None;
                    assert!(read_current(destination, &legacy, None)?.is_none());
                    assert_eq!(destination.total_changes(), before);
                    Ok(())
                });
            });
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn current_coverage_requires_distinct_write_transaction_fk_and_live_cancellation() {
    child::run(
        concat!(
            module_path!(),
            "::current_coverage_requires_distinct_write_transaction_fk_and_live_cancellation"
        ),
        120,
        || {
            child::declare();
            with_source(|source, f, activation, intent, destination_intent, _| {
                let current = read_source(
                    source,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    [0x41; 16],
                    intent,
                    None,
                )
                .unwrap();
                let before = source.total_changes();
                assert_invalid(
                    current.copy_fresh_into(source, destination_intent, None),
                    "current coverage destination must be a distinct connection",
                );
                assert_eq!(source.total_changes(), before);
                with_destination(|destination| {
                    let before = destination.total_changes();
                    destination.execute_batch("ROLLBACK")?;
                    assert!(
                        current
                            .copy_fresh_into(destination, destination_intent, None)
                            .is_err()
                    );
                    for begin in ["BEGIN DEFERRED", "SAVEPOINT outer_copy"] {
                        destination.execute_batch(begin)?;
                        assert!(
                            current
                                .copy_fresh_into(destination, destination_intent, None)
                                .is_err()
                        );
                        destination.execute_batch("ROLLBACK")?;
                    }
                    destination.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")?;
                    assert!(
                        current
                            .copy_fresh_into(destination, destination_intent, None)
                            .is_err()
                    );
                    destination.execute_batch(
                        "ROLLBACK; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; PRAGMA query_only=ON",
                    )?;
                    assert!(
                        current
                            .copy_fresh_into(destination, destination_intent, None)
                            .is_err()
                    );
                    destination.execute_batch("PRAGMA query_only=OFF")?;
                    let cancel = Cancellation::default();
                    cancel.cancel();
                    assert!(matches!(
                        read_source(
                            source,
                            &f.allocation,
                            &f.prepared,
                            &activation.binding,
                            [0x41; 16],
                            intent,
                            Some(&cancel)
                        ),
                        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
                    ));
                    assert!(matches!(
                        current.copy_fresh_into(destination, destination_intent, Some(&cancel)),
                        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
                    ));
                    assert!(matches!(
                        read_current(destination, destination_intent, Some(&cancel)),
                        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
                    ));
                    assert_eq!(destination.total_changes(), before);
                    current.copy_fresh_into(destination, destination_intent, None)?;
                    Ok(())
                });
            });
        },
    );
}

fn assert_invalid<T>(result: Result<T>, expected: &str) {
    match result {
        Err(ProviderBootstrapCause::Validation(cause)) => match *cause {
            crate::projection::ProjectionStoreError::Invalid(message) => {
                assert_eq!(message, expected)
            }
            other => panic!("expected Invalid({expected}), got {other:?}"),
        },
        Err(other) => panic!("expected Invalid({expected}), got {other:?}"),
        Ok(_) => panic!("expected Invalid({expected}), got success"),
    }
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn current_coverage_requires_actual_unreserved_source_copying_and_exact_destination_proof() {
    child::run(
        concat!(
            module_path!(),
            "::current_coverage_requires_actual_unreserved_source_copying_and_exact_destination_proof"
        ),
        120,
        || {
            child::declare();
            with_source_phase(false, |source, f, activation, planned, _, _| {
                let before = source.total_changes();
                assert_invalid(
                    read_source(
                        source,
                        &f.allocation,
                        &f.prepared,
                        &activation.binding,
                        [0x41; 16],
                        planned,
                        None,
                    ),
                    "current coverage requires unreserved source Copying with destination proof",
                );
                assert_eq!(before, source.total_changes());
            });
            with_source(|source, f, activation, copying, destination, coverage| {
                let current = read_source(
                    source,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    [0x41; 16],
                    copying,
                    None,
                )
                .unwrap();
                with_destination(|c| {
                    for changed in 0..3 {
                        let mut wrong =
                            ProviderTransitionIntentRecord::decode(&destination.encode()?)?;
                        match changed {
                            0 => {
                                wrong.destination.as_mut().unwrap().format_file_identity =
                                    "1:0:999".into()
                            }
                            1 => {
                                wrong.state = TransitionState::Staging;
                                wrong.reservation = Some(wrong.reservation_operation()?);
                            }
                            _ => wrong.state = TransitionState::Cancelled,
                        }
                        wrong.validate_fields()?;
                        let before = c.total_changes();
                        assert_invalid(
                            current.require_destination(&wrong, None),
                            "current coverage requires unreserved destination Copying with matching destination proof",
                        );
                        assert_invalid(
                            current.copy_fresh_into(c, &wrong, None),
                            "current coverage requires unreserved destination Copying with matching destination proof",
                        );
                        assert_eq!(before, c.total_changes());
                    }
                    // Source Copying with a reservation is a valid DATA codec but
                    // cannot manufacture a current transport scope in this API.
                    let mut reserved = ProviderTransitionIntentRecord::decode(&copying.encode()?)?;
                    reserved.reservation = Some(reserved.reservation_operation()?);
                    reserved.validate_fields()?;
                    assert_invalid(
                        read_source(
                            source,
                            &f.allocation,
                            &f.prepared,
                            &activation.binding,
                            [0x41; 16],
                            &reserved,
                            None,
                        ),
                        "current coverage requires unreserved source Copying with destination proof",
                    );
                    // A real source cancellation retains its document. Reading that
                    // actual Cancelled row must still refuse a fresh C transport.
                    let mut cancelled = ProviderTransitionIntentRecord::decode(&copying.encode()?)?;
                    cancelled.state = TransitionState::Cancelled;
                    apply_source_intent_with_coverage(
                        source,
                        &f.allocation,
                        &f.prepared,
                        &activation.binding,
                        [0x41; 16],
                        Some(copying),
                        &cancelled,
                        Some(coverage),
                        None,
                    )?;
                    let before_source = source.total_changes();
                    let before = c.total_changes();
                    assert_invalid(
                        read_source(
                            source,
                            &f.allocation,
                            &f.prepared,
                            &activation.binding,
                            [0x41; 16],
                            &cancelled,
                            None,
                        ),
                        "current coverage requires unreserved source Copying with destination proof",
                    );
                    assert_invalid(
                        current.require_destination(destination, None),
                        "current coverage exact source intent is absent or differs",
                    );
                    assert_invalid(
                        current.copy_fresh_into(c, destination, None),
                        "current coverage exact source intent is absent or differs",
                    );
                    assert_eq!(before_source, source.total_changes());
                    assert_eq!(before, c.total_changes());
                    let body = denied_coverage_storage::read_required(
                        source,
                        &coverage.record().reference()?,
                        None,
                    )?;
                    assert_eq!(&body, coverage.record());
                    Ok(())
                });
            });
        },
    );
}
