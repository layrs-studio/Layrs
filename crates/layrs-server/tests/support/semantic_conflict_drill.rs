#[path = "semantic_conflict_adversarial.rs"]
mod adversarial;
#[path = "semantic_review_drill.rs"]
mod review;
// Real pinned WASM -> TLS -> independent Store -> partial/final resolution.
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_core::scm::{ConflictId, ObjectId};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use layrs_weave::{graph_merge::ConflictBlockResolution, reconciliation::*, wasm_host::*};
use std::{collections::BTreeMap, path::Path};

fn choices(
    doc: &serde_json::Value,
    index: usize,
) -> BTreeMap<ConflictId, BTreeMap<ObjectId, ConflictBlockResolution>> {
    let block = &doc["blocks"][index];
    BTreeMap::from([(
        ConflictId::parse(doc["conflictId"].as_str().unwrap()).unwrap(),
        BTreeMap::from([(
            ObjectId::parse(block["blockId"].as_str().unwrap()).unwrap(),
            ConflictBlockResolution::SelectTerm(
                ObjectId::parse(block["baseValues"][0]["termId"].as_str().unwrap()).unwrap(),
            ),
        )]),
    )])
}

pub fn run(
    remote: &NativeRemote,
    root: &Path,
    origin: &str,
    token: &str,
    ca: &str,
    lose_response: &std::sync::atomic::AtomicBool,
) {
    let aroot = root.join("blocks-author");
    let broot = root.join("blocks-reader");
    std::fs::create_dir(&aroot).unwrap();
    std::fs::create_dir(&broot).unwrap();
    let original = b"zero\r\nsame\r\nzero\r\n";
    std::fs::write(aroot.join("code.ts"), original).unwrap();
    let a = NativeWorkspace::initialize(&aroot, "Shared blocks").unwrap();
    a.create_layer("review-target").unwrap();
    let review_layer = a
        .state()
        .unwrap()
        .layers
        .iter()
        .find(|l| l.name() == "review-target")
        .unwrap()
        .id()
        .clone();
    a.create_layer("topic").unwrap();
    a.switch_layer("topic").unwrap();
    std::fs::write(aroot.join("code.ts"), "\u{e9}\r\nsame\r\n\u{e9}\r\n").unwrap();
    a.capture().unwrap();
    let source = a
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == a.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    a.switch_layer("main").unwrap();
    std::fs::write(aroot.join("code.ts"), b"B\r\nsame\r\nB\r\n").unwrap();
    a.capture().unwrap();
    let layer = a
        .state()
        .unwrap()
        .layers
        .iter()
        .find(|l| l.name() == "main")
        .unwrap()
        .id()
        .clone();
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) {
            "layrs-lens-worker.exe"
        } else {
            "layrs-lens-worker"
        });
    let digest = layrs_lens_sdk::sha256_digest(
        &std::fs::read(&executable).expect("build pinned Lens worker first"),
    );
    let host = WasmHost::open(&executable, &digest, SandboxLimits::default()).unwrap();
    let preview = a
        .preview_sandboxed_merge(
            &[source],
            &layer,
            FileMergePolicy {
                unsupported: FailureAction::StructuralFallback,
                quota: FailureAction::Reject,
                limits: Default::default(),
            },
            Default::default(),
            &host,
        )
        .unwrap();
    let initial = preview
        .plan()
        .conflicts()
        .values()
        .next()
        .unwrap()
        .document()
        .clone();
    assert_eq!(initial["blocks"].as_array().unwrap().len(), 2);
    let accepted = a.accept_merge(&preview).unwrap();
    assert!(!accepted.materialized);
    let provision = remote.provision(&a, "semantic-sync", "blocks").unwrap();
    lose_response.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        remote.publish(&a, LayerMutationMode::Advance),
        Err(layrs_client_core::native_scm::NativeRemoteError::Http(502))
    ));
    let binding: serde_json::Value =
        serde_json::from_slice(&std::fs::read(a.store_root().join("native-remote.json")).unwrap())
            .unwrap();
    let seed = serde_json::from_value(binding["pending"].clone()).unwrap();
    let publish = remote.publish(&a, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        publish.status,
        ReceiptStatus::Applied,
        "{:?}",
        publish.rejections
    );
    adversarial::run(remote, origin, token, ca, &seed, &initial);
    let mut review = review::Review::start(
        remote,
        origin,
        token,
        ca,
        &seed,
        review_layer,
        &accepted.step_id,
    );
    let b = remote
        .clone_into(&broot, &provision.space_id, "Blocks clone")
        .unwrap();
    let doc = b.layer_conflicts(&layer).unwrap()[0]["document"].clone();
    assert_eq!(doc, initial);
    let preview = b
        .preview_conflict_blocks(&layer, &choices(&doc, 0), Default::default())
        .unwrap();
    let partial_result = b.accept_conflict_resolution(&preview).unwrap();
    assert!(!partial_result.materialized);
    let published = remote.publish(&b, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        published.status,
        ReceiptStatus::Applied,
        "{:?}",
        published.rejections
    );
    adversarial::reject_rollback(
        remote,
        origin,
        token,
        ca,
        &seed,
        partial_result.step_id.clone(),
    );
    review.partial(remote, &partial_result.step_id);
    drop(b);
    let b = NativeWorkspace::open(&broot).unwrap();
    let partial = b.layer_conflicts(&layer).unwrap()[0]["document"].clone();
    assert_eq!(partial["resolutionState"], "partial");
    assert_eq!(partial["bases"], initial["bases"]);
    assert_eq!(partial["sides"], initial["sides"]);
    assert!(!remote.receive(&a).unwrap().materialized);
    assert_eq!(a.layer_conflicts(&layer).unwrap()[0]["document"], partial);
    let preview = b
        .preview_conflict_blocks(&layer, &choices(&partial, 1), Default::default())
        .unwrap();
    let final_result = b.accept_conflict_resolution(&preview).unwrap();
    assert!(final_result.materialized);
    assert_eq!(std::fs::read(broot.join("code.ts")).unwrap(), original);
    let published = remote.publish(&b, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        published.status,
        ReceiptStatus::Applied,
        "{:?}",
        published.rejections
    );
    assert!(remote.receive(&a).unwrap().materialized);
    assert_eq!(std::fs::read(aroot.join("code.ts")).unwrap(), original);
    assert!(a.layer_conflicts(&layer).unwrap().is_empty());
    review.finish(remote, &final_result.step_id);
}
