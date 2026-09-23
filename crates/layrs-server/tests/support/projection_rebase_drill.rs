use super::*;
use layrs_client_core::native_scm::remote::ProjectionMergeChoice;
use std::collections::BTreeMap;

pub(super) fn run(
    reader: &NativeRemote,
    read: &Http<'_>,
    layer: &LayerId,
    store: &ProjectionStore,
) {
    let path = RepoPath::parse("public/allowed.txt").unwrap();
    let file = store.checkout_root().join(path.as_str());
    std::fs::write(&file, b"a\nb\nc\n").unwrap();
    let patch = store
        .capture_patch(store.state().unwrap().generation)
        .unwrap();
    assert_eq!(
        reader
            .publish_projection(store, patch.sequence, layer)
            .unwrap()
            .receipt
            .receipt
            .status,
        ReceiptStatus::Applied
    );
    let root = store
        .checkout_root()
        .parent()
        .unwrap()
        .join("partial-rebase-peer");
    let historical_step = store.state().unwrap().snapshot.base_step_id;
    std::fs::create_dir(&root).unwrap();
    let peer = reader
        .clone_projection_into(
            &root,
            &read.space,
            &store.state().unwrap().snapshot.base_step_id,
        )
        .unwrap();
    std::fs::write(&file, b"A\nb\nc\n").unwrap();
    let local = store
        .capture_patch(store.state().unwrap().generation)
        .unwrap();
    std::fs::write(root.join(path.as_str()), b"a\nb\nC\n").unwrap();
    let incoming = peer
        .capture_patch(peer.state().unwrap().generation)
        .unwrap();
    let published = reader
        .publish_projection(&peer, incoming.sequence, layer)
        .unwrap();
    assert_eq!(published.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(reader.receive_projection(store, layer).is_err());
    let preview = reader
        .preview_projection_rebase(store, local.sequence, layer)
        .unwrap();
    assert!(preview.files.iter().all(|f| !f.conflict));
    assert_eq!(std::fs::read(&file).unwrap(), b"A\nb\nc\n");
    let reopened = ProjectionStore::open(store.checkout_root()).unwrap();
    let saved = serde_json::from_slice(&reopened.rebase_draft().unwrap().unwrap()).unwrap();
    assert_eq!(preview, saved);
    let merged = reader
        .apply_projection_rebase(&reopened, &saved, &BTreeMap::new())
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"A\nb\nC\n");
    assert_eq!(
        reader
            .apply_projection_rebase(&reopened, &saved, &BTreeMap::new())
            .unwrap(),
        merged
    );
    assert_eq!(
        store.state().unwrap().snapshot.base_step_id,
        published.projection.snapshot.base_step_id
    );
    assert_eq!(
        reader
            .publish_projection(store, merged.sequence, layer)
            .unwrap()
            .receipt
            .receipt
            .status,
        ReceiptStatus::Applied
    );
    assert!(!store.checkout_root().join("private.txt").exists());
    reader.receive_projection(&peer, layer).unwrap();
    std::fs::write(&file, b"local conflict\n").unwrap();
    let local = store
        .capture_patch(store.state().unwrap().generation)
        .unwrap();
    std::fs::write(root.join(path.as_str()), b"remote conflict\n").unwrap();
    let incoming = peer
        .capture_patch(peer.state().unwrap().generation)
        .unwrap();
    reader
        .publish_projection(&peer, incoming.sequence, layer)
        .unwrap();
    let stale = reader
        .preview_projection_rebase(store, local.sequence, layer)
        .unwrap();
    assert!(stale.files.iter().any(|f| f.path == path && f.conflict));
    assert!(
        reader
            .apply_projection_rebase(store, &stale, &BTreeMap::new())
            .is_err()
    );
    std::fs::write(root.join(path.as_str()), b"newer remote conflict\n").unwrap();
    let incoming = peer
        .capture_patch(peer.state().unwrap().generation)
        .unwrap();
    reader
        .publish_projection(&peer, incoming.sequence, layer)
        .unwrap();
    let choices = BTreeMap::from([(path.clone(), ProjectionMergeChoice::Local)]);
    assert!(
        reader
            .apply_projection_rebase(store, &stale, &choices)
            .is_err()
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"local conflict\n");
    let preview = reader
        .preview_projection_rebase(store, local.sequence, layer)
        .unwrap();
    let merged = reader
        .apply_projection_rebase(store, &preview, &choices)
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"local conflict\n");
    assert_eq!(
        reader
            .publish_projection(store, merged.sequence, layer)
            .unwrap()
            .receipt
            .receipt
            .status,
        ReceiptStatus::Applied
    );
    let before_restore = store.state().unwrap();
    let current_capture = store.capture_patch(before_restore.generation).unwrap();
    let selection = layrs_client_core::native_scm::remote::ProjectionHistoricalSelection {
        step_id: historical_step,
        paths: vec![],
    };
    let mut denied = selection.clone();
    denied.paths = vec![RepoPath::parse("private.txt").unwrap()];
    assert!(
        reader
            .preview_projection_historical_restore(store, current_capture.sequence, layer, &denied)
            .is_err()
    );
    let historical = reader
        .preview_projection_historical_restore(store, current_capture.sequence, layer, &selection)
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"local conflict\n");
    let restored = reader
        .apply_projection_rebase(store, &historical, &BTreeMap::new())
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"a\nb\nc\n");
    assert_eq!(
        store.state().unwrap().snapshot.base_step_id,
        before_restore.snapshot.base_step_id
    );
    assert_eq!(
        reader
            .apply_projection_rebase(store, &historical, &BTreeMap::new())
            .unwrap(),
        restored
    );
    assert!(!store.checkout_root().join("private.txt").exists());
    assert_eq!(
        reader
            .publish_projection(store, restored.sequence, layer)
            .unwrap()
            .receipt
            .receipt
            .status,
        ReceiptStatus::Applied
    );
    store.fsck().unwrap();
    eprintln!(
        "HTTPS partial historical restore: complete writable leaves, forbidden selection refused, current remote base preserved and replay PASS"
    );
    eprintln!(
        "HTTPS partial rebase: disjoint text merge, durable draft, exact replay, unresolved conflict, stale View and explicit resolution PASS"
    );
}
