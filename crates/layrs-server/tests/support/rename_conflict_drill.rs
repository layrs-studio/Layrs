#[path = "rename_conflict_adversarial.rs"]
mod adversarial;
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_core::scm::{ConflictId, ObjectId};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use layrs_weave::reconciliation::*;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub fn run(
    remote: &NativeRemote,
    root: &Path,
    origin: &str,
    token: &str,
    ca: &str,
    lose_response: &AtomicBool,
) {
    let aroot = root.join("rename-author");
    let broot = root.join("rename-reader");
    std::fs::create_dir(&aroot).unwrap();
    std::fs::create_dir(&broot).unwrap();
    std::fs::create_dir(aroot.join("old")).unwrap();
    std::fs::write(aroot.join("old/file.bin"), [0, 255, 10, 13]).unwrap();
    std::fs::write(aroot.join("keep"), b"unchanged").unwrap();
    let a = NativeWorkspace::initialize(&aroot, "Rename sync").unwrap();
    a.create_layer("topic").unwrap();
    a.switch_layer("topic").unwrap();
    std::fs::rename(aroot.join("old"), aroot.join("new")).unwrap();
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
    std::fs::write(aroot.join("old/file.bin"), b"changed").unwrap();
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
    let preview = a
        .preview_structural_merge(
            std::slice::from_ref(&source),
            &layer,
            FileMergePolicy {
                unsupported: FailureAction::StructuralFallback,
                quota: FailureAction::Reject,
                limits: Default::default(),
            },
            Default::default(),
        )
        .unwrap();
    assert_eq!(preview.plan().conflicts().len(), 1);
    let doc = preview
        .plan()
        .conflicts()
        .values()
        .next()
        .unwrap()
        .document()
        .clone();
    assert_eq!(doc["placements"].as_array().unwrap().len(), 4);
    assert!(!a.accept_merge(&preview).unwrap().materialized);
    let provision = remote.provision(&a, "rename-sync", "rename").unwrap();
    lose_response.store(true, Ordering::SeqCst);
    assert!(matches!(
        remote.publish(&a, LayerMutationMode::Advance),
        Err(layrs_client_core::native_scm::NativeRemoteError::Http(502))
    ));
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(a.store_root().join("native-remote.json")).unwrap())
            .unwrap();
    let seed = serde_json::from_value(journal["pending"].clone()).unwrap();
    let published = remote.publish(&a, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        published.status,
        ReceiptStatus::Applied,
        "{:?}",
        published.rejections
    );
    adversarial::run(remote, origin, token, ca, &seed, &doc);
    let b = remote
        .clone_into(&broot, &provision.space_id, "rename clone")
        .unwrap();
    assert_eq!(b.layer_conflicts(&layer).unwrap()[0]["document"], doc);
    drop(b);
    let b = NativeWorkspace::open(&broot).unwrap();
    let term = ObjectId::parse(
        doc["sides"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["provenance"]["stepId"] == source.as_str())
            .unwrap()["termId"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let choice = BTreeMap::from([(
        ConflictId::parse(doc["conflictId"].as_str().unwrap()).unwrap(),
        term,
    )]);
    let plan = b
        .preview_conflict_resolution(&layer, &choice, Default::default())
        .unwrap();
    assert!(b.accept_conflict_resolution(&plan).unwrap().materialized);
    assert_eq!(
        std::fs::read(broot.join("new/file.bin")).unwrap(),
        [0, 255, 10, 13]
    );
    assert!(!broot.join("old/file.bin").exists());
    let published = remote.publish(&b, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        published.status,
        ReceiptStatus::Applied,
        "{:?}",
        published.rejections
    );
    assert!(remote.receive(&a).unwrap().materialized);
    assert_eq!(
        std::fs::read(aroot.join("new/file.bin")).unwrap(),
        [0, 255, 10, 13]
    );
    assert!(!aroot.join("old/file.bin").exists());
    assert_eq!(std::fs::read(aroot.join("keep")).unwrap(), b"unchanged");
    assert!(a.layer_conflicts(&layer).unwrap().is_empty());
}
