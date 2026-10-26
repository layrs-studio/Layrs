//! Memory SQL recipes exercise only reserve-only row interpretation/charge.
//! Actual source Ready, physical pins and both private Staging proofs require
//! the separate native constructor qualification; these fixtures mint none.
use super::*;
use crate::projection::{
    provider::{database_io::with_reference, owned_test as child},
    shared::schema_v2,
};

const OPERATION: &str =
    "provider-transition:1111111111111111111111111111111111111111111111111111111111111111";
const CHARGE: u64 = 65536;

fn populated(c: &Connection, limit: u64) {
    c.execute("INSERT INTO family VALUES(1,?1)", [b"family".as_slice()])
        .unwrap();
    c.execute("INSERT INTO quota VALUES(1,?1)", [limit])
        .unwrap();
    for (owner, member) in [
        ("destination", "member-destination"),
        ("sibling", "member-sibling"),
    ] {
        c.execute(
            "INSERT INTO worktrees VALUES(?1,?2,'database',?3,'initializing')",
            params![owner, member, b"marker".as_slice()],
        )
        .unwrap();
    }
    c.execute(
        "INSERT INTO chunks VALUES(?1,100)",
        [format!("blake3:{}", "c".repeat(64))],
    )
    .unwrap();
    c.execute(
        "INSERT INTO sweep VALUES('chunks',?1,20)",
        [format!("blake3:{}", "d".repeat(64))],
    )
    .unwrap();
}

fn spec() -> ReservationSpec<'static> {
    ReservationSpec {
        id: OPERATION,
        owner: "destination",
        operation: OPERATION,
        bytes: CHARGE,
    }
}

fn memory(action: impl FnOnce(&Connection) -> Result<()>) {
    with_reference("", |c| {
        schema_v2::initialize_reference(c)?;
        action(c)
    })
    .unwrap();
}

#[test]
#[ignore = "owned audited SQLite child; managed reservation insertion, exact replay and rollback"]
fn exact_managed_reservation_replays_without_writes_and_rolls_back() {
    child::run(
        concat!(
            module_path!(),
            "::exact_managed_reservation_replays_without_writes_and_rolls_back"
        ),
        120,
        || {
            child::declare();
            memory(|c| {
                populated(c, 100_000);
                c.execute("INSERT INTO reservations VALUES('sibling-reservation','sibling','operation',64)", [])?;
                c.execute_batch("BEGIN IMMEDIATE")?;
                require_transaction(c, true, None)?;
                let initial = validate_reservation_only(c, &spec(), None)?;
                assert_eq!(initial.reservation, ImportContentReservation::Absent);
                assert_eq!(
                    initial.usage,
                    ImportContentUsage {
                        content_bytes: 120,
                        reserved_bytes: 64,
                        limit_bytes: 100_000
                    }
                );
                assert_eq!(
                    insert_or_replay_reservation(c, &spec(), None)?,
                    ImportReservationEffect::Inserted
                );
                assert_eq!(
                    validate_reservation_only(c, &spec(), None)?.reservation,
                    ImportContentReservation::Present { remaining: CHARGE }
                );
                let before = c.total_changes();
                assert_eq!(
                    insert_or_replay_reservation(c, &spec(), None)?,
                    ImportReservationEffect::AlreadyPresent
                );
                assert_eq!(c.total_changes(), before);
                c.execute(
                    "UPDATE reservations SET remaining=remaining-1 WHERE id=?1",
                    [OPERATION],
                )?;
                assert!(validate_reservation_only(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("exact charge differs"));
                c.execute_batch("ROLLBACK")?;
                assert!(!c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1)",
                    [OPERATION],
                    |row| row.get::<_, bool>(0)
                )?);
                c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN DEFERRED")?;
                require_transaction(c, false, None)?;
                assert!(
                    require_transaction(c, true, None).is_err(),
                    "RO can keep FK OFF; writer cannot"
                );
                c.execute_batch("ROLLBACK")?;
                Ok(())
            });
        },
    );
}

#[test]
#[ignore = "owned audited SQLite child; managed quota and unsupported family effects refuse before insertion"]
fn managed_reservation_refuses_foreign_charge_and_untyped_effects() {
    child::run(
        concat!(
            module_path!(),
            "::managed_reservation_refuses_foreign_charge_and_untyped_effects"
        ),
        120,
        || {
            child::declare();
            memory(|c| {
                populated(c, CHARGE + 119);
                c.execute_batch("BEGIN IMMEDIATE")?;
                require_transaction(c, true, None)?;
                let before = c.total_changes();
                assert!(insert_or_replay_reservation(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("insufficient unreserved"));
                assert_eq!(c.total_changes(), before);
                c.execute("UPDATE quota SET bytes=100000", [])?;
                c.execute(
                    "INSERT INTO reservations VALUES(?1,'sibling',?1,?2)",
                    params![OPERATION, CHARGE],
                )?;
                assert!(validate_reservation_only(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("identity or exact charge differs"));
                c.execute("DELETE FROM reservations WHERE id=?1", [OPERATION])?;
                c.execute(
                    "INSERT INTO reservations VALUES('other-attempt','destination','operation',1)",
                    [],
                )?;
                assert!(validate_reservation_only(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("foreign reservation"));
                c.execute("DELETE FROM reservations WHERE id='other-attempt'", [])?;
                c.execute("INSERT INTO roots VALUES('root','destination','database','operation','digest',?1)", [b"root".as_slice()])?;
                assert!(validate_reservation_only(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("root or legacy installation"));
                c.execute("DELETE FROM roots WHERE id='root'", [])?;
                insert_or_replay_reservation(c, &spec(), None)?;
                let file = format!("blake3:{}", "a".repeat(64));
                c.execute(
                    "INSERT INTO installations VALUES(?1,?2,?3,NULL,0)",
                    params![OPERATION, file, b"legacy".as_slice()],
                )?;
                assert!(validate_reservation_only(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("root or legacy installation"));
                c.execute(
                    "DELETE FROM installations WHERE reservation=?1",
                    [OPERATION],
                )?;
                c.execute(
                    "INSERT INTO exact_installations VALUES(?1,?2,?2,0,?3,NULL,0)",
                    params![OPERATION, file, b"exact".as_slice()],
                )?;
                assert!(validate_reservation_only(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported exact installations"));
                c.execute(
                    "DELETE FROM exact_installations WHERE reservation=?1",
                    [OPERATION],
                )?;
                c.execute(
                    "UPDATE reservations SET operation=?1 WHERE id=?2",
                    params!["x".repeat(129), OPERATION],
                )?;
                assert!(validate_reservation_only(c, &spec(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("text exceeds bound"));
                c.execute(
                    "UPDATE reservations SET operation=?1 WHERE id=?1",
                    [OPERATION],
                )?;
                let cancel = Cancellation::default();
                cancel.cancel();
                let before = c.total_changes();
                assert!(insert_or_replay_reservation(c, &spec(), Some(&cancel)).is_err());
                assert_eq!(c.total_changes(), before);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            });
        },
    );
}
