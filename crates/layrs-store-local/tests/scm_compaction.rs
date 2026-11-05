use layrs_core::scm::StoreId;
use layrs_store_local::scm_v1::{self, compaction::*};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "layrs-compact-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        drop(scm_v1::init(&root, StoreId::parse("store:compaction").unwrap()).unwrap());
        Self(root)
    }
    fn database(&self) -> PathBuf {
        self.0.join("metadata/layrs.sqlite3")
    }
    fn inflate(&self) {
        let _guard = scm_v1::open_exclusive(&self.0).unwrap();
        let db = rusqlite::Connection::open(self.database()).unwrap();
        db.execute(
            "INSERT INTO pending_objects VALUES ('fixture','fixture','chunk_raw',?1,'abandoned',0)",
            ["x".repeat(2 * 1024 * 1024)],
        )
        .unwrap();
        db.execute(
            "DELETE FROM pending_objects WHERE transaction_id='fixture'",
            [],
        )
        .unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
struct Stop(CompactionFaultPoint);
impl CompactionFaults for Stop {
    fn check(&self, point: CompactionFaultPoint) -> CompactionResult<()> {
        if point == self.0 {
            Err("injected stop".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn physical_compaction_reclaims_pages_without_changing_rows_and_retains_backup() {
    let f = Fixture::new();
    f.inflate();
    let before = fs::read(f.database()).unwrap();
    let report = compact_metadata(&f.0).unwrap();
    println!(
        "physical SQLite compaction: {} -> {} bytes; {} rows retained",
        report.before_bytes, report.after_bytes, report.retained_rows
    );
    assert!(report.after_bytes < report.before_bytes);
    assert_eq!(
        fs::read(report.backup_directory.join("before.sqlite3")).unwrap(),
        before
    );
    assert!(!report.history_deleted);
    assert!(report.restart_history_traversal);
    drop(scm_v1::open(&f.0).unwrap());
    assert!(
        scm_v1::fsck_existing(&f.0, scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn interruptions_preserve_original_or_verified_replacement_and_resume() {
    for point in [
        CompactionFaultPoint::CandidateReady,
        CompactionFaultPoint::JournalDurable,
        CompactionFaultPoint::DatabasePublished,
        CompactionFaultPoint::CompletionDurable,
    ] {
        let f = Fixture::new();
        f.inflate();
        let before = fs::read(f.database()).unwrap();
        assert!(compact_metadata_with_faults(&f.0, &Stop(point)).is_err());
        if point == CompactionFaultPoint::CandidateReady {
            assert_eq!(fs::read(f.database()).unwrap(), before);
            drop(scm_v1::open(&f.0).unwrap());
            compact_metadata(&f.0).unwrap();
        } else {
            assert!(scm_v1::open(&f.0).is_err());
            assert!(scm_v1::open_exclusive(&f.0).is_err());
            assert!(scm_v1::open_audit(&f.0).is_err());
            let report = resume_metadata_compaction(&f.0).unwrap();
            assert_eq!(
                fs::read(report.backup_directory.join("before.sqlite3")).unwrap(),
                before
            );
            drop(scm_v1::open(&f.0).unwrap());
        }
        assert!(
            scm_v1::fsck_existing(&f.0, scm_v1::FsckOptions::default())
                .unwrap()
                .is_clean()
        );
    }
}

#[test]
fn reads_writes_and_pending_mutations_refuse_compaction_without_mutation() {
    let f = Fixture::new();
    let before = fs::read(f.database()).unwrap();
    let read = scm_v1::open(&f.0).unwrap();
    assert!(compact_metadata(&f.0).is_err());
    drop(read);
    let write = scm_v1::open_exclusive(&f.0).unwrap();
    assert!(compact_metadata(&f.0).is_err());
    drop(write);
    assert_eq!(fs::read(f.database()).unwrap(), before);
    let db = rusqlite::Connection::open(f.database()).unwrap();
    db.execute("INSERT INTO pending_objects VALUES ('fixture','fixture','chunk_raw','staging/fixture','staged',0)",[]).unwrap();
    drop(db);
    let before = fs::read(f.database()).unwrap();
    assert!(compact_metadata(&f.0).is_err());
    assert_eq!(fs::read(f.database()).unwrap(), before);
    assert!(!f.0.join("metadata/metadata-compaction.json").exists());
}

#[test]
#[ignore = "subprocess helper invoked by cross_process_reader_blocks_compaction"]
fn compaction_lock_child() {
    use std::io::{Read, Write};
    let root = std::env::var_os("LAYRS_COMPACTION_CHILD_ROOT").expect("parent fixture");
    let _reader = scm_v1::open(root).unwrap();
    println!("COMPACTION_READER_READY");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0_u8]).unwrap();
}

#[test]
fn cross_process_reader_blocks_compaction() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };
    let f = Fixture::new();
    let before = fs::read(f.database()).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "compaction_lock_child",
            "--nocapture",
        ])
        .env("LAYRS_COMPACTION_CHILD_ROOT", &f.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(output.read_line(&mut line).unwrap() > 0);
        if line.contains("COMPACTION_READER_READY") {
            break;
        }
    }
    let result = compact_metadata(&f.0);
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(result.is_err());
    assert_eq!(fs::read(f.database()).unwrap(), before);
    compact_metadata(&f.0).unwrap();
}

#[test]
fn altered_candidate_backup_and_journal_refuse_resume_preserving_live_database() {
    let f = Fixture::new();
    f.inflate();
    let before = fs::read(f.database()).unwrap();
    assert!(
        compact_metadata_with_faults(&f.0, &Stop(CompactionFaultPoint::JournalDurable)).is_err()
    );
    let active = f.0.join("metadata/metadata-compaction.json");
    let original_journal = fs::read(&active).unwrap();
    let record: serde_json::Value = serde_json::from_slice(&original_journal).unwrap();
    let directory = PathBuf::from(record["report"]["backupDirectory"].as_str().unwrap());
    for path in [
        directory.join("compacted.sqlite3"),
        directory.join("before.sqlite3"),
        active.clone(),
    ] {
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"altered").unwrap();
        assert!(resume_metadata_compaction(&f.0).is_err());
        assert_eq!(fs::read(f.database()).unwrap(), before);
        assert!(scm_v1::open(&f.0).is_err());
        fs::write(&path, original).unwrap();
    }
    resume_metadata_compaction(&f.0).unwrap();
}
