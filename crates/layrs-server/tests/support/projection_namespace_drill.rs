use super::*;
use layrs_client_core::native_scm::NativeProjectionNamespaceMutation as Mutation;
use layrs_core::scm::RepoPath;

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    admin: &NativeRemote,
    workspace: &NativeWorkspace,
    reader: &NativeRemote,
    read: &Http<'_>,
    layer: &LayerId,
    principal: &PrincipalId,
    store: &ProjectionStore,
) {
    let root = store.checkout_root();
    let create = |path: &str| Mutation::Create {
        path: RepoPath::parse(path).unwrap(),
    };
    let initial = store.state().unwrap();
    for path in [
        "private.txt",
        "private.txt/child",
        "public",
        "public/blocked.txt",
    ] {
        assert!(
            reader
                .preview_projection_namespace(store, initial.generation, &create(path))
                .is_err(),
            "reserved {path}"
        );
        assert_eq!(store.state().unwrap(), initial);
    }
    #[cfg(windows)]
    for path in [
        "PRIVATE.TXT",
        "private.txt.",
        "PRIVATE~1.TXT",
        "private.txt:stream",
    ] {
        assert!(
            reader
                .preview_projection_namespace(store, initial.generation, &create(path))
                .is_err(),
            "alias {path}"
        );
        assert_eq!(store.state().unwrap(), initial);
    }
    let preview = reader
        .preview_projection_namespace(
            store,
            initial.generation,
            &create("public/namespace-empty.txt"),
        )
        .unwrap();
    assert!(
        !root.join("public/namespace-empty.txt").exists(),
        "preview must not create a file"
    );
    let capture = reader.apply_projection_namespace(store, &preview).unwrap();
    assert_eq!(
        std::fs::read(root.join("public/namespace-empty.txt")).unwrap(),
        b""
    );
    assert_eq!(capture.changes.len(), 1);
    assert!(capture.changes[0].expected.is_none());
    assert_eq!(capture.changes[0].replacement.as_ref().unwrap().bytes, 0);
    let created = reader
        .publish_projection(store, capture.sequence, layer)
        .unwrap();
    assert_eq!(created.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!created.pending_local_acceptance);
    assert_eq!(
        read.file_id(
            &created.projection.snapshot.base_step_id,
            "public/namespace-empty.txt"
        ),
        FileId::from_bytes(b"")
    );

    let movement = Mutation::Move {
        source: RepoPath::parse("public/namespace-empty.txt").unwrap(),
        path: RepoPath::parse("public/namespace-moved.txt").unwrap(),
    };
    let preview = reader
        .preview_projection_namespace(store, store.state().unwrap().generation, &movement)
        .unwrap();
    let capture = reader.apply_projection_namespace(store, &preview).unwrap();
    assert!(!root.join("public/namespace-empty.txt").exists());
    assert_eq!(
        std::fs::read(root.join("public/namespace-moved.txt")).unwrap(),
        b""
    );
    assert_eq!(capture.changes.len(), 2);
    assert!(
        capture
            .changes
            .iter()
            .any(|c| c.path.as_str() == "public/namespace-empty.txt" && c.replacement.is_none())
    );
    let moved = reader
        .publish_projection(store, capture.sequence, layer)
        .unwrap();
    assert_eq!(moved.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!moved.pending_local_acceptance);
    assert_eq!(
        read.file_id(
            &moved.projection.snapshot.base_step_id,
            "public/namespace-moved.txt"
        ),
        FileId::from_bytes(b"")
    );
    assert!(store.fsck().unwrap().namespace_operations >= 2);

    let preview = reader
        .preview_projection_namespace(
            store,
            store.state().unwrap().generation,
            &create("public/directory-source/file.txt"),
        )
        .unwrap();
    let captured = reader.apply_projection_namespace(store, &preview).unwrap();
    assert_eq!(
        reader
            .publish_projection(store, captured.sequence, layer)
            .unwrap()
            .receipt
            .receipt
            .status,
        ReceiptStatus::Applied
    );
    let preview = reader
        .preview_projection_directory_move(
            store,
            store.state().unwrap().generation,
            &RepoPath::parse("public/directory-source").unwrap(),
            &RepoPath::parse("public/directory-moved").unwrap(),
        )
        .unwrap();
    std::fs::write(
        root.join("public/directory-source/unknown.txt"),
        b"external unobserved bytes",
    )
    .unwrap();
    assert!(
        reader
            .apply_projection_directory_move(store, &preview)
            .is_err()
    );
    assert_eq!(
        std::fs::read(root.join("public/directory-source/unknown.txt")).unwrap(),
        b"external unobserved bytes"
    );
    assert!(!root.join("public/directory-moved").exists());
    std::fs::remove_file(root.join("public/directory-source/unknown.txt")).unwrap();
    let captured = reader
        .apply_projection_directory_move(store, &preview)
        .unwrap();
    assert_eq!(
        reader
            .apply_projection_directory_move(store, &preview)
            .unwrap(),
        captured
    );
    assert!(!root.join("public/directory-source").exists());
    assert_eq!(
        std::fs::read(root.join("public/directory-moved/file.txt")).unwrap(),
        b""
    );
    let published = reader
        .publish_projection(store, captured.sequence, layer)
        .unwrap();
    assert_eq!(published.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!published.pending_local_acceptance);
    eprintln!(
        "HTTPS Store3 directory move: current per-leaf authority, unknown-file refusal, exact publication PASS"
    );

    let preview = reader
        .preview_projection_namespace(
            store,
            store.state().unwrap().generation,
            &create("public/unpublished-source.txt"),
        )
        .unwrap();
    reader.apply_projection_namespace(store, &preview).unwrap();
    let movement = Mutation::Rename {
        source: RepoPath::parse("public/unpublished-source.txt").unwrap(),
        path: RepoPath::parse("public/unpublished-renamed.txt").unwrap(),
    };
    let preview = reader
        .preview_projection_namespace(store, store.state().unwrap().generation, &movement)
        .unwrap();
    let capture = reader.apply_projection_namespace(store, &preview).unwrap();
    assert_eq!(
        capture.changes.len(),
        1,
        "unpublished source needs no remote deletion"
    );
    assert_eq!(
        capture.changes[0].path.as_str(),
        "public/unpublished-renamed.txt"
    );
    assert!(capture.changes[0].expected.is_none());
    let renamed = reader
        .publish_projection(store, capture.sequence, layer)
        .unwrap();
    assert_eq!(renamed.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!renamed.pending_local_acceptance);

    let preview = reader
        .preview_projection_namespace(
            store,
            store.state().unwrap().generation,
            &create("public/namespace-collision.txt"),
        )
        .unwrap();
    std::fs::write(
        root.join("public/namespace-collision.txt"),
        b"external editor bytes",
    )
    .unwrap();
    let before = store.state().unwrap();
    assert!(reader.apply_projection_namespace(store, &preview).is_err());
    assert_eq!(
        std::fs::read(root.join("public/namespace-collision.txt")).unwrap(),
        b"external editor bytes"
    );
    assert_eq!(store.state().unwrap(), before);
    std::fs::remove_file(root.join("public/namespace-collision.txt")).unwrap();

    let preview = reader
        .preview_projection_namespace(
            store,
            store.state().unwrap().generation,
            &create("public/namespace-revoked.txt"),
        )
        .unwrap();
    let before_policy = reader.bootstrap(&read.space).unwrap();
    let denied = json!({"default":"allow","rules":[{"effect":"deny","selector":{"kind":"exact","path":"public/namespace-revoked.txt"}}]});
    let rules = json!({"schemaVersion":"layrs.space-policy-rules.v3","rules":[],"pathPolicies":[{"principalIds":[principal],"read":denied,"write":denied}]});
    let policy = admin
        .preview_policy_rules(workspace, &serde_json::to_vec(&rules).unwrap())
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &policy)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let before = store.state().unwrap();
    assert!(
        reader.apply_projection_namespace(store, &preview).is_err(),
        "preview cannot retain revoked permission"
    );
    assert!(!root.join("public/namespace-revoked.txt").exists());
    assert_eq!(store.state().unwrap(), before);
    let restore = admin
        .preview_stored_policy(workspace, &before_policy.current_policy_set_id)
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &restore)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    eprintln!(
        "HTTPS partial namespace: empty create/capture/publication, journaled move/publication, reserved paths/aliases, external collision preservation and stale policy refusal PASS"
    );
}
