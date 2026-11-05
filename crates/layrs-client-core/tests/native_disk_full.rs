//! Actual ENOSPC on a bounded, disposable Docker tmpfs. No injected I/O error.
#![cfg(target_os = "linux")]
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_core::scm::Chunk;
use layrs_store::scm::CanonicalObject;
use layrs_store_local::scm_v1::{
    self,
    materialize::{MaterializationFaults, MaterializationPoint},
    NativeCas,
};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(
            std::env::var("LAYRS_TEST_TMPFS_ROOT").expect("use scripts/test-linux-disk-full.mjs"),
        );
        assert_eq!(root, Path::new("/layrs-full"));
        assert!(fs::symlink_metadata(&root).unwrap().is_dir());
        assert_eq!(fs::canonicalize(&root).unwrap(), root);
        assert!(
            fs::read_to_string("/proc/self/mountinfo")
                .unwrap()
                .lines()
                .any(|line| {
                    let Some((left, right)) = line.split_once(" - ") else {
                        return false;
                    };
                    left.split_whitespace().nth(4) == Some("/layrs-full")
                        && right.split_whitespace().next() == Some("tmpfs")
                }),
            "the disposable test root must be a real tmpfs mount"
        );
        assert!((16 * 1024 * 1024..=128 * 1024 * 1024).contains(&fs2::total_space(&root).unwrap()));
        let path = root.join(format!("disk-full-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn workspace(&self) -> NativeWorkspace {
        let root = self.0.join("checkout");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("data.bin"), b"original bytes").unwrap();
        fs::write(root.join("keep.txt"), b"preserved neighbour").unwrap();
        NativeWorkspace::initialize(root, "Disk full fixture").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert_eq!(self.0.parent(), Some(Path::new("/layrs-full")));
            assert!(self
                .0
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("disk-full-"));
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

struct FullDisk {
    file: File,
    path: PathBuf,
}
impl FullDisk {
    fn fill(root: &Path, reserve: u64) -> Self {
        let path = root.join("pressure.bin");
        let mut file = File::create_new(&path).unwrap();
        let block = [0x5a; 64 * 1024];
        loop {
            match file.write_all(&block) {
                Ok(()) => assert!(file.metadata().unwrap().len() <= 128 * 1024 * 1024),
                Err(error) => {
                    assert_eq!(
                        error.raw_os_error(),
                        Some(28),
                        "actual Linux ENOSPC required: {error}"
                    );
                    break;
                }
            }
        }
        let len = file.metadata().unwrap().len();
        assert!(len > reserve);
        file.set_len(len - reserve).unwrap();
        file.sync_all().unwrap();
        assert!(fs2::available_space(root).unwrap() <= reserve + 4096);
        Self { file, path }
    }
}
impl Drop for FullDisk {
    fn drop(&mut self) {
        // Only the exact file created by this fixture is removed.
        assert_eq!(self.path.file_name().unwrap(), "pressure.bin");
        self.file.sync_all().unwrap();
        fs::remove_file(&self.path).unwrap();
    }
}
struct FillAfterJournal {
    root: PathBuf,
    hit: AtomicBool,
    pressure: Mutex<Option<FullDisk>>,
}
impl MaterializationFaults for FillAfterJournal {
    fn check(&self, point: MaterializationPoint) -> io::Result<()> {
        if point == MaterializationPoint::AfterJournal && !self.hit.swap(true, Ordering::SeqCst) {
            // Journal already durable; only 64 KiB remains for a 2 MiB target.
            *self.pressure.lock().unwrap() = Some(FullDisk::fill(&self.root, 64 * 1024));
        }
        Ok(())
    }
}
fn clean(workspace: &NativeWorkspace) {
    assert!(
        scm_v1::fsck_existing(workspace.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
#[ignore = "requires the disposable 64 MiB tmpfs Docker recipe"]
fn actual_disk_full_cas_capture_and_checkout_recover_without_lost_heads() {
    let started = std::time::Instant::now();
    // Direct CAS failure cannot publish a truncated immutable object.
    {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let object = CanonicalObject::ChunkRaw(Chunk::new(vec![42; 1024 * 1024]).unwrap());
        let before = workspace.state().unwrap();
        let store = scm_v1::open_exclusive(workspace.store_root()).unwrap();
        let cas = NativeCas::open_exclusive(&store).unwrap();
        let pressure = FullDisk::fill(&fixture.0, 0);
        assert!(
            cas.put(&object).is_err(),
            "CAS must not report installation on ENOSPC"
        );
        assert!(!cas.contains(&object.object_id()).unwrap());
        drop(pressure);
        cas.recover_staging().unwrap();
        cas.put(&object).unwrap();
        cas.verify(&object.object_id()).unwrap();
        drop(cas);
        drop(store);
        assert_eq!(
            workspace.state().unwrap().operation.id(),
            before.operation.id()
        );
        clean(&workspace);
    }
    // Capture refusal preserves the old admitted head and the user's dirty bytes.
    {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let before = workspace.state().unwrap();
        let changed = vec![0x73; 128 * 1024];
        fs::write(fixture.0.join("checkout/data.bin"), &changed).unwrap();
        let pressure = FullDisk::fill(&fixture.0, 0);
        assert!(workspace.capture().is_err());
        assert_eq!(
            fs::read(fixture.0.join("checkout/data.bin")).unwrap(),
            changed
        );
        drop(pressure);
        assert_eq!(
            workspace.state().unwrap().view.layer_heads(),
            before.view.layer_heads()
        );
        assert_eq!(
            workspace.state().unwrap().operation.id(),
            before.operation.id()
        );
        workspace.capture().unwrap();
        assert!(!workspace.is_dirty().unwrap());
        clean(&workspace);
    }
    // Fill after a durable checkout journal: a real filesystem write fails,
    // recovery rolls back, and the identical desired restore can be retried.
    {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let original = workspace.create_marker("original", "").unwrap();
        let target = vec![0x29; 2 * 1024 * 1024];
        fs::write(fixture.0.join("checkout/data.bin"), &target).unwrap();
        workspace.capture().unwrap();
        let large = workspace.create_marker("large", "").unwrap();
        workspace
            .restore_step(original.target_step_id(), None)
            .unwrap();
        let before = workspace.state().unwrap();
        let hook = FillAfterJournal {
            root: fixture.0.clone(),
            hit: AtomicBool::new(false),
            pressure: Mutex::new(None),
        };
        let failure = workspace.restore_step_with_faults(large.target_step_id(), None, &hook);
        assert!(hook.hit.load(Ordering::SeqCst));
        assert!(
            failure.is_err(),
            "checkout must not report success on a full filesystem"
        );
        assert_eq!(
            fs::read(fixture.0.join("checkout/data.bin")).unwrap(),
            b"original bytes"
        );
        assert_eq!(
            fs::read(fixture.0.join("checkout/keep.txt")).unwrap(),
            b"preserved neighbour"
        );
        drop(hook.pressure.lock().unwrap().take());
        let store = scm_v1::open(workspace.store_root()).unwrap();
        assert!(scm_v1::materialize::recovery_pending(&store, workspace.worktree_id()).unwrap());
        drop(store);
        assert_eq!(
            workspace.state().unwrap().operation.id(),
            before.operation.id()
        );
        assert!(workspace.recover().unwrap() > 0);
        assert_eq!(
            workspace.state().unwrap().view.layer_heads(),
            before.view.layer_heads()
        );
        workspace
            .restore_step(large.target_step_id(), None)
            .unwrap();
        assert_eq!(
            fs::read(fixture.0.join("checkout/data.bin")).unwrap(),
            target
        );
        assert!(!workspace.is_dirty().unwrap());
        clean(&workspace);
    }
    eprintln!("Actual Linux ENOSPC: CAS install, native capture admission and journaled checkout failure/recovery passed in {} ms",started.elapsed().as_millis());
}
