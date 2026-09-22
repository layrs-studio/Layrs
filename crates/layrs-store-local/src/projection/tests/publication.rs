use super::capture::ready;
use super::*;
const REQUEST: &[u8] = br#"{"request":"exact-immutable-submission"}"#;
const RECEIPT: &[u8] = br#"{"result":"applied"}"#;
fn pending() -> (Fixture, ProjectionStore, CapturedPatch, ProjectionSnapshot) {
    let (f, store) = ready(false);
    fs::write(f.0.join("a.txt"), b"published bytes").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let mut next = store.state().unwrap().snapshot;
    next.base_step_id = StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap();
    next.base_root_tree_id = TreeId::parse(format!("blake3:{}", "3".repeat(64))).unwrap();
    next.entries[0].target = Some(TreeEntryTarget::file(FileId::from_bytes(
        b"published bytes",
    )));
    (f, store, capture, next)
}
struct StopPublication(PublicationPoint);
impl PublicationFaults for StopPublication {
    fn check(&self, p: PublicationPoint) -> io::Result<()> {
        if p == self.0 {
            Err(io::Error::other("publication interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn pending_publication_survives_reopen_blocks_mutation_and_keeps_exact_terminal_receipt() {
    let (f, store, capture, next) = pending();
    assert!(
        store
            .begin_publication_with_faults(
                capture.sequence,
                1,
                REQUEST,
                &StopPublication(PublicationPoint::RequestDurable)
            )
            .is_err()
    );
    let reopened = ProjectionStore::open(&f.0).unwrap();
    let saved = reopened.pending_publication().unwrap().unwrap();
    assert_eq!(saved.request_bytes, REQUEST);
    assert_eq!(
        reopened
            .begin_publication(capture.sequence, 1, REQUEST)
            .unwrap(),
        saved
    );
    assert!(
        reopened
            .begin_publication(capture.sequence, 1, br#"{"different":true}"#)
            .is_err()
    );
    assert!(reopened.capture_patch(1).is_err());
    assert!(reopened.compare_and_swap(1, &next).is_err());
    assert!(
        reopened
            .install_projection(1, &next, &mut |_, _| panic!(
                "network callback under pending publication"
            ))
            .is_err()
    );
    assert_eq!(reopened.state().unwrap().generation, 1);
    assert_eq!(reopened.captured_patch(capture.sequence).unwrap(), capture);
    let receipt = reopened
        .record_publication_receipt(capture.sequence, RECEIPT)
        .unwrap();
    assert_eq!(receipt.receipt_bytes.as_deref(), Some(RECEIPT));
    assert_eq!(
        reopened
            .begin_publication(capture.sequence, 1, REQUEST)
            .unwrap(),
        receipt
    );
    assert_eq!(
        ProjectionStore::open(&f.0)
            .unwrap()
            .pending_publication()
            .unwrap(),
        Some(receipt)
    );
    assert!(
        reopened
            .record_publication_receipt(capture.sequence, br#"{"result":"rejected"}"#)
            .is_err()
    );
    assert!(
        reopened
            .finish_publication(capture.sequence, br#"{"result":"rejected"}"#, None)
            .is_err()
    );
    assert!(
        reopened
            .finish_publication_with_faults(
                capture.sequence,
                RECEIPT,
                Some(&next),
                &StopPublication(PublicationPoint::BeforeReceiptCommit)
            )
            .is_err()
    );
    assert_eq!(reopened.state().unwrap().generation, 1);
    assert!(reopened.pending_publication().unwrap().is_some());
    assert!(
        reopened
            .finish_publication_with_faults(
                capture.sequence,
                RECEIPT,
                Some(&next),
                &StopPublication(PublicationPoint::ReceiptDurable)
            )
            .is_err()
    );
    assert!(reopened.pending_publication().unwrap().is_none());
    let state = reopened.state().unwrap();
    assert_eq!(state.generation, 2);
    let completed = reopened
        .completed_publication(capture.sequence)
        .unwrap()
        .unwrap();
    assert_eq!(completed.receipt_bytes, RECEIPT);
    assert_eq!(completed.projection, state);
    assert_eq!(
        reopened
            .finish_publication(capture.sequence, RECEIPT, Some(&next))
            .unwrap(),
        state
    );
    assert!(
        reopened
            .finish_publication(capture.sequence, RECEIPT, None)
            .is_err()
    );
    assert!(reopened.capture_patch(2).unwrap().changes.is_empty());
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"published bytes");
}
#[test]
fn receipt_cannot_change_reserved_uncaptured_paths_or_authorization_observation() {
    let (_f, store, capture, next) = pending();
    store
        .begin_publication(capture.sequence, 1, REQUEST)
        .unwrap();
    let mut invalids = Vec::new();
    let mut bad = next.clone();
    bad.entries[2].state = EntryState::Materialized;
    invalids.push(bad);
    let mut bad = next.clone();
    bad.entries[1].target = Some(TreeEntryTarget::file(FileId::from_bytes(b"other")));
    invalids.push(bad);
    let mut bad = next.clone();
    bad.observed_authorization_epoch += 1;
    invalids.push(bad);
    let mut bad = next.clone();
    bad.entries[0].writable = false;
    invalids.push(bad);
    for bad in invalids {
        assert!(
            store
                .finish_publication(capture.sequence, RECEIPT, Some(&bad))
                .is_err()
        );
        assert_eq!(store.state().unwrap().generation, 1);
        assert!(store.pending_publication().unwrap().is_some());
    }
    let receipt = br#"{"result":"rejected"}"#;
    let before = store.state().unwrap();
    assert_eq!(
        store
            .finish_publication(capture.sequence, receipt, None)
            .unwrap(),
        before
    );
    assert!(store.pending_publication().unwrap().is_none());
    assert_eq!(store.captured_patch(capture.sequence).unwrap(), capture);
}
#[test]
fn journal_rejects_noncanonical_oversized_and_corrupted_requests_before_replay() {
    let (f, store, capture, _) = pending();
    for bytes in [b"{} ".as_slice(), b"null", b"{\"a\":1,\"a\":1}"] {
        assert!(store.begin_publication(capture.sequence, 1, bytes).is_err());
    }
    assert!(
        store
            .begin_publication(capture.sequence, 1, &vec![b' '; 24 * 1024 * 1024 + 1])
            .is_err()
    );
    assert!(store.pending_publication().unwrap().is_none());
    store
        .begin_publication(capture.sequence, 1, REQUEST)
        .unwrap();
    let db = rusqlite::Connection::open(f.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute(
        "UPDATE projection_publication_pending SET request_bytes=?1",
        [br#"{"tampered":true}"#.as_slice()],
    )
    .unwrap();
    drop(db);
    assert!(store.pending_publication().is_err());
    assert!(
        store
            .record_publication_receipt(capture.sequence, RECEIPT)
            .is_err()
    );
    assert!(store.capture_patch(1).is_err());
}

#[test]
fn completed_receipt_checks_the_exact_capture_transition_before_replay() {
    for altered_next in [Some(b"{}".as_slice()), None] {
        let (f, store, capture, next) = pending();
        store
            .begin_publication(capture.sequence, 1, REQUEST)
            .unwrap();
        store
            .finish_publication(capture.sequence, RECEIPT, Some(&next))
            .unwrap();
        assert!(store.fsck().is_ok());
        let db =
            rusqlite::Connection::open(f.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
        db.execute(
            "UPDATE projection_publications SET next_snapshot=?1",
            [altered_next],
        )
        .unwrap();
        drop(db);
        assert!(store.completed_publication(capture.sequence).is_err());
        assert!(store.fsck().is_err());
        assert!(
            store
                .finish_publication(capture.sequence, RECEIPT, None)
                .is_err()
        );
        assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"published bytes");
    }
}
