use super::*;
use crate::projection::provider;
mod process;
mod revisions;

fn assert_fence<T>(result: Result<T>, expected: &str) {
    match result {
        Err(ProjectionStoreError::Invalid(message)) => assert!(
            message.contains(expected),
            "expected {expected}, got {message}"
        ),
        Err(error) => panic!("expected {expected}, got {error}"),
        Ok(_) => panic!("expected {expected}, operation succeeded"),
    }
}

fn db(store: &ProjectionStore) -> PathBuf {
    store
        .marker
        .metadata_path(store.checkout_root())
        .join("projection.sqlite3")
}
fn version(store: &ProjectionStore) -> u32 {
    database::connect(&db(store), false)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
fn assert_bytes(path: &Path, before: &[u8]) {
    let after = fs::read(path).unwrap();
    assert!(
        before == after,
        "{} changed: {}:{} -> {}:{}",
        path.display(),
        before.len(),
        blake3::hash(before),
        after.len(),
        blake3::hash(&after)
    );
}
struct Stop(ProviderSchemaUpgradePoint);
impl ProviderSchemaUpgradeFaults for Stop {
    fn check(&self, point: ProviderSchemaUpgradePoint) -> io::Result<()> {
        if self.0 == point {
            Err(io::Error::other("schema upgrade interruption"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn explicit_upgrade_preserves_bindings_history_and_ordinary_behavior() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "ordinary", false);
    assert_eq!(version(&store), 7);
    let marker = fs::read(store.checkout_root().join(".layrs/FORMAT")).unwrap();
    let catalog = fs::read(family.root().join("metadata/catalog.sqlite3")).unwrap();
    let binding = ProjectionStore::inspect_recovery(store.checkout_root())
        .unwrap()
        .binding;
    let state = store.state().unwrap();
    ProjectionStore::open(store.checkout_root()).unwrap();
    assert_eq!(version(&store), 7, "ordinary opening must never upgrade");
    store.upgrade_provider_schema(0).unwrap();
    assert_eq!(version(&store), PROJECTION_PROVIDER_SCHEMA_VERSION);
    assert_eq!(store.state().unwrap(), state);
    assert_eq!(
        ProjectionStore::inspect_recovery(store.checkout_root())
            .unwrap()
            .binding,
        binding
    );
    assert_bytes(&store.checkout_root().join(".layrs/FORMAT"), &marker);
    assert_bytes(&family.root().join("metadata/catalog.sqlite3"), &catalog);
    let upgraded = fs::read(db(&store)).unwrap();
    store.upgrade_provider_schema(0).unwrap();
    assert_bytes(&db(&store), &upgraded);
    let opened = ProjectionStore::open(store.checkout_root()).unwrap();
    opened
        .install_projection(0, &hydration().1, &mut provide)
        .unwrap();
    fs::write(
        store.checkout_root().join("a.txt"),
        b"ordinary schema8 edit",
    )
    .unwrap();
    let patch = store.capture_patch(1).unwrap();
    assert_eq!(patch.changes.len(), 1);
    store.fsck().unwrap();
    family.evict_unretained().unwrap();
}

#[test]
fn upgrade_faults_leave_complete_seven_or_complete_eight() {
    for point in [
        ProviderSchemaUpgradePoint::SchemaCreated,
        ProviderSchemaUpgradePoint::BeforeCommit,
        ProviderSchemaUpgradePoint::AfterCommit,
    ] {
        let f = Fixture::new();
        let family = family(&f);
        let store = member(&f, &family, "fault", false);
        let before = store.state().unwrap();
        assert!(
            store
                .upgrade_provider_schema_with_faults(0, &Stop(point))
                .is_err()
        );
        let expected = if point == ProviderSchemaUpgradePoint::AfterCommit {
            8
        } else {
            7
        };
        assert_eq!(version(&store), expected);
        let c = database::connect(&db(&store), false).unwrap();
        database::validate(&c, &store.marker).unwrap();
        let tables: u64 = c.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND (name='projection_backend' OR name LIKE 'projection_provider_%')", [], |r| r.get(0)).unwrap();
        assert_eq!(tables, if expected == 8 { 12 } else { 0 });
        drop(c);
        assert_eq!(store.state().unwrap(), before);
        store.upgrade_provider_schema(0).unwrap();
        store.fsck().unwrap();
    }
}

#[test]
fn durable_install_journal_and_private_budget_require_explicit_recovery_before_upgrade() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "pending-install", false);
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
    let before = fs::read(db(&store)).unwrap();
    assert!(store.upgrade_provider_schema(0).is_err());
    assert_bytes(&db(&store), &before);
    assert!(!store.checkout_root().join("a.txt").exists());
    ProjectionStore::resume_installation(store.checkout_root()).unwrap();
    // The existing journal engine leaves an empty unowned stage directory on
    // completion. Schema upgrade does not infer ownership from its name.
    store.cancel_shared_reservation().unwrap();
    assert!(store.upgrade_provider_schema(1).is_err());
    let stage = store.checkout_root().join(".layrs/projection-staging");
    for entry in fs::read_dir(&stage).unwrap() {
        assert!(
            fs::read_dir(entry.unwrap().path())
                .unwrap()
                .next()
                .is_none()
        );
    }
    fs::rename(&stage, f.0.join("retained-completed-stage")).unwrap();
    store.upgrade_provider_schema(1).unwrap();
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"alpha"
    );
    let access = format::access(&store, SessionMode::Exclusive).unwrap();
    retention::reserve(
        &access.connection,
        &store,
        "capture:upgrade-test",
        "exact-witness",
        &[],
        0,
    )
    .unwrap();
    drop(access);
    let before = fs::read(db(&store)).unwrap();
    assert!(store.upgrade_provider_schema(1).is_err());
    assert_bytes(&db(&store), &before);
    store.cancel_shared_reservation().unwrap();
    store.upgrade_provider_schema(1).unwrap();
}

#[test]
fn upgrade_refuses_stale_generation_every_journal_and_unowned_stage_without_writes() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "conflicts", false);
    let before = fs::read(db(&store)).unwrap();
    assert!(matches!(
        store.upgrade_provider_schema(1),
        Err(ProjectionStoreError::Stale {
            expected: 1,
            actual: 0
        })
    ));
    assert_bytes(&db(&store), &before);
    for table in [
        "projection_install",
        "projection_namespace_pending",
        "projection_refresh_pending",
        "projection_restore_pending",
        "projection_rebase_pending",
        "projection_rebase_previews",
    ] {
        let c = database::connect(&db(&store), true).unwrap();
        c.execute(
            &format!("INSERT INTO {table} VALUES(1,?1)"),
            [b"{}".as_slice()],
        )
        .unwrap();
        drop(c);
        let before = fs::read(db(&store)).unwrap();
        assert!(store.upgrade_provider_schema(0).is_err(), "{table}");
        assert_bytes(&db(&store), &before);
        let c = database::connect(&db(&store), true).unwrap();
        c.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    for slot in [
        "projection-staging",
        "namespace-staging",
        "projection-quarantine",
    ] {
        let stage = store.checkout_root().join(".layrs").join(slot);
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("unowned"), b"preserve unclassified staging").unwrap();
        let before = fs::read(db(&store)).unwrap();
        assert_fence(
            store.upgrade_provider_schema(0),
            "blocked by checkout staging",
        );
        assert_bytes(&db(&store), &before);
        assert_eq!(
            fs::read(stage.join("unowned")).unwrap(),
            b"preserve unclassified staging"
        );
        // Explicitly retain fixture data elsewhere; upgrade never removes it.
        fs::rename(stage.join("unowned"), f.0.join(format!("retained-{slot}"))).unwrap();
        assert!(stage.is_dir());
    }
    store.upgrade_provider_schema(0).unwrap();
}

#[test]
fn acquiring_and_shared_reservations_block_upgrade_but_ready_pins_survive() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "preparation", false);
    let req = request(&store, "a.txt");
    let intent = store.begin_preparation(&req).unwrap();
    let c = database::connect(&db(&store), false).unwrap();
    let other_private_blockers: u64 = c.query_row(
        "SELECT (SELECT COUNT(*) FROM projection_budgets)+(SELECT COUNT(*) FROM projection_prejournal)+(SELECT COUNT(*) FROM projection_rebase_previews)+(SELECT COUNT(*) FROM projection_install)+(SELECT COUNT(*) FROM projection_namespace_pending)+(SELECT COUNT(*) FROM projection_refresh_pending)+(SELECT COUNT(*) FROM projection_restore_pending)+(SELECT COUNT(*) FROM projection_rebase_pending)+(SELECT COUNT(*) FROM projection_publication_pending)",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(other_private_blockers, 0);
    let reservation: String = c
        .query_row(
            "SELECT reservation FROM projection_preparations WHERE id=?1 AND status='acquiring'",
            [&intent.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM projection_preparations WHERE status='acquiring'",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    drop(c);
    for slot in [
        "projection-staging",
        "namespace-staging",
        "projection-quarantine",
    ] {
        assert!(!store.checkout_root().join(".layrs").join(slot).exists());
    }
    // Keep the acquiring intent's real shared reservation and ownership intact.
    let lease = family.lease(SessionMode::Shared).unwrap();
    let catalog = lease.catalog_once(false).unwrap();
    let owner: String = catalog
        .connection
        .query_row(
            "SELECT owner FROM reservations WHERE id=?1",
            [&reservation],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(owner, store.identity().worktree_id.as_str());
    drop(catalog);
    drop(lease);
    let acquiring_usage = family.usage().unwrap();
    let before = fs::read(db(&store)).unwrap();
    assert_fence(
        store.upgrade_provider_schema(0),
        "blocked by pending private work",
    );
    assert_bytes(&db(&store), &before);
    assert_eq!(family.usage().unwrap(), acquiring_usage);
    store.release_preparation(&intent.id).unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    let ready = store
        .prepare_offline(&req, &mut |_, _, out| out.write_all(b"alpha"))
        .unwrap();
    store.upgrade_provider_schema(0).unwrap();
    assert_eq!(store.preparation(&ready.id).unwrap().unwrap().id, ready.id);
    family.evict_unretained().unwrap();
    let mut out = Vec::new();
    store
        .read_version_file(&req.version, &RepoPath::parse("a.txt").unwrap(), &mut out)
        .unwrap();
    assert_eq!(out, b"alpha");
    store.release_preparation(&ready.id).unwrap();
    let lease = family.lease(SessionMode::Shared).unwrap();
    let reservation = lease
        .reserve(
            &store.identity().worktree_id,
            "unaccepted-upgrade-test",
            65536,
        )
        .unwrap();
    drop(lease);
    let before = fs::read(db(&store)).unwrap();
    assert!(
        store.upgrade_provider_schema(0).is_err(),
        "idempotence still checks pending reservations"
    );
    assert_bytes(&db(&store), &before);
    family
        .lease(SessionMode::Shared)
        .unwrap()
        .release_reservation(&reservation, &store.identity().worktree_id)
        .unwrap();
}

fn provider_intent(store: &ProjectionStore) {
    let c = database::connect(&db(store), true).unwrap();
    let digest = format!("blake3:{}", "1".repeat(64));
    c.execute(
        "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?2,?3)",
        rusqlite::params![digest, "1".repeat(64), b"{}".as_slice()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_provider_mounts VALUES('mount',?1,'control_published',?2,?1)",
        rusqlite::params![digest, b"{}".as_slice()],
    )
    .unwrap();
    c.execute(
        "UPDATE projection_backend SET backend='provider',mount_id='mount'",
        [],
    )
    .unwrap();
}

#[test]
fn missing_backend_singleton_is_integrity_failure_not_provider_capability() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "backend-diagnostic", false);
    store.upgrade_provider_schema(0).unwrap();
    let c = database::connect(&db(&store), true).unwrap();
    c.execute("DELETE FROM projection_backend", []).unwrap();
    drop(c);
    let before = fs::read(db(&store)).unwrap();
    assert_fence(
        store.state(),
        "singleton missing or invalid; integrity failure",
    );
    assert_fence(
        ProjectionStore::open(store.checkout_root()),
        "singleton missing or invalid; integrity failure",
    );
    assert_bytes(&db(&store), &before);

    let c = database::connect(&db(&store), true).unwrap();
    c.execute(
        "INSERT INTO projection_backend VALUES(1,'ordinary',NULL)",
        [],
    )
    .unwrap();
    drop(c);
    provider_intent(&store);
    let c = database::connect(&db(&store), false).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, u64>(
            0
        ))
        .unwrap(),
        0
    );
    drop(c);
    let before = fs::read(db(&store)).unwrap();
    assert_fence(
        store.state(),
        "provider backend requires a provider capability",
    );
    assert_fence(
        ProjectionStore::open(store.checkout_root()),
        "provider backend requires a provider capability",
    );
    assert_bytes(&db(&store), &before);
}

#[test]
fn opened_ordinary_and_recovery_handles_refuse_provider_intent_before_effects() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "fenced", false);
    store.upgrade_provider_schema(0).unwrap();
    let binding = ProjectionStore::inspect_recovery(store.checkout_root())
        .unwrap()
        .binding;
    let handle = ProjectionStore::open_bound_recovery(store.checkout_root(), &binding).unwrap();
    provider_intent(&store);
    let before = fs::read(db(&store)).unwrap();
    let mut writer_called = false;
    let error = store
        .install_projection(0, &hydration().1, &mut |_, _| {
            writer_called = true;
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("provider backend"));
    assert!(!writer_called);
    assert_fence(store.state(), "provider backend");
    assert_fence(store.capture_patch(0), "provider backend");
    assert_fence(store.fsck(), "provider backend");
    assert_fence(store.upgrade_provider_schema(0), "provider backend");
    assert_fence(
        ProjectionStore::open(store.checkout_root()),
        "provider backend",
    );
    assert_fence(
        ProjectionStore::open_for_recovery(store.checkout_root()),
        "provider backend",
    );
    assert_fence(
        ProjectionStore::inspect_recovery(store.checkout_root()),
        "provider backend",
    );
    assert_fence(handle.resume_installation(), "provider backend");
    assert_fence(handle.resume_namespace(), "provider backend");
    assert_fence(handle.resume_refresh(), "provider backend");
    assert_fence(handle.resume_restore(), "provider backend");
    assert_fence(handle.resume_rebase(), "provider backend");
    assert_fence(handle.cancel_shared_reservation(), "provider backend");
    assert_fence(handle.resume_initialization(), "provider backend");
    assert_fence(handle.recover_databases(), "provider backend");
    assert_bytes(&db(&store), &before);
    assert!(!store.checkout_root().join("a.txt").exists());
    assert!(
        !store
            .checkout_root()
            .join(".layrs/projection-staging")
            .exists()
    );
}

#[test]
fn every_provider_evidence_table_is_a_closed_fence_even_without_live_mounts() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "evidence", false);
    store.upgrade_provider_schema(0).unwrap();
    let c = database::connect(&db(&store), true).unwrap();
    c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let id = format!("'blake3:{}'", "1".repeat(64));
    let raw = format!("'{}'", "1".repeat(64));
    let payload = "x'7b7d'";
    let rows = [
        format!("{id},0,{raw},{raw},{payload}"),
        format!("{id},'a.txt',{id},1,0,{id},{payload}"),
        format!("'mount',{id},'retired',{payload},{id}"),
        format!("{id},{id},'imported-mount',{id},0,{raw},0,{raw},{payload}"),
        format!("'txn','mount',0,{raw},0,NULL,'cancelled',NULL,{payload},{id}"),
        format!("'txn',1,{id},{payload},{id}"),
        format!("'txn','destination','published',0,NULL,NULL,{payload},{id}"),
        format!("{id},{id},'imported-txn',NULL,{payload}"),
        format!("{id},0,0,{id},{id},{payload}"),
        format!("{id},'file.txt',{id},1,0,{id},NULL,NULL,'unobserved',{id}"),
        format!("{id},'deleted.txt',{id},0,NULL,'revoked',{id}"),
    ];
    for (table, values) in provider::schema::EVIDENCE_TABLES.iter().zip(rows) {
        c.execute(&format!("INSERT INTO {table} VALUES({values})"), [])
            .unwrap();
        // Isolation deliberately leaves referenced rows absent. The backend
        // fence itself must reject each table, independently of FK validation.
        assert!(
            provider::require_ordinary(&c)
                .unwrap_err()
                .to_string()
                .contains("provider evidence"),
            "{table}"
        );
        c.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    database::validate(&c, &store.marker).unwrap();
}

#[test]
fn unknown_or_uninterpreted_owner_prevents_all_family_reclamation() {
    for populated in [false, true] {
        let f = Fixture::new();
        let family = family(&f);
        let owner = member(&f, &family, "z-owner", false);
        let other = member(&f, &family, "a-valid", false);
        let req = request(&other, "a.txt");
        let prepared = other
            .prepare_offline(&req, &mut |_, _, out| out.write_all(b"alpha"))
            .unwrap();
        other.release_preparation(&prepared.id).unwrap();
        owner.upgrade_provider_schema(0).unwrap();
        let c = database::connect(&db(&owner), true).unwrap();
        if populated {
            c.execute(
                "INSERT INTO projection_provider_companions VALUES(?1,0,0,?1,?1,?2)",
                rusqlite::params![format!("blake3:{}", "1".repeat(64)), b"{}".as_slice()],
            )
            .unwrap();
        } else {
            c.execute_batch("PRAGMA user_version=99").unwrap();
        }
        drop(c);
        let catalog = fs::read(family.root().join("metadata/catalog.sqlite3")).unwrap();
        let before = family.usage().unwrap().content_bytes;
        assert!(before > 0);
        assert!(family.evict_unretained().is_err());
        assert_eq!(family.usage().unwrap().content_bytes, before);
        assert_bytes(&family.root().join("metadata/catalog.sqlite3"), &catalog);
        let c = database::connect(&db(&owner), true).unwrap();
        if populated {
            c.execute("DELETE FROM projection_provider_companions", [])
                .unwrap();
        } else {
            c.execute_batch("PRAGMA user_version=8").unwrap();
        }
        drop(c);
        assert!(family.evict_unretained().unwrap() > 0);
    }
}

#[test]
fn exact_eight_refuses_silent_schema_extension_and_legacy_never_upgrades() {
    let f = Fixture::new();
    let legacy_root = f.0.join("legacy");
    fs::create_dir(&legacy_root).unwrap();
    let legacy = ProjectionStore::create(&legacy_root, identity(), hydration().0).unwrap();
    let before = fs::read(db(&legacy)).unwrap();
    assert!(legacy.upgrade_provider_schema(0).is_err());
    assert_bytes(&db(&legacy), &before);
    let family = family(&f);
    let store = member(&f, &family, "exact", false);
    store.upgrade_provider_schema(0).unwrap();
    let c = database::connect(&db(&store), true).unwrap();
    c.execute_batch("CREATE TABLE uncoordinated_extension(id INTEGER) STRICT")
        .unwrap();
    drop(c);
    let before = fs::read(db(&store)).unwrap();
    assert!(ProjectionStore::open(store.checkout_root()).is_err());
    assert!(store.upgrade_provider_schema(0).is_err());
    assert_bytes(&db(&store), &before);
}
