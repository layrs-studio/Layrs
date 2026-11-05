//! Hard-kill client processes immediately before transaction dispatch and after
//! the server commit, then replay the exact disk journal in a new process.
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::Response,
};
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

pub struct Probe {
    phase: AtomicU8,
    hit: AtomicBool,
    release: AtomicBool,
    receipt: Mutex<Option<serde_json::Value>>,
}
pub fn wrap(app: Router) -> (Router, Arc<Probe>) {
    let probe = Arc::new(Probe {
        phase: AtomicU8::new(0),
        hit: AtomicBool::new(false),
        release: AtomicBool::new(false),
        receipt: Mutex::new(None),
    });
    let state = probe.clone();
    let app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let state = state.clone();
        async move {
            let phase = if request.uri().path().ends_with("/transactions") {
                state.phase.swap(0, Ordering::SeqCst)
            } else {
                0
            };
            if phase == 1 {
                hold(&state).await;
                // This request was captured before dispatch; killing the client
                // cannot race an accidental commit when the fixture releases it.
                return Response::builder()
                    .status(502)
                    .body(axum::body::Body::empty())
                    .unwrap();
            }
            let response = next.run(request).await;
            if phase == 2 {
                assert!(response.status().is_success());
                let (parts, body) = response.into_parts();
                let bytes = axum::body::to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                *state.receipt.lock().unwrap() = Some(serde_json::from_slice(&bytes).unwrap());
                hold(&state).await;
                return Response::from_parts(parts, axum::body::Body::from(bytes));
            }
            response
        }
    }));
    (app, probe)
}
async fn hold(probe: &Probe) {
    probe.hit.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(25);
    while !probe.release.load(Ordering::Acquire) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
fn spawn(path: &Path, origin: &str, token: &str, ca: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "publication_restart_child",
            "--ignored",
            "--nocapture",
        ])
        .env("LAYRS_PUBLICATION_CHILD_ROOT", path)
        .env("LAYRS_PUBLICATION_CHILD_ORIGIN", origin)
        .env("LAYRS_PUBLICATION_CHILD_TOKEN", token)
        .env("LAYRS_PUBLICATION_CHILD_CA", ca)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}
fn journal(workspace: &NativeWorkspace) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(workspace.store_root().join("native-remote.json")).unwrap(),
    )
    .unwrap()
}
pub fn run(remote: &NativeRemote, root: &Path, origin: &str, token: &str, ca: &str, probe: &Probe) {
    let path = root.join("publication-process-restart");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("content"), b"initial").unwrap();
    let workspace = NativeWorkspace::initialize(&path, "Restart").unwrap();
    let provision = remote.provision(&workspace, "restart", "restart").unwrap();
    assert_eq!(
        remote
            .publish(&workspace, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    for phase in [1, 2] {
        let bytes = format!("phase {phase}: exact saved work");
        std::fs::write(path.join("content"), bytes.as_bytes()).unwrap();
        workspace.capture().unwrap();
        let before = remote.bootstrap(&provision.space_id).unwrap();
        let operation = workspace.state().unwrap().operation.id().clone();
        probe.hit.store(false, Ordering::SeqCst);
        probe.release.store(false, Ordering::SeqCst);
        *probe.receipt.lock().unwrap() = None;
        probe.phase.store(phase, Ordering::SeqCst);
        let mut child = spawn(&path, origin, token, ca);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !probe.hit.load(Ordering::Acquire) {
            if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("publication child did not reach fixture boundary; output withheld");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let pending = journal(&workspace)["pending"].clone();
        child.kill().unwrap();
        child.wait().unwrap();
        probe.release.store(true, Ordering::SeqCst);
        assert!(
            pending.is_object(),
            "journal durable before HTTP request, phase {phase}"
        );
        assert_eq!(workspace.state().unwrap().operation.id(), &operation);
        assert_eq!(
            std::fs::read(path.join("content")).unwrap(),
            bytes.as_bytes()
        );
        let interrupted = remote.bootstrap(&provision.space_id).unwrap();
        assert_eq!(
            interrupted.current_repository_view_generation,
            before.current_repository_view_generation + u64::from(phase == 2)
        );
        assert_eq!(journal(&workspace)["pending"], pending);
        let mut retry = spawn(&path, origin, token, ca);
        assert!(
            retry.wait().unwrap().success(),
            "new publication process failed; output withheld"
        );
        let journal = journal(&NativeWorkspace::open(&path).unwrap());
        assert!(journal["pending"].is_null());
        assert_eq!(
            journal["lastReceipt"]["idempotencyKey"],
            pending["idempotencyKey"]
        );
        if phase == 2 {
            assert_eq!(
                journal["lastReceipt"],
                probe.receipt.lock().unwrap().clone().unwrap(),
                "exact persisted server receipt replay"
            );
        }
        let after = remote.bootstrap(&provision.space_id).unwrap();
        assert_eq!(
            after.current_repository_view_generation,
            before.current_repository_view_generation + 1
        );
        assert_eq!(
            std::fs::read(path.join("content")).unwrap(),
            bytes.as_bytes()
        );
        assert!(workspace.working_diff().unwrap().is_empty());
    }
    eprintln!(
        "Publication process restart PASS: hard kill before dispatch and after commit, exact journal/receipt, one View advance"
    );
}
pub fn child() {
    let Some(root) = std::env::var_os("LAYRS_PUBLICATION_CHILD_ROOT") else {
        return;
    };
    let value = |name| std::env::var(name).unwrap();
    let remote = NativeRemote::with_trusted_ca(
        &value("LAYRS_PUBLICATION_CHILD_ORIGIN"),
        &value("LAYRS_PUBLICATION_CHILD_TOKEN"),
        value("LAYRS_PUBLICATION_CHILD_CA").as_bytes(),
    )
    .unwrap();
    let workspace = NativeWorkspace::open(root).unwrap();
    assert_eq!(
        remote
            .publish(&workspace, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
}
