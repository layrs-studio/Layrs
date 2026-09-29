//! Real HTTPS adversarial patches; no production bypass or simulated receipt.
use super::*;
use layrs_core::scm::LayerId;

fn head(base: &SpaceBootstrap, layer: &LayerId) -> StepId {
    let heads = &base
        .layers
        .iter()
        .find(|p| p.record.id() == layer)
        .unwrap()
        .head_step_ids;
    assert_eq!(heads.len(), 1);
    heads[0].clone()
}
fn seal(patch: &mut PathPatchSubmission) {
    patch.request.changes.sort_by(|a, b| a.path.cmp(&b.path));
    patch.files.sort_by(|a, b| a.path.cmp(&b.path));
    patch.request.request_digest = patch.request.computed_request_digest().unwrap();
}
fn unchanged(before: &SpaceBootstrap, after: &SpaceBootstrap) {
    assert_eq!(
        after.current_repository_view,
        before.current_repository_view
    );
    assert_eq!(
        after.current_repository_view_generation,
        before.current_repository_view_generation
    );
    assert_eq!(after.watermark_sequence, before.watermark_sequence);
}

fn verify_access(read: &Http<'_>, base: &SpaceBootstrap, step: &StepId) -> ProjectionAccessRequest {
    let request = ProjectionAccessRequest {
        step_id: step.clone(),
        expected_policy_set_id: base.current_policy_set_id.clone(),
        expected_authorization_epoch: base.authorization_epoch,
        paths: [
            "private.txt",
            "private/new.txt",
            "public/allowed.txt",
            "public/blocked.txt",
            "public/new-parent/new.txt",
        ]
        .into_iter()
        .map(|p| RepoPath::parse(p).unwrap())
        .collect(),
    };
    let endpoint = format!("/v3/spaces/{}/projection/access", read.space);
    let access: ProjectionAccess = serde_json::from_value(read.json(
        "POST",
        &endpoint,
        serde_json::to_value(&request).unwrap(),
    ))
    .unwrap();
    assert_eq!(access.principal_id, base.principal_id);
    assert_eq!(access.space_id, base.space_id);
    assert_eq!(access.step_id, *step);
    assert_eq!(access.policy_set_id, base.current_policy_set_id);
    assert_eq!(access.authorization_epoch, base.authorization_epoch);
    assert_eq!(
        access
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>(),
        request.paths
    );
    assert!(access.entries[0].target.as_ref().is_some());
    assert!(!access.entries[0].content_readable && !access.entries[0].writable);
    assert!(access.entries[1].target.as_ref().is_none());
    assert!(!access.entries[1].content_readable && !access.entries[1].writable);
    assert!(access.entries[2].content_readable && access.entries[2].writable);
    assert!(!access.entries[3].content_readable && !access.entries[3].writable);
    assert!(access.entries[4].target.as_ref().is_none());
    assert!(access.entries[4].content_readable && access.entries[4].writable);
    let mut bad = request.clone();
    bad.paths = vec![RepoPath::parse("private.txt/child").unwrap()];
    assert!(
        read.call("POST", &endpoint, serde_json::to_value(&bad).unwrap())
            .status()
            .is_client_error()
    );
    bad = request.clone();
    bad.paths.push(bad.paths[0].clone());
    assert_eq!(
        read.call("POST", &endpoint, serde_json::to_value(&bad).unwrap())
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    bad.paths = (0..201)
        .map(|n| RepoPath::parse(format!("public/{n:03}")).unwrap())
        .collect();
    assert_eq!(
        read.call("POST", &endpoint, serde_json::to_value(&bad).unwrap())
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    let unauthenticated = read
        .client
        .post(format!("{}{endpoint}", read.origin))
        .json(&request)
        .send()
        .unwrap();
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);
    request
}

pub(super) fn run(
    admin: &NativeRemote,
    workspace: &NativeWorkspace,
    reader: &NativeRemote,
    read: &Http<'_>,
    layer: &LayerId,
    admin_http: &Http<'_>,
    lose_response: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let base = reader.bootstrap(&read.space).unwrap();
    let step = head(&base, layer);
    let access_request = verify_access(read, &base, &step);
    let private = read.file_id(&step, "private.txt");
    // One denied leaf rejects the complete batch, including the allowed edit.
    let mut mixed = submission(
        &base,
        &step,
        "public/atomic.txt",
        None,
        b"must not appear",
        "security-mixed-batch",
    );
    let denied = submission(
        &base,
        &step,
        "private.txt",
        Some(private.clone()),
        b"must not overwrite",
        "security-unused",
    );
    mixed.request.changes.extend(denied.request.changes);
    mixed.files.extend(denied.files);
    seal(&mut mixed);
    let receipt = reader.submit_path_patch(&mixed).unwrap();
    assert_eq!(receipt.receipt.status, ReceiptStatus::Rejected);
    assert!(receipt.receipt.accepted_step_ids.is_empty());
    assert_eq!(reader.submit_path_patch(&mixed).unwrap(), receipt);
    unchanged(&base, &reader.bootstrap(&read.space).unwrap());

    // Distinct simultaneous requests have one exact View CAS winner. No implicit merge.
    let a = submission(
        &base,
        &step,
        "public/concurrent-a.txt",
        None,
        b"winner a",
        "security-concurrent-a",
    );
    let b = submission(
        &base,
        &step,
        "public/concurrent-b.txt",
        None,
        b"winner b",
        "security-concurrent-b",
    );
    let barrier = std::sync::Barrier::new(2);
    let (ra, rb) = std::thread::scope(|scope| {
        let one = scope.spawn(|| {
            barrier.wait();
            reader.submit_path_patch(&a).unwrap()
        });
        let two = scope.spawn(|| {
            barrier.wait();
            reader.submit_path_patch(&b).unwrap()
        });
        (one.join().unwrap(), two.join().unwrap())
    });
    assert_eq!(
        [&ra, &rb]
            .iter()
            .filter(|r| r.receipt.status == ReceiptStatus::Applied)
            .count(),
        1
    );
    assert_eq!(
        [&ra, &rb]
            .iter()
            .filter(|r| r.receipt.status == ReceiptStatus::Rejected)
            .count(),
        1
    );
    assert_eq!(reader.submit_path_patch(&a).unwrap(), ra);
    assert_eq!(reader.submit_path_patch(&b).unwrap(), rb);
    let after = reader.bootstrap(&read.space).unwrap();
    assert_eq!(
        after.current_repository_view_generation,
        base.current_repository_view_generation + 1
    );
    let latest = head(&after, layer);
    assert_eq!(read.file_id(&latest, "private.txt"), private);
    let page = reader
        .projection_page(
            &read.space,
            &ProjectionPageRequest {
                step_id: latest.clone(),
                expected_policy_set_id: after.current_policy_set_id.clone(),
                expected_authorization_epoch: after.authorization_epoch,
                cursor: None,
                limit: 200,
            },
        )
        .unwrap();
    assert!(page.next_cursor.is_none());
    let winner = if ra.receipt.status == ReceiptStatus::Applied {
        "public/concurrent-a.txt"
    } else {
        "public/concurrent-b.txt"
    };
    let loser = if ra.receipt.status == ReceiptStatus::Applied {
        "public/concurrent-b.txt"
    } else {
        "public/concurrent-a.txt"
    };
    assert!(page.entries.iter().any(|e| e.path.as_str() == winner));
    assert!(!page.entries.iter().any(|e| e.path.as_str() == loser));

    // An authorized rename is an atomic delete/create pair, not a directory move.
    let renamed_bytes = b"new allowed file";
    assert_eq!(
        read.file_id(&latest, "public/new.txt"),
        FileId::from_bytes(renamed_bytes)
    );
    let mut rename = submission(
        &after,
        &latest,
        "public/renamed.txt",
        None,
        renamed_bytes,
        "security-atomic-rename",
    );
    rename.request.changes.push(PathPatchChange {
        path: RepoPath::parse("public/new.txt").unwrap(),
        expected: Nullable::some(PathPatchEntry {
            target: TreeEntryTarget::file(FileId::from_bytes(renamed_bytes)),
            executable: false,
        }),
        replacement: Nullable::null(),
    });
    seal(&mut rename);
    let renamed = reader.submit_path_patch(&rename).unwrap();
    assert_eq!(renamed.receipt.status, ReceiptStatus::Applied);
    assert_eq!(reader.submit_path_patch(&rename).unwrap(), renamed);
    let after = reader.bootstrap(&read.space).unwrap();
    let latest = head(&after, layer);
    assert_eq!(
        read.file_id(&latest, "public/renamed.txt"),
        FileId::from_bytes(renamed_bytes)
    );
    let page = reader
        .projection_page(
            &read.space,
            &ProjectionPageRequest {
                step_id: latest.clone(),
                expected_policy_set_id: after.current_policy_set_id.clone(),
                expected_authorization_epoch: after.authorization_epoch,
                cursor: None,
                limit: 200,
            },
        )
        .unwrap();
    assert!(page.next_cursor.is_none());
    assert!(
        !page
            .entries
            .iter()
            .any(|e| e.path.as_str() == "public/new.txt")
    );

    // Read/write rights removed after preview invalidate the old exact request.
    let prepared = submission(
        &after,
        &latest,
        "public/revoked.txt",
        None,
        b"must remain local",
        "security-revoked-preview",
    );
    let active_policy = after.current_policy_set_id.clone();
    let rules = json!({"schemaVersion":"layrs.space-policy-rules.v3","rules":[],"pathPolicies":[{
        "principalIds":[after.principal_id],"read":{"default":"deny","rules":[]},"write":{"default":"deny","rules":[]}}]});
    let revoke = admin
        .preview_policy_rules(workspace, &serde_json::to_vec(&rules).unwrap())
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &revoke)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let revoked = reader.bootstrap(&read.space).unwrap();
    assert!(revoked.authorization_epoch > after.authorization_epoch);
    let access_response = read.call(
        "POST",
        &format!("/v3/spaces/{}/projection/access", read.space),
        serde_json::to_value(&access_request).unwrap(),
    );
    assert_eq!(access_response.status(), reqwest::StatusCode::CONFLICT);
    // Previously applied intent remains replayable after content rights vanish.
    // Invalid transport bytes prove replay happens before decoding new content.
    let mut replay_only = rename.clone();
    replay_only.files[0].bytes_base64 = "!not-base64!".into();
    assert_eq!(reader.submit_path_patch(&replay_only).unwrap(), renamed);
    let rejected = reader.submit_path_patch(&prepared).unwrap();
    assert_eq!(rejected.receipt.status, ReceiptStatus::Rejected);
    assert!(rejected.receipt.accepted_step_ids.is_empty());
    unchanged(&revoked, &reader.bootstrap(&read.space).unwrap());
    assert_eq!(reader.submit_path_patch(&prepared).unwrap(), rejected);
    let restore = admin
        .preview_stored_policy(workspace, &active_policy)
        .unwrap();
    assert_eq!(
        admin
            .apply_policy_preview(workspace, &restore)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    // Regrant does not turn a durable rejected intent into a new publication.
    assert_eq!(reader.submit_path_patch(&prepared).unwrap(), rejected);
    let restored = reader.bootstrap(&read.space).unwrap();
    assert_eq!(head(&restored, layer), latest);
    assert_eq!(read.file_id(&latest, "private.txt"), private);

    // Middleware drops only the HTTP response, after the transaction committed.
    let before_loss = reader.bootstrap(&read.space).unwrap();
    let lost = submission(
        &before_loss,
        &head(&before_loss, layer),
        "public/response-lost.txt",
        None,
        b"exact committed bytes",
        "security-response-lost",
    );
    lose_response.store(true, std::sync::atomic::Ordering::SeqCst);
    let response = read.call(
        "POST",
        &format!("/v3/spaces/{}/path-patches", read.space),
        serde_json::to_value(&lost).unwrap(),
    );
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    assert!(!lose_response.load(std::sync::atomic::Ordering::SeqCst));
    let committed = reader.bootstrap(&read.space).unwrap();
    assert_eq!(
        committed.current_repository_view_generation,
        before_loss.current_repository_view_generation + 1
    );
    let replayed = reader.submit_path_patch(&lost).unwrap();
    assert_eq!(replayed.receipt.status, ReceiptStatus::Applied);
    assert_eq!(reader.submit_path_patch(&lost).unwrap(), replayed);
    unchanged(&committed, &reader.bootstrap(&read.space).unwrap());
    let final_step = head(&committed, layer);
    assert_eq!(
        read.file_id(&final_step, "public/response-lost.txt"),
        FileId::from_bytes(b"exact committed bytes")
    );
    assert_eq!(read.file_id(&final_step, "private.txt"), private);
    // The restricted path endpoint must obey the same Layer protection gate.
    let protection_path = format!("/v3/spaces/{}/layers/{layer}/protection", read.space);
    assert!(
        admin_http
            .json("GET", &protection_path, Value::Null)
            .is_null()
    );
    let mut protection = json!({"minimumApprovals":1,"allowSelfApproval":false,
        "rejectConflicts":true,"recipe":null,"trustedRunnerPrincipalIds":[],
        "maximumProofAgeSeconds":3600});
    admin_http.json("PUT", &protection_path, protection.clone());
    let gated_base = reader.bootstrap(&read.space).unwrap();
    let gated = submission(
        &gated_base,
        &head(&gated_base, layer),
        "public/gated.txt",
        None,
        b"requires review",
        "security-protected-layer",
    );
    let blocked = reader.submit_path_patch(&gated).unwrap();
    assert_eq!(blocked.receipt.status, ReceiptStatus::Rejected);
    assert!(blocked.receipt.accepted_step_ids.is_empty());
    unchanged(&gated_base, &reader.bootstrap(&read.space).unwrap());
    assert_eq!(reader.submit_path_patch(&gated).unwrap(), blocked);
    protection["minimumApprovals"] = json!(0);
    admin_http.json("PUT", &protection_path, protection);

    eprintln!(
        "HTTPS scoped patch security: atomic batch/rename, concurrent CAS, revocation/regrant, protected Layer and lost-response exact replay PASS"
    );
}
