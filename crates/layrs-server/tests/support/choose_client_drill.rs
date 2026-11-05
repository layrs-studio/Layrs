//! Real TLS client preview, durable exact retry and explicit remote-only effect.
use layrs_client_core::native_scm::{NativeChoosePreview, NativeRemote, NativeWorkspace};
use layrs_sync::v3::{LayerMutationMode, PrincipalId, ReceiptStatus, RejectionCode};
use std::path::Path;

pub fn run(remote: &NativeRemote, root: &Path) {
    let aroot = root.join("choose-client-a");
    let broot = root.join("choose-client-b");
    std::fs::create_dir_all(&aroot).unwrap();
    std::fs::create_dir_all(&broot).unwrap();
    std::fs::write(aroot.join("choice.txt"), b"base").unwrap();
    let a = NativeWorkspace::initialize(&aroot, "Choose client fixture").unwrap();
    let project = remote
        .provision(&a, "client-choose", "exact-heads")
        .unwrap();
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let b = remote
        .clone_into(&broot, &project.space_id, "Choose second client")
        .unwrap();
    std::fs::write(aroot.join("choice.txt"), b"selected branch").unwrap();
    a.capture().unwrap();
    let selected = a.state().unwrap().view.layer_heads()[0].step_ids()[0].clone();
    std::fs::write(broot.join("choice.txt"), b"retained discarded branch").unwrap();
    b.capture().unwrap();
    let discarded = b.state().unwrap().view.layer_heads()[0].step_ids()[0].clone();
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert_eq!(
        remote
            .publish(&b, LayerMutationMode::Integrate)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let before = remote.bootstrap(&project.space_id).unwrap();
    assert_eq!(
        before.current_repository_view.record.layer_heads()[0]
            .step_ids()
            .len(),
        2
    );
    let preview = remote
        .preview_choose(
            &project.space_id,
            &project.layer_id,
            std::slice::from_ref(&selected),
            "Explicit administrative selection; retain other history",
        )
        .unwrap();
    assert_eq!(preview.previous_heads.len(), 2);
    assert_eq!(preview.chosen_heads, vec![selected.clone()]);
    assert_eq!(preview.discarded_heads, vec![discarded.clone()]);
    let stale = remote
        .preview_choose(
            &project.space_id,
            &project.layer_id,
            std::slice::from_ref(&discarded),
            "Alternate selection from the same old View",
        )
        .unwrap();
    let mut tampered = preview.clone();
    tampered.previous_heads.clear();
    assert!(remote.apply_choose(&tampered).is_err());
    let mut tampered = preview.clone();
    tampered.request.idempotency_key =
        layrs_sync::v3::IdempotencyKey::parse("choose-tampered-request-key").unwrap();
    assert!(remote.apply_choose(&tampered).is_err());
    let mut tampered = preview.clone();
    tampered.principal_id = PrincipalId::parse("principal:other-account").unwrap();
    assert!(remote.apply_choose(&tampered).is_err());
    let mut tampered = preview.clone();
    tampered.origin = "https://another.example/".into();
    assert!(remote.apply_choose(&tampered).is_err());
    assert_eq!(
        remote
            .bootstrap(&project.space_id)
            .unwrap()
            .current_repository_view,
        before.current_repository_view
    );
    let a_before = a.state().unwrap().operation.id().clone();
    let b_before = b.state().unwrap().operation.id().clone();
    // This exact JSON is safe to persist: no bearer/device/service credential.
    let path = root.join("choose-preview.json");
    let json = serde_json::to_vec(&preview).unwrap();
    assert!(json.len() < 256 * 1024);
    assert!(!String::from_utf8_lossy(&json).contains("access_token"));
    std::fs::write(&path, json).unwrap();
    let restored: NativeChoosePreview =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let receipt = remote.apply_choose(&restored).unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Applied);
    assert_eq!(
        remote.apply_choose(&restored).unwrap(),
        receipt,
        "same serialized request replays after the first response is lost"
    );
    assert_eq!(a.state().unwrap().operation.id(), &a_before);
    assert_eq!(b.state().unwrap().operation.id(), &b_before);
    assert_eq!(
        std::fs::read(broot.join("choice.txt")).unwrap(),
        b"retained discarded branch"
    );
    let refused = remote.apply_choose(&stale).unwrap();
    assert_eq!(refused.status, ReceiptStatus::Rejected);
    assert!(
        refused
            .rejections
            .iter()
            .any(|r| r.code == RejectionCode::ViewGenerationConflict)
    );
    let after = remote.bootstrap(&project.space_id).unwrap();
    assert_eq!(
        after.current_repository_view.record.layer_heads()[0].step_ids(),
        std::slice::from_ref(&selected)
    );
    assert!(
        !remote
            .fetch_step_files(&project.space_id, &discarded)
            .unwrap()
            .is_empty(),
        "discarded history and file closure remain fetchable"
    );
    remote.receive(&a).unwrap();
    assert_eq!(
        std::fs::read(aroot.join("choice.txt")).unwrap(),
        b"selected branch"
    );
    assert!(
        remote
            .preview_choose(
                &project.space_id,
                &project.layer_id,
                &[selected],
                "No change"
            )
            .is_err()
    );
    eprintln!(
        "HTTPS choose client: two-client divergence, exact persisted preview/replay, stale refusal, unchanged local Operations and retained history passed"
    );
}
