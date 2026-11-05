//! Test source with real shared ownership, a queued exclusive acquisition and
//! attempted shared acquisition. No boolean substitutes for try_read/WouldBlock.
use super::*;
use std::sync::{RwLock, TryLockError, mpsc};

#[derive(Default)]
pub(super) struct Witness {
    pub owner_live: AtomicBool,
    pub writer_requested: AtomicBool,
    pub writer_would_block: AtomicBool,
    pub writer_acquired: AtomicBool,
    pub attempts: AtomicUsize,
    pub would_block: AtomicUsize,
    pub active: AtomicUsize,
    pub completed: AtomicUsize,
    pub cancelled: AtomicBool,
    pub hold_cancel: AtomicBool,
    pub allow_cancel_return: AtomicBool,
}
struct Ownership {
    lock: Arc<RwLock<()>>,
    release: Option<mpsc::Sender<()>>,
    owner: Option<std::thread::JoinHandle<()>>,
    writer: Option<std::thread::JoinHandle<()>>,
}
impl Ownership {
    fn new(witness: Arc<Witness>) -> Self {
        let lock = Arc::new(RwLock::new(()));
        let (release, released) = mpsc::channel();
        let (ready, ready_rx) = mpsc::channel();
        let owner_lock = lock.clone();
        let owner_witness = witness.clone();
        let owner = std::thread::spawn(move || {
            let guard = owner_lock.read().unwrap();
            owner_witness.owner_live.store(true, Ordering::Release);
            ready.send(()).unwrap();
            let _ = released.recv();
            drop(guard);
            owner_witness.owner_live.store(false, Ordering::Release);
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let writer_lock = lock.clone();
        let writer_witness = witness.clone();
        let writer = std::thread::spawn(move || {
            match writer_lock.try_write() {
                Err(TryLockError::WouldBlock) => writer_witness
                    .writer_would_block
                    .store(true, Ordering::Release),
                _ => panic!(
                    "exclusive acquisition did not actually block on source lifetime ownership"
                ),
            }
            writer_witness
                .writer_requested
                .store(true, Ordering::Release);
            let _guard = writer_lock.write().unwrap();
            writer_witness
                .writer_acquired
                .store(true, Ordering::Release);
        });
        let value = Self {
            lock,
            release: Some(release),
            owner: Some(owner),
            writer: Some(writer),
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if witness.writer_requested.load(Ordering::Acquire)
                && matches!(value.lock.try_read(), Err(TryLockError::WouldBlock))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "UNQUALIFIED: real queued writer did not cause shared try_read WouldBlock; no simulated contention pass"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        value
    }
}
impl Drop for Ownership {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(owner) = self.owner.take() {
            owner.join().unwrap();
        }
        if let Some(writer) = self.writer.take() {
            writer.join().unwrap();
        }
    }
}

pub(super) struct BlockedSource {
    catalog: Arc<Catalog>,
    catalogs: AtomicUsize,
    ownership: Ownership,
    pub witness: Arc<Witness>,
    // The native callback recipe additionally owns this real private file pin.
    _native_ownership: Option<File>,
}
impl BlockedSource {
    pub fn new(catalog: Arc<Catalog>, native_ownership: Option<File>) -> Arc<Self> {
        let witness = Arc::new(Witness::default());
        let ownership = Ownership::new(witness.clone());
        Arc::new(Self {
            catalog,
            catalogs: AtomicUsize::new(0),
            ownership,
            witness,
            _native_ownership: native_ownership,
        })
    }
}
impl SnapshotSource for BlockedSource {
    fn catalog(&self) -> &Catalog {
        assert_eq!(self.catalogs.fetch_add(1, Ordering::SeqCst), 0);
        &self.catalog
    }
    fn read_exact_at(&self, _: &FileId, _: u64, _: &mut [u8], cancel: &Cancellation) -> Result<()> {
        struct Active<'a>(&'a Witness);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.completed.fetch_add(1, Ordering::Release);
                self.0.active.fetch_sub(1, Ordering::Release);
            }
        }
        self.witness.active.fetch_add(1, Ordering::Release);
        let _active = Active(&self.witness);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if cancel.is_cancelled() {
                self.witness.cancelled.store(true, Ordering::Release);
                // Explicitly bounded test synchronization, still honoring
                // cancellation within at most five seconds. Never detach IO.
                let until = Instant::now() + Duration::from_secs(5);
                while self.witness.hold_cancel.load(Ordering::Acquire)
                    && !self.witness.allow_cancel_return.load(Ordering::Acquire)
                    && Instant::now() < until
                {
                    std::thread::sleep(Duration::from_millis(2));
                }
                return Err(Error::Cancelled);
            }
            self.witness.attempts.fetch_add(1, Ordering::Relaxed);
            match self.ownership.lock.try_read() {
                Err(TryLockError::WouldBlock) => {
                    self.witness.would_block.fetch_add(1, Ordering::Release);
                }
                Err(TryLockError::Poisoned(_)) => {
                    return Err(Error::Unsupported("test acquisition lock poisoned"));
                }
                Ok(_guard) => {
                    return Err(Error::Unsupported(
                        "UNQUALIFIED: queued exclusive owner did not block actual shared acquisition",
                    ));
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
pub(super) fn assert_blocked(witness: &Witness) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while witness.would_block.load(Ordering::Acquire) == 0 {
        assert!(
            Instant::now() < deadline,
            "actual source never observed try_read WouldBlock"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(witness.attempts.load(Ordering::Acquire) > 0);
    assert!(witness.owner_live.load(Ordering::Acquire));
    assert!(witness.writer_requested.load(Ordering::Acquire));
    assert!(witness.writer_would_block.load(Ordering::Acquire));
    assert!(!witness.writer_acquired.load(Ordering::Acquire));
}

pub(super) fn unit_gate() {
    let catalog = catalog(false);
    let source = BlockedSource::new(catalog.clone(), None);
    let witness = source.witness.clone();
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    let worker_source = source.clone();
    let worker = std::thread::spawn(move || {
        let entry = catalog
            .entries()
            .iter()
            .find(|entry| entry.size > 0)
            .unwrap();
        crate::catalog::read_checked(
            worker_source.as_ref(),
            entry,
            0,
            &mut [0u8; 1],
            &worker_cancel,
        )
    });
    assert_blocked(&witness);
    cancel.cancel();
    assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
    assert!(witness.cancelled.load(Ordering::Acquire));
    assert!(witness.owner_live.load(Ordering::Acquire));
    assert!(!witness.writer_acquired.load(Ordering::Acquire));
    assert_eq!(witness.active.load(Ordering::Acquire), 0);
    drop(source);
    assert!(!witness.owner_live.load(Ordering::Acquire));
    assert!(witness.writer_acquired.load(Ordering::Acquire));
}
