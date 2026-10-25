mod worktree_support;
use layrs_client_core::native_scm::*;
use layrs_core::scm::WorktreeStatus;
use layrs_store_local::scm_v1::{
    self,
    checkout::CheckoutReader,
    materialize::{MaterializationFaults, MaterializationPoint, NoMaterializationFaults},
    worktrees::WorktreeCreationLease,
};
use std::{
    fs, io,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Barrier,
    },
    time::Duration,
};
use worktree_support::*;

fn lease(workspace: &NativeWorkspace, root: &Path) -> WorktreeCreationLease {
    let store_id = workspace.open_store().unwrap().store_id().clone();
    let identity = CheckoutReader::open(root, None).unwrap().identity();
    WorktreeCreationLease::acquire(
        workspace.store_root(),
        workspace.store_identity(),
        workspace.metadata_identity(),
        &store_id,
        root,
        &identity,
        |path| Ok(CheckoutReader::open(path, None)?.identity()),
    )
    .unwrap()
}

struct BeforeLease(Barrier);
impl WorktreeCreationFaults for BeforeLease {
    fn before_creation_lease(&self) -> io::Result<()> {
        self.0.wait();
        Ok(())
    }
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn concurrent_same_root_creation_has_one_registration_and_preserves_winner() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let base = own(&primary);
    let root = fixture.directory("same-root");
    let boundary = BeforeLease(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let create = || {
            primary.create_worktree_with_creation_faults(
                &root,
                base.active_layer_id().unwrap(),
                base.base_step_id().unwrap(),
                &NoMaterializationFaults,
                &boundary,
            )
        };
        let left = scope.spawn(create);
        let right = scope.spawn(create);
        (left.join().unwrap(), right.join().unwrap())
    });
    assert_ne!(results.0.is_ok(), results.1.is_ok());
    let (winner, error) = match results {
        (Ok(winner), Err(error)) | (Err(error), Ok(winner)) => (winner, error),
        _ => unreachable!(),
    };
    assert!(
        matches!(
            error,
            NativeClientError::InvalidState("secondary Worktree directory must be empty")
        ),
        "{error:?}"
    );
    let locator = scm_v1::worktrees::read_locator(&root).unwrap();
    assert_eq!(winner.worktree_id(), &locator.worktree_id);
    assert_eq!(primary.list_worktrees().unwrap().len(), 2);
    assert_eq!(own(&winner).status(), WorktreeStatus::Ready);
    assert_eq!(own(&winner).base_step_id(), base.base_step_id());
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_eq!(own(&primary), base);
    assert_clean(&primary, &[base.base_step_id().unwrap().clone()]);
}

struct Stage;
impl WorktreeCreationFaults for Stage {
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
    fn before_locator_publication(&self) -> io::Result<()> {
        Err(io::Error::other("staged reservation"))
    }
}

#[test]
fn concurrent_resume_of_one_staged_locator_keeps_exact_id_and_base() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let base = own(&primary);
    let root = fixture.directory("pending");
    assert!(primary
        .create_worktree_with_creation_faults(
            &root,
            base.active_layer_id().unwrap(),
            base.base_step_id().unwrap(),
            &NoMaterializationFaults,
            &Stage,
        )
        .is_err());
    let bytes = fs::read(root.join(".layrs.reservation")).unwrap();
    let pending = scm_v1::worktrees::read_creation_locator(&root).unwrap();
    let start = Barrier::new(2);
    let (left, right) = std::thread::scope(|scope| {
        let resume = || {
            start.wait();
            NativeWorkspace::resume_worktree(&root)
        };
        let left = scope.spawn(resume);
        let right = scope.spawn(resume);
        (
            left.join().unwrap().unwrap(),
            right.join().unwrap().unwrap(),
        )
    });
    for workspace in [&left, &right] {
        assert_eq!(workspace.worktree_id(), &pending.worktree_id);
        assert_eq!(own(workspace).status(), WorktreeStatus::Ready);
        assert_eq!(own(workspace).base_step_id(), base.base_step_id());
    }
    assert_eq!(primary.list_worktrees().unwrap().len(), 2);
    assert_eq!(fs::read(root.join(".layrs")).unwrap(), bytes);
    assert!(!root.join(".layrs.reservation").exists());
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_clean(&primary, &[base.base_step_id().unwrap().clone()]);
}

#[test]
fn root_creation_lease_waits_beyond_short_store_transaction_budget() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let root = fixture.directory("long-creation");
    let held = lease(&primary, &root);
    let store_id = held.store_id().clone();
    let identity = held.checkout_identity().to_owned();
    std::thread::scope(|scope| {
        let (started, started_rx) = mpsc::channel();
        let (finished, finished_rx) = mpsc::channel();
        let workspace = &primary;
        let root = &root;
        let store_id = &store_id;
        let identity = &identity;
        let worker = scope.spawn(move || {
            let result = WorktreeCreationLease::acquire(
                workspace.store_root(),
                workspace.store_identity(),
                workspace.metadata_identity(),
                store_id,
                root,
                identity,
                |path| {
                    let identity = CheckoutReader::open(path, None)?.identity();
                    started.send(()).unwrap();
                    Ok(identity)
                },
            )
            .and_then(|acquired| acquired.verify());
            finished.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Creation holds the root across physical work, not just a short Store
        // transaction. Its waiter must survive the old five-second deadline.
        let waiting = finished_rx.recv_timeout(Duration::from_secs(8));
        held.verify().unwrap();
        drop(held);
        assert!(
            matches!(waiting, Err(mpsc::RecvTimeoutError::Timeout)),
            "root lease stopped waiting before its owner released it: {waiting:?}"
        );
        finished_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
    });
    assert!(fs::read_dir(root).unwrap().next().is_none());
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
}

#[test]
fn namespace_gate_serializes_different_roots_without_holding_store() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let a = lease(&primary, &fixture.directory("a"));
    let b = lease(&primary, &fixture.directory("b"));
    assert_ne!(a.checkout_identity(), b.checkout_identity());
    let held = a.registration_gate().unwrap();
    std::thread::scope(|scope| {
        let (started, started_rx) = mpsc::channel();
        let (finished, finished_rx) = mpsc::channel();
        let b = &b;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            let result = b.registration_gate().and_then(|gate| gate.verify());
            finished.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let waiting = finished_rx.recv_timeout(Duration::from_millis(50));
        // A normal writer still progresses while a namespace gate is held.
        fs::write(
            primary.checkout_root().unwrap().join("same.txt"),
            b"independent writer",
        )
        .unwrap();
        primary.capture().unwrap();
        held.verify().unwrap();
        drop(held);
        assert!(
            matches!(waiting, Err(mpsc::RecvTimeoutError::Timeout)),
            "gate not exclusive: {waiting:?}"
        );
        finished_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
    });
    a.verify().unwrap();
    b.verify().unwrap();
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
    assert_clean(&primary, &[own(&primary).base_step_id().unwrap().clone()]);
}

struct ProbeGate<'a> {
    lease: &'a WorktreeCreationLease,
    observed: AtomicBool,
}
impl MaterializationFaults for ProbeGate<'_> {
    fn check(&self, point: MaterializationPoint) -> io::Result<()> {
        if point == MaterializationPoint::AfterJournal {
            let gate = self.lease.registration_gate()?;
            gate.verify()?;
            self.observed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[test]
fn ordinary_materialization_releases_namespace_gate() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let base = own(&primary);
    let other = lease(&primary, &fixture.directory("other"));
    let probe = ProbeGate {
        lease: &other,
        observed: AtomicBool::new(false),
    };
    let root = fixture.directory("secondary");
    let secondary = primary
        .create_worktree_with_faults(
            &root,
            base.active_layer_id().unwrap(),
            base.base_step_id().unwrap(),
            &probe,
        )
        .unwrap();
    assert!(probe.observed.load(Ordering::SeqCst));
    assert_eq!(own(&secondary).status(), WorktreeStatus::Ready);
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_clean(&primary, &[base.base_step_id().unwrap().clone()]);
}

struct FullCreationBarriers {
    registered: Barrier,
    metadata: Barrier,
}
impl WorktreeCreationFaults for FullCreationBarriers {
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
    fn after_registration(&self) -> io::Result<()> {
        // Both callbacks run after the short namespace gate was released.
        self.registered.wait();
        Ok(())
    }
}
impl MaterializationFaults for FullCreationBarriers {
    fn check(&self, point: MaterializationPoint) -> io::Result<()> {
        if point == MaterializationPoint::BeforeMetadata {
            self.metadata.wait();
        }
        Ok(())
    }
}

#[test]
fn concurrent_full_materializations_return_exact_cas_and_recover_both_roots() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let base = own(&primary);
    let a = fixture.directory("a");
    let b = fixture.directory("b");
    let barriers = FullCreationBarriers {
        registered: Barrier::new(2),
        metadata: Barrier::new(2),
    };
    let (left, right) = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            primary.create_worktree_with_creation_faults(
                &a,
                base.active_layer_id().unwrap(),
                base.base_step_id().unwrap(),
                &barriers,
                &barriers,
            )
        });
        let right = scope.spawn(|| {
            primary.create_worktree_with_creation_faults(
                &b,
                base.active_layer_id().unwrap(),
                base.base_step_id().unwrap(),
                &barriers,
                &barriers,
            )
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    let errors = [&left, &right].map(|result| result.as_ref().err());
    assert_ne!(
        left.is_ok(),
        right.is_ok(),
        "one exact global CAS must lose: {errors:?}"
    );
    let error = errors.into_iter().flatten().next().unwrap();
    let NativeClientError::Filesystem(error) = error else {
        panic!("unexpected creation failure: {error:?}");
    };
    assert!(
        matches!(
            error
                .get_ref()
                .and_then(|source| source.downcast_ref::<layrs_store::scm::StoreError>()),
            Some(layrs_store::scm::StoreError::CasMismatch { .. })
        ),
        "expected typed materialization CAS, not lock/admission fallback: {error:?}"
    );
    assert_eq!(primary.list_worktrees().unwrap().len(), 3);
    let mut repairs = 0;
    for root in [&a, &b] {
        let locator = scm_v1::worktrees::read_locator(root).unwrap();
        let workspace = NativeWorkspace::open(root).unwrap();
        assert_eq!(workspace.worktree_id(), &locator.worktree_id);
        if workspace.needs_repair().unwrap() {
            repairs += 1;
            assert_eq!(own(&workspace).status(), WorktreeStatus::Stale);
            assert!(matches!(
                NativeWorkspace::resume_worktree(root),
                Err(NativeClientError::NeedsRepair)
            ));
            workspace.recover().unwrap();
        }
        let resumed = NativeWorkspace::resume_worktree(root).unwrap();
        assert_eq!(resumed.worktree_id(), &locator.worktree_id);
        assert_eq!(own(&resumed).status(), WorktreeStatus::Ready);
        assert_eq!(own(&resumed).base_step_id(), base.base_step_id());
        assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    }
    assert_eq!(
        repairs, 1,
        "only the rejected materialization retains a journal"
    );
    assert_eq!(own(&primary), base);
    assert_clean(&primary, &[base.base_step_id().unwrap().clone()]);
}
