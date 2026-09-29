//! Real TLS upload cancellation and the exact durable publication boundary.
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
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

pub struct Probe {
    phase: AtomicU8,
    hit: AtomicBool,
    release: tokio::sync::Semaphore,
}
impl Probe {
    fn arm(&self, phase: u8) {
        self.hit.store(false, Ordering::Release);
        self.phase.store(phase, Ordering::Release);
    }
    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.hit.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "publication did not reach expected HTTP boundary"
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
        phase: AtomicU8::new(0),
        hit: AtomicBool::new(false),
        release: tokio::sync::Semaphore::new(0),
    });
    let state = probe.clone();
    let app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let state = state.clone();
        async move {
            let phase = if request.method() == axum::http::Method::PUT
                && request.uri().path() == "/v3/transfers/uploads"
            {
                1
            } else if request.method() == axum::http::Method::POST
                && request.uri().path().ends_with("/transactions")
            {
                2
            } else {
                0
            };
            if phase != 0
                && state
                    .phase
                    .compare_exchange(phase, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
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
fn journal(workspace: &NativeWorkspace) -> Vec<u8> {
    std::fs::read(workspace.store_root().join("native-remote.json")).unwrap()
}

pub fn run(remote: &NativeRemote, root: &Path, probe: &Probe, lose_response: &AtomicBool) {
    let directory = root.join("publication-cancellation");
    std::fs::create_dir(&directory).unwrap();
    let file = directory.join("content");
    std::fs::write(&file, b"initial published bytes").unwrap();
    let workspace = NativeWorkspace::initialize(&directory, "Publication cancellation").unwrap();
    let project = remote
        .provision(&workspace, "publication-cancellation", "project")
        .unwrap();
    assert_eq!(
        remote
            .publish_cancellable(
                &workspace,
                LayerMutationMode::Advance,
                &NativeTransferControl::new()
            )
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let observed = remote.bootstrap(&project.space_id).unwrap();
    let before = journal(&workspace);
    let operation = workspace.state().unwrap().operation.id().clone();
    // Dirty publication never implicitly creates a Capture.
    std::fs::write(&file, b"unsaved user edits").unwrap();
    assert!(matches!(
        remote.publish_cancellable(
            &workspace,
            LayerMutationMode::Advance,
            &NativeTransferControl::new()
        ),
        Err(NativeRemoteError::Invalid(_))
    ));
    assert_eq!(workspace.state().unwrap().operation.id(), &operation);
    assert_eq!(journal(&workspace), before);
    workspace.capture().unwrap();
    let control = NativeTransferControl::new();
    control.request_cancel();
    assert!(matches!(
        remote.publish_cancellable(&workspace, LayerMutationMode::Advance, &control),
        Err(NativeRemoteError::Cancelled)
    ));
    assert_eq!(journal(&workspace), before);
    // Both public entrypoints stop an actual authenticated upload before a
    // transaction journal is created. Uploaded CAS objects may remain orphaned.
    for candidate in [false, true] {
        std::fs::write(&file, format!("unique upload {candidate}")).unwrap();
        workspace.capture().unwrap();
        let operation = workspace.state().unwrap().operation.id().clone();
        let control = NativeTransferControl::new();
        probe.arm(1);
        std::thread::scope(|scope| {
            let task = scope.spawn(|| {
                if candidate {
                    remote.publish_candidates_cancellable(&workspace, &control)
                } else {
                    remote.publish_cancellable(&workspace, LayerMutationMode::Advance, &control)
                }
            });
            probe.wait();
            assert_eq!(
                control.request_cancel(),
                NativeTransferCancelStatus::CancelRequested
            );
            probe.resume();
            assert!(matches!(
                task.join().unwrap(),
                Err(NativeRemoteError::Cancelled)
            ));
        });
        assert_eq!(workspace.state().unwrap().operation.id(), &operation);
        assert_eq!(journal(&workspace), before);
        let after = remote.bootstrap(&project.space_id).unwrap();
        assert_eq!(
            after.current_repository_view,
            observed.current_repository_view
        );
        assert_eq!(
            after.current_repository_view_generation,
            observed.current_repository_view_generation
        );
    }
    // HTTP does not hold the native writer: a concurrent Capture succeeds and
    // the stale prepared request refuses to journal or move remote references.
    std::fs::write(&file, b"upload before concurrent capture").unwrap();
    workspace.capture().unwrap();
    let control = NativeTransferControl::new();
    probe.arm(1);
    std::thread::scope(|scope| {
        let task = scope
            .spawn(|| remote.publish_cancellable(&workspace, LayerMutationMode::Advance, &control));
        probe.wait();
        std::fs::write(&file, b"concurrent captured bytes").unwrap();
        workspace.capture().unwrap();
        let operation = workspace.state().unwrap().operation.id().clone();
        probe.resume();
        assert!(task.join().unwrap().is_err());
        assert_eq!(workspace.state().unwrap().operation.id(), &operation);
    });
    assert_eq!(journal(&workspace), before);
    // Once the journal is durable, cancellation is TooLate even before the
    // server receives a transaction. Simulate a committed, lost HTTP response.
    let control = NativeTransferControl::new();
    probe.arm(2);
    lose_response.store(true, Ordering::SeqCst);
    let pending = std::thread::scope(|scope| {
        let task = scope
            .spawn(|| remote.publish_cancellable(&workspace, LayerMutationMode::Advance, &control));
        probe.wait();
        assert_eq!(
            control.request_cancel(),
            NativeTransferCancelStatus::TooLate
        );
        let pending = journal(&workspace);
        let value: serde_json::Value = serde_json::from_slice(&pending).unwrap();
        assert!(value["pending"].is_object());
        probe.resume();
        assert!(matches!(
            task.join().unwrap(),
            Err(NativeRemoteError::Http(502))
        ));
        pending
    });
    assert_eq!(journal(&workspace), pending);
    let committed = remote.bootstrap(&project.space_id).unwrap();
    assert!(
        committed.current_repository_view_generation > observed.current_repository_view_generation
    );
    let cancelled = NativeTransferControl::new();
    cancelled.request_cancel();
    assert!(matches!(
        remote.publish_cancellable(&workspace, LayerMutationMode::Advance, &cancelled),
        Err(NativeRemoteError::PublicationPending)
    ));
    assert_eq!(journal(&workspace), pending);
    let receipt = remote
        .publish_cancellable(
            &workspace,
            LayerMutationMode::Advance,
            &NativeTransferControl::new(),
        )
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Applied);
    let request: serde_json::Value = serde_json::from_slice(&pending).unwrap();
    assert_eq!(
        serde_json::to_value(&receipt).unwrap()["requestDigest"],
        request["pending"]["requestDigest"]
    );
    assert_eq!(
        remote
            .bootstrap(&project.space_id)
            .unwrap()
            .current_repository_view_generation,
        committed.current_repository_view_generation
    );
    // Candidate publication follows the same boundary but never mutates View.
    std::fs::write(&file, b"candidate only").unwrap();
    workspace.capture().unwrap();
    let control = NativeTransferControl::new();
    probe.arm(2);
    std::thread::scope(|scope| {
        let task = scope.spawn(|| remote.publish_candidates_cancellable(&workspace, &control));
        probe.wait();
        assert_eq!(
            control.request_cancel(),
            NativeTransferCancelStatus::TooLate
        );
        let request: serde_json::Value = serde_json::from_slice(&journal(&workspace)).unwrap();
        assert!(request["pending"]["viewMutation"].is_null());
        probe.resume();
        assert_eq!(task.join().unwrap().unwrap().status, ReceiptStatus::Applied);
    });
    assert_eq!(
        remote
            .bootstrap(&project.space_id)
            .unwrap()
            .current_repository_view_generation,
        committed.current_repository_view_generation
    );
    eprintln!(
        "TLS publication cancellation: both uploads cancelled, no implicit Capture, concurrent writer/stale refusal, durable TooLate, lost response exact replay and candidate View preservation passed"
    );
}
