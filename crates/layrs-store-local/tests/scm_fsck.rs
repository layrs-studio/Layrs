use layrs_core::scm::{
    ActorId, CaptureReason, Chunk, ChunkingDescriptor, DeviceId, FileManifest, Metadata,
    ObservationId, Operation, OperationId, OperationKind, RepoPathComponent, RepositoryView, Space,
    SpaceId, Step, StepObservation, StoreId, Timestamp, Tree, TreeEntry, TreeEntryTarget,
    WorktreeId, WorktreeStatus, WorktreeViewState, canonical_json,
};
use layrs_store::scm::{CanonicalObject, ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::fsck::{NativeFsck, fsck_existing};
use layrs_store_local::scm_v1::{
    FsckFindingCode, FsckOptions, NativeCas, NativeScmReader, init, open, open_audit,
    open_exclusive,
};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
            "layrs-native-fsck-{name}-{}-{nanos:032x}-{sequence:016x}",
            std::process::id()
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn database(&self) -> PathBuf {
        self.0.join("metadata").join("layrs.sqlite3")
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn initialize(root: &TestRoot) {
    drop(init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap());
}

fn mutate(root: &TestRoot, operation: impl FnOnce(&Connection)) {
    let connection = Connection::open(root.database()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    operation(&connection);
    connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
}

fn codes(root: &TestRoot, max_findings: usize) -> Vec<FsckFindingCode> {
    fsck_existing(root.path(), FsckOptions { max_findings })
        .unwrap()
        .findings
        .into_iter()
        .map(|finding| finding.code)
        .collect()
}

#[path = "scm_fsck/integrity.rs"]
mod integrity;
#[path = "scm_fsck/limits.rs"]
mod limits;
#[path = "scm_fsck/projections.rs"]
mod projections;
#[path = "scm_fsck/reachability.rs"]
mod reachability;

fn assert_timestamp_rebuild(root: &TestRoot, table: &str, entity_id: &str) {
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(report.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::ProjectionDivergence
            && finding.entity_id.as_deref() == Some(entity_id)
            && finding.message.contains("created_at_ms")
    }));
    assert!(report.projection_rebuild_plan.actions.iter().any(|action| {
        action.table == table
            && action.entity_id == entity_id
            && action.reason.contains("created_at_ms")
    }));
}

fn kind_name(object: &StoreObjectId) -> &'static str {
    match object {
        StoreObjectId::ChunkRaw(_) => "chunk_raw",
        StoreObjectId::FileManifest(_) => "file_manifest",
        StoreObjectId::Tree(_) => "tree",
        StoreObjectId::Symlink(_) => "symlink",
        StoreObjectId::Tombstone(_) => "tombstone",
        StoreObjectId::Step(_) => "step",
        StoreObjectId::RepositoryView(_) => "repository_view",
        StoreObjectId::Conflict(_) => "conflict",
        StoreObjectId::Product(_) => "product",
    }
}

fn oid(value: u64) -> String {
    format!("blake3:{value:064x}")
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, current: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries: Vec<_> = fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
