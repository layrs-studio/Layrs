//! Native recipe extension only. ROOT calls this AFTER dropping the original
//! attempt lock and every managed proof/lease/image/checkpoint, before draining
//! the still-connected session. No new fixture, fake SQL, BUSY or cleanup.
use super::*;
use crate::projection::{
    provider::{
        database_io,
        records::{
            capture::{ProviderCaptureReceiptRecord, ProviderCaptureState},
            import::ProviderTransitionReceiptRecord,
            transition::{ProviderTransitionIntentRecord, TransitionState},
            Record,
        },
    },
    shared::{provider_access::ActivationRegistration, provider_content},
};
use layrs_core::{
    projection::EntryState,
    scm::{FileId, FileManifestId, TreeEntryTarget},
};
use std::fs;

const ATTEMPT: &str = "after-source-published";
const REPLACEMENT: &[u8] = b"authorized README replacement after SourcePublished";

pub(super) fn capture_after_published(
    session: &mut ProviderActiveSession,
    expected: &ProviderTransitionIntentRecord,
    outgoing_receipt: &ProviderTransitionReceiptRecord,
) {
    assert_eq!(expected.state, TransitionState::Published);
    expected.require_receipt(outgoing_receipt).unwrap();
    assert!(session.is_connected());
    let cancel = Cancellation::default();
    let before = super::super::capture_sql::observe(
        &session.source.held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
    )
    .unwrap();
    assert!(!before
        .capture()
        .transactions
        .iter()
        .any(|tx| tx.transaction_id == ATTEMPT));
    let (transitions, receipts) = before.outgoing();
    assert_eq!(transitions.len(), 1);
    assert_eq!(receipts.len(), 1);
    assert_eq!(transitions[0].encode().unwrap(), expected.encode().unwrap());
    assert_eq!(
        receipts[0].encode().unwrap(),
        outgoing_receipt.encode().unwrap()
    );
    let entry = before
        .active()
        .activated
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "README.md")
        .unwrap();
    assert!(entry.writable && entry.state == EntryState::Materialized);
    let Some(TreeEntryTarget::File { file_id: base_file }) = &entry.target else {
        panic!("fixture README must have its original authorized file base")
    };
    let replacement_id = FileId::from_bytes(REPLACEMENT);
    assert_ne!(
        &replacement_id, base_file,
        "this must be a real new capture change"
    );
    let old_capture_count = before.capture().patches.len();
    let old_capture_head = before
        .capture()
        .patches
        .last()
        .map(|patch| patch.digest.clone());
    let readme = session.root().join("README.md");
    // The root and fixed literal path are already authorized by the original
    // writable snapshot above. No permission change, delete or orphan adoption.
    fs::write(&readme, REPLACEMENT).unwrap();
    let acknowledgement = session.capture(ATTEMPT, &cancel).unwrap();
    assert_eq!(acknowledgement.transaction_id, ATTEMPT);
    assert_eq!(acknowledgement.patch.sequence, old_capture_count as u64 + 1);
    assert_eq!(
        acknowledgement.patch.previous_patch_digest,
        old_capture_head
    );
    assert_eq!(acknowledgement.patch.changes.len(), 1);
    let change = &acknowledgement.patch.changes[0];
    assert_eq!(change.path.as_str(), "README.md");
    assert_eq!(&change.expected.as_ref().unwrap().file_id, base_file);
    let replacement = change.replacement.as_ref().unwrap();
    assert_eq!(replacement.file_id, replacement_id);
    assert_eq!(replacement.bytes, REPLACEMENT.len() as u64);
    assert_eq!(replacement.executable, entry.executable);
    assert_eq!(fs::read(&readme).unwrap(), REPLACEMENT);
    let held = &session.source.held;
    let after = super::super::capture_sql::observe(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
    )
    .unwrap();
    after.require_same_publications(&before).unwrap();
    assert!(
        after.require_same_state(&before).is_err(),
        "the capture state really advanced"
    );
    assert_eq!(after.capture().patches.len(), old_capture_count + 1);
    let transaction = after
        .capture()
        .transactions
        .iter()
        .find(|tx| tx.transaction_id == ATTEMPT)
        .unwrap();
    let ProviderCaptureState::Accepted { stage, patch } = &transaction.state else {
        panic!("a real capture must durably reach Accepted")
    };
    assert_eq!(patch, &acknowledgement.patch);
    let capture_receipt = ProviderCaptureReceiptRecord::derive(transaction).unwrap();
    assert_eq!(
        capture_receipt.digest().unwrap(),
        acknowledgement.receipt_digest
    );
    assert!(after
        .capture()
        .receipts
        .iter()
        .any(|receipt| receipt == &capture_receipt));
    assert!(after
        .capture()
        .claims
        .iter()
        .any(|claim| claim.owner == ATTEMPT && claim.kind == "provider-capture"));
    let (transitions, receipts) = after.outgoing();
    assert_eq!(transitions.len(), 1);
    assert_eq!(receipts.len(), 1);
    assert_eq!(transitions[0].encode().unwrap(), expected.encode().unwrap());
    assert_eq!(
        receipts[0].encode().unwrap(),
        outgoing_receipt.encode().unwrap()
    );
    let content = after
        .capture()
        .content
        .iter()
        .find(|row| row.file_id == replacement_id.as_str())
        .unwrap();
    assert_eq!(content.bytes, REPLACEMENT.len() as u64);
    let manifest = FileManifestId::parse(&content.manifest_id).unwrap();
    let bytes = {
        let locks = held
            .family
            .locks(
                &held.artifacts.allocation.format.identity.worktree_id,
                false,
            )
            .unwrap();
        super::super::managed_destination::valid(
            database_io::read_cancellable(&held.family.catalog, &cancel, |c| {
                assert_eq!(
                    held.family.validate_capture_catalog(
                        c,
                        &held.artifacts.allocation,
                        after.capture(),
                        Some(&cancel)
                    )?,
                    ActivationRegistration::Ready
                );
                assert!(
                    !c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1)",
                        [&stage.reservation],
                        |row| row.get::<_, bool>(0),
                    )?,
                    "the new capture's own accepted reservation was released"
                );
                provider_content::resident_range(
                    &held.family,
                    &locks,
                    c,
                    &replacement_id,
                    Some(&manifest),
                    REPLACEMENT.len() as u64,
                    0,
                    REPLACEMENT.len(),
                    &cancel,
                )
            })
            .unwrap(),
        )
        .unwrap()
        .expect("Accepted replacement bytes must be locally resident")
    };
    // Native/caller output follows the positive read-only catalog close.
    assert_eq!(bytes, REPLACEMENT);
    session.source.validate().unwrap();
    let final_gate = super::super::capture_sql::observe(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
    )
    .unwrap();
    final_gate.require_same_state(&after).unwrap();
    final_gate.require_same_publications(&before).unwrap();
}
