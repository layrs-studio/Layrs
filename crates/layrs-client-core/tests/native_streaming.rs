use layrs_client_core::native_scm::*;
use layrs_core::scm::{Chunk, ChunkId};
use layrs_store::scm::{CanonicalObject, StoreError};
use layrs_store_local::scm_v1::{
    self,
    checkout::{CheckoutProjectionContent, CheckoutProjectionEntry},
    materialize::{
        self, CheckoutSources, MaterializationFaults, MaterializationPoint, NoMaterializationFaults,
    },
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Meter;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(n: usize) {
    PEAK.fetch_max(LIVE.fetch_add(n, Ordering::Relaxed) + n, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            add(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            add(n);
        }
        q
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("layrs-streaming-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn bytes(index: usize) -> Vec<u8> {
    let mut b = vec![37; 1024 * 1024];
    b[..8].copy_from_slice(&(index as u64).to_le_bytes());
    b
}
fn write_large(output: &mut dyn Write, count: usize) -> io::Result<()> {
    for i in 0..count {
        output.write_all(&bytes(i))?;
    }
    Ok(())
}
fn limits() -> SnapshotLimits {
    SnapshotLimits {
        max_file_bytes: 1024 * 1024 * 1024,
        max_total_bytes: 4 * 1024 * 1024 * 1024,
        ..SnapshotLimits::default()
    }
}
struct Fault;
impl MaterializationFaults for Fault {
    fn check(&self, p: MaterializationPoint) -> io::Result<()> {
        if p == MaterializationPoint::AfterPath {
            Err(io::Error::other("streaming recovery fault"))
        } else {
            Ok(())
        }
    }
}

fn exercise(count: usize) {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("large.bin"), b"original").unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "Streaming").unwrap();
    let original = workspace.state().unwrap();
    let old_step = workspace
        .create_marker("before", "retained")
        .unwrap()
        .target_step_id()
        .clone();
    let before = workspace.read_step_projection(&old_step).unwrap();
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    write_large(
        &mut fs::File::create(fixture.0.join("large.bin")).unwrap(),
        count,
    )
    .unwrap();
    let client = NativeClient::open(workspace.store_root()).unwrap();
    let CapturePlan::Change(prepared) = client
        .prepare_disk_capture(
            original.worktree.id(),
            limits(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("capture missing")
    };
    let manifest = prepared
        .batch()
        .canonical_objects()
        .iter()
        .find_map(|o| match o {
            CanonicalObject::FileManifest(m) => Some(m),
            _ => None,
        })
        .unwrap();
    assert_eq!(manifest.byte_length(), count as u64 * 1024 * 1024);
    assert_eq!(manifest.chunks().len(), count);
    assert!(!prepared
        .batch()
        .canonical_objects()
        .iter()
        .any(|o| matches!(o, CanonicalObject::ChunkRaw(_))));
    let after = vec![CheckoutProjectionEntry {
        path: "large.bin".into(),
        content: CheckoutProjectionContent::File {
            file_id: manifest.file_id().clone(),
            byte_length: manifest.byte_length(),
            executable: false,
        },
    }];
    let descriptors: Vec<_> = manifest
        .chunks()
        .iter()
        .map(|c| (c.chunk_id().clone(), c.byte_length()))
        .collect();
    let indexes: BTreeMap<_, _> = manifest
        .chunks()
        .iter()
        .enumerate()
        .map(|(i, c)| (c.chunk_id().clone(), i))
        .collect();
    fs::write(fixture.0.join("large.bin"), b"original").unwrap();
    let mut read = |id: &ChunkId| {
        Ok(bytes(*indexes.get(id).ok_or(StoreError::Corruption {
            message: "unknown test chunk".into(),
        })?))
    };
    let mut write = |path: &str, out: &mut dyn Write| {
        assert_eq!(path, "large.bin");
        write_large(out, count)
    };
    let mut sources = CheckoutSources {
        chunks: &descriptors,
        read_chunk: &mut read,
        write_file: &mut write,
    };
    // Interrupt after replacing the real large file; recovery must stream its
    // fingerprint and restore the exact old original without changing heads.
    assert!(materialize::apply_checkout_with_sources(
        workspace.store_root(),
        original.worktree.id(),
        &before,
        &after,
        prepared.batch(),
        &Fault,
        &mut sources
    )
    .is_err());
    materialize::recover_checkout(workspace.store_root(), original.worktree.id()).unwrap();
    assert_eq!(fs::read(fixture.0.join("large.bin")).unwrap(), b"original");
    let applied = materialize::apply_checkout_with_sources(
        workspace.store_root(),
        original.worktree.id(),
        &before,
        &after,
        prepared.batch(),
        &NoMaterializationFaults,
        &mut sources,
    )
    .unwrap();
    assert!(!applied.was_replay());
    assert!(client.accept(&prepared).unwrap().was_replay());
    let reopened = NativeWorkspace::open(&fixture.0).unwrap();
    assert_eq!(
        reopened.read_step_projection(prepared.step_id()).unwrap(),
        after
    );
    assert_eq!(reopened.read_step_projection(&old_step).unwrap(), before);
    let mut disk = fs::File::open(fixture.0.join("large.bin")).unwrap();
    use std::io::Read;
    for descriptor in &descriptors {
        let mut b = vec![0; 1024 * 1024];
        disk.read_exact(&mut b).unwrap();
        assert_eq!(Chunk::new(b).unwrap().id(), &descriptor.0);
    }
    assert_eq!(disk.read(&mut [0u8; 1]).unwrap(), 0);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    eprintln!("streaming_fixture_mib={count} rust_heap_additional_peak_bytes={peak}");
    assert!(
        peak < 64 * 1024 * 1024,
        "streaming heap unexpectedly accumulated content: {peak}"
    );
    let report =
        scm_v1::fsck_existing(workspace.store_root(), scm_v1::FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    assert!(!report.is_truncated());
}

#[test]
fn capture_materialize_recover_100_mib_with_bounded_heap() {
    exercise(100);
}
#[test]
#[ignore = "writes real multi-GiB fixture; run explicitly during streaming qualification"]
fn capture_materialize_recover_one_gib_with_bounded_heap() {
    exercise(1024);
}
