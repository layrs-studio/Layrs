//! Deterministic cancellation while a real authenticated TLS download is in flight.
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
};
use layrs_client_core::native_scm::{
    NativeRemote, NativeRemoteError, NativeTransferCancelStatus, NativeTransferControl,
    NativeWorkspace,
};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub struct Probe {
    armed: AtomicBool,
    hit: AtomicBool,
    release: tokio::sync::Semaphore,
}
impl Probe {
    fn arm(&self) {
        self.hit.store(false, Ordering::Release);
        self.armed.store(true, Ordering::Release);
    }
    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.hit.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "client did not reach an actual download"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn resume(&self) {
        self.release.add_permits(1);
    }
}
pub fn wrap(app: Router) -> (Router, Arc<Probe>) {
    let probe = Arc::new(Probe {
        armed: AtomicBool::new(false),
        hit: AtomicBool::new(false),
        release: tokio::sync::Semaphore::new(0),
    });
    let state = probe.clone();
    let app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let state = state.clone();
        async move {
            if request.method() == axum::http::Method::GET
                && request.uri().path() == "/v3/transfers/downloads"
                && state.armed.swap(false, Ordering::AcqRel)
            {
                state.hit.store(true, Ordering::Release);
                if let Ok(Ok(permit)) =
                    tokio::time::timeout(Duration::from_secs(25), state.release.acquire()).await
                {
                    permit.forget();
                }
            }
            next.run(request).await
        }
    }));
    (app, probe)
}

pub fn run(remote: &NativeRemote, root: &Path, probe: &Probe) {
    let source_root = root.join("cancel-source");
    let dest = root.join("cancel-destination");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(source_root.join("contents.txt"), b"before cancellation").unwrap();
    let source = NativeWorkspace::initialize(&source_root, "Cancellation fixture").unwrap();
    let project = remote
        .provision(&source, "cancellation", "project")
        .unwrap();
    assert_eq!(
        remote
            .publish(&source, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let control = NativeTransferControl::new();
    probe.arm();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            remote.clone_into_cancellable(&dest, &project.space_id, "Clone cancellation", &control)
        });
        probe.wait();
        assert_eq!(
            control.request_cancel(),
            NativeTransferCancelStatus::CancelRequested
        );
        probe.resume();
        assert!(matches!(
            worker.join().unwrap(),
            Err(NativeRemoteError::Cancelled)
        ));
    });
    assert!(!dest.join(".layrs").exists());
    assert!(
        std::fs::read_dir(&dest).unwrap().next().is_none(),
        "cancelled clone leaves target untouched"
    );
    let destination = remote
        .clone_into_cancellable(
            &dest,
            &project.space_id,
            "Clone retry",
            &NativeTransferControl::new(),
        )
        .unwrap();
    std::fs::write(source_root.join("contents.txt"), b"after cancellation").unwrap();
    source.capture().unwrap();
    assert_eq!(
        remote
            .publish(&source, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let before = destination.state().unwrap();
    let control = NativeTransferControl::new();
    probe.arm();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| remote.receive_cancellable(&destination, &control));
        probe.wait();
        assert_eq!(
            control.request_cancel(),
            NativeTransferCancelStatus::CancelRequested
        );
        probe.resume();
        assert!(matches!(
            worker.join().unwrap(),
            Err(NativeRemoteError::Cancelled)
        ));
    });
    assert_eq!(
        destination.state().unwrap().operation.id(),
        before.operation.id()
    );
    assert_eq!(destination.state().unwrap().view, before.view);
    assert_eq!(
        std::fs::read(dest.join("contents.txt")).unwrap(),
        b"before cancellation"
    );
    // User edits made during download must be refused before any capture/commit.
    let control = NativeTransferControl::new();
    probe.arm();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| remote.receive_cancellable(&destination, &control));
        probe.wait();
        std::fs::write(dest.join("contents.txt"), b"user edit during HTTP").unwrap();
        probe.resume();
        assert!(matches!(
            worker.join().unwrap(),
            Err(NativeRemoteError::Invalid(_))
        ));
    });
    assert_eq!(
        destination.state().unwrap().operation.id(),
        before.operation.id()
    );
    assert_eq!(
        std::fs::read(dest.join("contents.txt")).unwrap(),
        b"user edit during HTTP"
    );
    assert!(
        remote
            .receive_cancellable(&destination, &NativeTransferControl::new())
            .is_err()
    );
    assert_eq!(
        destination.state().unwrap().operation.id(),
        before.operation.id()
    );
    std::fs::write(dest.join("contents.txt"), b"before cancellation").unwrap();
    remote
        .receive_cancellable(&destination, &NativeTransferControl::new())
        .unwrap();
    assert_eq!(
        std::fs::read(dest.join("contents.txt")).unwrap(),
        b"after cancellation"
    );
    // A concurrent native write succeeds during HTTP, then stale receive refuses
    // to replace that new user Operation after regaining its journal lock.
    std::fs::write(source_root.join("contents.txt"), b"third server revision").unwrap();
    source.capture().unwrap();
    assert_eq!(
        remote
            .publish(&source, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let control = NativeTransferControl::new();
    probe.arm();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| remote.receive_cancellable(&destination, &control));
        probe.wait();
        std::fs::write(dest.join("local.txt"), b"concurrent local capture").unwrap();
        destination.capture().unwrap();
        let operation = destination.state().unwrap().operation.id().clone();
        probe.resume();
        assert!(worker.join().unwrap().is_err());
        assert_eq!(destination.state().unwrap().operation.id(), &operation);
    });
    assert_eq!(
        std::fs::read(dest.join("local.txt")).unwrap(),
        b"concurrent local capture"
    );
    eprintln!(
        "TLS cancellation: in-flight clone/receive cancelled before mutation, clean retry, dirty/stale refusal without implicit capture, concurrent native write during HTTP passed"
    );
}
