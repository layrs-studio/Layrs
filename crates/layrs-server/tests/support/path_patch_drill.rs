use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
#[path = "projection_publication_drill.rs"]
mod publication;
#[path = "projection_refresh_drill.rs"]
mod refresh;
#[path = "path_patch_scoped_drill.rs"]
mod scoped;
#[path = "path_patch_security_drill.rs"]
mod security;

fn submission(
    base: &SpaceBootstrap,
    step: &StepId,
    path: &str,
    old: Option<FileId>,
    bytes: &[u8],
    key: &str,
) -> PathPatchSubmission {
    let mut request = PathPatchRequest {
        schema_version: PathPatchSchema::V1,
        space_id: base.space_id.clone(),
        client_id: ClientId::parse("client:scoped-patch-test").unwrap(),
        idempotency_key: IdempotencyKey::parse(format!("fixture-{key}")).unwrap(),
        request_digest: base.current_policy_set_id.clone(),
        layer_id: base
            .layers
            .iter()
            .find(|layer| layer.head_step_ids.contains(step))
            .unwrap()
            .record
            .id()
            .clone(),
        base_step_id: step.clone(),
        expected_repository_view_id: base.current_repository_view.repository_view_id.clone(),
        expected_generation: base.current_repository_view_generation,
        expected_policy_set_id: base.current_policy_set_id.clone(),
        expected_authorization_epoch: base.authorization_epoch,
        changes: vec![PathPatchChange {
            path: RepoPath::parse(path).unwrap(),
            expected: Nullable(old.map(|id| PathPatchEntry {
                target: TreeEntryTarget::file(id),
                executable: false,
            })),
            replacement: Nullable::some(PathPatchEntry {
                target: TreeEntryTarget::file(FileId::from_bytes(bytes)),
                executable: false,
            }),
        }],
    };
    request.request_digest = request.computed_request_digest().unwrap();
    PathPatchSubmission {
        request,
        files: vec![PathPatchFile {
            path: RepoPath::parse(path).unwrap(),
            bytes_base64: STANDARD.encode(bytes),
        }],
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    admin: &NativeRemote,
    workspace: &NativeWorkspace,
    reader: &NativeRemote,
    read: &Http<'_>,
    managed: &SpaceBootstrap,
    step: &StepId,
    principal: &PrincipalId,
    admin_http: &Http<'_>,
    lose_response: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let restore = admin
        .preview_stored_policy(workspace, &managed.current_policy_set_id)
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &restore)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let permission = json!({"default":"deny","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"public"}},{"effect":"deny","selector":{"kind":"exact","path":"public/blocked.txt"}}]});
    let rules = json!({"schemaVersion":"layrs.space-policy-rules.v3","rules":[],"pathPolicies":[{"principalIds":[principal],"read":permission,"write":permission}]});
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
    let base = reader.bootstrap(&read.space).unwrap();
    let clone_root = workspace
        .checkout_root()
        .unwrap()
        .parent()
        .unwrap()
        .join("scoped-partial-checkout");
    std::fs::create_dir(&clone_root).unwrap();
    let projected = reader
        .clone_projection_into(&clone_root, &read.space, step)
        .unwrap();
    let state = projected.state().unwrap();
    assert_eq!(projected.identity().principal_id, principal.as_str());
    assert_eq!(state.snapshot.base_step_id, *step);
    assert!(!clone_root.join("private.txt").exists());
    assert!(!clone_root.join("public/blocked.txt").exists());
    assert_eq!(
        std::fs::read(clone_root.join("public/allowed.txt")).unwrap(),
        b"same bytes with three distinct access paths"
    );
    assert!(
        state
            .snapshot
            .reserved_paths()
            .unwrap()
            .check_path("private.txt")
            .is_err()
    );
    assert!(
        state
            .snapshot
            .reserved_paths()
            .unwrap()
            .check_move("public", "moved")
            .is_err()
    );
    assert!(
        NativeWorkspace::open(&clone_root).is_err(),
        "full reader must refuse format3"
    );
    let old = read.file_id(step, "public/allowed.txt");
    scoped::run(reader, read, &base, step, &old);
    let migration_root = clone_root
        .parent()
        .unwrap()
        .join("migrated-partial-checkout");
    std::fs::create_dir(&migration_root).unwrap();
    let source_operation = workspace.state().unwrap().operation.id().clone();
    let source_marker = std::fs::read(workspace.store_root().join("FORMAT")).unwrap();
    let migration_layer = base
        .layers
        .iter()
        .find(|l| l.head_step_ids.contains(step))
        .unwrap()
        .record
        .id();
    let migrated = reader
        .migrate_projection_into(workspace, &migration_root, migration_layer)
        .unwrap();
    assert!(migrated.source_retained);
    assert!(!migrated.source_changed_during_migration);
    assert_eq!(workspace.state().unwrap().operation.id(), &source_operation);
    assert_eq!(
        std::fs::read(workspace.store_root().join("FORMAT")).unwrap(),
        source_marker
    );
    assert!(!migration_root.join("private.txt").exists());
    assert_eq!(migrated.projection.snapshot.base_step_id, *step);
    let private = read.file_id(step, "private.txt");
    let patch = submission(
        &base,
        step,
        "public/allowed.txt",
        Some(old.clone()),
        b"authorized edit with private sibling preserved",
        "scoped-allowed-edit",
    );
    let receipt = reader.submit_path_patch(&patch).unwrap();
    assert_eq!(
        receipt.receipt.status,
        ReceiptStatus::Applied,
        "{:?}",
        receipt.receipt.rejections
    );
    assert_eq!(
        reader.submit_path_patch(&patch).unwrap(),
        receipt,
        "exact retry"
    );
    let next = receipt.receipt.accepted_step_ids[0].clone();
    assert_eq!(read.file_id(&next, "private.txt"), private);
    assert_eq!(
        read.file_id(&next, "public/allowed.txt"),
        FileId::from_bytes(b"authorized edit with private sibling preserved")
    );
    // Same idempotency key with a new canonical patch is not a fresh write.
    let collision = submission(
        &base,
        step,
        "public/allowed.txt",
        Some(old),
        b"different intent",
        "scoped-allowed-edit",
    );
    assert!(reader.submit_path_patch(&collision).is_err());
    let current = reader.bootstrap(&read.space).unwrap();
    let hidden = submission(
        &current,
        &next,
        "private.txt",
        Some(private.clone()),
        b"must not replace hidden bytes",
        "scoped-hidden-denied",
    );
    let denied = reader.submit_path_patch(&hidden).unwrap();
    assert_eq!(denied.receipt.status, ReceiptStatus::Rejected);
    assert!(denied.receipt.accepted_step_ids.is_empty());
    assert_eq!(reader.submit_path_patch(&hidden).unwrap(), denied);
    assert_eq!(
        reader
            .bootstrap(&read.space)
            .unwrap()
            .current_repository_view,
        current.current_repository_view
    );
    // Knowing a hidden FileId cannot copy its contents through deduplication.
    let mut stolen = submission(
        &current,
        &next,
        "public/copied.txt",
        None,
        b"wrong supplied bytes",
        "scoped-no-dedup-oracle",
    );
    stolen.request.changes[0]
        .replacement
        .0
        .as_mut()
        .unwrap()
        .target = TreeEntryTarget::file(private);
    stolen.request.request_digest = stolen.request.computed_request_digest().unwrap();
    assert!(reader.submit_path_patch(&stolen).is_err());
    // Structural metadata and write rights are visible, but generic admission stays closed.
    let page = reader
        .projection_page(
            &read.space,
            &ProjectionPageRequest {
                step_id: next.clone(),
                expected_policy_set_id: current.current_policy_set_id.clone(),
                expected_authorization_epoch: current.authorization_epoch,
                cursor: None,
                limit: 200,
            },
        )
        .unwrap();
    assert!(
        page.entries
            .iter()
            .any(|entry| entry.path.as_str() == "public/allowed.txt" && entry.writable)
    );
    assert!(
        page.entries
            .iter()
            .any(|entry| entry.path.as_str() == "private.txt"
                && !entry.content_readable
                && !entry.writable)
    );
    let create = submission(
        &current,
        &next,
        "public/new.txt",
        None,
        b"new allowed file",
        "scoped-create",
    );
    assert_eq!(
        reader.submit_path_patch(&create).unwrap().receipt.status,
        ReceiptStatus::Applied
    );
    let stale = submission(
        &current,
        &next,
        "public/stale.txt",
        None,
        b"must conflict",
        "scoped-stale-base",
    );
    assert_eq!(
        reader.submit_path_patch(&stale).unwrap().receipt.status,
        ReceiptStatus::Rejected
    );
    publication::run(
        workspace,
        reader,
        read,
        &patch.request.layer_id,
        lose_response,
        admin,
        principal,
    );
    refresh::run(
        admin,
        workspace,
        reader,
        read,
        &patch.request.layer_id,
        principal,
    );
    security::run(
        admin,
        workspace,
        reader,
        read,
        &patch.request.layer_id,
        admin_http,
        lose_response,
    );
    scoped::review_integration(reader, read, admin_http, &patch.request.layer_id);
    eprintln!(
        "HTTPS scoped file patches: edit/create, hidden preservation/refusal, supplied-bytes proof, idempotency and stale View PASS"
    );
}
