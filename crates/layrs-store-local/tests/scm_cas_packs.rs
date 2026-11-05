//! CAS packs of small objects (ADR 0026). Every test holds `serial()`: some
//! set process-wide variables that change how Stores and batches are made.
use layrs_core::scm::{Chunk, StoreId};
use layrs_store::scm::{CanonicalObject, StoreObjectId};
use layrs_store_local::scm_v1::{
    CasFaultInjector, CasFaultPoint, CasLocation, CasResult, InstallOutcome, NativeCas,
    NativeCasWriter, NativeStoreExclusive, PutItem, init as init_store, open as open_store,
    open_exclusive,
};
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

static SERIAL: Mutex<()> = Mutex::new(());
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "layrs-scm-packs-{name}-{}-{nanos:x}-{sequence}",
            std::process::id()
        )))
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn exclusive_store(root: &TestRoot, format_2: bool) -> NativeStoreExclusive {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    if format_2 {
        unsafe { std::env::set_var("LAYRS_STORE_FORMAT", "2") };
    }
    let created = init_store(
        &root.0,
        StoreId::parse(format!("store:packs-{sequence}")).unwrap(),
    );
    unsafe { std::env::remove_var("LAYRS_STORE_FORMAT") };
    drop(created.unwrap());
    open_exclusive(&root.0).unwrap()
}

fn chunks(prefix: &str, count: usize) -> Vec<CanonicalObject> {
    (0..count)
        .map(|index| {
            CanonicalObject::ChunkRaw(Chunk::new(format!("{prefix} {index}").into_bytes()).unwrap())
        })
        .collect()
}

fn put_all(cas: &NativeCasWriter<'_>, objects: &[CanonicalObject]) -> Vec<CasResult<InstallOutcome>> {
    let mut items = objects.iter();
    cas.put_ordered::<()>(|| items.next().map(|object| Ok(PutItem::Borrowed(object))))
        .unwrap()
}

fn packs(cas: &NativeCas) -> Vec<PathBuf> {
    match fs::read_dir(cas.objects_dir().join("packs")) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

fn staging_leftovers(cas: &NativeCas) -> usize {
    fs::read_dir(cas.staging_dir())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("scm-v1-")
        })
        .count()
}

fn packed_at(cas: &NativeCas, id: &StoreObjectId) -> (PathBuf, u64) {
    match cas.locate(id).unwrap() {
        Some(CasLocation::Packed { pack, offset, .. }) => (pack, offset),
        other => panic!("{id} is not packed: {other:?}"),
    }
}

fn flip_byte(path: &Path, at: u64) {
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
    let mut byte = [0_u8; 1];
    file.seek(SeekFrom::Start(at)).unwrap();
    std::io::Read::read_exact(&mut file, &mut byte).unwrap();
    file.seek(SeekFrom::Start(at)).unwrap();
    file.write_all(&[byte[0] ^ 0x55]).unwrap();
}

const RECORD_HEADER: u64 = 37;

#[test]
fn sixty_three_new_small_objects_stay_separate_and_sixty_four_are_packed() {
    let _serial = serial();
    let root = TestRoot::new("threshold-63");
    let exclusive = exclusive_store(&root, false);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let objects = chunks("below", 63);
    for outcome in put_all(&cas, &objects) {
        assert!(!outcome.unwrap().existed);
    }
    assert!(packs(&cas).is_empty());
    for object in &objects {
        assert!(cas.object_path(&object.object_id()).unwrap().is_file());
        assert_eq!(cas.read(&object.object_id()).unwrap(), *object);
    }

    let root = TestRoot::new("threshold-64");
    let exclusive = exclusive_store(&root, false);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let objects = chunks("at", 64);
    for (object, outcome) in objects.iter().zip(put_all(&cas, &objects)) {
        let outcome = outcome.unwrap();
        assert!(!outcome.existed);
        // The canonical path stays the metadata path.
        assert_eq!(outcome.path, cas.object_path(&object.object_id()).unwrap());
    }
    assert_eq!(packs(&cas).len(), 1);
    for object in &objects {
        let id = object.object_id();
        assert!(!cas.object_path(&id).unwrap().exists());
        assert!(cas.contains(&id).unwrap());
        cas.verify(&id).unwrap();
        assert_eq!(cas.read(&id).unwrap(), *object);
        let CanonicalObject::ChunkRaw(chunk) = object else { unreachable!() };
        let mut streamed = Vec::new();
        cas.stream_verified_chunk(chunk.id(), |bytes| streamed.extend_from_slice(bytes))
            .unwrap();
        assert_eq!(streamed, chunk.bytes());
        packed_at(&cas, &id);
    }
    let report = cas.scrub().unwrap();
    assert!(report.is_clean(), "{:?}", report.issues);
    assert_eq!(report.checked_objects, 64);
    assert_eq!(staging_leftovers(&cas), 0);
}

#[test]
fn duplicates_are_packed_once_and_large_objects_stay_separate() {
    let _serial = serial();
    let root = TestRoot::new("mixed");
    let exclusive = exclusive_store(&root, false);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let mut objects = chunks("mixed", 70);
    objects.extend(chunks("mixed", 70));
    let large: Vec<_> = (0..2_u8)
        .map(|seed| CanonicalObject::ChunkRaw(Chunk::new(vec![seed; 100 * 1024]).unwrap()))
        .collect();
    objects.extend(large.iter().cloned());
    let outcomes: Vec<_> = put_all(&cas, &objects)
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(outcomes.iter().filter(|outcome| !outcome.existed).count(), 72);
    assert_eq!(packs(&cas).len(), 1);
    for object in &large {
        let id = object.object_id();
        assert!(cas.object_path(&id).unwrap().is_file());
        assert!(matches!(cas.locate(&id).unwrap(), Some(CasLocation::Loose { .. })));
    }
    for object in &objects {
        assert_eq!(cas.read(&object.object_id()).unwrap(), *object);
    }
    assert!(cas.scrub().unwrap().is_clean());
}

#[test]
fn a_second_batch_reuses_verified_members_in_each_durability_mode() {
    let _serial = serial();
    for mode in [None, Some("coalesced")] {
        let root = TestRoot::new("reuse");
        let exclusive = exclusive_store(&root, false);
        if let Some(mode) = mode {
            unsafe { std::env::set_var("LAYRS_CAS_BATCH_DURABILITY", mode) };
        }
        let objects = chunks("reuse", 100);
        {
            let cas = NativeCas::open_exclusive(&exclusive).unwrap();
            put_all(&cas, &objects).into_iter().for_each(|outcome| {
                outcome.unwrap();
            });
            for outcome in put_all(&cas, &objects) {
                assert!(outcome.unwrap().existed, "{mode:?}");
            }
            assert_eq!(packs(&cas).len(), 1, "{mode:?}");
        }
        unsafe { std::env::remove_var("LAYRS_CAS_BATCH_DURABILITY") };
        drop(exclusive);
        // A reader view of the reopened Store finds every member.
        let store = open_store(&root.0).unwrap();
        let reader = NativeCas::open(&store).unwrap();
        for object in &objects {
            assert_eq!(reader.read(&object.object_id()).unwrap(), *object);
        }
    }
}

#[test]
fn a_damaged_member_fails_alone_is_named_by_scrub_and_is_rewritten() {
    let _serial = serial();
    let root = TestRoot::new("damaged-member");
    let exclusive = exclusive_store(&root, false);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let objects = chunks("damaged", 64);
    put_all(&cas, &objects).into_iter().for_each(|outcome| {
        outcome.unwrap();
    });
    let victim = objects[10].object_id();
    let (pack, offset) = packed_at(&cas, &victim);
    flip_byte(&pack, offset + RECORD_HEADER);

    assert!(cas.read(&victim).is_err());
    assert!(cas.verify(&victim).is_err());
    for object in objects.iter().filter(|object| object.object_id() != victim) {
        assert_eq!(cas.read(&object.object_id()).unwrap(), *object);
    }
    let report = cas.scrub().unwrap();
    assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
    assert!(report.issues[0].message.contains(victim.as_str()));

    // The damaged copy is ignored, never reused: the object is rewritten.
    let outcomes = put_all(&cas, std::slice::from_ref(&objects[10]));
    assert!(!outcomes[0].as_ref().unwrap().existed);
    assert_eq!(cas.read(&victim).unwrap(), objects[10]);
    assert!(matches!(
        cas.locate(&victim).unwrap(),
        Some(CasLocation::Packed { .. })
    ));
    // Scrub still reports the damaged member, even though a sound copy
    // exists elsewhere.
    assert_eq!(cas.scrub().unwrap().issues.len(), 1);
}

#[test]
fn an_invalid_index_or_truncated_pack_is_reported_and_never_trusted() {
    let _serial = serial();
    let root = TestRoot::new("damaged-index");
    let exclusive = exclusive_store(&root, false);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let objects = chunks("index", 64);
    put_all(&cas, &objects).into_iter().for_each(|outcome| {
        outcome.unwrap();
    });
    let pack = packs(&cas).remove(0);
    let length = fs::metadata(&pack).unwrap().len();
    // Inside the index, just before the 64-byte footer.
    flip_byte(&pack, length - 64 - 3);
    let report = cas.scrub().unwrap();
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "cas_pack_invalid"),
        "{:?}",
        report.issues
    );
    // Self-describing records still verify one by one.
    assert_eq!(report.issues.len(), 1, "{:?}", report.issues);

    let file = fs::OpenOptions::new().write(true).open(&pack).unwrap();
    file.set_len(length / 2).unwrap();
    drop(file);
    assert!(cas.read(&objects[0].object_id()).is_err());
    assert!(!cas.scrub().unwrap().is_clean());
}

#[test]
fn a_format_2_store_never_receives_a_pack() {
    let _serial = serial();
    let root = TestRoot::new("format-2");
    let exclusive = exclusive_store(&root, true);
    assert_eq!(exclusive.format_version(), 2);
    assert!(!exclusive.packs_enabled());
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let objects = chunks("format2", 100);
    put_all(&cas, &objects).into_iter().for_each(|outcome| {
        outcome.unwrap();
    });
    assert!(!cas.objects_dir().join("packs").exists());
    for object in &objects {
        assert!(cas.object_path(&object.object_id()).unwrap().is_file());
    }
    drop(cas);
    drop(exclusive);
    let format = fs::read_to_string(open_exclusive(&root.0).unwrap().layout().format_path()).unwrap();
    assert!(format.contains("\"formatVersion\":2"), "{format}");

    let root = TestRoot::new("format-5");
    let exclusive = exclusive_store(&root, false);
    assert_eq!(exclusive.format_version(), 5);
    let format = fs::read_to_string(exclusive.layout().format_path()).unwrap();
    assert!(format.contains("\"formatVersion\":5"), "{format}");
    assert!(format.contains("\"minReaderVersion\":5"), "{format}");
}

#[test]
fn per_object_durability_disables_packs() {
    let _serial = serial();
    let root = TestRoot::new("per-object");
    let exclusive = exclusive_store(&root, false);
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    unsafe { std::env::set_var("LAYRS_CAS_BATCH_DURABILITY", "per-object") };
    let outcomes = put_all(&cas, &chunks("per-object", 80));
    unsafe { std::env::remove_var("LAYRS_CAS_BATCH_DURABILITY") };
    outcomes.into_iter().for_each(|outcome| {
        outcome.unwrap();
    });
    assert!(packs(&cas).is_empty());
}

struct FailAt {
    point: CasFaultPoint,
    fired: AtomicBool,
}

impl CasFaultInjector for FailAt {
    fn check(&self, point: CasFaultPoint) -> Result<(), String> {
        if point == self.point && !self.fired.swap(true, Ordering::AcqRel) {
            return Err(format!("injected at {point:?}"));
        }
        Ok(())
    }
}

#[test]
fn a_failed_pack_publication_is_retried_and_an_orphan_pack_is_reused() {
    let _serial = serial();
    for point in [
        CasFaultPoint::BeforePackPublication,
        CasFaultPoint::AfterPackPublication,
    ] {
        let root = TestRoot::new("pack-fault");
        let exclusive = exclusive_store(&root, false);
        let objects = chunks("fault", 64);
        {
            let cas = NativeCasWriter::with_fault_injector(
                &exclusive,
                Arc::new(FailAt {
                    point,
                    fired: AtomicBool::new(false),
                }),
            )
            .unwrap();
            let outcomes = put_all(&cas, &objects);
            assert!(outcomes.iter().any(Result::is_err), "{point:?}");
            assert_eq!(staging_leftovers(&cas), 0, "{point:?}");
            let published = packs(&cas).len();
            assert_eq!(
                published,
                usize::from(point == CasFaultPoint::AfterPackPublication),
                "{point:?}"
            );
        }
        let cas = NativeCas::open_exclusive(&exclusive).unwrap();
        let outcomes: Vec<_> = put_all(&cas, &objects)
            .into_iter()
            .map(Result::unwrap)
            .collect();
        // An orphan pack from the failed batch is verified and reused.
        let reused = point == CasFaultPoint::AfterPackPublication;
        assert!(outcomes.iter().all(|outcome| outcome.existed == reused), "{point:?}");
        assert_eq!(packs(&cas).len(), 1, "{point:?}");
        for object in &objects {
            assert_eq!(cas.read(&object.object_id()).unwrap(), *object);
        }
        assert!(cas.scrub().unwrap().is_clean());
    }
}
