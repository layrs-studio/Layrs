use super::*;
use rusqlite::TransactionBehavior;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn test_store_id() -> StoreId {
    StoreId::parse("store:sqlite-test").unwrap()
}

#[test]
fn rfc3339_epoch_millis_is_exact_and_offset_independent() {
    assert_eq!(
        rfc3339_to_epoch_millis("1970-01-01T00:00:00.1239Z").unwrap(),
        123
    );
    assert_eq!(
        rfc3339_to_epoch_millis("1969-12-31T23:59:59.9999Z").unwrap(),
        -1
    );
    assert_eq!(
        rfc3339_to_epoch_millis("1970-01-01T01:00:00.1239+01:00").unwrap(),
        123
    );
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "layrs-scm-sqlite-{label}-{}-{nonce}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn database(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn directory_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut snapshot = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_file() {
            snapshot.push((name, fs::read(entry.path()).unwrap()));
        } else {
            snapshot.push((format!("{name}/"), Vec::new()));
        }
    }
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

#[test]
fn fresh_bootstrap_is_complete_and_publishes_no_partial_file() {
    let temporary = TempDirectory::new("bootstrap");
    let path = temporary.database("layrs.sqlite3");
    let connection = create_fresh(&path, Duration::from_millis(250), &test_store_id()).unwrap();

    assert!(path.is_file());
    assert!(!staged_database_path(&path).unwrap().exists());
    assert_eq!(schema_fingerprint(&connection).unwrap(), SCHEMA_FINGERPRINT);
    let actual: BTreeSet<_> = table_names(&connection).unwrap().into_iter().collect();
    let expected: BTreeSet<_> = REQUIRED_TABLES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(actual, expected);
    for table in REQUIRED_TABLES {
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.ends_with("STRICT"), "{table} is not STRICT: {sql}");
    }
    let violations: u32 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
}

#[test]
fn retry_recreates_only_the_authenticated_staging_name() {
    let temporary = TempDirectory::new("staged-retry");
    let path = temporary.database("layrs.sqlite3");
    let staged = staged_database_path(&path).unwrap();
    fs::write(&staged, b"interrupted sqlite bootstrap").unwrap();
    fs::write(sidecar_path(&staged, "-wal"), b"interrupted wal").unwrap();
    fs::write(sidecar_path(&staged, "-shm"), b"interrupted shm").unwrap();
    let unrelated = temporary.database("unrelated.initializing");
    fs::write(&unrelated, b"must survive").unwrap();

    drop(create_fresh(&path, Duration::from_millis(100), &test_store_id()).unwrap());
    assert!(path.is_file());
    assert!(!staged.exists());
    assert!(!sidecar_path(&staged, "-wal").exists());
    assert!(!sidecar_path(&staged, "-shm").exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"must survive");
}

#[test]
fn reopen_validates_identity_and_read_only_open_does_not_mutate() {
    let temporary = TempDirectory::new("reopen");
    let path = temporary.database("layrs.sqlite3");
    drop(create_fresh(&path, Duration::from_millis(100), &test_store_id()).unwrap());
    drop(open_existing(&path, Duration::from_millis(100)).unwrap());

    let before = directory_snapshot(&temporary.0);
    let reader = open_read_only(&path, Duration::from_millis(100)).unwrap();
    assert!(
        reader
            .execute(
                "INSERT INTO applied_transactions VALUES ('x', 'x', '{}', 0)",
                [],
            )
            .is_err()
    );
    drop(reader);
    assert_eq!(directory_snapshot(&temporary.0), before);

    let missing = temporary.database("missing.sqlite3");
    assert!(matches!(
        open_read_only(&missing, Duration::from_millis(10)),
        Err(ScmSchemaError::NotFound(found)) if found == missing
    ));
    assert!(!missing.exists());
    assert!(matches!(
        create_fresh(&path, Duration::from_millis(10), &test_store_id()),
        Err(ScmSchemaError::AlreadyExists(existing)) if existing == path
    ));
}

#[test]
fn audit_open_is_immutable_and_does_not_require_a_valid_schema() {
    let temporary = TempDirectory::new("audit-open");
    let path = temporary.database("foreign.sqlite3");
    let foreign = Connection::open(&path).unwrap();
    foreign
        .execute("CREATE TABLE legacy(value TEXT)", [])
        .unwrap();
    drop(foreign);

    let before = directory_snapshot(&temporary.0);
    let audit = open_audit_read_only(&path, Duration::from_millis(50)).unwrap();
    let count: u32 = audit
        .query_row("SELECT COUNT(*) FROM legacy", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(audit);
    assert_eq!(directory_snapshot(&temporary.0), before);

    let wal = sidecar_path(&path, "-wal");
    fs::write(&wal, b"uncheckpointed").unwrap();
    assert!(matches!(
        open_audit_read_only(&path, Duration::from_millis(50)),
        Err(ScmSchemaError::UncheckpointedWal(found)) if found == wal
    ));
}

#[test]
fn metadata_foreign_and_real_schema_tampering_are_refused() {
    let temporary = TempDirectory::new("tampering");

    let old_path = temporary.database("old.sqlite3");
    let old = create_fresh(&old_path, Duration::from_millis(50), &test_store_id()).unwrap();
    old.pragma_update(None, "user_version", 0).unwrap();
    drop(old);
    assert!(matches!(
        open_existing(&old_path, Duration::from_millis(50)),
        Err(ScmSchemaError::UnsupportedSchema {
            found: 0,
            supported: SCHEMA_VERSION
        })
    ));

    let future_path = temporary.database("future.sqlite3");
    let future = create_fresh(&future_path, Duration::from_millis(50), &test_store_id()).unwrap();
    future
        .execute("UPDATE meta SET schema_version = ?1", [SCHEMA_VERSION + 1])
        .unwrap();
    drop(future);
    assert!(matches!(
        open_existing(&future_path, Duration::from_millis(50)),
        Err(ScmSchemaError::UnsupportedSchema {
            found,
            supported: SCHEMA_VERSION
        }) if found == SCHEMA_VERSION + 1
    ));

    let metadata_path = temporary.database("metadata.sqlite3");
    let metadata =
        create_fresh(&metadata_path, Duration::from_millis(50), &test_store_id()).unwrap();
    metadata
        .execute("UPDATE meta SET schema_checksum = ?1", ["0".repeat(64)])
        .unwrap();
    drop(metadata);
    assert!(matches!(
        open_existing(&metadata_path, Duration::from_millis(50)),
        Err(ScmSchemaError::ChecksumMismatch { .. })
    ));

    let foreign_path = temporary.database("foreign.sqlite3");
    let foreign = Connection::open(&foreign_path).unwrap();
    foreign
        .execute("CREATE TABLE legacy(value TEXT)", [])
        .unwrap();
    drop(foreign);
    assert!(matches!(
        open_existing(&foreign_path, Duration::from_millis(50)),
        Err(ScmSchemaError::ForeignDatabase { .. })
    ));

    let index_path = temporary.database("missing-index.sqlite3");
    let index = create_fresh(&index_path, Duration::from_millis(50), &test_store_id()).unwrap();
    index.execute("DROP INDEX objects_kind_idx", []).unwrap();
    drop(index);
    assert!(matches!(
        open_existing(&index_path, Duration::from_millis(50)),
        Err(ScmSchemaError::SchemaFingerprintMismatch { .. })
    ));

    let table_path = temporary.database("missing-table.sqlite3");
    let table = create_fresh(&table_path, Duration::from_millis(50), &test_store_id()).unwrap();
    table.execute("DROP TABLE gc_roots", []).unwrap();
    drop(table);
    assert!(matches!(
        open_existing(&table_path, Duration::from_millis(50)),
        Err(ScmSchemaError::SchemaStructureMismatch { .. })
    ));

    let constraint_path = temporary.database("missing-constraint.sqlite3");
    let constraint = create_fresh(
        &constraint_path,
        Duration::from_millis(50),
        &test_store_id(),
    )
    .unwrap();
    constraint
        .execute_batch(
            "ALTER TABLE applied_transactions RENAME TO applied_transactions_old;
                 CREATE TABLE applied_transactions (
                     transaction_id TEXT PRIMARY KEY,
                     request_digest TEXT NOT NULL,
                     receipt_json TEXT NOT NULL,
                     applied_at_ms INTEGER NOT NULL
                 ) STRICT;
                 DROP TABLE applied_transactions_old;",
        )
        .unwrap();
    drop(constraint);
    assert!(matches!(
        open_existing(&constraint_path, Duration::from_millis(50)),
        Err(ScmSchemaError::SchemaFingerprintMismatch { .. })
    ));
}

#[test]
fn begin_immediate_is_atomic_and_observes_writer_timeout() {
    let temporary = TempDirectory::new("immediate");
    let path = temporary.database("layrs.sqlite3");
    let mut first = create_fresh(&path, Duration::from_millis(25), &test_store_id()).unwrap();
    let mut second = open_existing(&path, Duration::from_millis(25)).unwrap();

    let first_transaction = first
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let error = with_immediate_transaction(&mut second, |_| Ok(())).unwrap_err();
    assert!(matches!(
        error,
        ScmSchemaError::Sqlite(rusqlite::Error::SqliteFailure(ref code, _))
            if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    ));
    drop(first_transaction);

    with_immediate_transaction(&mut second, |transaction| {
        transaction.execute(
            "INSERT INTO applied_transactions( \
                     transaction_id, request_digest, receipt_json, applied_at_ms \
                 ) VALUES ('transaction:test', ?1, ?2, 0)",
            params![
                format!("blake3:{}", "0".repeat(64)),
                canonical_receipt_json(&serde_json::json!({
                    "requestDigest": format!("blake3:{}", "0".repeat(64))
                }))?
            ],
        )?;
        Ok(())
    })
    .unwrap();
    let count: u32 = second
        .query_row("SELECT COUNT(*) FROM applied_transactions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn applied_transaction_receipt_boundary_requires_canonical_jcs() {
    let canonical = canonical_receipt_json(&serde_json::json!({
        "requestDigest": format!("blake3:{}", "0".repeat(64)),
        "result": {"generation": 1, "viewId": "view:test"}
    }))
    .unwrap();
    assert_eq!(
        validate_canonical_receipt_json(&canonical).unwrap(),
        serde_json::from_str::<serde_json::Value>(&canonical).unwrap()
    );

    let noncanonical = format!(
        "{{ \"result\": {{\"viewId\":\"view:test\", \"generation\":1}}, \
             \"requestDigest\":\"blake3:{}\" }}",
        "0".repeat(64)
    );
    assert!(matches!(
        validate_canonical_receipt_json(&noncanonical),
        Err(ScmSchemaError::InvalidMetadata(_))
    ));
    assert!(matches!(
        validate_canonical_receipt_json("{not-json}"),
        Err(ScmSchemaError::InvalidMetadata(_))
    ));
}
