use super::*;
use layrs_core::scm::LayerId;
use layrs_store_local::projection::ProjectionStore;
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
#[path = "projection_namespace_drill.rs"]
mod namespace;
#[path = "projection_rebase_drill.rs"]
mod rebase;
#[path = "projection_stream_drill.rs"]
mod streaming;

pub(super) fn run(
    workspace: &NativeWorkspace,
    reader: &NativeRemote,
    read: &Http<'_>,
    layer: &LayerId,
    lose_response: &Arc<AtomicBool>,
    admin: &NativeRemote,
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
        .join("partial-publication-checkout");
    std::fs::create_dir(&root).unwrap();
    let store = reader
        .clone_projection_into(&root, &read.space, &step)
        .unwrap();
    let state = store.state().unwrap();
    let protected = read.file_id(&step, "private.txt");
    // A bypass through an external editor preserves its bytes and blocks capture.
    std::fs::write(
        root.join("private.txt"),
        b"external protected-path collision",
    )
    .unwrap();
    assert!(store.capture_patch(state.generation).is_err());
    assert_eq!(
        std::fs::read(root.join("private.txt")).unwrap(),
        b"external protected-path collision"
    );
    std::fs::remove_file(root.join("private.txt")).unwrap();
    std::fs::write(
        root.join("public/allowed.txt"),
        b"captured immutable partial bytes",
    )
    .unwrap();
    let capture = store.capture_patch(state.generation).unwrap();
    assert_eq!(capture.changes.len(), 1);
    // Crash after journaling but before dispatch, then switch accounts. The new
    // principal must not execute the original account's pending intent.
    let mut prepared = submission(
        &base,
        &step,
        "public/allowed.txt",
        Some(read.file_id(&step, "public/allowed.txt")),
        b"captured immutable partial bytes",
        "partial-account-bound-intent",
    );
    prepared.request.client_id = ClientId::parse(format!(
        "client:{}",
        store
            .identity()
            .worktree_id
            .as_str()
            .trim_start_matches("worktree:")
    ))
    .unwrap();
    prepared.request.idempotency_key = IdempotencyKey::parse(format!(
        "projection-{}",
        blake3::hash(format!("{}\0{layer}", capture.digest).as_bytes())
    ))
    .unwrap();
    prepared.request.request_digest = prepared.request.computed_request_digest().unwrap();
    store
        .begin_publication(
            capture.sequence,
            state.generation,
            &serde_jcs::to_vec(&prepared).unwrap(),
        )
        .unwrap();
    assert!(
        admin
            .publish_projection(&store, capture.sequence, layer)
            .is_err()
    );
    assert_eq!(
        reader
            .bootstrap(&read.space)
            .unwrap()
            .current_repository_view_generation,
        base.current_repository_view_generation
    );
    // Capture is immutable, subsequent editing must survive publication.
    std::fs::write(root.join("public/allowed.txt"), b"newer unsent local edit").unwrap();
    lose_response.store(true, Ordering::SeqCst);
    assert!(
        reader
            .publish_projection(&store, capture.sequence, layer)
            .is_err()
    );
    assert!(store.pending_publication().unwrap().is_some());
    assert!(store.capture_patch(state.generation).is_err());
    drop(store);
    let reopened = ProjectionStore::open(&root).unwrap();
    let outcome = reader
        .publish_projection(&reopened, capture.sequence, layer)
        .unwrap();
    assert_eq!(outcome.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!outcome.pending_local_acceptance);
    assert!(
        reader
            .publish_projection(
                &reopened,
                capture.sequence,
                &LayerId::parse("layer:wrong-replay-target").unwrap()
            )
            .is_err()
    );
    assert_eq!(
        reader
            .publish_projection(&reopened, capture.sequence, layer)
            .unwrap()
            .receipt,
        outcome.receipt,
        "completed local acceptance is replayable"
    );
    assert!(reopened.pending_publication().unwrap().is_none());
    let current = reader.bootstrap(&read.space).unwrap();
    assert_eq!(
        current.current_repository_view_generation,
        base.current_repository_view_generation + 1
    );
    let published = &outcome.receipt.receipt.accepted_step_ids[0];
    assert_eq!(&outcome.projection.snapshot.base_step_id, published);
    assert_eq!(read.file_id(published, "private.txt"), protected);
    assert_eq!(
        read.file_id(published, "public/allowed.txt"),
        FileId::from_bytes(b"captured immutable partial bytes")
    );
    assert_eq!(
        std::fs::read(root.join("public/allowed.txt")).unwrap(),
        b"newer unsent local edit"
    );
    assert!(!root.join("private.txt").exists());
    let next = reopened
        .capture_patch(outcome.projection.generation)
        .unwrap();
    assert_eq!(next.changes.len(), 1);
    assert_eq!(
        next.changes[0].expected.as_ref().unwrap().file_id,
        FileId::from_bytes(b"captured immutable partial bytes")
    );
    let second = reader
        .publish_projection(&reopened, next.sequence, layer)
        .unwrap();
    assert_eq!(second.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!second.pending_local_acceptance);
    assert!(
        reopened
            .capture_patch(second.projection.generation)
            .unwrap()
            .changes
            .is_empty()
    );
    eprintln!(
        "HTTPS format3 capture/publication: protected collision, immutable capture, lost response, reopen/replay, exact base acceptance and preserved later edits PASS"
    );
    streaming::run(reader, read, layer, lose_response, &reopened);
    rebase::run(reader, read, layer, &reopened);
    namespace::run(admin, workspace, reader, read, layer, principal, &reopened);
}
