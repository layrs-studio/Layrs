//! Physical C14 admission from the genuine Published B, then fresh terminal
//! resolution after all source holders close and only B's database is moved.
//! No fixture phase, proof, admission row or recovery state is manufactured.
use super::*;
use crate::projection::{
    format,
    provider::{
        admitted_private_io::AdmittedPreflight,
        denied_coverage_storage,
        managed_ready_reopen::{AdmissionStep, DenialTerminalResolution, ManagedReadyAdmission},
        records::{
            import::ProviderTransitionReceiptRecord,
            transition::{TransitionRole, TransitionState},
        },
    },
};
use std::path::{Path, PathBuf};

pub(super) struct Published {
    pub(super) root: PathBuf,
    pub(super) receipt: ProviderTransitionReceiptRecord,
}

pub(super) struct Completed {
    pub(super) root: PathBuf,
    pub(super) digest: String,
    pub(super) receipt: ProviderTransitionReceiptRecord,
    pub(super) required: Vec<FileManifest>,
    source_database: PathBuf,
    source_quarantine: PathBuf,
    source_witness: (String, u64, String),
    pub(super) destination_database: PathBuf,
    destination_witness: (String, u64, String),
    pub(super) family_database: PathBuf,
    family_witness: (String, u64, String),
}

/// Called only after the publication recipe drops C's allocation, checkout/CAS
/// pins and legacy failure holder. B's actual native session remains retained.
pub(super) fn commit(
    f: &Fixture,
    session: &ProviderActiveSession,
    root: &Path,
    receipt: &ProviderTransitionReceiptRecord,
    required: &[FileManifest],
    cancel: &Cancellation,
) -> Completed {
    let mut timing = child::phase("c14-admission-witnesses");
    let held = &session.source.held;
    let marker = format::read_marker(root).unwrap();
    let destination_database = marker
        .shared
        .as_ref()
        .unwrap()
        .metadata_path
        .join("projection.sqlite3");
    let source_database = held.artifacts.database.path().to_path_buf();
    let family_database = held.family.catalog.path().to_path_buf();
    let source_quarantine = f.outer.join("published-source-quarantine.sqlite3");
    assert!(source_database.starts_with(&f.outer));
    assert!(source_quarantine.starts_with(&f.outer));
    assert_ne!(source_database, destination_database);
    assert_ne!(source_database, family_database);
    assert!(!source_quarantine.exists());
    let witness = database_io::tests::support::witness;
    let source_witness = witness(&source_database);
    let family_witness = witness(&family_database);
    let destination_before = witness(&destination_database);
    timing.next("c14-admission-open-ready");
    let admission = ManagedReadyAdmission::open(root, cancel).unwrap();
    // The genuine session adapter must refuse before any writer and retain the
    // destination owner so the exact same holder can retry normally.
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let mut refused_close = Close::default();
    timing.next("c14-admission-precancel");
    let failure = session
        .admit_denial_ready_destination(admission, &mut refused_close, &cancelled)
        .unwrap_err();
    assert_eq!(failure.step, AdmissionStep::Validating);
    assert!(matches!(
        &failure.cause,
        Cause::Vfs(layrs_vfs::Error::Cancelled)
    ));
    assert_eq!((refused_close.opened, refused_close.closed), (0, 0));
    assert_eq!(witness(&source_database), source_witness);
    assert_eq!(witness(&family_database), family_witness);
    assert_eq!(witness(&destination_database), destination_before);
    let mut close = Close::default();
    timing.next("c14-admission-commit");
    let digest = session
        .admit_denial_ready_destination(failure.held, &mut close, cancel)
        .unwrap_or_else(|failure| {
            panic!(
                "physical C14 admission {:?}: {}",
                failure.step, failure.cause
            )
        });
    timing.next("c14-admission-committed");
    assert_eq!((close.opened, close.closed), (1, 1));
    assert_eq!(witness(&source_database), source_witness);
    assert_eq!(witness(&family_database), family_witness);
    let destination_witness = witness(&destination_database);
    assert_ne!(destination_witness, destination_before);
    Completed {
        root: root.to_path_buf(),
        digest,
        receipt: ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap(),
        required: required.to_vec(),
        source_database,
        source_quarantine,
        source_witness,
        destination_database,
        destination_witness,
        family_database,
        family_witness,
    }
}

/// The caller has disconnected and dropped the source session, attempt, image,
/// checkpoint, range reader, allocation and fixture before entering this helper.
pub(super) fn require_reopened_without_source(
    completed: Completed,
    coverage: &DerivedDeniedCoverage,
    cancel: &Cancellation,
) {
    require_reopened_without_source_ref(&completed, coverage, cancel);
}

pub(super) fn require_reopened_without_source_then(
    completed: Completed,
    coverage: &DerivedDeniedCoverage,
    cancel: &Cancellation,
    operation: impl FnOnce(&Completed),
) {
    require_reopened_without_source_ref(&completed, coverage, cancel);
    let source_database = completed.source_database.clone();
    let source_quarantine = completed.source_quarantine.clone();
    with_observer(
        move |path, _| {
            assert_ne!(path, source_database);
            assert_ne!(path, source_quarantine);
            Ok(())
        },
        || operation(&completed),
    );
    assert!(!completed.source_database.exists());
    assert_eq!(
        database_io::tests::support::witness(&completed.source_quarantine),
        completed.source_witness
    );
}

fn require_reopened_without_source_ref(
    completed: &Completed,
    coverage: &DerivedDeniedCoverage,
    cancel: &Cancellation,
) {
    let mut timing = child::phase("c14-terminal-source-quarantine");
    let witness = database_io::tests::support::witness;
    let source = fs::symlink_metadata(&completed.source_database).unwrap();
    assert!(source.is_file() && !source.file_type().is_symlink());
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", completed.source_database.display()));
        assert!(
            !sidecar.exists(),
            "source must close cleanly before quarantine"
        );
    }
    // Move only this owned fixture's B database after normal closure. Retain it
    // in quarantine: neither restore, failed-fixture reopen nor cleanup follows.
    fs::rename(&completed.source_database, &completed.source_quarantine).unwrap();
    assert!(!completed.source_database.exists());
    assert_eq!(
        witness(&completed.source_quarantine),
        completed.source_witness
    );
    let source_database = completed.source_database.clone();
    let source_quarantine = completed.source_quarantine.clone();
    with_observer(
        move |path, _| {
            assert_ne!(path, source_database);
            assert_ne!(path, source_quarantine);
            Ok(())
        },
        || {
            timing.next("c14-terminal-open");
            let terminal = DenialTerminalResolution::open(&completed.root, cancel).unwrap();
            timing.next("c14-terminal-require");
            terminal.require(cancel).unwrap();
            timing.next("c14-terminal-data-and-mutation-fences");
            assert_eq!(terminal.root(), completed.root);
            assert_eq!(terminal.admission_digest(), completed.digest);
            assert_eq!(terminal.receipt(), &completed.receipt);
            assert_eq!(terminal.ready().role, TransitionRole::Destination);
            assert_eq!(terminal.ready().state, TransitionState::Ready);
            let mut actual_required = terminal.required().to_vec();
            let mut expected_required = completed.required.clone();
            actual_required.sort_by_key(|manifest| manifest.id().to_string());
            expected_required.sort_by_key(|manifest| manifest.id().to_string());
            assert_eq!(actual_required, expected_required);
            assert!(terminal.journal().is_complete());
            assert_eq!(
                terminal.expected().files().len(),
                if completed.receipt.companion_id.is_some() {
                    1
                } else {
                    2
                }
            );
            assert_eq!(
                terminal.ready().plan.denial_coverage.as_ref(),
                Some(&coverage.record().reference().unwrap())
            );
            let admitted = terminal.admitted();
            assert_eq!(admitted.private_schema(), 9);
            assert_eq!(admitted.source_published().role, TransitionRole::Source);
            assert_eq!(
                admitted.source_published().state,
                TransitionState::Published
            );
            admitted
                .source_published()
                .require_receipt(terminal.receipt())
                .unwrap();
            let marker = admitted.marker();
            let identity = &marker.shared.as_ref().unwrap().database_identity;
            // This direct typed preflight borrows the genuine terminal owner.
            // The exact guard returns before consuming clean.open; an arbitrary
            // legacy opener failure would not establish this refusal.
            let preflight =
                AdmittedPreflight::open(&completed.destination_database, identity, marker).unwrap();
            let error = preflight
                .into_handle(true)
                .err()
                .expect("C14 terminal must refuse a writable admitted handle");
            require_validation_message(error, "admitted denial coverage remains read-only");
            let database =
                database_io::DatabaseFile::open(completed.destination_database.clone(), identity)
                    .unwrap();
            database_io::read_cancellable_strict(&database, cancel, |c| {
                assert_eq!(
                    c.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?,
                    9
                );
                let body = denied_coverage_storage::read_required(
                    c,
                    terminal.receipt().denial_coverage.as_ref().unwrap(),
                    Some(cancel),
                )?;
                assert_eq!(&body, coverage.record());
                body.require_intent(terminal.ready(), Some(cancel))?;
                let changes = c.total_changes();
                let error = import_validation::plan_admitted_import(
                    c,
                    marker,
                    completed.receipt.source_identity.clone(),
                    &completed
                        .receipt
                        .source_prefix
                        .source
                        .version()
                        .effective_image_digest,
                    Some(cancel),
                )
                .err()
                .expect("C14 terminal must refuse re-export planning");
                require_validation_message(error, "admitted denial re-export is not integrated");
                assert_eq!(c.total_changes(), changes);
                Ok(())
            })
            .and_then(super::super::super::managed_destination::valid)
            .unwrap();
            drop(database);
            terminal.require(cancel).unwrap();
            assert_eq!(
                witness(&completed.destination_database),
                completed.destination_witness
            );
            assert_eq!(
                witness(&completed.family_database),
                completed.family_witness
            );
            drop(terminal);
        },
    );
    assert!(!completed.source_database.exists());
    assert_eq!(
        witness(&completed.source_quarantine),
        completed.source_witness
    );
    assert_eq!(
        witness(&completed.destination_database),
        completed.destination_witness
    );
    assert_eq!(
        witness(&completed.family_database),
        completed.family_witness
    );
}

#[test]
#[ignore = "UNQUALIFIED native C14 physical plain admission and source-absent terminal; ROOT serial child3600/outer3660 required"]
fn c14_plain_denial_admits_physically_then_resolves_without_source_database() {
    child::run(
        concat!(
            module_path!(),
            "::c14_plain_denial_admits_physically_then_resolves_without_source_database"
        ),
        3600,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::ContentPlainAdmission),
            );
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED native C14 physical companion admission and source-absent terminal; ROOT serial child3600/outer3660 required"]
fn c14_companion_denial_admits_physically_then_resolves_without_source_database() {
    child::run(
        concat!(
            module_path!(),
            "::c14_companion_denial_admits_physically_then_resolves_without_source_database"
        ),
        3600,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::ContentAdmission),
            );
        },
    );
}
