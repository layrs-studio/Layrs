use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use layrs_store_local::{
    CURRENT_SCHEMA_VERSION, LocalStore, LocalStoreConfig, SessionMode, StoreError, fsck_existing,
};
use rusqlite::{Connection, OptionalExtension};

#[test]
fn fresh_store_is_real_strict_sqlite_with_verified_pragmas() {
    let root = unique_test_dir("fresh");
    let config = LocalStoreConfig::new(&root);
    let store = LocalStore::init(config).expect("initialize store");

    let header = fs::read(&store.layout().sqlite_path).expect("read SQLite header");
    assert!(header.starts_with(b"SQLite format 3\0"));
    assert!(
        !store
            .layout()
            .metadata_dir
            .join("store-layout.txt")
            .exists()
    );

    let session = store
        .session(SessionMode::Shared)
        .expect("open shared session");
    let pragmas = session.pragmas().expect("read pragmas");
    assert!(version_at_least(&pragmas.sqlite_version, (3, 46, 0)));
    assert_eq!(pragmas.journal_mode, "wal");
    assert_eq!(pragmas.synchronous, 2);
    assert!(pragmas.foreign_keys);
    assert!(!pragmas.trusted_schema);
    assert!(pragmas.query_only);
    assert_eq!(pragmas.busy_timeout_ms, 5_000);
    assert_eq!(pragmas.wal_autocheckpoint_pages, 1_000);

    let required_tables = [
        "schema_migrations",
        "store_meta",
        "local_spaces",
        "layers",
        "layer_access_rules",
        "refs",
        "steps",
        "step_paths",
        "pending_publish",
        "pending_layer_deletions",
        "sync_cursors",
        "scan_cache",
        "weave_sessions",
        "weave_steps",
        "weave_conflicts",
        "weave_blocks",
        "weave_segments",
        "weave_resolutions",
        "operation_journal",
        "operation_paths",
        "gc_roots",
        "object_index",
        "object_edges",
    ];
    session
        .repository()
        .with_read(|connection| {
            for table in required_tables {
                let sql: String = connection.query_row(
                    "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )?;
                assert!(sql.contains("STRICT"), "{table} is not STRICT");
            }
            let user_version: u32 =
                connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
            assert_eq!(user_version, CURRENT_SCHEMA_VERSION);
            Ok(())
        })
        .expect("inspect schema");

    drop(session);
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn migration_checksum_tampering_blocks_open() {
    let root = unique_test_dir("checksum");
    let config = LocalStoreConfig::new(&root);
    let store = LocalStore::init(config.clone()).expect("initialize store");
    let sqlite_path = store.layout().sqlite_path.clone();
    drop(store);

    let connection = Connection::open(sqlite_path).expect("open raw SQLite");
    connection
        .execute(
            "UPDATE schema_migrations SET checksum = ?1 WHERE version = 1",
            ["0".repeat(64)],
        )
        .expect("tamper checksum");
    drop(connection);

    let error = LocalStore::open(config).expect_err("checksum mismatch must block open");
    assert!(error.to_string().contains("checksum mismatch"));

    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn two_shared_handles_coexist_and_exclusive_contention_is_explicit() {
    let root = unique_test_dir("contention");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let first = store
        .session(SessionMode::Shared)
        .expect("first shared session");
    let second = store
        .session(SessionMode::Shared)
        .expect("second shared session");

    assert!(matches!(
        store.session(SessionMode::Exclusive),
        Err(StoreError::ReentrantLock(_))
    ));
    drop(first);
    drop(second);

    let exclusive = store
        .session(SessionMode::Exclusive)
        .expect("exclusive after readers close");
    assert!(matches!(
        store.session(SessionMode::Exclusive),
        Err(StoreError::ReentrantLock(_))
    ));
    drop(exclusive);
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn exclusive_lock_contends_across_real_processes() {
    let root = unique_test_dir("process-contention");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let ready = root.join("tmp").join("child-lock-ready");
    let release = root.join("tmp").join("child-lock-release");
    let mut child = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--ignored")
        .arg("--exact")
        .arg("child_holds_exclusive_store_lock")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("LAYRS_LOCK_HELPER_ROOT", &root)
        .env("LAYRS_LOCK_HELPER_READY", &ready)
        .env("LAYRS_LOCK_HELPER_RELEASE", &release)
        .spawn()
        .expect("spawn lock helper process");

    if !wait_for_path(&ready, Duration::from_secs(10)) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child process did not acquire the store lock");
    }
    let contention = store.session(SessionMode::Exclusive);
    fs::write(&release, b"release").expect("release child lock");
    let status = child.wait().expect("wait for lock helper");

    assert!(status.success());
    assert!(matches!(contention, Err(StoreError::LockBusy(_))));
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
#[ignore = "spawned by exclusive_lock_contends_across_real_processes"]
fn child_holds_exclusive_store_lock() {
    let Ok(root) = std::env::var("LAYRS_LOCK_HELPER_ROOT") else {
        return;
    };
    let ready =
        PathBuf::from(std::env::var("LAYRS_LOCK_HELPER_READY").expect("lock helper ready path"));
    let release = PathBuf::from(
        std::env::var("LAYRS_LOCK_HELPER_RELEASE").expect("lock helper release path"),
    );
    let store = LocalStore::open(LocalStoreConfig::new(root)).expect("open helper store");
    let _session = store
        .session(SessionMode::Exclusive)
        .expect("acquire helper lock");
    fs::write(&ready, b"ready").expect("signal helper ready");
    assert!(
        wait_for_path(&release, Duration::from_secs(15)),
        "parent did not release helper lock"
    );
}

#[test]
fn read_only_fsck_creates_no_files_or_sqlite_sidecars() {
    let root = unique_test_dir("fsck-no-files");
    let config = LocalStoreConfig::new(&root);
    let store = LocalStore::init(config.clone()).expect("initialize store");
    drop(store);
    let before = snapshot_entries(&root);
    assert!(!has_sqlite_sidecar(&before));

    let report = fsck_existing(&config).expect("run read-only fsck");
    assert!(report.is_clean());
    let after = snapshot_entries(&root);

    assert_eq!(after, before);
    assert!(!has_sqlite_sidecar(&after));
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn read_only_fsck_refuses_nonempty_wal_without_creating_shm() {
    let root = unique_test_dir("fsck-uncheckpointed-wal");
    let config = LocalStoreConfig::new(&root);
    let store = LocalStore::init(config.clone()).expect("initialize store");
    let wal_path = PathBuf::from(format!("{}-wal", store.layout().sqlite_path.display()));
    drop(store);
    fs::write(&wal_path, b"uncheckpointed").expect("create WAL sentinel");
    let before = snapshot_entries(&root);

    assert!(matches!(
        fsck_existing(&config),
        Err(StoreError::Conflict(_))
    ));
    let after = snapshot_entries(&root);
    assert_eq!(after, before);
    assert!(
        !after
            .keys()
            .any(|path| path.to_string_lossy().ends_with("-shm"))
    );

    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn fsck_existing_does_not_create_a_missing_store() {
    let root = unique_test_dir("missing-fsck");
    let config = LocalStoreConfig::new(&root);

    assert!(fsck_existing(&config).is_err());
    assert!(!root.exists());
}

#[test]
fn schema_history_has_no_missing_versions() {
    let root = unique_test_dir("history");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let connection = Connection::open(&store.layout().sqlite_path).expect("open SQLite");
    let missing: Option<u32> = connection
        .query_row(
            "WITH RECURSIVE expected(version) AS (
                SELECT 1 UNION ALL SELECT version + 1 FROM expected WHERE version < ?1
             )
             SELECT expected.version FROM expected
             LEFT JOIN schema_migrations USING (version)
             WHERE schema_migrations.version IS NULL LIMIT 1",
            [CURRENT_SCHEMA_VERSION],
            |row| row.get(0),
        )
        .optional()
        .expect("query migration gaps");
    assert_eq!(missing, None);

    drop(connection);
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

fn version_at_least(version: &str, minimum: (u32, u32, u32)) -> bool {
    let parts: Vec<_> = version
        .split('.')
        .map(|part| part.parse::<u32>().expect("numeric SQLite version"))
        .collect();
    (parts[0], parts[1], parts[2]) >= minimum
}

fn wait_for_path(path: &std::path::Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

fn snapshot_entries(root: &std::path::Path) -> BTreeMap<PathBuf, (bool, u64)> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        entries: &mut BTreeMap<PathBuf, (bool, u64)>,
    ) {
        for entry in fs::read_dir(directory).expect("read snapshot directory") {
            let entry = entry.expect("read snapshot entry");
            let metadata = entry.metadata().expect("read snapshot metadata");
            let path = entry.path();
            entries.insert(
                path.strip_prefix(root)
                    .expect("relative snapshot path")
                    .into(),
                (metadata.is_dir(), metadata.len()),
            );
            if metadata.is_dir() {
                visit(root, &path, entries);
            }
        }
    }

    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn has_sqlite_sidecar(entries: &BTreeMap<PathBuf, (bool, u64)>) -> bool {
    entries.keys().any(|path| {
        let name = path.to_string_lossy();
        name.ends_with("-wal") || name.ends_with("-shm")
    })
}

fn unique_test_dir(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "layrs-store-local-integration-{label}-{}-{timestamp}",
        std::process::id()
    ))
}
