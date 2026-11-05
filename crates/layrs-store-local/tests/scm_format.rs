use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_core::scm::StoreId;
use layrs_store_local::scm_v1::{
    FormatMarker, FormatState, NativeRecovery, NativeStoreInspection, NativeStoreLayout, init,
    inspect, open, open_audit, open_exclusive,
};
use rusqlite::Connection;

#[test]
fn missing_inspection_and_open_are_strictly_read_only() {
    let root = unique_root("missing-read-only");
    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::AbsentOrEmpty { .. }
    ));
    assert!(!root.exists());
    assert!(open(&root).is_err());
    assert!(!root.exists());
}

#[test]
fn fresh_bootstrap_publishes_canonical_ready_marker_and_layout() {
    let root = unique_root("fresh");
    let store_id = store_id("fresh");
    let opened = init(&root, store_id.clone()).unwrap();
    let layout = opened.layout();

    assert_eq!(opened.store_id(), &store_id);
    let connection = Connection::open(layout.sqlite_path()).unwrap();
    let (stored_id, payload): (String, String) = connection
        .query_row("SELECT store_id, payload_json FROM stores", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(stored_id, store_id.as_str());
    assert_eq!(payload, format!(r#"{{"storeId":"{}"}}"#, store_id.as_str()));
    drop(connection);
    assert!(layout.objects_dir().is_dir());
    assert!(layout.staging_dir().is_dir());
    assert!(layout.staging_lease_path().is_file());
    assert!(layout.metadata_dir().is_dir());
    assert!(layout.repair_backups_dir().is_dir());
    let bytes = fs::read(layout.format_path()).unwrap();
    assert_eq!(
        bytes,
        FormatMarker::new(store_id.clone(), FormatState::Ready)
            .canonical_bytes()
            .unwrap()
    );

    let before_open = snapshot(&root);
    let reopened = open(&root).unwrap();
    assert_eq!(reopened.store_id(), &store_id);
    assert_snapshot_eq(&snapshot(&root), &before_open);
    let before = snapshot(&root);
    assert!(init(&root, store_id).is_err());
    assert_snapshot_eq(&snapshot(&root), &before);
    drop(reopened);
    drop(opened);
    cleanup(&root);
}

#[test]
fn ready_opens_and_writer_boundary_require_exact_format_store_identity_without_mutation() {
    let payload_root = unique_root("identity-payload");
    let payload_id = store_id("identity-payload");
    drop(init(&payload_root, payload_id.clone()).unwrap());
    let connection = Connection::open(payload_root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute(
            r#"UPDATE stores SET payload_json='{"storeId":"store:replacement"}'"#,
            [],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);
    let before = snapshot(&payload_root);
    assert!(open(&payload_root).is_err());
    assert_snapshot_eq(&snapshot(&payload_root), &before);
    cleanup(&payload_root);

    let replacement_root = unique_root("identity-replacement");
    let replacement_id = store_id("identity-replacement");
    drop(init(&replacement_root, replacement_id).unwrap());
    let connection = Connection::open(replacement_root.join("metadata/layrs.sqlite3")).unwrap();
    connection.execute("DELETE FROM stores", []).unwrap();
    connection
        .execute(
            r#"INSERT INTO stores(store_id, payload_json, created_at_ms)
             VALUES ('store:replacement', '{"storeId":"store:replacement"}', 0)"#,
            [],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);
    let before = snapshot(&replacement_root);
    assert!(open_exclusive(&replacement_root).is_err());
    assert_snapshot_eq(&snapshot(&replacement_root), &before);
    cleanup(&replacement_root);

    let extra_root = unique_root("identity-extra");
    let extra_id = store_id("identity-extra");
    drop(init(&extra_root, extra_id).unwrap());
    let mut exclusive = open_exclusive(&extra_root).unwrap();
    let connection = Connection::open(extra_root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute(
            r#"INSERT INTO stores(store_id, payload_json, created_at_ms)
             VALUES ('store:extra', '{"storeId":"store:extra"}', 0)"#,
            [],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);
    assert!(NativeRecovery::new(&mut exclusive).is_err());
    drop(exclusive);
    cleanup(&extra_root);
}

#[test]
fn open_handle_keeps_shared_lock_and_immutable_connection_alive() {
    let root = unique_root("open-handle-lock");
    let opened = init(&root, store_id("open-handle-lock")).unwrap();
    let before = snapshot(&root);

    assert_snapshot_eq(&snapshot(&root), &before);

    let contender = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(opened.layout().lock_path())
        .unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&contender).is_err());

    drop(opened);
    fs2::FileExt::try_lock_exclusive(&contender).unwrap();
    fs2::FileExt::unlock(&contender).unwrap();
    drop(contender);
    cleanup(&root);
}

#[test]
fn exclusive_handle_is_noop_at_open_and_blocks_other_capabilities() {
    let root = unique_root("exclusive-handle-lock");
    let shared = init(&root, store_id("exclusive-handle-lock")).unwrap();
    assert!(open_exclusive(&root).is_err());
    drop(shared);

    let before = snapshot(&root);
    let exclusive = open_exclusive(&root).unwrap();
    assert!(!root.join("metadata/layrs.sqlite3-wal").exists());
    assert!(!root.join("metadata/layrs.sqlite3-shm").exists());
    assert!(open(&root).is_err());
    assert!(open_audit(&root).is_err());
    drop(exclusive);
    assert_snapshot_eq(&snapshot(&root), &before);
    cleanup(&root);
}

#[test]
fn audit_handle_does_not_open_or_repair_a_corrupt_schema() {
    let root = unique_root("audit-corrupt-schema");
    let opened = init(&root, store_id("audit-corrupt-schema")).unwrap();
    let database = opened.layout().sqlite_path().to_path_buf();
    drop(opened);
    fs::write(&database, b"not sqlite").unwrap();
    let before = snapshot(&root);

    let audit = open_audit(&root).unwrap();
    assert_eq!(audit.store_id(), &store_id("audit-corrupt-schema"));
    assert_snapshot_eq(&snapshot(&root), &before);
    drop(audit);
    assert!(open(&root).is_err());
    assert_snapshot_eq(&snapshot(&root), &before);
    cleanup(&root);
}

#[test]
fn native_initializing_marker_resumes_after_a_bootstrap_boundary_failure() {
    let root = unique_root("resume");
    fs::create_dir_all(&root).unwrap();
    let store_id = store_id("resume");
    let marker = FormatMarker::new(store_id.clone(), FormatState::Initializing);
    fs::write(root.join("FORMAT"), marker.canonical_bytes().unwrap()).unwrap();
    fs::write(root.join("objects"), b"bootstrap blocker").unwrap();

    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::NativeInitializing { .. }
    ));
    assert!(init(&root, store_id.clone()).is_err());
    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::NativeInitializing { .. }
    ));

    fs::remove_file(root.join("objects")).unwrap();
    init(&root, store_id.clone()).unwrap();
    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::NativeReady { .. }
    ));
    assert_eq!(open(&root).unwrap().store_id(), &store_id);
    cleanup(&root);
}

#[test]
fn initializing_resume_reclaims_only_its_exact_sqlite_stage() {
    let root = unique_root("resume-sqlite-stage");
    fs::create_dir_all(root.join("metadata")).unwrap();
    let store_id = store_id("resume-sqlite-stage");
    let marker = FormatMarker::new(store_id.clone(), FormatState::Initializing);
    fs::write(root.join("FORMAT"), marker.canonical_bytes().unwrap()).unwrap();
    let database = root.join("metadata").join("layrs.sqlite3");
    let stage = database.with_file_name(".layrs.sqlite3.initializing");
    let unrelated = root.join("metadata").join("layrs.sqlite3.not-ours");
    fs::write(&stage, b"abandoned authenticated bootstrap stage").unwrap();
    fs::write(&unrelated, b"must survive").unwrap();

    let opened = init(&root, store_id).unwrap();
    assert!(!stage.exists());
    assert_eq!(fs::read(&unrelated).unwrap(), b"must survive");
    assert!(opened.layout().sqlite_path().is_file());
    drop(opened);
    cleanup(&root);
}

#[test]
fn initializing_resume_never_replaces_a_partial_final_database() {
    let root = unique_root("resume-partial-final");
    fs::create_dir_all(root.join("metadata")).unwrap();
    let store_id = store_id("resume-partial-final");
    let marker = FormatMarker::new(store_id.clone(), FormatState::Initializing);
    let marker_bytes = marker.canonical_bytes().unwrap();
    fs::write(root.join("FORMAT"), &marker_bytes).unwrap();
    let database = root.join("metadata").join("layrs.sqlite3");
    fs::write(&database, b"partial final must not be replaced").unwrap();

    assert!(init(&root, store_id).is_err());
    assert_eq!(
        fs::read(&database).unwrap(),
        b"partial final must not be replaced"
    );
    assert_eq!(fs::read(root.join("FORMAT")).unwrap(), marker_bytes);
    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::NativeInitializing { .. }
    ));
    cleanup(&root);
}

#[test]
fn legacy_development_sqlite_is_refused_without_mutation() {
    let root = unique_root("legacy-current");
    let metadata = root.join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    rusqlite::Connection::open(metadata.join("layrs.sqlite3"))
        .unwrap()
        .execute_batch("CREATE TABLE spaces (space_id TEXT PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    let before = snapshot(&root);

    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::LegacySqlite { .. }
    ));
    assert_refused_without_mutation(&root, &before);
    cleanup(&root);
}

#[test]
fn client_documents_sqlite_is_classified_and_refused_without_mutation() {
    let root = unique_root("legacy-documents");
    let metadata = root.join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    let database = metadata.join("layrs.sqlite3");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE client_documents (path TEXT PRIMARY KEY, payload_json TEXT NOT NULL);",
        )
        .unwrap();
    drop(connection);
    let before = snapshot(&root);

    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::LegacyClientDocuments { .. }
    ));
    assert_refused_without_mutation(&root, &before);
    cleanup(&root);
}

#[test]
fn legacy_json_future_and_unknown_roots_are_refused_without_mutation() {
    for (label, file, bytes, expected_kind) in [
        (
            "legacy-json",
            "local-space.json",
            br#"{"schemaVersion":1}"#.as_slice(),
            "legacy_json",
        ),
        (
            "future",
            "FORMAT",
            br#"{"formatVersion":999,"magic":"LAYRS_NATIVE_STORE","minReaderVersion":999}"#
                .as_slice(),
            "future_format",
        ),
        (
            "unknown",
            "owner-note.txt",
            b"do not touch".as_slice(),
            "unknown_nonempty",
        ),
    ] {
        let root = unique_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(file), bytes).unwrap();
        let before = snapshot(&root);
        let inspection = inspect(&root).unwrap();
        assert_eq!(inspection.kind(), expected_kind);
        assert_refused_without_mutation(&root, &before);
        cleanup(&root);
    }
}

#[test]
fn invalid_or_incomplete_ready_markers_never_create_missing_layout() {
    let root = unique_root("invalid-ready");
    fs::create_dir_all(&root).unwrap();
    let marker = FormatMarker::new(store_id("invalid-ready"), FormatState::Ready);
    fs::write(root.join("FORMAT"), marker.canonical_bytes().unwrap()).unwrap();
    let before = snapshot(&root);
    assert!(matches!(
        inspect(&root).unwrap(),
        NativeStoreInspection::InvalidNative { .. }
    ));
    assert!(open(&root).is_err());
    assert_snapshot_eq(&snapshot(&root), &before);
    cleanup(&root);
}

#[test]
fn oversized_format_is_classified_with_a_bounded_read_and_no_mutation() {
    let root = unique_root("oversized-format");
    fs::create_dir_all(&root).unwrap();
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(root.join("FORMAT"))
        .unwrap();
    file.set_len(128 * 1024).unwrap();
    drop(file);
    let before = snapshot(&root);
    let inspection = inspect(&root).unwrap();
    let NativeStoreInspection::InvalidNative { reason, .. } = inspection else {
        panic!("oversized FORMAT must be invalid native state");
    };
    assert!(reason.contains("65536-byte FORMAT limit"));
    assert_snapshot_eq(&snapshot(&root), &before);
    cleanup(&root);
}

#[test]
fn layout_rejects_empty_and_filesystem_root_targets() {
    assert!(NativeStoreLayout::new("").is_err());
    let filesystem_root = if cfg!(windows) {
        Path::new(r"C:\")
    } else {
        Path::new("/")
    };
    assert!(NativeStoreLayout::new(filesystem_root).is_err());
}

fn assert_refused_without_mutation(root: &Path, before: &BTreeMap<PathBuf, Vec<u8>>) {
    assert!(open(root).is_err());
    assert_snapshot_eq(&snapshot(root), before);
    assert!(init(root, store_id("must-not-install")).is_err());
    assert_snapshot_eq(&snapshot(root), before);
}

fn assert_snapshot_eq(actual: &BTreeMap<PathBuf, Vec<u8>>, expected: &BTreeMap<PathBuf, Vec<u8>>) {
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    for (path, expected_bytes) in expected {
        let actual_bytes = actual.get(path).unwrap();
        assert_eq!(
            actual_bytes,
            expected_bytes,
            "filesystem bytes changed at {}",
            path.display()
        );
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    snapshot_directory(root, root, &mut result);
    result
}

fn snapshot_directory(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            result.insert(relative.clone(), b"<directory>".to_vec());
            snapshot_directory(root, &path, result);
        } else if metadata.is_file() {
            result.insert(relative, fs::read(path).unwrap());
        } else {
            result.insert(relative, b"<special>".to_vec());
        }
    }
}

fn checkpoint(connection: &Connection) {
    let result: (u32, u32, u32) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(result.0, 0);
    assert_eq!(result.1, result.2);
}

fn store_id(label: &str) -> StoreId {
    StoreId::parse(format!("store:{label}")).unwrap()
}

fn unique_root(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "layrs-store-local-scm-format-{label}-{}-{timestamp}",
        std::process::id()
    ))
}

fn cleanup(root: &Path) {
    assert!(root.starts_with(std::env::temp_dir()));
    assert!(
        root.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("layrs-store-local-scm-format-"))
    );
    fs::remove_dir_all(root).unwrap();
}
