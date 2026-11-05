//! Authenticated unchanged receive: real TLS requests, immutable witness and races.
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
};
use layrs_client_core::native_scm::{
    NativeClientError, NativeRemote, NativeRemoteError, NativeTransferCancelStatus,
    NativeTransferControl, NativeWorkspace,
};
use layrs_sync::v3::*;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub struct Probe {
    downloads: AtomicUsize,
    plans: AtomicUsize,
    bootstraps: AtomicUsize,
    hold_at: AtomicUsize,
    hit: AtomicBool,
    release: tokio::sync::Semaphore,
}
impl Probe {
    fn counts(&self) -> (usize, usize, usize) {
        (
            self.downloads.load(Ordering::Acquire),
            self.plans.load(Ordering::Acquire),
            self.bootstraps.load(Ordering::Acquire),
        )
    }
    fn hold_second(&self) {
        self.hit.store(false, Ordering::Release);
        self.hold_at.store(self.counts().2 + 2, Ordering::Release);
    }
    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.hit.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "second authenticated bootstrap never reached"
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
        downloads: AtomicUsize::new(0),
        plans: AtomicUsize::new(0),
        bootstraps: AtomicUsize::new(0),
        hold_at: AtomicUsize::new(usize::MAX),
        hit: AtomicBool::new(false),
        release: tokio::sync::Semaphore::new(0),
    });
    let state = probe.clone();
    (
        app.layer(middleware::from_fn(move |request: Request, next: Next| {
            let state = state.clone();
            async move {
                let path = request.uri().path();
                if path == "/v3/transfers/downloads" {
                    state.downloads.fetch_add(1, Ordering::AcqRel);
                }
                if path.ends_with("/fetch-plans") {
                    state.plans.fetch_add(1, Ordering::AcqRel);
                }
                let hold = path.ends_with("/bootstrap")
                    && state.bootstraps.fetch_add(1, Ordering::AcqRel) + 1
                        == state.hold_at.load(Ordering::Acquire);
                let response = next.run(request).await;
                if hold {
                    state.hit.store(true, Ordering::Release);
                    if let Ok(Ok(permit)) =
                        tokio::time::timeout(Duration::from_secs(25), state.release.acquire()).await
                    {
                        permit.forget();
                    }
                }
                response
            }
        })),
        probe,
    )
}
pub fn run(
    remote: &NativeRemote,
    root: &Path,
    origin: &str,
    token: &str,
    ca: &str,
    probe: &Probe,
    original: &serde_json::Value,
) {
    let source_root = root.join("unchanged-source");
    let dest = root.join("unchanged-clone");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(source_root.join("contents.txt"), b"unchanged bytes").unwrap();
    let source = NativeWorkspace::initialize(&source_root, "Unchanged source").unwrap();
    let project = remote.provision(&source, "unchanged", "project").unwrap();
    assert_eq!(
        remote
            .publish(&source, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let clone = remote
        .clone_into(&dest, &project.space_id, "Unchanged clone")
        .unwrap();
    let operation = clone.state().unwrap().operation.id().clone();
    let binding_path = clone.store_root().join("native-remote.json");
    let binding = std::fs::read(&binding_path).unwrap();
    for controlled in [false, true] {
        let before = probe.counts();
        let report = if controlled {
            remote.receive_cancellable(&clone, &NativeTransferControl::new())
        } else {
            remote.receive(&clone)
        }
        .unwrap();
        assert_eq!(report.operation_id, operation);
        assert!(!report.materialized);
        assert_eq!(
            probe.counts(),
            (before.0, before.1, before.2 + 2),
            "unchanged must authenticate twice and download nothing"
        );
        assert_eq!(std::fs::read(&binding_path).unwrap(), binding);
    }
    let invalid = NativeRemote::with_trusted_ca(origin, &"a".repeat(64), ca.as_bytes()).unwrap();
    assert!(
        invalid
            .receive_cancellable(&clone, &NativeTransferControl::new())
            .is_err()
    );
    assert_eq!(clone.state().unwrap().operation.id(), &operation);

    // A previously received canonical object is rehashed, never trusted by ID.
    let tree_path = {
        use layrs_store::scm::{ScmReader, StoreObjectId};
        use layrs_store_local::scm_v1::{self, NativeScmReader, cas::NativeCas};
        let state = clone.state().unwrap();
        let head = state
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == clone.worktree_id())
            .unwrap()
            .base_step_id()
            .unwrap();
        let store = scm_v1::open(clone.store_root()).unwrap();
        let step = NativeScmReader::new(&store)
            .unwrap()
            .get_step(head)
            .unwrap()
            .unwrap();
        NativeCas::open(&store)
            .unwrap()
            .object_path(&StoreObjectId::Tree(step.root_tree_id().clone()))
            .unwrap()
    };
    let bytes = std::fs::read(&tree_path).unwrap();
    let mut corrupted = bytes.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 1;
    std::fs::write(&tree_path, &corrupted).unwrap();
    assert!(
        remote
            .receive_cancellable(&clone, &NativeTransferControl::new())
            .is_err()
    );
    assert_eq!(
        std::fs::read(&tree_path).unwrap(),
        corrupted,
        "receive must not silently repair corruption"
    );
    assert_eq!(std::fs::read(&binding_path).unwrap(), binding);
    std::fs::write(&tree_path, bytes).unwrap();

    // Cancellation after the verified local closure still precedes commit.
    probe.hold_second();
    let control = NativeTransferControl::new();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| remote.receive_cancellable(&clone, &control));
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
    assert_eq!(clone.state().unwrap().operation.id(), &operation);

    // A mutable binding cannot forge the Operation witness for a new Product.
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    let old = remote.bootstrap(&project.space_id).unwrap();
    let template: SyncTransactionRequest = serde_json::from_value(original.clone()).unwrap();
    let mut product = serde_json::to_value(
        template
            .products
            .iter()
            .find(|p| matches!(p, ProductEnvelope::Marker { .. }))
            .unwrap(),
    )
    .unwrap();
    let marker_id = format!("marker:{}", uuid::Uuid::new_v4());
    product["productId"] = marker_id.clone().into();
    product["record"]["markerId"] = marker_id.into();
    product["record"]["spaceId"] = serde_json::to_value(&project.space_id).unwrap();
    let target = &old.graph_root_step_ids[0];
    product["record"]["targetStepId"] = serde_json::to_value(target).unwrap();
    product["dependsOnStepIds"] = serde_json::json!([target]);
    let mut product: ProductEnvelope = serde_json::from_value(product).unwrap();
    let id = product.computed_object_id().unwrap();
    if let ProductEnvelope::Marker {
        product_object_id, ..
    } = &mut product
    {
        *product_object_id = id;
    }
    let mut request = template;
    request.space_id = project.space_id.clone();
    request.idempotency_key =
        IdempotencyKey::parse(format!("idempotency:{}", uuid::Uuid::new_v4())).unwrap();
    request.known_event_cursor = None;
    request.steps.clear();
    request.observations.clear();
    request.products = vec![product];
    request.view_mutation = None;
    request.request_digest = request.computed_request_digest().unwrap();
    let response = http
        .post(format!(
            "{origin}/v3/spaces/{}/transactions",
            project.space_id
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .unwrap();
    assert!(
        response.status().is_success(),
        "Product-only HTTP {}",
        response.status()
    );
    let receipt: SyncTransactionReceipt = response.json().unwrap();
    assert_eq!(
        receipt.status,
        ReceiptStatus::Applied,
        "{:?}",
        receipt.rejections
    );
    let new = remote.bootstrap(&project.space_id).unwrap();
    assert_ne!(old.product_roots, new.product_roots);
    assert_eq!(old.current_repository_view, new.current_repository_view);
    let mut forged: serde_json::Value = serde_json::from_slice(&binding).unwrap();
    forged["observed"] = serde_json::to_value(new).unwrap();
    std::fs::write(&binding_path, serde_json::to_vec(&forged).unwrap()).unwrap();
    let before = probe.counts();
    remote
        .receive_cancellable(&clone, &NativeTransferControl::new())
        .unwrap();
    assert!(
        probe.counts().1 > before.1,
        "new Product must force full fetch even when binding is forged"
    );

    // Obtain a new witnessed receive by publishing an actual change.
    std::fs::write(source_root.join("contents.txt"), b"new remote bytes").unwrap();
    source.capture().unwrap();
    assert_eq!(
        remote
            .publish(&source, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    remote.receive(&clone).unwrap();
    probe.hold_second();
    std::thread::scope(|scope| {
        let worker =
            scope.spawn(|| remote.receive_cancellable(&clone, &NativeTransferControl::new()));
        probe.wait();
        std::fs::write(dest.join("contents.txt"), b"concurrent local bytes").unwrap();
        clone.capture().unwrap();
        probe.resume();
        assert!(matches!(
            worker.join().unwrap(),
            Err(NativeRemoteError::Local(NativeClientError::StaleCapture))
        ));
    });
    assert_eq!(
        std::fs::read(dest.join("contents.txt")).unwrap(),
        b"concurrent local bytes"
    );
    let before = probe.counts();
    remote.receive(&clone).unwrap();
    assert!(
        probe.counts().1 > before.1,
        "local capture cannot reuse witness"
    );
    // A local rewrite retains different heads even with unchanged remote state.
    let rewrite_root = root.join("unchanged-rewrite");
    std::fs::create_dir(&rewrite_root).unwrap();
    let rewritten = remote
        .clone_into(&rewrite_root, &project.space_id, "Rewrite witness")
        .unwrap();
    std::fs::write(rewrite_root.join("contents.txt"), b"first local rewrite").unwrap();
    rewritten.capture().unwrap();
    let state = rewritten.state().unwrap();
    let first = state
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == rewritten.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    std::fs::write(rewrite_root.join("contents.txt"), b"second local rewrite").unwrap();
    rewritten.capture().unwrap();
    let rewritten_step = rewritten
        .apply_rewrite(&rewritten.prepare_squash(&first).unwrap())
        .unwrap();
    let before = probe.counts();
    remote.receive(&rewritten).unwrap();
    assert!(probe.counts().1 > before.1);
    assert!(
        rewritten
            .state()
            .unwrap()
            .view
            .layer_heads()
            .iter()
            .any(|heads| heads.step_ids().contains(&rewritten_step)),
        "local rewrite must not disappear"
    );

    // Epoch and authenticated principal changes always force revalidation.
    let epoch_root = root.join("unchanged-epoch");
    std::fs::create_dir(&epoch_root).unwrap();
    let epoch_clone = remote
        .clone_into(&epoch_root, &project.space_id, "Epoch witness")
        .unwrap();
    let before_epoch = remote
        .bootstrap(&project.space_id)
        .unwrap()
        .authorization_epoch;
    let response=http.post(format!("{origin}/v3/auth/accounts")).bearer_auth(token).json(&serde_json::json!({"login":"unchanged@example.com","password":"unchanged correct horse battery staple","displayName":"Unchanged member"})).send().unwrap();
    assert!(response.status().is_success());
    let account: serde_json::Value = response.json().unwrap();
    let response = http
        .post(format!("{origin}/v3/spaces/{}/members", project.space_id))
        .bearer_auth(token)
        .json(&serde_json::json!({"principalId":account["principalId"],"role":"member"}))
        .send()
        .unwrap();
    assert!(response.status().is_success());
    assert!(
        remote
            .bootstrap(&project.space_id)
            .unwrap()
            .authorization_epoch
            > before_epoch
    );
    let before = probe.counts();
    remote.receive(&epoch_clone).unwrap();
    assert!(
        probe.counts().1 > before.1,
        "epoch change cannot reuse witness"
    );
    let session = NativeRemote::login(
        origin,
        "unchanged@example.com",
        "unchanged correct horse battery staple",
        Some(ca.as_bytes()),
    )
    .unwrap();
    let member =
        NativeRemote::with_trusted_ca(origin, &session.access_token, ca.as_bytes()).unwrap();
    let principal_root = root.join("unchanged-principal");
    std::fs::create_dir(&principal_root).unwrap();
    let principal_clone = remote
        .clone_into(&principal_root, &project.space_id, "Principal witness")
        .unwrap();
    let before = probe.counts();
    member.receive(&principal_clone).unwrap();
    assert!(
        probe.counts().1 > before.1,
        "principal change cannot reuse witness"
    );
    let revoked_root = root.join("unchanged-revoked");
    std::fs::create_dir(&revoked_root).unwrap();
    let revoked = member
        .clone_into(&revoked_root, &project.space_id, "Revoked witness")
        .unwrap();
    let before_operation = revoked.state().unwrap().operation.id().clone();
    let before_binding = std::fs::read(revoked.store_root().join("native-remote.json")).unwrap();
    // Hold the first authenticated response; revoke before local verification
    // and the second bootstrap, which must independently reject this session.
    probe.hit.store(false, Ordering::Release);
    probe.hold_at.store(probe.counts().2 + 1, Ordering::Release);
    std::thread::scope(|scope| {
        let worker =
            scope.spawn(|| member.receive_cancellable(&revoked, &NativeTransferControl::new()));
        probe.wait();
        let response = http
            .post(format!("{origin}/v3/auth/logout"))
            .bearer_auth(&session.access_token)
            .send()
            .unwrap();
        assert!(response.status().is_success());
        probe.resume();
        let result = worker.join().unwrap();
        assert!(
            matches!(
                result,
                Err(NativeRemoteError::Http(401) | NativeRemoteError::Service { status: 401, .. })
            ),
            "revoked second authentication must fail"
        );
    });
    assert_eq!(revoked.state().unwrap().operation.id(), &before_operation);
    assert_eq!(
        std::fs::read(revoked.store_root().join("native-remote.json")).unwrap(),
        before_binding
    );
    eprintln!(
        "unchanged receive: zero downloads, corruption, cancellation, rewrite, capture, Product-only forged witness, epoch/principal and revoked second authentication PASS"
    );
}
