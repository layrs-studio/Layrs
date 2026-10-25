use super::*;
use layrs_store_local::projection::{CapturedPatch, ProjectionIdentity};

const COLD: &[u8] = b"cold unchanged bytes\n";
const EDITED: &[u8] = b"protected capture replacement\n";

fn capture_request(store: &ProjectionStore, patch: &CapturedPatch) -> AvailabilityRequest {
    AvailabilityRequest {
        version: ProjectionVersion::Capture {
            worktree_id: store.identity().worktree_id.clone(),
            sequence: patch.sequence,
            digest: patch.digest.clone(),
        },
        other: None,
        paths: vec![path("allowed.txt"), path("excluded.txt")],
        operation: OfflineOperation::Read,
    }
}

fn assert_cold(store: &ProjectionStore, request: &AvailabilityRequest) {
    let coverage = store.availability(request).unwrap();
    let cold = coverage
        .paths
        .iter()
        .find(|p| p.path == path("excluded.txt"))
        .unwrap();
    assert!(cold.manifest_id.is_none());
    assert_eq!(cold.blockers, vec![AvailabilityBlocker::MissingContent]);
    assert!(
        store
            .read_version_file(&request.version, &cold.path, &mut Vec::new())
            .is_err()
    );
}

fn prepare_from_anchored_base(f: &Fixture, store: &ProjectionStore, patch: &CapturedPatch) {
    let req = capture_request(store, patch);
    assert_cold(store, &req);
    let base = ProjectionVersion::Generation {
        worktree_id: store.identity().worktree_id.clone(),
        generation: patch.projection_generation,
        digest: patch.projection_digest.clone(),
    };
    let snapshot = store.version_metadata(&base).unwrap().0;
    let state = store.state().unwrap();
    store
        .compare_and_swap(state.generation, &state.snapshot)
        .unwrap();
    let generation = store.state().unwrap().generation;
    assert_ne!(generation, patch.projection_generation);
    {
        let mut server = f.server.lock().unwrap();
        server.advance(b"new remote head\n");
        assert_ne!(server.step.id(), &snapshot.base_step_id);
        // This replacement stays local even when its old remote path is denied.
        server.denied.insert(path("allowed.txt"));
    }
    let result = f
        .remote
        .prepare_projection_offline(store, generation, &req)
        .unwrap();
    assert_eq!(result.status, "ready");
    assert_eq!(result.request, req);
    assert!(store.availability(&req).unwrap().prepared);
    assert_eq!(store.captured_patch(patch.sequence).unwrap(), *patch);
    assert!(!store.checkout_root().join("excluded.txt").exists());
    assert!(f.file_gets() > 0);
    let events = {
        let server = f.server.lock().unwrap();
        assert!(!server.access.is_empty());
        for access in &server.access {
            assert_eq!(access.step_id, snapshot.base_step_id);
            assert_eq!(access.paths, vec![path("excluded.txt")]);
            assert_eq!(
                access.expected_policy_set_id,
                snapshot.observed_policy_set_id
            );
            assert_eq!(
                access.expected_authorization_epoch,
                snapshot.observed_authorization_epoch
            );
        }
        for claim in &server.claims {
            match claim {
                FetchClaim::File {
                    root, repo_path, ..
                } => {
                    assert_eq!(
                        root,
                        &AuthorizationRoot::Step {
                            step_id: snapshot.base_step_id.clone()
                        }
                    );
                    assert_eq!(repo_path, &path("excluded.txt"));
                }
                FetchClaim::Object { root, .. } => {
                    assert_eq!(
                        root,
                        &AuthorizationRoot::Step {
                            step_id: snapshot.base_step_id.clone()
                        }
                    );
                }
                _ => panic!("unexpected capture acquisition claim"),
            }
        }
        server.events.len()
    };
    for (name, expected) in [("allowed.txt", EDITED), ("excluded.txt", COLD)] {
        let mut bytes = Vec::new();
        store
            .read_version_file(&req.version, &path(name), &mut bytes)
            .unwrap();
        assert_eq!(bytes, expected);
    }
    assert!(
        store
            .compare_versions(&AvailabilityRequest {
                other: Some(req.version.clone()),
                operation: OfflineOperation::CompareContent,
                ..req
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.server.lock().unwrap().events.len(), events);
    store.fsck().unwrap();
}

#[test]
fn capture_unchanged_cold_path_acquires_exact_base_without_fetching_replacement() {
    let f = Fixture::with_excluded_bytes(COLD);
    let store = f.store();
    hydrate(&store);
    std::fs::write(store.checkout_root().join("allowed.txt"), EDITED).unwrap();
    let patch = store.capture_patch(1).unwrap();
    assert_eq!(patch.changes.len(), 1);
    assert_eq!(patch.changes[0].path, path("allowed.txt"));
    prepare_from_anchored_base(&f, &store, &patch);
    assert!(!store.selection().unwrap().includes(&path("excluded.txt")));
}

#[test]
fn imported_capture_unchanged_path_uses_copied_generation_and_preserves_source_identity() {
    let f = Fixture::with_excluded_bytes(COLD);
    let source_root = f.root.join("source");
    std::fs::create_dir(&source_root).unwrap();
    let identity = {
        let server = f.server.lock().unwrap();
        ProjectionIdentity {
            store_id: StoreId::parse("store:source").unwrap(),
            worktree_id: WorktreeId::parse("worktree:source").unwrap(),
            space_id: server.base.space_id.clone(),
            origin: f.remote.origin().trim_end_matches('/').into(),
            principal_id: server.base.principal_id.to_string(),
        }
    };
    let source = ProjectionStore::create(&source_root, identity.clone(), f.snapshot()).unwrap();
    hydrate(&source);
    std::fs::write(source_root.join("allowed.txt"), EDITED).unwrap();
    let patch = source.capture_patch(1).unwrap();
    let destination = f.root.join("converted");
    std::fs::create_dir(&destination).unwrap();
    let (store, receipt) = source
        .convert_into_family(
            &f.family(),
            &destination,
            WorktreeId::parse("worktree:converted").unwrap(),
        )
        .unwrap();
    f.server.lock().unwrap().probe = Some(destination);
    assert_eq!(receipt.source_identity, identity);
    assert_eq!(patch.identity, identity);
    assert_ne!(store.identity(), &identity);
    assert!(store.is_imported_capture(patch.sequence).unwrap());
    let mut wrong = capture_request(&store, &patch);
    wrong.version = ProjectionVersion::Capture {
        worktree_id: identity.worktree_id.clone(),
        sequence: patch.sequence,
        digest: patch.digest.clone(),
    };
    assert!(
        f.remote
            .prepare_projection_offline(&store, 1, &wrong)
            .is_err()
    );
    assert!(
        store
            .version_metadata(&ProjectionVersion::Generation {
                worktree_id: identity.worktree_id,
                generation: patch.projection_generation,
                digest: patch.projection_digest.clone()
            })
            .is_err()
    );
    assert!(f.server.lock().unwrap().events.is_empty());
    assert!(store.preparations().unwrap().is_empty());
    prepare_from_anchored_base(&f, &store, &patch);
    assert_eq!(source.captured_patch(patch.sequence).unwrap(), patch);
    assert!(store.require_publishable_capture(patch.sequence).is_err());
}

#[test]
fn capture_deletion_is_explicit_absence_and_never_fetches_its_base_file() {
    let f = Fixture::with_excluded_bytes(COLD);
    let store = f.store();
    hydrate(&store);
    std::fs::remove_file(store.checkout_root().join("allowed.txt")).unwrap();
    let patch = store.capture_patch(1).unwrap();
    assert!(patch.changes[0].replacement.is_none());
    let mut req = capture_request(&store, &patch);
    req.paths = vec![path("allowed.txt")];
    let result = f
        .remote
        .prepare_projection_offline(&store, 1, &req)
        .unwrap();
    assert_eq!(result.status, "ready");
    let coverage = store.availability(&req).unwrap();
    assert!(coverage.prepared);
    assert!(coverage.paths[0].absent);
    assert!(coverage.paths[0].file_id.is_none());
    let metadata = store.version_metadata(&req.version).unwrap().0;
    let deleted = metadata
        .entries
        .iter()
        .find(|e| e.path == path("allowed.txt"))
        .unwrap();
    assert_eq!(deleted.state, EntryState::Absent);
    assert!(deleted.target.is_none());
    let mut bytes = Vec::new();
    assert!(
        store
            .read_version_file(&req.version, &req.paths[0], &mut bytes)
            .is_err()
    );
    assert!(bytes.is_empty());
    assert!(f.server.lock().unwrap().events.is_empty());
    assert_cold(&store, &capture_request(&store, &patch));
    store.fsck().unwrap();
}
