use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_store_local::{
    FsckCode, LocalStore, LocalStoreConfig, ObjectRecord, SessionMode, SpaceRecord, StoreError,
    normalize_path_key, path_has_reparse_point,
};
use rusqlite::Connection;

#[test]
fn fsck_detects_object_corruption_and_repair_only_quarantines_metadata() {
    let root = unique_test_dir("object-corruption");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let original = b"object bytes";
    let digest = layrs_sync::ObjectDigest::blake3_for(original);
    let outcome = store
        .write_object_v2(original, &digest)
        .expect("write object");
    let relative_path = outcome
        .path
        .strip_prefix(&store.layout().cas_dir)
        .expect("relative path")
        .to_string_lossy()
        .replace('\\', "/");
    let mut session = store
        .session(SessionMode::Exclusive)
        .expect("exclusive session");
    session
        .repository_mut()
        .insert_space(&space_record(&root))
        .expect("insert space");
    session
        .repository_mut()
        .insert_object(&ObjectRecord {
            object_id: digest.hex().into(),
            algorithm: "blake3".into(),
            byte_len: original.len() as u64,
            object_kind: "opaque".into(),
            relative_path,
            state: "available".into(),
            verified_at_ms: Some(2),
            created_at_ms: 2,
        })
        .expect("index object");
    drop(session);

    let corrupt = b"broken bytes";
    assert_eq!(corrupt.len(), original.len());
    fs::write(&outcome.path, corrupt).expect("corrupt object");
    let report = store.fsck().expect("run fsck");
    assert!(
        report
            .issues_with_code(FsckCode::ObjectCorrupt)
            .any(|issue| issue.entity.as_deref() == Some(digest.hex()))
    );

    let repair = store.repair(&report, 3).expect("quarantine metadata");
    assert_eq!(repair.actions.len(), 1);
    assert!(repair.backup_path.is_file());
    assert!(
        fs::read(&repair.backup_path)
            .expect("read repair backup")
            .starts_with(b"SQLite format 3\0")
    );
    assert!(!sidecar_path(&repair.backup_path, "-wal").exists());
    assert!(!sidecar_path(&repair.backup_path, "-shm").exists());
    assert_eq!(fs::read(&outcome.path).expect("read bytes"), corrupt);
    let session = store.session(SessionMode::Shared).expect("shared session");
    let state: String = session
        .repository()
        .with_read(|connection| {
            Ok(connection.query_row(
                "SELECT state FROM object_index WHERE object_id = ?1",
                [digest.hex()],
                |row| row.get(0),
            )?)
        })
        .expect("read object state");
    assert_eq!(state, "quarantined");

    drop(session);
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn failed_repair_backup_leaves_metadata_unchanged() {
    let root = unique_test_dir("backup-failure");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let mut session = store
        .session(SessionMode::Exclusive)
        .expect("exclusive session");
    session
        .repository_mut()
        .insert_space(&space_record(&root))
        .expect("insert space");
    drop(session);

    let connection = Connection::open(&store.layout().sqlite_path).expect("open raw SQLite");
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .expect("disable foreign keys");
    connection
        .execute_batch(
            "INSERT INTO layers (
                layer_id, space_id, name, layer_kind, state, base_layer_id,
                timeline_active, generation, created_at_ms, updated_at_ms
             ) VALUES ('layer-1', 'space-1', 'main', 'local', 'active', NULL, 0, 0, 1, 1);
             INSERT INTO refs (
                space_id, ref_name, ref_kind, layer_id, step_id, generation, updated_at_ms
             ) VALUES ('space-1', 'head', 'layer_head', 'layer-1', 'missing-step', 1, 1);",
        )
        .expect("insert invalid ref fixture");
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("checkpoint fixture");
    drop(connection);
    let findings = store.fsck().expect("fsck invalid ref fixture");
    assert!(
        findings
            .issues_with_code(FsckCode::RefInvariant)
            .next()
            .is_some()
    );

    fs::write(store.layout().repair_backup_dir(), b"blocks directory")
        .expect("create backup blocker");
    store
        .repair(&findings, 4)
        .expect_err("backup failure must abort repair");

    let session = store.session(SessionMode::Shared).expect("shared session");
    let (state, quarantine_generation): (String, i64) = session
        .repository()
        .with_read(|connection| {
            Ok(connection.query_row(
                "SELECT s.state, m.quarantine_generation
                 FROM local_spaces s CROSS JOIN store_meta m
                 WHERE s.space_id = 'space-1' AND m.singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .expect("read unchanged metadata");
    assert_eq!(state, "active");
    assert_eq!(quarantine_generation, 0);

    drop(session);
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn fsck_reports_foreign_key_corruption_inserted_with_checks_disabled() {
    let root = unique_test_dir("foreign-key");
    let config = LocalStoreConfig::new(&root);
    let store = LocalStore::init(config.clone()).expect("initialize store");
    let sqlite_path = store.layout().sqlite_path.clone();
    drop(store);

    let connection = Connection::open(sqlite_path).expect("open raw SQLite");
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .expect("disable foreign keys");
    connection
        .execute(
            "INSERT INTO layers (
                layer_id, space_id, name, layer_kind, state, base_layer_id,
                timeline_active, generation, created_at_ms, updated_at_ms
             ) VALUES ('orphan-layer', 'missing-space', 'orphan', 'local',
                       'active', NULL, 0, 0, 1, 1)",
            [],
        )
        .expect("insert orphan layer");
    drop(connection);

    let report = layrs_store_local::fsck_existing(&config).expect("run read-only fsck");
    assert!(
        report
            .issues_with_code(FsckCode::ForeignKey)
            .next()
            .is_some()
    );

    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn repair_refuses_report_from_another_store_identity() {
    let (first_root, first, _, _, _) = corrupted_object_store("cross-store-source");
    let report = first.fsck().expect("fsck source store");
    let second_root = unique_test_dir("cross-store-target");
    let second = LocalStore::init(LocalStoreConfig::new(&second_root)).expect("init target store");

    assert!(matches!(
        second.repair(&report, 10),
        Err(StoreError::InvalidFsckReport(_))
    ));
    assert!(!second.layout().repair_backup_dir().exists());

    drop(first);
    drop(second);
    fs::remove_dir_all(first_root).expect("remove source root");
    fs::remove_dir_all(second_root).expect("remove target root");
}

#[test]
fn repair_refuses_report_when_a_finding_was_fixed_after_fsck() {
    let (root, store, object_path, original, _) = corrupted_object_store("stale-fixed");
    let report = store.fsck().expect("fsck corrupted object");
    fs::write(&object_path, &original).expect("restore object after fsck");

    assert!(matches!(
        store.repair(&report, 11),
        Err(StoreError::InvalidFsckReport(_))
    ));
    assert_eq!(
        fs::read_dir(store.layout().repair_backup_dir())
            .expect("read cleaned backup directory")
            .count(),
        0
    );

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn repair_refuses_report_after_quarantine_generation_advances() {
    let (root, store, _, _, _) = corrupted_object_store("stale-generation");
    let report = store.fsck().expect("fsck corrupted object");
    store.repair(&report, 12).expect("first repair");

    assert!(matches!(
        store.repair(&report, 13),
        Err(StoreError::InvalidFsckReport(_))
    ));

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn path_helpers_normalize_case_and_recognize_regular_directories() {
    let root = unique_test_dir("path-helper");
    fs::create_dir_all(&root).expect("create root");
    assert!(!path_has_reparse_point(&root).expect("inspect directory"));

    #[cfg(windows)]
    assert_eq!(
        normalize_path_key(Path::new(r"C:\Workspace\Foo\..\Bar"), false),
        "c:/workspace/bar"
    );
    #[cfg(not(windows))]
    assert_eq!(
        normalize_path_key(Path::new("/Work/Foo/../Bar"), false),
        "/work/bar"
    );

    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn configurable_layout_names_reject_traversal_absolute_and_nested_paths() {
    let base = unique_test_dir("invalid-config-names");
    fs::create_dir_all(&base).expect("create base");

    let mut cases = Vec::new();
    let mut cas = LocalStoreConfig::new(base.join("cas-store"));
    cas.cas_dir_name = "../outside-cas".to_string();
    cases.push(cas);
    let mut metadata = LocalStoreConfig::new(base.join("metadata-store"));
    metadata.metadata_dir_name = "../outside-metadata".to_string();
    cases.push(metadata);
    let mut temporary = LocalStoreConfig::new(base.join("tmp-store"));
    temporary.tmp_dir_name = Path::new("nested")
        .join("tmp")
        .to_string_lossy()
        .into_owned();
    cases.push(temporary);
    let mut sqlite = LocalStoreConfig::new(base.join("sqlite-store"));
    sqlite.sqlite_file_name = "../outside.sqlite3".to_string();
    cases.push(sqlite);

    let absolute_target = base.join("absolute-target");
    let mut absolute = LocalStoreConfig::new(base.join("absolute-store"));
    absolute.cas_dir_name = absolute_target.to_string_lossy().into_owned();
    cases.push(absolute);

    for config in cases {
        let root = config.root.clone();
        let error = LocalStore::init(config).expect_err("invalid component must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!root.exists(), "invalid config created {}", root.display());
    }
    assert!(!absolute_target.exists());
    assert!(!base.join("outside-cas").exists());
    assert!(!base.join("outside-metadata").exists());
    assert!(!base.join("outside.sqlite3").exists());

    fs::remove_dir_all(base).expect("remove base");
}

#[test]
fn preplaced_reparse_points_at_store_boundaries_are_rejected() {
    for boundary in ["objects", "metadata", "tmp"] {
        let base = unique_test_dir(&format!("reparse-{boundary}"));
        let root = base.join("store");
        let outside = base.join("outside");
        fs::create_dir_all(&root).expect("create root");
        fs::create_dir_all(&outside).expect("create outside");
        let link = root.join(boundary);
        create_directory_link(&outside, &link);

        let error = LocalStore::init(LocalStoreConfig::new(&root))
            .expect_err("preplaced reparse boundary must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            fs::read_dir(&outside).expect("read outside").count(),
            0,
            "store wrote through {boundary} reparse boundary"
        );

        remove_directory_link(&link);
        fs::remove_dir_all(base).expect("remove fixture");
    }

    let base = unique_test_dir("reparse-root");
    let outside = base.join("outside-root");
    let linked_root = base.join("linked-root");
    fs::create_dir_all(&outside).expect("create outside root");
    create_directory_link(&outside, &linked_root);
    assert_eq!(
        LocalStore::init(LocalStoreConfig::new(&linked_root))
            .expect_err("reparse root must fail")
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        fs::read_dir(&outside).expect("read outside root").count(),
        0
    );
    remove_directory_link(&linked_root);
    fs::remove_dir_all(base).expect("remove root fixture");
}

#[test]
fn cas_publish_refuses_a_preplaced_reparse_shard() {
    let base = unique_test_dir("reparse-cas-shard");
    let root = base.join("store");
    let outside = base.join("outside");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    fs::create_dir_all(&outside).expect("create outside");
    let algorithm_boundary = store.layout().cas_dir.join("blake3");
    create_directory_link(&outside, &algorithm_boundary);
    let bytes = b"must remain inside the CAS root";
    let digest = layrs_sync::ObjectDigest::blake3_for(bytes);

    let error = store
        .write_object_v2(bytes, &digest)
        .expect_err("reparse shard must reject publication");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(fs::read_dir(&outside).expect("read outside").count(), 0);

    remove_directory_link(&algorithm_boundary);
    drop(store);
    fs::remove_dir_all(base).expect("remove fixture");
}

fn space_record(root: &Path) -> SpaceRecord {
    SpaceRecord {
        space_id: "space-1".into(),
        canonical_root: root.to_string_lossy().into_owned(),
        display_name: None,
        case_sensitive: false,
        identity_token: None,
        state: "active".into(),
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn corrupted_object_store(label: &str) -> (PathBuf, LocalStore, PathBuf, Vec<u8>, String) {
    let root = unique_test_dir(label);
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let original = b"bound report object".to_vec();
    let digest = layrs_sync::ObjectDigest::blake3_for(&original);
    let outcome = store
        .write_object_v2(&original, &digest)
        .expect("write object");
    let relative_path = outcome
        .path
        .strip_prefix(&store.layout().cas_dir)
        .expect("relative path")
        .to_string_lossy()
        .replace('\\', "/");
    let mut session = store
        .session(SessionMode::Exclusive)
        .expect("exclusive session");
    session
        .repository_mut()
        .insert_space(&space_record(&root))
        .expect("insert space");
    session
        .repository_mut()
        .insert_object(&ObjectRecord {
            object_id: digest.hex().to_string(),
            algorithm: "blake3".to_string(),
            byte_len: original.len() as u64,
            object_kind: "opaque".to_string(),
            relative_path,
            state: "available".to_string(),
            verified_at_ms: Some(2),
            created_at_ms: 2,
        })
        .expect("index object");
    drop(session);
    fs::write(&outcome.path, vec![b'x'; original.len()]).expect("corrupt object");
    (
        root,
        store,
        outcome.path,
        original,
        digest.hex().to_string(),
    )
}

fn sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", database.display()))
}

fn unique_test_dir(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "layrs-store-local-fsck-{label}-{}-{timestamp}",
        std::process::id()
    ))
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create directory symlink");
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("run mklink");
    assert!(status.success(), "create directory junction");
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) {
    fs::remove_file(link).expect("remove directory symlink");
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) {
    fs::remove_dir(link).expect("remove directory junction");
}
