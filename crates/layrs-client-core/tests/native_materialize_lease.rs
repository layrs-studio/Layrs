//! Virtual wall time advances over multiple five-minute leases while real files
//! are installed/restored. No sleeps and no production TTL override.
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_core::scm::StepId;
use layrs_store_local::scm_v1::{
    self,
    materialize::{MaterializationFaults, MaterializationPoint},
    NativeRecovery,
};
use std::{
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicI64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("layrs-materialize-lease-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn files(&self, old: bool) {
        for i in 0..12 {
            fs::write(
                self.0.join(format!("file-{i:02}")),
                if old { b"old bytes" } else { b"new bytes" },
            )
            .unwrap();
        }
    }
    fn assert_files(&self, old: bool) {
        for i in 0..12 {
            assert_eq!(
                fs::read(self.0.join(format!("file-{i:02}"))).unwrap(),
                if old { b"old bytes" } else { b"new bytes" }
            );
        }
    }
    fn prepare(&self) -> (NativeWorkspace, StepId) {
        self.files(true);
        let w = NativeWorkspace::initialize(&self.0, "Lease fixture").unwrap();
        let target = w.create_marker("old", "").unwrap().target_step_id().clone();
        self.files(false);
        w.capture().unwrap();
        (w, target)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
struct Clock {
    now: AtomicI64,
    stop: Option<MaterializationPoint>,
    stopped: AtomicBool,
}
impl Clock {
    fn new(stop: Option<MaterializationPoint>) -> Self {
        Self {
            now: AtomicI64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            ),
            stop,
            stopped: AtomicBool::new(false),
        }
    }
}
impl MaterializationFaults for Clock {
    fn lease_time_ms(&self) -> io::Result<i64> {
        Ok(self.now.fetch_add(31_000, Ordering::SeqCst))
    }
    fn check(&self, point: MaterializationPoint) -> io::Result<()> {
        if self.stop == Some(point) && !self.stopped.swap(true, Ordering::SeqCst) {
            Err(io::Error::other("interrupted virtual-time checkout"))
        } else {
            Ok(())
        }
    }
}
fn assert_renewed(w: &NativeWorkspace) {
    let db = rusqlite::Connection::open(w.store_root().join("metadata/layrs.sqlite3")).unwrap();
    let (elapsed,pending):(i64,i64)=db.query_row("SELECT min(heartbeat_at_ms-acquired_at_ms),sum(released_at_ms IS NULL) FROM materialization_leases",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert!(
        elapsed > 300_000,
        "the recipe must span more than the original lease"
    );
    assert_eq!(pending, 0);
    assert!(
        scm_v1::fsck_existing(w.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}
#[test]
fn checkout_and_both_recovery_directions_renew_over_multiple_lease_periods() {
    for stop in [
        None,
        Some(MaterializationPoint::BeforeMetadata),
        Some(MaterializationPoint::AfterMetadata),
    ] {
        let f = Fixture::new();
        let (w, target) = f.prepare();
        let old = w.state().unwrap().operation.id().clone();
        let clock = Clock::new(stop);
        let result = w.restore_step_with_faults(&target, None, &clock);
        if let Some(point) = stop {
            assert!(result.is_err());
            assert!(w.needs_repair().unwrap());
            assert_eq!(
                scm_v1::materialize::recover_checkout_with_faults(
                    w.store_root(),
                    w.worktree_id(),
                    &clock
                )
                .unwrap(),
                1
            );
            f.assert_files(point == MaterializationPoint::AfterMetadata);
            if point == MaterializationPoint::BeforeMetadata {
                assert_eq!(w.state().unwrap().operation.id(), &old);
            }
        } else {
            result.unwrap();
            f.assert_files(true);
            assert_ne!(w.state().unwrap().operation.id(), &old);
        }
        assert_renewed(&w);
        assert!(!w.needs_repair().unwrap());
    }
}

struct LoseLease {
    clock: Clock,
    store: PathBuf,
    mode: u8,
    hit: AtomicBool,
}
impl MaterializationFaults for LoseLease {
    fn lease_time_ms(&self) -> io::Result<i64> {
        self.clock.lease_time_ms()
    }
    fn check(&self, point: MaterializationPoint) -> io::Result<()> {
        if point != MaterializationPoint::AfterBackup || self.hit.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let mut store = scm_v1::open_exclusive(&self.store).unwrap();
        let mut journal = NativeRecovery::new(&mut store).unwrap();
        let record = journal.list_pending().unwrap().remove(0);
        // Advance beyond expiry then exercise the real claim API. Mode0 leaves
        // the original owner expired; modes1/2 change owner or generation.
        let takeover_time = self.clock.now.load(Ordering::SeqCst) + 300_001;
        self.clock.now.store(takeover_time, Ordering::SeqCst);
        if self.mode != 0 {
            let owner = if self.mode == 1 {
                "competing-owner"
            } else {
                record.owner_token.as_deref().unwrap()
            };
            journal
                .claim(&record.id, owner, takeover_time, 300_000)
                .unwrap();
        }
        Ok(())
    }
}
#[test]
fn expired_or_reclaimed_lease_stops_before_next_filesystem_mutation() {
    for mode in 0..3 {
        let f = Fixture::new();
        let (w, target) = f.prepare();
        let old = w.state().unwrap().operation.id().clone();
        let faults = LoseLease {
            clock: Clock::new(None),
            store: w.store_root().to_path_buf(),
            mode,
            hit: AtomicBool::new(false),
        };
        assert!(w.restore_step_with_faults(&target, None, &faults).is_err());
        assert!(faults.hit.load(Ordering::SeqCst));
        assert_eq!(w.state().unwrap().operation.id(), &old);
        assert!(w.needs_repair().unwrap());
        assert!(
            !f.0.join("file-00").exists(),
            "first original was backed up before loss"
        );
        for i in 1..12 {
            assert_eq!(
                fs::read(f.0.join(format!("file-{i:02}"))).unwrap(),
                b"new bytes",
                "no later path may be mutated after lease loss"
            );
        }
        assert_eq!(
            scm_v1::materialize::recover_checkout_with_faults(
                w.store_root(),
                w.worktree_id(),
                &faults.clock
            )
            .unwrap(),
            1
        );
        f.assert_files(false);
        assert_eq!(w.state().unwrap().operation.id(), &old);
        assert_renewed(&w);
    }
}
