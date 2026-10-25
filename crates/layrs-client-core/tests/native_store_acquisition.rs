mod worktree_support;
use layrs_client_core::native_scm::*;
use layrs_core::scm::{OperationId, StepId};
use layrs_store::scm::{ScmReader, ScmRepository, StoreError, StoreObjectId};
use layrs_store_local::scm_v1::{
    NativeScmReader, NativeScmRepository, RepositoryFaultInjector, RepositoryFaultPoint,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use worktree_support::*;

fn wait_for(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.try_exists()? {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("waiting for {}", path.display()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

struct WriterProcess(Option<Child>);
impl WriterProcess {
    fn finish(mut self) {
        let child = self.0.as_mut().unwrap();
        let deadline = Instant::now() + Duration::from_secs(75);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "acceptance helper timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = self.0.take().unwrap().wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
impl Drop for WriterProcess {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct HoldAfterCas(PathBuf);
impl RepositoryFaultInjector for HoldAfterCas {
    fn check(&self, point: RepositoryFaultPoint) -> std::result::Result<(), String> {
        if point == RepositoryFaultPoint::AfterAllCas {
            fs::write(self.0.join("held"), b"exclusive acceptance held")
                .map_err(|e| e.to_string())?;
            wait_for(&self.0.join("release")).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[test]
#[ignore = "spawned only by long_acceptance_reaches_exact_cas_and_read_observes_commit"]
fn held_acceptance_worker() {
    let root = PathBuf::from(std::env::var_os("LAYRS_HELD_ACCEPTANCE_ROOT").expect("helper root"));
    let control =
        PathBuf::from(std::env::var_os("LAYRS_HELD_ACCEPTANCE_CONTROL").expect("helper control"));
    let workspace = NativeWorkspace::open(&root).unwrap();
    let state = workspace.state().unwrap();
    let base = workspace
        .client()
        .capture_base(workspace.worktree_id())
        .unwrap();
    // In-memory snapshot keeps every Chunk in the batch so the real repository
    // writer, with its AfterAllCas seam, performs the complete acceptance.
    let (_, snapshot) = capture_checkout(
        &root,
        Some(state.worktree.filesystem_identity()),
        SnapshotLimits::default(),
    )
    .unwrap();
    let CapturePlan::Change(change) = workspace
        .client()
        .prepare_capture(&base, snapshot, MutationStamp::fresh().unwrap())
        .unwrap()
    else {
        panic!("expected changed writer checkout")
    };
    let mut repository = NativeScmRepository::with_fault_injector(
        workspace.store_root(),
        Arc::new(HoldAfterCas(control.clone())),
    )
    .with_store_identity(workspace.store_identity())
    .with_metadata_identity(workspace.metadata_identity())
    .with_store_id(state.worktree.store_id().clone());
    repository.apply_batch(change.batch()).unwrap();
    fs::write(
        control.join("committed.json"),
        serde_json::to_vec(&serde_json::json!({
            "pid": std::process::id(), "step": change.step_id(),
            "operation": workspace.state().unwrap().operation.id(),
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn long_acceptance_reaches_exact_cas_and_read_observes_commit() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let initial = own(&primary);
    let unrelated = primary
        .create_worktree(
            fixture.directory("unrelated"),
            initial.active_layer_id().unwrap(),
            initial.base_step_id().unwrap(),
        )
        .unwrap();
    fs::write(
        unrelated.checkout_root().unwrap().join("same.txt"),
        b"unrelated capture",
    )
    .unwrap();
    unrelated.capture().unwrap();
    let unrelated_before = unrelated.state().unwrap();
    let primary_before = primary.state().unwrap();
    let secondary_before = secondary.state().unwrap();
    let root_a = primary.checkout_root().unwrap();
    let root_b = secondary.checkout_root().unwrap();
    fs::write(root_a.join("same.txt"), b"accepted by held writer").unwrap();
    fs::write(root_b.join("same.txt"), b"explicitly recaptured after CAS").unwrap();
    let CapturePlan::Change(prepared) = secondary
        .client()
        .prepare_disk_capture(
            secondary.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected competing capture")
    };
    let digest = prepared.batch().request_digest();
    let transaction = prepared.batch().transaction_id().clone();
    let control = fixture.directory("control");
    let child = WriterProcess(Some(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "held_acceptance_worker",
                "--nocapture",
            ])
            .env("LAYRS_HELD_ACCEPTANCE_ROOT", &root_a)
            .env("LAYRS_HELD_ACCEPTANCE_CONTROL", &control)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    wait_for(&control.join("held")).unwrap();
    let (acceptance, observed) = std::thread::scope(|scope| {
        let (started, started_rx) = mpsc::channel();
        let (finished, finished_rx) = mpsc::channel();
        let acceptance = {
            let started = started.clone();
            let finished = finished.clone();
            let workspace = &secondary;
            let prepared = &prepared;
            scope.spawn(move || {
                started.send("accept").unwrap();
                let result = workspace.client().accept(prepared);
                finished.send("accept").unwrap();
                result
            })
        };
        let read = {
            let workspace = &primary;
            scope.spawn(move || {
                started.send("read").unwrap();
                let result = workspace.state();
                finished.send("read").unwrap();
                result
            })
        };
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // A genuine cross-process exclusive holder outlasts the old five-second
        // acquisition deadline. Neither waiter may report success or failure yet.
        let premature = finished_rx.recv_timeout(Duration::from_secs(7));
        fs::write(control.join("release"), b"commit now").unwrap();
        child.finish();
        let acceptance = acceptance.join().unwrap();
        let observed = read.join().unwrap();
        assert!(
            matches!(premature, Err(mpsc::RecvTimeoutError::Timeout)),
            "waiter ended before holder release: {premature:?}"
        );
        (acceptance, observed)
    });
    let error = acceptance.unwrap_err();
    assert!(
        matches!(
            error,
            NativeClientError::Store(StoreError::CasMismatch { .. })
        ),
        "acceptance must reach exact CAS after acquiring the Store: {error:?}"
    );
    assert_eq!(prepared.batch().request_digest(), digest);
    assert_eq!(prepared.batch().transaction_id(), &transaction);
    let committed: serde_json::Value =
        serde_json::from_slice(&fs::read(control.join("committed.json")).unwrap()).unwrap();
    assert_ne!(
        committed["pid"].as_u64().unwrap(),
        u64::from(std::process::id())
    );
    let operation_a = OperationId::parse(committed["operation"].as_str().unwrap()).unwrap();
    let step_a = StepId::parse(committed["step"].as_str().unwrap()).unwrap();
    assert_eq!(observed.unwrap().operation.id(), &operation_a);
    assert_eq!(
        secondary.state().unwrap().operation.id(),
        secondary_before.operation.id()
    );
    assert_eq!(
        unrelated.state().unwrap().operation.id(),
        unrelated_before.operation.id()
    );
    {
        let store = primary.open_store().unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        assert!(!reader
            .object_admitted_to_space(
                &StoreObjectId::Step(prepared.step_id().clone()),
                primary_before.space.id()
            )
            .unwrap());
    }
    // This is a new preparation, never a retry of the original immutable batch.
    let CapturePlan::Change(fresh) = secondary
        .client()
        .prepare_disk_capture(
            secondary.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected explicit recapture")
    };
    assert_ne!(fresh.batch().transaction_id(), &transaction);
    secondary.client().accept(&fresh).unwrap();
    let reopened_a = NativeWorkspace::open(&root_a).unwrap();
    let reopened_b = NativeWorkspace::open(&root_b).unwrap();
    let a = reopened_a.state().unwrap();
    let b = reopened_b.state().unwrap();
    assert_eq!(
        a.operation.parent_operation_ids(),
        &[primary_before.operation.id().clone()]
    );
    assert_eq!(
        b.operation.parent_operation_ids(),
        &[secondary_before.operation.id().clone()]
    );
    assert_eq!(
        fs::read(root_a.join("same.txt")).unwrap(),
        b"accepted by held writer"
    );
    assert_eq!(
        fs::read(root_b.join("same.txt")).unwrap(),
        b"explicitly recaptured after CAS"
    );
    assert_eq!(
        fs::read(unrelated.checkout_root().unwrap().join("same.txt")).unwrap(),
        b"unrelated capture"
    );
    let store = primary.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let heads = reader.operation_heads(primary_before.space.id()).unwrap();
    assert_eq!(heads.len(), 3);
    for operation in [
        a.operation.id(),
        b.operation.id(),
        unrelated_before.operation.id(),
    ] {
        assert!(heads.contains(operation));
    }
    for step in [&step_a, fresh.step_id()] {
        assert_eq!(
            reader.get_step(step).unwrap().unwrap().parent_step_ids(),
            &[initial.base_step_id().unwrap().clone()]
        );
    }
    drop(reader);
    drop(store);
    assert_clean(
        &primary,
        &[
            initial.base_step_id().unwrap().clone(),
            step_a,
            fresh.step_id().clone(),
            own(&unrelated).base_step_id().unwrap().clone(),
        ],
    );
}
