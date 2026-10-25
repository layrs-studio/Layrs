use super::*;
pub(in crate::projection::provider) mod fixture;
use super::super::{database_io::BareConnection, process_profile::tests::child};
use fixture::Fixture;

fn with_fixture(
    label: &str,
    executable: bool,
    phase: MountPhase,
    action: impl FnOnce(&Connection, &Fixture),
) {
    let f = Fixture::new(label, executable);
    let c = BareConnection::reference().unwrap();
    f.populate(c.connection(), phase);
    action(c.connection(), &f);
    c.finish(Ok(())).unwrap();
}

#[test]
#[ignore = "owned audited Windows SQLite process; no filesystem fixture"]
fn exact_bootstrap_schema_phases_sources_and_all_39_tables() {
    child::run(
        concat!(
            module_path!(),
            "::exact_bootstrap_schema_phases_sources_and_all_39_tables"
        ),
        120,
        || {
            child::declare();
            for label in ["G0", "G1", "G2"] {
                for phase in [
                    MountPhase::Allocating,
                    MountPhase::ControlPrepared,
                    MountPhase::ControlPublished,
                ] {
                    with_fixture(label, false, phase, |c, f| {
                        let observed = private(c, &f.allocation, Some(&f.prepared)).unwrap();
                        assert!(
                            matches!(observed,PrivateValidation::Initialized(m) if m.phase==phase)
                        );
                        let tables:Vec<String>=c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND substr(name,1,7) COLLATE NOCASE <> 'sqlite_' ORDER BY name").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
                        let mut expected: Vec<_> = EMPTY_TABLES
                            .into_iter()
                            .chain(SINGLE_TABLES)
                            .chain(["projection_provider_descriptors"])
                            .map(str::to_owned)
                            .collect();
                        expected.sort();
                        assert_eq!(tables, expected);
                        assert_eq!(tables.len(), 39);
                    });
                }
            }
            with_fixture("G2", true, MountPhase::ControlPrepared, |c, f| {
                private(c, &f.allocation, Some(&f.prepared)).unwrap();
                c.execute(
                    "UPDATE projection_provider_descriptors SET executable=0",
                    [],
                )
                .unwrap();
                let error = private(c, &f.allocation, Some(&f.prepared))
                    .err()
                    .unwrap()
                    .to_string();
                assert!(
                    error.contains("descriptor row projection/order differs"),
                    "{error}"
                );
            });
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; corrupt-row emptiness fixtures, not accepted records"]
fn every_nonbootstrap_table_is_fenced_before_record_or_fk_interpretation() {
    child::run(
        concat!(
            module_path!(),
            "::every_nonbootstrap_table_is_fenced_before_record_or_fk_interpretation"
        ),
        120,
        || {
            child::declare();
            for table in EMPTY_TABLES {
                with_fixture("G1", false, MountPhase::Allocating, |c, f| {
                    private(c, &f.allocation, Some(&f.prepared)).unwrap();
                    // Populate exactly one deliberately uninterpretable row. This
                    // tests the earlier empty-table gate, not valid lineage/FKs.
                    c.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;")
                        .unwrap();
                    let info: Vec<(String, String)> = c
                        .prepare(&format!("PRAGMA table_info({table})"))
                        .unwrap()
                        .query_map([], |r| Ok((r.get(1)?, r.get(2)?)))
                        .unwrap()
                        .collect::<rusqlite::Result<_>>()
                        .unwrap();
                    let names = info
                        .iter()
                        .map(|r| r.0.as_str())
                        .collect::<Vec<_>>()
                        .join(",");
                    let values = info
                        .iter()
                        .map(|r| match r.1.as_str() {
                            "INTEGER" => "1",
                            "TEXT" => "'invalid-evidence'",
                            "BLOB" => "x'7b7d'",
                            other => panic!("unexpected fixture type {other}"),
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    c.execute(
                        &format!("INSERT INTO {table}({names}) VALUES({values})"),
                        [],
                    )
                    .unwrap();
                    let error = private(c, &f.allocation, Some(&f.prepared))
                        .err()
                        .unwrap()
                        .to_string();
                    assert!(
                        error.contains(&format!("requires empty {table}")),
                        "{table}: {error}"
                    );
                });
            }
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; exact source/descriptor/identity mutations"]
fn independent_schema_identity_history_selection_and_descriptor_mutations() {
    child::run(
        concat!(
            module_path!(),
            "::independent_schema_identity_history_selection_and_descriptor_mutations"
        ),
        120,
        || {
            child::declare();
            let cases = [
                ("PRAGMA user_version=7", "requires exact private schema8"),
                (
                    "UPDATE projection_meta SET identity=x'7b7d'",
                    "SQL identity differs",
                ),
                (
                    "UPDATE projection_backend SET backend='ordinary',mount_id=NULL",
                    "backend/mount binding differs",
                ),
                (
                    "UPDATE projection_snapshots SET digest=replace(digest,substr(digest,1,1),'0')",
                    "snapshot anchor differs",
                ),
                (
                    "UPDATE projection_selections SET payload=x'7b7d'",
                    "selection digest mismatch",
                ),
                (
                    "UPDATE projection_provider_descriptors SET bytes=1",
                    "descriptor row projection/order differs",
                ),
                (
                    "UPDATE projection_provider_descriptors SET executable=1",
                    "descriptor row projection/order differs",
                ),
                (
                    "UPDATE projection_provider_descriptors SET file_id='blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                    "descriptor row projection/order differs",
                ),
                (
                    "UPDATE projection_provider_descriptors SET manifest_id='blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                    "descriptor row projection/order differs",
                ),
                (
                    "UPDATE projection_provider_descriptors SET catalog_id='blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                    "descriptor row projection/order differs",
                ),
                (
                    "UPDATE projection_provider_descriptors SET path='OTHER.md'",
                    "source descriptor does not match selected snapshot target",
                ),
                (
                    "UPDATE projection_provider_descriptors SET manifest=x'5b5d'",
                    "invalid length 0, expected struct FileManifestWire with 6 elements at line 1 column 2",
                ),
                (
                    "UPDATE projection_provider_mounts SET digest='blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                    "mount row projection differs",
                ),
            ];
            for (sql, cause) in cases {
                with_fixture("G1", false, MountPhase::ControlPrepared, |c, f| {
                    c.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
                    c.execute_batch(sql).unwrap();
                    let error = private(c, &f.allocation, Some(&f.prepared))
                        .err()
                        .unwrap()
                        .to_string();
                    assert!(
                        error.contains(cause),
                        "mutation {sql}: wanted {cause}; {error}"
                    );
                });
            }
            for field in [
                "storeIdentity",
                "initialSnapshotDigest",
                "shared.databaseIdentity",
            ] {
                with_fixture("G1", false, MountPhase::Allocating, |c, f| {
                    let mut marker = serde_json::to_value(&f.allocation.format).unwrap();
                    match field {
                        "storeIdentity" => {
                            marker[field] = "fs:v1:0000000000000001:0000000000000031".into()
                        }
                        "initialSnapshotDigest" => marker[field] = "0".repeat(64).into(),
                        _ => marker["shared"]["databaseIdentity"] = "1:0:12".into(),
                    }
                    // Independently refreshed canonical SQL FORMAT bytes; no stale
                    // wrapper digest is used to stand in for exact identity checking.
                    let marker: crate::projection::format::Marker =
                        serde_json::from_value(marker).unwrap();
                    c.execute(
                        "UPDATE projection_meta SET identity=?1",
                        [marker.bytes().unwrap()],
                    )
                    .unwrap();
                    let error = private(c, &f.allocation, Some(&f.prepared))
                        .err()
                        .unwrap()
                        .to_string();
                    assert!(error.contains("SQL identity differs"), "{field}: {error}");
                });
            }
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; FK-valid extra catalog/mount before phase acceptance"]
fn fk_valid_second_catalog_mount_and_wrong_backend_target_are_refused() {
    child::run(
        concat!(
            module_path!(),
            "::fk_valid_second_catalog_mount_and_wrong_backend_target_are_refused"
        ),
        120,
        || {
            child::declare();
            with_fixture("G1", false, MountPhase::Allocating, |c, f| {
                private(c, &f.allocation, Some(&f.prepared)).unwrap();
                let mut second = Fixture::new("G0", false);
                second.allocation.transaction_id = "00000000000000000000000000000002".into();
                second.allocation.validate_fields().unwrap();
                let catalog = second.source.catalog();
                let mount = second.mount(MountPhase::Allocating);
                c.execute(
                    "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?3,?4)",
                    rusqlite::params![
                        second.source.commitment().catalog_id,
                        catalog.source_digest,
                        catalog.selection_digest,
                        catalog.encode().unwrap()
                    ],
                )
                .unwrap();
                c.execute(
                    "INSERT INTO projection_provider_mounts VALUES(?1,?2,'allocating',?3,?4)",
                    rusqlite::params![
                        mount.transaction_id,
                        mount.catalog_id,
                        mount.encode().unwrap(),
                        mount.digest().unwrap()
                    ],
                )
                .unwrap();
                c.execute(
                    "UPDATE projection_backend SET mount_id=?1",
                    [mount.transaction_id.as_str()],
                )
                .unwrap();
                let violations: u64 = c
                    .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(violations, 0);
                // This is a cardinality + wrong-binding fixture, not a claim that
                // exactly one predicate was isolated or that the extra source is
                // accepted history in this Worktree.
                let error = private(c, &f.allocation, Some(&f.prepared))
                    .err()
                    .unwrap()
                    .to_string();
                assert!(
                    error.contains("exactly one projection_provider_catalogs row"),
                    "{error}"
                );
            });
        },
    );
}

mod r2;
