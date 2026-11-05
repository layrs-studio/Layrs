//! Full recovery-opener refusals after genuine begin/ACK gaps and owner loss.
//! Each deliberate fixture corruption is named below. None of these corrupt
//! SQL/FS states is claimed to be reachable through the production writers.
use super::*;
use crate::projection::{
    provider::records::{AllocationRecord, PreparedRecord, ProviderBootstrapBinding},
    shared::source_denial::SourceDeniedCoverageReservation,
};
use rusqlite::{Connection, OpenFlags, params};
use std::path::Path;

#[derive(Clone, Copy)]
pub(super) enum Case {
    OrphanWithoutReceipt,
    ConflictingActualIntent,
    UnownedDocument,
    ForeignOrphan,
}

// Test-only corruption of a fresh owned fixture, after all production owners
// are dropped. This bypasses the protocol deliberately; it supplies no proof.
fn corrupt_fixture_sql(f: &Fixture, path: &Path, mutate: impl FnOnce(&Connection)) {
    assert!(path.is_absolute() && path.starts_with(&f.outer));
    assert!(
        path == f.metadata.join("projection.sqlite3")
            || path == f.family.root().join("metadata/catalog.sqlite3")
    );
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
        .unwrap();
    mutate(&c);
    c.execute_batch("COMMIT").unwrap();
    c.close().unwrap();
}

fn source_rows(path: &Path) -> CatalogSnapshot {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let rows = table_snapshot(&c).unwrap();
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
    rows
}

fn receipt_witness(f: &Fixture) -> Option<(String, u64, String)> {
    let path = f
        .metadata
        .join("provider-control")
        .join(denial_receipt::FILE_NAME);
    match fs::metadata(&path) {
        Ok(_) => Some(database_io::tests::support::witness(&path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("receipt witness failed: {error}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn assert_fresh_refusal(
    f: &Fixture,
    case: Case,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    planned: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    funding: &SourceDeniedCoverageReservation,
    cancel: &Cancellation,
) {
    let source_path = f.metadata.join("projection.sqlite3");
    let family_path = f.family.root().join("metadata/catalog.sqlite3");
    assert!(
        receipt_witness(f).is_some(),
        "real begin did not retain receipt at the ACK gap"
    );
    let expected = match case {
        Case::OrphanWithoutReceipt => {
            // Deliberate FS fixture corruption: remove the real orphan's
            // receipt using its authenticated pinned-inode cleanup primitive.
            // This intentionally violates the production cleanup precondition.
            denial_receipt::reopen(allocation, prepared, Some(cancel))
                .unwrap()
                .unwrap()
                .cleanup(Some(cancel))
                .unwrap();
            assert!(receipt_witness(f).is_none());
            "provider capture has unproven family reservation"
        }
        Case::ConflictingActualIntent => {
            // Deliberate SQL fixture corruption of the real durable Planned:
            // change an immutable plan field while retaining its actual body,
            // transaction, row scalars and genuine original compact receipt.
            let mut changed =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            changed.plan.quota_bytes = changed.plan.quota_bytes.checked_add(1).unwrap();
            changed.encode().unwrap();
            assert_ne!(
                changed.plan.digest().unwrap(),
                planned.plan.digest().unwrap()
            );
            corrupt_fixture_sql(f, &source_path, |c| {
                assert_eq!(c.execute(
                    "UPDATE projection_provider_transitions SET payload=?1,digest=?2 WHERE id=?3 AND payload=?4 AND digest=?5",
                    params![changed.encode().unwrap(), changed.digest().unwrap(), planned.transaction_id,
                        planned.encode().unwrap(), planned.digest().unwrap()],
                ).unwrap(), 1);
            });
            // Establish the exact earlier reader used by recover still accepts
            // all source SQL: codec, projected scalars, body and inventory. This
            // is actual SQL DATA with root=None, never a forged native guard.
            let c = Connection::open_with_flags(&source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
            c.execute_batch("BEGIN DEFERRED").unwrap();
            let full = validation::private_active_publication_with_source(
                &c,
                allocation,
                prepared,
                binding,
                None,
                Some(cancel),
            )
            .unwrap();
            assert_eq!(full.source_transitions(), std::slice::from_ref(&changed));
            let actual = SourceDeniedCoverageReservation::new(
                allocation,
                &changed,
                coverage.record(),
                Some(cancel),
            )
            .unwrap();
            assert_eq!(
                full.capture_data().denial_reservations(),
                std::slice::from_ref(&actual)
            );
            assert_ne!(actual.operation(), funding.operation());
            c.execute_batch("ROLLBACK").unwrap();
            c.close().unwrap();
            "denial receipt conflicts with actual source intent"
        }
        Case::UnownedDocument => {
            // Deliberate SQL fixture corruption: commit the genuine canonical
            // body alone. The real start always inserts body + intent atomically.
            corrupt_fixture_sql(f, &source_path, |c| {
                assert_eq!(
                    c.execute(
                        "INSERT INTO projection_observations(digest,payload) VALUES(?1,?2)",
                        params![
                            coverage.record().reference().unwrap().id,
                            coverage.record().encode().unwrap()
                        ],
                    )
                    .unwrap(),
                    1
                );
            });
            // Full publication interpretation rejects exact inventory first.
            // This does NOT independently qualify actual_funding's EXISTS arm.
            "source denied coverage exact inventory differs"
        }
        Case::ForeignOrphan => {
            // Deliberate family SQL fixture corruption: another unproven denial
            // operation of this same owner, foreign to the one exact release.
            // A different worktree's reservation would be outside this scope.
            let mut foreign = format!("provider-denial:{}", "f".repeat(64));
            if foreign == funding.operation() {
                foreign = format!("provider-denial:{}", "e".repeat(64));
            }
            assert_ne!(foreign, funding.operation());
            corrupt_fixture_sql(f, &family_path, |c| {
                assert_eq!(c.execute("INSERT INTO reservations(id,owner,operation,remaining) VALUES(?1,?2,?1,?3)",
                    params![foreign, allocation.format.identity.worktree_id.as_str(), funding.reservation_bytes()],
                ).unwrap(), 1);
            });
            "provider capture has unproven family reservation"
        }
    };
    // All witnesses are taken AFTER the explicitly requested corruption.
    // Recovery must neither release B nor delete any other row or receipt.
    let source_before = database_io::tests::support::witness(&source_path);
    let family_before = database_io::tests::support::witness(&family_path);
    let source_rows_before = source_rows(&source_path);
    let catalog_before = catalog_snapshot(f, cancel);
    let receipt_before = receipt_witness(f);
    let original_row = vec![
        rusqlite::types::Value::Text(funding.operation().into()),
        rusqlite::types::Value::Text(allocation.format.identity.worktree_id.as_str().into()),
        rusqlite::types::Value::Text(funding.operation().into()),
        rusqlite::types::Value::Integer(funding.reservation_bytes().try_into().unwrap()),
    ];
    assert_eq!(
        catalog_before["reservations"]
            .iter()
            .filter(|row| **row == original_row)
            .count(),
        1
    );
    let mut close = Close::default();
    let error = denial_recovery::recover(
        f.family.root(),
        &f.worktree,
        &binding.transaction_id,
        binding,
        &mut close,
        cancel,
    )
    .unwrap_err();
    require_validation_message(error, expected);
    assert_eq!(
        (close.opened, close.closed),
        (0, 0),
        "recovery entered a release writer"
    );
    assert_eq!(
        database_io::tests::support::witness(&source_path),
        source_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );
    assert_eq!(source_rows(&source_path), source_rows_before);
    assert_eq!(catalog_snapshot(f, cancel), catalog_before);
    assert_eq!(receipt_witness(f), receipt_before);
}

macro_rules! recovery_refusal {
    ($name:ident, $case:ident) => {
        #[test]
        #[ignore = "owned native C14 recovery refusal after explicit fixture corruption; no power-loss claim"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 1200, || {
                child::declare();
                run_start_configuration(false, false, Some(RecoveryCase::Reject(Case::$case)),
                    PreimageCase::DescriptorOnly, None);
            });
        }
    };
}

recovery_refusal!(
    fresh_recovery_refuses_orphan_without_compact_receipt,
    OrphanWithoutReceipt
);
recovery_refusal!(
    fresh_recovery_refuses_receipt_conflicting_with_actual_intent,
    ConflictingActualIntent
);
recovery_refusal!(
    fresh_recovery_full_reader_refuses_document_without_intent,
    UnownedDocument
);
recovery_refusal!(
    fresh_recovery_refuses_second_unproven_owner_reservation,
    ForeignOrphan
);
