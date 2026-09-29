//! A writer killed after commit leaves committed frames in the SQLite WAL.
//! The Store must reopen with those frames applied instead of refusing.

use layrs_core::scm::StoreId;
use layrs_store_local::scm_v1::{init, open, open_exclusive};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "layrs-crash-reopen-{label}-{}-{nonce}-{serial}",
            std::process::id()
        )))
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn sqlite_path(root: &Path) -> PathBuf {
    root.join("metadata").join("layrs.sqlite3")
}

/// Commits one row through a live WAL writer, then snapshots the Store while
/// that writer is still open: the copy is the on-disk state a kill leaves.
fn crashed_store(label: &str) -> (TestRoot, TestRoot) {
    let live = TestRoot::new(&format!("{label}-live"));
    drop(init(&live.0, StoreId::parse(format!("store:{label}")).unwrap()).unwrap());
    let writer = Connection::open(sqlite_path(&live.0)).unwrap();
    writer
        .execute(
            "INSERT INTO spaces(space_id, payload_json, created_at_ms) VALUES ('space:crash', '{}', 0)",
            [],
        )
        .unwrap();
    let crashed = TestRoot::new(&format!("{label}-crashed"));
    copy_tree(&live.0, &crashed.0);
    drop(writer);
    let wal = crashed.0.join("metadata").join("layrs.sqlite3-wal");
    assert!(
        fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false),
        "fixture must leave committed frames in the WAL"
    );
    (live, crashed)
}

fn committed_row_survives(root: &Path) {
    let reader = Connection::open(sqlite_path(root)).unwrap();
    let count: u32 = reader
        .query_row(
            "SELECT COUNT(*) FROM spaces WHERE space_id = 'space:crash'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "committed row must survive the crash");
}

#[test]
fn open_recovers_committed_wal_left_by_killed_writer() {
    let (_live, crashed) = crashed_store("open");
    drop(open(&crashed.0).expect("Store must reopen after a writer crash"));
    let wal = crashed.0.join("metadata").join("layrs.sqlite3-wal");
    assert!(!wal.is_file() || fs::metadata(&wal).unwrap().len() == 0);
    committed_row_survives(&crashed.0);
}

#[test]
fn open_exclusive_recovers_committed_wal_left_by_killed_writer() {
    let (_live, crashed) = crashed_store("exclusive");
    drop(open_exclusive(&crashed.0).expect("Store must reopen after a writer crash"));
    drop(open(&crashed.0).expect("recovered Store must open read-only"));
    committed_row_survives(&crashed.0);
}
