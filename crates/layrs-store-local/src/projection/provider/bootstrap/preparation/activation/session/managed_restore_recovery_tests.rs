//! Explicit interrupted-restore recovery on a genuine admitted destination.
//! Fault callbacks return errors; no process crash or SQLite-journal recovery.
use super::*;
use crate::projection::{
    format,
    provider::{
        admitted_ordinary,
        database_io::{WriterCloseObserver, WriterOpenPoint},
        managed_ready_reopen::ManagedReadyAdmission,
        process_profile::tests::child,
    },
    ProjectionRecoveryReason, ProjectionStore, RestoreFaults, RestorePoint, RestoreSource,
    SessionMode,
};
use layrs_core::scm::RepoPath;
use rusqlite::ffi;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, _: *mut ffi::sqlite3) -> CauseResult<()> {
        if point == WriterOpenPoint::BeforeRaw {
            self.opens += 1;
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, success: bool) -> Option<Cause> {
        assert!(success);
        self.closes += 1;
        None
    }
}
struct Stop(RestorePoint);
impl RestoreFaults for Stop {
    fn check(&self, point: RestorePoint) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other(format!("owned restore stop {point:?}")))
        } else {
            Ok(())
        }
    }
}

pub(super) fn admitted_destination() -> PathBuf {
    admitted_destination_with_source().0
}

/// Fresh fixture path DATA only; all source/session holders are released by
/// the staging helper before either checkout path escapes to the caller.
pub(super) fn admitted_destination_with_source() -> (PathBuf, PathBuf) {
    let mut destination = None;
    super::managed_staging_tests::with_content_planned_bytes_then(
        b"provider base content",
        |_, session, attempt, planned| {
            super::managed_ready_reopen_tests::publish_destination(session, attempt, planned)
        },
        |fixture, session, root| {
            let cancel = Cancellation::default();
            let held = ManagedReadyAdmission::open(&root, &cancel).unwrap();
            let mut close = Close::default();
            session
                .admit_ready_destination(held, &mut close, &cancel)
                .unwrap_or_else(|failure| {
                    panic!("admission {:?}: {}", failure.step, failure.cause)
                });
            assert_eq!((close.opens, close.closes), (1, 1));
            destination = Some((root, fixture.checkout.clone()));
        },
    );
    destination.unwrap()
}

fn witnesses(
    root: &Path,
    family: &Path,
) -> (
    std::collections::BTreeMap<PathBuf, super::managed_ready_reopen_tests::Node>,
    std::collections::BTreeMap<PathBuf, super::managed_ready_reopen_tests::Node>,
) {
    (
        super::managed_ready_reopen_tests::tree(root),
        super::managed_ready_reopen_tests::tree(family),
    )
}

/// Exercise the DATA refusals inside transactions that are explicitly rolled
/// back. Only a separate ordinary protection is committed to the owned family;
/// it proves same-owner/same-bytes is insufficient without the restore operation.
fn pending_data_refusals(store: &ProjectionStore, operation: &str) {
    use crate::projection::{database, restore, selection};
    use layrs_core::projection::ProjectionSelection;
    format::access(store, SessionMode::Exclusive).unwrap().run(|access| {
        let lease = access.family.as_ref().unwrap();
        let (id, digest): (String, String) = access.connection.query_row(
            "SELECT root_id,digest FROM projection_claims WHERE kind='restore' AND owner=?1",
            [operation], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let manifests = lease.verify_protection_metadata(&store.marker, &id, &digest)?;
        let (other, other_digest) = lease.protect(&store.marker, "recovery-test-other", &manifests)?;
        let tx = access.connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE projection_claims SET root_id=?1,digest=?2 WHERE root_id=?3",
            rusqlite::params![other, other_digest, id])?;
        let detached = restore::recovery_preflight(&tx, store)?;
        assert!(detached.validate_family(lease, store).unwrap_err().to_string().contains("another operation"));
        tx.rollback()?;

        let tx = access.connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let state = database::current(&tx)?;
        let payload = selection::encode(&ProjectionSelection::Selected { paths: Vec::new(), subtrees: Vec::new() })?;
        let digest = database::digest(state.generation, Some(&state.digest), &payload);
        tx.execute("INSERT INTO projection_selections VALUES(?1,?2,?3) ON CONFLICT(generation) DO UPDATE SET digest=excluded.digest,payload=excluded.payload",
            rusqlite::params![state.generation, digest, payload])?;
        assert!(restore::recovery_preflight(&tx, store).unwrap_err().to_string().contains("excluded path"));
        tx.rollback()?;

        let tx = access.connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE projection_budgets SET operation='restore:wrong' WHERE singleton=1", [])?;
        assert!(restore::recovery_preflight(&tx, store).unwrap_err().to_string().contains("does not own"));
        tx.rollback()?;

        let tx = access.connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE projection_restore_pending SET payload=x'7b' WHERE singleton=1", [])?;
        assert!(restore::recovery_preflight(&tx, store).is_err());
        tx.rollback()?;
        Ok(())
    }).unwrap();
}

fn malformed_journal_refuses_without_effects(store: &ProjectionStore, family: &Path) {
    let original: Vec<u8> = format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            let original = access.connection.query_row(
                "SELECT payload FROM projection_restore_pending WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            access.connection.execute(
                "UPDATE projection_restore_pending SET payload=x'7b' WHERE singleton=1",
                [],
            )?;
            Ok(original)
        })
        .unwrap();
    let before = witnesses(&store.root, family);
    assert!(ProjectionStore::resume_restore(&store.root).is_err());
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "malformed journal refusal changed evidence"
    );
    // Explicit fixture repair from the saved exact bytes, outside the recovery
    // operation being tested. The valid resume below uses a fresh owner scope.
    format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            access.connection.execute(
                "UPDATE projection_restore_pending SET payload=?1 WHERE singleton=1",
                [original],
            )?;
            Ok(())
        })
        .unwrap();
}

#[test]
#[ignore = "UNQUALIFIED interrupted admitted restore; ROOT source release, owned child720 / outer780 required"]
fn managed_admitted_restore_resumes_four_durable_interruptions_with_readonly_inspection() {
    child::run(concat!(module_path!(), "::managed_admitted_restore_resumes_four_durable_interruptions_with_readonly_inspection"), 720, || {
        child::declare();
        let root = admitted_destination();
        let store = ProjectionStore::open(&root).unwrap();
        let marker = format::read_marker(&root).unwrap();
        let family = marker.shared.as_ref().unwrap().root.clone();
        let state = store.state().unwrap();
        let path = RepoPath::parse("README.md").unwrap();
        let baseline = format::access(&store, SessionMode::Shared).unwrap().run(|access| {
            admitted_ordinary::inspect(&access.connection, &marker, None).map_err(Into::into)
        }).unwrap();
        drop(store);
        for (index, point) in [RestorePoint::JournalDurable, RestorePoint::PathQuarantined,
            RestorePoint::PathInstalled, RestorePoint::BeforeCommit].into_iter().enumerate() {
            eprintln!("RESTORE_RECOVERY_PHASE={point:?}");
            let store = ProjectionStore::open(&root).unwrap();
            let target = format!("target capture {index}").into_bytes();
            let dirty = format!("preserved dirty bytes {index}").into_bytes();
            fs::write(root.join(path.as_str()), &target).unwrap();
            let captured = store.capture_patch(state.generation).unwrap();
            fs::write(root.join(path.as_str()), &dirty).unwrap();
            let preview = store.preview_restore(state.generation, &RestoreSource::Capture {
                sequence: captured.sequence, paths: vec![path.clone()],
            }).unwrap();
            let error = store.apply_restore_with_faults(&preview, &Stop(point)).unwrap_err();
            assert!(error.to_string().contains("owned restore stop"), "{error}");
            if index == 0 {
                pending_data_refusals(&store, &preview.operation_id);
                malformed_journal_refuses_without_effects(&store, &family);
            }
            drop(store);
            let before = witnesses(&root, &family);
            let inspection = ProjectionStore::inspect_recovery(&root).unwrap();
            assert_eq!(inspection.recovery, vec![ProjectionRecoveryReason::FilesystemOperation]);
            assert_eq!(witnesses(&root, &family), before, "inspection is nonmutating");
            let mut wrong = inspection.binding.clone();
            wrong.database_identity.push_str("-changed");
            assert!(ProjectionStore::open_bound_recovery(&root, &wrong).is_err());
            assert_eq!(witnesses(&root, &family), before, "wrong binding has no effects");
            let bound = ProjectionStore::open_bound_recovery(&root, &inspection.binding).unwrap();
            if index == 0 {
                assert!(bound.resume_installation().is_err());
                assert!(bound.resume_namespace().is_err());
                assert!(bound.resume_refresh().is_err());
                assert!(bound.resume_rebase().is_err());
                assert!(bound.cancel_shared_reservation().is_err());
                assert!(bound.recover_databases().is_err());
                assert_eq!(witnesses(&root, &family), before, "other recovery routes remain fenced");
            }
            let receipt = if index % 2 == 0 { bound.resume_restore().unwrap() }
                else { drop(bound); ProjectionStore::resume_restore(&root).unwrap() };
            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(fs::read(root.join(path.as_str())).unwrap(), target);
            assert_eq!(store.apply_restore(&preview).unwrap(), receipt, "completed operation replays its receipt");
            let mut retained = Vec::new();
            store.write_captured_file(captured.sequence, &path, &mut retained).unwrap();
            assert_eq!(retained, target);
            let inverse = store.preview_restore(state.generation, &RestoreSource::Inverse {
                operation_id: receipt.operation_id,
            }).unwrap();
            store.apply_restore(&inverse).unwrap();
            assert_eq!(fs::read(root.join(path.as_str())).unwrap(), dirty);
            format::access(&store, SessionMode::Shared).unwrap().run(|access| {
                baseline.require_same_import(&admitted_ordinary::inspect(&access.connection, &marker, None)?)?;
                access.with_content(|c, scope| crate::projection::retention::fsck_with_scope(c, &store, Some(scope)))
            }).unwrap();
            drop(store);
            assert!(ProjectionStore::inspect_recovery(&root).unwrap().recovery.is_empty());
        }
        assert_eq!(ProjectionStore::open(&root).unwrap().state().unwrap(), state);
    });
}
