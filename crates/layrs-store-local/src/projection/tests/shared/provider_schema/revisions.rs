use super::*;
use crate::secure_fs::PinnedDirectory;
use rusqlite::{Connection, params};

fn id(byte: u8) -> String {
    format!("blake3:{}", format!("{byte:02x}").repeat(32))
}
fn companion(c: &Connection) {
    // SQL-valid retained evidence only; Stage1a has no typed companion reader.
    c.execute(
        "INSERT INTO projection_provider_companions VALUES(?1,1,1,?2,?3,?4)",
        params![
            id(1),
            id(2),
            id(3),
            br#"{"schemaVersion":"layrs.projection-provider-companion.v1"}"#.as_slice()
        ],
    )
    .unwrap();
    c.execute("INSERT INTO projection_provider_companion_files VALUES(?1,'preserved.txt',?2,5,0,?3,NULL,NULL,'unobserved',?4)", params![id(1), FileId::from_bytes(b"alpha").as_str(), id(4), id(5)]).unwrap();
    c.execute("INSERT INTO projection_provider_companion_deletions VALUES(?1,'deleted.txt',?2,0,NULL,'revoked',?3)", params![id(1), id(6), id(7)]).unwrap();
}
fn fk_clean(c: &Connection) {
    assert_eq!(
        c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, u64>(
            0
        ))
        .unwrap(),
        0
    );
}
fn evidence_fences(store: &ProjectionStore) {
    let before = fs::read(db(store)).unwrap();
    assert_fence(
        ProjectionStore::open(store.checkout_root()),
        "provider evidence",
    );
    assert_fence(store.state(), "provider evidence");
    assert_fence(store.capture_patch(0), "provider evidence");
    assert_bytes(&db(store), &before);
}

#[test]
fn inherited_scalar_activation_and_companion_receipt_need_no_live_owner_rows() {
    for activation in [true, false] {
        let f = Fixture::new();
        let family = family(&f);
        let store = member(&f, &family, "inherited", false);
        store.upgrade_provider_schema(0).unwrap();
        let c = database::connect(&db(&store), true).unwrap();
        if activation {
            let snapshot: String = c
                .query_row(
                    "SELECT digest FROM projection_snapshots WHERE generation=0",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let selection: String = c
                .query_row(
                    "SELECT digest FROM projection_selections WHERE generation=0",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            c.execute(
                "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?3,?4)",
                params![id(1), snapshot, selection, b"{}".as_slice()],
            )
            .unwrap();
            // There is no live mount, range cache, or per-path activation array.
            // Only the SQL reference contract is asserted; opaque evidence is
            // deliberately refused before a future typed interpreter joins.
            c.execute("INSERT INTO projection_provider_activations VALUES(?1,?2,'inherited-mount',?3,0,?4,0,?4,?5)", params![id(2), id(3), id(1), snapshot, b"{}".as_slice()]).unwrap();
            assert_eq!(c.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name LIKE 'projection_provider_%range%'", [], |r| r.get::<_, u64>(0)).unwrap(), 0, "range state must remain ephemeral");
        } else {
            companion(&c);
            c.execute("INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,'inherited-transition',?3,?4)", params![id(8), id(9), id(1), b"{}".as_slice()]).unwrap();
        }
        assert_eq!(c.query_row("SELECT (SELECT COUNT(*) FROM projection_provider_mounts)+(SELECT COUNT(*) FROM projection_provider_transitions)", [], |r| r.get::<_, u64>(0)).unwrap(), 0);
        fk_clean(&c);
        drop(c);
        evidence_fences(&store);
    }
}

#[test]
fn private_budget_only_and_prejournal_only_each_block_upgrade_before_other_gates() {
    let f = Fixture::new();
    let family = family(&f);
    let budget_store = member(&f, &family, "budget-only", false);
    let access = format::access(&budget_store, SessionMode::Exclusive).unwrap();
    retention::reserve(
        &access.connection,
        &budget_store,
        "capture:budget-only",
        "witness",
        &[],
        0,
    )
    .unwrap();
    let reservation = retention::active_budget(&access.connection).unwrap();
    // Leave the durable private intent after a shared cancellation boundary.
    // No shared reservation, prejournal, acquisition or disk stage can mask
    // removal of the private-budget acceptance check in this fixture.
    access
        .family
        .as_ref()
        .unwrap()
        .release_reservation(&reservation, &budget_store.identity().worktree_id)
        .unwrap();
    assert_eq!(access.connection.query_row("SELECT (SELECT COUNT(*) FROM projection_prejournal)+(SELECT COUNT(*) FROM projection_preparations)", [], |r| r.get::<_, u64>(0)).unwrap(), 0);
    drop(access);
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    let before = fs::read(db(&budget_store)).unwrap();
    assert_fence(
        budget_store.upgrade_provider_schema(0),
        "blocked by pending private work",
    );
    assert_bytes(&db(&budget_store), &before);

    let stage_store = member(&f, &family, "prejournal-only", false);
    let stage = stage_store
        .checkout_root()
        .join(".layrs/projection-staging/owned");
    fs::create_dir_all(&stage).unwrap();
    let pin = PinnedDirectory::open(&stage).unwrap();
    let access = format::access(&stage_store, SessionMode::Exclusive).unwrap();
    retention::track_directory(
        &access.connection,
        &stage_store,
        "capture:prejournal-only",
        ".layrs/projection-staging/owned",
        &pin,
        vec![],
    )
    .unwrap();
    assert_eq!(access.connection.query_row("SELECT (SELECT COUNT(*) FROM projection_budgets)+(SELECT COUNT(*) FROM projection_preparations)", [], |r| r.get::<_, u64>(0)).unwrap(), 0);
    drop(access);
    drop(pin);
    let before = fs::read(db(&stage_store)).unwrap();
    assert_fence(
        stage_store.upgrade_provider_schema(0),
        "blocked by pending private work",
    );
    assert_bytes(&db(&stage_store), &before);
    assert!(stage.is_dir());
    stage_store.cancel_shared_reservation().unwrap();
    stage_store.upgrade_provider_schema(0).unwrap();
}

#[test]
fn real_pending_publication_refuses_upgrade_before_staging_checks() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "publication", true);
    fs::write(
        store.checkout_root().join("a.txt"),
        b"pending published work",
    )
    .unwrap();
    let capture = store.capture_patch(1).unwrap();
    store.cancel_shared_reservation().unwrap();
    store
        .begin_publication(capture.sequence, 1, br#"{"request":"immutable"}"#)
        .unwrap();
    let before = fs::read(db(&store)).unwrap();
    assert_fence(store.upgrade_provider_schema(1), "publication pending");
    assert_bytes(&db(&store), &before);
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"pending published work"
    );
}

fn evidence_rows(c: &Connection) -> Vec<(String, Vec<rusqlite::types::Value>)> {
    let mut result = Vec::new();
    for (table, order) in [
        ("projection_provider_companions", "id"),
        ("projection_provider_companion_files", "companion_id,path"),
        (
            "projection_provider_companion_deletions",
            "companion_id,path",
        ),
        ("projection_claims", "root_id"),
    ] {
        let mut q = c
            .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
            .unwrap();
        let columns = q.column_count();
        for row in q
            .query_map([], |r| {
                (0..columns)
                    .map(|i| r.get(i))
                    .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
            })
            .unwrap()
        {
            result.push((table.to_owned(), row.unwrap()));
        }
    }
    result
}
fn object_bytes(family: &PartialStoreFamily) -> Vec<(PathBuf, Vec<u8>)> {
    let mut bytes = Vec::new();
    for kind in ["chunks", "manifests"] {
        for entry in fs::read_dir(family.root().join("objects").join(kind)).unwrap() {
            let path = entry.unwrap().path();
            bytes.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    bytes.sort_by(|a, b| a.0.cmp(&b.0));
    bytes
}

#[test]
fn hot_populated_eight_owner_restores_committed_evidence_but_reclaims_nothing() {
    let f = Fixture::new();
    let family = family(&f);
    let owner = member(&f, &family, "z-provider", false);
    owner.upgrade_provider_schema(0).unwrap();
    let keep = owner
        .prepare_offline(&request(&owner, "a.txt"), &mut |_, _, out| {
            out.write_all(b"alpha")
        })
        .unwrap();
    let other = member(&f, &family, "a-ordinary", false);
    let mut observed = other.state().unwrap().snapshot;
    observed.entries[0].target = Some(TreeEntryTarget::file(FileId::from_bytes(
        b"garbage candidate",
    )));
    let version = other
        .record_version_observation(&observed, &ProjectionSelection::All)
        .unwrap();
    let mut req = request(&other, "a.txt");
    req.version = version;
    let discard = other
        .prepare_offline(&req, &mut |_, _, out| out.write_all(b"garbage candidate"))
        .unwrap();
    other.release_preparation(&discard.id).unwrap();
    // An unreferenced reservation would also be reclaimed by a successful scan.
    let lease = family.lease(SessionMode::Shared).unwrap();
    lease
        .reserve(
            &other.identity().worktree_id,
            "unclaimed-reservation",
            65536,
        )
        .unwrap();
    drop(lease);
    let c = database::connect(&db(&owner), true).unwrap();
    companion(&c);
    fk_clean(&c);
    let before_rows = evidence_rows(&c);
    assert!(
        before_rows
            .iter()
            .any(|(table, _)| table == "projection_claims")
    );
    drop(c);
    let before_usage = family.usage().unwrap();
    let before_catalog = fs::read(family.root().join("metadata/catalog.sqlite3")).unwrap();
    let before_objects = object_bytes(&family);
    assert!(before_usage.reserved_bytes > 0);
    super::super::r2::kill_spilled_writer(&db(&owner), "provider-owner", &f.0.join("spill-ready"));
    let spilled = fs::read(db(&owner)).unwrap();
    let inspection = ProjectionStore::inspect_recovery(owner.checkout_root()).unwrap();
    assert_eq!(
        inspection.recovery,
        vec![ProjectionRecoveryReason::WorktreeRollback]
    );
    assert_bytes(&db(&owner), &spilled);
    // Maintenance is the existing explicit, metadata-only hot rollback route.
    // It cannot proceed from rollback to accepting/interpreting provider rows.
    assert_fence(family.evict_unretained(), "provider evidence");
    assert_fence(family.reconcile_retention(), "provider evidence");
    let c = database::connect(&db(&owner), false).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        8
    );
    assert_eq!(evidence_rows(&c), before_rows);
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM projection_observations WHERE digest LIKE 'spill-%'",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT status FROM projection_preparations WHERE id=?1",
            [&keep.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "ready"
    );
    fk_clean(&c);
    drop(c);
    assert_eq!(family.usage().unwrap(), before_usage);
    assert_bytes(
        &family.root().join("metadata/catalog.sqlite3"),
        &before_catalog,
    );
    let after_objects = object_bytes(&family);
    assert_eq!(after_objects.len(), before_objects.len());
    for ((path, before), (after_path, after)) in before_objects.iter().zip(&after_objects) {
        assert_eq!(path, after_path);
        assert!(before == after, "shared object changed: {}", path.display());
    }
}

#[test]
fn provider_path_limits_count_utf8_bytes_and_compact_records_reject_namespace_blobs() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "byte-bounds", false);
    store.upgrade_provider_schema(0).unwrap();
    let c = database::connect(&db(&store), true).unwrap();
    companion(&c);
    c.execute(
        "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?2,?3)",
        params![id(10), "1".repeat(64), b"{}".as_slice()],
    )
    .unwrap();
    let within = "é".repeat(2048);
    let beyond = format!("{within}a");
    assert_eq!(within.len(), 4096);
    for (table, columns, values) in [
        (
            "projection_provider_descriptors",
            "catalog_id,path,file_id,bytes,executable,manifest_id,manifest",
            "?1,?2,?3,0,0,?3,?4",
        ),
        (
            "projection_provider_companion_files",
            "companion_id,path,file_id,bytes,executable,manifest_id,reason,digest",
            "?1,?2,?3,0,0,?3,'unobserved',?3",
        ),
        (
            "projection_provider_companion_deletions",
            "companion_id,path,base_file_id,base_executable,reason,digest",
            "?1,?2,?3,0,'revoked',?3",
        ),
    ] {
        let owner = if table.ends_with("descriptors") {
            id(10)
        } else {
            id(1)
        };
        let sql = format!("INSERT INTO {table}({columns}) VALUES({values})");
        for (path, succeeds) in [(&within, true), (&beyond, false)] {
            let inserted = if table.ends_with("descriptors") {
                c.execute(&sql, params![owner, path, id(11), b"{}".as_slice()])
            } else {
                c.execute(&sql, params![owner, path, id(11)])
            };
            assert_eq!(inserted.is_ok(), succeeds, "{table} UTF8 bytes");
        }
    }
    assert!(
        c.execute(
            "UPDATE projection_provider_companions SET payload=?1",
            [vec![0u8; 65537]]
        )
        .is_err()
    );
    assert!(
        c.execute(
            "INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,'oversize',?3,?4)",
            params![id(12), id(13), id(1), vec![0u8; 65537]]
        )
        .is_err()
    );
    fk_clean(&c);
}

#[test]
fn normalized_companion_schema_holds_full_bounded_rowsets_without_large_headers() {
    // Storage-capacity test only, not an implementation of companion authority.
    // Row/header digests will be enforced by the later typed interpreter.
    let mut c = Connection::open_in_memory().unwrap();
    c.execute_batch(database::SQL).unwrap();
    c.execute_batch(crate::projection::selection::SQL).unwrap();
    c.execute_batch(retention::SQL).unwrap();
    c.execute_batch(provider::schema::SQL).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let tx = c.transaction().unwrap();
    tx.execute(
        "INSERT INTO projection_provider_companions VALUES(?1,?2,?3,?1,?1,?4)",
        params![
            id(1),
            PROVIDER_COMPANION_MAX_FILES,
            PROVIDER_COMPANION_MAX_DELETIONS,
            b"{}".as_slice()
        ],
    )
    .unwrap();
    let padding = "p".repeat(192);
    tx.execute("WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM n WHERE i+1<?3) INSERT INTO projection_provider_companion_files SELECT ?1,printf('file-%06d-',i)||?2,?1,0,0,?1,NULL,NULL,'unobserved',?1 FROM n", params![id(1), padding, PROVIDER_COMPANION_MAX_FILES]).unwrap();
    tx.execute("WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM n WHERE i+1<?3) INSERT INTO projection_provider_companion_deletions SELECT ?1,printf('deleted-%06d-',i)||?2,?1,0,NULL,'revoked',?1 FROM n", params![id(1), padding, PROVIDER_COMPANION_MAX_DELETIONS]).unwrap();
    tx.commit().unwrap();
    let files: (u64,u64) = c.query_row("SELECT COUNT(*),SUM(length(CAST(path AS BLOB))) FROM projection_provider_companion_files", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let deletions: (u64,u64) = c.query_row("SELECT COUNT(*),SUM(length(CAST(path AS BLOB))) FROM projection_provider_companion_deletions", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(files.0, PROVIDER_COMPANION_MAX_FILES);
    assert_eq!(deletions.0, PROVIDER_COMPANION_MAX_DELETIONS);
    assert!(files.1 > 16 * 1024 * 1024 && deletions.1 > 16 * 1024 * 1024);
    let metadata_bytes: u64 = c.query_row("SELECT (SELECT SUM(length(CAST(path AS BLOB))+length(file_id)+8+1+length(manifest_id)+COALESCE(length(base_file_id),0)+1+length(CAST(reason AS BLOB))+length(digest)) FROM projection_provider_companion_files)+(SELECT SUM(length(CAST(path AS BLOB))+length(base_file_id)+1+COALESCE(length(base_manifest_id),0)+length(CAST(reason AS BLOB))+length(digest)) FROM projection_provider_companion_deletions)+(SELECT length(id)+16+length(files_digest)+length(deletions_digest)+length(payload) FROM projection_provider_companions)", [], |r| r.get(0)).unwrap();
    assert!(metadata_bytes < PROVIDER_COMPANION_AGGREGATE_MAX_BYTES);
    assert!(
        c.execute(
            "UPDATE projection_provider_companions SET file_count=100001",
            []
        )
        .is_err()
    );
    assert!(
        c.execute(
            "UPDATE projection_provider_companions SET deletion_count=100001",
            []
        )
        .is_err()
    );
    assert_eq!(
        c.query_row(
            "SELECT length(payload) FROM projection_provider_companions",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        2
    );
    fk_clean(&c);
}
