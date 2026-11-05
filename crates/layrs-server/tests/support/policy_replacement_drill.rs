use layrs_client_core::native_scm::{NativePolicyPreview, NativeRemote, NativeWorkspace};
use layrs_core::scm::RepoPath;
use layrs_sync::v3::{NativePolicyRules, ReceiptStatus, RejectionCode};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn run(
    admin: &NativeRemote,
    reader: &NativeRemote,
    workspace: &NativeWorkspace,
    lose: &AtomicBool,
) {
    let space = workspace.state().unwrap().space.id().clone();
    let base = admin.bootstrap(&space).unwrap();
    let reader_id = reader.bootstrap(&space).unwrap().principal_id;
    let step = base.layers[0].head_step_ids[0].clone();
    let path = RepoPath::parse("retained.txt").unwrap();
    let before = workspace.state().unwrap();
    let rules = |effect: &str| {
        serde_json::to_vec(&json!({"schemaVersion":"layrs.space-policy-rules.v3","rules":[],"pathPolicies":[{"principalIds":[reader_id],"read":{"default":"deny","rules":[{"effect":effect,"selector":{"kind":"exact","path":"retained.txt"}}]},"write":{"default":"deny","rules":[]}}]})).unwrap()
    };
    let denied = admin
        .preview_policy_rules(workspace, &rules("deny"))
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &denied)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert!(reader.read_remote_file(workspace, &step, &path).is_err());
    assert!(
        reader.read_policy_rules(workspace).is_err(),
        "restricted reader cannot enumerate opaque policies/rules"
    );
    let old = admin.read_policy_rules(workspace).unwrap();
    let selected = old
        .policies
        .iter()
        .find(|p| matches!(p.rules, NativePolicyRules::V3(_)))
        .unwrap();
    let policy_id = selected.policy_id.clone();
    let previous = selected.policy_product_object_id.clone();
    let journal = std::fs::read(workspace.store_root().join("native-remote.json")).unwrap();
    let preview = admin
        .preview_replace_policy_rules(workspace, &policy_id, &rules("allow"))
        .unwrap();
    assert_eq!(preview.schema_version, "layrs.policy-root-preview.v3");
    assert_eq!(
        std::fs::read(workspace.store_root().join("native-remote.json")).unwrap(),
        journal,
        "replacement preview never changes pending"
    );
    let replacement = preview
        .inline_rules
        .as_ref()
        .unwrap()
        .replacement
        .as_ref()
        .unwrap();
    assert_eq!(replacement.policy_id, policy_id);
    assert_eq!(replacement.previous_product_object_id, previous);
    let stale = admin
        .preview_replace_policy_rules(workspace, &policy_id, &rules("deny"))
        .unwrap();
    lose.store(true, Ordering::SeqCst);
    assert!(admin.apply_policy_preview(workspace, &preview).is_err());
    let pending: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.store_root().join("native-policy-intent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(pending["evidenceUploaded"], true);
    assert!(
        admin
            .preview_policy_rules(workspace, &rules("allow"))
            .is_err(),
        "pending replacement cannot become additive"
    );
    assert!(
        admin
            .preview_replace_policy_rules(workspace, &policy_id, &rules("deny"))
            .is_err(),
        "different intent cannot replace pending"
    );
    let recovered = admin
        .preview_replace_policy_rules(workspace, &policy_id, &rules("allow"))
        .unwrap();
    assert_eq!(
        serde_jcs::to_vec(&recovered).unwrap(),
        serde_jcs::to_vec(&preview).unwrap()
    );
    let roundtrip: NativePolicyPreview =
        serde_json::from_slice(&serde_json::to_vec(&recovered).unwrap()).unwrap();
    let receipt = admin.apply_policy_preview(workspace, &roundtrip).unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Applied);
    assert_eq!(
        admin.apply_policy_preview(workspace, &preview).unwrap(),
        receipt
    );
    let (_, bytes) = reader.read_remote_file(workspace, &step, &path).unwrap();
    assert_eq!(bytes, b"immutable checkout bytes");
    let current = admin.read_policy_rules(workspace).unwrap();
    assert_eq!(current.policies.len(), old.policies.len());
    for old_entry in &old.policies {
        let new_entry = current
            .policies
            .iter()
            .find(|p| p.policy_id == old_entry.policy_id)
            .unwrap();
        if old_entry.policy_id == policy_id {
            assert_ne!(new_entry.policy_product_object_id, previous);
            assert_ne!(new_entry.policy_version_id, old_entry.policy_version_id);
        } else {
            assert_eq!(
                serde_jcs::to_vec(new_entry).unwrap(),
                serde_jcs::to_vec(old_entry).unwrap()
            );
        }
    }
    let rejected = admin.apply_policy_preview(workspace, &stale).unwrap();
    assert_eq!(rejected.status, ReceiptStatus::Rejected);
    assert!(
        rejected
            .rejections
            .iter()
            .any(|r| r.code == RejectionCode::AuthorizationEpochChanged)
    );
    assert_eq!(
        admin.apply_policy_preview(workspace, &stale).unwrap(),
        rejected
    );
    let restore = admin
        .preview_stored_policy(workspace, &base.current_policy_set_id)
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &restore)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert_eq!(workspace.state().unwrap().operation, before.operation);
    eprintln!(
        "HTTPS Policy replacement: exact predecessor, rules readback, deny removed, other policies retained, lost-response pending/replay, stale rejection and managed restore passed"
    );
}
