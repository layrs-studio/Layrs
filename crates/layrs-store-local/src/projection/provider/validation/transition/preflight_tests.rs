//! Prospective checks are read-only DATA admission, never actual funding.
use super::*;
use crate::projection::provider::{
    database_io::BareConnection,
    process_profile::tests::child,
    records::capture::{ProviderCaptureStage, ProviderCaptureTransactionRecord},
    transition_sql::coverage_tests::planned_with_coverage,
    validation::active_fixture,
};
use rusqlite::params;

fn require_preflight_error(
    error: super::super::super::recovery::error::ProviderBootstrapCause,
    expected: &str,
) {
    assert!(
        matches!(&error,
        crate::projection::provider::recovery::error::ProviderBootstrapCause::Validation(cause)
        if matches!(cause.as_ref(), crate::projection::ProjectionStoreError::Invalid(message)
            if message == expected)),
        "{error}"
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; source CAS preflight negatives"]
fn source_cas_preflight_requires_exact_row_transaction_role_and_uncancelled_scope() {
    child::run(
        concat!(
            module_path!(),
            "::source_cas_preflight_requires_exact_row_transaction_role_and_uncancelled_scope"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let (initial, coverage) = planned_with_coverage(c, &f, &activation, &activated);
            crate::projection::provider::transition_sql::apply_source_intent_with_coverage(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                [0x41; 16],
                None,
                &initial,
                Some(&coverage),
                None,
            )
            .unwrap();
            let mut next =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            next.state = TransitionState::Cancelled;
            c.execute_batch("COMMIT").unwrap();
            let changes = c.total_changes();
            require_preflight_error(
                preflight_source_transition_update(c, &initial, &next, None).unwrap_err(),
                "source transition preflight requires retained transaction",
            );
            c.execute_batch("BEGIN").unwrap();
            let full = private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            assert_eq!(full.transitions.as_slice(), std::slice::from_ref(&initial));
            preflight_source_transition_update(c, &initial, &next, None).unwrap();
            require_preflight_error(
                preflight_source_transition_update(c, &next, &initial, None).unwrap_err(),
                "transition intent or phase changed",
            );
            let mut foreign =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            foreign.transaction_id = "ffffffffffffffffffffffffffffffff".into();
            let mut foreign_next =
                ProviderTransitionIntentRecord::decode(&foreign.encode().unwrap()).unwrap();
            foreign_next.state = TransitionState::Cancelled;
            require_preflight_error(
                preflight_source_transition_update(c, &foreign, &foreign_next, None).unwrap_err(),
                "source transition preflight actual row differs",
            );
            let mut wrong_role =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            wrong_role.role = TransitionRole::Destination;
            require_preflight_error(
                preflight_source_transition_update(c, &wrong_role, &next, None).unwrap_err(),
                "source transition preflight requires source records",
            );
            let token = layrs_vfs::Cancellation::default();
            token.cancel();
            assert!(matches!(
                preflight_source_transition_update(c, &initial, &next, Some(&token)),
                Err(
                    crate::projection::provider::recovery::error::ProviderBootstrapCause::Vfs(
                        layrs_vfs::Error::Cancelled
                    )
                )
            ));
            assert_eq!(c.total_changes(), changes);
            let after = private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            assert_eq!(after.transitions.as_slice(), std::slice::from_ref(&initial));
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; synthetic aggregate DATA guard only"]
fn source_cas_preflight_bounds_aggregate_and_count_without_writes() {
    child::run(
        concat!(
            module_path!(),
            "::source_cas_preflight_bounds_aggregate_and_count_without_writes"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let (initial, coverage) = planned_with_coverage(c, &f, &activation, &activated);
            crate::projection::provider::transition_sql::apply_source_intent_with_coverage(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                [0x41; 16],
                None,
                &initial,
                Some(&coverage),
                None,
            )
            .unwrap();
            let mut next =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            next.state = TransitionState::Cancelled;
            let after_len = next.encode().unwrap().len() as u64;
            let padding_digest = initial.digest().unwrap();
            assert!(after_len > initial.encode().unwrap().len() as u64);
            // Deliberate synthetic SQL padding tests ONLY this numeric guard. These
            // rows are not protocol records, never passed as admitted source DATA
            // or native authority, and the entire fixture transaction is rolled back.
            c.execute_batch("SAVEPOINT padding").unwrap();
            let mut left = MAX_PAYLOAD_TOTAL - after_len;
            let mut index = 0;
            while left > 0 {
                let bytes = left.min(ProviderTransitionIntentRecord::MAX_BYTES as u64);
                c.execute("INSERT INTO projection_provider_transitions VALUES(?1,'source','cancelled',?4,NULL,NULL,zeroblob(?2),?3)",
                params![format!("padding-{index}"), bytes, padding_digest, initial.local_generation]).unwrap();
                left -= bytes;
                index += 1;
            }
            let changes = c.total_changes();
            preflight_source_transition_update(c, &initial, &next, None).unwrap();
            assert_eq!(c.total_changes(), changes);
            c.execute("INSERT INTO projection_provider_transitions VALUES('extra-byte','source','cancelled',?1,NULL,NULL,zeroblob(1),?2)", params![initial.local_generation, padding_digest]).unwrap();
            let changes = c.total_changes();
            require_preflight_error(
                preflight_source_transition_update(c, &initial, &next, None).unwrap_err(),
                "source transition payload exceeds bound",
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO padding; RELEASE padding; SAVEPOINT row_count")
                .unwrap();
            for i in 0..MAX_ROWS {
                c.execute("INSERT INTO projection_provider_transitions VALUES(?1,'source','cancelled',?2,NULL,NULL,zeroblob(0),?3)", params![format!("row-{i}"), initial.local_generation, padding_digest]).unwrap();
            }
            let changes = c.total_changes();
            require_preflight_error(
                preflight_source_transition_update(c, &initial, &next, None).unwrap_err(),
                "source transition row count exceeds bound",
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO row_count; RELEASE row_count")
                .unwrap();
            // Remove the explicit corruptions and prove the admitted source again.
            let full = private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            assert_eq!(full.transitions.as_slice(), std::slice::from_ref(&initial));
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
fn prospective_reader_limits_include_the_new_row_and_document() {
    assert!(require_bounds(MAX_ROWS, MAX_PAYLOAD_TOTAL, 1).is_ok());
    assert!(require_bounds(MAX_ROWS + 1, 1, 1).is_err());
    assert!(require_bounds(1, MAX_PAYLOAD_TOTAL + 1, 1).is_err());
    assert!(coverage::require_count(MAX_ROWS).is_ok());
    assert!(coverage::require_count(MAX_ROWS + 1).is_err());
    assert!(require_pending_head(false, false, 2, 2).is_ok());
    assert!(require_pending_head(true, false, 2, 2).is_err());
    assert!(require_pending_head(false, true, 2, 2).is_err());
    assert!(require_pending_head(false, false, 1, 2).is_err());
}

#[test]
#[ignore = "owned audited Windows SQLite process; read-only denial preflight"]
fn durable_pending_capture_refuses_prospective_denial_without_changes() {
    child::run(
        concat!(
            module_path!(),
            "::durable_pending_capture_refuses_prospective_denial_without_changes"
        ),
        120,
        || {
            child::declare();
            for staging in [false, true] {
                let bare = BareConnection::reference().unwrap();
                let c = bare.connection();
                let (f, _, mount, activation, activated) = active_fixture(c, true);
                c.execute_batch("BEGIN IMMEDIATE").unwrap();
                let (next, body) = planned_with_coverage(c, &f, &activation, &activated);
                let mut tx = ProviderCaptureTransactionRecord::begin(
                    &f.allocation,
                    &mount,
                    &activation,
                    "preflight-pending".into(),
                    0,
                    None,
                )
                .unwrap();
                if staging {
                    tx.state = ProviderCaptureState::Staging {
                        stage: ProviderCaptureStage {
                            image_digest: "a".repeat(64),
                            image_path_count: 0,
                            changes: vec![],
                            reservation: "preflight-staging".into(),
                        },
                    };
                }
                c.execute(
                    "INSERT INTO projection_provider_capture_transactions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![tx.transaction_id, tx.mount_id, tx.capture_generation, tx.capture_digest,
                        tx.expected_capture_sequence, tx.expected_capture_digest, tx.sql_state(),
                        tx.reservation(), tx.encode().unwrap(), tx.digest().unwrap()],
                ).unwrap();
                c.execute_batch("COMMIT; BEGIN").unwrap();
                let before = private_active_transition_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    None,
                    None,
                )
                .unwrap();
                let changes = c.total_changes();
                let error = preflight_denial_start(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &before,
                    &next,
                    &body,
                    None,
                )
                .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("pending head or capture state differs"),
                    "{error}"
                );
                assert_eq!(c.total_changes(), changes);
                let counts: (u64, u64) = c.query_row(
                    "SELECT (SELECT count(*) FROM projection_provider_transitions),(SELECT count(*) FROM projection_observations)",
                    [], |row| Ok((row.get(0)?, row.get(1)?)),
                ).unwrap();
                assert_eq!(counts, (0, 0));
                c.execute_batch("ROLLBACK").unwrap();
                bare.finish(Ok(())).unwrap();
            }
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; retained-history denial preflight"]
fn retained_history_reserves_future_live_payload_before_any_insertion() {
    child::run(
        concat!(
            module_path!(),
            "::retained_history_reserves_future_live_payload_before_any_insertion"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let (next, body) = planned_with_coverage(c, &f, &activation, &activated);
            let state = private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            let changes = c.total_changes();
            preflight_denial_start(c, &f.allocation, &f.prepared, &state, &next, &body, None)
                .unwrap();
            assert_eq!(c.total_changes(), changes);
            assert!(state.capture.denial_reservations().is_empty());
            let planned_bytes = next.encode().unwrap().len() as u64;
            let live_payload_bound = ProviderTransitionIntentRecord::MAX_BYTES as u64;
            assert!(planned_bytes < live_payload_bound);
            let mut total = 0;
            for index in 0..MAX_ROWS - 1 {
                // Stop as soon as future growth cannot fit. The actual new
                // Planned payload must still fit, distinguishing this fence
                // from the old current-payload-only admission.
                if total + live_payload_bound > MAX_PAYLOAD_TOTAL {
                    break;
                }
                let mut history =
                    ProviderTransitionIntentRecord::decode(&next.encode().unwrap()).unwrap();
                history.schema_version = "layrs.projection-provider-transition-intent.v1".into();
                history.plan.denial_coverage = None;
                history.transaction_id = format!("{:032x}", index + 1000);
                history.state = TransitionState::Cancelled;
                // A valid bounded absolute DATA locator makes each historical
                // payload large enough to hit aggregate bytes before MAX_ROWS.
                // No directory is opened or claimed by this Cancelled record.
                history.plan.destination_parent = format!("C:\\{}", "history/".repeat(500));
                let payload = history.encode().unwrap();
                assert!(
                    (MAX_ROWS - 1) * payload.len() as u64 + live_payload_bound > MAX_PAYLOAD_TOTAL
                );
                if total + payload.len() as u64 > MAX_PAYLOAD_TOTAL {
                    break;
                }
                c.execute(
                    "INSERT INTO projection_provider_transitions VALUES(?1,'source','cancelled',?2,NULL,NULL,?3,?4)",
                    params![history.transaction_id, history.local_generation, payload, history.digest().unwrap()],
                ).unwrap();
                total += payload.len() as u64;
            }
            c.execute_batch("COMMIT; BEGIN").unwrap();
            // Every retained row and the original source still pass the full
            // production reader. Planned itself still fits; only its reserved
            // growth through later phases exceeds the logical journal fence.
            let state = private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            assert!(!state.transitions.is_empty());
            assert!((state.transitions.len() as u64) < MAX_ROWS);
            assert!(total + planned_bytes <= MAX_PAYLOAD_TOTAL);
            assert!(total + live_payload_bound > MAX_PAYLOAD_TOTAL);
            require_bounds(
                state.transitions.len() as u64 + 1,
                total + planned_bytes,
                planned_bytes,
            )
            .unwrap();
            let changes = c.total_changes();
            let error =
                preflight_denial_start(c, &f.allocation, &f.prepared, &state, &next, &body, None)
                    .unwrap_err();
            assert!(
                matches!(&error,
                    crate::projection::provider::recovery::error::ProviderBootstrapCause::Validation(cause)
                    if matches!(cause.as_ref(), crate::projection::ProjectionStoreError::Invalid(message)
                        if message == "source transition payload exceeds bound")),
                "{error}"
            );
            assert_eq!(c.total_changes(), changes);
            assert!(state.capture.denial_reservations().is_empty());
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
