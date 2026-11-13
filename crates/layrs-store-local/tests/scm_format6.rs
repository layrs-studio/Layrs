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
    MaintenanceOutcome, NativeCas, NativeScmReader, NativeScmRepository, RepositoryFaultInjector,
    RepositoryFaultPoint, existence_proofs_on_this_thread, forget_loaded_packs, fsck_existing,
    held_pack_files, init, maintain, maintain_with_pause, maintenance_paused, open, open_exclusive,
    set_consolidation_budget, set_inline_pack_limit, take_maintenance_due,
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
    /// One chunk named by the FileId (Store format 7, council s13 Q5).
    Whole,
}

/// One File: its chunks (by its recipe) and manifest.
#[derive(Clone)]
struct File {
    name: String,
    chunks: Vec<Chunk>,
    manifest: FileManifest,
}

fn file(name: &str, bytes: &[u8], recipe: Recipe) -> File {
    if let Recipe::Whole = recipe {
        let chunks = vec![Chunk::whole(bytes.to_vec()).unwrap()];
        let manifest = FileManifest::from_chunks(ChunkingDescriptor::whole(), &chunks).unwrap();
        return File {
            name: name.into(),
            chunks,
            manifest,
        };
    }
    let (chunking, size) = match recipe {
        Recipe::Whole => unreachable!("handled above"),
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

/// The pack version of a pack file (its header).
fn pack_version(path: &Path) -> u32 {
    let bytes = fs::read(path).unwrap();
    u32::from_le_bytes(bytes[8..12].try_into().unwrap())
}

fn many(count: usize, recipe: Recipe) -> Vec<File> {
    (0..count)
        .map(|i| {
            file(
                &format!("f{i}.txt"),
                format!("line {i}\n{}", "\n".repeat(i % 40)).as_bytes(),
                recipe,
            )
        })
        .collect()
}

/// Format 7 keeps a small file as one `whole` record named by its FileId
/// (council s13 Q5): a version 2 pack without a second digest, smaller than
/// format 6's for the same content, read, admitted, closed and checked as
/// format 6 answers.
#[test]
fn format_7_keeps_small_files_whole_in_smaller_packs() {
    let mut sizes = Vec::new();
    for (format, recipe) in [("6", Recipe::Fixed), ("7", Recipe::Whole)] {
        let root = TestRoot::new(&format!("whole-{format}"));
        create(&root.0, &format!("whole-{format}"), format);
        let mut files = many(200, recipe);
        // A file over 1 MiB keeps its chunks in both formats; a file of
        // format 7 may still be v1 (received, or captured before).
        let large: Vec<u8> = (0..(MIB + 300)).map(|i| (i * 7 % 253) as u8).collect();
        files.push(file("large.bin", &large, Recipe::Fixed));
        files.push(file("v1.txt", b"a v1 file in any format", Recipe::Fixed));
        let (lane, batch) = Lane::begin("alpha", &files);
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .unwrap();
        let [pack] = packs(&root.0).try_into().unwrap();
        assert_eq!(pack_version(&pack), if format == "7" { 2 } else { 1 });
        sizes.push(fs::metadata(&pack).unwrap().len());
        let store = open(&root.0).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        for file in &files {
            let id = file.manifest.file_id();
            assert!(reader.file_present_metadata(id).unwrap(), "{}", file.name);
            assert_eq!(
                reader.file_manifest_ids_metadata(id).unwrap(),
                vec![file.manifest.id().clone()]
            );
            for chunk in &file.chunks {
                let read = reader.get_chunk(chunk.id()).unwrap().unwrap();
                assert_eq!(&read, chunk);
                assert_eq!(read.is_whole(), format == "7" && file.name.starts_with('f'));
            }
            assert_eq!(
                reader.get_file_manifest(file.manifest.id()).unwrap(),
                Some(file.manifest.clone())
            );
        }
        // The v1 chunk of a whole file's bytes is not stored.
        if format == "7" {
            let v1 = Chunk::new(files[0].chunks[0].bytes().to_vec()).unwrap();
            assert!(reader.get_chunk(v1.id()).unwrap().is_none());
        }
        drop(reader);
        drop(store);
        assert_closure_matches(&root.0, &files, lane.space.id());
        assert_fsck_clean(&root.0);
        let store = open(&root.0).unwrap();
        assert!(NativeCas::open(&store).unwrap().scrub().unwrap().is_clean());
    }
    // 200 whole records: at least 32 bytes each saved.
    assert!(sizes[1] + 200 * 32 <= sizes[0], "{sizes:?}");
}

/// Consolidation copies whole records as they are: every answer and
/// admission is kept.
#[test]
fn whole_records_survive_consolidation() {
    let root = TestRoot::new("whole-consolidation");
    create(&root.0, "whole-consolidation", "7");
    let mut files = vec![file("first.txt", b"first", Recipe::Whole)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    for step in 0..7 {
        let recipe = if step % 2 == 0 {
            Recipe::Whole
        } else {
            Recipe::Fixed
        };
        let added = file(&format!("f{step}.txt"), &noise(300, step), recipe);
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        repository
            .apply_batch(&lane.capture(&files, carried))
            .unwrap();
    }
    drop(repository);
    assert!(registered_packs(&root.0).len() < 8);
    for pack in packs(&root.0) {
        assert_eq!(pack_version(&pack), 2);
    }
    let answers = reader_answers(&root.0, &files, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false")),
        "{answers:?}"
    );
    assert_closure_matches(&root.0, &files, lane.space.id());
    assert_fsck_clean(&root.0);
}

/// A whole file and the v1 file of the same bytes in one batch: the whole
/// manifest takes the file record, the v1 manifest and chunk stay explicit;
/// both manifests answer.
#[test]
fn a_whole_and_a_v1_manifest_of_one_file_share_a_batch() {
    let root = TestRoot::new("whole-and-v1");
    create(&root.0, "whole-and-v1", "7");
    let whole = file("same.txt", b"the same bytes", Recipe::Whole);
    let v1 = file("same.txt", b"the same bytes", Recipe::Fixed);
    assert_eq!(whole.manifest.file_id(), v1.manifest.file_id());
    let (lane, batch) = Lane::begin("alpha", std::slice::from_ref(&whole));
    let mut objects = batch.canonical_objects().to_vec();
    objects.extend(file_objects(std::slice::from_ref(&v1)));
    let mut admissions = batch.admissions().to_vec();
    admissions.push(SpaceObjectAdmission::new(
        lane.space.id().clone(),
        StoreObjectId::FileManifest(v1.manifest.id().clone()),
    ));
    admissions.push(SpaceObjectAdmission::new(
        lane.space.id().clone(),
        StoreObjectId::ChunkRaw(v1.chunks[0].id().clone()),
    ));
    let batch = WriteBatch::new(
        batch.transaction_id().clone(),
        objects,
        batch.entity_records().to_vec(),
        admissions,
        batch.mutations().to_vec(),
    )
    .unwrap();
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let mut expected = vec![whole.manifest.id().clone(), v1.manifest.id().clone()];
    expected.sort();
    assert_eq!(
        reader
            .file_manifest_ids_metadata(whole.manifest.file_id())
            .unwrap(),
        expected
    );
    assert_eq!(
        reader.get_chunk(v1.chunks[0].id()).unwrap().as_ref(),
        Some(&v1.chunks[0])
    );
    assert_eq!(
        reader.get_chunk(whole.chunks[0].id()).unwrap().as_ref(),
        Some(&whole.chunks[0])
    );
    drop(reader);
    drop(store);
    assert_fsck_clean(&root.0);
}

/// A whole file belongs only in a Store of format 7, and a format 7 pack
/// is never read by a Store that is not.
#[test]
fn whole_files_and_their_packs_stay_in_format_7() {
    // A format 6 Store refuses a batch carrying a whole file.
    let root = TestRoot::new("whole-in-6");
    create(&root.0, "whole-in-6", "6");
    let files = vec![file("a.txt", b"alpha", Recipe::Whole)];
    let (_, batch) = Lane::begin("alpha", &files);
    assert!(
        NativeScmRepository::new(&root.0)
            .apply_batch(&batch)
            .is_err()
    );
    assert!(registered_packs(&root.0).is_empty());
    // A format 7 Store whose marker is rewritten to format 6 reads its
    // version 2 pack as corruption, never as format 6 content.
    let root = TestRoot::new("whole-marker");
    create(&root.0, "whole-marker", "7");
    let (_, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let marker = root.0.join("FORMAT");
    let text = fs::read_to_string(&marker).unwrap();
    fs::write(
        &marker,
        text.replace("\"formatVersion\":7", "\"formatVersion\":6")
            .replace("\"minReaderVersion\":7", "\"minReaderVersion\":6"),
    )
    .unwrap();
    let store = open(&root.0).unwrap();
    assert_eq!(store.format_version(), 6);
    let reader = NativeScmReader::new(&store).unwrap();
    // This process cached the pack while it wrote it.
    assert!(reader.get_chunk(files[0].chunks[0].id()).is_err());
    drop(reader);
    drop(store);
    // A copy elsewhere: the pack is loaded cold, from its file.
    let cold = TestRoot::new("whole-marker-cold");
    copy_tree(&root.0, &cold.0);
    let store = open(&cold.0).unwrap();
    assert_eq!(store.format_version(), 6);
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(reader.get_chunk(files[0].chunks[0].id()).is_err());
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A File kept whole in one pack and as a v1 file record in another (its
/// v1 representation received after, or captured before): consolidation
/// keeps both identities, in either order (council s16 r1).
#[test]
fn a_whole_and_a_v1_record_of_one_file_consolidate_in_either_order() {
    for whole_first in [true, false] {
        let label = format!("mixed-{whole_first}");
        let root = TestRoot::new(&label);
        create(&root.0, &label, "7");
        let content = b"the same bytes, twice represented";
        let whole = file("whole.txt", content, Recipe::Whole);
        let v1 = file("v1.txt", content, Recipe::Fixed);
        let (first, second) = if whole_first {
            (whole.clone(), v1.clone())
        } else {
            (v1.clone(), whole.clone())
        };
        let mut files = vec![first];
        let (mut lane, batch) = Lane::begin("alpha", &files);
        let mut repository = NativeScmRepository::new(&root.0);
        repository.apply_batch(&batch).unwrap();
        let carried = file_objects(std::slice::from_ref(&second));
        files.push(second);
        repository
            .apply_batch(&lane.capture(&files, carried))
            .unwrap();
        // The pack holding the whole record and the one holding the v1 record.
        let mixed = registered_packs(&root.0);
        assert_eq!(mixed.len(), 2, "{label}");
        for step in 0..6 {
            let added = file(&format!("f{step}.bin"), &noise(300, step), Recipe::Fixed);
            let carried = file_objects(std::slice::from_ref(&added));
            files.push(added);
            repository
                .apply_batch(&lane.capture(&files, carried))
                .unwrap();
        }
        drop(repository);
        // Both were consolidated, neither left out (council s16 r2).
        let registered = registered_packs(&root.0);
        assert!(
            mixed.iter().all(|pack| !registered.contains(pack)),
            "{label}: {registered:?}"
        );
        assert!(
            !root.0.join("metadata/consolidation-skipped").exists(),
            "{label}"
        );
        let store = open(&root.0).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let mut both = vec![whole.manifest.id().clone(), v1.manifest.id().clone()];
        both.sort();
        assert_eq!(
            reader
                .file_manifest_ids_metadata(whole.manifest.file_id())
                .unwrap(),
            both
        );
        assert_eq!(
            reader.get_chunk(whole.chunks[0].id()).unwrap().as_ref(),
            Some(&whole.chunks[0])
        );
        assert_eq!(
            reader.get_chunk(v1.chunks[0].id()).unwrap().as_ref(),
            Some(&v1.chunks[0])
        );
        drop(reader);
        drop(store);
        let answers = reader_answers(&root.0, &files, lane.space.id());
        assert!(
            answers
                .iter()
                .all(|answer| !answer.contains("admitted=false")),
            "{answers:?}"
        );
        assert_closure_matches(&root.0, &files, lane.space.id());
        assert_fsck_clean(&root.0);
    }
}

/// A whole chunk is stored only as its file's record, even when the batch
/// does not plan one: a chunk carried without its manifest, or a sound copy
/// of a record whose block is damaged (council s16 r1).
#[test]
fn a_whole_chunk_alone_or_replacing_a_damaged_record_gets_its_record() {
    let root = TestRoot::new("whole-alone");
    create(&root.0, "whole-alone", "7");
    let damaged = file(
        "damaged.txt",
        b"bytes whose block is damaged",
        Recipe::Whole,
    );
    let files = vec![damaged.clone()];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    drop(repository);
    flip_located(
        &root.0,
        &StoreObjectId::ChunkRaw(damaged.chunks[0].id().clone()),
    );
    // The sound bytes again: a new record, read before the damaged one.
    let mut repository = NativeScmRepository::new(&root.0);
    repository
        .apply_batch(&lane.capture(&files, file_objects(&files)))
        .unwrap();
    // A whole chunk without its manifest, admitted to another Space.
    let alone = Chunk::whole(b"a whole chunk carried alone".to_vec()).unwrap();
    let (lane_b, batch) = Lane::begin("beta", &[]);
    repository.apply_batch(&batch).unwrap();
    repository
        .apply_batch(
            &WriteBatch::new(
                TransactionId::new("transaction:whole-alone").unwrap(),
                vec![CanonicalObject::ChunkRaw(alone.clone())],
                vec![],
                vec![SpaceObjectAdmission::new(
                    lane_b.space.id().clone(),
                    StoreObjectId::ChunkRaw(alone.id().clone()),
                )],
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
    drop(repository);
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader.get_chunk(damaged.chunks[0].id()).unwrap().as_ref(),
        Some(&damaged.chunks[0])
    );
    assert_eq!(reader.get_chunk(alone.id()).unwrap().as_ref(), Some(&alone));
    let alone_file = FileId::from_bytes(alone.bytes());
    assert_eq!(
        reader.file_manifest_ids_metadata(&alone_file).unwrap(),
        vec![
            FileManifest::whole(alone_file.clone(), alone.byte_length())
                .unwrap()
                .id()
                .clone()
        ]
    );
}

/// A v1 manifest naming a whole chunk (the same bytes, its file-domain
/// id) is refused: a chunk is checked in the domain its manifest's recipe
/// declares, not in the one its stored copy has (council s16 r1).
#[test]
fn a_v1_manifest_naming_a_whole_chunk_is_refused() {
    let root = TestRoot::new("v1-names-whole");
    create(&root.0, "v1-names-whole", "7");
    let whole = file("a.txt", b"alpha bytes", Recipe::Whole);
    let mut files = vec![whole.clone()];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let wrong = FileManifest::new(
        whole.manifest.file_id().clone(),
        whole.manifest.byte_length(),
        ChunkingDescriptor::single(),
        whole.manifest.chunks().to_vec(),
    )
    .unwrap();
    let forged = File {
        name: "b.txt".into(),
        chunks: whole.chunks.clone(),
        manifest: wrong.clone(),
    };
    files.push(forged.clone());
    let batch = lane.capture(&files, vec![CanonicalObject::FileManifest(wrong.clone())]);
    let refused = repository.apply_batch(&batch).unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("admission rejected for FileManifest"),
        "{refused}"
    );
    drop(repository);
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(reader.get_file_manifest(wrong.id()).unwrap().is_none());
}

/// A batch carrying a whole chunk without its manifest and the v1 form of
/// the same bytes: the whole chunk keeps the file record, the v1 chunk and
/// manifest stay explicit (council s16 r2).
#[test]
fn a_whole_chunk_alone_and_the_v1_form_of_its_file_share_a_batch() {
    let root = TestRoot::new("whole-alone-v1");
    create(&root.0, "whole-alone-v1", "7");
    let v1 = file("v1.txt", b"bytes in two forms at once", Recipe::Fixed);
    let whole = Chunk::whole(b"bytes in two forms at once".to_vec()).unwrap();
    let (lane, batch) = Lane::begin("alpha", std::slice::from_ref(&v1));
    let mut objects = batch.canonical_objects().to_vec();
    objects.push(CanonicalObject::ChunkRaw(whole.clone()));
    let mut admissions = batch.admissions().to_vec();
    admissions.push(SpaceObjectAdmission::new(
        lane.space.id().clone(),
        StoreObjectId::ChunkRaw(whole.id().clone()),
    ));
    let batch = WriteBatch::new(
        batch.transaction_id().clone(),
        objects,
        batch.entity_records().to_vec(),
        admissions,
        batch.mutations().to_vec(),
    )
    .unwrap();
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(reader.get_chunk(whole.id()).unwrap().as_ref(), Some(&whole));
    assert_eq!(
        reader.get_chunk(v1.chunks[0].id()).unwrap().as_ref(),
        Some(&v1.chunks[0])
    );
    assert_eq!(
        reader.get_file_manifest(v1.manifest.id()).unwrap().as_ref(),
        Some(&v1.manifest)
    );
    drop(reader);
    drop(store);
    assert_fsck_clean(&root.0);
}

/// An external whole chunk no manifest of its batch names takes the domain
/// of its registered record, and its File's record with it: a sound copy
/// of a damaged whole record shares a batch with the v1 form of the same
/// bytes (council s16 r3).
#[test]
fn an_external_whole_chunk_and_the_v1_form_of_its_file_share_a_batch() {
    let root = TestRoot::new("whole-external-v1");
    create(&root.0, "whole-external-v1", "7");
    let bytes = b"bytes whose whole record is damaged";
    let whole = file("whole.txt", bytes, Recipe::Whole);
    let (_lane, batch) = Lane::begin("alpha", std::slice::from_ref(&whole));
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let sound = whole.chunks[0].clone();
    flip_located(&root.0, &StoreObjectId::ChunkRaw(sound.id().clone()));
    // Another Space receives the v1 form, and the whole chunk from outside
    // the batch.
    let v1 = file("v1.txt", bytes, Recipe::Fixed);
    let (lane_b, batch) = Lane::begin("beta", std::slice::from_ref(&v1));
    let mut admissions = batch.admissions().to_vec();
    admissions.push(SpaceObjectAdmission::new(
        lane_b.space.id().clone(),
        StoreObjectId::ChunkRaw(sound.id().clone()),
    ));
    let batch = WriteBatch::new(
        batch.transaction_id().clone(),
        batch.canonical_objects().to_vec(),
        batch.entity_records().to_vec(),
        admissions,
        batch.mutations().to_vec(),
    )
    .unwrap();
    let descriptors = vec![(sound.id().clone(), sound.byte_length())];
    NativeScmRepository::new(&root.0)
        .apply_batch_with_chunks(&batch, &descriptors, |id: &layrs_core::scm::ChunkId| {
            assert_eq!(id, sound.id());
            Ok(sound.bytes().to_vec())
        })
        .unwrap();
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(reader.get_chunk(sound.id()).unwrap().as_ref(), Some(&sound));
    assert_eq!(
        reader.get_chunk(v1.chunks[0].id()).unwrap().as_ref(),
        Some(&v1.chunks[0])
    );
    assert_eq!(
        reader.get_file_manifest(v1.manifest.id()).unwrap().as_ref(),
        Some(&v1.manifest)
    );
}

struct NoCasFault;
impl CasFaultInjector for NoCasFault {
    fn check(&self, _: CasFaultPoint) -> Result<(), String> {
        Ok(())
    }
}

/// A Store that is not format 7 refuses a whole object on every write
/// path, staging included (council s16 r1).
#[test]
fn a_store_before_format_7_refuses_to_stage_a_whole_object() {
    let root = TestRoot::new("whole-stage-5");
    create(&root.0, "whole-stage-5", "5");
    let whole = file("a.txt", b"alpha", Recipe::Whole);
    let exclusive = open_exclusive(&root.0).unwrap();
    let cas = layrs_store_local::scm_v1::NativeCasWriter::with_fault_injector(
        &exclusive,
        Arc::new(NoCasFault),
    )
    .unwrap();
    assert!(
        cas.stage(&CanonicalObject::FileManifest(whole.manifest.clone()))
            .is_err()
    );
    assert!(
        cas.stage(&CanonicalObject::ChunkRaw(whole.chunks[0].clone()))
            .is_err()
    );
}

/// A step carrying the manifest of its new content still proves its other
/// Files closed by existence (council s14 r2): only a manifest a batch
/// admits without carrying it sends the batch's Files to the exact path.
#[test]
fn a_step_with_new_content_proves_its_other_files_by_existence() {
    let root = TestRoot::new("existence-step");
    create(&root.0, "existence-step", "6");
    let mut files = vec![
        file("a.txt", b"alpha", Recipe::Fixed),
        file("b.txt", b"beta", Recipe::Fixed),
    ];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let added = file("c.txt", b"gamma", Recipe::Fixed);
    let carried = file_objects(std::slice::from_ref(&added));
    files.push(added);
    let before = existence_proofs_on_this_thread();
    repository
        .apply_batch(&lane.capture(&files, carried))
        .unwrap();
    assert_eq!(existence_proofs_on_this_thread() - before, 2);
    drop(repository);
    assert_closure_matches(&root.0, &files, lane.space.id());
}

/// The Files the bitmaps prove closed (council s13 Q2) are exactly those
/// the exact path gives an admitted manifest.
fn assert_closure_matches(root: &Path, files: &[File], space: &SpaceId) {
    let store = open(root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let ids: Vec<&FileId> = files.iter().map(|file| file.manifest.file_id()).collect();
    let closed = reader
        .files_durably_closed_metadata(ids.iter().copied(), space)
        .unwrap();
    let exact: std::collections::BTreeSet<FileId> =
        NativeScmReader::admitted_manifests_metadata(&reader, ids.iter().copied(), space)
            .unwrap()
            .into_keys()
            .collect();
    assert_eq!(closed, exact, "{space}");
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
    // A chunk admitted alone closes no File: only A's Files are closed.
    let both = [shared.clone(), other.clone()];
    assert_closure_matches(&root.0, &both, lane_b.space.id());
    assert_closure_matches(&root.0, &both, lane_a.space.id());
    {
        let store = open(&root.0).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let ids = [shared.manifest.file_id(), other.manifest.file_id()];
        assert!(
            reader
                .files_durably_closed_metadata(ids, lane_b.space.id())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            reader
                .files_durably_closed_metadata(ids, lane_a.space.id())
                .unwrap()
                .len(),
            2
        );
    }
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
    // Several manifests of one File (records and explicit ones): closed
    // exactly where the exact path finds an admitted one.
    assert_closure_matches(
        &root.0,
        &[fixed.clone(), single.clone(), small.clone(), large.clone()],
        lane.space.id(),
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
    // A damaged catalog proves no File closed from its bitmaps (council s14
    // r1): the exact path, which reports the damage, decides instead.
    {
        let store = open(&root.0).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let closed =
            reader.files_durably_closed_metadata([files[0].manifest.file_id()], lane.space.id());
        assert!(closed.map_or(true, |closed| closed.is_empty()));
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

/// Every registered pack has its keys row (council s17, B1b): beside an
/// unreadable pack, a key its row excludes is answered exactly (absent, or
/// present elsewhere, a File proven closed by its bits), a key it may hold
/// is still an error, and a row that does not check excludes nothing.
#[test]
fn keys_rows_answer_exactly_beside_a_lost_pack() {
    let root = TestRoot::new("keys");
    create(&root.0, "keys", "7");
    let kept = vec![file("kept.txt", b"kept bytes", Recipe::Whole)];
    let (mut lane, batch) = Lane::begin("keys", &kept);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let lost = vec![file("lost.txt", b"lost bytes", Recipe::Fixed)];
    repository
        .apply_batch(&lane.capture(&lost, file_objects(&lost)))
        .unwrap();
    drop(repository);
    assert_eq!(
        count(&root.0, "SELECT count(*) FROM cas_pack_keys"),
        count(&root.0, "SELECT count(*) FROM cas_packs")
    );
    // The second registered pack (names are random: by registration).
    let second = registered_packs(&root.0).pop().unwrap();
    fs::remove_file(root.0.join("objects/packs").join(second)).unwrap();
    forget_loaded_packs();
    let absent = Chunk::new(b"never stored".to_vec()).unwrap();
    let repository = NativeScmRepository::new(&root.0);
    assert!(!repository.chunk_present_metadata(absent.id()).unwrap());
    assert!(
        repository
            .chunk_present_metadata(kept[0].chunks[0].id())
            .unwrap()
    );
    let error = repository
        .chunk_present_metadata(lost[0].chunks[0].id())
        .unwrap_err();
    assert!(matches!(error, StoreError::Corruption { .. }), "{error:?}");
    {
        let store = open(&root.0).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let closed = reader
            .files_durably_closed_metadata(
                [kept[0].manifest.file_id(), lost[0].manifest.file_id()],
                lane.space.id(),
            )
            .unwrap();
        assert_eq!(
            closed.into_iter().collect::<Vec<_>>(),
            vec![kept[0].manifest.file_id().clone()]
        );
    }
    // A keys row that does not check excludes nothing, and fsck says so.
    sql(&root.0)
        .execute("UPDATE cas_pack_keys SET checksum = zeroblob(16)", [])
        .unwrap();
    forget_loaded_packs();
    let repository = NativeScmRepository::new(&root.0);
    let error = repository.chunk_present_metadata(absent.id()).unwrap_err();
    assert!(matches!(error, StoreError::Corruption { .. }), "{error:?}");
    drop(repository);
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(
        report.findings.iter().any(|finding| {
            finding.code == FsckFindingCode::CasPackKeysMismatch
                && finding.message.contains("no usable keys row")
        }),
        "{:#?}",
        report.findings
    );
}

/// Beside a lost pack, a writer's evidence must be readable (council s18
/// r1): a File whose explicit manifest is readable but one of whose chunks
/// was only in the lost pack is not proven closed, and an admitted Tree
/// whose only copy was there is neither present nor admitted to a writer
/// (to a reader its rows still answer). fsck names the lost pack once.
#[test]
fn beside_a_lost_pack_a_writer_needs_readable_evidence() {
    let root = TestRoot::new("evidence");
    create(&root.0, "evidence", "7");
    let half = 512 * 1024;
    let shared = vec![0xa_u8; half];
    let first = file(
        "first.bin",
        &[shared.clone(), vec![0xb; half]].concat(),
        Recipe::Half,
    );
    let second = file(
        "second.bin",
        &[shared, vec![0xc; half]].concat(),
        Recipe::Half,
    );
    let (mut lane, batch) = Lane::begin("evidence", std::slice::from_ref(&first));
    let first_tree = tree_of(std::slice::from_ref(&first));
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    // The second File's manifest goes to the second pack; their shared
    // chunk stays in the first.
    let both = vec![first.clone(), second.clone()];
    repository
        .apply_batch(&lane.capture(&both, file_objects(std::slice::from_ref(&second))))
        .unwrap();
    drop(repository);
    let lost = registered_packs(&root.0).remove(0);
    fs::remove_file(root.0.join("objects/packs").join(lost)).unwrap();
    forget_loaded_packs();
    let store = open(&root.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(
        reader
            .files_durably_closed_metadata([second.manifest.file_id()], lane.space.id())
            .unwrap()
            .is_empty()
    );
    let tree = StoreObjectId::Tree(first_tree.id().clone());
    assert!(reader.tree_present_metadata(first_tree.id()).unwrap());
    assert!(
        reader
            .object_admitted_metadata(&tree, lane.space.id())
            .unwrap()
    );
    let writer = NativeScmReader::new(&store)
        .unwrap()
        .with_positive_evidence();
    assert!(!writer.tree_present_metadata(first_tree.id()).unwrap());
    assert!(
        !writer
            .object_admitted_metadata(&tree, lane.space.id())
            .unwrap()
    );
    assert!(!writer.durable_admission_proves_closure().unwrap());
    drop(writer);
    drop(reader);
    drop(store);
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    let corruption = report
        .findings
        .iter()
        .filter(|finding| finding.code == FsckFindingCode::CasCorruption)
        .count();
    assert_eq!(corruption, 1, "{:#?}", report.findings);
    // A Tree naming the second File is refused while its shared chunk is
    // only in the lost pack, and accepted once the batch carries it.
    let tree = tree_of(std::slice::from_ref(&second));
    let admit = |objects: Vec<CanonicalObject>, label: &str| {
        let admissions = objects
            .iter()
            .map(|object| SpaceObjectAdmission::new(lane.space.id().clone(), object.object_id()))
            .collect();
        WriteBatch::new(
            TransactionId::new(format!("transaction:evidence-{label}")).unwrap(),
            objects,
            vec![],
            admissions,
            vec![],
        )
        .unwrap()
    };
    let mut repository = NativeScmRepository::new(&root.0);
    let refused = repository
        .apply_batch(&admit(vec![CanonicalObject::Tree(tree.clone())], "tree"))
        .unwrap_err();
    assert!(
        refused.to_string().contains("admission rejected"),
        "{refused}"
    );
    repository
        .apply_batch(&admit(
            vec![
                CanonicalObject::ChunkRaw(second.chunks[0].clone()),
                CanonicalObject::Tree(tree),
            ],
            "tree-again",
        ))
        .unwrap();
}

/// Beside a lost content-only pack, a durably admitted Tree whose File was
/// only there no longer proves its closure (council s18 r2): admitting a
/// Tree that contains it is refused, and accepted once the batch carries the
/// File again.
#[test]
fn a_durable_tree_does_not_prove_content_lost_with_its_pack() {
    let root = TestRoot::new("induction");
    create(&root.0, "induction", "7");
    let lost = file("lost.txt", b"content only in the lost pack", Recipe::Whole);
    let (lane, batch) = Lane::begin("induction", &[]);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let admit = |objects: Vec<CanonicalObject>, label: &str| {
        let admissions = objects
            .iter()
            .map(|object| SpaceObjectAdmission::new(lane.space.id().clone(), object.object_id()))
            .collect();
        WriteBatch::new(
            TransactionId::new(format!("transaction:induction-{label}")).unwrap(),
            objects,
            vec![],
            admissions,
            vec![],
        )
        .unwrap()
    };
    // The File alone (a content-only pack), then a Tree naming it.
    repository
        .apply_batch(&admit(file_objects(std::slice::from_ref(&lost)), "content"))
        .unwrap();
    let inner = tree_of(std::slice::from_ref(&lost));
    repository
        .apply_batch(&admit(vec![CanonicalObject::Tree(inner.clone())], "inner"))
        .unwrap();
    drop(repository);
    let content_pack = registered_packs(&root.0)[1].clone();
    fs::remove_file(root.0.join("objects/packs").join(content_pack)).unwrap();
    forget_loaded_packs();
    let outer = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("dir").unwrap(),
        TreeEntryTarget::tree(inner.id().clone()),
        false,
    )])
    .unwrap();
    let mut repository = NativeScmRepository::new(&root.0);
    let refused = repository
        .apply_batch(&admit(vec![CanonicalObject::Tree(outer.clone())], "outer"))
        .unwrap_err();
    assert!(
        refused.to_string().contains("admission rejected"),
        "{refused}"
    );
    let mut carried = file_objects(std::slice::from_ref(&lost));
    carried.push(CanonicalObject::Tree(outer));
    repository
        .apply_batch(&admit(carried, "outer-again"))
        .unwrap();
    assert_eq!(
        repository.get_chunk(lost.chunks[0].id()).unwrap().as_ref(),
        Some(&lost.chunks[0])
    );
}

/// Beside a lost content-only pack, a durable Step's history is not the
/// batch's to prove (council s18 r3): a new Step whose parent names content
/// only that pack held is accepted when everything it carries is provable.
#[test]
fn a_lost_content_only_pack_does_not_block_an_unrelated_step() {
    let root = TestRoot::new("history");
    create(&root.0, "history", "7");
    let old = file("old.txt", b"old content in a lost pack", Recipe::Whole);
    let (mut lane, batch) = Lane::begin("history", &[]);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let space = lane.space.id().clone();
    let admit = |objects: Vec<CanonicalObject>, label: &str| {
        let admissions = objects
            .iter()
            .map(|object| SpaceObjectAdmission::new(space.clone(), object.object_id()))
            .collect();
        WriteBatch::new(
            TransactionId::new(format!("transaction:history-{label}")).unwrap(),
            objects,
            vec![],
            admissions,
            vec![],
        )
        .unwrap()
    };
    // The old content alone (a content-only pack), then a Step naming it.
    repository
        .apply_batch(&admit(file_objects(std::slice::from_ref(&old)), "content"))
        .unwrap();
    let captured = lane.capture(std::slice::from_ref(&old), vec![]);
    let old_step = captured
        .canonical_objects()
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Step(step) => Some(step.id().clone()),
            _ => None,
        })
        .unwrap();
    repository.apply_batch(&captured).unwrap();
    drop(repository);
    let content_pack = registered_packs(&root.0)[1].clone();
    fs::remove_file(root.0.join("objects/packs").join(content_pack)).unwrap();
    forget_loaded_packs();
    // A new Step after it, with other content, carried.
    let new = file("new.txt", b"new content", Recipe::Whole);
    let tree = tree_of(std::slice::from_ref(&new));
    let step = Step::new(vec![old_step], tree.id().clone()).unwrap();
    let mut objects = file_objects(std::slice::from_ref(&new));
    objects.push(CanonicalObject::Tree(tree));
    objects.push(CanonicalObject::Step(step));
    NativeScmRepository::new(&root.0)
        .apply_batch(&admit(objects, "new-step"))
        .unwrap();
}

/// A Store whose pack files all exceed the in-batch budget (forced tiny
/// here), as packs written past 64 MiB do: one pack per batch, never
/// consolidated by a batch (council s17, B2a), each of `files` captured in
/// its own batch.
fn store_of_unmerged_packs(label: &str, count: usize) -> (TestRoot, Lane, Vec<File>) {
    let root = TestRoot::new(label);
    create(&root.0, label, "7");
    set_consolidation_budget(&root.0, Some(1));
    let (mut lane, batch) = Lane::begin(label, &[]);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let mut files = Vec::new();
    for n in 0..count {
        let next = file(
            &format!("f{n}.txt"),
            format!("{label} content number {n}").as_bytes(),
            Recipe::Whole,
        );
        files.push(next.clone());
        repository
            .apply_batch(&lane.capture(&files, file_objects(&[next])))
            .unwrap();
    }
    drop(repository);
    assert!(
        registered_packs(&root.0).len() > count,
        "batches consolidated"
    );
    (root, lane, files)
}

fn maintenance_leftovers(root: &Path) -> usize {
    fs::read_dir(root.join("objects/packs"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".maint-")
        })
        .count()
}

/// A maintenance pass writes the pack files batches leave out into one,
/// built outside the exclusive lock and registered with the exact admission
/// remap (council s17, B2b): everything stays readable and admitted, fsck
/// is clean, and a second pass has nothing to do.
#[test]
fn maintenance_consolidates_the_packs_batches_leave_out() {
    let (root, lane, files) = store_of_unmerged_packs("maintenance", 7);
    let before = registered_packs(&root.0).len();
    let MaintenanceOutcome::Consolidated { packs } = maintain(&root.0).unwrap() else {
        panic!("nothing consolidated");
    };
    assert!(packs >= 2);
    assert_eq!(registered_packs(&root.0).len(), before - packs + 1);
    assert_eq!(maintenance_leftovers(&root.0), 0);
    forget_loaded_packs();
    let repository = NativeScmRepository::new(&root.0);
    for file in &files {
        assert_eq!(
            repository.get_chunk(file.chunks[0].id()).unwrap().as_ref(),
            Some(&file.chunks[0])
        );
        assert!(
            repository
                .object_admitted_metadata(
                    &StoreObjectId::ChunkRaw(file.chunks[0].id().clone()),
                    lane.space.id()
                )
                .unwrap()
        );
    }
    drop(repository);
    assert_fsck_clean(&root.0);
    let again = maintain(&root.0).unwrap();
    assert!(
        matches!(
            again,
            MaintenanceOutcome::NothingToDo | MaintenanceOutcome::Consolidated { .. }
        ),
        "{again:?}"
    );
}

/// Admissions made while the maintenance pack is built are kept: the
/// registration remaps the bitmaps as they are then (council s17, B2b).
#[test]
fn maintenance_keeps_admissions_made_while_it_builds() {
    let (root, _lane, files) = store_of_unmerged_packs("maintenance-admit", 6);
    let (other, batch) = Lane::begin("maintenance-other", &[]);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    drop(repository);
    let chunk = files[0].chunks[0].clone();
    let admit = WriteBatch::new(
        TransactionId::new("transaction:maintenance-admit").unwrap(),
        vec![CanonicalObject::ChunkRaw(chunk.clone())],
        vec![],
        vec![SpaceObjectAdmission::new(
            other.space.id().clone(),
            StoreObjectId::ChunkRaw(chunk.id().clone()),
        )],
        vec![],
    )
    .unwrap();
    let outcome = maintain_with_pause(&root.0, &mut || {
        NativeScmRepository::new(&root.0)
            .apply_batch(&admit)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .unwrap();
    assert!(
        matches!(outcome, MaintenanceOutcome::Consolidated { .. }),
        "{outcome:?}"
    );
    forget_loaded_packs();
    let repository = NativeScmRepository::new(&root.0);
    assert!(
        repository
            .object_admitted_metadata(
                &StoreObjectId::ChunkRaw(chunk.id().clone()),
                other.space.id()
            )
            .unwrap()
    );
    drop(repository);
    assert_fsck_clean(&root.0);
}

/// Candidates retired while the pack is built (a batch's consolidation
/// took them) abandon the pass: nothing is published and the built file is
/// removed (council s17, B2b).
#[test]
fn maintenance_abandons_a_pack_whose_candidates_moved() {
    let (root, mut lane, mut files) = store_of_unmerged_packs("maintenance-moved", 6);
    let outcome = maintain_with_pause(&root.0, &mut || {
        // The default budget again: the next batch consolidates them all.
        set_consolidation_budget(&root.0, None);
        let next = file("late.txt", b"a late file", Recipe::Whole);
        files.push(next.clone());
        NativeScmRepository::new(&root.0)
            .apply_batch(&lane.capture(&files, file_objects(&[next])))
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .unwrap();
    assert_eq!(outcome, MaintenanceOutcome::Abandoned);
    assert_eq!(maintenance_leftovers(&root.0), 0);
    forget_loaded_packs();
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

/// A batch leaving a group only maintenance may write notes the pass due
/// on its Store, for the CLI to start detached, from the batch that writes
/// the sixth pack file on; once maintenance wrote it, the next batch notes
/// nothing (council s17, B2c; s19 r1). One test: notes are per process.
#[test]
fn a_batch_notes_maintenance_due_until_it_runs() {
    let (root, mut lane, mut files) = store_of_unmerged_packs("maintenance-due", 4);
    assert_eq!(registered_packs(&root.0).len(), 5);
    let canonical = fs::canonicalize(&root.0).unwrap();
    let noted = |due: Vec<PathBuf>| {
        due.iter()
            .any(|path| fs::canonicalize(path).is_ok_and(|path| path == canonical))
    };
    assert!(!noted(take_maintenance_due()));
    let mut step = |name: &str| {
        let next = file(name, name.as_bytes(), Recipe::Whole);
        files.push(next.clone());
        NativeScmRepository::new(&root.0)
            .apply_batch(&lane.capture(&files, file_objects(&[next])))
            .unwrap();
    };
    step("sixth.txt");
    assert_eq!(registered_packs(&root.0).len(), 6);
    assert!(noted(take_maintenance_due()));
    assert!(matches!(
        maintain(&root.0).unwrap(),
        MaintenanceOutcome::Consolidated { .. }
    ));
    step("after.txt");
    assert!(!noted(take_maintenance_due()));
}

/// A crashed pass's temporary pack is removed by the next exclusive
/// session when no pass runs (automatic maintenance may never run again),
/// never while one builds (council s19 r1).
#[test]
fn a_batch_removes_a_crashed_maintenance_pack_when_no_pass_runs() {
    let (root, mut lane, mut files) = store_of_unmerged_packs("maintenance-stale", 3);
    let crashed = root.0.join("objects/packs/.maint-crashed.tmp");
    let mut step = |name: &str| {
        let next = file(name, name.as_bytes(), Recipe::Whole);
        files.push(next.clone());
        NativeScmRepository::new(&root.0)
            .apply_batch(&lane.capture(&files, file_objects(&[next])))
            .unwrap();
    };
    // No lock file (a crash lost its unsynced entry): no pass is building.
    assert!(!root.0.join("metadata/maintenance.lock").exists());
    fs::write(&crashed, b"a pack a crashed pass left").unwrap();
    step("first.txt");
    assert!(!crashed.exists());
    // A lock file no pass holds.
    maintain(&root.0).unwrap();
    assert!(root.0.join("metadata/maintenance.lock").exists());
    fs::write(&crashed, b"a pack a crashed pass left").unwrap();
    step("second.txt");
    assert!(!crashed.exists());
    assert_fsck_clean(&root.0);
}

/// General failures in a row pause automatic maintenance as they pause a
/// batch's consolidation; a pass that completes clears them (council s19
/// r1).
#[test]
fn maintenance_failures_pause_automatic_passes_until_one_completes() {
    let (root, _lane, _files) = store_of_unmerged_packs("maintenance-paused", 5);
    let note = root.0.join("metadata/maintenance-failed");
    fs::write(&note, b"2 0\nan old failure\n").unwrap();
    assert!(
        !maintenance_paused(&root.0),
        "an old failure does not pause"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    fs::write(&note, format!("2 {now}\nno space left\n")).unwrap();
    assert!(maintenance_paused(&root.0));
    // An explicit pass still runs, and clears the note.
    assert!(matches!(
        maintain(&root.0).unwrap(),
        MaintenanceOutcome::Consolidated { .. }
    ));
    assert!(!note.exists());
    assert!(!maintenance_paused(&root.0));
}

/// A pass whose pack is built waits for the exclusive session as a batch
/// does, instead of discarding the pack (council s19 r1).
#[test]
fn maintenance_waits_for_a_busy_store_to_publish() {
    let (root, _lane, files) = store_of_unmerged_packs("maintenance-wait", 5);
    let mut holder = None;
    let outcome = maintain_with_pause(&root.0, &mut || {
        let (held, release) = std::sync::mpsc::channel();
        let path = root.0.clone();
        holder = Some(std::thread::spawn(move || {
            let store = open_exclusive(&path).unwrap();
            held.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(store);
        }));
        release.recv().map_err(|error| error.to_string())
    })
    .unwrap();
    holder.unwrap().join().unwrap();
    assert!(
        matches!(outcome, MaintenanceOutcome::Consolidated { .. }),
        "{outcome:?}"
    );
    forget_loaded_packs();
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

/// Damaged candidates are all found in one scan and left out: the pass
/// consolidates the healthy group (council s19 r3; past the scan budget, a
/// pass is `Abandoned` and the next goes on).
#[test]
fn maintenance_goes_on_past_damaged_candidates() {
    let (root, lane, files) = store_of_unmerged_packs("maintenance-damaged", 7);
    let rows: Vec<(i64, Vec<u8>)> = {
        let connection = sql(&root.0);
        let mut statement = connection
            .prepare(
                "SELECT pack_seq, bitmap FROM space_pack_admissions WHERE space_id=?1                  ORDER BY pack_seq",
            )
            .unwrap();
        statement
            .query_map([lane.space.id().as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(rows.len() >= 6, "{}", rows.len());
    // Three bitmaps lose a byte, as damage would.
    let damaged: Vec<i64> = rows[1..4].iter().map(|(seq, _)| *seq).collect();
    for (seq, bitmap) in &rows[1..4] {
        let broken = if bitmap.len() > 1 {
            bitmap[..bitmap.len() - 1].to_vec()
        } else {
            vec![bitmap[0], 0, 0]
        };
        sql(&root.0)
            .execute(
                "UPDATE space_pack_admissions SET bitmap=?1 WHERE space_id=?2 AND pack_seq=?3",
                rusqlite::params![broken, lane.space.id().as_str(), seq],
            )
            .unwrap();
    }
    assert!(matches!(
        maintain(&root.0).unwrap(),
        MaintenanceOutcome::Consolidated { .. }
    ));
    let skipped = fs::read_to_string(root.0.join("metadata/consolidation-skipped")).unwrap();
    assert_eq!(skipped.lines().count(), 3);
    // The damaged packs are still registered, untouched.
    let registered: Vec<i64> = {
        let connection = sql(&root.0);
        let mut statement = connection
            .prepare("SELECT pack_seq FROM cas_packs")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    for seq in &damaged {
        assert!(registered.contains(seq));
    }
    forget_loaded_packs();
    let repository = NativeScmRepository::new(&root.0);
    assert_eq!(
        repository
            .get_chunk(files[0].chunks[0].id())
            .unwrap()
            .as_ref(),
        Some(&files[0].chunks[0])
    );
}

/// One pass at a time: a pass started while another builds is `Busy`. A
/// crashed pass's temporary pack is removed by the next one (council s17,
/// B2b).
#[test]
fn maintenance_runs_one_pass_at_a_time_and_cleans_a_crashed_one() {
    let (root, _lane, _files) = store_of_unmerged_packs("maintenance-busy", 5);
    let crashed = root.0.join("objects/packs/.maint-crashed.tmp");
    fs::write(&crashed, b"a pack a crashed pass left").unwrap();
    let mut inner = None;
    let outcome = maintain_with_pause(&root.0, &mut || {
        inner = Some(maintain(&root.0)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(inner, Some(MaintenanceOutcome::Busy));
    assert!(
        matches!(outcome, MaintenanceOutcome::Consolidated { .. }),
        "{outcome:?}"
    );
    assert!(!crashed.exists());
    assert_eq!(maintenance_leftovers(&root.0), 0);
    assert_fsck_clean(&root.0);
}

/// fsck checks every readable pack's keys row against its index (council
/// s17, B1c): a missing row, or one that does not check, is reported.
#[test]
fn fsck_reports_a_missing_or_altered_keys_row() {
    let root = TestRoot::new("keys-fsck");
    create(&root.0, "keys-fsck", "7");
    let files = vec![
        file("a.txt", b"alpha", Recipe::Whole),
        file("b.txt", b"beta beta", Recipe::Fixed),
    ];
    let (_lane, batch) = Lane::begin("keys-fsck", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    assert_fsck_clean(&root.0);
    let keys_findings = |root: &Path| -> Vec<String> {
        fsck_existing(root, FsckOptions::default())
            .unwrap()
            .findings
            .into_iter()
            .filter(|finding| finding.code == FsckFindingCode::CasPackKeysMismatch)
            .map(|finding| finding.message)
            .collect()
    };
    sql(&root.0)
        .execute("UPDATE cas_pack_keys SET key_count = key_count + 1", [])
        .unwrap();
    assert_eq!(keys_findings(&root.0), vec!["keys row checksum differs"]);
    sql(&root.0)
        .execute("DELETE FROM cas_pack_keys", [])
        .unwrap();
    assert_eq!(
        keys_findings(&root.0),
        vec!["registered pack has no keys row"]
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
            // Closure from the remapped bitmaps matches the exact path.
            assert_closure_matches(&root.0, &files, lane.space.id());
            assert_closure_matches(&root.0, std::slice::from_ref(&shared), lane_b.space.id());
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
    let big = file("big.bin", &noise(300 * 1024, 7), Recipe::Fixed);
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

/// One inline pack, its sequence number and its label, after one batch.
fn one_inline_pack(label: &str) -> (TestRoot, Vec<File>, Lane, i64, PathBuf) {
    let root = TestRoot::new(label);
    create(&root.0, label, "6");
    set_inline_pack_limit(&root.0, None);
    let files = vec![file("a.txt", b"alpha bytes", Recipe::Fixed)];
    let (lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let (seq, name): (i64, String) = sql(&root.0)
        .query_row(
            "SELECT pack_seq, name FROM cas_packs WHERE stored_inline=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let label = root
        .0
        .join("metadata")
        .join(format!("layrs.sqlite3#{name}"));
    (root, files, lane, seq, label)
}

/// Pack-level damage of an inline pack (its metadata, a missing image) is
/// corruption named at its database row, never a file (council s8 r1).
#[test]
fn a_damaged_or_missing_inline_image_is_named_at_its_row() {
    for missing in [false, true] {
        let (root, files, lane, seq, label) = one_inline_pack(&format!("inline-row-{missing}"));
        let connection = sql(&root.0);
        if missing {
            connection
                .execute("DELETE FROM cas_pack_inline WHERE pack_seq=?1", [seq])
                .unwrap();
        } else {
            let mut image: Vec<u8> = connection
                .query_row(
                    "SELECT image FROM cas_pack_inline WHERE pack_seq=?1",
                    [seq],
                    |row| row.get(0),
                )
                .unwrap();
            // A byte of its metadata, before the footer.
            let at = image.len() - 72 - 3;
            image[at] ^= 0x5a;
            connection
                .execute(
                    "UPDATE cas_pack_inline SET image=?1 WHERE pack_seq=?2",
                    rusqlite::params![image, seq],
                )
                .unwrap();
        }
        drop(connection);
        let repository = NativeScmRepository::new(&root.0);
        let chunk = StoreObjectId::ChunkRaw(files[0].chunks[0].id().clone());
        assert!(repository.get_chunk(files[0].chunks[0].id()).is_err());
        assert!(
            repository
                .object_admitted_metadata(&chunk, lane.space.id())
                .is_err()
        );
        drop(repository);
        let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
        assert!(
            report.findings.iter().any(|finding| {
                finding.code == FsckFindingCode::CasCorruption
                    && finding.path.as_deref() == Some(label.as_path())
            }),
            "{missing}: {:#?}",
            report.findings
        );
        assert!(
            report.findings.iter().all(|finding| finding
                .path
                .as_deref()
                .is_none_or(|path| !path.starts_with(root.0.join("objects")))),
            "{missing}: {:#?}",
            report.findings
        );
    }
}

/// A scrub reads inline images from the database now: damage done after a
/// CAS view loaded its catalog is reported (council s8 r1).
#[test]
fn scrub_rereads_inline_images_damaged_after_the_view_loaded() {
    let (root, files, _lane, _seq, label) = one_inline_pack("inline-scrub");
    let store = open(&root.0).unwrap();
    let cas = NativeCas::open(&store).unwrap();
    assert!(cas.scrub().unwrap().is_clean());
    let location = cas
        .locate(&StoreObjectId::ChunkRaw(files[0].chunks[0].id().clone()))
        .unwrap()
        .unwrap();
    location.flip_stored_byte().unwrap();
    let report = cas.scrub().unwrap();
    assert!(!report.is_clean());
    assert!(report.issues.iter().any(|issue| issue.path == label));
}

/// Inline packs merge at eight; a merge whose image outgrows the inline
/// limit becomes a pack file.
#[test]
fn inline_packs_that_outgrow_the_limit_merge_into_a_file() {
    let root = TestRoot::new("inline-spill");
    create(&root.0, "inline-spill", "6");
    // Each batch's pack fits, eight of them together do not.
    set_inline_pack_limit(&root.0, Some(2048));
    let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    for step in 0..9 {
        let added = file(&format!("f{step}.txt"), &noise(300, step), Recipe::Fixed);
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        repository
            .apply_batch(&lane.capture(&files, carried))
            .unwrap();
    }
    drop(repository);
    assert_eq!(packs(&root.0).len(), 1);
    assert!(inline_rows(&root.0) < 8);
    assert_eq!(
        count(
            &root.0,
            "SELECT count(*) FROM cas_packs WHERE stored_inline=0"
        ),
        1
    );
    let answers = reader_answers(&root.0, &files, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false"))
    );
    assert_fsck_clean(&root.0);
}

/// Inline packs of up to 256 KiB merge once they hold 1 MiB, before eight of
/// them, and the merged pack, past 64 KiB, is a file (council s13 Q1).
#[test]
fn large_inline_packs_merge_by_size_into_a_file() {
    let root = TestRoot::new("inline-bytes");
    create(&root.0, "inline-bytes", "6");
    set_inline_pack_limit(&root.0, None);
    let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    for step in 0..5 {
        let added = file(
            &format!("f{step}.bin"),
            &noise(200 * 1024, step),
            Recipe::Fixed,
        );
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        repository
            .apply_batch(&lane.capture(&files, carried))
            .unwrap();
    }
    // Six inline packs, under 1 MiB together: no merge yet.
    assert_eq!(inline_rows(&root.0), 6);
    assert!(packs(&root.0).is_empty());
    let added = file("f5.bin", &noise(200 * 1024, 5), Recipe::Fixed);
    let carried = file_objects(std::slice::from_ref(&added));
    files.push(added);
    repository
        .apply_batch(&lane.capture(&files, carried))
        .unwrap();
    drop(repository);
    // Seven packs over 1 MiB: merged into one pack file.
    assert_eq!(packs(&root.0).len(), 1);
    assert_eq!(inline_rows(&root.0), 0);
    let answers = reader_answers(&root.0, &files, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false"))
    );
    assert_fsck_clean(&root.0);
}

/// A batch stopped after its commit, before its checked checkpoint: the
/// commit (inline image included) is recovered by the next open.
#[test]
fn an_inline_pack_committed_before_a_stop_is_recovered() {
    let root = TestRoot::new("inline-stop");
    create(&root.0, "inline-stop", "6");
    set_inline_pack_limit(&root.0, None);
    let files = vec![file("a.txt", b"alpha bytes", Recipe::Fixed)];
    let (lane, batch) = Lane::begin("alpha", &files);
    let mut faulty = NativeScmRepository::with_fault_injector(
        &root.0,
        Arc::new(RepositoryFault(RepositoryFaultPoint::AfterCommit)),
    );
    assert!(faulty.apply_batch(&batch).is_err());
    drop(faulty);
    let answers = reader_answers(&root.0, &files, lane.space.id());
    assert!(
        answers
            .iter()
            .all(|answer| !answer.contains("admitted=false")),
        "{answers:?}"
    );
    assert_eq!(inline_rows(&root.0), 1);
    assert!(packs(&root.0).is_empty());
    assert_fsck_clean(&root.0);
}

/// An invalid declared form is an invalid registration, even when the pack
/// file it names is sound (council s8 r2).
#[test]
fn an_invalid_declared_form_is_never_read_as_a_file() {
    let root = TestRoot::new("invalid-form");
    create(&root.0, "invalid-form", "6");
    let files = vec![file("a.txt", b"alpha bytes", Recipe::Fixed)];
    let (lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let connection = sql(&root.0);
    connection
        .execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE cas_packs SET stored_inline=2;")
        .unwrap();
    drop(connection);
    let repository = NativeScmRepository::new(&root.0);
    assert!(repository.get_chunk(files[0].chunks[0].id()).is_err());
    assert!(
        repository
            .object_admitted_metadata(
                &StoreObjectId::ChunkRaw(files[0].chunks[0].id().clone()),
                lane.space.id()
            )
            .is_err()
    );
    drop(repository);
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(!report.is_clean());
}

/// A recent object is read from the most recent packs without loading the
/// others, an old one through the whole catalog; a damaged recent copy is
/// corruption either way, never absence.
#[test]
fn recent_objects_read_from_recent_packs_and_damage_stays_corruption() {
    let root = TestRoot::new("recent-read");
    create(&root.0, "recent-read", "6");
    let mut files = vec![file("first.txt", b"first", Recipe::Fixed)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let first_step = {
        let store = open(&root.0).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let view = reader.get_repository_view(&lane.view).unwrap().unwrap();
        view.layer_heads()[0].step_ids()[0].clone()
    };
    for step in 0..5 {
        let added = file(
            &format!("f{step}.txt"),
            format!("content {step}").as_bytes(),
            Recipe::Fixed,
        );
        let carried = file_objects(std::slice::from_ref(&added));
        files.push(added);
        NativeScmRepository::new(&root.0)
            .apply_batch(&lane.capture(&files, carried))
            .unwrap();
    }
    // Each read in a fresh session: the first read loads no catalog.
    let read_view = || {
        let store = open(&root.0).unwrap();
        NativeScmReader::new(&store)
            .unwrap()
            .get_repository_view(&lane.view)
    };
    let view = read_view().unwrap().unwrap();
    assert_eq!(view.id(), &lane.view);
    let store = open(&root.0).unwrap();
    // An old Step, in the oldest pack: through the whole catalog.
    assert!(
        NativeScmReader::new(&store)
            .unwrap()
            .get_step(&first_step)
            .unwrap()
            .is_some()
    );
    drop(store);
    // The newest View's block damaged: an error, not an absence.
    flip_located(&root.0, &StoreObjectId::RepositoryView(lane.view.clone()));
    assert!(read_view().is_err());
}

/// A pack file whose row is damaged to declare it inline keeps its file:
/// orphan collection keeps every registered name (council s8 r3).
#[test]
fn a_file_pack_declared_inline_by_damage_keeps_its_file() {
    let root = TestRoot::new("form-flip");
    create(&root.0, "form-flip", "6");
    let mut files = vec![file("a.txt", b"alpha bytes", Recipe::Fixed)];
    let (mut lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    let pack = packs(&root.0).remove(0);
    sql(&root.0)
        .execute("UPDATE cas_packs SET stored_inline=1", [])
        .unwrap();
    // The next write opens the CAS writer (orphan collection runs), whether
    // or not the damaged pack lets the batch through.
    let added = file("b.txt", b"beta bytes", Recipe::Fixed);
    let carried = file_objects(std::slice::from_ref(&added));
    files.push(added);
    let _ = NativeScmRepository::new(&root.0).apply_batch(&lane.capture(&files, carried));
    assert!(pack.is_file(), "{pack:?} was removed");
    assert!(
        NativeScmRepository::new(&root.0)
            .get_chunk(files[0].chunks[0].id())
            .is_err()
    );
}

/// An inline image past the limit (its CHECK bypassed) is damage to its
/// pack, found from its length without reading it (council s8 r3).
#[test]
fn an_oversized_inline_image_is_damage_without_being_read() {
    let (root, files, _lane, seq, label) = one_inline_pack("inline-oversized");
    sql(&root.0)
        .execute_batch(&format!(
            "PRAGMA ignore_check_constraints=ON; \
             UPDATE cas_pack_inline SET image=zeroblob(2097152) WHERE pack_seq={seq};"
        ))
        .unwrap();
    assert!(
        NativeScmRepository::new(&root.0)
            .get_chunk(files[0].chunks[0].id())
            .is_err()
    );
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(
        report.findings.iter().any(|finding| {
            finding.code == FsckFindingCode::CasCorruption
                && finding.path.as_deref() == Some(label.as_path())
        }),
        "{:#?}",
        report.findings
    );
}

/// A pack file stays open only while a bulk read holds its catalog
/// (ADR 0026, council s9 r2): never after a read without a hold, and no
/// longer once the last hold is dropped.
#[test]
fn pack_files_stay_open_only_while_held() {
    let root = TestRoot::new("pack-holds");
    create(&root.0, "pack-holds", "6");
    let files = vec![
        file("a.txt", b"alpha bytes", Recipe::Fixed),
        file("b.txt", b"beta bytes", Recipe::Fixed),
    ];
    let (_lane, batch) = Lane::begin("alpha", &files);
    NativeScmRepository::new(&root.0)
        .apply_batch(&batch)
        .unwrap();
    assert_eq!(packs(&root.0).len(), 1);
    let chunk = |index: usize| StoreObjectId::ChunkRaw(files[index].chunks[0].id().clone());
    // Each phase on a view of its own: a fresh catalog, an empty block cache.
    let view = || NativeCas::open(&open(&root.0).unwrap()).unwrap();
    // Without a hold, a read keeps nothing open.
    let cas = view();
    cas.read(&chunk(0)).unwrap();
    assert_eq!(held_pack_files(&cas), 0);
    // While held (twice), the pack read stays open; until the last hold ends.
    let cas = view();
    let first = cas.hold_pack_files().unwrap();
    let second = cas.hold_pack_files().unwrap();
    assert!(first.holds(&cas));
    cas.read(&chunk(0)).unwrap();
    assert_eq!(held_pack_files(&cas), 1);
    drop(first);
    assert_eq!(held_pack_files(&cas), 1);
    drop(second);
    assert_eq!(held_pack_files(&cas), 0);
    // A read after the last hold keeps nothing open again.
    let cas = view();
    drop(cas.hold_pack_files());
    cas.read(&chunk(1)).unwrap();
    assert_eq!(held_pack_files(&cas), 0);
}

/// A format 6 database returns the pages a commit frees (auto_vacuum FULL);
/// a format 5 one keeps SQLite's default.
#[test]
fn format_6_databases_return_freed_pages() {
    for (format, mode) in [("6", 1), ("5", 0)] {
        let root = TestRoot::new(&format!("vacuum-{format}"));
        create(&root.0, "vacuum", format);
        let auto_vacuum: i64 = sql(&root.0)
            .pragma_query_value(None, "auto_vacuum", |row| row.get(0))
            .unwrap();
        assert_eq!(auto_vacuum, mode, "format {format}");
    }
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
