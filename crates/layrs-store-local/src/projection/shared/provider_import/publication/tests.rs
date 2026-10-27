//! Lower SQL layers only: these synthetic members are not private Ready or
//! filesystem proofs. Full publication additionally requires FamilyAccess and
//! independently authenticated caller inputs in the physical driver.
use super::*;
use crate::projection::{
    provider::{database_io::with_reference, owned_test as child},
    shared::schema_v2,
};

fn ledger(
    c: &Connection,
) -> (
    u64,
    Vec<(String, String, String, u64)>,
    Vec<(String, Vec<u8>)>,
    Vec<(String, Vec<u8>)>,
) {
    let quota = c
        .query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let reservations = c
        .prepare("SELECT id,owner,operation,remaining FROM reservations ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let roots = c
        .prepare("SELECT id,payload FROM roots ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let journals = c
        .prepare("SELECT manifest_id,payload FROM exact_installations ORDER BY manifest_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    (quota, reservations, roots, journals)
}

#[test]
#[ignore = "owned audited SQLite child; family publication exact row CAS preserves retained effects"]
fn member_publication_cas_preserves_retained_charge_and_refuses_mismatches() {
    child::run(
        concat!(
            module_path!(),
            "::member_publication_cas_preserves_retained_charge_and_refuses_mismatches"
        ),
        120,
        || {
            child::declare();
            with_reference("",|c| {
            schema_v2::initialize_test_reference(c)?;
            c.execute("INSERT INTO family VALUES(1,?1)",[b"family".as_slice()])?;
            c.execute("INSERT INTO quota VALUES(1,1000000)",[])?;
            c.execute("INSERT INTO worktrees VALUES('destination','member','database',?1,'initializing')",[b"marker".as_slice()])?;
            c.execute("INSERT INTO reservations VALUES('reservation','destination','operation',600000)",[])?;
            c.execute("INSERT INTO roots VALUES('root','destination','database','operation','root',?1)",[b"retained root".as_slice()])?;
            let id = format!("blake3:{}","1".repeat(64));
            c.execute("INSERT INTO exact_installations VALUES('reservation',?1,?1,0,?2,NULL,0)",params![id,b"retained journal DATA".as_slice()])?;
            c.execute_batch("BEGIN IMMEDIATE")?;
            let before = ledger(c);
            assert_eq!(member_phase(c,"destination",None)?,ImportMemberPhase::Initializing);
            for (owner,path,database,marker) in [
                ("foreign","member","database",b"marker".as_slice()),
                ("destination","other","database",b"marker".as_slice()),
                ("destination","member","other",b"marker".as_slice()),
                ("destination","member","database",b"other".as_slice()),
            ] {
                let changes = c.total_changes();
                assert!(compare_and_publish(c,owner,path,database,marker).is_err());
                assert_eq!(c.total_changes(),changes);
                assert_eq!(member_phase(c,"destination",None)?,ImportMemberPhase::Initializing);
            }
            compare_and_publish(c,"destination","member","database",b"marker")?;
            assert_eq!(member_phase(c,"destination",None)?,ImportMemberPhase::Ready);
            assert_eq!(ledger(c),before);
            c.execute_batch("ROLLBACK; PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")?;
            let changes = c.total_changes();
            assert!(compare_and_publish(c,"destination","member","database",b"marker").unwrap_err().to_string().contains("FK ON"));
            assert_eq!(c.total_changes(),changes);
            assert_eq!(member_phase(c,"destination",None)?,ImportMemberPhase::Initializing);
            c.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            compare_and_publish(c,"destination","member","database",b"marker")?;
            c.execute_batch("COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED")?;
            let changes = c.total_changes();
            assert_eq!(member_phase(c,"destination",None)?,ImportMemberPhase::Ready);
            assert_eq!(ledger(c),before);
            assert_eq!(c.total_changes(),changes,"publication phase inspection is a no-write replay input");
            c.execute_batch("ROLLBACK")?;
            Ok(())
        }).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited SQLite child; family publication dispatch fingerprints schema before reading member view"]
fn publication_phase_dispatch_refuses_schema_drift_before_member_read() {
    child::run(
        concat!(
            module_path!(),
            "::publication_phase_dispatch_refuses_schema_drift_before_member_read"
        ),
        120,
        || {
            child::declare();
            with_reference("",|c| {
            schema_v2::initialize_test_reference(c)?;
            c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE; DROP TABLE worktrees; CREATE VIEW worktrees AS SELECT 'destination' AS id, 'ready' AS state")?;
            let changes = c.total_changes();
            assert!(member_phase(c,"destination",None).is_err());
            assert_eq!(c.total_changes(),changes);
            c.execute_batch("ROLLBACK")?;
            Ok(())
        }).unwrap();
        },
    );
}
