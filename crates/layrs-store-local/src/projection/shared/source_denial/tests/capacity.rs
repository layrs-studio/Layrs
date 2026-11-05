//! SQL-only prospective sizing. These synthetic rows test accounting and exact
//! binding; they do not establish a complete family or native owner capability.
use super::*;
use crate::projection::provider::recovery::ProviderBootstrapCause;

fn require_invalid(error: ProviderBootstrapCause, expected: &str) {
    match error {
        ProviderBootstrapCause::Validation(error) => match error.as_ref() {
            crate::projection::ProjectionStoreError::Invalid(message) => {
                assert_eq!(message, expected)
            }
            other => panic!("unexpected validation error: {other}"),
        },
        other => panic!("unexpected error category: {other}"),
    }
}

#[test]
#[ignore = "owned SQL recipe; future C quota before document B, not native qualification"]
fn denial_future_capacity_counts_global_usage_and_b_once_at_exact_boundary() {
    child::run(
        concat!(
            module_path!(),
            "::denial_future_capacity_counts_global_usage_and_b_once_at_exact_boundary"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, mut planned, body) = fixture();
                planned.plan.quota_bytes = 1_000_003;
                let funding = SourceDeniedCoverageReservation::new(&a, &planned, &body, None)?;
                let content = 11 + 13 + 17 + 8192 + 23;
                let reserved = 19;
                let physical = reserved + funding.reservation_bytes() + planned.plan.quota_bytes;
                seed(c, &funding, content + physical - 1)?;
                c.execute("INSERT INTO chunks VALUES('chunk',11)", [])?;
                c.execute("INSERT INTO sweep VALUES('chunks','retired',13)", [])?;
                c.execute("INSERT INTO files VALUES(?1,0)", [TOKEN])?;
                c.execute(
                    "INSERT INTO manifests VALUES(?1,?1,0,?2)",
                    params![TOKEN, vec![0u8; 17]],
                )?;
                c.execute(
                    "INSERT INTO reservations VALUES('foreign','worktree:foreign','foreign',19)",
                    [],
                )?;
                c.execute(
                    "INSERT INTO exact_installations VALUES('foreign',?1,?1,0,?2,?3,0)",
                    params![TOKEN, b"journal".as_slice(), vec![0u8; 23]],
                )?;
                let changes = c.total_changes();
                require_invalid(
                    funding
                        .prospective_destination_capacity(c, &planned, None)
                        .unwrap_err(),
                    "partial_store_quota: denial source and future destination unfunded",
                );
                assert_eq!(c.total_changes(), changes);
                assert_eq!(
                    funding.read_presence(c, None)?,
                    SourceDeniedCoveragePresence::Absent
                );
                c.execute("UPDATE quota SET bytes=bytes+1", [])?;
                let changes = c.total_changes();
                assert_eq!(
                    funding.prospective_destination_capacity(c, &planned, None)?,
                    physical
                );
                assert_eq!(c.total_changes(), changes);
                assert_eq!(funding.reserve(c, None)?, ProtectionEffect::Inserted);
                let changes = c.total_changes();
                assert_eq!(
                    funding.prospective_destination_capacity(c, &planned, None)?,
                    physical
                );
                assert_eq!(c.total_changes(), changes);
                c.execute("UPDATE quota SET bytes=bytes-1", [])?;
                let changes = c.total_changes();
                require_invalid(
                    funding
                        .prospective_destination_capacity(c, &planned, None)
                        .unwrap_err(),
                    "partial_store_quota: denial source and future destination unfunded",
                );
                funding.require_retained(c, None)?;
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "owned SQL recipe; changed future plan and colliding B refuse without writes"]
fn denial_future_capacity_refuses_changed_plan_and_colliding_reservation_without_effects() {
    child::run(
        concat!(
            module_path!(),
            "::denial_future_capacity_refuses_changed_plan_and_colliding_reservation_without_effects"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, planned, body) = fixture();
                let funding = SourceDeniedCoverageReservation::new(&a, &planned, &body, None)?;
                seed(c, &funding, 1_000_000_000)?;
                let mut altered = ProviderTransitionIntentRecord::decode(&planned.encode()?)?;
                altered.plan.quota_bytes = altered.plan.quota_bytes.checked_add(1).unwrap();
                altered.validate_fields()?;
                let changes = c.total_changes();
                require_invalid(
                    funding
                        .prospective_destination_capacity(c, &altered, None)
                        .unwrap_err(),
                    "source denial prospective destination plan differs",
                );
                assert_eq!(c.total_changes(), changes);
                for state_change in [false, true] {
                    let mut wrong = ProviderTransitionIntentRecord::decode(&planned.encode()?)?;
                    if state_change {
                        wrong.state = TransitionState::Cancelled;
                    } else {
                        wrong.transaction_id = "00000000000000000000000000000009".into();
                    }
                    wrong.validate_fields()?;
                    let changes = c.total_changes();
                    require_invalid(
                        funding
                            .prospective_destination_capacity(c, &wrong, None)
                            .unwrap_err(),
                        "source denial prospective destination plan differs",
                    );
                    assert_eq!(c.total_changes(), changes);
                }
                c.execute_batch("SAVEPOINT foreign_owner")?;
                c.execute(
                    "INSERT INTO reservations VALUES(?1,'worktree:foreign',?1,?2)",
                    params![funding.operation(), funding.reservation_bytes()],
                )?;
                let changes = c.total_changes();
                require_invalid(
                    funding
                        .prospective_destination_capacity(c, &planned, None)
                        .unwrap_err(),
                    "source denial fixed reservation differs",
                );
                assert_eq!(c.total_changes(), changes);
                let count: u64 =
                    c.query_row("SELECT count(*) FROM reservations", [], |r| r.get(0))?;
                assert_eq!(count, 1);
                c.execute_batch("ROLLBACK TO foreign_owner; RELEASE foreign_owner")?;
                funding.reserve(c, None)?;
                c.execute_batch("SAVEPOINT duplicate")?;
                c.execute(
                    "INSERT INTO reservations VALUES('different-id','worktree:foreign',?1,1)",
                    [funding.operation()],
                )?;
                let changes = c.total_changes();
                require_invalid(
                    funding
                        .prospective_destination_capacity(c, &planned, None)
                        .unwrap_err(),
                    "source denial reservation ID/operation collision",
                );
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK TO duplicate; RELEASE duplicate")?;
                for exact in [false, true] {
                    c.execute_batch("SAVEPOINT installation")?;
                    if exact {
                        c.execute(
                            "INSERT INTO exact_installations VALUES(?1,?2,?2,0,?3,NULL,0)",
                            params![funding.operation(), TOKEN, b"journal".as_slice()],
                        )?;
                    } else {
                        c.execute(
                            "INSERT INTO installations VALUES(?1,?2,?3,NULL,0)",
                            params![funding.operation(), TOKEN, b"journal".as_slice()],
                        )?;
                    }
                    let changes = c.total_changes();
                    require_invalid(
                        funding
                            .prospective_destination_capacity(c, &planned, None)
                            .unwrap_err(),
                        "source denial reservation cannot fund installations",
                    );
                    assert_eq!(c.total_changes(), changes);
                    c.execute_batch("ROLLBACK TO installation; RELEASE installation")?;
                }
                c.execute_batch("ROLLBACK")?;
                let changes = c.total_changes();
                require_invalid(
                    funding
                        .prospective_destination_capacity(c, &planned, None)
                        .unwrap_err(),
                    "source denial requires caller-owned transaction",
                );
                assert_eq!(c.total_changes(), changes);
                Ok(())
            })
            .unwrap();
        },
    );
}
