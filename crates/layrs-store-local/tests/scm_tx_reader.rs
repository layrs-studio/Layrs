use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, StepId, StoreId, canonical_json};
use layrs_store::scm::{CanonicalObject, ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::{
    NativeCas, NativeScmReader, NativeTransactionalScmReader, init, open, open_exclusive,
};
use rusqlite::{Connection, OptionalExtension, params};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "layrs-scm-tx-reader-{name}-{}-{nanos:032x}-{sequence:016x}",
            std::process::id()
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_reader(_reader: &dyn ScmReader) {}

#[test]
fn transaction_reader_sees_uncommitted_empty_file_manifest_but_other_connection_does_not() {
    let root = TestRoot::new("isolation-rollback-empty-file");
    let store_id = StoreId::parse("store:tx-reader-isolation").unwrap();
    drop(init(root.path(), store_id).unwrap());

    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let manifest = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let object = CanonicalObject::FileManifest(manifest.clone());
    let object_id = StoreObjectId::FileManifest(manifest.id().clone());
    cas.put(&object).unwrap();

    let object_path = cas.object_path(&object_id).unwrap();
    let object_bytes = i64::try_from(fs::metadata(&object_path).unwrap().len()).unwrap();
    let storage_path = object_path
        .strip_prefix(root.path())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let connection = Mutex::new(Connection::open(exclusive.layout().sqlite_path()).unwrap());
    connection
        .lock()
        .unwrap()
        .execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")
        .unwrap();
    connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO objects(object_id, object_kind, storage_path, byte_length, verified, created_at_ms) \
             VALUES (?1, 'file_manifest', ?2, ?3, 1, 0)",
            params![object_id.as_str(), storage_path, object_bytes],
        )
        .unwrap();
    connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO file_availability(file_id, availability, verified_at_ms) \
             VALUES (?1, 'present', 0)",
            [manifest.file_id().as_str()],
        )
        .unwrap();
    let chunking_json = String::from_utf8(canonical_json(manifest.chunking()).unwrap()).unwrap();
    connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO file_manifests(manifest_id, file_id, file_byte_length, chunking_json) \
             VALUES (?1, ?2, 0, ?3)",
            params![
                manifest.id().as_str(),
                manifest.file_id().as_str(),
                chunking_json
            ],
        )
        .unwrap();

    let reader = NativeTransactionalScmReader::new(&connection, &cas).unwrap();
    assert_reader(&reader);
    let shortcut_step =
        StepId::parse("blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap();
    assert!(
        !reader
            .is_step_ancestor(&shortcut_step, &shortcut_step)
            .unwrap()
    );
    assert_eq!(
        reader.get_file_manifest(manifest.id()).unwrap(),
        Some(manifest.clone())
    );

    let outside = Connection::open(exclusive.layout().sqlite_path()).unwrap();
    let visible: Option<String> = outside
        .query_row(
            "SELECT object_id FROM objects WHERE object_id=?1",
            [object_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(visible, None);
    drop(outside);

    connection
        .lock()
        .unwrap()
        .execute_batch("ROLLBACK;")
        .unwrap();
    assert_eq!(
        reader.get_file_manifest(manifest.id()).unwrap_err().code(),
        "backend_error"
    );
    assert_eq!(
        reader
            .is_step_ancestor(&shortcut_step, &shortcut_step)
            .unwrap_err()
            .code(),
        "backend_error"
    );
    drop(reader);
    drop(connection);
    drop(cas);
    drop(exclusive);

    let shared = open(root.path()).unwrap();
    let shared_reader = NativeScmReader::new(&shared).unwrap();
    assert_eq!(
        shared_reader.get_file_manifest(manifest.id()).unwrap(),
        None
    );
}

#[test]
fn transaction_reader_requires_active_transaction_and_checks_sql_cas_projection() {
    let root = TestRoot::new("gate-projection");
    let store_id = StoreId::parse("store:tx-reader-projection").unwrap();
    drop(init(root.path(), store_id).unwrap());

    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let chunk = Chunk::new(b"transactional projection".to_vec()).unwrap();
    let object = CanonicalObject::ChunkRaw(chunk.clone());
    let object_id = StoreObjectId::ChunkRaw(chunk.id().clone());
    cas.put(&object).unwrap();
    let connection = Mutex::new(Connection::open(exclusive.layout().sqlite_path()).unwrap());

    let outside_transaction = match NativeTransactionalScmReader::new(&connection, &cas) {
        Ok(_) => panic!("reader unexpectedly accepted an autocommit connection"),
        Err(error) => error,
    };
    assert_eq!(outside_transaction.code(), "backend_error");

    connection
        .lock()
        .unwrap()
        .execute_batch("BEGIN IMMEDIATE;")
        .unwrap();
    connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO objects(object_id, object_kind, storage_path, byte_length, verified, created_at_ms) \
             VALUES (?1, 'chunk_raw', 'objects/not-the-id-path', ?2, 1, 0)",
            params![object_id.as_str(), i64::try_from(chunk.bytes().len()).unwrap()],
        )
        .unwrap();
    let reader = NativeTransactionalScmReader::new(&connection, &cas).unwrap();
    let shortcut_step =
        StepId::parse("blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .unwrap();
    assert!(
        !reader
            .is_step_ancestor(&shortcut_step, &shortcut_step)
            .unwrap()
    );
    assert_eq!(
        reader.get_chunk(chunk.id()).unwrap_err().code(),
        "corruption"
    );
    connection.lock().unwrap().execute_batch("COMMIT;").unwrap();
    assert_eq!(
        reader
            .is_step_ancestor(&shortcut_step, &shortcut_step)
            .unwrap_err()
            .code(),
        "backend_error"
    );
}
