mod worktree_support;
use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::{
    CanonicalObject, EntityRecord, OperationHeadsCas, RepositoryViewCas, ScmReader, ScmRepository,
    SpaceObjectAdmission, StoreMutation, StoreObjectId, WorktreeActiveOperationCas, WriteBatch,
};
use layrs_store_local::scm_v1::{
    self, FsckFindingCode, FsckOptions, NativeCas, NativeScmReader, NativeScmRepository,
};
use std::{collections::BTreeSet, fs};
use worktree_support::*;

// Exercise admitted historical states directly through the Store publication
// contract. No journal or physical checkout is fabricated by this helper.
fn publish_state(
    workspace: &NativeWorkspace,
    state: WorktreeViewState,
    mut objects: Vec<CanonicalObject>,
) -> RepositoryViewId {
    let before = workspace.state().unwrap();
    let store = workspace.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let current = reader
        .current_repository_view(before.space.id())
        .unwrap()
        .unwrap();
    let heads = reader.operation_heads(before.space.id()).unwrap();
    let states = before
        .view
        .worktrees()
        .iter()
        .map(|old| {
            if old.worktree_id() == state.worktree_id() {
                state.clone()
            } else {
                old.clone()
            }
        })
        .collect();
    let view = RepositoryView::new(
        before.space.id().clone(),
        before.view.layer_heads().to_vec(),
        before.view.marker_ids().to_vec(),
        before.view.release_ids().to_vec(),
        states,
    )
    .unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let operation = Operation::new(
        stamp.operation_id.clone(),
        before.space.id().clone(),
        vec![before.operation.id().clone()],
        view.id().clone(),
        OperationKind::Restore,
        "Retained Worktree state fixture",
        vec![],
        None,
        None,
        stamp.timestamp,
        Metadata::empty(),
    )
    .unwrap();
    objects.push(CanonicalObject::RepositoryView(view.clone()));
    let admissions = objects
        .iter()
        .map(|object| SpaceObjectAdmission::new(before.space.id().clone(), object.object_id()))
        .collect();
    let next_heads = heads
        .iter()
        .filter(|id| *id != before.operation.id())
        .cloned()
        .chain([operation.id().clone()])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let batch = WriteBatch::new(
        stamp.transaction_id,
        objects,
        vec![EntityRecord::Operation(operation.clone())],
        admissions,
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    before.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    before.space.id().clone(),
                    heads,
                    operation.id().clone(),
                    next_heads,
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    workspace.worktree_id().clone(),
                    before.operation.id().clone(),
                    operation.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    drop(reader);
    drop(store);
    NativeScmRepository::new(workspace.store_root())
        .with_store_identity(workspace.store_identity())
        .with_metadata_identity(workspace.metadata_identity())
        .with_store_id(before.worktree.store_id().clone())
        .apply_batch(&batch)
        .unwrap();
    view.id().clone()
}

struct HistoryFixture {
    _fixture: Fixture,
    workspace: NativeWorkspace,
    historical_view: RepositoryViewId,
    base: StepId,
    materialized: TreeId,
    protected: Vec<String>,
    orphan: Chunk,
}

fn historical_states() -> HistoryFixture {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let original = own(&secondary);
    let empty = Tree::new(vec![]).unwrap();
    let whole = Chunk::new(b"historical-only bytes".to_vec()).unwrap();
    let left = Chunk::new(b"historical-".to_vec()).unwrap();
    let right = Chunk::new(b"only bytes".to_vec()).unwrap();
    let one = FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&whole))
        .unwrap();
    let two = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(11).unwrap(),
        &[left.clone(), right.clone()],
    )
    .unwrap();
    assert_eq!(one.file_id(), two.file_id());
    let child = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("payload").unwrap(),
        TreeEntryTarget::file(one.file_id().clone()),
        false,
    )])
    .unwrap();
    let materialized = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("nested").unwrap(),
        TreeEntryTarget::tree(child.id().clone()),
        false,
    )])
    .unwrap();
    // This base is absent from every Layer head. Its Step and root Tree must
    // survive solely through the historical Worktree base dependency.
    let base_tree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("base-only").unwrap(),
        TreeEntryTarget::file(one.file_id().clone()),
        false,
    )])
    .unwrap();
    let base = Step::new(
        vec![original.base_step_id().unwrap().clone()],
        base_tree.id().clone(),
    )
    .unwrap();
    let orphan = Chunk::new(b"unrelated admitted orphan".to_vec()).unwrap();
    let objects = vec![
        CanonicalObject::ChunkRaw(whole),
        CanonicalObject::ChunkRaw(left),
        CanonicalObject::ChunkRaw(right),
        CanonicalObject::FileManifest(one),
        CanonicalObject::FileManifest(two),
        CanonicalObject::Tree(child),
        CanonicalObject::Tree(materialized.clone()),
        CanonicalObject::Tree(base_tree),
        CanonicalObject::Step(base.clone()),
    ];
    let mut protected: Vec<_> = objects
        .iter()
        .map(|object| object.object_id().to_string())
        .collect();
    protected.push(empty.id().to_string());
    let historical_view = publish_state(
        &secondary,
        WorktreeViewState::new(
            secondary.worktree_id().clone(),
            original.active_layer_id().cloned(),
            Some(base.id().clone()),
            Some(materialized.id().clone()),
            WorktreeStatus::Stale,
        ),
        objects
            .into_iter()
            .chain([CanonicalObject::ChunkRaw(orphan.clone())])
            .collect(),
    );
    publish_state(&secondary, original, vec![]);
    HistoryFixture {
        _fixture: fixture,
        workspace: secondary,
        historical_view,
        base: base.id().clone(),
        materialized: materialized.id().clone(),
        protected,
        orphan,
    }
}

#[test]
fn historical_worktree_states_retain_empty_and_nonempty_closures_without_retaining_orphans() {
    let fixture = historical_states();
    let workspace = &fixture.workspace;
    let store = workspace.open_store().unwrap();
    let database = workspace.store_root().join("metadata/layrs.sqlite3");
    assert!(!database.with_extension("sqlite3-wal").exists());
    let mut uri = url::Url::from_file_path(database).unwrap();
    uri.set_query(Some("mode=ro&immutable=1"));
    let connection = rusqlite::Connection::open_with_flags(
        uri.as_str(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let empty = Tree::new(vec![]).unwrap();
    let indexed_empty: u64 = connection
        .query_row(
            "SELECT count(*) FROM objects WHERE object_id=?1 AND object_kind='tree' AND verified=1",
            [empty.id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed_empty, 1, "empty historical Tree must be indexed");
    let historical_empty: u64 = connection
        .query_row(
            "SELECT count(*) FROM repository_view_worktrees state
             WHERE state.materialized_tree_id=?1 AND state.worktree_id=?2
               AND EXISTS (SELECT 1 FROM gc_roots root
                           WHERE root.root_kind='retained_operation'
                             AND root.object_id=state.repository_view_id)
               AND NOT EXISTS (SELECT 1 FROM current_repository_views cv
                               WHERE cv.repository_view_id=state.repository_view_id)
               AND NOT EXISTS (SELECT 1 FROM worktrees w JOIN operations active
                               ON active.operation_id=w.active_operation_id
                               WHERE active.resulting_view_id=state.repository_view_id)",
            rusqlite::params![empty.id().as_str(), workspace.worktree_id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        historical_empty, 1,
        "empty Tree must occur in a retained, noncurrent Worktree state"
    );
    let historical_root: u64 = connection
        .query_row(
            "SELECT count(*) FROM gc_roots WHERE root_kind='retained_operation' AND object_id=?1",
            [fixture.historical_view.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(historical_root, 1);
    for id in &fixture.protected {
        let roots: u64 = connection
            .query_row(
                "SELECT count(*) FROM gc_roots WHERE object_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            roots, 0,
            "historical dependency unexpectedly has a direct root: {id}"
        );
    }
    let edges: u64 = connection
        .query_row(
            "SELECT count(*) FROM object_edges WHERE source_object_id=?1",
            [fixture.historical_view.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        edges, 1,
        "fixture must use existing Stores' Layer-only View edges"
    );
    drop(connection);
    drop(store);
    let report = scm_v1::fsck_existing(workspace.store_root(), FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    assert!(!report.is_truncated());
    assert_eq!(
        report.gc_reachability.unreachable_objects,
        vec![fixture.orphan.id().to_string()]
    );
    for id in fixture.protected {
        assert!(
            !report.gc_reachability.unreachable_objects.contains(&id),
            "{id}"
        );
    }
    assert_eq!(
        fs::read(workspace.checkout_root().unwrap().join("same.txt")).unwrap(),
        b"base\0bytes"
    );
}

#[test]
fn historical_worktree_missing_dependencies_still_report_errors() {
    let fixture = historical_states();
    let workspace = &fixture.workspace;
    let tree = StoreObjectId::Tree(fixture.materialized.clone());
    let path = {
        let store = workspace.open_store().unwrap();
        NativeCas::open(&store).unwrap().object_path(&tree).unwrap()
    };
    // Deliberate corruption is confined to this newly generated test fixture.
    fs::remove_file(path).unwrap();
    let missing_bytes =
        scm_v1::fsck_existing(workspace.store_root(), FsckOptions::default()).unwrap();
    assert!(!missing_bytes.is_clean());
    assert!(missing_bytes
        .findings
        .iter()
        .any(|finding| finding.code == FsckFindingCode::MissingCasObject
            && finding.object_id.as_deref() == Some(fixture.materialized.as_str())));
    let exclusive = scm_v1::open_exclusive(workspace.store_root()).unwrap();
    let connection =
        rusqlite::Connection::open(workspace.store_root().join("metadata/layrs.sqlite3")).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "DELETE FROM objects WHERE object_id=?1",
                [fixture.materialized.as_str()]
            )
            .unwrap(),
        1
    );
    connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(connection);
    drop(exclusive);
    let missing_index =
        scm_v1::fsck_existing(workspace.store_root(), FsckOptions::default()).unwrap();
    assert!(!missing_index.is_clean());
    assert!(!missing_index.is_truncated());
    assert!(missing_index.findings.iter().any(|finding| finding.code
        == FsckFindingCode::AdmissionClosureBroken
        && finding.object_id.as_deref() == Some(fixture.historical_view.as_str())
        && finding.message.contains(fixture.materialized.as_str())));
    assert!(missing_index.gc_reachability.reachable_objects <= missing_index.checked_objects);
}

#[test]
fn missing_historical_worktree_base_step_reports_an_explicit_dependency_error() {
    let fixture = historical_states();
    let workspace = &fixture.workspace;
    let before = scm_v1::fsck_existing(workspace.store_root(), FsckOptions::default()).unwrap();
    assert!(before.is_clean(), "{:?}", before.findings);
    let exclusive = scm_v1::open_exclusive(workspace.store_root()).unwrap();
    let connection =
        rusqlite::Connection::open(workspace.store_root().join("metadata/layrs.sqlite3")).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    // Remove only this isolated fixture's historical base index entry. Its
    // canonical View still refers to it; no current Worktree uses this base.
    assert_eq!(
        connection
            .execute(
                "DELETE FROM objects WHERE object_id=?1",
                [fixture.base.as_str()]
            )
            .unwrap(),
        1
    );
    connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(connection);
    drop(exclusive);
    let report = scm_v1::fsck_existing(workspace.store_root(), FsckOptions::default()).unwrap();
    assert!(!report.is_clean());
    assert!(!report.is_truncated());
    assert!(report.findings.iter().any(|finding| finding.code
        == FsckFindingCode::AdmissionClosureBroken
        && finding.object_id.as_deref() == Some(fixture.historical_view.as_str())
        && finding.message.contains(fixture.base.as_str())));
    assert!(report.gc_reachability.reachable_objects <= report.checked_objects);
}
