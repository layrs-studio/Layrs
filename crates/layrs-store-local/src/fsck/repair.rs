use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};

use super::report::{self, FsckCode, FsckReport};
use crate::error::{StoreError, StoreResult};
use crate::layout::{
    StoreLayout, create_dir_all_durable, create_new_file_no_follow, open_read_write_no_follow,
    sync_directory,
};
use crate::lock::SessionMode;
use crate::schema::CURRENT_SCHEMA_VERSION;
use crate::sqlite;
use crate::{LocalStore, MetadataRepository};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairAction {
    ObjectQuarantined(String),
    SpaceQuarantined(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RepairReport {
    pub backup_path: PathBuf,
    pub actions: Vec<RepairAction>,
    pub skipped_issues: usize,
}

/// Applies metadata-only quarantine actions. Object bytes are never created,
/// moved, rewritten, or removed by repair.
pub fn repair(
    store: &LocalStore,
    findings: &FsckReport,
    updated_at_ms: i64,
) -> StoreResult<RepairReport> {
    if updated_at_ms < 0 {
        return Err(StoreError::InvalidState(
            "repair timestamp must not be negative".to_string(),
        ));
    }
    let mut session = store.session(SessionMode::Exclusive)?;
    report::validate_header(store.layout(), session.repository().connection(), findings)?;
    let backup_path = create_repair_backup(store.layout(), session.repository(), updated_at_ms)?;
    let repair_result = repair_repository(
        store.layout(),
        session.repository_mut(),
        findings,
        updated_at_ms,
    );
    let mut result = match repair_result {
        Ok(result) => result,
        Err(error) => {
            remove_repair_backup(store.layout(), &backup_path);
            return Err(error);
        }
    };
    result.backup_path = backup_path;
    Ok(result)
}

fn create_repair_backup(
    layout: &StoreLayout,
    repository: &MetadataRepository,
    timestamp_ms: i64,
) -> StoreResult<PathBuf> {
    let source_signature: (u32, u64) = repository.connection().query_row(
        "SELECT schema_version, quarantine_generation FROM store_meta WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let backup_dir = layout.repair_backup_dir();
    create_dir_all_durable(&backup_dir)?;
    let backup_path = backup_dir.join(format!(
        "layrs-repair-{timestamp_ms}-q{}.sqlite3",
        source_signature.1
    ));
    match fs::symlink_metadata(&backup_path) {
        Ok(_) => {
            return Err(StoreError::Conflict(format!(
                "repair backup already exists: {}",
                backup_path.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let backup_result = (|| -> StoreResult<()> {
        let reservation = create_new_file_no_follow(&backup_path)?;
        let mut destination = Connection::open_with_flags(
            &backup_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        {
            let backup = rusqlite::backup::Backup::new(repository.connection(), &mut destination)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
        }
        let journal_mode: String =
            destination.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("delete") {
            return Err(StoreError::InvalidState(format!(
                "repair backup journal mode remained {journal_mode}"
            )));
        }
        drop(destination);
        drop(reservation);
        open_read_write_no_follow(&backup_path)?.sync_all()?;
        sync_directory(&backup_dir)?;
        validate_repair_backup(&backup_path, source_signature)?;
        Ok(())
    })();
    if let Err(error) = backup_result {
        cleanup_backup_files(&backup_dir, &backup_path);
        return Err(error);
    }
    Ok(backup_path)
}

fn remove_repair_backup(layout: &StoreLayout, backup_path: &Path) {
    cleanup_backup_files(&layout.repair_backup_dir(), backup_path);
}

fn cleanup_backup_files(backup_dir: &Path, backup_path: &Path) {
    let mut removed = false;
    for path in [
        backup_path.to_path_buf(),
        backup_sidecar_path(backup_path, "-wal"),
        backup_sidecar_path(backup_path, "-shm"),
    ] {
        match fs::remove_file(path) {
            Ok(()) => removed = true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
    if removed {
        let _ = sync_directory(backup_dir);
    }
}

fn backup_sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut value = database.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn validate_repair_backup(backup_path: &Path, source_signature: (u32, u64)) -> StoreResult<()> {
    let connection = Connection::open_with_flags(
        backup_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    connection.pragma_update(None, "query_only", "ON")?;
    let mut integrity = connection.prepare("PRAGMA integrity_check")?;
    let rows = integrity.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let message = row?;
        if !message.eq_ignore_ascii_case("ok") {
            return Err(StoreError::CorruptMetadata(format!(
                "repair backup integrity check failed: {message}"
            )));
        }
    }
    drop(integrity);
    sqlite::verify_applied_migrations(&connection, CURRENT_SCHEMA_VERSION)?;
    let backup_signature: (u32, u64) = connection.query_row(
        "SELECT schema_version, quarantine_generation FROM store_meta WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if backup_signature != source_signature {
        return Err(StoreError::CorruptMetadata(format!(
            "repair backup signature mismatch: expected {source_signature:?}, got {backup_signature:?}"
        )));
    }
    Ok(())
}

fn repair_repository(
    layout: &StoreLayout,
    repository: &mut MetadataRepository,
    findings: &FsckReport,
    updated_at_ms: i64,
) -> StoreResult<RepairReport> {
    let object_codes = [
        FsckCode::ObjectMissing,
        FsckCode::ObjectCorrupt,
        FsckCode::ObjectPathUnsafe,
    ];
    let space_codes = [
        FsckCode::RefInvariant,
        FsckCode::StepInvariant,
        FsckCode::PendingInvariant,
        FsckCode::JournalInvariant,
        FsckCode::TimelineInvariant,
        FsckCode::WeaveInvariant,
        FsckCode::GcInvariant,
    ];
    let objects: BTreeSet<_> = findings
        .issues
        .iter()
        .filter(|issue| object_codes.contains(&issue.code))
        .filter_map(|issue| issue.entity.clone())
        .collect();
    let spaces: BTreeSet<_> = findings
        .issues
        .iter()
        .filter(|issue| space_codes.contains(&issue.code))
        .filter_map(|issue| issue.entity.clone())
        .collect();
    let actionable = objects.len() + spaces.len();

    let actions = repository.with_immediate_transaction(|transaction| {
        let fresh = super::check_connection(layout, transaction)?;
        report::validate_snapshot(findings, &fresh)?;
        let mut actions = Vec::new();
        for object_id in &objects {
            let changed = transaction.execute(
                "UPDATE object_index SET state = 'quarantined'
                 WHERE object_id = ?1 AND state <> 'quarantined'",
                [object_id],
            )?;
            if changed == 1 {
                actions.push(RepairAction::ObjectQuarantined(object_id.clone()));
            }
        }
        for space_id in &spaces {
            let changed = transaction.execute(
                "UPDATE local_spaces SET state = 'quarantined', updated_at_ms = ?2
                 WHERE space_id = ?1 AND state <> 'quarantined'",
                params![space_id, updated_at_ms],
            )?;
            if changed == 1 {
                actions.push(RepairAction::SpaceQuarantined(space_id.clone()));
            }
        }
        if !actions.is_empty() {
            transaction.execute(
                "UPDATE store_meta
                 SET quarantine_generation = quarantine_generation + 1,
                     updated_at_ms = MAX(updated_at_ms, ?1)
                 WHERE singleton = 1",
                [updated_at_ms],
            )?;
        }
        Ok(actions)
    })?;

    Ok(RepairReport {
        backup_path: PathBuf::new(),
        skipped_issues: findings
            .issues
            .len()
            .saturating_sub(actionable)
            .saturating_add(usize::try_from(findings.omitted_issues).unwrap_or(usize::MAX)),
        actions,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::fault::{self, FaultPoint};
    use crate::{LocalStoreConfig, ObjectRecord};

    #[test]
    fn forged_report_binding_is_refused_before_backup() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "layrs-store-local-forged-report-{}-{timestamp}",
            std::process::id()
        ));
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
        let mut forged = store.fsck().expect("create genuine report");
        forged.binding.store_identity = "forged-store-identity".to_string();

        assert!(matches!(
            repair(&store, &forged, 1),
            Err(StoreError::InvalidFsckReport(_))
        ));
        assert!(!store.layout().repair_backup_dir().exists());

        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn publication_after_scan_fences_repair_and_preserves_available_object() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "layrs-store-local-repair-publication-race-{}-{timestamp}",
            std::process::id()
        ));
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
        let bytes = b"published after fsck".to_vec();
        let digest = layrs_sync::ObjectDigest::blake3_for(&bytes);
        let target = store.cas_path_v2(&digest);
        let relative_path = target
            .strip_prefix(&store.layout().cas_dir)
            .expect("relative CAS path")
            .to_string_lossy()
            .replace('\\', "/");
        let mut session = store
            .session(SessionMode::Exclusive)
            .expect("exclusive session");
        session
            .repository_mut()
            .insert_object(&ObjectRecord {
                object_id: digest.hex().to_string(),
                algorithm: "blake3".to_string(),
                byte_len: bytes.len() as u64,
                object_kind: "opaque".to_string(),
                relative_path,
                state: "available".to_string(),
                verified_at_ms: None,
                created_at_ms: 1,
            })
            .expect("index missing object");
        drop(session);
        let findings = store.fsck().expect("scan missing object");
        assert_eq!(
            findings.issues_with_code(FsckCode::ObjectMissing).count(),
            1
        );

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let writer_store = store.clone();
        let writer_entered = Arc::clone(&entered);
        let writer_release = Arc::clone(&release);
        let writer_digest = digest.clone();
        let writer = thread::spawn(move || {
            fault::pause_once(FaultPoint::BeforeTempHash, writer_entered, writer_release);
            writer_store.write_object_v2(&bytes, &writer_digest)
        });

        entered.wait();
        assert!(matches!(
            store.repair(&findings, 2),
            Err(StoreError::ReentrantLock(_) | StoreError::LockBusy(_))
        ));
        release.wait();
        writer
            .join()
            .expect("writer thread")
            .expect("publish valid object");
        assert_eq!(
            fs::read(&target).expect("read published object"),
            b"published after fsck"
        );
        assert!(!store.layout().repair_backup_dir().exists());
        assert!(matches!(
            store.repair(&findings, 2),
            Err(StoreError::InvalidFsckReport(_))
        ));
        let session = store.session(SessionMode::Shared).expect("shared session");
        let state: String = session
            .repository()
            .connection()
            .query_row(
                "SELECT state FROM object_index WHERE object_id = ?1",
                [digest.hex()],
                |row| row.get(0),
            )
            .expect("read object state");
        assert_eq!(state, "available");
        assert_eq!(
            fs::read_dir(store.layout().repair_backup_dir())
                .expect("read cleaned repair backup directory")
                .count(),
            0
        );

        drop(session);
        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }
}
