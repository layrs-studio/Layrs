use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_core::scm::{
    CaptureReason, Chunk, ChunkingDescriptor, FileChunk, FileManifest, Layer, LayerHeadSet,
    LayerId, LayerKind, LayerSettings, LayerStatus, Marker, MarkerId, MarkerKind, Metadata,
    ObservationId, Operation, OperationId, OperationKind, Release, ReleaseId, ReleasePublication,
    ReleasePublicationId, RepoPathComponent, RepositoryView, RewriteTarget, Space, SpaceId, Step,
    StepId, StepObservation, StoreId, Symlink, Timestamp, Tombstone, Tree, TreeEntry,
    TreeEntryTarget, Worktree, WorktreeId, canonical_json,
};
use layrs_store::scm::{
    CanonicalConflict, CanonicalObject, EntityRecord, FileAvailability, OperationHeadsCas,
    RepositoryViewCas, ScmReader, SpaceObjectAdmission, StoreMutation, StoreObjectId,
    TransactionId, WorktreeActiveOperationCas, WriteBatch,
};
use layrs_store_local::scm_v1::{
    CompositeAdmissionResolver, NativeCas, NativeScmReader, NativeTransactionalScmReader, init,
    open, open_exclusive,
};
use rusqlite::{Connection, params};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    store_id: StoreId,
    space: Space,
    layer: Layer,
    observation: StepObservation,
    operation: Operation,
    worktree: Worktree,
    marker: Marker,
    release: Release,
    publication: ReleasePublication,
    objects: Vec<CanonicalObject>,
}

#[path = "scm_admission/budgets.rs"]
mod budgets;
#[path = "scm_admission/closure.rs"]
mod closure;
#[path = "scm_admission/history.rs"]
mod history;
#[path = "scm_admission/reads.rs"]
mod reads;

fn chunk_file_id(fixture: &Fixture) -> &layrs_core::scm::FileId {
    fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::FileManifest(value) => Some(value.file_id()),
            _ => None,
        })
        .unwrap()
}

fn fixture() -> Fixture {
    let timestamp = Timestamp::parse("2026-07-19T12:00:00Z").unwrap();
    let metadata = Metadata::empty();
    let store_id = StoreId::parse("store:reader").unwrap();
    let space = Space::new(
        SpaceId::parse("space:reader").unwrap(),
        "Reader",
        timestamp.clone(),
        metadata.clone(),
    )
    .unwrap();
    let layer = Layer::new(
        LayerId::parse("layer:reader").unwrap(),
        space.id().clone(),
        "Reader",
        LayerKind::Base,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 100).unwrap(),
        timestamp.clone(),
        metadata.clone(),
    )
    .unwrap();
    let chunk = Chunk::new(b"reader bytes".to_vec()).unwrap();
    let manifest =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let tree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("file.txt").unwrap(),
        TreeEntryTarget::file(manifest.file_id().clone()),
        false,
    )])
    .unwrap();
    let step = Step::new(vec![], tree.id().clone()).unwrap();
    let view = RepositoryView::new(space.id().clone(), vec![], vec![], vec![], vec![]).unwrap();
    let operation = Operation::new(
        OperationId::parse("operation:reader").unwrap(),
        space.id().clone(),
        vec![],
        view.id().clone(),
        OperationKind::Initialize,
        "initialize",
        vec![],
        None,
        None,
        timestamp.clone(),
        metadata.clone(),
    )
    .unwrap();
    let worktree = Worktree::new(
        WorktreeId::parse("worktree:reader").unwrap(),
        store_id.clone(),
        space.id().clone(),
        operation.id().clone(),
        "C:/reader",
        "volume:reader",
        timestamp.clone(),
        metadata.clone(),
    )
    .unwrap();
    let observation = StepObservation::new(
        ObservationId::parse("observation:reader").unwrap(),
        step.id().clone(),
        Some(space.id().clone()),
        Some(operation.id().clone()),
        Some(worktree.id().clone()),
        None,
        None,
        timestamp.clone(),
        CaptureReason::Received,
        metadata.clone(),
    );
    let marker = Marker::new(
        MarkerId::parse("marker:reader").unwrap(),
        space.id().clone(),
        step.id().clone(),
        MarkerKind::Checkpoint,
        "checkpoint",
        "",
        "",
        None,
        timestamp.clone(),
        vec![],
        metadata.clone(),
    )
    .unwrap();
    let release = Release::new(
        ReleaseId::parse("release:reader").unwrap(),
        space.id().clone(),
        step.id().clone(),
        "1.0.0",
        "",
        "",
        None,
        timestamp.clone(),
        vec![],
        metadata.clone(),
    )
    .unwrap();
    let publication = ReleasePublication::new(
        ReleasePublicationId::parse("release-publication:reader").unwrap(),
        release.id().clone(),
        space.id().clone(),
        Some(operation.id().clone()),
        None,
        None,
        timestamp,
        metadata,
    );
    let conflict = conflict();
    Fixture {
        store_id,
        space,
        layer,
        observation,
        operation,
        worktree,
        marker,
        release,
        publication,
        objects: vec![
            CanonicalObject::ChunkRaw(chunk),
            CanonicalObject::FileManifest(manifest),
            CanonicalObject::Tree(tree),
            CanonicalObject::Symlink(Symlink::new("target").unwrap()),
            CanonicalObject::Tombstone(Tombstone::new(None).unwrap()),
            CanonicalObject::Step(step),
            CanonicalObject::RepositoryView(view),
            CanonicalObject::Conflict(conflict),
        ],
    }
}

fn conflict() -> CanonicalConflict {
    let mut payload = serde_json::json!({"schemaVersion":"layrs.conflict.v1","sides":[]});
    let payload_bytes = canonical_json(&payload).unwrap();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-conflict-v1");
    hasher.update(&[0]);
    hasher.update(&payload_bytes);
    let id = layrs_core::scm::ConflictId::parse(format!("blake3:{}", hasher.finalize().to_hex()))
        .unwrap();
    payload
        .as_object_mut()
        .unwrap()
        .insert("conflictId".into(), id.to_string().into());
    CanonicalConflict::new(id, canonical_json(&payload).unwrap()).unwrap()
}

fn seed(root: &Path, fixture: &Fixture) {
    drop(init(root, fixture.store_id.clone()).unwrap());
    let store = open_exclusive(root).unwrap();
    let cas = NativeCas::open_exclusive(&store).unwrap();
    let mut rows = Vec::new();
    for object in &fixture.objects {
        let outcome = cas.put(object).unwrap();
        let relative = outcome
            .path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        rows.push((outcome.object_id, relative, outcome.bytes_len));
    }
    drop(cas);
    drop(store);

    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    for (id, path, length) in rows {
        connection.execute(
            "INSERT INTO objects(object_id, object_kind, storage_path, byte_length, verified, created_at_ms) VALUES (?1, ?2, ?3, ?4, 1, 0)",
            params![id.as_str(), object_kind(&id), path, i64::try_from(length).unwrap()],
        ).unwrap();
    }
    let space_json = json(&fixture.space);
    connection
        .execute(
            "UPDATE stores SET payload_json = ?2, created_at_ms = 0 WHERE store_id = ?1",
            params![
                fixture.store_id.as_str(),
                format!("{{\"storeId\":\"{}\"}}", fixture.store_id)
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO spaces VALUES (?1, ?2, CAST(unixepoch(?3, 'subsec') * 1000 AS INTEGER))",
            params![
                fixture.space.id().as_str(),
                space_json,
                fixture.space.created_at().as_str()
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO layers VALUES (?1, ?2, 'base', 'active', ?3, CAST(unixepoch(?4, 'subsec') * 1000 AS INTEGER))",
            params![
                fixture.layer.id().as_str(),
                fixture.space.id().as_str(),
                json(&fixture.layer),
                fixture.layer.created_at().as_str()
            ],
        )
        .unwrap();

    let manifest = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::FileManifest(value) => Some(value),
            _ => None,
        })
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_availability VALUES (?1, 'present', 0)",
            [manifest.file_id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_manifests VALUES (?1, ?2, ?3, ?4)",
            params![
                manifest.id().as_str(),
                manifest.file_id().as_str(),
                i64::try_from(manifest.byte_length()).unwrap(),
                json(manifest.chunking())
            ],
        )
        .unwrap();
    for (ordinal, entry) in manifest.chunks().iter().enumerate() {
        connection
            .execute(
                "INSERT INTO file_manifest_chunks VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    manifest.id().as_str(),
                    i64::try_from(ordinal).unwrap(),
                    entry.chunk_id().as_str(),
                    i64::try_from(entry.offset()).unwrap(),
                    i64::try_from(entry.byte_length()).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO object_edges VALUES (?1, ?2, 'manifest_chunk', ?3)",
                params![
                    manifest.id().as_str(),
                    entry.chunk_id().as_str(),
                    i64::try_from(ordinal).unwrap()
                ],
            )
            .unwrap();
    }
    let step = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Step(value) => Some(value),
            _ => None,
        })
        .unwrap();
    connection
        .execute(
            "INSERT INTO steps VALUES (?1, ?2, 0)",
            params![step.id().as_str(), step.root_tree_id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO object_edges VALUES (?1, ?2, 'step_tree', 0)",
            params![step.id().as_str(), step.root_tree_id().as_str()],
        )
        .unwrap();
    let view = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::RepositoryView(value) => Some(value),
            _ => None,
        })
        .unwrap();
    connection
        .execute(
            "INSERT INTO repository_views VALUES (?1, ?2)",
            params![view.id().as_str(), fixture.space.id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO operations VALUES (?1, ?2, ?3, 'initialize', NULL, NULL, ?4, ?5)",
            params![
                fixture.operation.id().as_str(),
                fixture.space.id().as_str(),
                view.id().as_str(),
                fixture.operation.created_at().as_str(),
                json(&fixture.operation)
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO worktrees VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CAST(unixepoch(?8, 'subsec') * 1000 AS INTEGER))",
            params![
                fixture.worktree.id().as_str(),
                fixture.store_id.as_str(),
                fixture.space.id().as_str(),
                fixture.operation.id().as_str(),
                fixture.worktree.checkout_root(),
                fixture.worktree.filesystem_identity(),
                json(&fixture.worktree),
                fixture.worktree.created_at().as_str()
            ],
        )
        .unwrap();
    connection.execute("INSERT INTO step_observations VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, 'received', ?7)", params![fixture.observation.id().as_str(), step.id().as_str(), fixture.space.id().as_str(), fixture.operation.id().as_str(), fixture.worktree.id().as_str(), fixture.observation.observed_at().as_str(), json(&fixture.observation)]).unwrap();
    connection
        .execute(
            "INSERT INTO markers VALUES (?1, ?2, ?3, 'checkpoint', ?4, ?5)",
            params![
                fixture.marker.id().as_str(),
                fixture.space.id().as_str(),
                step.id().as_str(),
                fixture.marker.created_at().as_str(),
                json(&fixture.marker)
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO releases VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                fixture.release.id().as_str(),
                fixture.space.id().as_str(),
                step.id().as_str(),
                fixture.release.version(),
                fixture.release.created_at().as_str(),
                json(&fixture.release)
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO release_publications VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                fixture.publication.id().as_str(),
                fixture.release.id().as_str(),
                fixture.space.id().as_str(),
                fixture.operation.id().as_str(),
                fixture.publication.published_at().as_str(),
                json(&fixture.publication)
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO current_repository_views VALUES (?1, 1, ?2, 'seed')",
            params![fixture.space.id().as_str(), view.id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO operation_heads VALUES (?1, ?2, 0, 'seed')",
            params![fixture.space.id().as_str(), fixture.operation.id().as_str()],
        )
        .unwrap();
    for object in &fixture.objects {
        connection
            .execute(
                "INSERT INTO space_object_admissions VALUES (?1, ?2, NULL, 0)",
                params![fixture.space.id().as_str(), object.object_id().as_str()],
            )
            .unwrap();
    }
    checkpoint(&connection);
}

fn json(value: &impl serde::Serialize) -> String {
    String::from_utf8(canonical_json(value).unwrap()).unwrap()
}

fn object_kind(id: &StoreObjectId) -> &'static str {
    match id {
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

fn checkpoint(connection: &Connection) {
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
}

fn transaction(label: &str) -> TransactionId {
    TransactionId::new(format!("transaction:{label}")).unwrap()
}

fn unique_root(label: &str) -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "layrs-scm-admission-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, output: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(path).unwrap().map(Result::unwrap).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                output.push((relative, b"<directory>".to_vec()));
                visit(root, &path, output);
            } else {
                output.push((relative, fs::read(path).unwrap()));
            }
        }
    }
    let mut result = Vec::new();
    visit(root, root, &mut result);
    result
}

fn assert_snapshot_eq(actual: &[(PathBuf, Vec<u8>)], expected: &[(PathBuf, Vec<u8>)]) {
    assert_eq!(actual.len(), expected.len());
    for ((actual_path, actual_bytes), (expected_path, expected_bytes)) in
        actual.iter().zip(expected)
    {
        assert_eq!(actual_path, expected_path);
        assert_eq!(
            actual_bytes,
            expected_bytes,
            "bytes changed at {}",
            actual_path.display()
        );
    }
}

fn cleanup(root: &Path) {
    fs::remove_dir_all(root).unwrap();
}
