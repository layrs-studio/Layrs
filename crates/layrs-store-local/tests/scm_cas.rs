use layrs_core::scm::{
    Chunk, ChunkingDescriptor, ConflictId, FileManifest, LayerHeadSet, LayerId, ObjectId,
    RepoPathComponent, RepositoryView, SpaceId, Step, StoreId, Symlink, Tombstone, Tree, TreeEntry,
    TreeEntryTarget, TreeId, canonical_json,
};
use layrs_store::scm::{CanonicalConflict, CanonicalObject, StoreObjectId};
use layrs_store_local::scm_v1::{
    CasError, CasFaultInjector, CasFaultPoint, NativeCas, NativeStoreExclusive, NativeStoreOpen,
    init as init_store, open as open_store, open_audit, open_exclusive,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "layrs-scm-cas-{name}-{}-{nanos:032x}-{sequence:016x}",
            std::process::id()
        ));
        Self(path)
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

fn fixture_objects() -> Vec<CanonicalObject> {
    let chunk = Chunk::new(b"native cas bytes".to_vec()).unwrap();
    let manifest =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let conflict = fixture_conflict();
    let tree = Tree::new(vec![
        TreeEntry::new(
            RepoPathComponent::parse("file.txt").unwrap(),
            TreeEntryTarget::file(manifest.file_id().clone()),
            false,
        ),
        TreeEntry::new(
            RepoPathComponent::parse("conflict.txt").unwrap(),
            TreeEntryTarget::conflict(conflict.id().clone()),
            false,
        ),
    ])
    .unwrap();
    let symlink = Symlink::new("../shared/file.txt").unwrap();
    let tombstone = Tombstone::new(Some(
        ObjectId::parse("blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap(),
    ))
    .unwrap();
    let step = Step::new(vec![], tree.id().clone()).unwrap();
    let view = RepositoryView::new(
        SpaceId::parse("space:cas-tests").unwrap(),
        vec![
            LayerHeadSet::new(
                LayerId::parse("layer:main").unwrap(),
                vec![step.id().clone()],
            )
            .unwrap(),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    vec![
        CanonicalObject::ChunkRaw(chunk),
        CanonicalObject::FileManifest(manifest),
        CanonicalObject::Tree(tree),
        CanonicalObject::Symlink(symlink),
        CanonicalObject::Tombstone(tombstone),
        CanonicalObject::Step(step),
        CanonicalObject::RepositoryView(view),
        CanonicalObject::Conflict(conflict),
    ]
}

fn fixture_conflict() -> CanonicalConflict {
    let payload = serde_json::json!({
        "schemaVersion": "layrs.conflict.opaque.v1",
        "opaque": {"kind": "test", "sides": [1, 2]}
    });
    let payload_bytes = canonical_json(&payload).unwrap();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-conflict-v1");
    hasher.update(&[0]);
    hasher.update(&payload_bytes);
    let id = ConflictId::parse(format!("blake3:{}", hasher.finalize().to_hex())).unwrap();
    let mut document = payload;
    document
        .as_object_mut()
        .unwrap()
        .insert("conflictId".into(), id.to_string().into());
    CanonicalConflict::new(id, canonical_json(&document).unwrap()).unwrap()
}

fn init_handle(root: &TestRoot) -> NativeStoreOpen {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    init_store(
        root.path(),
        StoreId::parse(format!("store:cas-{sequence}")).unwrap(),
    )
    .unwrap()
}

fn exclusive_store(root: &TestRoot) -> NativeStoreExclusive {
    drop(init_handle(root));
    open_exclusive(root.path()).unwrap()
}

#[test]
fn cas_open_requires_a_ready_handle_and_never_bootstraps_paths() {
    let absent = TestRoot::new("absent-gate");
    assert!(open_store(absent.path()).is_err());
    assert!(!absent.path().exists());

    let unknown = TestRoot::new("unknown-gate");
    fs::create_dir_all(unknown.path()).unwrap();
    fs::write(unknown.path().join("foreign.data"), b"foreign").unwrap();
    assert!(open_store(unknown.path()).is_err());
    assert!(!unknown.path().join("objects").exists());
    assert!(!unknown.path().join("staging").exists());

    let ready = TestRoot::new("ready-gate");
    let handle = init_handle(&ready);
    fs::remove_dir_all(handle.layout().objects_dir()).unwrap();
    assert!(NativeCas::open(&handle).is_err());
    assert!(!handle.layout().objects_dir().exists());
}

#[test]
fn audit_and_exclusive_capabilities_open_only_their_cas_surface() {
    let root = TestRoot::new("capability-surfaces");
    let handle = init_handle(&root);
    let object = CanonicalObject::ChunkRaw(Chunk::new(b"capability object".to_vec()).unwrap());
    let object_id = object.object_id();
    drop(handle);
    let exclusive = open_exclusive(root.path()).unwrap();
    NativeCas::open_exclusive(&exclusive)
        .unwrap()
        .put(&object)
        .unwrap();
    drop(exclusive);

    let audit = open_audit(root.path()).unwrap();
    let audit_cas = NativeCas::open_audit(&audit).unwrap();
    assert_eq!(audit_cas.read(&object_id).unwrap(), object);
    drop(audit_cas);
    drop(audit);

    let exclusive = open_exclusive(root.path()).unwrap();
    let exclusive_cas = NativeCas::open_exclusive(&exclusive).unwrap();
    assert_eq!(exclusive_cas.recover_staging().unwrap(), 0);
}

#[test]
fn all_canonical_kinds_roundtrip_and_chunks_remain_raw() {
    let root = TestRoot::new("all-kinds");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let objects = fixture_objects();
    assert_eq!(objects.len(), 8);

    for object in &objects {
        let outcome = cas.put(object).unwrap();
        assert!(!outcome.existed);
        assert_eq!(outcome.object_id, object.object_id());
        assert!(cas.contains(&object.object_id()).unwrap());
        cas.verify(&object.object_id()).unwrap();
        assert_eq!(cas.read(&object.object_id()).unwrap(), *object);
    }

    let chunk = &objects[0];
    let CanonicalObject::ChunkRaw(chunk_value) = chunk else {
        unreachable!();
    };
    assert_eq!(
        fs::read(cas.object_path(&chunk.object_id()).unwrap()).unwrap(),
        chunk_value.bytes()
    );
    let tree_bytes = fs::read(cas.object_path(&objects[2].object_id()).unwrap()).unwrap();
    assert!(tree_bytes.starts_with(b"{"));
    assert!(
        String::from_utf8(tree_bytes)
            .unwrap()
            .contains("layrs.cas-envelope.v1")
    );

    let report = cas.scrub().unwrap();
    assert_eq!(report.checked_objects, objects.len() as u64);
    assert!(report.is_clean(), "{:?}", report.issues);
}

#[test]
fn shard_path_is_typed_and_idempotent_put_is_byte_exact() {
    let root = TestRoot::new("shards");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let object = CanonicalObject::Symlink(Symlink::new("target").unwrap());
    let id = object.object_id();
    let digest = id.as_str().strip_prefix("blake3:").unwrap();
    let path = cas.object_path(&id).unwrap();
    assert_eq!(
        path.strip_prefix(cas.objects_dir()).unwrap(),
        Path::new("symlink")
            .join(&digest[0..2])
            .join(&digest[2..4])
            .join(digest)
    );

    let first = cas.put(&object).unwrap();
    let bytes = fs::read(&first.path).unwrap();
    let second = cas.put(&object).unwrap();
    assert!(!first.existed);
    assert!(second.existed);
    assert_eq!(fs::read(&second.path).unwrap(), bytes);
}

#[test]
fn tampering_is_reported_and_existing_bytes_are_never_overwritten() {
    let root = TestRoot::new("tamper");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let object = CanonicalObject::ChunkRaw(Chunk::new(b"original".to_vec()).unwrap());
    let id = object.object_id();
    let path = cas.put(&object).unwrap().path;
    fs::write(&path, b"tampered").unwrap();

    assert!(matches!(cas.verify(&id), Err(CasError::IdMismatch { .. })));
    assert_eq!(
        cas.put(&object).unwrap_err().code(),
        "cas_existing_object_mismatch"
    );
    assert_eq!(fs::read(&path).unwrap(), b"tampered");
    let scrub = cas.scrub().unwrap();
    assert!(!scrub.is_clean());
    assert_eq!(scrub.issues[0].code, "cas_id_mismatch");
}

#[test]
fn wrong_kind_and_envelope_id_have_stable_errors() {
    let root = TestRoot::new("wrong-kind");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let object = CanonicalObject::Symlink(Symlink::new("target").unwrap());
    let source = cas.put(&object).unwrap().path;
    let fake_tree = StoreObjectId::Tree(TreeId::parse(object.object_id().as_str()).unwrap());
    let fake_path = cas.object_path(&fake_tree).unwrap();
    fs::create_dir_all(fake_path.parent().unwrap()).unwrap();
    fs::copy(source, &fake_path).unwrap();
    assert_eq!(
        cas.verify(&fake_tree).unwrap_err().code(),
        "cas_kind_mismatch"
    );

    let tree = fixture_objects().remove(2);
    let tree_id = tree.object_id();
    let path = cas.put(&tree).unwrap().path;
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    envelope["objectId"] = serde_json::Value::String(
        "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
    );
    fs::write(&path, serde_jcs::to_vec(&envelope).unwrap()).unwrap();
    assert_eq!(cas.verify(&tree_id).unwrap_err().code(), "cas_id_mismatch");

    let corrupt = CanonicalObject::Tombstone(Tombstone::new(None).unwrap());
    let corrupt_id = corrupt.object_id();
    let corrupt_path = cas.put(&corrupt).unwrap().path;
    fs::write(corrupt_path, b"{").unwrap();
    assert_eq!(
        cas.verify(&corrupt_id).unwrap_err().code(),
        "cas_corruption"
    );

    let conflict = CanonicalObject::Conflict(fixture_conflict());
    let conflict_id = conflict.object_id();
    let conflict_path = cas.put(&conflict).unwrap().path;
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&conflict_path).unwrap()).unwrap();
    envelope["value"]["opaque"]["sides"][0] = 999.into();
    fs::write(&conflict_path, serde_jcs::to_vec(&envelope).unwrap()).unwrap();
    assert_eq!(
        cas.verify(&conflict_id).unwrap_err().code(),
        "cas_corruption"
    );
}

#[test]
fn concurrent_writers_publish_one_immutable_object() {
    let root = TestRoot::new("concurrent");
    let exclusive = exclusive_store(&root);
    let cas = Arc::new(NativeCas::open_exclusive(&exclusive).unwrap());
    let object = Arc::new(CanonicalObject::ChunkRaw(
        Chunk::new(vec![42_u8; 256 * 1024]).unwrap(),
    ));
    let barrier = Arc::new(Barrier::new(2));
    let outcomes: Vec<_> = thread::scope(|scope| {
        let mut threads = Vec::new();
        for _ in 0..2 {
            let cas = Arc::clone(&cas);
            let object = Arc::clone(&object);
            let barrier = Arc::clone(&barrier);
            threads.push(scope.spawn(move || {
                let prepared = cas.stage(&object).unwrap();
                barrier.wait();
                cas.install(prepared).unwrap()
            }));
        }
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect()
    });
    assert_eq!(outcomes.iter().filter(|outcome| outcome.existed).count(), 1);
    assert_eq!(
        outcomes.iter().filter(|outcome| !outcome.existed).count(),
        1
    );
    cas.verify(&object.object_id()).unwrap();
}

#[derive(Debug)]
struct FailOnce {
    point: CasFaultPoint,
    armed: AtomicBool,
}

impl FailOnce {
    fn new(point: CasFaultPoint) -> Self {
        Self {
            point,
            armed: AtomicBool::new(true),
        }
    }
}

impl CasFaultInjector for FailOnce {
    fn check(&self, point: CasFaultPoint) -> Result<(), String> {
        if point == self.point && self.armed.swap(false, Ordering::SeqCst) {
            Err("simulated crash boundary".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn crash_boundaries_leave_recoverable_staging_and_never_hide_published_objects() {
    let root = TestRoot::new("fault-staging");
    let exclusive = exclusive_store(&root);
    let cas = layrs_store_local::scm_v1::NativeCasWriter::with_fault_injector(
        &exclusive,
        Arc::new(FailOnce::new(CasFaultPoint::AfterStagingSync)),
    )
    .unwrap();
    let object = CanonicalObject::ChunkRaw(Chunk::new(b"fault staged".to_vec()).unwrap());
    assert_eq!(cas.stage(&object).unwrap_err().code(), "cas_injected_fault");
    drop(cas);
    drop(exclusive);
    let exclusive = open_exclusive(root.path()).unwrap();
    let recovered = NativeCas::open_exclusive(&exclusive)
        .unwrap()
        .recover_staging()
        .unwrap();
    assert_eq!(recovered, 1);

    let root = TestRoot::new("fault-published");
    let exclusive = exclusive_store(&root);
    let cas = layrs_store_local::scm_v1::NativeCasWriter::with_fault_injector(
        &exclusive,
        Arc::new(FailOnce::new(CasFaultPoint::AfterPublication)),
    )
    .unwrap();
    let prepared = cas.stage(&object).unwrap();
    assert_eq!(
        cas.install(prepared).unwrap_err().code(),
        "cas_injected_fault"
    );
    drop(cas);
    drop(exclusive);
    let exclusive = open_exclusive(root.path()).unwrap();
    let recovered = NativeCas::open_exclusive(&exclusive).unwrap();
    recovered.verify(&object.object_id()).unwrap();
    let _ = recovered.recover_staging().unwrap();
}

#[test]
fn orphan_staging_cleanup_is_explicit_and_durable() {
    let root = TestRoot::new("orphan");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let object = CanonicalObject::ChunkRaw(Chunk::new(b"orphan".to_vec()).unwrap());
    let prepared = cas.stage(&object).unwrap();
    assert!(prepared.staging_path().exists());
    drop(prepared);
    assert_eq!(cas.recover_staging().unwrap(), 1);
    let remaining: Vec<_> = fs::read_dir(cas.staging_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(remaining, [".lease"]);
}

#[test]
fn recovery_in_another_process_waits_for_active_staging_lease() {
    let root = TestRoot::new("lease-fence");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let object = CanonicalObject::ChunkRaw(Chunk::new(b"active stage".to_vec()).unwrap());
    let prepared = cas.stage(&object).unwrap();
    let staged_path = prepared.staging_path().to_path_buf();
    let started_path = root.path().join("recovery-child.started");
    let done_path = root.path().join("recovery-child.done");
    drop(cas);
    drop(exclusive);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "staging_recovery_child_process", "--ignored"])
        .env("LAYRS_CAS_RECOVERY_CHILD_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();

    wait_for_path(&started_path);
    thread::sleep(Duration::from_millis(150));
    assert!(child.try_wait().unwrap().is_none());
    assert!(!done_path.exists());
    assert!(staged_path.exists());
    drop(prepared);
    assert!(wait_for_child(&mut child).success());
    assert_eq!(fs::read_to_string(done_path).unwrap(), "1");
    assert!(!staged_path.exists());
}

#[test]
#[ignore = "process helper, launched by recovery_in_another_process_waits_for_active_staging_lease"]
fn staging_recovery_child_process() {
    let root = PathBuf::from(std::env::var_os("LAYRS_CAS_RECOVERY_CHILD_ROOT").unwrap());
    let exclusive = open_exclusive(&root).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    fs::write(root.join("recovery-child.started"), b"started").unwrap();
    let recovered = cas.recover_staging().unwrap();
    fs::write(root.join("recovery-child.done"), recovered.to_string()).unwrap();
}

fn wait_for_path(path: &Path) {
    for _ in 0..500 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {}", path.display());
}

fn wait_for_child(child: &mut Child) -> ExitStatus {
    for _ in 0..500 {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    let _ = child.wait();
    panic!("timed out waiting for recovery child process");
}

#[test]
fn one_logical_file_can_install_two_distinct_manifests() {
    let root = TestRoot::new("manifests");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let chunk = Chunk::new(b"same logical file".to_vec()).unwrap();
    let first =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let second = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(chunk.byte_length()).unwrap(),
        std::slice::from_ref(&chunk),
    )
    .unwrap();
    assert_eq!(first.file_id(), second.file_id());
    assert_ne!(first.id(), second.id());

    let first_id = StoreObjectId::FileManifest(first.id().clone());
    let second_id = StoreObjectId::FileManifest(second.id().clone());
    cas.put(&CanonicalObject::ChunkRaw(chunk)).unwrap();
    cas.put(&CanonicalObject::FileManifest(first)).unwrap();
    cas.put(&CanonicalObject::FileManifest(second)).unwrap();
    assert!(cas.contains(&first_id).unwrap());
    assert!(cas.contains(&second_id).unwrap());
}

#[test]
fn verified_chunk_stream_stays_on_its_pinned_handle_when_path_is_replaced() {
    let root = TestRoot::new("pinned-stream");
    let exclusive = exclusive_store(&root);
    let writer = NativeCas::open_exclusive(&exclusive).unwrap();
    let chunk = Chunk::new(vec![17_u8; 256 * 1024]).unwrap();
    let outcome = writer
        .put(&CanonicalObject::ChunkRaw(chunk.clone()))
        .unwrap();
    drop(writer);
    drop(exclusive);

    let audit = open_audit(root.path()).unwrap();
    let cas = NativeCas::open_audit(&audit).unwrap();
    let replacement = outcome.path.with_extension("replaced-during-stream");
    let mut replaced = false;
    let mut observed = 0_u64;
    let length = cas
        .stream_verified_chunk(chunk.id(), |bytes| {
            observed += bytes.len() as u64;
            if !replaced {
                fs::rename(&outcome.path, &replacement).unwrap();
                fs::write(&outcome.path, b"hostile replacement").unwrap();
                replaced = true;
            }
        })
        .unwrap();
    assert!(replaced);
    assert_eq!(length, chunk.byte_length());
    assert_eq!(observed, chunk.byte_length());
    assert_eq!(fs::read(&outcome.path).unwrap(), b"hostile replacement");
}

#[test]
fn publication_never_follows_a_preplaced_shard_link() {
    let root = TestRoot::new("linked-shard");
    let exclusive = exclusive_store(&root);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let outside = root.path().with_extension("outside-shard");
    fs::create_dir_all(&outside).unwrap();
    let linked_kind = cas.objects_dir().join("chunk_raw");
    create_directory_link(&outside, &linked_kind);
    let before: Vec<_> = fs::read_dir(&outside).unwrap().collect();
    let object = CanonicalObject::ChunkRaw(Chunk::new(b"must stay inside".to_vec()).unwrap());
    assert!(cas.put(&object).is_err());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), before.len());
    remove_directory_link(&linked_kind);
    fs::remove_dir_all(outside).unwrap();
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) {
    fs::remove_file(link).unwrap();
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) {
    fs::remove_dir(link).unwrap();
}
