use super::*;
use crate::projection::provider::{
    database_io::{self, ReadState},
    recovery::tests::fixture::NativeFixture,
};
use crate::projection::shared::provider_access::{FamilyAccess, Registration};

#[test]
#[ignore = "owned audited Windows SQLite test process; literal-prefix and bounded inventory"]
fn literal_internal_prefix_private_schema8_and_version0() {
    child::run(
        concat!(
            module_path!(),
            "::literal_internal_prefix_private_schema8_and_version0"
        ),
        120,
        || {
            child::declare();
            let f = Fixture::new("G0", false);
            let c = BareConnection::reference().unwrap();
            assert!(
                database_io::schema_inventory(c.connection())
                    .unwrap()
                    .is_empty()
            );
            assert!(matches!(
                private(c.connection(), &f.allocation, None).unwrap(),
                PrivateValidation::Uninitialized
            ));
            c.connection().execute_batch("CREATE TABLE sqliteXextra(value TEXT); INSERT INTO sqliteXextra VALUES('retained');").unwrap();
            assert_eq!(
                database_io::schema_inventory(c.connection()).unwrap()[0].1,
                "sqliteXextra"
            );
            let error = private(c.connection(), &f.allocation, None)
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("requires exact private schema8"), "{error}");
            c.finish(Ok(())).unwrap();
            with_fixture("G1", false, MountPhase::Allocating, |c, f| {
                private(c, &f.allocation, Some(&f.prepared)).unwrap();
                c.execute_batch("CREATE TABLE sqliteXextra(value TEXT UNIQUE); INSERT INTO sqliteXextra VALUES('retained');").unwrap();
                let inventory = database_io::schema_inventory(c).unwrap();
                assert!(inventory.iter().any(|r| r.1 == "sqliteXextra"));
                assert!(
                    !inventory
                        .iter()
                        .any(|r| r.1.starts_with("sqlite_autoindex_"))
                );
                let error = private(c, &f.allocation, Some(&f.prepared))
                    .err()
                    .unwrap()
                    .to_string();
                assert!(error.contains("schema fingerprint differs"), "{error}");
            });
            for mode in ["count", "utf8-bytes"] {
                let c = BareConnection::reference().unwrap();
                if mode == "count" {
                    for i in 0..64 {
                        c.connection()
                            .execute_batch(&format!("CREATE TABLE t{i}(x INTEGER)"))
                            .unwrap();
                    }
                    assert_eq!(
                        database_io::schema_inventory(c.connection()).unwrap().len(),
                        64
                    );
                    c.connection()
                        .execute_batch("CREATE TABLE over(x INTEGER)")
                        .unwrap();
                } else {
                    let sql = format!("CREATE TABLE t(x INTEGER /*{}*/)", "é".repeat(33000));
                    assert!(sql.chars().count() < 65536);
                    c.connection().execute_batch(&sql).unwrap();
                }
                let error = database_io::schema_inventory(c.connection())
                    .err()
                    .unwrap()
                    .to_string();
                assert!(
                    error.contains("schema exceeds validation budget"),
                    "{mode}: {error}"
                );
                c.finish(Ok(())).unwrap();
            }
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite test process; independent rowset/cardinality predicates"]
fn independent_catalog_mount_backend_and_descriptor_rowsets() {
    child::run(
        concat!(
            module_path!(),
            "::independent_catalog_mount_backend_and_descriptor_rowsets"
        ),
        120,
        || {
            child::declare();
            for case in [
                "catalog",
                "mount",
                "backend-missing",
                "backend-ordinary",
                "backend-wrong",
                "descriptor-add",
                "descriptor-remove",
            ] {
                with_fixture("G1", false, MountPhase::Allocating, |c, f| {
                    private(c, &f.allocation, Some(&f.prepared)).unwrap();
                    let expected = match case {
                        "catalog" => {
                            let other = Fixture::new("G0", false);
                            let q = other.source.catalog();
                            c.execute(
                                "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?3,?4)",
                                rusqlite::params![
                                    other.source.commitment().catalog_id,
                                    q.source_digest,
                                    q.selection_digest,
                                    q.encode().unwrap()
                                ],
                            )
                            .unwrap();
                            "exactly one projection_provider_catalogs row"
                        }
                        "mount" => {
                            let mut m = f.mount(MountPhase::Allocating);
                            m.transaction_id = "00000000000000000000000000000002".into();
                            c.execute("INSERT INTO projection_provider_mounts VALUES(?1,?2,'allocating',?3,?4)",rusqlite::params![m.transaction_id,m.catalog_id,m.encode().unwrap(),m.digest().unwrap()]).unwrap();
                            "exactly one projection_provider_mounts row"
                        }
                        "backend-missing" => {
                            c.execute("DELETE FROM projection_backend", []).unwrap();
                            "exactly one projection_backend row"
                        }
                        "backend-ordinary" => {
                            c.execute(
                                "UPDATE projection_backend SET backend='ordinary',mount_id=NULL",
                                [],
                            )
                            .unwrap();
                            "backend/mount binding differs"
                        }
                        "backend-wrong" => {
                            c.execute_batch("PRAGMA foreign_keys=OFF; UPDATE projection_backend SET mount_id='wrong-target';").unwrap();
                            "backend/mount binding differs"
                        }
                        "descriptor-add" => {
                            c.execute("INSERT INTO projection_provider_descriptors SELECT catalog_id,'zzz.md',file_id,bytes,executable,manifest_id,manifest FROM projection_provider_descriptors",[]).unwrap();
                            "extra source descriptor"
                        }
                        _ => {
                            c.execute("DELETE FROM projection_provider_descriptors", [])
                                .unwrap();
                            "missing or unsupported selected source descriptor"
                        }
                    };
                    let fk: u64 = c
                        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    if case == "backend-wrong" {
                        assert_eq!(
                            fk, 1,
                            "deliberate one dangling backend FK, not a FK-valid control"
                        );
                    } else {
                        assert_eq!(
                            fk, 0,
                            "{case} SQL FKs must remain valid; extra mount is unaccepted DATA"
                        );
                    }
                    let error = private(c, &f.allocation, Some(&f.prepared))
                        .err()
                        .unwrap()
                        .to_string();
                    assert!(error.contains(expected), "{case}: {error}");
                });
            }
        },
    );
}

fn family_read(f: &NativeFixture) -> ReadState<Registration> {
    let family = FamilyAccess::open(&f.family).unwrap();
    let _locks = family
        .locks(&f.allocation.format.identity.worktree_id, false)
        .unwrap();
    database_io::read(&family.catalog, |c| {
        family.validate_catalog(c, &f.allocation)
    })
    .unwrap()
}
#[test]
#[ignore = "owned audited Windows/VFS host and disposable parent; independent family negatives, fixtures retained"]
fn family_schema_identity_registry_and_owned_dependencies() {
    child::run(
        concat!(
            module_path!(),
            "::family_schema_identity_registry_and_owned_dependencies"
        ),
        240,
        || {
            child::declare();
            for case in [
                "schema",
                "version",
                "marker",
                "target-marker",
                "target-path",
                "target-database",
                "target-ready",
                "roots",
                "reservation",
                "installation-with-reservation",
            ] {
                let f = NativeFixture::create(MountPhase::Allocating);
                assert!(matches!(
                    family_read(&f),
                    ReadState::Valid(Registration::Initializing)
                ));
                let c = Connection::open(f.family.join("metadata/catalog.sqlite3")).unwrap();
                c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
                let owner = f.allocation.format.identity.worktree_id.as_str();
                let expected = match case {
                    "schema" => {
                        c.execute_batch("CREATE TABLE sqliteXextra(value TEXT); INSERT INTO sqliteXextra VALUES('retained')").unwrap();
                        "catalog fingerprint differs"
                    }
                    "version" => {
                        c.execute_batch("PRAGMA user_version=2").unwrap();
                        "requires exact catalog schema1"
                    }
                    "marker" => {
                        c.execute("UPDATE family SET marker=x'7b7d'", []).unwrap();
                        "family SQL identity differs"
                    }
                    "target-marker" => {
                        c.execute("UPDATE worktrees SET marker=x'7b7d' WHERE id=?1", [owner])
                            .unwrap();
                        "not exact initializing ownership"
                    }
                    "target-path" => {
                        c.execute(
                            r"UPDATE worktrees SET metadata_path='C:\wrong' WHERE id=?1",
                            [owner],
                        )
                        .unwrap();
                        "not exact initializing ownership"
                    }
                    "target-database" => {
                        c.execute(
                            "UPDATE worktrees SET database_identity='1:0:99' WHERE id=?1",
                            [owner],
                        )
                        .unwrap();
                        "not exact initializing ownership"
                    }
                    "target-ready" => {
                        c.execute("UPDATE worktrees SET state='ready' WHERE id=?1", [owner])
                            .unwrap();
                        "not exact initializing ownership"
                    }
                    "roots" => {
                        c.execute("INSERT INTO roots VALUES('target-root',?1,?2,'fixture','digest',x'7b7d')",rusqlite::params![owner,f.allocation.private.database_file_identity]).unwrap();
                        "accepted roots/reservations/installations"
                    }
                    _ => {
                        c.execute(
                            "INSERT INTO reservations VALUES('target-budget',?1,'fixture',1)",
                            [owner],
                        )
                        .unwrap();
                        if case == "installation-with-reservation" {
                            c.execute("INSERT INTO installations VALUES('target-budget','file',x'7b7d',NULL,0)",[]).unwrap();
                        }
                        "accepted roots/reservations/installations"
                    }
                };
                assert_eq!(
                    c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                        .get::<_, u64>(
                        0
                    ))
                    .unwrap(),
                    0
                );
                c.close().unwrap();
                let before = crate::projection::provider::database_io::tests::support::witness(
                    &f.family.join("metadata/catalog.sqlite3"),
                );
                let ReadState::Rejected {
                    cause,
                    pager_read: true,
                } = family_read(&f)
                else {
                    panic!("{case}: known semantic rejection required")
                };
                assert!(cause.to_string().contains(expected), "{case}: {cause}");
                assert_eq!(
                    crate::projection::provider::database_io::tests::support::witness(
                        &f.family.join("metadata/catalog.sqlite3")
                    ),
                    before
                );
                // installations necessarily has its reservation. The earlier reservation
                // fence is the deliberate dependency guard, not an isolated installation predicate.
            }
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; actual unrelated captured member and protected roots remain untouched"]
fn family_allows_unrelated_protected_member_records() {
    child::run(
        concat!(
            module_path!(),
            "::family_allows_unrelated_protected_member_records"
        ),
        180,
        || {
            use crate::projection::provider::database_io::tests::support::witness;
            use layrs_core::{
                projection::{EntryState, ProjectionSelection},
                scm::WorktreeId,
            };
            child::declare();
            let mut protected = Vec::new();
            let owner = WorktreeId::parse("worktree:unrelated").unwrap();
            let f =
                NativeFixture::create_with_family_setup(MountPhase::Allocating, |family, outer| {
                    let checkout = outer.join("unrelated");
                    std::fs::create_dir(&checkout).unwrap();
                    let source = Fixture::new("G1", false);
                    let mut initial = source.source.initial().clone();
                    initial.entries[0].writable = true;
                    let store = family
                        .create_worktree(
                            &checkout,
                            owner.clone(),
                            initial.clone(),
                            ProjectionSelection::All,
                        )
                        .unwrap();
                    let mut materialized = initial;
                    materialized.entries[0].state = EntryState::Materialized;
                    // The literal G1 base is the actual empty file; no remote provider.
                    store
                        .install_projection(0, &materialized, &mut |_, _| Ok(()))
                        .unwrap();
                    std::fs::write(
                        checkout.join("README.md"),
                        b"accepted unrelated local capture",
                    )
                    .unwrap();
                    assert_eq!(store.capture_patch(1).unwrap().sequence, 1);
                    for p in [
                        checkout.join("README.md"),
                        family.member_metadata(&owner).join("projection.sqlite3"),
                    ] {
                        protected.push((p.clone(), witness(&p)));
                    }
                });
            let c = Connection::open(f.family.join("metadata/catalog.sqlite3")).unwrap();
            c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            assert_eq!(
                c.query_row(
                    "SELECT state FROM worktrees WHERE id=?1",
                    [owner.as_str()],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "ready"
            );
            assert!(
                c.query_row(
                    "SELECT count(*) FROM roots WHERE owner=?1",
                    [owner.as_str()],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap()
                    > 0
            );
            // Unrelated in-progress dependencies are opaque to this target-owner
            // validator. These two SQL rows test that scope, not an installation
            // journal or authority for the other member's pending operation.
            c.execute(
                "INSERT INTO reservations VALUES('other-budget',?1,'fixture',1)",
                [owner.as_str()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO installations VALUES('other-budget','file',x'7b7d',NULL,0)",
                [],
            )
            .unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, u64>(
                    0
                ))
                .unwrap(),
                0
            );
            c.close().unwrap();
            let catalog = f.family.join("metadata/catalog.sqlite3");
            let before = witness(&catalog);
            assert!(matches!(
                family_read(&f),
                ReadState::Valid(Registration::Initializing)
            ));
            assert_eq!(witness(&catalog), before);
            for (p, expected) in protected {
                assert_eq!(witness(&p), expected);
            }
        },
    );
}
