use super::*;
use crate::projection::provider::owned_test as child;
use rusqlite::params;

fn id(ch: char) -> String {
    format!("blake3:{}", ch.to_string().repeat(64))
}

fn populated(c: &Connection) {
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    c.execute("INSERT INTO family VALUES(1,?1)", [b"family".as_slice()])
        .unwrap();
    c.execute("INSERT INTO quota VALUES(1,?1)", [42u64])
        .unwrap();
    c.execute(
        "INSERT INTO worktrees VALUES('w','member','database',?1,'ready')",
        [b"marker".as_slice()],
    )
    .unwrap();
    let chunk = id('c');
    let file = id('f');
    let first = id('a');
    c.execute("INSERT INTO chunks VALUES(?1,3)", [&chunk])
        .unwrap();
    c.execute(
        "INSERT INTO manifests VALUES(?1,?2,3,?3)",
        params![first, file, b"payload".as_slice()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO manifest_chunks VALUES(?1,0,?2)",
        params![first, chunk],
    )
    .unwrap();
    c.execute(
        "INSERT INTO roots VALUES('root','w','database','operation','digest',?1)",
        [b"root-payload".as_slice()],
    )
    .unwrap();
    c.execute("INSERT INTO root_manifests VALUES('root',?1)", [&first])
        .unwrap();
    c.execute(
        "INSERT INTO reservations VALUES('reserve','w','operation',8)",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO installations VALUES('reserve',?1,?2,NULL,0)",
        params![file, b"stage".as_slice()],
    )
    .unwrap();
    c.execute("INSERT INTO sweep VALUES('chunks',?1,3)", [&id('d')])
        .unwrap();
}

#[test]
#[ignore = "owned audited Windows SQLite child; exact family schema2 migration"]
fn populated_migration_accepts_two_representations_and_exact_replay() {
    child::run(
        concat!(
            module_path!(),
            "::populated_migration_accepts_two_representations_and_exact_replay"
        ),
        120,
        || {
            child::declare();
            with_reference(SQL1, |c| {
                populated(c);
                c.execute_batch("BEGIN IMMEDIATE")?;
                assert_eq!(
                    migrate(c, &Cancellation::default())?,
                    MigrationEffect::Migrated
                );
                assert_eq!(schema_inventory(c)?, reference_inventory()?);
                let file = id('f');
                let second = id('b');
                c.execute(
                    "INSERT INTO manifests VALUES(?1,?2,3,?3)",
                    params![second, file, b"second".as_slice()],
                )?;
                c.execute("INSERT INTO root_manifests VALUES('root',?1)", [&second])?;
                c.execute(
                    "INSERT INTO exact_installations VALUES('reserve',?1,?2,3,?3,NULL,0)",
                    params![second, file, b"exact-stage".as_slice()],
                )?;
                assert!(c
                    .execute(
                        "INSERT INTO exact_installations VALUES('foreign',?1,?2,3,?3,NULL,0)",
                        params![id('e'), file, b"foreign".as_slice()],
                    )
                    .is_err());
                assert_eq!(
                    c.query_row(
                        "SELECT count(*) FROM manifests WHERE file_id=?1",
                        [&file],
                        |r| r.get::<_, u64>(0)
                    )?,
                    2
                );
                assert_eq!(
                    c.query_row("SELECT payload FROM manifests WHERE id=?1", [&id('a')], |r| r.get::<_, Vec<u8>>(0))?,
                    b"payload".to_vec()
                );
                assert_eq!(
                    c.query_row("SELECT chunk_id FROM manifest_chunks WHERE manifest_id=?1 AND ordinal=0", [&id('a')], |r| r.get::<_, String>(0))?,
                    id('c')
                );
                assert_eq!(
                    c.query_row("SELECT payload FROM installations WHERE reservation='reserve' AND file_id=?1", [&file], |r| r.get::<_, Vec<u8>>(0))?,
                    b"stage".to_vec()
                );
                assert_eq!(
                    c.query_row("SELECT payload FROM roots WHERE id='root'", [], |r| r.get::<_, Vec<u8>>(0))?,
                    b"root-payload".to_vec()
                );
                assert!(c
                    .execute(
                        "INSERT INTO manifests VALUES(?1,?2,4,?3)",
                        params![id('e'), file, b"bad-length".as_slice()],
                    )
                    .is_err());
                require_foreign_keys(c)?;
                c.execute_batch("COMMIT")?;
                c.execute_batch("BEGIN DEFERRED")?;
                let before = c.total_changes();
                assert_eq!(
                    migrate(c, &Cancellation::default())?,
                    MigrationEffect::AlreadyV2
                );
                assert_eq!(c.total_changes(), before);
                for (table, count) in [
                    ("family", 1),
                    ("quota", 1),
                    ("worktrees", 1),
                    ("chunks", 1),
                    ("manifests", 2),
                    ("manifest_chunks", 1),
                    ("roots", 1),
                    ("root_manifests", 2),
                    ("reservations", 1),
                    ("installations", 1),
                    ("sweep", 1),
                    ("exact_installations", 1),
                ] {
                    let actual: u64 =
                        c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
                    assert_eq!(actual, count, "{table}");
                }
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite child; rollback and foreign schema refusal"]
fn rollback_restores_v1_and_foreign_schema_is_refused_before_ddl() {
    child::run(
        concat!(
            module_path!(),
            "::rollback_restores_v1_and_foreign_schema_is_refused_before_ddl"
        ),
        120,
        || {
            child::declare();
            with_reference(SQL1, |c| {
                populated(c);
                let original = schema_inventory(c)?;
                c.execute_batch("BEGIN IMMEDIATE")?;
                let cancelled = Cancellation::default();
                cancelled.cancel();
                assert!(migrate(c, &cancelled).is_err());
                assert_eq!(schema_inventory(c)?, original);
                assert_eq!(
                    migrate(c, &Cancellation::default())?,
                    MigrationEffect::Migrated
                );
                c.execute_batch("ROLLBACK")?;
                assert_eq!(schema_inventory(c)?, original);
                assert_eq!(
                    c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
                    1
                );
                c.execute_batch("CREATE TABLE foreign_table(value TEXT) STRICT; BEGIN IMMEDIATE")?;
                assert!(migrate(c, &Cancellation::default()).is_err());
                assert_eq!(
                    c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
                    1
                );
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
            for oversized_chunk in [true, false] {
                with_reference(SQL1, |c| {
                    populated(c);
                    let huge = "x".repeat(1024 * 1024);
                    if oversized_chunk {
                        c.execute("INSERT INTO chunks VALUES(?1,3)", [&huge])?;
                        c.execute(
                            "INSERT INTO manifest_chunks VALUES(?1,1,?2)",
                            params![id('a'), huge],
                        )?;
                    } else {
                        c.execute(
                            "INSERT INTO roots VALUES(?1,'w','database','operation','digest',?2)",
                            params![huge, b"root".as_slice()],
                        )?;
                        c.execute(
                            "INSERT INTO root_manifests VALUES(?1,?2)",
                            params![huge, id('a')],
                        )?;
                    }
                    require_foreign_keys(c)?;
                    let original = schema_inventory(c)?;
                    c.execute_batch("BEGIN IMMEDIATE")?;
                    let changes = c.total_changes();
                    let refusal = migrate(c, &Cancellation::default()).unwrap_err();
                    assert!(refusal.to_string().contains("relation key exceeds bound"));
                    assert_eq!(c.total_changes(), changes);
                    assert_eq!(schema_inventory(c)?, original);
                    assert_eq!(
                        c.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?,
                        1
                    );
                    c.execute_batch("ROLLBACK")?;
                    Ok(())
                })
                .unwrap();
            }
        },
    );
}
