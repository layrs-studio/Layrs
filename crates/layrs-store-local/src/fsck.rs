//! Read-only consistency checks and metadata-only quarantine repair.

use std::io::{self, Read};
use std::path::{Component, Path};

use rusqlite::{Connection, OptionalExtension, params};

mod graph;
mod repair;
mod report;

pub use repair::{RepairAction, RepairReport, repair};
use report::MAX_FSCK_TRAVERSAL_ROWS;
pub use report::{FsckCode, FsckIssue, FsckReport};

use crate::error::StoreResult;
use crate::fault::{self, FaultPoint};
use crate::layout::{StoreLayout, validate_store_boundaries};
use crate::lock::{SessionMode, StoreLock};
use crate::schema::CURRENT_SCHEMA_VERSION;
use crate::secure_fs::PinnedDirectory;
use crate::sqlite::{self, SqliteOptions};

const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// Opens only an existing database and lock file. This path never creates or
/// migrates the store.
pub(crate) fn fsck_read_only(
    layout: &StoreLayout,
    options: SqliteOptions,
) -> StoreResult<FsckReport> {
    validate_store_boundaries(layout)?;
    let _lock = StoreLock::acquire_existing(&layout.lock_path(), SessionMode::Shared)?;
    let connection = sqlite::open_read_only_unchecked(&layout.sqlite_path, options)?;
    check_connection(layout, &connection)
}

pub fn fsck_existing(config: &crate::LocalStoreConfig) -> StoreResult<FsckReport> {
    let layout = config.validated_layout()?;
    fsck_read_only(&layout, config.sqlite_options())
}

pub(super) fn check_connection(
    layout: &StoreLayout,
    connection: &Connection,
) -> StoreResult<FsckReport> {
    let sqlite_version = connection.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    let metadata_snapshot = connection
        .query_row(
            "SELECT schema_version, quarantine_generation, created_at_ms, updated_at_ms
             FROM store_meta WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .unwrap_or(None);
    let schema_version = metadata_snapshot.map(|snapshot| snapshot.0);
    let quarantine_generation = metadata_snapshot.map(|snapshot| snapshot.1);
    let created_at_ms = metadata_snapshot.map(|snapshot| snapshot.2);
    let updated_at_ms = metadata_snapshot.map(|snapshot| snapshot.3);
    let schema_cookie: u32 =
        connection.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    let mut report = FsckReport::new(
        layout,
        sqlite_version,
        schema_version,
        quarantine_generation,
        created_at_ms,
    )?;

    check_integrity(connection, &mut report)?;
    check_foreign_keys(connection, &mut report)?;
    if let Err(error) = sqlite::verify_applied_migrations(connection, CURRENT_SCHEMA_VERSION) {
        push_issue(&mut report, FsckCode::Migration, None, error.to_string());
    }
    if schema_version != Some(CURRENT_SCHEMA_VERSION) {
        push_issue(
            &mut report,
            FsckCode::SchemaVersion,
            None,
            format!(
                "store_meta schema version {:?} does not equal {}",
                schema_version, CURRENT_SCHEMA_VERSION
            ),
        );
    }

    if has_table(connection, "refs")? {
        check_relational_invariants(connection, &mut report)?;
        check_object_graph(connection, &mut report)?;
        check_indexed_objects(layout, connection, &mut report)?;
    }
    report.bind_snapshot(schema_cookie, updated_at_ms);
    Ok(report)
}

fn check_integrity(connection: &Connection, report: &mut FsckReport) -> StoreResult<()> {
    let mut statement = connection.prepare("PRAGMA integrity_check(101)")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let message = row?;
        if !message.eq_ignore_ascii_case("ok") {
            if report.remaining_issue_capacity() == 0 {
                report.note_issue_query_truncated();
                break;
            }
            push_issue(report, FsckCode::Integrity, None, message);
        }
    }
    Ok(())
}

fn check_foreign_keys(connection: &Connection, report: &mut FsckReport) -> StoreResult<()> {
    let limit = report.issue_query_limit();
    let mut statement = connection
        .prepare("SELECT * FROM pragma_foreign_key_check ORDER BY 1, 2, 3, 4 LIMIT ?1")?;
    let rows = statement.query_map([limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for (index, row) in rows.enumerate() {
        if index as i64 == limit - 1 {
            row?;
            report.note_issue_query_truncated();
            break;
        }
        let (table, row_id, parent, fk_id) = row?;
        push_issue(
            report,
            FsckCode::ForeignKey,
            Some(table.clone()),
            format!("table {table} row {row_id:?} violates FK {fk_id} to {parent}"),
        );
    }
    Ok(())
}

fn check_relational_invariants(
    connection: &Connection,
    report: &mut FsckReport,
) -> StoreResult<()> {
    collect_space_query(
        connection,
        report,
        FsckCode::RefInvariant,
        "SELECT r.space_id, r.ref_name
         FROM refs r
         LEFT JOIN steps s ON s.step_id = r.step_id
         WHERE r.generation <= 0 OR s.step_id IS NULL
            OR s.space_id <> r.space_id OR s.layer_id <> r.layer_id",
        "invalid ref",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::StepInvariant,
        "SELECT s.space_id, s.step_id
         FROM steps s
         LEFT JOIN steps p ON p.step_id = s.parent_step_id
         WHERE s.parent_step_id IS NOT NULL
           AND (p.step_id IS NULL OR p.space_id <> s.space_id OR p.layer_id <> s.layer_id)",
        "invalid parent for step",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::PendingInvariant,
        "SELECT p.space_id, p.publish_id
         FROM pending_publish p
         LEFT JOIN steps s ON s.step_id = p.step_id
         LEFT JOIN layers l ON l.layer_id = p.layer_id
         WHERE s.step_id IS NULL OR l.layer_id IS NULL
            OR s.space_id <> p.space_id OR s.layer_id <> p.layer_id
            OR l.space_id <> p.space_id",
        "invalid pending publish",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::JournalInvariant,
        "SELECT space_id, CAST(COUNT(*) AS TEXT)
         FROM operation_journal
         WHERE state IN (
            'prepared', 'external_pending', 'applying', 'metadata_committed', 'rolling_back'
         )
         GROUP BY space_id HAVING COUNT(*) > 1",
        "multiple non-terminal journal operations",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::TimelineInvariant,
        "SELECT space_id, CAST(COUNT(*) AS TEXT)
         FROM layers WHERE timeline_active = 1 AND state = 'active'
         GROUP BY space_id HAVING COUNT(*) > 1",
        "multiple active timeline layers",
    )?;
    if has_table(connection, "weave_sessions")? {
        collect_space_query(
            connection,
            report,
            FsckCode::WeaveInvariant,
            "SELECT space_id, CAST(COUNT(*) AS TEXT)
             FROM weave_sessions
             WHERE state IN ('prepared', 'in_progress', 'conflicted', 'resolved', 'applying')
             GROUP BY space_id HAVING COUNT(*) > 1",
            "multiple active weave sessions",
        )?;
    }
    collect_space_query(
        connection,
        report,
        FsckCode::GcInvariant,
        "SELECT g.space_id, g.root_id
         FROM gc_roots g
         LEFT JOIN object_index o ON o.object_id = g.object_id
         WHERE o.object_id IS NULL OR o.state <> 'available'",
        "GC root references unavailable object",
    )?;
    Ok(())
}

fn collect_space_query(
    connection: &Connection,
    report: &mut FsckReport,
    code: FsckCode,
    sql: &str,
    label: &str,
) -> StoreResult<()> {
    let limit = report.issue_query_limit();
    let bounded = format!("SELECT * FROM ({sql}) ORDER BY 1, 2 LIMIT ?1");
    let mut statement = connection.prepare(&bounded)?;
    let rows = statement.query_map([limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for (index, row) in rows.enumerate() {
        if index as i64 == limit - 1 {
            row?;
            report.note_issue_query_truncated();
            break;
        }
        let (space_id, detail) = row?;
        push_issue(report, code, Some(space_id), format!("{label}: {detail}"));
    }
    Ok(())
}

fn check_object_graph(connection: &Connection, report: &mut FsckReport) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "SELECT parent_object_id, child_object_id
         FROM object_edges ORDER BY parent_object_id, child_object_id LIMIT ?1",
    )?;
    let rows = statement.query_map([MAX_FSCK_TRAVERSAL_ROWS as i64 + 1], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut edges = Vec::new();
    for (index, row) in rows.enumerate() {
        if index == MAX_FSCK_TRAVERSAL_ROWS {
            report.note_truncated_checks(1);
            break;
        }
        edges.push(row?);
    }
    let cycles = graph::cycle_edges(edges);
    if cycles.truncated {
        report.note_truncated_checks(1);
    }
    for (parent, child) in cycles.items {
        push_issue(
            report,
            FsckCode::ObjectGraphCycle,
            Some(parent),
            format!("object graph cycle reaches {child}"),
        );
    }
    Ok(())
}

fn check_indexed_objects(
    layout: &StoreLayout,
    connection: &Connection,
    report: &mut FsckReport,
) -> StoreResult<()> {
    let cas_root =
        PinnedDirectory::open(&layout.cas_dir).map_err(|error| (error.kind(), error.to_string()));
    let mut statement = connection.prepare(
        "SELECT object_id, algorithm, byte_len, relative_path, state
         FROM object_index ORDER BY object_id LIMIT ?1",
    )?;
    let rows = statement.query_map(params![MAX_FSCK_TRAVERSAL_ROWS as i64 + 1], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    for (index, row) in rows.enumerate() {
        if index == MAX_FSCK_TRAVERSAL_ROWS {
            report.note_truncated_checks(1);
            break;
        }
        let (object_id, algorithm, expected_len, relative_path, state) = row?;
        report.checked_objects += 1;
        let Some(path) = safe_object_path(&layout.cas_dir, &relative_path) else {
            push_issue(
                report,
                FsckCode::ObjectPathUnsafe,
                Some(object_id),
                format!("unsafe object path {relative_path:?}"),
            );
            continue;
        };
        fault::hit(FaultPoint::BeforeFsckObjectOpen)?;
        let mut file = match &cas_root {
            Ok(root) => root.open_relative_file(Path::new(&relative_path)),
            Err((kind, detail)) => Err(io::Error::new(*kind, detail.clone())),
        };
        let file = match &mut file {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if state == "available" {
                    push_issue(
                        report,
                        FsckCode::ObjectMissing,
                        Some(object_id),
                        format!("indexed object is missing: {}", path.display()),
                    );
                }
                continue;
            }
            Err(error) => {
                push_issue(
                    report,
                    FsckCode::ObjectPathUnsafe,
                    Some(object_id),
                    format!("unsafe object path {}: {error}", path.display()),
                );
                continue;
            }
        };
        let Ok(expected_len) = u64::try_from(expected_len) else {
            push_issue(
                report,
                FsckCode::ObjectCorrupt,
                Some(object_id),
                format!("negative object length at {}", path.display()),
            );
            continue;
        };
        match stream_verify_object(file.file_mut(), &algorithm, &object_id, expected_len)? {
            StreamVerification::Verified => {}
            StreamVerification::LengthMismatch { actual } => push_issue(
                report,
                FsckCode::ObjectCorrupt,
                Some(object_id),
                format!(
                    "object length mismatch at {}: expected {expected_len}, got {actual}",
                    path.display()
                ),
            ),
            StreamVerification::DigestMismatch => push_issue(
                report,
                FsckCode::ObjectCorrupt,
                Some(object_id),
                format!("object digest mismatch at {}", path.display()),
            ),
        }
    }
    Ok(())
}

fn safe_object_path(cas_dir: &Path, relative_path: &str) -> Option<std::path::PathBuf> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        None
    } else {
        Some(cas_dir.join(relative))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamVerification {
    Verified,
    LengthMismatch { actual: u64 },
    DigestMismatch,
}

fn stream_verify_object(
    file: &mut impl Read,
    algorithm: &str,
    object_id: &str,
    expected_len: u64,
) -> io::Result<StreamVerification> {
    let expected_hex = object_id
        .strip_prefix(&format!("{algorithm}:"))
        .unwrap_or(object_id);
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    let mut byte_len = 0_u64;
    let mut blake3 = blake3::Hasher::new();
    let mut placeholder = 0xcbf2_9ce4_8422_2325_u64;

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        byte_len = byte_len
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("object length overflow during fsck"))?;
        match algorithm {
            "blake3" => {
                blake3.update(&buffer[..read]);
            }
            "placeholder" => {
                for byte in &buffer[..read] {
                    placeholder ^= u64::from(*byte);
                    placeholder = placeholder.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            _ => return Ok(StreamVerification::DigestMismatch),
        }
    }

    if byte_len != expected_len {
        return Ok(StreamVerification::LengthMismatch { actual: byte_len });
    }
    let actual = match algorithm {
        "blake3" => blake3.finalize().to_hex().to_string(),
        "placeholder" => format!("{placeholder:016x}{byte_len:016x}"),
        _ => return Ok(StreamVerification::DigestMismatch),
    };
    if actual == expected_hex {
        Ok(StreamVerification::Verified)
    } else {
        Ok(StreamVerification::DigestMismatch)
    }
}

fn has_table(connection: &Connection, name: &str) -> StoreResult<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

fn push_issue(report: &mut FsckReport, code: FsckCode, entity: Option<String>, detail: String) {
    report.push_issue(FsckIssue {
        code,
        entity,
        detail,
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::{LocalStore, LocalStoreConfig, ObjectRecord};

    #[test]
    fn rejects_parent_components_in_indexed_paths() {
        assert!(safe_object_path(Path::new("objects"), "../outside").is_none());
        assert!(safe_object_path(Path::new("objects"), "blake3/ab/cd/hash").is_some());
    }

    #[test]
    fn object_verification_streams_across_multiple_fixed_buffers() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "layrs-store-local-stream-{}-{timestamp}",
            std::process::id()
        ));
        let bytes = vec![0x5a; HASH_BUFFER_BYTES * 3 + 17];
        fs::write(&path, &bytes).expect("write streamed fixture");
        let digest = blake3::hash(&bytes).to_hex().to_string();
        let mut file = fs::File::open(&path).expect("open streamed fixture");

        assert_eq!(
            stream_verify_object(&mut file, "blake3", &digest, bytes.len() as u64)
                .expect("stream object"),
            StreamVerification::Verified
        );

        fs::remove_file(path).expect("remove streamed fixture");
    }

    #[test]
    fn swapped_object_ancestor_is_not_followed_during_fsck() {
        let base = unique_test_dir("swapped-object-ancestor");
        let root = base.join("store");
        let outside = base.join("outside");
        fs::create_dir_all(&outside).expect("create outside directory");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
        let bytes = b"valid bytes only outside after swap";
        let digest = layrs_sync::ObjectDigest::blake3_for(bytes);
        let target = store
            .write_object_v2(bytes, &digest)
            .expect("write indexed object")
            .path;
        let relative_path = target
            .strip_prefix(&store.layout().cas_dir)
            .expect("relative object path")
            .to_string_lossy()
            .replace('\\', "/");
        let mut session = store
            .session(SessionMode::Exclusive)
            .expect("exclusive session");
        session
            .repository_mut()
            .insert_object(&ObjectRecord {
                object_id: digest.hex().to_string(),
                algorithm: digest.algorithm().to_string(),
                byte_len: bytes.len() as u64,
                object_kind: "opaque".to_string(),
                relative_path,
                state: "available".to_string(),
                verified_at_ms: None,
                created_at_ms: 1,
            })
            .expect("index object");
        drop(session);

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let scan_store = store.clone();
        let scan_entered = Arc::clone(&entered);
        let scan_release = Arc::clone(&release);
        let scan = thread::spawn(move || {
            fault::pause_once(FaultPoint::BeforeFsckObjectOpen, scan_entered, scan_release);
            scan_store.fsck()
        });

        entered.wait();
        let algorithm = store.layout().cas_dir.join("blake3");
        let parked = store.layout().cas_dir.join("blake3-pinned");
        fs::rename(&algorithm, &parked).expect("park object algorithm directory");
        let outside_target = outside
            .join(&digest.hex()[0..2])
            .join(&digest.hex()[2..4])
            .join(digest.hex());
        fs::create_dir_all(outside_target.parent().expect("outside object parent"))
            .expect("create outside object shards");
        fs::write(&outside_target, bytes).expect("write valid object outside store");
        create_directory_link(&outside, &algorithm);
        release.wait();

        let report = scan
            .join()
            .expect("fsck thread")
            .expect("fsck swapped object ancestor");
        assert_eq!(
            report.issues_with_code(FsckCode::ObjectPathUnsafe).count(),
            1
        );

        remove_directory_link(&algorithm);
        fs::rename(&parked, &algorithm).expect("restore algorithm directory");
        drop(store);
        fs::remove_dir_all(base).expect("remove fixture");
    }

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
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
}
