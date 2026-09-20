use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::{forget_cached_store, migration, write_materialized_file, LocalStoreSession};

#[test]
fn migration_installs_verified_backup_and_removes_legacy_json() {
    let root = test_root("migration-authority");
    let layrs_dir = root.join(".layrs");
    fs::create_dir_all(&layrs_dir).unwrap();
    let legacy = legacy_space(&root, "local-migrated");
    fs::write(
        layrs_dir.join("local-space.json"),
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let store = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    assert_eq!(
        store
            .read_value(&layrs_dir.join("local-space.json"))
            .unwrap(),
        legacy
    );
    assert!(layrs_dir.join("metadata/layrs.sqlite3").is_file());
    assert!(layrs_dir.join("legacy-json-v1/manifest.json").is_file());
    assert!(!layrs_dir.join("local-space.json").exists());
    drop(store);

    let reopened = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    assert_eq!(
        reopened
            .read_value(&layrs_dir.join("local-space.json"))
            .unwrap(),
        legacy
    );
    drop(reopened);
    cleanup(&root);
}

#[test]
fn migration_recovers_after_crash_before_sqlite_install() {
    let root = test_root("migration-recovery");
    let layrs_dir = root.join(".layrs");
    fs::create_dir_all(&layrs_dir).unwrap();
    fs::write(
        layrs_dir.join("local-space.json"),
        serde_json::to_vec(&legacy_space(&root, "local-recovery")).unwrap(),
    )
    .unwrap();

    migration::fail_before_install_once();
    let error = match LocalStoreSession::open_exclusive(&layrs_dir) {
        Ok(_) => panic!("migration fault should prevent SQLite installation"),
        Err(error) => error,
    };
    assert!(error.contains("injected migration fault"));
    assert!(layrs_dir.join("legacy-json-v1/manifest.json").is_file());
    assert!(layrs_dir.join("local-space.json").is_file());
    assert!(!layrs_dir.join("metadata/layrs.sqlite3").exists());

    let recovered = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    assert_eq!(
        recovered
            .read_value(&layrs_dir.join("local-space.json"))
            .unwrap()["localSpaceId"],
        "local-recovery"
    );
    assert!(!layrs_dir.join("local-space.json").exists());
    drop(recovered);
    cleanup(&root);
}

#[test]
fn stale_network_finalize_preserves_pending_documents() {
    let root = test_root("stale-finalize");
    let layrs_dir = root.join(".layrs");
    fs::create_dir_all(&layrs_dir).unwrap();
    let mut store = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    let pending_path = layrs_dir.join("layers/main/pending-publish/step-1.json");
    let pending = json!({"stepId": "step-1", "rootTreeId": "blake3:pending"});
    store.write(&pending_path, &pending).unwrap();
    let lease = store
        .prepare_network(
            "space-1",
            "publish",
            &json!({
                "expectedAckStepIds": ["step-1"],
                "targetLayerHead": "blake3:pending"
            }),
        )
        .unwrap();

    store
        .write(&layrs_dir.join("scan-cache.json"), &json!({"files": []}))
        .unwrap();
    let error = store.begin_network_finalize(&lease).unwrap_err();
    assert!(error.contains("pending work was preserved"));
    assert_eq!(store.read_value(&pending_path).unwrap(), pending);
    let state: String = store
        .inner
        .repository()
        .with_read(|connection| {
            connection
                .query_row(
                    "SELECT state FROM client_network_operations WHERE operation_id = ?1",
                    [&lease.operation_id],
                    |row| row.get(0),
                )
                .map_err(layrs_store_local::StoreError::from)
        })
        .unwrap();
    assert_eq!(state, "stale");
    drop(store);
    cleanup(&root);
}

#[test]
fn interrupted_materialization_recovers_original_bytes_on_open() {
    let root = test_root("materialization-recovery");
    let layrs_dir = root.join(".layrs");
    fs::create_dir_all(&layrs_dir).unwrap();
    let path = root.join("document.bin");
    let original = [0, 1, 2, 0xff, 4, 5];
    fs::write(&path, original).unwrap();
    let mut store = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    let _lease = store
        .begin_materialization(&root, ["document.bin".to_string()])
        .unwrap();
    write_materialized_file(&path, b"replacement").unwrap();
    drop(store);

    let recovered = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    drop(recovered);
    cleanup(&root);
}

#[test]
fn explicit_materialization_rollback_is_byte_exact() {
    let root = test_root("materialization-rollback");
    let layrs_dir = root.join(".layrs");
    fs::create_dir_all(&layrs_dir).unwrap();
    let path = root.join("document.bin");
    let original = [0, 1, 2, 3, 0xfe, 0xff];
    fs::write(&path, original).unwrap();
    let mut store = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    let lease = store
        .begin_materialization(&root, ["document.bin".to_string()])
        .unwrap();
    write_materialized_file(&path, b"replacement").unwrap();
    store.rollback_materialization(&lease).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    drop(store);
    cleanup(&root);
}

#[test]
fn exclusive_session_is_non_reentrant_and_reopens_after_drop() {
    let root = test_root("non-reentrant");
    let layrs_dir = root.join(".layrs");
    fs::create_dir_all(&layrs_dir).unwrap();
    let first = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    let nested = match LocalStoreSession::open_exclusive(&layrs_dir) {
        Ok(_) => panic!("exclusive sessions must not be reentrant"),
        Err(error) => error,
    };
    assert!(nested.contains("lock") || nested.contains("busy") || nested.contains("Busy"));
    drop(first);
    let reopened = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    drop(reopened);
    cleanup(&root);
}

#[test]
fn weave_state_remains_authoritative_without_legacy_json() {
    let root = test_root("weave-authority");
    let layrs_dir = root.join(".layrs");
    fs::create_dir_all(&layrs_dir).unwrap();
    let weave_path = layrs_dir.join("weaves/weave-1/session.json");
    let weave = json!({
        "schema": "layrs.weave_session.v1",
        "weaveId": "weave-1",
        "status": "conflicted",
        "conflicts": [{"conflictId": "conflict-1", "status": "open"}]
    });
    let mut store = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    store.write(&weave_path, &weave).unwrap();
    drop(store);
    assert!(!weave_path.exists());

    let reopened = LocalStoreSession::open_exclusive(&layrs_dir).unwrap();
    assert_eq!(reopened.read_value(&weave_path).unwrap(), weave);
    drop(reopened);
    cleanup(&root);
}

fn legacy_space(root: &Path, local_space_id: &str) -> Value {
    json!({
        "schema": "layrs.local_space.v1",
        "state": "draft",
        "localSpaceId": local_space_id,
        "spaceId": local_space_id,
        "serverSpaceId": null,
        "workspaceId": "",
        "name": "Migration test",
        "rootPath": root.display().to_string(),
        "createdAtUnix": 1,
        "updatedAtUnix": 1,
        "layers": []
    })
}

fn test_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "layrs-core-{label}-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn cleanup(root: &Path) {
    let layrs_dir = root.join(".layrs");
    forget_cached_store(&layrs_dir);
    let _ = fs::remove_dir_all(root);
}
