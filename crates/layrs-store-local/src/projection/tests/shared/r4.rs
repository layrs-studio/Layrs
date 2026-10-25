use super::*;

struct MarkerStop;
impl CreationFaults for MarkerStop {
    fn check(&self, point: CreationPoint) -> io::Result<()> {
        if point == CreationPoint::MarkerDurable {
            Err(io::Error::other("marker durable"))
        } else {
            Ok(())
        }
    }
}

fn reject_all(handle: &ProjectionRecovery) {
    assert!(handle.recover_databases().is_err());
    assert!(handle.resume_initialization().is_err());
    assert!(handle.resume_installation().is_err());
    assert!(handle.resume_namespace().is_err());
    assert!(handle.resume_refresh().is_err());
    assert!(handle.resume_restore().is_err());
    assert!(handle.resume_rebase().is_err());
    assert!(handle.cancel_shared_reservation().is_err());
}

#[test]
fn inspected_binding_refuses_independently_valid_replacement_even_with_same_logical_ids() {
    for same_ids in [false, true] {
        let f = Fixture::new();
        let a_family = family(&f);
        let a = member(&f, &a_family, "live", false);
        let root = a.checkout_root().to_owned();
        fs::write(root.join("user-data"), b"original user data").unwrap();
        let a_binding = ProjectionStore::inspect_recovery(&root).unwrap().binding;
        // Preserve A intact. B is independently created at the exact same path.
        let retained = f.0.join("retained-a");
        fs::rename(&root, &retained).unwrap();
        fs::create_dir(&root).unwrap();
        let family_root = f.0.join("family-b");
        fs::create_dir(&family_root).unwrap();
        let mut identity = a_family.identity().clone();
        if !same_ids {
            identity.store_id = StoreId::parse("store:replacement").unwrap();
        }
        let b_family = PartialStoreFamily::create(
            family_root,
            identity,
            SharedQuota {
                bytes: 64 * 1024 * 1024,
            },
        )
        .unwrap();
        let id = if same_ids {
            a.identity().worktree_id.clone()
        } else {
            WorktreeId::parse("worktree:replacement").unwrap()
        };
        assert!(
            b_family
                .create_worktree_with_faults(
                    &root,
                    id.clone(),
                    hydration().0,
                    ProjectionSelection::All,
                    &MarkerStop
                )
                .is_err()
        );
        let b = ProjectionStore::inspect_recovery(&root).unwrap();
        assert_eq!(b.recovery, vec![ProjectionRecoveryReason::Initializing]);
        assert_eq!(a_binding.identity == b.binding.identity, same_ids);
        assert_ne!(a_binding.checkout_identity, b.binding.checkout_identity);
        let db = b_family.member_metadata(&id).join("projection.sqlite3");
        r2::kill_spilled_writer(&db, "owner", &f.0.join("ready"));
        let before = fs::read(&db).unwrap();
        assert!(ProjectionStore::open_bound_recovery(&root, &a_binding).is_err());
        assert!(ProjectionStore::recover_databases(&root, &a_binding).is_err());
        assert!(ProjectionStore::resume_initialization(&root, &a_binding).is_err());
        r2::assert_unchanged_database(&fs::read(&db).unwrap(), &before);
        let recovered = ProjectionStore::recover_databases(&root, &b.binding).unwrap();
        assert_eq!(
            recovered.recovery,
            vec![ProjectionRecoveryReason::Initializing]
        );
        assert!(
            ProjectionStore::resume_initialization(&root, &b.binding)
                .unwrap()
                .recovery
                .is_empty()
        );
        assert_eq!(
            fs::read(retained.join("user-data")).unwrap(),
            b"original user data"
        );
    }
}

#[test]
fn retained_handle_refuses_legacy_database_replacement_with_identical_sql_bytes() {
    let f = Fixture::new();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    let inspected = ProjectionStore::inspect_recovery(&f.0).unwrap();
    let handle = ProjectionStore::open_bound_recovery(&f.0, &inspected.binding).unwrap();
    let db = f.0.join(".layrs/metadata/projection.sqlite3");
    let old = db.with_extension("retained");
    fs::rename(&db, &old).unwrap();
    fs::copy(&old, &db).unwrap();
    let replacement = ProjectionStore::inspect_recovery(&f.0).unwrap();
    assert_eq!(
        replacement.binding.marker_digest,
        inspected.binding.marker_digest
    );
    assert_ne!(
        replacement.binding.database_identity,
        inspected.binding.database_identity
    );
    let before = fs::read(&db).unwrap();
    reject_all(&handle);
    assert!(ProjectionStore::recover_databases(store.checkout_root(), &inspected.binding).is_err());
    r2::assert_unchanged_database(&fs::read(&db).unwrap(), &before);
}

#[test]
fn retained_handle_refuses_valid_family_marker_drift_before_hot_catalog_rollback() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let before = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
    let handle =
        ProjectionStore::open_bound_recovery(store.checkout_root(), &before.binding).unwrap();
    let catalog = family.root().join("metadata/catalog.sqlite3");
    let retained = catalog.with_extension("retained");
    fs::rename(&catalog, &retained).unwrap();
    fs::copy(&retained, &catalog).unwrap();
    // Model an independently valid family catalog replacement. The locator
    // still has identical logical identity/incarnation/root/private bindings.
    let pin = crate::projection::file::DatabasePin::open(&catalog).unwrap();
    let marker_path = family.root().join("FORMAT");
    let mut marker: serde_json::Value =
        serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
    marker["databaseIdentity"] = serde_json::Value::String(pin.identity().to_owned());
    drop(pin);
    let bytes = layrs_core::scm::canonical_json(&marker).unwrap();
    let c = database::connect(&catalog, true).unwrap();
    c.execute("UPDATE family SET marker=?1 WHERE singleton=1", [&bytes])
        .unwrap();
    drop(c);
    fs::write(&marker_path, bytes).unwrap();
    let replacement = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
    assert_eq!(
        before.binding.marker_digest,
        replacement.binding.marker_digest
    );
    assert_eq!(
        before.binding.database_identity,
        replacement.binding.database_identity
    );
    assert_ne!(
        before.binding.family_marker_digest,
        replacement.binding.family_marker_digest
    );
    r2::kill_spilled_writer(&catalog, "catalog", &f.0.join("ready"));
    let spilled = fs::read(&catalog).unwrap();
    reject_all(&handle);
    assert!(ProjectionStore::recover_databases(store.checkout_root(), &before.binding).is_err());
    r2::assert_unchanged_database(&fs::read(&catalog).unwrap(), &spilled);
    assert!(
        ProjectionStore::recover_databases(store.checkout_root(), &replacement.binding)
            .unwrap()
            .recovery
            .is_empty()
    );
}

fn assert_schema5_fixture(db: &Path) {
    let c = database::connect(db, false).unwrap();
    let version: u32 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 5);
    let selections: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_selections')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!selections, "recovery must not create the selection table");
    let reference = rusqlite::Connection::open_in_memory().unwrap();
    reference.execute_batch(database::SQL).unwrap();
    assert!(
        database::schema(&c).unwrap() == database::schema(&reference).unwrap(),
        "fixture must have the exact historical schema-5 fingerprint"
    );
}

fn make_schema5_fixture(db: &Path, shared: bool) {
    let mut c = database::connect(db, true).unwrap();
    let tx = c.transaction().unwrap();
    tx.execute_batch("DROP TABLE projection_selections;")
        .unwrap();
    if shared {
        // Also exercise refusal of an unsupported legacy private schema behind
        // a format-4 locator. Remove the empty draft-7 tables, so this really
        // is schema 5 rather than just an inconsistent user_version label.
        tx.execute_batch(
            "DROP TABLE projection_content;
            DROP TABLE projection_claims;
            DROP TABLE projection_budgets;
            DROP TABLE projection_prejournal;
            DROP TABLE projection_preparations;
            DROP TABLE projection_observations;
            DROP TABLE projection_imports;
            DROP TABLE projection_conversion;",
        )
        .unwrap();
    }
    tx.execute_batch("PRAGMA user_version=5;").unwrap();
    tx.commit().unwrap();
    drop(c);
    assert_schema5_fixture(db);
}

#[test]
fn every_recovery_entry_refuses_schema5_without_writes_or_migration() {
    for shared in [false, true] {
        let f = Fixture::new();
        let store = if shared {
            let family = family(&f);
            member(&f, &family, "a", false)
        } else {
            ProjectionStore::create(&f.0, identity(), snapshot()).unwrap()
        };
        let root = store.checkout_root();
        let inspected = ProjectionStore::inspect_recovery(root).unwrap();
        let handle = ProjectionStore::open_bound_recovery(root, &inspected.binding).unwrap();
        let db = store.marker.metadata_path(root).join("projection.sqlite3");
        make_schema5_fixture(&db, shared);
        let before = fs::read(&db).unwrap();
        reject_all(&handle);
        assert!(ProjectionStore::inspect_recovery(root).is_err());
        assert!(ProjectionStore::open_for_recovery(root).is_err());
        assert!(ProjectionStore::open_bound_recovery(root, &inspected.binding).is_err());
        assert!(ProjectionStore::recover_databases(root, &inspected.binding).is_err());
        assert!(ProjectionStore::resume_initialization(root, &inspected.binding).is_err());
        assert!(ProjectionStore::resume_installation(root).is_err());
        assert!(ProjectionStore::resume_namespace(root).is_err());
        assert!(ProjectionStore::resume_refresh(root).is_err());
        assert!(ProjectionStore::resume_restore(root).is_err());
        assert!(ProjectionStore::resume_rebase(root).is_err());
        assert!(store.cancel_shared_reservation().is_err());
        assert_schema5_fixture(&db);
        r2::assert_unchanged_database(&fs::read(&db).unwrap(), &before);
    }

    // Discriminating positive control: ordinary open would migrate this exact
    // historical schema. Create its own correctly bound root, never copy a DB
    // or FORMAT from the negative fixtures to manufacture another identity.
    let control = Fixture::new();
    let store = ProjectionStore::create(&control.0, identity(), snapshot()).unwrap();
    let db = store
        .marker
        .metadata_path(&control.0)
        .join("projection.sqlite3");
    make_schema5_fixture(&db, false);
    let before = fs::read(&db).unwrap();
    let migrated = ProjectionStore::open(&control.0).unwrap();
    assert_eq!(migrated.identity(), store.identity());
    assert_eq!(migrated.state().unwrap().snapshot, snapshot());
    let c = database::connect(&db, false).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        6
    );
    assert!(
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_selections')",
            [],
            |r| r.get::<_, bool>(0),
        )
        .unwrap()
    );
    drop(c);
    assert!(
        fs::read(&db).unwrap() != before,
        "positive control must actually migrate"
    );
}

#[test]
fn matching_bound_handle_resumes_installation_using_the_accepted_private_journal() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    assert!(
        store
            .install_projection_with_faults(
                0,
                &hydration().1,
                &mut provide,
                &StopInstall(InstallPoint::JournalDurable)
            )
            .is_err()
    );
    let inspected = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
    let handle =
        ProjectionStore::open_bound_recovery(store.checkout_root(), &inspected.binding).unwrap();
    assert_eq!(handle.resume_installation().unwrap().generation, 1);
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"alpha"
    );
    store.fsck().unwrap();
}

#[test]
fn recovery_binding_has_strict_canonical_digest_and_native_identity_bounds() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let expected = ProjectionStore::inspect_recovery(store.checkout_root())
        .unwrap()
        .binding;
    for mutate in 0..4 {
        let mut bad = expected.clone();
        match mutate {
            0 => bad.marker_digest = "A".repeat(64),
            1 => bad.family_marker_digest = Some(format!("blake3:{}", "A".repeat(64))),
            2 => bad.database_identity = "x".repeat(513),
            _ => bad.checkout_identity.clear(),
        }
        assert!(ProjectionStore::open_bound_recovery(store.checkout_root(), &bad).is_err());
        assert!(ProjectionStore::recover_databases(store.checkout_root(), &bad).is_err());
    }
    let mut json = serde_json::to_value(&expected).unwrap();
    json["unexpected"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<RecoveryBinding>(json).is_err());
}
