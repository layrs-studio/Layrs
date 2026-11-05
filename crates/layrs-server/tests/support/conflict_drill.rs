use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_core::scm::{ConflictId, FileId, LayerId, ObjectId, WorktreeStatus};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use std::{collections::BTreeMap, path::Path};

pub fn shared_revert(
    remote: &NativeRemote,
    a: &NativeWorkspace,
    b: &NativeWorkspace,
    aroot: &Path,
    broot: &Path,
) {
    let name = "shared-change.bin";
    std::fs::write(aroot.join(name), [0, 255, 10]).unwrap();
    a.capture().unwrap();
    let original = a
        .create_marker("before-shared-revert", "retain original published change")
        .unwrap();
    assert_eq!(
        remote
            .publish(a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert!(remote.receive(b).unwrap().materialized);
    assert_eq!(std::fs::read(broot.join(name)).unwrap(), [0, 255, 10]);
    let inverse = b.preview_revert(original.target_step_id(), None).unwrap();
    assert!(inverse.conflicts().is_empty());
    b.apply_rewrite(&inverse).unwrap();
    assert!(!broot.join(name).exists());
    assert_eq!(
        remote
            .publish(b, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert!(remote.receive(a).unwrap().materialized);
    assert!(!aroot.join(name).exists());
    assert_eq!(
        std::fs::read(aroot.join("docs/readme.txt")).unwrap(),
        b"bob conflict"
    );
    for workspace in [a, b] {
        assert!(workspace.markers().unwrap().contains(&original));
        let old = workspace
            .read_step_files(original.target_step_id())
            .unwrap();
        let file = old.iter().find(|entry| entry.path == name).unwrap();
        assert!(
            matches!(&file.content, layrs_store_local::scm_v1::checkout::CheckoutContent::File { bytes, .. } if bytes == &[0,255,10])
        );
    }
}

pub fn run(
    remote: &NativeRemote,
    a: &NativeWorkspace,
    b: &NativeWorkspace,
    aroot: &Path,
    broot: &Path,
    layer: &LayerId,
) {
    std::fs::write(aroot.join("docs/readme.txt"), b"alice conflict").unwrap();
    std::fs::write(broot.join("docs/readme.txt"), b"bob conflict").unwrap();
    a.capture().unwrap();
    b.capture().unwrap();
    assert_eq!(
        remote
            .publish(a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert_eq!(
        remote
            .publish(b, LayerMutationMode::Integrate)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert!(remote.receive(a).unwrap().divergent);
    let heads = a
        .state()
        .unwrap()
        .view
        .layer_heads()
        .iter()
        .find(|h| h.layer_id() == layer)
        .unwrap()
        .step_ids()
        .to_vec();
    let preview = a
        .preview_structural_merge(
            &heads,
            layer,
            layrs_weave::reconciliation::FileMergePolicy {
                unsupported: layrs_weave::reconciliation::FailureAction::StructuralFallback,
                quota: layrs_weave::reconciliation::FailureAction::Reject,
                limits: Default::default(),
            },
            Default::default(),
        )
        .unwrap();
    assert_eq!(preview.plan().conflicts().len(), 1);
    let accepted = a.accept_merge(&preview).unwrap();
    assert!(!accepted.materialized);
    let published = remote.publish(a, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        published.status,
        ReceiptStatus::Applied,
        "{:?}",
        published.rejections
    );
    assert!(
        published
            .accepted_products
            .iter()
            .any(|p| p.object_id() == &accepted.product_object_id)
    );
    let received = remote.receive(b).unwrap();
    assert!(!received.materialized && !received.divergent);
    assert_eq!(
        std::fs::read(broot.join("docs/readme.txt")).unwrap(),
        b"bob conflict"
    );
    assert_eq!(
        b.state()
            .unwrap()
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == b.worktree_id())
            .unwrap()
            .status(),
        WorktreeStatus::Conflicted
    );
    let conflicts = b.layer_conflicts(layer).unwrap();
    assert_eq!(conflicts.len(), 1);
    let conflict = &conflicts[0]["document"];
    let wanted = FileId::from_bytes(b"bob conflict");
    let term = conflict["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["values"][0]["value"]["objectId"] == wanted.as_str())
        .unwrap();
    let choices = BTreeMap::from([(
        ConflictId::parse(conflict["conflictId"].as_str().unwrap()).unwrap(),
        ObjectId::parse(term["termId"].as_str().unwrap()).unwrap(),
    )]);
    let resolution = b
        .preview_conflict_resolution(layer, &choices, Default::default())
        .unwrap();
    let result = b.accept_conflict_resolution(&resolution).unwrap();
    assert!(result.materialized && result.conflict_ids.is_empty());
    let published = remote.publish(b, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        published.status,
        ReceiptStatus::Applied,
        "{:?}",
        published.rejections
    );
    assert!(remote.receive(a).unwrap().materialized);
    assert_eq!(
        std::fs::read(aroot.join("docs/readme.txt")).unwrap(),
        b"bob conflict"
    );
    assert!(a.layer_conflicts(layer).unwrap().is_empty());
}
