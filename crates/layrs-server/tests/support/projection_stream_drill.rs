use super::*;
use std::io::Write;

pub(super) fn run(
    reader: &NativeRemote,
    read: &Http<'_>,
    layer: &LayerId,
    lose_response: &Arc<AtomicBool>,
    store: &ProjectionStore,
) {
    let root = store.checkout_root();
    let file_path = root.join("public/allowed.txt");
    let mut file = std::fs::File::create(&file_path).unwrap();
    let block = [0x5au8; 64 * 1024];
    for _ in 0..1600 {
        file.write_all(&block).unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    let before = store.state().unwrap();
    let capture = store.capture_patch(before.generation).unwrap();
    let expected = capture
        .changes
        .iter()
        .find(|c| c.path.as_str() == "public/allowed.txt")
        .unwrap()
        .replacement
        .as_ref()
        .unwrap();
    assert_eq!(expected.bytes, 100 * 1024 * 1024);
    std::fs::write(&file_path, b"edit after large capture").unwrap();
    lose_response.store(true, Ordering::SeqCst);
    assert!(
        reader
            .publish_projection(store, capture.sequence, layer)
            .is_err()
    );
    let status = layrs_client_core::native_scm::execute_projection(
        root,
        layrs_client_core::native_scm::ProjectionAction::Status,
    )
    .unwrap();
    assert_eq!(status["pendingPublication"]["intent"], "advance");
    let reopened = ProjectionStore::open(root).unwrap();
    let outcome = reader
        .publish_projection(&reopened, capture.sequence, layer)
        .unwrap();
    assert_eq!(outcome.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!outcome.pending_local_acceptance);
    assert_eq!(
        read.file_id(
            &outcome.projection.snapshot.base_step_id,
            "public/allowed.txt"
        ),
        expected.file_id
    );
    assert_eq!(
        std::fs::read(&file_path).unwrap(),
        b"edit after large capture"
    );
    assert_eq!(
        reader
            .publish_projection(&reopened, capture.sequence, layer)
            .unwrap()
            .receipt,
        outcome.receipt
    );
    assert!(
        reader
            .publish_projection(
                &reopened,
                capture.sequence,
                &LayerId::parse("layer:other-scoped-target").unwrap()
            )
            .is_err()
    );
    let next = reopened
        .capture_patch(outcome.projection.generation)
        .unwrap();
    let small = reader
        .publish_projection(&reopened, next.sequence, layer)
        .unwrap();
    assert_eq!(small.receipt.receipt.status, ReceiptStatus::Applied);
    let base = reader.bootstrap(&read.space).unwrap();
    std::fs::write(&file_path, b"candidate without head advance").unwrap();
    let capture = reopened.capture_patch(small.projection.generation).unwrap();
    let candidate = reader
        .publish_projection_candidate(&reopened, capture.sequence, layer)
        .unwrap();
    assert_eq!(candidate.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(candidate.receipt.candidate_id.is_some());
    assert!(candidate.receipt.receipt.view_result.0.is_none());
    assert_eq!(candidate.projection, small.projection);
    assert_eq!(
        reader
            .bootstrap(&read.space)
            .unwrap()
            .current_repository_view,
        base.current_repository_view
    );
    assert_eq!(
        reader
            .publish_projection_candidate(&reopened, capture.sequence, layer)
            .unwrap()
            .receipt,
        candidate.receipt
    );
    assert!(
        reader
            .publish_projection(&reopened, capture.sequence, layer)
            .is_err()
    );
    std::fs::write(&file_path, b"edit after large capture").unwrap();
    assert!(
        reopened
            .capture_patch(candidate.projection.generation)
            .unwrap()
            .changes
            .is_empty()
    );
    eprintln!(
        "HTTPS Store3 streaming: 100MiB exact upload, lost finalize/reopen/replay, preserved later edits, candidate without View advance PASS"
    );
}
