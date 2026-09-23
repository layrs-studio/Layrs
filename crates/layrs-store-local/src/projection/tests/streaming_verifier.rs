use super::*;
use layrs_core::scm::Chunk;
use std::cell::Cell;

#[test]
fn conflicting_legacy_and_chunk_representations_fail_closed() {
    let (fixture, store) = super::capture::ready(false);
    fs::write(fixture.0.join("a.txt"), b"captured").unwrap();
    let captured = store.capture_patch(1).unwrap();
    let file = captured.changes[0].replacement.as_ref().unwrap();
    let db = rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3"))
        .unwrap();
    db.execute(
        "INSERT INTO projection_capture_files(file_id,bytes) VALUES(?1,?2)",
        rusqlite::params![file.file_id.as_str(), b"corrupt!".as_slice()],
    )
    .unwrap();
    drop(db);
    assert!(store.captured_patch(captured.sequence).is_err());
    assert!(store.fsck().is_err());
}

#[test]
fn chunk_reader_rejects_self_consistent_row_for_another_file() {
    let (fixture, store) = super::capture::ready(false);
    fs::write(fixture.0.join("a.txt"), b"captured").unwrap();
    let captured = store.capture_patch(1).unwrap();
    let file = captured.changes[0].replacement.as_ref().unwrap();
    let verified = store
        .verify_captured_file(captured.sequence, &RepoPath::parse("a.txt").unwrap())
        .unwrap();
    let replacement = Chunk::new(b"corrupt!".to_vec()).unwrap();
    let db = rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3"))
        .unwrap();
    db.execute(
        "UPDATE projection_capture_chunks SET chunk_id=?2,bytes=?3 WHERE file_id=?1 AND ordinal=0",
        rusqlite::params![file.file_id.as_str(), replacement.id().as_str(), replacement.bytes()],
    )
    .unwrap();
    drop(db);
    assert!(store.captured_patch(captured.sequence).is_err());
    let mut output = Vec::new();
    assert!(
        store
            .read_captured_chunk(
                captured.sequence,
                &RepoPath::parse("a.txt").unwrap(),
                0,
                &mut output,
            )
            .is_err()
    );
    assert!(output.is_empty());
    assert!(
        store
            .read_verified_captured_chunk(&verified, 0, &mut output)
            .is_err()
    );
    assert!(output.is_empty());
}

#[test]
fn verified_chunk_witness_survives_checkout_edits_and_rejects_oversized_rows() {
    let (fixture, store) = super::capture::ready(false);
    fs::write(fixture.0.join("a.txt"), b"captured").unwrap();
    let captured = store.capture_patch(1).unwrap();
    let verified = store
        .verify_captured_file(captured.sequence, &RepoPath::parse("a.txt").unwrap())
        .unwrap();
    fs::write(fixture.0.join("a.txt"), b"new live content").unwrap();
    let reopened = ProjectionStore::open(&fixture.0).unwrap();
    let mut output = Vec::new();
    reopened
        .read_verified_captured_chunk(&verified, 0, &mut output)
        .unwrap();
    assert_eq!(output, b"captured");
    output.clear();
    assert!(
        reopened
            .read_verified_captured_chunk(&verified, usize::MAX, &mut output)
            .is_err()
    );
    assert!(output.is_empty());
    let db = rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3"))
        .unwrap();
    db.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE projection_capture_chunks SET bytes=zeroblob(2097152); PRAGMA ignore_check_constraints=OFF;")
        .unwrap();
    drop(db);
    assert!(reopened.captured_patch(captured.sequence).is_err());
    assert!(
        reopened
            .read_verified_captured_chunk(&verified, 0, &mut output)
            .is_err()
    );
    assert!(output.is_empty());
}

#[test]
fn restore_rechecks_installed_identity_after_last_path_hook() {
    struct ReplaceInstalled {
        root: PathBuf,
        replaced: Cell<bool>,
    }
    impl RestoreFaults for ReplaceInstalled {
        fn check(&self, point: RestorePoint) -> io::Result<()> {
            if point == RestorePoint::PathInstalled && !self.replaced.replace(true) {
                let path = self.root.join("a.txt");
                fs::rename(&path, self.root.join(".layrs/saved-original-stage"))?;
                fs::write(&path, b"captured")?;
            }
            Ok(())
        }
    }
    let (fixture, store) = super::capture::ready(false);
    fs::write(fixture.0.join("a.txt"), b"captured").unwrap();
    let captured = store.capture_patch(1).unwrap();
    fs::write(fixture.0.join("a.txt"), b"uncaptured original").unwrap();
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: captured.sequence,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    let result = store.apply_restore_with_faults(
        &preview,
        &ReplaceInstalled {
            root: fixture.0.clone(),
            replaced: Cell::new(false),
        },
    );
    assert!(result.is_err());
    assert_eq!(fs::read(fixture.0.join("a.txt")).unwrap(), b"captured");
    assert_eq!(
        fs::read(fixture.0.join(".layrs/saved-original-stage")).unwrap(),
        b"captured"
    );
    assert!(ProjectionStore::resume_restore(&fixture.0).is_err());
}

#[test]
fn restore_rechecks_deleted_path_absence_before_commit() {
    struct RecreateAtCommit(PathBuf);
    impl RestoreFaults for RecreateAtCommit {
        fn check(&self, point: RestorePoint) -> io::Result<()> {
            if point == RestorePoint::BeforeCommit {
                fs::write(self.0.join("a.txt"), b"external new bytes")?;
            }
            Ok(())
        }
    }
    let (fixture, store) = super::capture::ready(false);
    fs::remove_file(fixture.0.join("a.txt")).unwrap();
    let captured = store.capture_patch(1).unwrap();
    fs::write(fixture.0.join("a.txt"), b"uncaptured original").unwrap();
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: captured.sequence,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    assert!(store
        .apply_restore_with_faults(&preview, &RecreateAtCommit(fixture.0.clone()))
        .is_err());
    assert_eq!(fs::read(fixture.0.join("a.txt")).unwrap(), b"external new bytes");
    assert!(ProjectionStore::resume_restore(&fixture.0).is_err());
}

#[test]
fn failed_schema_migration_rolls_back_all_ddl_and_retries() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let old_sql = database::SQL
        .lines()
        .filter(|line| !line.starts_with("CREATE TABLE projection_capture_chunks("))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("user_version=4", "user_version=3")
        .replace("projection_namespace_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608))", "projection_namespace_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=65536))")
        .replace("projection_namespace_operations(preview_digest TEXT PRIMARY KEY CHECK(length(preview_digest)=71),capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=8388608))", "projection_namespace_operations(preview_digest TEXT PRIMARY KEY CHECK(length(preview_digest)=71),capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=65536))");
    db.execute_batch(&old_sql).unwrap();
    db.execute_batch("PRAGMA ignore_check_constraints=ON; INSERT INTO projection_namespace_pending VALUES(1,zeroblob(8388609)); PRAGMA ignore_check_constraints=OFF;")
        .unwrap();
    assert!(database::migrate(&mut db).is_err());
    assert_eq!(db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0)).unwrap(), 3);
    assert_eq!(db.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name='projection_capture_chunks' OR name LIKE '%_old'", [], |r| r.get::<_, u32>(0)).unwrap(), 0);
    assert_eq!(db.query_row("SELECT length(payload) FROM projection_namespace_pending", [], |r| r.get::<_, u32>(0)).unwrap(), 8388609);
    db.execute("DELETE FROM projection_namespace_pending", []).unwrap();
    database::migrate(&mut db).unwrap();
    database::migrate(&mut db).unwrap();
    assert_eq!(db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0)).unwrap(), 4);
}
