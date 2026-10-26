//! Owned end-to-end capture recipes. Only the named child harness runs native
//! CFAPI and real provider SQL; each recipe begins from a new fixture.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::capture_family::{
        active_holder_with_private, active_holder_with_writable_file_private,
    },
    process_profile::tests::child,
    records::{AllocationRecord, PreparedRecord, ProviderBootstrapBinding},
    validation,
};
use layrs_core::scm::FileId;
use layrs_vfs::{Catalog, SnapshotSource};
use rusqlite::{Connection, OpenFlags};
use std::{fs, sync::Arc};

fn remove_placeholder(path: &std::path::Path) {
    if fs::remove_file(path).is_err() {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
        fs::remove_file(path).unwrap();
    }
}

struct NoRead(Catalog);
impl SnapshotSource for NoRead {
    fn catalog(&self) -> &Catalog {
        &self.0
    }
    fn read_exact_at(
        &self,
        file_id: &FileId,
        offset: u64,
        output: &mut [u8],
        _: &Cancellation,
    ) -> layrs_vfs::Result<()> {
        Err(layrs_vfs::Error::MissingBytes {
            file_id: file_id.clone(),
            offset,
            length: output.len(),
        })
    }
}

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned native capture recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                240,
                || {
                    child::declare();
                    $body
                },
            );
        }
    };
}

fn reopen(
    f: &crate::projection::provider::bootstrap::tests::fixture::Fixture,
    binding: &ProviderBootstrapBinding,
) -> ProviderActiveReopened {
    PartialStoreFamily::open_provider_active(
        f.family.root(),
        &f.worktree,
        &binding.transaction_id,
        binding,
    )
    .unwrap()
}

fn private_proof(
    f: &crate::projection::provider::bootstrap::tests::fixture::Fixture,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
) -> validation::PrivateCaptureProof {
    let c = Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let inspected = validation::private_active_capture_with_source(
        &c,
        allocation,
        prepared,
        binding,
        Some([0x63; 16]),
        None,
    )
    .unwrap();
    c.close().unwrap();
    inspected.capture
}

fn assert_reservation_released(
    f: &crate::projection::provider::bootstrap::tests::fixture::Fixture,
) {
    let c = Connection::open_with_flags(
        f.family.root().join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count: u64 = c
        .query_row(
            "SELECT count(*) FROM reservations WHERE owner=?1",
            [f.worktree.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    c.close().unwrap();
    assert_eq!(
        count, 0,
        "acknowledged capture releases its remaining quota reservation"
    );
}

owned!(empty_capture_is_durable_and_deduplicates_across_attempts, {
    let (f, holder, allocation, prepared, binding) = active_holder_with_private();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
    session.connect().unwrap();
    let first = session
        .capture("empty-attempt-one", &Cancellation::default())
        .unwrap();
    assert_eq!(first.transaction_id, "empty-attempt-one");
    assert_eq!(first.patch.sequence, 1);
    assert!(first.patch.changes.is_empty());
    assert_reservation_released(&f);
    let repeated = session
        .capture("empty-attempt-one", &Cancellation::default())
        .unwrap();
    assert_eq!(repeated.receipt_digest, first.receipt_digest);
    assert_eq!(repeated.patch, first.patch);
    let second = session
        .capture("empty-attempt-two", &Cancellation::default())
        .unwrap();
    assert_eq!(
        second.patch, first.patch,
        "identical image reuses durable capture head"
    );
    assert_ne!(second.receipt_digest, first.receipt_digest);
    assert_reservation_released(&f);
    session.disconnect().unwrap();
    drop(session);
    let reopened = reopen(&f, &binding);
    reopened.revalidate().unwrap();
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert_eq!(proof.transactions.len(), 2);
    assert_eq!(proof.receipts.len(), 2);
    assert_eq!(proof.patches.len(), 1);
    assert_eq!(proof.claims.len(), 2);
    let mut resumed = reopened
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("reopened session withheld: {}", error.cause()));
    resumed.connect().unwrap();
    let durable = resumed
        .capture("empty-attempt-one", &Cancellation::default())
        .unwrap();
    assert_eq!(durable.receipt_digest, first.receipt_digest);
    resumed.disconnect().unwrap();
});

owned!(cancel_before_first_write_preserves_both_databases, {
    let (f, holder, allocation, prepared, binding) = active_holder_with_private();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
    session.connect().unwrap();
    // The checkout root is now a CFAPI reparse point. The ordinary bootstrap
    // witness also opens checkout/.layrs/FORMAT and correctly refuses it;
    // this cancellation predicate concerns the two ordinary metadata files.
    let database_witness = || {
        [
            f.family.root().join("metadata/catalog.sqlite3"),
            f.metadata.join("projection.sqlite3"),
        ]
        .map(|path| crate::projection::provider::database_io::tests::support::witness(&path))
    };
    let before = database_witness();
    let cancel = Cancellation::default();
    cancel.cancel();
    let failure = session.capture("cancel-before-write", &cancel).unwrap_err();
    assert_eq!(failure.step, ProviderCaptureStep::Checkpoint);
    assert!(matches!(
        failure.cause,
        Cause::Vfs(layrs_vfs::Error::Cancelled)
    ));
    assert_eq!(database_witness(), before);
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert!(proof.transactions.is_empty());
    assert!(proof.receipts.is_empty());
    assert!(proof.patches.is_empty());
    // A fresh token must still complete; cancellation has granted no SQL state.
    let accepted = session
        .capture("cancel-before-write", &Cancellation::default())
        .unwrap();
    assert_eq!(accepted.patch.sequence, 1);
    session.disconnect().unwrap();
});

owned!(
    writable_replacement_capture_reopens_with_fixed_base_reader,
    {
        let (f, holder, allocation, prepared, binding) = active_holder_with_writable_file_private();
        let source =
            crate::projection::provider::bootstrap::tests::fixture::Fixture::writable_file_source();
        let baseline = source
            .descriptors()
            .iter()
            .find(|row| row.path.as_str() == "README.md")
            .unwrap()
            .file_id
            .clone();
        let mut session = holder
            .into_disconnected_session()
            .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
        session.connect().unwrap();
        let path = f.checkout.join("README.md");
        assert!(
            path.exists(),
            "writable cold file has an installed placeholder"
        );
        remove_placeholder(&path); // metadata-only; never hydrates the cold original
        let replacement = b"provider capture ordinary replacement";
        fs::write(&path, replacement).unwrap();
        let accepted = session
            .capture("writable-replacement", &Cancellation::default())
            .unwrap();
        assert_eq!(accepted.patch.sequence, 1);
        assert_eq!(accepted.patch.changes.len(), 1);
        assert_reservation_released(&f);
        let change = &accepted.patch.changes[0];
        assert_eq!(change.path.as_str(), "README.md");
        assert_eq!(
            change.expected.as_ref().map(|file| &file.file_id),
            Some(&baseline)
        );
        let target = change.replacement.as_ref().expect("replacement target");
        assert_eq!(target.file_id, FileId::from_bytes(replacement));
        assert_eq!(target.bytes, replacement.len() as u64);
        session.disconnect().unwrap();
        drop(session);
        let reopened = reopen(&f, &binding);
        reopened.revalidate().unwrap();
        let proof = private_proof(&f, &allocation, &prepared, &binding);
        assert_eq!(proof.transactions.len(), 1);
        assert_eq!(proof.receipts.len(), 1);
        assert_eq!(proof.content.len(), 1);
        assert_eq!(proof.claims.len(), 1);
        let mut one = [0u8; 1];
        assert!(matches!(
            reopened.read_resident_range(&baseline, 0, &mut one, &Cancellation::default()),
            Err(ProviderActiveRangeFailure::NeedsHydration { .. })
        ));
        assert!(matches!(
            reopened.read_resident_range(&target.file_id, 0, &mut one, &Cancellation::default()),
            Err(ProviderActiveRangeFailure::InvalidRange)
        ));
        let mut resumed = reopened
            .into_disconnected_session()
            .unwrap_or_else(|error| panic!("reopened session withheld: {}", error.cause()));
        resumed.connect().unwrap();
        let durable = resumed
            .capture("writable-replacement", &Cancellation::default())
            .unwrap();
        assert_eq!(durable.receipt_digest, accepted.receipt_digest);
        resumed.disconnect().unwrap();
    }
);

owned!(writable_placeholder_deletion_captures_expected_base, {
    let (f, holder, allocation, prepared, binding) = active_holder_with_writable_file_private();
    let base =
        crate::projection::provider::bootstrap::tests::fixture::Fixture::writable_file_source()
            .descriptors()
            .iter()
            .find(|row| row.path.as_str() == "README.md")
            .unwrap()
            .file_id
            .clone();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("delete session withheld: {}", error.cause()));
    session.connect().unwrap();
    let path = f.checkout.join("README.md");
    assert!(path.exists());
    remove_placeholder(&path); // no cold byte read or callback hydration
    assert!(!path.exists());
    let ack = session
        .capture("writable-deletion", &Cancellation::default())
        .unwrap();
    assert_eq!(ack.patch.changes.len(), 1);
    let change = &ack.patch.changes[0];
    assert_eq!(change.path.as_str(), "README.md");
    assert_eq!(
        change.expected.as_ref().map(|file| &file.file_id),
        Some(&base)
    );
    assert!(change.replacement.is_none());
    assert_reservation_released(&f);
    session.disconnect().unwrap();
    drop(session);
    let reopened = reopen(&f, &binding);
    reopened.revalidate().unwrap();
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert_eq!(proof.patches.len(), 1);
    assert_eq!(proof.receipts.len(), 1);
    assert!(proof.content.is_empty());
    assert_eq!(proof.claims.len(), 1);
    let mut one = [0u8; 1];
    assert!(matches!(
        reopened.read_resident_range(&base, 0, &mut one, &Cancellation::default()),
        Err(ProviderActiveRangeFailure::NeedsHydration { .. })
    ));
    let mut resumed = reopened
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("delete reopen withheld: {}", error.cause()));
    resumed.connect().unwrap();
    let durable = resumed
        .capture("writable-deletion", &Cancellation::default())
        .unwrap();
    assert_eq!(durable.receipt_digest, ack.receipt_digest);
    resumed.disconnect().unwrap();
});

owned!(protected_placeholder_mutation_refuses_accepted_history, {
    let (f, holder, allocation, prepared, binding) = active_holder_with_private();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("protected session withheld: {}", error.cause()));
    session.connect().unwrap();
    let path = f.checkout.join("README.md");
    assert!(path.exists());
    remove_placeholder(&path);
    fs::write(&path, b"forbidden ordinary replacement").unwrap();
    let family_database = f.family.root().join("metadata/catalog.sqlite3");
    let before_family =
        crate::projection::provider::database_io::tests::support::witness(&family_database);
    let failure = session
        .capture("protected-mutation", &Cancellation::default())
        .unwrap_err();
    assert_eq!(failure.step, ProviderCaptureStep::Hashing);
    assert!(failure
        .cause
        .to_string()
        .contains("provider protected file changed"));
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert!(proof.patches.is_empty());
    assert!(proof.receipts.is_empty());
    assert!(proof.content.is_empty());
    assert!(proof.claims.is_empty());
    assert_eq!(
        crate::projection::provider::database_io::tests::support::witness(&family_database),
        before_family
    );
    assert_eq!(proof.transactions.len(), 1);
    assert!(matches!(
        proof.transactions[0].state,
        ProviderCaptureState::Hashing
    ));
    // A refused image can leave its durable Hashing intent. Explicit
    // cancellation records abandonment; it never fabricates an ACK.
    session.cancel_capture("protected-mutation").unwrap();
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert!(matches!(
        proof.transactions[0].state,
        ProviderCaptureState::Cancelled { stage: None }
    ));
    assert!(proof.patches.is_empty() && proof.receipts.is_empty() && proof.claims.is_empty());
    assert_eq!(
        crate::projection::provider::database_io::tests::support::witness(&family_database),
        before_family
    );
    session.disconnect().unwrap();
});

owned!(durable_staging_reservation_resumes_same_native_image, {
    let (f, holder, allocation, prepared, binding) = active_holder_with_private();
    let source: Arc<dyn SnapshotSource> = Arc::new(NoRead(holder.root.catalog().clone()));
    let native = layrs_vfs::Session::reconnect_disconnected_with_control_directory(
        holder.root.try_clone().unwrap(),
        source,
    )
    .unwrap();
    let mut checkpoint = native.checkpoint().unwrap();
    let initial = capture_sql::observe(
        &holder.held,
        &holder.root,
        &holder.record,
        &holder.activation,
    )
    .unwrap();
    let image = capture_image::plan(
        &mut checkpoint,
        &initial.active.source,
        &initial.active.activated.snapshot,
        &Cancellation::default(),
    )
    .unwrap();
    assert!(image.changes.is_empty());
    let transaction = "resume-staging-reservation";
    let hashing = ProviderCaptureTransactionRecord::begin(
        &allocation,
        &holder.record,
        &holder.activation,
        transaction.into(),
        0,
        None,
    )
    .unwrap();
    // This recipe constructs a durable prefix with production primitives. It
    // does not claim to inject a process crash or an uncertain SQLite close.
    capture_sql::begin(
        &holder.held,
        &holder.root,
        &holder.record,
        &holder.activation,
        &hashing,
        &mut Close,
    )
    .unwrap();
    let mut staged = hashing.clone();
    let reservation = super::reservation(&hashing);
    staged.state = ProviderCaptureState::Staging {
        stage: ProviderCaptureStage {
            image_digest: image.image_digest,
            image_path_count: image.image_path_count,
            changes: image.changes,
            reservation: reservation.clone(),
        },
    };
    capture_sql::transition(
        &holder.held,
        &holder.root,
        &holder.record,
        &holder.activation,
        &hashing,
        &staged,
        &checkpoint,
        &mut Close,
    )
    .unwrap();
    let proof = capture_sql::observe(
        &holder.held,
        &holder.root,
        &holder.record,
        &holder.activation,
    )
    .unwrap()
    .capture;
    let content = lease(&holder.held, proof).unwrap();
    let operation = format!(
        "provider-capture:{}",
        blake3::hash(transaction.as_bytes()).to_hex()
    );
    content
        .reserve_exact(
            &reservation,
            &f.worktree,
            &operation,
            retention::budget_for(&[], 0).unwrap(),
        )
        .unwrap();
    drop(content);
    let base = &initial.active.source.descriptors()[0].file_id;
    holder
        .read_resident_range(base, 0, &mut [], &Cancellation::default())
        .unwrap();
    drop(checkpoint);
    drop(native);
    drop(holder);
    let reopened = reopen(&f, &binding);
    reopened.revalidate().unwrap();
    let mut session = reopened
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("reopened session withheld: {}", error.cause()));
    session.connect().unwrap();
    let ack = session
        .capture(transaction, &Cancellation::default())
        .unwrap();
    assert_eq!(ack.patch.sequence, 1);
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert_eq!(proof.transactions.len(), 1);
    assert!(matches!(&proof.transactions[0].state,
        ProviderCaptureState::Accepted { stage, .. } if stage.reservation == reservation));
    assert_eq!(proof.receipts.len(), 1);
    assert_eq!(proof.claims.len(), 1);
    session.disconnect().unwrap();
    drop(session);

    // Construct a second durable Staging prefix after the first acknowledgement.
    // This is an exact fixture predecessor for cancellation, not fault injection.
    let second = reopen(&f, &binding);
    let source: Arc<dyn SnapshotSource> = Arc::new(NoRead(second.root.catalog().clone()));
    let native = layrs_vfs::Session::reconnect_disconnected_with_control_directory(
        second.root.try_clone().unwrap(),
        source,
    )
    .unwrap();
    let mut checkpoint = native.checkpoint().unwrap();
    let observed = capture_sql::observe(
        &second.held,
        &second.root,
        &second.record,
        &second.activation,
    )
    .unwrap();
    let image = capture_image::plan(
        &mut checkpoint,
        &observed.active.source,
        &observed.active.activated.snapshot,
        &Cancellation::default(),
    )
    .unwrap();
    assert!(image.changes.is_empty());
    let cancelled_id = "cancel-staged-reservation";
    let hashing = ProviderCaptureTransactionRecord::begin(
        &allocation,
        &second.record,
        &second.activation,
        cancelled_id.into(),
        observed.capture.patches.len() as u64,
        observed
            .capture
            .patches
            .last()
            .map(|patch| patch.digest.clone()),
    )
    .unwrap();
    capture_sql::begin(
        &second.held,
        &second.root,
        &second.record,
        &second.activation,
        &hashing,
        &mut Close,
    )
    .unwrap();
    let cancelled_reservation = super::reservation(&hashing);
    let mut staged = hashing.clone();
    staged.state = ProviderCaptureState::Staging {
        stage: ProviderCaptureStage {
            image_digest: image.image_digest,
            image_path_count: image.image_path_count,
            changes: image.changes,
            reservation: cancelled_reservation.clone(),
        },
    };
    capture_sql::transition(
        &second.held,
        &second.root,
        &second.record,
        &second.activation,
        &hashing,
        &staged,
        &checkpoint,
        &mut Close,
    )
    .unwrap();
    let proof = capture_sql::observe(
        &second.held,
        &second.root,
        &second.record,
        &second.activation,
    )
    .unwrap()
    .capture;
    let content = lease(&second.held, proof).unwrap();
    let operation = format!(
        "provider-capture:{}",
        blake3::hash(cancelled_id.as_bytes()).to_hex()
    );
    content
        .reserve_exact(
            &cancelled_reservation,
            &f.worktree,
            &operation,
            retention::budget_for(&[], 0).unwrap(),
        )
        .unwrap();
    drop(content);
    drop(checkpoint);
    drop(native);
    drop(second);

    let mut session = reopen(&f, &binding)
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("cancel session withheld: {}", error.cause()));
    session.connect().unwrap();
    session.cancel_capture(cancelled_id).unwrap();
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert!(proof
        .transactions
        .iter()
        .any(|row| row.transaction_id == cancelled_id
            && matches!(&row.state, ProviderCaptureState::Cancelled { stage: Some(stage) }
                if stage.reservation == cancelled_reservation)));
    let catalog = Connection::open_with_flags(
        f.family.root().join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let remaining: u64 = catalog
        .query_row(
            "SELECT remaining FROM reservations WHERE id=?1 AND owner=?2 AND operation=?3",
            rusqlite::params![cancelled_reservation, f.worktree.as_str(), operation],
            |row| row.get(0),
        )
        .unwrap();
    assert!(remaining > 0, "cancelled attempt retains its family budget");
    catalog.close().unwrap();
    let fresh = session
        .capture("after-cancel", &Cancellation::default())
        .unwrap();
    assert_eq!(
        fresh.patch, ack.patch,
        "new attempt may dedup unchanged image"
    );
    let proof = private_proof(&f, &allocation, &prepared, &binding);
    assert_eq!(proof.transactions.len(), 3);
    assert_eq!(proof.receipts.len(), 2);
    assert_eq!(proof.patches.len(), 1);
    assert_eq!(proof.claims.len(), 2);
    session.disconnect().unwrap();
});
