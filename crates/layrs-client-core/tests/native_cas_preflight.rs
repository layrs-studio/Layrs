mod worktree_support;
use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::{CanonicalObject, ScmReader, StoreError, StoreObjectId};
use layrs_store_local::scm_v1::{
    CasFaultInjector, CasFaultPoint, NativeScmReader, NativeScmRepository, RepositoryFaultInjector,
    RepositoryFaultPoint,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use worktree_support::*;

#[derive(Default)]
struct InstallationChecks(AtomicUsize);
impl CasFaultInjector for InstallationChecks {
    fn check(&self, _: CasFaultPoint) -> std::result::Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl RepositoryFaultInjector for InstallationChecks {
    fn check(&self, _: RepositoryFaultPoint) -> std::result::Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// Include directories and exact file contents: an unchanged object count alone
// would miss a replaced object, SQLite mutation, or a new staging directory.
fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(kind.is_dir() || kind.is_file());
            let bytes = if kind.is_dir() {
                None
            } else {
                Some(fs::read(&path).unwrap())
            };
            out.insert(path.strip_prefix(root).unwrap().to_owned(), bytes);
            if kind.is_dir() {
                visit(root, &path, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn prepare(workspace: &NativeWorkspace) -> PreparedMutation {
    let CapturePlan::Change(change) = workspace
        .client()
        .prepare_disk_capture(
            workspace.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected changed checkout")
    };
    change
}

fn history(workspace: &NativeWorkspace) -> Vec<OperationId> {
    workspace
        .operation_history(None, 20, "")
        .unwrap()
        .operations
        .into_iter()
        .map(|operation| operation.id().clone())
        .collect()
}

#[test]
fn stale_independent_capture_installs_nothing_then_fresh_capture_preserves_both_histories() {
    let fixture = Fixture::new();
    let a = fixture.primary();
    let b = fixture.secondary(&a);
    let before_a = a.state().unwrap();
    let before_b = b.state().unwrap();
    let history_a = history(&a);
    let history_b = history(&b);
    let base = own(&a).base_step_id().unwrap().clone();
    let root_a = a.checkout_root().unwrap();
    let root_b = b.checkout_root().unwrap();
    let bytes_a = b"independent winner bytes";
    let bytes_b = b"independent losing attempt bytes";
    fs::write(root_a.join("same.txt"), bytes_a).unwrap();
    fs::write(root_b.join("same.txt"), bytes_b).unwrap();
    let winner = prepare(&a);
    let loser = prepare(&b);
    let digest = loser.batch().request_digest();
    let transaction = loser.batch().transaction_id().clone();
    let chunk = Chunk::new(bytes_b.to_vec()).unwrap();
    let descriptors: BTreeSet<_> = loser
        .batch()
        .canonical_objects()
        .iter()
        .filter_map(|object| match object {
            CanonicalObject::FileManifest(manifest) => Some(manifest),
            _ => None,
        })
        .flat_map(|manifest| {
            manifest
                .chunks()
                .iter()
                .map(|entry| (entry.chunk_id().clone(), entry.byte_length()))
        })
        .collect();
    assert_eq!(
        descriptors,
        BTreeSet::from([(chunk.id().clone(), bytes_b.len() as u64)])
    );
    a.client().accept(&winner).unwrap();
    let committed_a = a.state().unwrap().operation.id().clone();
    let checks = Arc::new(InstallationChecks::default());
    let mut repository =
        NativeScmRepository::with_fault_injectors(a.store_root(), checks.clone(), checks.clone())
            .with_store_identity(a.store_identity())
            .with_metadata_identity(a.metadata_identity())
            .with_store_id(before_a.worktree.store_id().clone());
    let before = inventory(a.store_root());
    let mut chunk_reads = 0;
    let error = repository
        .apply_batch_with_chunks(
            loser.batch(),
            &descriptors.into_iter().collect::<Vec<_>>(),
            |id| {
                chunk_reads += 1;
                assert_eq!(id, chunk.id());
                Ok(bytes_b.to_vec())
            },
        )
        .unwrap_err();
    // Mutation targets are canonically ordered; operation_heads is first.
    assert!(
        matches!(&error, StoreError::CasMismatch { target }
        if target == &format!("operation_heads:{}", before_a.space.id())),
        "{error:?}"
    );
    assert_eq!(chunk_reads, 0);
    assert_eq!(checks.0.load(Ordering::SeqCst), 0);
    assert_eq!(inventory(a.store_root()), before);
    // The public facade must preserve the original prepared batch's exact CAS,
    // rather than silently reprepare it or install its streamed bytes.
    let facade_before = inventory(a.store_root());
    let facade_error = b.client().accept(&loser).unwrap_err();
    assert!(
        matches!(&facade_error, NativeClientError::Store(StoreError::CasMismatch { target })
        if target == &format!("operation_heads:{}", before_a.space.id())),
        "{facade_error:?}"
    );
    assert_eq!(inventory(a.store_root()), facade_before);
    assert_eq!(facade_before, before);
    assert_eq!(loser.batch().request_digest(), digest);
    assert_eq!(loser.batch().transaction_id(), &transaction);
    assert_eq!(b.state().unwrap().operation.id(), before_b.operation.id());
    assert!(!repository
        .object_admitted_to_space(
            &StoreObjectId::Step(loser.step_id().clone()),
            before_a.space.id(),
        )
        .unwrap());

    let fresh = prepare(&b);
    assert_ne!(fresh.batch().transaction_id(), &transaction);
    assert_eq!(fresh.step_id(), loser.step_id());
    b.client().accept(&fresh).unwrap();
    let reopened_a = NativeWorkspace::open(&root_a).unwrap();
    let reopened_b = NativeWorkspace::open(&root_b).unwrap();
    let state_a = reopened_a.state().unwrap();
    let state_b = reopened_b.state().unwrap();
    assert_eq!(state_a.operation.id(), &committed_a);
    assert_eq!(
        state_a.operation.parent_operation_ids(),
        &[before_a.operation.id().clone()]
    );
    assert_eq!(
        state_b.operation.parent_operation_ids(),
        &[before_b.operation.id().clone()]
    );
    assert_eq!(
        history(&reopened_a),
        [vec![committed_a.clone()], history_a].concat()
    );
    assert_eq!(
        history(&reopened_b),
        [vec![state_b.operation.id().clone()], history_b].concat()
    );
    assert_eq!(fs::read(root_a.join("same.txt")).unwrap(), bytes_a);
    assert_eq!(fs::read(root_b.join("same.txt")).unwrap(), bytes_b);
    let store = a.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .operation_heads(before_a.space.id())
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([committed_a, state_b.operation.id().clone()])
    );
    for step in [winner.step_id(), fresh.step_id()] {
        assert_eq!(
            reader.get_step(step).unwrap().unwrap().parent_step_ids(),
            &[base.clone()]
        );
    }
    drop(reader);
    drop(store);
    assert_clean(
        &a,
        &[base, winner.step_id().clone(), fresh.step_id().clone()],
    );
    // Reconcile exact paths and lengths after the explicit recapture, without
    // a cleanup pass or treating unindexed files as acceptable audit results.
    let store = a.open_store().unwrap();
    let database = a.store_root().join("metadata/layrs.sqlite3");
    assert!(!database.with_extension("sqlite3-wal").exists());
    let mut uri = url::Url::from_file_path(database).unwrap();
    uri.set_query(Some("mode=ro&immutable=1"));
    let connection = rusqlite::Connection::open_with_flags(
        uri.as_str(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let indexed: BTreeMap<String, u64> = connection
        .prepare("SELECT storage_path,byte_length FROM objects")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let physical: BTreeMap<String, u64> = inventory(&a.store_root().join("objects"))
        .into_iter()
        .filter_map(|(path, bytes)| {
            bytes.map(|bytes| {
                (
                    format!("objects/{}", path.to_string_lossy().replace('\\', "/")),
                    bytes.len() as u64,
                )
            })
        })
        .collect();
    assert_eq!(physical, indexed);
    drop(connection);
    drop(store);
}
