//! Native recipe extension wired into the retained source-publication fixture.
//! Call outside every owner/catalog gate, after SourceReady and Published.
use super::*;
use crate::projection::provider::{
    database_io,
    records::{
        Record,
        import::ProviderTransitionReceiptRecord,
        transition::{ProviderTransitionIntentRecord, TransitionState},
    },
};

/// Real callback path against the source fixture's installed base README.
/// Ready blocks capture; Published admits an explicit full mutation gate.
/// Obtaining that gate here performs no capture or SQL mutation.
pub(super) fn assert_retained_terminal_reads(
    session: &ProviderActiveSession,
    expected: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) {
    assert!(matches!(
        expected.state,
        TransitionState::Ready | TransitionState::Published
    ));
    let cancel = Cancellation::default();
    let held = &session.source.held;
    let source_path = held.artifacts.database.path();
    let family_path = held.family.catalog.path();
    let source_before = database_io::tests::support::witness(source_path);
    let family_before = database_io::tests::support::witness(family_path);
    session.source.validate().unwrap();
    let proof = {
        let _locks = held
            .family
            .locks(
                &held.artifacts.allocation.format.identity.worktree_id,
                false,
            )
            .unwrap();
        let proof = capture_state::require_exact_live(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
        )
        .unwrap_or_else(|failure| panic!("live terminal read: {}", failure.cause));
        assert_eq!(proof.transitions.len(), 1);
        assert_eq!(&proof.transitions[0], expected);
        assert_eq!(proof.receipts.len(), 1);
        assert_eq!(
            proof.receipts[0].encode().unwrap(),
            receipt.encode().unwrap()
        );
        assert!(
            capture_state::read(held, Some(&session.source.root), Some(&cancel)).is_err(),
            "legacy protocol reader remains strict"
        );
        proof
    };
    let descriptor = proof
        .active
        .source
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.path.as_str() == "README.md")
        .unwrap();
    let base = b"provider base content";
    assert_eq!(descriptor.bytes, base.len() as u64);
    let mut output = vec![0x7f; base.len()];
    // This is precisely LocalSource's SnapshotSource entry point, including
    // range admission, exact descriptor/CAS bytes and final positive closes.
    layrs_vfs::SnapshotSource::read_exact_at(
        session.source.as_ref(),
        &descriptor.file_id,
        0,
        &mut output,
        &cancel,
    )
    .unwrap();
    assert_eq!(output, base);
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let mut untouched = [0x7f; 3];
    assert!(matches!(
        layrs_vfs::SnapshotSource::read_exact_at(
            session.source.as_ref(),
            &descriptor.file_id,
            0,
            &mut untouched,
            &cancelled
        ),
        Err(layrs_vfs::Error::Cancelled)
    ));
    assert_eq!(untouched, [0x7f; 3]);
    let mutation = super::super::capture_sql::observe(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
    );
    if expected.state == TransitionState::Ready {
        assert!(mutation.is_err(), "Ready remains pending for capture");
    } else {
        let mutation = mutation.unwrap();
        let (intents, receipts) = mutation.outgoing();
        assert_eq!(intents, std::slice::from_ref(expected));
        assert_eq!(receipts, std::slice::from_ref(receipt));
        assert_eq!(mutation.capture().patches, proof.capture.patches);
    }
    assert_eq!(
        database_io::tests::support::witness(source_path),
        source_before
    );
    assert_eq!(
        database_io::tests::support::witness(family_path),
        family_before
    );
    if expected.state == TransitionState::Ready {
        assert!(proof.require_no_pending_transition().is_err());
    } else {
        proof.require_no_pending_transition().unwrap();
    }
}
