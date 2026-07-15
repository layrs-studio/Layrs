use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_store_local::{
    CURRENT_SCHEMA_VERSION, FsckCode, FsckIssue, FsckReport, LocalStore, LocalStoreConfig,
    ObjectRecord, SessionMode, SpaceRecord, normalize_path_key, path_has_reparse_point,
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

    fs::write(store.layout().repair_backup_dir(), b"blocks directory")
        .expect("create backup blocker");
    let findings = FsckReport {
        sqlite_version: rusqlite::version().to_string(),
        schema_version: Some(CURRENT_SCHEMA_VERSION),
        checked_objects: 0,
        issues: vec![FsckIssue {
            code: FsckCode::RefInvariant,
            entity: Some("space-1".into()),
            detail: "synthetic repair gate finding".into(),
        }],
    };
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
