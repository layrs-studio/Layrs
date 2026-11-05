use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_core::scm::{
    Chunk, ChunkingDescriptor, FileManifest, Layer, LayerHeadSet, LayerId, LayerKind,
    LayerSettings, LayerStatus, Metadata, Operation, OperationId, OperationKind, Release,
    ReleaseId, ReleasePublication, ReleasePublicationId, RepoPathComponent, RepositoryView, Space,
    SpaceId, Step, StoreId, Timestamp, Tree, TreeEntry, TreeEntryTarget, TreeId, Worktree,
    WorktreeId, WorktreeStatus, WorktreeViewState,
};
use layrs_store::scm::{
    ApplyBatchOutcome, CanonicalObject, EntityRecord, OperationHeadsCas, RepositoryViewCas,
    ScmReader, ScmRepository, SpaceObjectAdmission, StoreMutation, StoreObjectId, TransactionId,
    WorktreeActiveOperationCas, WriteBatch,
};
use layrs_store_local::scm_v1::{
    CasFaultInjector, CasFaultPoint, FsckOptions, NativeCas, NativeScmRepository,
    RepositoryFaultInjector, RepositoryFaultPoint, fsck_existing, init, open, open_exclusive,
};
use rusqlite::Connection;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[path = "scm_write_batch/atomic.rs"]
mod atomic;
#[path = "scm_write_batch/history.rs"]
mod history;
#[path = "scm_write_batch/integrity.rs"]
mod integrity;
#[path = "scm_write_batch/publication.rs"]
mod publication;
#[path = "scm_write_batch/streaming.rs"]
mod streaming;

fn initialize(root: &Path, label: &str) -> WriteBatch {
    let store_id = StoreId::parse(format!("store:{label}")).unwrap();
    drop(init(root, store_id).unwrap());
    let space = Space::new(
        SpaceId::parse(format!("space:{label}")).unwrap(),
        "Space",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let view = RepositoryView::new(space.id().clone(), vec![], vec![], vec![], vec![]).unwrap();
    let operation = Operation::new(
        OperationId::parse(format!("operation:{label}")).unwrap(),
        space.id().clone(),
        vec![],
        view.id().clone(),
        OperationKind::Initialize,
        "Initialize",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    WriteBatch::new(
        TransactionId::new(format!("transaction:{label}")).unwrap(),
        vec![CanonicalObject::RepositoryView(view.clone())],
        vec![
            EntityRecord::Space(space.clone()),
            EntityRecord::Operation(operation.clone()),
        ],
        vec![SpaceObjectAdmission::new(
            space.id().clone(),
            StoreObjectId::RepositoryView(view.id().clone()),
        )],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![],
                    operation.id().clone(),
                    vec![operation.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap()
}

struct OneFault {
    point: RepositoryFaultPoint,
    fired: AtomicBool,
}
impl OneFault {
    fn new(point: RepositoryFaultPoint) -> Self {
        Self {
            point,
            fired: AtomicBool::new(false),
        }
    }
}
impl RepositoryFaultInjector for OneFault {
    fn check(&self, point: RepositoryFaultPoint) -> Result<(), String> {
        if point == self.point && !self.fired.swap(true, Ordering::SeqCst) {
            Err("test".into())
        } else {
            Ok(())
        }
    }
}
struct NthFault {
    point: RepositoryFaultPoint,
    target: u64,
    count: AtomicU64,
}
struct OneCasFault {
    point: CasFaultPoint,
}
impl CasFaultInjector for OneCasFault {
    fn check(&self, point: CasFaultPoint) -> Result<(), String> {
        if point == self.point {
            Err("cas test".into())
        } else {
            Ok(())
        }
    }
}
impl RepositoryFaultInjector for NthFault {
    fn check(&self, point: RepositoryFaultPoint) -> Result<(), String> {
        if point == self.point && self.count.fetch_add(1, Ordering::SeqCst) + 1 == self.target {
            Err("nth test".into())
        } else {
            Ok(())
        }
    }
}
fn timestamp() -> Timestamp {
    Timestamp::parse("2026-07-20T00:00:00Z").unwrap()
}
fn unique_root(label: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let s = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "layrs-write-{label}-{}-{n}-{s}",
        std::process::id()
    ))
}
fn cleanup(root: &Path) {
    fs::remove_dir_all(root).unwrap();
}
fn assert_clean_db(root: &Path) {
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap(),
        "ok"
    );
    let fk: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk, 0);
    drop(connection);
    let report = fsck_existing(root, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:#?}", report.findings);
}
