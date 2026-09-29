use super::*;
use layrs_core::{projection::EntryState, scm::LayerId};
use layrs_store_local::projection::RestoreSource;

pub(super) fn run(
    admin: &NativeRemote,
    workspace: &NativeWorkspace,
    reader: &NativeRemote,
    read: &Http<'_>,
    layer: &LayerId,
    principal: &PrincipalId,
) {
    let base = reader.bootstrap(&read.space).unwrap();
    let step = base
        .layers
        .iter()
        .find(|l| l.record.id() == layer)
        .unwrap()
        .head_step_ids[0]
        .clone();
    let root = workspace
        .checkout_root()
        .unwrap()
        .parent()
        .unwrap()
        .join("partial-refresh-checkout");
    std::fs::create_dir(&root).unwrap();
    let store = reader
        .clone_projection_into(&root, &read.space, &step)
        .unwrap();
    let patch = submission(
        &base,
        &step,
        "public/allowed.txt",
        Some(read.file_id(&step, "public/allowed.txt")),
        b"received remote edit",
        "partial-receive-first",
    );
    assert_eq!(
        reader.submit_path_patch(&patch).unwrap().receipt.status,
        ReceiptStatus::Applied
    );
    let refreshed = reader.receive_projection(&store, layer).unwrap();
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"received remote edit"
    );
    assert!(!root.join("private.txt").exists());
    let current = reader.bootstrap(&read.space).unwrap();
    let patch = submission(
        &current,
        &refreshed.snapshot.base_step_id,
        "public/allowed.txt",
        Some(FileId::from_bytes(b"received remote edit")),
        b"second remote edit",
        "partial-receive-second",
    );
    assert_eq!(
        reader.submit_path_patch(&patch).unwrap().receipt.status,
        ReceiptStatus::Applied
    );
    std::fs::write(root.join("public/allowed.txt"), b"local edit to preserve").unwrap();
    assert!(reader.receive_projection(&store, layer).is_err());
    assert_eq!(store.state().unwrap(), refreshed);
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"local edit to preserve"
    );
    std::fs::write(root.join("public/allowed.txt"), b"received remote edit").unwrap();
    reader.receive_projection(&store, layer).unwrap();
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"second remote edit"
    );
    let before_revoke = reader.bootstrap(&read.space).unwrap();
    let generation = store.state().unwrap().generation;
    std::fs::write(
        root.join("public/allowed.txt"),
        b"restore first captured bytes",
    )
    .unwrap();
    let first = store.capture_patch(generation).unwrap();
    std::fs::write(
        root.join("public/allowed.txt"),
        b"restore later local bytes",
    )
    .unwrap();
    store.capture_patch(generation).unwrap();
    let restore_source = RestoreSource::Capture {
        sequence: first.sequence,
        paths: vec![RepoPath::parse("public/allowed.txt").unwrap()],
    };
    let preview = reader
        .preview_projection_restore(&store, generation, &restore_source)
        .unwrap();
    let restored = reader.apply_projection_restore(&store, &preview).unwrap();
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"restore first captured bytes"
    );
    let undo = reader
        .preview_projection_restore(
            &store,
            generation,
            &RestoreSource::Inverse {
                operation_id: restored.operation_id,
            },
        )
        .unwrap();
    let undone = reader.apply_projection_restore(&store, &undo).unwrap();
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"restore later local bytes"
    );
    let redo = reader
        .preview_projection_restore(
            &store,
            generation,
            &RestoreSource::Inverse {
                operation_id: undone.operation_id,
            },
        )
        .unwrap();
    reader.apply_projection_restore(&store, &redo).unwrap();
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"restore first captured bytes"
    );
    std::fs::write(root.join("public/allowed.txt"), b"second remote edit").unwrap();
    let stale_restore = reader
        .preview_projection_restore(&store, generation, &restore_source)
        .unwrap();
    let hide = json!({"default":"allow","rules":[{"effect":"deny","selector":{"kind":"exact","path":"public/allowed.txt"}}]});
    let rules = json!({"schemaVersion":"layrs.space-policy-rules.v3","rules":[],"pathPolicies":[{"principalIds":[principal],"read":hide,"write":hide}]});
    let preview = admin
        .preview_policy_rules(workspace, &serde_json::to_vec(&rules).unwrap())
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &preview)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert!(
        reader
            .apply_projection_restore(&store, &stale_restore)
            .is_err()
    );
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"second remote edit"
    );
    let revoked = reader.receive_projection(&store, layer).unwrap();
    assert!(!root.join("public/allowed.txt").exists());
    assert_eq!(
        revoked
            .snapshot
            .entries
            .iter()
            .find(|e| e.path.as_str() == "public/allowed.txt")
            .unwrap()
            .state,
        EntryState::Reserved
    );
    let restore = admin
        .preview_stored_policy(workspace, &before_revoke.current_policy_set_id)
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &restore)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let restored = reader.receive_projection(&store, layer).unwrap();
    assert_eq!(
        restored
            .snapshot
            .entries
            .iter()
            .find(|e| e.path.as_str() == "public/allowed.txt")
            .unwrap()
            .state,
        EntryState::Materialized
    );
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"second remote edit"
    );
    store.fsck().unwrap();
    eprintln!(
        "HTTPS partial receive: exact remote changes, preserved dirty edits, revoked path quarantine and reauthorized hydration PASS"
    );
}
