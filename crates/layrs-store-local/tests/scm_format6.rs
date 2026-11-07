//! Store format 6 (ADR 0028, `docs/plans/store-format-6.md`): packs v2
//! registered in SQLite, per-Space admission bitmaps, file records.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use layrs_core::scm::{
    Chunk, ChunkingDescriptor, FileId, FileManifest, Layer, LayerHeadSet, LayerId, LayerKind,
    LayerSettings, LayerStatus, Metadata, Operation, OperationId, OperationKind, RepoPathComponent,
    RepositoryView, RepositoryViewId, Space, SpaceId, Step, StoreId, Timestamp, Tree, TreeEntry,
    TreeEntryTarget, fixed_chunking,
};
use layrs_store::scm::{
    CanonicalObject, EntityRecord, FileAvailability, OperationHeadsCas, RepositoryViewCas,
    ScmReader, ScmRepository, SpaceObjectAdmission, StoreError, StoreMutation, StoreObjectId,
    TransactionId, WriteBatch,
};
use layrs_store_local::scm_v1::{
    CasError, CasFaultInjector, CasFaultPoint, CasLocation, FsckFindingCode, FsckOptions,
    NativeCas, NativeScmReader, NativeScmRepository, RepositoryFaultInjector, RepositoryFaultPoint,
    fsck_existing, init, open, open_exclusive, set_inline_pack_limit,
};
use rusqlite::Connection;

const MIB: usize = 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// `init` reads `LAYRS_STORE_FORMAT`; tests of this binary set it only
/// under this lock.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(label: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "layrs-format6-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn create(root: &Path, label: &str, format: &str) {
    let _guard = serial();
    // These tests damage and count pack files: every pack is a file here.
    // The inline packs of small batches (council s7 Q1) have tests of their
    // own, which set the limit back.
    set_inline_pack_limit(root, Some(0));
    // SAFETY: every reader of this variable in this binary holds `serial`.
    unsafe { std::env::set_var("LAYRS_STORE_FORMAT", format) };
    let opened = init(root, StoreId::parse(format!("store:{label}")).unwrap());
    unsafe { std::env::remove_var("LAYRS_STORE_FORMAT") };
    let opened = opened.unwrap();
    assert_eq!(opened.format_version(), format.parse::<u32>().unwrap());
}

fn timestamp() -> Timestamp {
    Timestamp::parse("2026-11-06T00:00:00Z").unwrap()
}

#[derive(Clone, Copy)]
enum Recipe {
    Fixed,
    Single,
    Half,
}

/// One File: its chunks (by its recipe) and manifest.
#[derive(Clone)]
struct File {
    name: String,
    chunks: Vec<Chunk>,
    manifest: FileManifest,
}

fn file(name: &str, bytes: &[u8], recipe: Recipe) -> File {
    let (chunking, size) = match recipe {
        Recipe::Fixed => (fixed_chunking(bytes.len() as u64), MIB),
        Recipe::Single => (ChunkingDescriptor::single(), usize::MAX),
        Recipe::Half => (
            ChunkingDescriptor::deterministic_fixed(512 * 1024).unwrap(),
            512 * 1024,
        ),
    };
    let chunks: Vec<Chunk> = bytes
        .chunks(size.min(bytes.len().max(1)))
        .filter(|part| !part.is_empty())
        .map(|part| Chunk::new(part.to_vec()).unwrap())
        .collect();
    let manifest = FileManifest::from_chunks(chunking, &chunks).unwrap();
    File {
        name: name.into(),
        chunks,
        manifest,
    }
}

fn file_objects(files: &[File]) -> Vec<CanonicalObject> {
    let mut objects = Vec::new();
    for file in files {
        objects.extend(file.chunks.iter().cloned().map(CanonicalObject::ChunkRaw));
        objects.push(CanonicalObject::FileManifest(file.manifest.clone()));
    }
    objects
}

fn tree_of(files: &[File]) -> Tree {
    let mut entries: Vec<TreeEntry> = files
        .iter()
        .map(|file| {
            TreeEntry::new(
                RepoPathComponent::parse(&file.name).unwrap(),
                TreeEntryTarget::file(file.manifest.file_id().clone()),
                false,
            )
        })
        .collect();
    entries.sort_by(|left, right| left.name().as_str().cmp(right.name().as_str()));
    Tree::new(entries).unwrap()
}

/// A Space and its current head, to chain capture batches.
struct Lane {
    space: Space,
    layer: Layer,
    head: OperationId,
    view: RepositoryViewId,
    generation: u64,
    sequence: u64,
}

fn dedup(objects: Vec<CanonicalObject>) -> Vec<CanonicalObject> {
    let mut seen = std::collections::BTreeSet::new();
    objects
        .into_iter()
        .filter(|object| seen.insert(object.object_id()))
        .collect()
}

impl Lane {
    /// The Initialize batch of a new Space holding `files` in one Step.
    fn begin(label: &str, files: &[File]) -> (Self, WriteBatch) {
        let space = Space::new(
            SpaceId::parse(format!("space:{label}")).unwrap(),
            "Space",
            timestamp(),
            Metadata::empty(),
        )
        .unwrap();
        let layer = Layer::new(
            LayerId::parse(format!("layer:{label}")).unwrap(),
            space.id().clone(),
            "Main",
            LayerKind::Base,
            LayerStatus::Active,
            vec![],
            LayerSettings::new(true, 100).unwrap(),
            timestamp(),
            Metadata::empty(),
        )
        .unwrap();
        let tree = tree_of(files);
        let step = Step::new(vec![], tree.id().clone()).unwrap();
        let view = RepositoryView::new(
            space.id().clone(),
            vec![LayerHeadSet::new(layer.id().clone(), vec![step.id().clone()]).unwrap()],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        let operation = Operation::new(
            OperationId::parse(format!("operation:{label}-0")).unwrap(),
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
        let mut objects = file_objects(files);
        objects.extend([
            CanonicalObject::Tree(tree),
            CanonicalObject::Step(step),
            CanonicalObject::RepositoryView(view.clone()),
        ]);
        let objects = dedup(objects);
        let admissions = objects
            .iter()
            .map(|object| SpaceObjectAdmission::new(space.id().clone(), object.object_id()))
            .collect();
        let batch = WriteBatch::new(
            TransactionId::new(format!("transaction:{label}-0")).unwrap(),
            objects,
            vec![
                EntityRecord::Space(space.clone()),
                EntityRecord::Layer(layer.clone()),
                EntityRecord::Operation(operation.clone()),
            ],
            admissions,
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
        .unwrap();
        (
            Self {
                space,
                layer,
                head: operation.id().clone(),
                view: view.id().clone(),
                generation: 1,
                sequence: 0,
            },
            batch,
        )
    }

    /// A capture whose Step holds `files`; `carried` objects are in the
    /// batch (the others must be durably admitted already).
    fn capture(&mut self, files: &[File], carried: Vec<CanonicalObject>) -> WriteBatch {
        self.sequence += 1;
        let label = self
            .space
            .id()
            .as_str()
            .trim_start_matches("space:")
            .to_owned();
        let tree = tree_of(files);
        let step = Step::new(vec![], tree.id().clone()).unwrap();
        let view = RepositoryView::new(
            self.space.id().clone(),
            vec![LayerHeadSet::new(self.layer.id().clone(), vec![step.id().clone()]).unwrap()],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        let operation = Operation::new(
            OperationId::parse(format!("operation:{label}-{}", self.sequence)).unwrap(),
            self.space.id().clone(),
            vec![self.head.clone()],
            view.id().clone(),
            OperationKind::CaptureWorktree,
            "Capture",
            vec![],
            None,
            None,
            timestamp(),
            Metadata::empty(),
        )
        .unwrap();
        let mut objects = carried;
        objects.extend([
            CanonicalObject::Tree(tree),
            CanonicalObject::Step(step),
            CanonicalObject::RepositoryView(view.clone()),
        ]);
        let objects = dedup(objects);
        let admissions = objects
            .iter()
            .map(|object| SpaceObjectAdmission::new(self.space.id().clone(), object.object_id()))
            .collect();
        let batch = WriteBatch::new(
            TransactionId::new(format!("transaction:{label}-{}", self.sequence)).unwrap(),
            objects,
            vec![EntityRecord::Operation(operation.clone())],
            admissions,
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        self.space.id().clone(),
                        self.generation,
                        Some(self.view.clone()),
                        view.id().clone(),
                    )
                    .unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        self.space.id().clone(),
                        vec![self.head.clone()],
                        operation.id().clone(),
                        vec![operation.id().clone()],
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap();
        self.head = operation.id().clone();
        self.view = view.id().clone();
        self.generation += 1;
        batch
    }
}

fn sql(root: &Path) -> Connection {
    Connection::open(root.join("metadata/layrs.sqlite3")).unwrap()
}

fn count(root: &Path, query: &str) -> i64 {
    sql(root).query_row(query, [], |row| row.get(0)).unwrap()
}

fn packs(root: &Path) -> Vec<PathBuf> {
    let mut packs: Vec<PathBuf> = fs::read_dir(root.join("objects/packs"))
        .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
        .unwrap_or_default();
    packs.sort();
    packs
}

fn registered_packs(root: &Path) -> Vec<String> {
    let connection = sql(root);
    let mut statement = connection
        .prepare("SELECT name FROM cas_packs ORDER BY pack_seq")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// (explicit records, file records) from a pack v2 footer.
fn pack_counts(path: &Path) -> (u64, u64) {
    let bytes = fs::read(path).unwrap();
    let footer = &bytes[bytes.len() - 72..];
    let word = |at: usize| u64::from_le_bytes(footer[at..at + 8].try_into().unwrap());
    (word(16), word(24))
}

fn assert_fsck_clean(root: &Path) {
    let report = fsck_existing(root, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:#?}", report.findings);
}

fn reader_answers(root: &Path, files: &[File], space: &SpaceId) -> Vec<String> {
    let store = open(root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let mut answers = Vec::new();
    for file in files {
        let id = file.manifest.file_id();
        answers.push(format!(
            "{} length={:?} manifests={:?} present={} admitted={:?} chunks={:?}",
            file.name,
            reader.file_length_metadata(id).unwrap(),
            reader.file_manifest_ids_metadata(id).unwrap(),
            reader.file_present_metadata(id).unwrap(),
            NativeScmReader::admitted_manifests_metadata(&reader, [id], space).unwrap(),
            reader
                .admitted_chunk_ids_of_files(space, [id], 100)
                .unwrap(),
        ));
        answers.push(format!(
            "manifest {:?} admitted={}",
            reader
                .get_file_manifest(file.manifest.id())
                .unwrap()
                .map(|m| m.id().clone()),
            reader
                .object_admitted_metadata(
                    &StoreObjectId::FileManifest(file.manifest.id().clone()),
                    space
                )
                .unwrap()
        ));
        for chunk in &file.chunks {
            let object = StoreObjectId::ChunkRaw(chunk.id().clone());
            answers.push(format!(
                "chunk {} present={} read={} admitted={} admitted_to={}",
                chunk.id(),
                reader.chunk_present_metadata(chunk.id()).unwrap(),
                reader.get_chunk(chunk.id()).unwrap().as_ref() == Some(chunk),
                reader.object_admitted_metadata(&object, space).unwrap(),
                reader.object_admitted_to_space(&object, space).unwrap(),
            ));
        }
        answers.push(format!(
            "availability {:?}",
            match reader.file_availability(id).unwrap() {
                FileAvailability::Present(present) => present.verified_manifest_ids().to_vec(),
                FileAvailability::Absent => Vec::new(),
            }
        ));
    }
    answers
}

fn sizes_files() -> Vec<File> {
    let one_mib: Vec<u8> = (0..MIB).map(|i| (i % 251) as u8).collect();
    let mut over: Vec<u8> = one_mib.clone();
    over.push(7);
    let large_single: Vec<u8> = (0..(MIB + 300)).map(|i| (i * 7 % 253) as u8).collect();
    vec![
        file("empty", b"", Recipe::Fixed),
        file("one", b"x", Recipe::Fixed),
        file("small", b"line 1\n\n\n", Recipe::Fixed),
        file("block", &vec![b'a'; 20 * 1024], Recipe::Fixed),
        file("mib", &one_mib, Recipe::Fixed),
        file("over", &over, Recipe::Fixed),
        file("single", b"single recipe", Recipe::Single),
        file("bigsingle", &large_single, Recipe::Single),
        file("half", b"half recipe stays explicit", Recipe::Half),
    ]
}

#[test]
fn content_round_trips_through_packs_file_records_and_loose_objects() {
    let root = TestRoot::new("round");
    create(&root.0, "round", "6");
    let files = sizes_files();
    let (lane, batch) = Lane::begin("round", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();

    // No content row in SQLite; one registered pack; loose records > 1 MiB.
    assert_eq!(
        count(
            &root.0,
            "SELECT count(*) FROM objects WHERE object_kind IN ('chunk_raw','file_manifest')"
        ),
        0
    );
    assert_eq!(registered_packs(&root.0).len(), 1);
    assert_eq!(packs(&root.0).len(), 1);
    // Single-chunk Files of a carried recipe up to 1 MiB are file records:
    // one, small, block, mib, single. The empty File, the two-chunk one, the
    // 512 KiB recipe and the loose chunk's manifest stay explicit.
    assert_eq!(pack_counts(&packs(&root.0)[0]).1, 5);
    // The 1 MiB + 300 `single` chunk is the only record over 1 MiB.
    assert_eq!(count(&root.0, "SELECT count(*) FROM loose_objects"), 1);
    assert!(
        count(&root.0, "SELECT count(*) FROM space_pack_admissions") == 1,
        "one bitmap for (Space, pack)"
    );
    for table in [
        "file_availability",
        "file_manifests",
        "file_manifest_chunks",
    ] {
        assert_eq!(
            count(
                &root.0,
                &format!("SELECT count(*) FROM sqlite_schema WHERE name='{table}'")
            ),
            0
        );
    }

    let repository = NativeScmRepository::new(&root.0);
    for file in &files {
        let id = file.manifest.file_id();
        assert_eq!(
            repository.get_file_manifest(file.manifest.id()).unwrap(),
            Some(file.manifest.clone()),
            "{}",
            file.name
        );
        for chunk in &file.chunks {
            assert_eq!(
                repository.get_chunk(chunk.id()).unwrap().as_ref(),
                Some(chunk)
            );
            assert!(repository.chunk_present_metadata(chunk.id()).unwrap());
        }
        assert_eq!(
            repository.file_manifest_ids_metadata(id).unwrap(),
            vec![file.manifest.id().clone()]
        );
        match repository.file_availability(id).unwrap() {
            FileAvailability::Present(present) => assert_eq!(
                present.verified_manifest_ids(),
                &[file.manifest.id().clone()]
            ),
            FileAvailability::Absent => panic!("{} absent", file.name),
        }
        assert!(
            repository
                .object_admitted_metadata(
                    &StoreObjectId::FileManifest(file.manifest.id().clone()),
                    lane.space.id()
                )
                .unwrap()
        );
    }
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    for file in &files {
        assert_eq!(
            reader
                .file_length_metadata(file.manifest.file_id())
                .unwrap(),
            Some(file.manifest.byte_length())
        );
    }
    let absent = Chunk::new(b"never stored".to_vec()).unwrap();
    assert!(!reader.chunk_present_metadata(absent.id()).unwrap());
    assert_eq!(reader.get_chunk(absent.id()).unwrap(), None);
    assert_eq!(
        reader
            .file_length_metadata(&FileId::from_bytes(b"never stored"))
            .unwrap(),
        None
    );
    drop(reader);
    drop(store);
    assert_fsck_clean(&root.0);
}

#[test]
fn format_5_and_format_6_answer_the_same_batch_sequence_alike() {
    let files = sizes_files();
    let more = vec![
        file("added", b"added later", Recipe::Fixed),
        files[2].clone(),
        file("alt", b"x", Recipe::Single),
    ];
    let mut answers = Vec::new();
    for format in ["5", "6"] {
        let root = TestRoot::new(&format!("differential-{format}"));
        create(&root.0, "differential", format);
        let (mut lane, batch) = Lane::begin("differential", &files);
        let mut repository = NativeScmRepository::new(&root.0);
        repository.apply_batch(&batch).unwrap();
        let carried = file_objects(&more);
        let batch = lane.capture(&more, carried);
        repository.apply_batch(&batch).unwrap();
        let all: Vec<File> = files.iter().chain(&more).cloned().collect();
        answers.push(reader_answers(&root.0, &all, lane.space.id()));
        assert_fsck_clean(&root.0);
    }
    assert_eq!(answers[0], answers[1]);
}

#[test]
fn spaces_sharing_bytes_keep_exact_admissions_and_bitmaps_only_grow() {
    let root = TestRoot::new("spaces");
    create(&root.0, "spaces", "6");
    let shared = file("shared.txt", b"the same bytes", Recipe::Fixed);
    let other = file("other.txt", b"only in A", Recipe::Fixed);
    let (lane_a, batch) = Lane::begin("alpha", &[shared.clone(), other.clone()]);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let bitmap = |space: &str| -> Vec<u8> {
        sql(&root.0)
            .query_row(
                "SELECT bitmap FROM space_pack_admissions WHERE space_id=?1 ORDER BY pack_seq LIMIT 1",
                [space],
                |row| row.get(0),
            )
            .unwrap()
    };
    let before = bitmap(lane_a.space.id().as_str());

    // Space B is created empty, then admits only the shared chunk: the
    // bytes are not written again and the manifest is not admitted to B.
    let (lane_b, batch) = Lane::begin("beta", &[]);
    repository.apply_batch(&batch).unwrap();
    let chunk = shared.chunks[0].clone();
    let alone = WriteBatch::new(
        TransactionId::new("transaction:beta-chunk").unwrap(),
        vec![CanonicalObject::ChunkRaw(chunk.clone())],
        vec![],
        vec![SpaceObjectAdmission::new(
            lane_b.space.id().clone(),
            StoreObjectId::ChunkRaw(chunk.id().clone()),
        )],
        vec![],
    )
    .unwrap();
    repository.apply_batch(&alone).unwrap();
    let chunk_object = StoreObjectId::ChunkRaw(chunk.id().clone());
    let manifest_object = StoreObjectId::FileManifest(shared.manifest.id().clone());
    assert!(
        repository
            .object_admitted_metadata(&chunk_object, lane_b.space.id())
            .unwrap()
    );
    assert!(
        !repository
            .object_admitted_metadata(&manifest_object, lane_b.space.id())
            .unwrap()
    );
    assert!(
        repository
            .object_admitted_to_space(&chunk_object, lane_b.space.id())
            .unwrap()
    );
    assert!(
        repository
            .object_admitted_metadata(&chunk_object, lane_a.space.id())
            .unwrap()
    );
    assert!(
        repository
            .object_admitted_metadata(&manifest_object, lane_a.space.id())
            .unwrap()
    );
    let other_object = StoreObjectId::FileManifest(other.manifest.id().clone());
    assert!(
        !repository
            .object_admitted_metadata(&other_object, lane_b.space.id())
            .unwrap()
    );
    // B's admission lives in A's pack (bytes shared), A's bits unchanged.
    let first_pack: i64 = sql(&root.0)
        .query_row("SELECT min(pack_seq) FROM cas_packs", [], |row| row.get(0))
        .unwrap();
    let b_rows: Vec<i64> = {
        let connection = sql(&root.0);
        let mut statement = connection
            .prepare("SELECT pack_seq FROM space_pack_admissions WHERE space_id=?1")
            .unwrap();
        statement
            .query_map([lane_b.space.id().as_str()], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert!(b_rows.contains(&first_pack));
    assert_eq!(bitmap(lane_a.space.id().as_str()), before);

    // Later admissions to A only OR bits in.
    let mut lane_a = lane_a;
    let added = file("added.txt", b"new in A", Recipe::Fixed);
    let carried = file_objects(std::slice::from_ref(&added));
    let batch = lane_a.capture(&[shared.clone(), other.clone(), added], carried);
    repository.apply_batch(&batch).unwrap();
    let after = bitmap(lane_a.space.id().as_str());
    assert_eq!(after.len(), before.len());
    assert!(
        before
            .iter()
            .zip(&after)
            .all(|(old, new)| old & new == *old)
    );
    drop(repository);
    assert_fsck_clean(&root.0);
}

#[test]
fn alternative_manifests_of_one_file_and_a_shared_chunk_resolve_exactly() {
    let root = TestRoot::new("alternatives");
    create(&root.0, "alternatives", "6");
    let bytes = b"one file, two recipes".to_vec();
    let fixed = file("fixed", &bytes, Recipe::Fixed);
    let single = file("single", &bytes, Recipe::Single);
    assert_eq!(fixed.manifest.file_id(), single.manifest.file_id());
    assert_ne!(fixed.manifest.id(), single.manifest.id());
    // A 1 MiB small file whose bytes are the first chunk of a larger file.
    let first: Vec<u8> = (0..MIB).map(|i| (i % 241) as u8).collect();
    let mut larger = first.clone();
    larger.extend_from_slice(b"tail");
    let small = file("small", &first, Recipe::Fixed);
    let large = file("large", &larger, Recipe::Fixed);
    assert_eq!(small.chunks[0], large.chunks[0]);
    let (lane, batch) = Lane::begin(
        "alternatives",
        &[fixed.clone(), small.clone(), large.clone()],
    );
    // The Initialize batch carries the second recipe too.
    let mut carried = batch.canonical_objects().to_vec();
    carried.push(CanonicalObject::FileManifest(single.manifest.clone()));
    let mut admissions = batch.admissions().to_vec();
    admissions.push(SpaceObjectAdmission::new(
        lane.space.id().clone(),
        StoreObjectId::FileManifest(single.manifest.id().clone()),
    ));
    let batch = WriteBatch::new(
        batch.transaction_id().clone(),
        dedup(carried),
        batch.entity_records().to_vec(),
        admissions,
        batch.mutations().to_vec(),
    )
    .unwrap();
    let repository = &mut NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let mut expected = vec![fixed.manifest.id().clone(), single.manifest.id().clone()];
    expected.sort();
    assert_eq!(
        repository
            .file_manifest_ids_metadata(fixed.manifest.file_id())
            .unwrap(),
        expected
    );
    for manifest in [
        &fixed.manifest,
        &single.manifest,
        &small.manifest,
        &large.manifest,
    ] {
        assert_eq!(
            repository
                .get_file_manifest(manifest.id())
                .unwrap()
                .as_ref(),
            Some(manifest)
        );
    }
    match repository
        .file_availability(large.manifest.file_id())
        .unwrap()
    {
        FileAvailability::Present(present) => {
            assert_eq!(
                present.verified_manifest_ids(),
                &[large.manifest.id().clone()]
            )
        }
        FileAvailability::Absent => panic!("large file absent"),
    }
    assert_eq!(
        repository.get_chunk(small.chunks[0].id()).unwrap().as_ref(),
        Some(&small.chunks[0])
    );
    drop(lane);
    assert_fsck_clean(&root.0);
}

struct CasFault(CasFaultPoint);
impl CasFaultInjector for CasFault {
    fn check(&self, point: CasFaultPoint) -> Result<(), String> {
        if point == self.0 {
            Err("format 6 test".into())
        } else {
            Ok(())
        }
    }
}

struct RepositoryFault(RepositoryFaultPoint);
impl RepositoryFaultInjector for RepositoryFault {
    fn check(&self, point: RepositoryFaultPoint) -> Result<(), String> {
        if point == self.0 {
            Err("format 6 test".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn crash_points_leave_the_old_state_and_orphan_packs_are_never_present() {
    let files = vec![
        file("a.txt", b"alpha", Recipe::Fixed),
        file("b.txt", b"beta", Recipe::Fixed),
    ];
    let cas_points = [
        CasFaultPoint::BeforePackPublication,
        CasFaultPoint::AfterPackPublication,
    ];
    let repository_points = [
        RepositoryFaultPoint::AfterAllCas,
        RepositoryFaultPoint::AfterValidation,
        RepositoryFaultPoint::AfterAdmissions,
        RepositoryFaultPoint::AfterReceiptBeforeCommit,
    ];
    for index in 0..cas_points.len() + repository_points.len() {
        let root = TestRoot::new("crash");
        create(&root.0, "crash", "6");
        let (lane, batch) = Lane::begin("crash", &files);
        let mut faulty = if index < cas_points.len() {
            NativeScmRepository::with_cas_fault_injector(
                &root.0,
                Arc::new(CasFault(cas_points[index])),
            )
        } else {
            NativeScmRepository::with_fault_injector(
                &root.0,
                Arc::new(RepositoryFault(repository_points[index - cas_points.len()])),
            )
        };
        assert!(faulty.apply_batch(&batch).is_err(), "fault {index}");
        drop(faulty);
        // Old state: nothing registered, nothing present, whatever files
        // a published orphan pack left.
        assert!(registered_packs(&root.0).is_empty(), "fault {index}");
        let repository = NativeScmRepository::new(&root.0);
        for file in &files {
            assert_eq!(repository.get_chunk(file.chunks[0].id()).unwrap(), None);
            assert!(
                !repository
                    .chunk_present_metadata(file.chunks[0].id())
                    .unwrap()
            );
            assert_eq!(
                repository.get_file_manifest(file.manifest.id()).unwrap(),
                None
            );
            assert_eq!(
                repository
                    .file_manifest_ids_metadata(file.manifest.file_id())
                    .unwrap(),
                Vec::new()
            );
        }
        assert_eq!(
            repository.current_repository_view(lane.space.id()).unwrap(),
            None
        );
        drop(repository);
        // A pack published before the failure stays an unregistered orphan;
        // the next exclusive session removes it, and the retry applies the
        // batch with a new pack.
        let orphans = packs(&root.0).len();
        if index >= 1 {
            assert_eq!(orphans, 1, "fault {index} leaves its published pack");
        }
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
        let registered = registered_packs(&root.0);
        assert_eq!(registered.len(), 1);
        assert_eq!(
            packs(&root.0),
            vec![root.0.join("objects/packs").join(&registered[0])],
            "fault {index}: orphans are collected"
        );
        let repository = NativeScmRepository::new(&root.0);
        for file in &files {
            assert_eq!(
                repository.get_chunk(file.chunks[0].id()).unwrap().as_ref(),
                Some(&file.chunks[0])
            );
        }
        drop(repository);
        assert_fsck_clean(&root.0);
    }
}

fn flip(path: &Path, at: u64) {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(at)).unwrap();
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(at)).unwrap();
    file.write_all(&[byte[0] ^ 0x5a]).unwrap();
}

#[test]
fn a_damaged_block_fails_only_its_records_and_scrub_names_them() {
    let root = TestRoot::new("block");
    create(&root.0, "block", "6");
    // Two blocks: the first holds small records, the second a 20 KiB one.
    let small = file("small.txt", b"small record", Recipe::Fixed);
    let big = file("big.txt", &vec![b'q'; 20 * 1024], Recipe::Fixed);
    let (_lane, batch) = Lane::begin("block", &[small.clone(), big.clone()]);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    // Damage the stored bytes of the small record's block, wherever the
    // batch placed it (objects are written concurrently: the large one may
    // come first).
    flip_located(
        &root.0,
        &StoreObjectId::ChunkRaw(small.chunks[0].id().clone()),
    );
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let damaged = reader.get_chunk(small.chunks[0].id()).unwrap_err();
    assert!(
        matches!(damaged, StoreError::Corruption { .. }),
        "{damaged:?}"
    );
    // The other block still reads; metadata answers stay index answers.
    let big_read = reader.get_chunk(big.chunks[0].id());
    let small_present = reader.chunk_present_metadata(small.chunks[0].id()).unwrap();
    assert!(small_present);
    drop(reader);
    let cas = NativeCas::open(&store).unwrap();
    let report = cas.scrub().unwrap();
    drop(cas);
    drop(store);
    let named: Vec<&str> = report
        .issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect();
    assert!(
        named
            .iter()
            .any(|message| message.contains(&small.manifest.file_id().to_string()[7..])),
        "{named:?}"
    );
    // Records of the undamaged block are not reported.
    if big_read.is_ok() {
        assert!(
            !named
                .iter()
                .any(|message| message.contains(&big.chunks[0].id().to_string()))
        );
    }
}

#[test]
fn damaged_metadata_or_a_digest_mismatch_is_corruption_never_absence() {
    let root = TestRoot::new("digest");
    create(&root.0, "digest", "6");
    let files = vec![file("a.txt", b"alpha", Recipe::Fixed)];
    let (lane, batch) = Lane::begin("digest", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    // Registered digest differs from the pack's metadata digest.
    sql(&root.0)
        .execute("UPDATE cas_packs SET index_digest = zeroblob(32)", [])
        .unwrap();
    let repository = NativeScmRepository::new(&root.0);
    let absent = Chunk::new(b"absent".to_vec()).unwrap();
    for result in [
        repository.chunk_present_metadata(absent.id()),
        repository.chunk_present_metadata(files[0].chunks[0].id()),
        repository.object_admitted_metadata(
            &StoreObjectId::ChunkRaw(files[0].chunks[0].id().clone()),
            lane.space.id(),
        ),
    ] {
        let error = result.unwrap_err();
        assert!(matches!(error, StoreError::Corruption { .. }), "{error:?}");
    }
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == FsckFindingCode::CasCorruption)
    );
    // The unreadable pack is reported once, before anything else about the
    // objects it holds; Tree, Step and View rows whose only copy it holds are
    // unknowable, not missing, and neither their edges nor Tree and admission
    // closure are judged on a catalog that cannot answer absence.
    let corruption: Vec<_> = report
        .findings
        .iter()
        .filter(|finding| finding.code == FsckFindingCode::CasCorruption)
        .collect();
    assert_eq!(corruption.len(), 1, "{:#?}", report.findings);
    assert_eq!(
        corruption[0].path.as_deref(),
        Some(packs(&root.0)[0].as_path())
    );
    assert!(
        !report.findings.iter().any(|finding| matches!(
            finding.code,
            FsckFindingCode::TreeClosureBroken
                | FsckFindingCode::AdmissionClosureBroken
                | FsckFindingCode::ObjectEdgeMismatch
                | FsckFindingCode::MissingCasObject
        )),
        "{:#?}",
        report.findings
    );
}

#[test]
fn a_corrupt_file_record_fails_its_chunk_but_not_its_metadata() {
    let root = TestRoot::new("record");
    create(&root.0, "record", "6");
    let files = vec![file("a.txt", b"file record bytes", Recipe::Fixed)];
    let (_lane, batch) = Lane::begin("record", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let pack = packs(&root.0).remove(0);
    flip(&pack, 16 + 5 + 1);
    let repository = NativeScmRepository::new(&root.0);
    let error = repository.get_chunk(files[0].chunks[0].id()).unwrap_err();
    assert!(matches!(error, StoreError::Corruption { .. }), "{error:?}");
    // The manifest is rebuilt from the index and verified against its id.
    assert_eq!(
        repository
            .get_file_manifest(files[0].manifest.id())
            .unwrap()
            .as_ref(),
        Some(&files[0].manifest)
    );
    assert!(
        repository
            .file_availability(files[0].manifest.file_id())
            .is_err()
    );
}

#[test]
fn format_6_stores_reopen_and_refuse_a_v2_database() {
    let root = TestRoot::new("reopen");
    create(&root.0, "reopen", "6");
    let store = open(&root.0).unwrap();
    assert_eq!(store.format_version(), 6);
    drop(store);
    let version: u32 = sql(&root.0)
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let page_size: u32 = sql(&root.0)
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap();
    assert_eq!(page_size, 1024);
    // A format 5 Store keeps its v2 database.
    let five = TestRoot::new("reopen5");
    create(&five.0, "reopen", "5");
    let version: u32 = sql(&five.0)
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    // Swapping the databases is refused both ways.
    fs::copy(
        five.0.join("metadata/layrs.sqlite3"),
        root.0.join("metadata/layrs.sqlite3"),
    )
    .unwrap();
    assert!(open(&root.0).is_err());
}

#[test]
fn a_replayed_batch_registers_nothing_new() {
    let root = TestRoot::new("replay");
    create(&root.0, "replay", "6");
    let files = vec![file("a.txt", b"alpha", Recipe::Fixed)];
    let (_lane, batch) = Lane::begin("replay", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    assert!(!repository.apply_batch(&batch).unwrap().was_replay());
    // Every object is found registered and reused: no second pack, not even
    // an orphan one.
    assert!(repository.apply_batch(&batch).unwrap().was_replay());
    assert_eq!(registered_packs(&root.0).len(), 1);
    assert_eq!(packs(&root.0).len(), 1);
    drop(repository);
    assert_fsck_clean(&root.0);
}

#[test]
fn streamed_chunks_become_file_records_and_stay_atomic() {
    let root = TestRoot::new("streamed");
    create(&root.0, "streamed", "6");
    let small = file("small.txt", b"streamed small file", Recipe::Fixed);
    let large = file("large.bin", &vec![9_u8; MIB + 5], Recipe::Single);
    let files = vec![small.clone(), large.clone()];
    let (lane, batch) = Lane::begin("streamed", &files);
    // The chunks are supplied by the callback, not carried by the batch.
    let objects: Vec<CanonicalObject> = batch
        .canonical_objects()
        .iter()
        .filter(|object| !matches!(object, CanonicalObject::ChunkRaw(_)))
        .cloned()
        .collect();
    let batch = WriteBatch::new(
        batch.transaction_id().clone(),
        objects,
        batch.entity_records().to_vec(),
        batch.admissions().to_vec(),
        batch.mutations().to_vec(),
    )
    .unwrap();
    let chunks: Vec<Chunk> = files.iter().flat_map(|file| file.chunks.clone()).collect();
    let descriptors: Vec<_> = chunks
        .iter()
        .map(|chunk| (chunk.id().clone(), chunk.byte_length()))
        .collect();
    let read = |id: &layrs_core::scm::ChunkId| {
        Ok(chunks
            .iter()
            .find(|chunk| chunk.id() == id)
            .unwrap()
            .bytes()
            .to_vec())
    };
    let mut faulty = NativeScmRepository::with_fault_injector(
        &root.0,
        Arc::new(RepositoryFault(RepositoryFaultPoint::AfterValidation)),
    );
    assert!(
        faulty
            .apply_batch_with_chunks(&batch, &descriptors, read)
            .is_err()
    );
    drop(faulty);
    assert!(registered_packs(&root.0).is_empty());
    assert_eq!(count(&root.0, "SELECT count(*) FROM loose_objects"), 0);
    let mut repository = NativeScmRepository::new(&root.0);
    repository
        .apply_batch_with_chunks(&batch, &descriptors, read)
        .unwrap();
    // The small File is one file record; the large chunk is loose.
    assert_eq!(count(&root.0, "SELECT count(*) FROM loose_objects"), 1);
    let registered = registered_packs(&root.0);
    assert_eq!(
        pack_counts(&root.0.join("objects/packs").join(&registered[0])).1,
        1
    );
    for chunk in &chunks {
        assert_eq!(
            repository.get_chunk(chunk.id()).unwrap().as_ref(),
            Some(chunk)
        );
        assert!(
            repository
                .object_admitted_metadata(
                    &StoreObjectId::ChunkRaw(chunk.id().clone()),
                    lane.space.id()
                )
                .unwrap()
        );
    }
    for file in &files {
        assert_eq!(
            repository
                .get_file_manifest(file.manifest.id())
                .unwrap()
                .as_ref(),
            Some(&file.manifest)
        );
    }
    drop(repository);
    assert_fsck_clean(&root.0);
}

/// The pack and offset of `object`'s first registered copy.
fn located(root: &Path, object: &StoreObjectId) -> (PathBuf, u64) {
    let store = open(root).unwrap();
    let location = NativeCas::open(&store)
        .unwrap()
        .locate(object)
        .unwrap()
        .unwrap();
    drop(store);
    let CasLocation::Packed { pack, offset, .. } = location else {
        panic!("{object} is not packed");
    };
    (pack, offset)
}

fn flip_located(root: &Path, object: &StoreObjectId) -> PathBuf {
    let (pack, offset) = located(root, object);
    flip(&pack, offset);
    pack
}

#[test]
fn audits_reread_pack_metadata_damaged_after_this_process_cached_it() {
    let root = TestRoot::new("stale-cache");
    create(&root.0, "stale-cache", "6");
    let files = vec![file("a.txt", b"alpha", Recipe::Fixed)];
    let (_lane, batch) = Lane::begin("stale-cache", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    // This process loads and caches the pack's metadata.
    let repository = NativeScmRepository::new(&root.0);
    assert!(
        repository
            .get_chunk(files[0].chunks[0].id())
            .unwrap()
            .is_some()
    );
    drop(repository);
    // Damage the metadata without changing the pack's length.
    let pack = packs(&root.0).remove(0);
    let length = fs::metadata(&pack).unwrap().len();
    flip(&pack, length - 72 - 3);
    assert_eq!(fs::metadata(&pack).unwrap().len(), length);
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == FsckFindingCode::CasCorruption),
        "{:#?}",
        report.findings
    );
    let store = open(&root.0).unwrap();
    let scrub = NativeCas::open(&store).unwrap().scrub().unwrap();
    assert!(!scrub.is_clean());
}

#[test]
fn fsck_reports_a_damaged_copy_that_a_sound_duplicate_hides() {
    let root = TestRoot::new("duplicate");
    create(&root.0, "duplicate", "6");
    let shared = file("shared.txt", b"bytes stored twice", Recipe::Fixed);
    let (_alpha, batch) = Lane::begin("alpha", std::slice::from_ref(&shared));
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let chunk = shared.chunks[0].clone();
    let chunk_object = StoreObjectId::ChunkRaw(chunk.id().clone());
    let damaged_pack = flip_located(&root.0, &chunk_object);
    // A later batch carrying the chunk finds its registered copy damaged
    // and writes a sound one into its own pack.
    let (beta, batch) = Lane::begin("beta", &[]);
    repository.apply_batch(&batch).unwrap();
    let alone = WriteBatch::new(
        TransactionId::new("transaction:beta-chunk").unwrap(),
        vec![CanonicalObject::ChunkRaw(chunk.clone())],
        vec![],
        vec![SpaceObjectAdmission::new(
            beta.space.id().clone(),
            chunk_object.clone(),
        )],
        vec![],
    )
    .unwrap();
    repository.apply_batch(&alone).unwrap();
    assert_eq!(
        repository.get_chunk(chunk.id()).unwrap().as_ref(),
        Some(&chunk)
    );
    drop(repository);
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(
        report.findings.iter().any(|finding| {
            finding.code == FsckFindingCode::CasCorruption
                && finding.object_id.as_deref() == Some(chunk_object.as_str())
                && finding.path.as_deref() == Some(damaged_pack.as_path())
        }),
        "{:#?}",
        report.findings
    );
}

#[test]
fn format_6_writers_refuse_unregistered_single_object_publication() {
    let root = TestRoot::new("unregistered");
    create(&root.0, "unregistered", "6");
    let exclusive = open_exclusive(&root.0).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let chunk = CanonicalObject::ChunkRaw(Chunk::new(b"lone".to_vec()).unwrap());
    assert_eq!(cas.put(&chunk), Err(CasError::UnsupportedPublication));
    assert!(matches!(
        cas.stage(&chunk),
        Err(CasError::UnsupportedPublication)
    ));
    drop(cas);
    drop(exclusive);
    assert!(packs(&root.0).is_empty());
}

/// ADR 0028, section 6: past six registered packs, the smallest are written
/// into one; every object stays readable and every Space admits exactly
/// what it admitted (compared with format 5 on the same batches, and for a
/// Space admitting only a shared chunk); retired files are removed.
#[test]
fn small_packs_are_consolidated_and_every_admission_is_kept() {
    let shared = file("shared.txt", b"the same bytes", Recipe::Fixed);
    let mut answers = Vec::new();
    for format in ["5", "6"] {
        let root = TestRoot::new(&format!("consolidation-{format}"));
        create(&root.0, "consolidation", format);
        let mut files = vec![shared.clone(), file("alt", b"x", Recipe::Single)];
        let (mut lane, batch) = Lane::begin("alpha", &files);
        let mut repository = NativeScmRepository::new(&root.0);
        repository.apply_batch(&batch).unwrap();
        // Space B admits only the shared chunk (in A's first pack).
        let (lane_b, batch) = Lane::begin("beta", &[]);
        repository.apply_batch(&batch).unwrap();
        let chunk = shared.chunks[0].clone();
        repository
            .apply_batch(
                &WriteBatch::new(
                    TransactionId::new("transaction:beta-chunk").unwrap(),
                    vec![CanonicalObject::ChunkRaw(chunk.clone())],
                    vec![],
                    vec![SpaceObjectAdmission::new(
                        lane_b.space.id().clone(),
                        StoreObjectId::ChunkRaw(chunk.id().clone()),
                    )],
                    vec![],
                )
                .unwrap(),
            )
            .unwrap();
        let b_rows = |root: &Path| -> Vec<i64> {
            let connection = sql(root);
            let mut statement = connection
                .prepare("SELECT pack_seq FROM space_pack_admissions WHERE space_id=?1")
                .unwrap();
            statement
                .query_map([lane_b.space.id().as_str()], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let b_before = if format == "6" {
            b_rows(&root.0)
        } else {
            Vec::new()
        };
        let mut most = 0;
        for step in 0..12 {
            let added = file(
                &format!("f{step}.txt"),
                format!("content {step}").as_bytes(),
                Recipe::Fixed,
            );
            let carried = file_objects(std::slice::from_ref(&added));
            files.push(added);
            let batch = lane.capture(&files, carried);
            repository.apply_batch(&batch).unwrap();
            if format == "6" {
                most = most.max(registered_packs(&root.0).len());
            }
        }
        let chunk_object = StoreObjectId::ChunkRaw(chunk.id().clone());
        let manifest_object = StoreObjectId::FileManifest(shared.manifest.id().clone());
        assert!(
            repository
                .object_admitted_metadata(&chunk_object, lane_b.space.id())
                .unwrap()
        );
        assert!(
            !repository
                .object_admitted_metadata(&manifest_object, lane_b.space.id())
                .unwrap()
        );
        drop(repository);
        if format == "6" {
            assert!(most <= 6, "consolidated along the way: {most} packs");
            assert!(registered_packs(&root.0).len() < 6);
            // Retired files are removed: on disk exactly what is registered.
            assert_eq!(packs(&root.0).len(), registered_packs(&root.0).len());
            assert_eq!(
                count(
                    &root.0,
                    "SELECT count(*) FROM space_pack_admissions \
                     WHERE pack_seq NOT IN (SELECT pack_seq FROM cas_packs)"
                ),
                0
            );
            // B's row moved to the new pack: its original pack_seq is gone,
            // from its admissions and from the catalog.
            assert_eq!(b_before.len(), 1);
            let b_after = b_rows(&root.0);
            assert_eq!(b_after.len(), 1);
            assert_ne!(b_after, b_before);
            assert_eq!(
                count(
                    &root.0,
                    &format!(
                        "SELECT count(*) FROM cas_packs WHERE pack_seq={}",
                        b_before[0]
                    )
                ),
                0
            );
        }
        let mut answer = reader_answers(&root.0, &files, lane.space.id());
        answer.extend(reader_answers(
            &root.0,
            std::slice::from_ref(&shared),
            lane_b.space.id(),
        ));
        answers.push(answer);
        assert_fsck_clean(&root.0);
    }
    assert_eq!(answers[0], answers[1]);
}

/// A malformed admission bitmap of another Space is corruption: a
/// consolidation never rewrites it into a narrower answer (council s6 r1).
#[test]
fn a_malformed_bitmap_of_another_space_is_never_consolidated_away() {
    let root = TestRoot::new("consolidation-malformed");
    create(&root.0, "consolidation-malformed", "6");
    let shared = file("shared.txt", b"the same bytes", Recipe::Fixed);
    let mut files = vec![shared.clone()];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let (lane_b, batch) = Lane::begin("beta", &[]);
    repository.apply_batch(&batch).unwrap();
    let chunk = shared.chunks[0].clone();
    repository
        .apply_batch(
            &WriteBatch::new(
                TransactionId::new("transaction:beta-chunk").unwrap(),
                vec![CanonicalObject::ChunkRaw(chunk.clone())],
                vec![],
                vec![SpaceObjectAdmission::new(
                    lane_b.space.id().clone(),
                    StoreObjectId::ChunkRaw(chunk.id().clone()),
                )],
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
    drop(repository);
    // B's bitmap loses its last byte (written directly, as damage would).
    let (seq, bitmap): (i64, Vec<u8>) = sql(&root.0)
        .query_row(
            "SELECT pack_seq, bitmap FROM space_pack_admissions WHERE space_id=?1",
            [lane_b.space.id().as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let damaged = if bitmap.len() > 1 {
        bitmap[..bitmap.len() - 1].to_vec()
    } else {
        vec![bitmap[0], 0, 0]
    };
    sql(&root.0)
        .execute(
            "UPDATE space_pack_admissions SET bitmap=?1 WHERE space_id=?2 AND pack_seq=?3",
            rusqlite::params![damaged, lane_b.space.id().as_str(), seq],
        )
        .unwrap();
    let mut repository = NativeScmRepository::new(&root.0);
    for step in 0..10 {
        let added = file(
            &format!("f{step}.txt"),
            format!("content {step}").as_bytes(),
            Recipe::Fixed,
        );
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        let batch = lane.capture(&files, carried);
        repository.apply_batch(&batch).unwrap();
    }
    // B's damaged row is still there, unchanged, and still answers as
    // corruption; A's files all read.
    let still: Vec<u8> = sql(&root.0)
        .query_row(
            "SELECT bitmap FROM space_pack_admissions WHERE space_id=?1 AND pack_seq=?2",
            rusqlite::params![lane_b.space.id().as_str(), seq],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still, damaged);
    // The other packs still consolidated around it (council s6 r2), and
    // later sessions leave it out without trying it again.
    assert!(registered_packs(&root.0).len() < 6);
    let skipped = fs::read_to_string(root.0.join("metadata/consolidation-skipped")).unwrap();
    let damaged_pack: String = sql(&root.0)
        .query_row(
            "SELECT name FROM cas_packs WHERE pack_seq=?1",
            [seq],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        skipped.lines().collect::<Vec<_>>(),
        vec![damaged_pack.as_str()]
    );
    assert_eq!(packs(&root.0).len(), registered_packs(&root.0).len());
    assert!(
        repository
            .object_admitted_metadata(
                &StoreObjectId::ChunkRaw(chunk.id().clone()),
                lane_b.space.id()
            )
            .is_err()
    );
    drop(repository);
    let answers = reader_answers(&root.0, &files, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false"))
    );
}

/// A candidate whose block is damaged is left out (named in the skip file,
/// still registered, its rows untouched); the sound packs consolidate
/// around it (council s6 r2).
#[test]
fn a_damaged_candidate_is_left_out_and_the_others_consolidate() {
    let root = TestRoot::new("consolidation-damaged");
    create(&root.0, "consolidation-damaged", "6");
    let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let next = |files: &mut Vec<File>, lane: &mut Lane| {
        let step = files.len();
        let added = file(
            &format!("f{step}.txt"),
            format!("content {step}").as_bytes(),
            Recipe::Fixed,
        );
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        lane.capture(files, carried)
    };
    for _ in 0..4 {
        let batch = next(&mut files, &mut lane);
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
    }
    assert_eq!(registered_packs(&root.0).len(), 5);
    let damaged = files[2].clone();
    let damaged_pack = flip_located(
        &root.0,
        &StoreObjectId::ChunkRaw(damaged.chunks[0].id().clone()),
    );
    let damaged_name = damaged_pack
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let damaged_seq: i64 = sql(&root.0)
        .query_row(
            "SELECT pack_seq FROM cas_packs WHERE name=?1",
            [&damaged_name],
            |row| row.get(0),
        )
        .unwrap();
    let rows_before = count(
        &root.0,
        &format!("SELECT count(*) FROM space_pack_admissions WHERE pack_seq={damaged_seq}"),
    );
    // A link planted at the skip file is replaced, never written through
    // (council s6 r3).
    let victim = root.0.join("victim.txt");
    fs::write(&victim, b"victim bytes").unwrap();
    fs::hard_link(&victim, root.0.join("metadata/consolidation-skipped")).unwrap();
    for _ in 0..3 {
        let batch = next(&mut files, &mut lane);
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
    }
    let registered = registered_packs(&root.0);
    assert!(registered.len() < 6, "{registered:?}");
    assert!(registered.contains(&damaged_name));
    assert_eq!(packs(&root.0).len(), registered.len());
    assert!(packs(&root.0).contains(&damaged_pack));
    assert_eq!(
        count(
            &root.0,
            &format!("SELECT count(*) FROM space_pack_admissions WHERE pack_seq={damaged_seq}"),
        ),
        rows_before
    );
    let skipped = fs::read_to_string(root.0.join("metadata/consolidation-skipped")).unwrap();
    assert_eq!(
        skipped.lines().collect::<Vec<_>>(),
        vec![damaged_name.as_str()]
    );
    assert_eq!(fs::read(&victim).unwrap(), b"victim bytes");
    // Every other file reads and stays admitted; the damaged chunk is
    // corruption, never absence.
    let sound: Vec<File> = files
        .iter()
        .filter(|file| file.name != damaged.name)
        .cloned()
        .collect();
    let answers = reader_answers(&root.0, &sound, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false") && !answer.contains("read=false")),
        "{answers:?}"
    );
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(reader.get_chunk(damaged.chunks[0].id()).is_err());
}

/// A record two candidates hold is written once, and explicit manifests (a
/// multi-chunk File) keep their FileId entry through consolidation (council
/// s6 r2): every answer of both Spaces is unchanged.
#[test]
fn a_record_in_two_candidates_and_explicit_manifests_survive_consolidation() {
    let root = TestRoot::new("consolidation-dedup");
    create(&root.0, "consolidation-dedup", "6");
    let big: Vec<u8> = (0..MIB + MIB / 4).map(|i| (i % 251) as u8).collect();
    let shared = vec![
        file("big.bin", &big, Recipe::Half),
        file("shared.txt", b"the same bytes", Recipe::Fixed),
    ];
    assert!(shared[0].chunks.len() > 1);
    let (mut alpha, batch) = Lane::begin("alpha", &shared);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    // The block holding the Tree is damaged; Space B's batch, carrying the
    // same Tree, writes a sound copy into its own pack; then the damage is
    // undone: two sound copies in two registered packs.
    let tree = StoreObjectId::Tree(tree_of(&shared).id().clone());
    let (pack, offset) = located(&root.0, &tree);
    flip(&pack, offset);
    let (beta, batch) = Lane::begin("beta", &shared);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    // The Tree reads while the first copy is damaged: the second exists.
    let tree_id = tree_of(&shared).id().clone();
    let store = open(&root.0).unwrap();
    assert!(
        NativeScmReader::new(&store)
            .unwrap()
            .get_tree(&tree_id)
            .unwrap()
            .is_some()
    );
    drop(store);
    flip(&pack, offset);
    let first: Vec<i64> = {
        let connection = sql(&root.0);
        let mut statement = connection
            .prepare("SELECT pack_seq FROM cas_packs ORDER BY pack_seq")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(first.len(), 2);
    assert_fsck_clean(&root.0);
    let before = (
        reader_answers(&root.0, &shared, alpha.space.id()),
        reader_answers(&root.0, &shared, beta.space.id()),
    );
    let mut files = shared.clone();
    for step in 0..5 {
        let added = file(
            &format!("f{step}.txt"),
            format!("content {step}").as_bytes(),
            Recipe::Fixed,
        );
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        let batch = alpha.capture(&files, carried);
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
    }
    let registered = registered_packs(&root.0);
    assert!(registered.len() < 6, "{registered:?}");
    assert_eq!(packs(&root.0).len(), registered.len());
    for seq in &first {
        assert_eq!(
            count(
                &root.0,
                &format!("SELECT count(*) FROM cas_packs WHERE pack_seq={seq}")
            ),
            0,
            "pack {seq} was not consolidated"
        );
    }
    assert_eq!(
        (
            reader_answers(&root.0, &shared, alpha.space.id()),
            reader_answers(&root.0, &shared, beta.space.id()),
        ),
        before
    );
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    for space in [alpha.space.id(), beta.space.id()] {
        assert!(reader.object_admitted_metadata(&tree, space).unwrap());
    }
    drop(reader);
    drop(store);
    assert_fsck_clean(&root.0);
}

/// Incompressible bytes (a fixed linear congruential sequence).
fn noise(length: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    (0..length)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}

fn inline_rows(root: &Path) -> i64 {
    count(root, "SELECT count(*) FROM cas_pack_inline")
}

/// Small batches register their pack inline (council s7 Q1): no pack file,
/// the image committed with the registration; a batch past the limit
/// spills to a pack file. Every answer equals format 5's.
#[test]
fn small_batches_register_inline_packs_that_answer_like_files() {
    let big = file("big.bin", &noise(200 * 1024, 7), Recipe::Fixed);
    let mut answers = Vec::new();
    for format in ["5", "6"] {
        let root = TestRoot::new(&format!("inline-{format}"));
        create(&root.0, "inline", format);
        set_inline_pack_limit(&root.0, None);
        let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
        let (mut lane, batch) = Lane::begin("alpha", &files);
        let mut repository = NativeScmRepository::new(&root.0);
        repository.apply_batch(&batch).unwrap();
        for step in 0..3 {
            let added = file(
                &format!("f{step}.txt"),
                format!("content {step}").as_bytes(),
                Recipe::Fixed,
            );
            let carried = file_objects(std::slice::from_ref(&added));
            files.push(added);
            repository
                .apply_batch(&lane.capture(&files, carried))
                .unwrap();
        }
        if format == "6" {
            // Four small batches: four inline packs, no file.
            assert_eq!(registered_packs(&root.0).len(), 4);
            assert_eq!(inline_rows(&root.0), 4);
            assert!(packs(&root.0).is_empty());
        }
        // A batch past the limit is a pack file.
        let carried = file_objects(std::slice::from_ref(&big));
        files.push(big.clone());
        repository
            .apply_batch(&lane.capture(&files, carried))
            .unwrap();
        drop(repository);
        if format == "6" {
            assert_eq!(registered_packs(&root.0).len(), 5);
            assert_eq!(inline_rows(&root.0), 4);
            assert_eq!(packs(&root.0).len(), 1);
        }
        answers.push(reader_answers(&root.0, &files, lane.space.id()));
        assert_fsck_clean(&root.0);
    }
    assert_eq!(answers[0], answers[1]);
}

/// A damaged block of an inline pack fails its records as corruption,
/// never absence, and fsck names the inline pack.
#[test]
fn a_damaged_inline_block_is_corruption_never_absence() {
    let root = TestRoot::new("inline-damage");
    create(&root.0, "inline-damage", "6");
    set_inline_pack_limit(&root.0, None);
    let files = vec![file("a.txt", b"alpha bytes", Recipe::Fixed)];
    let (lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let chunk = StoreObjectId::ChunkRaw(files[0].chunks[0].id().clone());
    let store = open(&root.0).unwrap();
    let location = NativeCas::open(&store)
        .unwrap()
        .locate(&chunk)
        .unwrap()
        .unwrap();
    drop(store);
    assert!(
        matches!(location, CasLocation::Inline { .. }),
        "{location:?}"
    );
    location.flip_stored_byte().unwrap();
    let repository = NativeScmRepository::new(&root.0);
    assert!(repository.get_chunk(files[0].chunks[0].id()).is_err());
    // Metadata answers (the index is intact) still hold.
    assert!(
        repository
            .object_admitted_metadata(&chunk, lane.space.id())
            .unwrap()
    );
    drop(repository);
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(
        report.findings.iter().any(|finding| {
            finding.code == FsckFindingCode::CasCorruption
                && finding.path.as_deref() == Some(location.path())
        }),
        "{:#?}",
        report.findings
    );
}

/// Inline packs consolidate into an inline pack (no file is ever written),
/// keep every admission, and a sequence number is never reused.
#[test]
fn inline_packs_consolidate_inline() {
    let root = TestRoot::new("inline-consolidation");
    create(&root.0, "inline-consolidation", "6");
    set_inline_pack_limit(&root.0, None);
    let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let mut highest = 0_i64;
    for step in 0..10 {
        let added = file(
            &format!("f{step}.txt"),
            format!("content {step}").as_bytes(),
            Recipe::Fixed,
        );
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        repository
            .apply_batch(&lane.capture(&files, carried))
            .unwrap();
        let seq: i64 = count(&root.0, "SELECT max(pack_seq) FROM cas_packs");
        assert!(seq > highest, "pack_seq {seq} after {highest}");
        highest = seq;
    }
    drop(repository);
    assert!(registered_packs(&root.0).len() < 6);
    assert_eq!(inline_rows(&root.0), registered_packs(&root.0).len() as i64);
    assert!(packs(&root.0).is_empty());
    let answers = reader_answers(&root.0, &files, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false"))
    );
    assert_fsck_clean(&root.0);
}

/// Fails every pack publication after the first `allowed`.
struct PublicationsAfter {
    allowed: usize,
    seen: AtomicU64,
}
impl CasFaultInjector for PublicationsAfter {
    fn check(&self, point: CasFaultPoint) -> Result<(), String> {
        if point == CasFaultPoint::AfterPackPublication
            && self.seen.fetch_add(1, Ordering::SeqCst) >= self.allowed as u64
        {
            Err("consolidation test".into())
        } else {
            Ok(())
        }
    }
}

/// A general failure is retried by the next batch; after two in a row,
/// consolidation pauses (as git pauses its automatic gc after a failure),
/// and the batches themselves keep succeeding (council s6 r3).
#[test]
fn repeated_general_failures_pause_consolidation() {
    let root = TestRoot::new("consolidation-pause");
    create(&root.0, "consolidation-pause", "6");
    let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let next = |files: &mut Vec<File>, lane: &mut Lane| {
        let step = files.len();
        let added = file(
            &format!("f{step}.txt"),
            format!("content {step}").as_bytes(),
            Recipe::Fixed,
        );
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        lane.capture(files, carried)
    };
    for _ in 0..4 {
        let batch = next(&mut files, &mut lane);
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
    }
    let note = root.0.join("metadata/consolidation-failed");
    // Each batch publishes its own pack, then its consolidation fails.
    let failing = || {
        NativeScmRepository::with_cas_fault_injector(
            &root.0,
            Arc::new(PublicationsAfter {
                allowed: 1,
                seen: AtomicU64::new(0),
            }),
        )
    };
    for failures in 1..=2 {
        let batch = next(&mut files, &mut lane);
        failing().apply_batch(&batch).unwrap();
        let text = fs::read_to_string(&note).unwrap();
        assert!(text.starts_with(&format!("{failures} ")), "{text}");
    }
    let registered = registered_packs(&root.0).len();
    assert!(registered >= 6);
    // Paused: a sound batch does not consolidate.
    let batch = next(&mut files, &mut lane);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    assert_eq!(registered_packs(&root.0).len(), registered + 1);
    // Once the pause is over (the note removed here), it consolidates and
    // forgets the failures.
    fs::remove_file(&note).unwrap();
    let batch = next(&mut files, &mut lane);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    assert!(registered_packs(&root.0).len() < 6);
    assert!(!note.exists());
    let answers = reader_answers(&root.0, &files, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false"))
    );
    assert_fsck_clean(&root.0);
}

/// Fails the `at`-th pack publication.
struct NthPublication {
    at: usize,
    seen: AtomicU64,
}
impl CasFaultInjector for NthPublication {
    fn check(&self, point: CasFaultPoint) -> Result<(), String> {
        if point == CasFaultPoint::AfterPackPublication
            && self.seen.fetch_add(1, Ordering::SeqCst) + 1 == self.at as u64
        {
            Err("consolidation test".into())
        } else {
            Ok(())
        }
    }
}

/// A consolidation stopped after publishing its pack, or after its
/// registration commit, leaves a consistent Store: everything reads, and
/// the next exclusive session removes the files nothing registers.
#[test]
fn a_stopped_consolidation_leaves_a_consistent_store() {
    for stop in ["after publication", "after commit"] {
        let root = TestRoot::new("consolidation-stop");
        create(&root.0, "consolidation-stop", "6");
        let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
        let (mut lane, batch) = Lane::begin("alpha", &files);
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
        let next = |files: &mut Vec<File>, lane: &mut Lane| {
            let step = files.len();
            let added = file(
                &format!("f{step}.txt"),
                format!("content {step}").as_bytes(),
                Recipe::Fixed,
            );
            let carried = file_objects(std::slice::from_ref(&added));
            files.push(added);
            lane.capture(files, carried)
        };
        for _ in 0..4 {
            let batch = next(&mut files, &mut lane);
            NativeScmRepository::new(&root.0)
                .apply_batch(&batch)
                .unwrap();
        }
        assert_eq!(registered_packs(&root.0).len(), 5);
        // The sixth batch registers its pack, then consolidates and stops.
        let batch = next(&mut files, &mut lane);
        let mut faulty = match stop {
            "after publication" => NativeScmRepository::with_cas_fault_injector(
                &root.0,
                Arc::new(NthPublication {
                    at: 2,
                    seen: AtomicU64::new(0),
                }),
            ),
            _ => NativeScmRepository::with_fault_injector(
                &root.0,
                Arc::new(RepositoryFault(
                    RepositoryFaultPoint::AfterConsolidationCommit,
                )),
            ),
        };
        // Best effort: the batch itself succeeded.
        faulty.apply_batch(&batch).unwrap();
        drop(faulty);
        let registered = registered_packs(&root.0).len();
        let on_disk = packs(&root.0).len();
        match stop {
            // The consolidated pack is published but never registered.
            "after publication" => assert_eq!((registered, on_disk), (6, 7), "{stop}"),
            // Registered, the retired files not yet removed.
            _ => assert!(registered < 6 && on_disk > registered, "{stop}"),
        }
        let answers = reader_answers(&root.0, &files, lane.space.id());
        assert!(
            answers
                .iter()
                .all(|answer| !answer.contains("admitted=false")),
            "{stop}: {answers:?}"
        );
        // The next session removes the unregistered files (and consolidates
        // if still due).
        let batch = next(&mut files, &mut lane);
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
        assert!(registered_packs(&root.0).len() < 6, "{stop}");
        assert_eq!(
            packs(&root.0).len(),
            registered_packs(&root.0).len(),
            "{stop}"
        );
        let answers = reader_answers(&root.0, &files, lane.space.id());
        assert!(
            answers
                .iter()
                .all(|answer| !answer.contains("admitted=false"))
        );
        assert_fsck_clean(&root.0);
    }
}
