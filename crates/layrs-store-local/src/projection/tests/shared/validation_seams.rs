//! Narrow preparation seams: parsed data and schema/identity checks alone never
//! grant filesystem, provider, history, or readiness authority.
use super::*;
use rusqlite::params;

fn assert_invalid<T>(result: Result<T>, expected: &str) {
    match result {
        Err(ProjectionStoreError::Invalid(message)) => {
            assert!(
                message.contains(expected),
                "expected {expected}, got {message}"
            );
        }
        Err(error) => panic!("expected {expected}, got {error}"),
        Ok(_) => panic!("expected {expected}, validation succeeded"),
    }
}

fn canonical_marker_fixture() -> Vec<u8> {
    // Pure parse data: no directory is created or authenticated for these IDs.
    layrs_core::scm::canonical_json(&serde_json::json!({
        "schemaVersion": "layrs.projection-store-format.v1",
        "magic": "LAYRS_NATIVE_STORE",
        "formatVersion": 3,
        "minReaderVersion": 3,
        "identity": identity(),
        "checkoutRoot": "unopened-parser-fixture",
        "checkoutIdentity": "fs:v1:0000000000000001:0000000000000002",
        "storeIdentity": "fs:v1:0000000000000001:0000000000000003",
        "metadataIdentity": "fs:v1:0000000000000001:0000000000000004",
        "initialSnapshotDigest": "0".repeat(64),
    }))
    .unwrap()
}

fn db_path(store: &ProjectionStore) -> PathBuf {
    store
        .marker
        .metadata_path(store.checkout_root())
        .join("projection.sqlite3")
}

fn assert_unchanged(path: &Path, before: &[u8]) {
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

#[test]
fn pure_marker_parser_accepts_canonical_data_at_exact_bound_without_io_authority() {
    let bytes = canonical_marker_fixture();
    let mut marker = format::parse_marker_bytes(&bytes).unwrap();
    assert_eq!(marker.identity, identity());
    assert_eq!(marker.bytes().unwrap(), bytes);
    // The parser intentionally does not interpret or open the root. Oversized
    // path data remains subject to later filesystem binding requirements.
    marker.checkout_root.clear();
    let overhead = marker.bytes().unwrap().len();
    marker.checkout_root = "x".repeat(64 * 1024 - overhead);
    let bounded = marker.bytes().unwrap();
    assert_eq!(bounded.len(), 64 * 1024);
    assert_eq!(format::parse_marker_bytes(&bounded).unwrap(), marker);
    marker.checkout_root.push('x');
    let oversized = marker.bytes().unwrap();
    assert_eq!(oversized.len(), 64 * 1024 + 1);
    assert_invalid(
        format::parse_marker_bytes(&oversized),
        "projection FORMAT too large",
    );
    // Size rejection precedes JSON parsing, even for oversized malformed input.
    assert_invalid(
        format::parse_marker_bytes(&vec![0xff; 64 * 1024 + 1]),
        "projection FORMAT too large",
    );
}

#[test]
fn pure_marker_parser_preserves_canonical_semantic_and_serde_refusals() {
    let bytes = canonical_marker_fixture();
    let mut spaced = bytes.clone();
    spaced.push(b'\n');
    assert_invalid(
        format::parse_marker_bytes(&spaced),
        "projection FORMAT is not canonical",
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_invalid(
        format::parse_marker_bytes(&serde_json::to_vec_pretty(&value).unwrap()),
        "projection FORMAT is not canonical",
    );
    let mut changed = value.clone();
    changed["minReaderVersion"] = serde_json::json!(99);
    assert_invalid(
        format::parse_marker_bytes(&layrs_core::scm::canonical_json(&changed).unwrap()),
        "unsupported projection FORMAT",
    );
    let mut changed = value.clone();
    changed["identity"]["origin"] = serde_json::json!("https://example.test/path");
    assert_invalid(
        format::parse_marker_bytes(&layrs_core::scm::canonical_json(&changed).unwrap()),
        "exact origin",
    );
    let mut changed = value;
    changed["unexpected"] = serde_json::json!(true);
    assert_invalid(
        format::parse_marker_bytes(&layrs_core::scm::canonical_json(&changed).unwrap()),
        "unknown field",
    );
    let mut duplicate = br#"{"magic":"LAYRS_NATIVE_STORE","#.to_vec();
    duplicate.extend_from_slice(&bytes[1..]);
    assert_invalid(format::parse_marker_bytes(&duplicate), "duplicate field");
    for malformed in [b"".as_slice(), b"{", b"null", &[0xff]] {
        assert!(format::parse_marker_bytes(malformed).is_err());
    }
}

#[test]
fn marker_reader_keeps_existing_size_and_canonical_errors() {
    let f = Fixture::new();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    let path = store.checkout_root().join(".layrs/FORMAT");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        format::parse_marker_bytes(&bytes).unwrap(),
        format::read_marker(store.checkout_root()).unwrap()
    );
    let mut spaced = bytes;
    spaced.push(b'\n');
    fs::write(&path, &spaced).unwrap();
    assert_invalid(
        format::read_marker(store.checkout_root()),
        "projection FORMAT is not canonical",
    );
    fs::write(&path, vec![b'x'; 64 * 1024 + 1]).unwrap();
    assert_invalid(
        format::read_marker(store.checkout_root()),
        "projection FORMAT too large",
    );
}

fn valid_readonly_prefix(store: &ProjectionStore, version: u32) {
    let path = db_path(store);
    let before = fs::read(&path).unwrap();
    let c = database::connect(&path, false).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        version
    );
    database::validate_schema_identity(&c, &store.marker).unwrap();
    database::validate(&c, &store.marker).unwrap();
    assert_eq!(
        format::parse_marker_bytes(&store.marker.bytes().unwrap()).unwrap(),
        store.marker
    );
    drop(c);
    assert_unchanged(&path, &before);
}

#[test]
fn schema_identity_prefix_preserves_exact_six_seven_and_eight_without_migration() {
    let legacy_fixture = Fixture::new();
    let legacy = ProjectionStore::create(&legacy_fixture.0, identity(), snapshot()).unwrap();
    valid_readonly_prefix(&legacy, 6);
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "schema-prefix", false);
    valid_readonly_prefix(&store, 7);
    store.upgrade_provider_schema(0).unwrap();
    valid_readonly_prefix(&store, 8);
}

#[test]
fn schema_identity_prefix_preserves_fingerprint_version_and_schema_budgets() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "schema-refusals", false);
    let mut c = database::connect(&db_path(&store), true).unwrap();
    for mutation in ["version", "fingerprint", "schema-count", "schema-bytes"] {
        let tx = c.transaction().unwrap();
        match mutation {
            "version" => tx.execute_batch("PRAGMA user_version=99;").unwrap(),
            "fingerprint" => tx
                .execute_batch("CREATE TABLE unexpected(value BLOB) STRICT;")
                .unwrap(),
            "schema-count" => {
                for number in 0..65 {
                    tx.execute_batch(&format!("CREATE TABLE extra_{number}(value BLOB) STRICT;"))
                        .unwrap();
                }
            }
            "schema-bytes" => tx
                .execute_batch(&format!(
                    "CREATE TABLE extra_large(value TEXT DEFAULT '{}') STRICT;",
                    "x".repeat(64 * 1024)
                ))
                .unwrap(),
            _ => unreachable!(),
        }
        let expected = if mutation.starts_with("schema-") {
            "projection schema exceeds validation budget"
        } else {
            "projection schema mismatch; explicit migration required"
        };
        let changes: u64 = tx
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap();
        assert_invalid(
            database::validate_schema_identity(&tx, &store.marker),
            expected,
        );
        assert_invalid(database::validate(&tx, &store.marker), expected);
        assert_eq!(
            tx.query_row("SELECT total_changes()", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            changes
        );
        tx.rollback().unwrap();
        database::validate(&c, &store.marker).unwrap();
    }
}

#[test]
fn schema_identity_prefix_refuses_other_private_bindings_and_oversized_identity() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "identity-prefix", false);
    let path = db_path(&store);
    let before = fs::read(&path).unwrap();
    let c = database::connect(&path, false).unwrap();
    for native_binding_only in [false, true] {
        let mut other = store.marker.clone();
        if native_binding_only {
            other.metadata_identity.push_str("-different-object");
            assert_eq!(other.identity, store.marker.identity);
        } else {
            other.identity.worktree_id = WorktreeId::parse("worktree:other").unwrap();
        }
        assert_invalid(
            database::validate_schema_identity(&c, &other),
            "projection identity differs from FORMAT",
        );
        assert_invalid(
            database::validate(&c, &other),
            "projection identity differs from FORMAT",
        );
    }
    drop(c);
    assert_unchanged(&path, &before);
    let mut c = database::connect(&path, true).unwrap();
    let tx = c.transaction().unwrap();
    tx.execute(
        "UPDATE projection_meta SET identity=?1 WHERE singleton=1",
        [vec![b'x'; 64 * 1024 + 1]],
    )
    .unwrap();
    assert_invalid(
        database::validate_schema_identity(&tx, &store.marker),
        "projection identity exceeds limit",
    );
    assert_invalid(
        database::validate(&tx, &store.marker),
        "projection identity exceeds limit",
    );
    tx.rollback().unwrap();
    database::validate(&c, &store.marker).unwrap();
}

#[test]
fn provider_rows_pass_only_structure_prefix_and_still_fence_before_data_traversal() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "provider-prefix", false);
    store.upgrade_provider_schema(0).unwrap();
    let mut c = database::connect(&db_path(&store), true).unwrap();
    // Deliberately introduce an FK failure later to test validation ordering.
    // This test-only connection never commits the altered rows.
    c.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    for provider_backend in [false, true] {
        let tx = c.transaction().unwrap();
        let catalog = FileId::from_bytes(b"structure-only catalog");
        let record = FileId::from_bytes(b"structure-only mount record");
        let initial = store.marker.initial_snapshot_digest.clone();
        let selection_digest: String = tx
            .query_row(
                "SELECT digest FROM projection_selections WHERE generation=0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // SQL-valid evidence is intentionally not a typed canonical provider
        // document. The extracted prefix must never be mistaken for admission.
        tx.execute(
            "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?3,?4)",
            params![
                catalog.as_str(),
                initial,
                selection_digest,
                b"{}".as_slice()
            ],
        )
        .unwrap();
        if provider_backend {
            tx.execute(
                "INSERT INTO projection_provider_mounts VALUES('test-mount',?1,'allocating',?2,?3)",
                params![catalog.as_str(), b"{}".as_slice(), record.as_str()],
            )
            .unwrap();
            tx.execute(
                "UPDATE projection_backend SET backend='provider',mount_id='test-mount'",
                [],
            )
            .unwrap();
        }
        assert_eq!(
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
                .get::<_, u64>(
                0
            ))
            .unwrap(),
            0
        );
        database::validate_schema_identity(&tx, &store.marker).unwrap();
        let fence = if provider_backend {
            "provider backend requires a provider capability"
        } else {
            "provider evidence requires a supported typed interpreter"
        };
        assert_invalid(database::validate(&tx, &store.marker), fence);
        let bad = "f".repeat(64);
        assert_ne!(initial, bad);
        assert_ne!(selection_digest, bad);
        tx.execute(
            "UPDATE projection_snapshots SET digest=?1 WHERE generation=0",
            [&bad],
        )
        .unwrap();
        tx.execute(
            "UPDATE projection_selections SET digest=?1 WHERE generation=0",
            [&bad],
        )
        .unwrap();
        tx.execute(
            "UPDATE projection_head SET generation=999 WHERE singleton=1",
            [],
        )
        .unwrap();
        database::validate_schema_identity(&tx, &store.marker).unwrap();
        assert_invalid(database::validate(&tx, &store.marker), fence);
        tx.execute(
            "UPDATE projection_backend SET backend='ordinary',mount_id=NULL",
            [],
        )
        .unwrap();
        tx.execute("DELETE FROM projection_provider_mounts", [])
            .unwrap();
        tx.execute("DELETE FROM projection_provider_catalogs", [])
            .unwrap();
        assert_invalid(
            database::validate(&tx, &store.marker),
            "initial projection differs from FORMAT",
        );
        tx.execute(
            "UPDATE projection_snapshots SET digest=?1 WHERE generation=0",
            [&initial],
        )
        .unwrap();
        assert_invalid(
            database::validate(&tx, &store.marker),
            "selection digest mismatch",
        );
        tx.execute(
            "UPDATE projection_selections SET digest=?1 WHERE generation=0",
            [&selection_digest],
        )
        .unwrap();
        assert_invalid(
            database::validate(&tx, &store.marker),
            "projection foreign key corruption",
        );
        tx.execute(
            "UPDATE projection_head SET generation=0 WHERE singleton=1",
            [],
        )
        .unwrap();
        database::validate(&tx, &store.marker).unwrap();
        tx.rollback().unwrap();
    }
}
