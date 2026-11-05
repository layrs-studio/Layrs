use super::*;
#[path = "projection_offline_capture_tests.rs"]
mod capture_tests;
#[path = "projection_offline_fixture.rs"]
mod fixture;
use fixture::{Fixture, path};

fn request(store: &ProjectionStore, name: &str) -> AvailabilityRequest {
    AvailabilityRequest {
        version: store.current_version().unwrap(),
        other: None,
        paths: vec![path(name)],
        operation: OfflineOperation::Read,
    }
}
fn hydrate(store: &ProjectionStore) {
    let before = store.state().unwrap();
    let mut target = before.snapshot;
    target
        .entries
        .iter_mut()
        .find(|e| e.path == path("allowed.txt"))
        .unwrap()
        .state = EntryState::Materialized;
    store
        .install_projection(before.generation, &target, &mut |_, out| {
            out.write_all(b"exact offline bytes\n")
        })
        .unwrap();
}

#[test]
fn explicit_generation_preparation_uses_exact_excluded_path_and_offline_read_has_no_http() {
    let f = Fixture::new();
    let store = f.store();
    let req = request(&store, "excluded.txt");
    assert!(
        store
            .read_version_file(&req.version, &req.paths[0], &mut Vec::new())
            .is_err()
    );
    assert!(f.server.lock().unwrap().events.is_empty());
    let prepared = f
        .remote
        .prepare_projection_offline(&store, 0, &req)
        .unwrap();
    assert_eq!(prepared.status, "ready");
    assert!(store.availability(&req).unwrap().prepared);
    assert!(!store.selection().unwrap().includes(&path("excluded.txt")));
    assert!(!store.checkout_root().join("excluded.txt").exists());
    let mut bytes = Vec::new();
    let network = f.server.lock().unwrap().events.len();
    store
        .read_version_file(&req.version, &req.paths[0], &mut bytes)
        .unwrap();
    assert_eq!(bytes, b"exact offline bytes\n");
    let compare = AvailabilityRequest {
        other: Some(req.version.clone()),
        operation: OfflineOperation::CompareContent,
        ..req.clone()
    };
    assert!(store.compare_versions(&compare).unwrap().is_empty());
    assert_eq!(f.server.lock().unwrap().events.len(), network);
    assert!(f.file_gets() > 0);
    for claim in &f.server.lock().unwrap().claims {
        if let FetchClaim::File {
            repo_path, root, ..
        } = claim
        {
            assert_eq!(repo_path, &path("excluded.txt"));
            assert_eq!(
                root,
                &AuthorizationRoot::Step {
                    step_id: store.state().unwrap().snapshot.base_step_id
                }
            );
        }
    }
    store.fsck().unwrap();
}

#[test]
fn remote_observation_binds_step_root_absence_and_fresh_rights_without_hydrating() {
    let f = Fixture::new();
    let store = f.store();
    let step = store.state().unwrap().snapshot.base_step_id;
    let version = f
        .remote
        .observe_projection_version(
            &store,
            0,
            &step,
            &[path("allowed.txt"), path("missing.txt")],
        )
        .unwrap();
    let (snapshot, selection) = store.version_metadata(&version).unwrap();
    assert_eq!(
        snapshot.base_root_tree_id,
        *f.server.lock().unwrap().step.root_tree_id()
    );
    assert_eq!(snapshot.entries[1].state, EntryState::Absent);
    assert!(selection.includes(&path("missing.txt")));
    assert!(!store.selection().unwrap().includes(&path("missing.txt")));
    assert_eq!(f.file_gets(), 0);
    let req = AvailabilityRequest {
        version,
        other: None,
        paths: vec![path("allowed.txt")],
        operation: OfflineOperation::Read,
    };
    f.remote
        .prepare_projection_offline(&store, 0, &req)
        .unwrap();
    assert!(f.file_gets() > 0);
    let mut bytes = Vec::new();
    store
        .read_version_file(&req.version, &req.paths[0], &mut bytes)
        .unwrap();
    assert_eq!(bytes, b"exact offline bytes\n");
}

#[test]
fn acquisition_refuses_new_denial_same_file_alias_stale_authority_and_wrong_root() {
    for mode in ["denied", "epoch", "principal", "root"] {
        let f = Fixture::new();
        let store = f.store();
        let req = request(&store, "allowed.txt");
        {
            let mut s = f.server.lock().unwrap();
            match mode {
                "denied" => {
                    s.denied.insert(path("allowed.txt"));
                }
                "epoch" => s.base.authorization_epoch += 1,
                "principal" => s.base.principal_id = PrincipalId::parse("principal:other").unwrap(),
                "root" => s.wrong_root = true,
                _ => unreachable!(),
            }
        }
        assert!(
            f.remote
                .prepare_projection_offline(&store, 0, &req)
                .is_err(),
            "{mode}"
        );
        assert_eq!(f.file_gets(), 0, "{mode}");
        assert!(!store.availability(&req).unwrap().prepared);
    }
    let f = Fixture::new();
    let store = f.store();
    hydrate(&store);
    // Same FileId is retained via allowed.txt, but private.txt stays unreadable.
    let before = f.server.lock().unwrap().events.len();
    assert!(
        f.remote
            .prepare_projection_offline(&store, 1, &request(&store, "private.txt"))
            .is_err()
    );
    assert_eq!(f.server.lock().unwrap().events.len(), before);
}

#[test]
fn final_authority_and_generation_checks_leave_resumable_intent_without_private_acceptance() {
    for drift in ["epoch", "generation"] {
        let f = Fixture::new();
        let store = f.store();
        let req = request(&store, "allowed.txt");
        {
            let mut s = f.server.lock().unwrap();
            if drift == "epoch" {
                s.revoke_after_download = true;
            } else {
                s.advance_after_download = Some(store.checkout_root().into());
            }
        }
        assert!(
            f.remote
                .prepare_projection_offline(&store, 0, &req)
                .is_err()
        );
        let p = store.preparations().unwrap().pop().unwrap();
        assert_eq!(p.status, "acquiring");
        assert_eq!(p.request, req);
        assert!(
            store
                .read_version_file(&req.version, &req.paths[0], &mut Vec::new())
                .is_err()
        );
        if drift == "epoch" {
            f.server.lock().unwrap().base.authorization_epoch = 1;
        }
        let generation = store.state().unwrap().generation;
        let ready = f
            .remote
            .resume_projection_offline(&store, generation, &p.id)
            .unwrap();
        assert_eq!(ready.request, req);
        assert_eq!(ready.status, "ready");
        assert_eq!(store.state().unwrap().generation, generation);
    }
}

#[test]
fn resume_uses_exact_stored_request_outside_bounded_history_and_reauthorizes() {
    let f = Fixture::with_excluded_bytes(b"cold exact resume bytes\n");
    let store = f.store();
    let req = request(&store, "excluded.txt");
    let wanted = store.begin_preparation_at_generation(0, &req).unwrap();
    let state = store.state().unwrap();
    store.compare_and_swap(0, &state.snapshot).unwrap();
    let unrelated = request(&store, "allowed.txt");
    assert_ne!(unrelated.version, req.version);

    // A draft writer could exceed today's active bound. Seed only this private
    // fixture's unrelated history; the requested preparation and reservation
    // were created through the real Store API above.
    {
        let marker: serde_json::Value = serde_json::from_slice(
            &std::fs::read(store.checkout_root().join(".layrs/FORMAT")).unwrap(),
        )
        .unwrap();
        let database = std::path::PathBuf::from(marker["shared"]["metadataPath"].as_str().unwrap())
            .join("projection.sqlite3");
        assert!(database.starts_with(&f.root));
        let mut connection = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        let tx = connection.transaction().unwrap();
        for (status, count) in [("acquiring", 1024), ("released", 65)] {
            for n in 0..count {
                let p = OfflinePreparation {
                    id: format!("legacy-{status}-{n:04}"),
                    request: unrelated.clone(),
                    status: status.into(),
                };
                tx.execute(
                    "INSERT INTO projection_preparations(id,payload,status,root_id,reservation) VALUES(?1,?2,?3,NULL,?4)",
                    rusqlite::params![p.id, canonical_json(&p).unwrap(), status, format!("legacy-reservation-{}", p.id)],
                ).unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let listed = store.preparations().unwrap();
    assert_eq!(
        listed.iter().filter(|p| p.status != "released").count(),
        1024
    );
    assert_eq!(listed.iter().filter(|p| p.status == "released").count(), 64);
    assert!(!listed.iter().any(|p| p.id == wanted.id));
    assert_eq!(store.preparation(&wanted.id).unwrap(), Some(wanted.clone()));
    assert!(
        f.remote
            .resume_projection_offline(&store, 0, &wanted.id)
            .is_err()
    );
    for id in ["unknown-preparation", "legacy-released-0064"] {
        assert!(f.remote.resume_projection_offline(&store, 1, id).is_err());
    }
    assert!(f.server.lock().unwrap().events.is_empty());

    f.server.lock().unwrap().denied.insert(path("excluded.txt"));
    assert!(matches!(
        f.remote.resume_projection_offline(&store, 1, &wanted.id),
        Err(NativeRemoteError::ContentRestricted)
    ));
    assert_eq!(f.file_gets(), 0);
    assert_eq!(store.preparation(&wanted.id).unwrap(), Some(wanted.clone()));
    f.server
        .lock()
        .unwrap()
        .denied
        .remove(&path("excluded.txt"));
    let ready = f
        .remote
        .resume_projection_offline(&store, 1, &wanted.id)
        .unwrap();
    assert_eq!(ready.id, wanted.id);
    assert_eq!(ready.request, req);
    assert_eq!(ready.status, "ready");
    assert_eq!(store.state().unwrap().generation, 1);
    assert_eq!(
        store
            .preparation("legacy-acquiring-0000")
            .unwrap()
            .unwrap()
            .request,
        unrelated
    );
    assert_eq!(
        store
            .preparation("legacy-acquiring-0000")
            .unwrap()
            .unwrap()
            .status,
        "acquiring"
    );
    assert!(f.file_gets() > 0);
    let events = {
        let server = f.server.lock().unwrap();
        assert_eq!(server.access.len(), 2);
        for access in &server.access {
            assert_eq!(access.paths, req.paths);
            assert_eq!(access.step_id, state.snapshot.base_step_id);
        }
        for claim in &server.claims {
            if let FetchClaim::File {
                root, repo_path, ..
            } = claim
            {
                assert_eq!(repo_path, &path("excluded.txt"));
                assert_eq!(
                    root,
                    &AuthorizationRoot::Step {
                        step_id: state.snapshot.base_step_id.clone()
                    }
                );
            }
        }
        server.events.len()
    };
    let mut bytes = Vec::new();
    store
        .read_version_file(&req.version, &req.paths[0], &mut bytes)
        .unwrap();
    assert_eq!(bytes, b"cold exact resume bytes\n");
    assert_eq!(f.server.lock().unwrap().events.len(), events);
    assert!(!store.checkout_root().join("excluded.txt").exists());
}

#[test]
fn metadata_and_equal_content_preparation_do_not_download_payloads() {
    for op in [
        OfflineOperation::CompareMetadata,
        OfflineOperation::CompareContent,
    ] {
        let f = Fixture::new();
        let store = f.store();
        let mut req = request(&store, "allowed.txt");
        req.other = Some(req.version.clone());
        req.operation = op;
        f.remote
            .prepare_projection_offline(&store, 0, &req)
            .unwrap();
        assert!(f.server.lock().unwrap().events.is_empty());
        assert!(store.compare_versions(&req).unwrap().is_empty());
    }
}

#[test]
fn corrupt_download_and_missing_captured_chunk_never_become_ready_or_remote_repairs() {
    let f = Fixture::new();
    let store = f.store();
    let req = request(&store, "allowed.txt");
    f.server.lock().unwrap().corrupt_payload = true;
    assert!(
        f.remote
            .prepare_projection_offline(&store, 0, &req)
            .is_err()
    );
    assert_eq!(store.preparations().unwrap()[0].status, "acquiring");
    assert!(
        store
            .read_version_file(&req.version, &req.paths[0], &mut Vec::new())
            .is_err()
    );
    let f = Fixture::with_excluded_bytes(b"cold unchanged bytes\n");
    let store = f.store();
    hydrate(&store);
    std::fs::write(
        store.checkout_root().join("allowed.txt"),
        b"captured private bytes",
    )
    .unwrap();
    let patch = store.capture_patch(1).unwrap();
    let manifest = store
        .captured_file_manifest(patch.sequence, &path("allowed.txt"))
        .unwrap();
    let family = store.shared_family().unwrap().unwrap();
    let chunk = family.root().join("objects/chunks").join(
        manifest.chunks()[0]
            .chunk_id()
            .as_str()
            .strip_prefix("blake3:")
            .unwrap(),
    );
    assert!(chunk.starts_with(&f.root));
    std::fs::remove_file(chunk).unwrap();
    let version = ProjectionVersion::Capture {
        worktree_id: store.identity().worktree_id.clone(),
        sequence: patch.sequence,
        digest: patch.digest,
    };
    let req = AvailabilityRequest {
        version,
        other: None,
        paths: vec![path("allowed.txt"), path("excluded.txt")],
        operation: OfflineOperation::Read,
    };
    assert!(
        f.remote
            .prepare_projection_offline(&store, 1, &req)
            .is_err()
    );
    assert!(store.preparations().unwrap().is_empty());
    assert!(f.server.lock().unwrap().events.is_empty());
}

#[test]
fn imported_capture_refuses_network_and_receipt_replay_uses_original_identity() {
    use layrs_store_local::projection::ProjectionIdentity;
    for protocol in [0, 1, 2] {
        let f = Fixture::new();
        let family = f.family();
        let source_root = f.root.join("source");
        std::fs::create_dir(&source_root).unwrap();
        let s = f.server.lock().unwrap();
        let identity = ProjectionIdentity {
            store_id: StoreId::parse("store:source").unwrap(),
            worktree_id: WorktreeId::parse("worktree:source").unwrap(),
            space_id: s.base.space_id.clone(),
            origin: f.remote.origin().trim_end_matches('/').into(),
            principal_id: s.base.principal_id.to_string(),
        };
        drop(s);
        let source = ProjectionStore::create(&source_root, identity.clone(), f.snapshot()).unwrap();
        hydrate(&source);
        std::fs::write(source_root.join("allowed.txt"), b"original capture").unwrap();
        let capture = source.capture_patch(1).unwrap();
        let layer = LayerId::parse("layer:test").unwrap();
        if protocol != 0 {
            let request_digest = ObjectId::parse(format!("blake3:{}", "d".repeat(64))).unwrap();
            let mut receipt: serde_json::Value = serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/contracts/sync-v3/examples/rejected-receipt.json"
            )))
            .unwrap();
            receipt["clientId"] = serde_json::json!("client:source");
            let key = if protocol == 1 {
                format!(
                    "projection-{}",
                    blake3::hash(format!("{}\0{layer}", capture.digest).as_bytes())
                )
            } else {
                format!(
                    "projection-v2-{}",
                    blake3::hash(format!("{}\0{layer}\0advance", capture.digest).as_bytes())
                )
            };
            receipt["idempotencyKey"] = serde_json::json!(if protocol == 1 {
                key.clone()
            } else {
                format!("scoped-v2-{}", &request_digest.as_str()[7..])
            });
            let inner: SyncTransactionReceipt = serde_json::from_value(receipt).unwrap();
            inner.validate_v3().unwrap();
            let bytes = if protocol == 1 {
                canonical_json(&PathPatchReceipt {
                    request_digest: request_digest.clone(),
                    receipt: inner,
                })
                .unwrap()
            } else {
                canonical_json(&ScopedPathPatchReceipt {
                    request_digest,
                    client_id: ClientId::parse("client:source").unwrap(),
                    idempotency_key: IdempotencyKey::parse(key).unwrap(),
                    capture_digest: ObjectId::parse(capture.digest.clone()).unwrap(),
                    layer_id: layer.clone(),
                    base_step_id: source.state().unwrap().snapshot.base_step_id,
                    intent: ScopedPathPatchIntent::Advance,
                    candidate_id: None,
                    receipt: inner,
                })
                .unwrap()
            };
            source
                .begin_publication(capture.sequence, 1, b"{\"fixture\":true}")
                .unwrap();
            source
                .record_publication_receipt(capture.sequence, &bytes)
                .unwrap();
            source
                .finish_publication(capture.sequence, &bytes, None)
                .unwrap();
        }
        let dest = f.root.join("converted");
        std::fs::create_dir(&dest).unwrap();
        let (store, _) = source
            .convert_into_family(
                &family,
                &dest,
                WorktreeId::parse("worktree:converted").unwrap(),
            )
            .unwrap();
        let result = f
            .remote
            .publish_projection(&store, capture.sequence, &layer);
        if protocol == 0 {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("imported_capture_not_publishable")
            );
            assert!(
                f.remote
                    .publish_projection_candidate(&store, capture.sequence, &layer)
                    .is_err()
            );
        } else {
            let result = result.unwrap();
            assert!(result.historical);
            assert_eq!(result.receipt.receipt.client_id.as_str(), "client:source");
            assert!(!result.pending_local_acceptance);
        }
        assert!(f.server.lock().unwrap().events.is_empty());
    }
}

#[test]
fn shared_rebase_retains_incoming_base_separately_from_local_resolution() {
    let f = Fixture::new();
    let store = f.store();
    hydrate(&store);
    let old_version = store.current_version().unwrap();
    std::fs::write(
        store.checkout_root().join("allowed.txt"),
        b"chosen local bytes\0",
    )
    .unwrap();
    let capture = store.capture_patch(1).unwrap();
    f.server.lock().unwrap().advance(b"incoming base bytes\0");
    let layer = LayerId::parse("layer:test").unwrap();
    let preview = f
        .remote
        .preview_projection_rebase(&store, capture.sequence, &layer)
        .unwrap();
    assert!(preview.files[0].conflict);
    let choices = BTreeMap::from([(
        path("allowed.txt"),
        super::super::ProjectionMergeChoice::Local,
    )]);
    let result = f
        .remote
        .apply_projection_rebase(&store, &preview, &choices)
        .unwrap();
    assert_eq!(
        std::fs::read(store.checkout_root().join("allowed.txt")).unwrap(),
        b"chosen local bytes\0"
    );
    let new_version = store.current_version().unwrap();
    let mut incoming = Vec::new();
    store
        .read_version_file(&new_version, &path("allowed.txt"), &mut incoming)
        .unwrap();
    assert_eq!(incoming, b"incoming base bytes\0");
    let mut original = Vec::new();
    store
        .read_version_file(&old_version, &path("allowed.txt"), &mut original)
        .unwrap();
    assert_eq!(original, b"exact offline bytes\n");
    let mut accepted = Vec::new();
    store
        .write_captured_file(result.sequence, &path("allowed.txt"), &mut accepted)
        .unwrap();
    assert_eq!(accepted, b"chosen local bytes\0");
    assert!(!store.checkout_root().join("excluded.txt").exists());
    store.fsck().unwrap();
}
