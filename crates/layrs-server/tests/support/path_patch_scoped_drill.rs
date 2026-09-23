use super::*;
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, LayerId, TreeId};

fn request(
    base: &SpaceBootstrap,
    step: &StepId,
    root: &TreeId,
    old: &FileId,
    bytes: &[u8],
    key: &str,
    capture: char,
) -> ScopedPathPatchRequest {
    let manifest = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(SCOPED_CHUNK_BYTES).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap();
    let path = RepoPath::parse("public/allowed.txt").unwrap();
    let mut request = ScopedPathPatchRequest {
        schema_version: ScopedPathPatchSchema::V2,
        intent: ScopedPathPatchIntent::Candidate,
        space_id: base.space_id.clone(),
        client_id: ClientId::parse("client:scoped-stream-test").unwrap(),
        idempotency_key: IdempotencyKey::parse(format!("scoped-stream-{key}")).unwrap(),
        request_digest: base.current_policy_set_id.clone(),
        capture_digest: ObjectId::parse(format!("blake3:{}", capture.to_string().repeat(64)))
            .unwrap(),
        layer_id: base
            .layers
            .iter()
            .find(|layer| layer.head_step_ids.contains(step))
            .unwrap()
            .record
            .id()
            .clone(),
        base_step_id: step.clone(),
        base_root_tree_id: root.clone(),
        expected_repository_view_id: base.current_repository_view.repository_view_id.clone(),
        expected_generation: base.current_repository_view_generation,
        expected_policy_set_id: base.current_policy_set_id.clone(),
        expected_authorization_epoch: base.authorization_epoch,
        changes: vec![PathPatchChange {
            path: path.clone(),
            expected: Nullable::some(PathPatchEntry {
                target: TreeEntryTarget::file(old.clone()),
                executable: false,
            }),
            replacement: Nullable::some(PathPatchEntry {
                target: TreeEntryTarget::file(FileId::from_bytes(bytes)),
                executable: false,
            }),
        }],
        files: vec![ScopedPatchFile { path, manifest }],
    };
    request.request_digest = request.computed_request_digest().unwrap();
    request.validate().unwrap();
    request
}

fn begin(read: &Http<'_>, request: &ScopedPathPatchRequest) -> ScopedPatchProgress {
    serde_json::from_value(read.json(
        "POST",
        &format!("/v3/spaces/{}/path-patches/scoped", read.space),
        serde_json::to_value(request).unwrap(),
    ))
    .unwrap()
}

fn chunk_url(read: &Http<'_>, progress: &ScopedPatchProgress) -> String {
    format!(
        "{}/v3/spaces/{}/path-patches/scoped/{}/files/0/chunks/0",
        read.origin, read.space, progress.session_id
    )
}

fn finalize(read: &Http<'_>, progress: &ScopedPatchProgress) -> ScopedPathPatchReceipt {
    serde_json::from_value(read.json(
        "POST",
        &format!(
            "/v3/spaces/{}/path-patches/scoped/{}/finalize",
            read.space, progress.session_id
        ),
        json!({}),
    ))
    .unwrap()
}

pub(super) fn run(
    reader: &NativeRemote,
    read: &Http<'_>,
    base: &SpaceBootstrap,
    step: &StepId,
    old: &FileId,
) {
    let canonical: Step =
        serde_json::from_slice(&read.object(step, step.as_str(), ObjectKind::Step)).unwrap();
    let bytes = b"candidate streamed bytes prove per-session possession";
    let first = request(
        base,
        step,
        canonical.root_tree_id(),
        old,
        bytes,
        "candidate-0001",
        'b',
    );
    let progress = begin(read, &first);
    assert_eq!(progress.missing.len(), 1);
    assert_eq!(progress.missing[0].file_index, 0);
    assert_eq!(progress.missing[0].ordinal, 0);
    assert_eq!(begin(read, &first), progress);
    let url = chunk_url(read, &progress);
    let wrong = read
        .client
        .put(&url)
        .bearer_auth(read.token)
        .body(vec![b'x'; bytes.len()])
        .send()
        .unwrap();
    assert!(
        !wrong.status().is_success(),
        "wrong body must fail before CAS admission"
    );
    assert!(
        read.client
            .put(&url)
            .bearer_auth(read.token)
            .body(bytes.to_vec())
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    let complete = begin(read, &first);
    assert!(complete.missing.is_empty());
    let receipt = finalize(read, &complete);
    assert_eq!(
        receipt.receipt.status,
        ReceiptStatus::Applied,
        "{:?}",
        receipt.receipt.rejections
    );
    assert_eq!(receipt.receipt.view_result, Nullable::null());
    assert_eq!(receipt.request_digest, first.request_digest);
    assert_eq!(receipt.idempotency_key, first.idempotency_key);
    assert_eq!(finalize(read, &complete), receipt);
    assert_eq!(
        reader
            .bootstrap(&read.space)
            .unwrap()
            .current_repository_view,
        base.current_repository_view
    );

    // The same canonical Step may be admitted by another source capture. The
    // second session must supply its own bytes even when the chunk exists.
    let second = request(
        base,
        step,
        canonical.root_tree_id(),
        old,
        bytes,
        "candidate-0002",
        'c',
    );
    let second_progress = begin(read, &second);
    assert_eq!(second_progress.missing.len(), 1);
    let url = chunk_url(read, &second_progress);
    let wrong = read
        .client
        .put(&url)
        .bearer_auth(read.token)
        .body(vec![b'x'; bytes.len()])
        .send()
        .unwrap();
    assert!(
        !wrong.status().is_success(),
        "known ChunkId is not possession proof"
    );
    assert!(
        read.client
            .put(&url)
            .bearer_auth(read.token)
            .body(bytes.to_vec())
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    let second_receipt = finalize(read, &second_progress);
    assert_eq!(
        second_receipt.receipt.status,
        ReceiptStatus::Applied,
        "{:?}",
        second_receipt.receipt.rejections
    );
    assert_ne!(receipt.candidate_id, second_receipt.candidate_id);
    assert_eq!(receipt.receipt.accepted_step_ids.len(), 1);
    assert!(
        second_receipt.receipt.accepted_step_ids.is_empty(),
        "existing Step has a separate candidate provenance"
    );
}

pub(super) fn review_integration(
    reader: &NativeRemote,
    read: &Http<'_>,
    admin: &Http<'_>,
    layer: &LayerId,
) {
    let base = reader.bootstrap(&read.space).unwrap();
    let step = base
        .layers
        .iter()
        .find(|entry| entry.record.id() == layer)
        .unwrap()
        .head_step_ids[0]
        .clone();
    let canonical: Step =
        serde_json::from_slice(&read.object(&step, step.as_str(), ObjectKind::Step)).unwrap();
    let old = read.file_id(&step, "public/allowed.txt");
    let bytes = b"reviewed candidate with scoped approval";
    let source = request(
        &base,
        &step,
        canonical.root_tree_id(),
        &old,
        bytes,
        "reviewed-0001",
        'd',
    );
    let progress = begin(read, &source);
    assert_eq!(progress.missing.len(), 1);
    assert!(
        read.client
            .put(chunk_url(read, &progress))
            .bearer_auth(read.token)
            .body(bytes.to_vec())
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    let candidate = finalize(read, &progress);
    assert_eq!(
        candidate.receipt.status,
        ReceiptStatus::Applied,
        "{:?}",
        candidate.receipt.rejections
    );
    assert!(candidate.receipt.view_result.0.is_none());
    assert_eq!(
        reader
            .bootstrap(&read.space)
            .unwrap()
            .current_repository_view,
        base.current_repository_view
    );
    let stale_bytes = b"candidate uploaded before a competing View advance";
    let stale_source = request(
        &base,
        &step,
        canonical.root_tree_id(),
        &old,
        stale_bytes,
        "stale-candidate-0001",
        'e',
    );
    let stale_progress = begin(read, &stale_source);
    assert!(
        read.client
            .put(chunk_url(read, &stale_progress))
            .bearer_auth(read.token)
            .body(stale_bytes.to_vec())
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    let never_uploaded = request(
        &base,
        &step,
        canonical.root_tree_id(),
        &old,
        b"stale metadata without an uploaded Chunk",
        "stale-candidate-0002",
        'f',
    );
    let review: ScopedReview = serde_json::from_value(
        read.json(
            "POST",
            &format!("/v3/spaces/{}/scoped-reviews", read.space),
            serde_json::to_value(ScopedReviewProposalRequest {
                candidate_id: candidate.candidate_id.clone().unwrap(),
                idempotency_key: "reviewed-candidate-proposal-0001".into(),
                title: "Scoped candidate review".into(),
            })
            .unwrap(),
        ),
    )
    .unwrap();
    let actions = format!(
        "/v3/spaces/{}/scoped-reviews/{}/actions",
        read.space, review.review_id
    );
    let comment: ScopedReviewAction = serde_json::from_value(
        admin.json(
            "POST",
            &actions,
            serde_json::to_value(ScopedReviewActionRequest {
                idempotency_key: "scoped-admin-comment-0001".into(),
                action: ScopedReviewActionKind::Comment,
                body: "full-reader-only review detail".into(),
                anchor_path: None,
                reply_to: None,
            })
            .unwrap(),
        ),
    )
    .unwrap();
    let detail: ScopedReviewDetail = serde_json::from_value(read.json(
        "GET",
        &format!(
            "/v3/spaces/{}/scoped-reviews/{}",
            read.space, review.review_id
        ),
        Value::Null,
    ))
    .unwrap();
    let visible = detail
        .actions
        .iter()
        .find(|action| action.action_id == comment.action_id)
        .unwrap();
    assert!(
        visible.body_redacted && visible.body.is_none(),
        "restricted reader must not see full review comment"
    );
    let _: ScopedReviewAction = serde_json::from_value(
        admin.json(
            "POST",
            &actions,
            serde_json::to_value(ScopedReviewActionRequest {
                idempotency_key: "scoped-admin-approve-0001".into(),
                action: ScopedReviewActionKind::Approve,
                body: "approved exact candidate".into(),
                anchor_path: None,
                reply_to: None,
            })
            .unwrap(),
        ),
    )
    .unwrap();
    let path = format!(
        "/v3/spaces/{}/scoped-reviews/{}/integrate",
        read.space, review.review_id
    );
    let body = serde_json::to_value(ScopedReviewIntegrationRequest {
        idempotency_key: IdempotencyKey::parse("scoped-integration-0001").unwrap(),
    })
    .unwrap();
    let conflicted: SyncTransactionReceipt =
        serde_json::from_value(read.json("POST", &path, body.clone())).unwrap();
    assert_eq!(conflicted.status, ReceiptStatus::Rejected);
    assert_eq!(conflicted.rejections[0].code, RejectionCode::GateRejected);
    assert_eq!(
        serde_json::from_value::<SyncTransactionReceipt>(read.json("POST", &path, body)).unwrap(),
        conflicted
    );
    assert_eq!(
        reader
            .bootstrap(&read.space)
            .unwrap()
            .current_repository_view,
        base.current_repository_view
    );
    // The protected Step inherited an unresolved conflict outside the edited
    // path. Disabling only that whole-Step rule permits this approved review.
    let protection_path = format!("/v3/spaces/{}/layers/{layer}/protection", read.space);
    admin.json(
        "PUT",
        &protection_path,
        json!({
            "minimumApprovals":0,"allowSelfApproval":false,"rejectConflicts":false,
            "recipe":null,"trustedRunnerPrincipalIds":[],"maximumProofAgeSeconds":3600
        }),
    );
    let body = serde_json::to_value(ScopedReviewIntegrationRequest {
        idempotency_key: IdempotencyKey::parse("scoped-integration-0002").unwrap(),
    })
    .unwrap();
    let integrated: SyncTransactionReceipt =
        serde_json::from_value(read.json("POST", &path, body.clone())).unwrap();
    assert_eq!(
        integrated.status,
        ReceiptStatus::Applied,
        "{:?}",
        integrated.rejections
    );
    assert!(integrated.view_result.0.is_some());
    assert_eq!(
        serde_json::from_value::<SyncTransactionReceipt>(read.json("POST", &path, body)).unwrap(),
        integrated
    );
    assert_ne!(
        reader
            .bootstrap(&read.space)
            .unwrap()
            .current_repository_view,
        base.current_repository_view
    );
    assert!(begin(read, &stale_source).missing.is_empty());
    let stale_receipt = finalize(read, &stale_progress);
    assert_eq!(stale_receipt.receipt.status, ReceiptStatus::Rejected);
    assert_eq!(
        stale_receipt.receipt.rejections[0].code,
        RejectionCode::ViewGenerationConflict
    );
    assert_eq!(finalize(read, &stale_progress), stale_receipt);
    let no_chunk_progress = begin(read, &never_uploaded);
    assert!(no_chunk_progress.missing.is_empty());
    let no_chunk_receipt = finalize(read, &no_chunk_progress);
    assert_eq!(no_chunk_receipt.receipt.status, ReceiptStatus::Rejected);
    assert_eq!(finalize(read, &no_chunk_progress), no_chunk_receipt);
    eprintln!(
        "HTTPS scoped review: candidate, redaction, integration and stale terminal replay PASS"
    );
}
