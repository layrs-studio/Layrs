//! Owned native preparation recipes. The parent selects each audited case.
use super::super::preparation::tests::hooks as preparation_hooks;
use super::super::preparation::{
    ProviderPreparationOutcome as Outcome, ProviderPreparationSqlPhase as Phase,
    ProviderPreparationWriteState as State, ProviderPreparationWriteStep as Step,
};
use super::*;
use std::fs;

macro_rules! owned {
    ($name:ident,$seconds:literal,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT authorizes this exact owned audited-host native recipe and disposable parent"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), $seconds, || {
                child::declare();
                hooks::reset();
                preparation_hooks::reset();
                $body
            });
        }
    };
}

fn state(f: &super::fixture::Fixture) -> String {
    let c = rusqlite::Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let phase = c
        .query_row("SELECT phase FROM projection_provider_mounts", [], |row| {
            row.get(0)
        })
        .unwrap();
    c.close().unwrap();
    phase
}

fn has(
    failure: &super::super::preparation::ProviderPreparationFailure,
    step: Step,
    state: State,
) -> bool {
    failure
        .write_report
        .effects
        .iter()
        .any(|effect| effect.step == step && effect.state == state)
}

pub(super) fn cold_nonempty_source() -> ProviderBootstrapSource {
    use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, TreeEntryTarget};
    let chunk = Chunk::new(b"native metadata without Store content".to_vec()).unwrap();
    let manifest = FileManifest::from_chunks(ChunkingDescriptor::single(), &[chunk]).unwrap();
    let old = Fixture::source();
    let mut snapshot = old.initial().clone();
    let entry = snapshot.entries.first_mut().unwrap();
    let Some(TreeEntryTarget::File { file_id }) = &mut entry.target else {
        panic!("G1 source must select one file")
    };
    *file_id = manifest.file_id().clone();
    let path = entry.path.clone();
    ProviderBootstrapSource::validate(
        snapshot,
        old.selection().clone(),
        vec![ProviderSourceDescriptor { path, manifest }],
    )
    .unwrap()
}

fn resident_content_counts(f: &Fixture) -> Vec<u64> {
    let c = rusqlite::Connection::open_with_flags(
        f.family.root().join("metadata/catalog.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let counts = [
        "chunks",
        "manifests",
        "manifest_chunks",
        "roots",
        "root_manifests",
        "reservations",
        "installations",
    ]
    .into_iter()
    .map(|table| {
        c.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    })
    .collect();
    c.close().unwrap();
    counts
}

owned!(invalid_root_id_retains_owner_without_mutation, 180, {
    let f = Fixture::new();
    let published = f.published();
    let before = f.witness();
    let withheld = match published.prepare_provider([0; 16]) {
        Outcome::Withheld(value) => value,
        Outcome::Prepared(_) => panic!("zero root ID prepared"),
    };
    assert_eq!(
        withheld.failure().kind,
        super::super::preparation::ProviderPreparationFailureKind::NeedsRepair
    );
    assert!(withheld.failure().write_report.effects.is_empty());
    assert!(!withheld.retained_native_ready_proof());
    assert_eq!(f.witness(), before);
    assert_eq!(state(&f), "control_published");
});

owned!(
    changed_prepared_receipt_refuses_before_native_or_sql_write,
    180,
    {
        let f = Fixture::new();
        let published = f.published();
        fs::write(
            f.receipt(ProviderBootstrapReceipt::Prepared),
            b"changed receipt",
        )
        .unwrap();
        let withheld = match published.prepare_provider([0x4a; 16]) {
            Outcome::Withheld(value) => value,
            Outcome::Prepared(_) => panic!("changed receipt prepared"),
        };
        assert!(withheld.failure().write_report.effects.is_empty());
        assert!(!withheld.retained_native_ready_proof());
        assert!(!withheld.pending_candidate_retained());
        assert_eq!(state(&f), "control_published");
    }
);

owned!(
    healthy_native_preparation_is_source_free_and_revalidated,
    240,
    {
        let f = Fixture::new();
        let source = cold_nonempty_source();
        assert_eq!(source.descriptors().len(), 1);
        assert!(source.descriptors()[0].bytes > 0);
        assert!(!source.descriptors()[0].manifest.chunks().is_empty());
        let published = match f
            .family
            .begin_provider_bootstrap(&f.checkout, &f.stages, &f.worktree, fixture::TX, source)
            .unwrap()
        {
            ProviderControlPublicationOutcome::Published(value) => value,
            _ => panic!("nonempty source did not publish"),
        };
        assert_eq!(resident_content_counts(&f), vec![0; 7]);
        let root_id = [0x4c; 16];
        let prepared = match published.prepare_provider(root_id) {
            Outcome::Prepared(value) => value,
            Outcome::Withheld(value) => {
                panic!("native preparation withheld: {:?}", value.failure())
            }
        };
        assert_eq!(prepared.root(), f.checkout.as_path());
        assert_eq!(prepared.binding().transaction_id, fixture::TX);
        assert_eq!(state(&f), "provider_prepared");
        prepared.revalidate().unwrap();
        assert_eq!(
            prepared.report().provenance,
            super::super::preparation::ProviderPreparationWriteReportProvenance::CurrentInvocation
        );
        match PartialStoreFamily::inspect_provider_bootstrap(
            f.family.root(),
            &f.worktree,
            fixture::TX,
        ) {
            Ok(inspection) => {
                assert_ne!(inspection.state, ProviderBootstrapState::ControlPublished)
            }
            Err(_) => (), // v1-only inspection may refuse the successor at the API boundary.
        }
        assert!(prepared.report().effects.iter().any(|effect| effect.step
            == Step::NativePopulation
            && effect.state == State::Confirmed));
        for phase in [Phase::RegistrationPending, Phase::ProviderPrepared] {
            assert!(
                prepared
                    .report()
                    .effects
                    .iter()
                    .any(|effect| effect.step == Step::SqlCommit(phase)
                        && effect.state == State::Confirmed)
            );
            assert!(
                prepared
                    .report()
                    .effects
                    .iter()
                    .any(|effect| effect.step == Step::SqlClose(phase)
                        && effect.state == State::Confirmed)
            );
        }
        assert!(fs::metadata(f.checkout.join("README.md")).unwrap().len() > 0);
        assert_eq!(resident_content_counts(&f), vec![0; 7]);
    }
);

owned!(
    pending_commit_close_busy_withholds_before_native_registration,
    240,
    {
        let f = Fixture::new();
        let published = f.published();
        preparation_hooks::busy_close(Phase::RegistrationPending);
        let withheld = match published.prepare_provider([0x4d; 16]) {
            Outcome::Withheld(value) => value,
            Outcome::Prepared(_) => panic!("BUSY close prepared"),
        };
        let failure = withheld.failure();
        assert_eq!(
            failure.kind,
            super::super::preparation::ProviderPreparationFailureKind::ObservableCloseFailure
        );
        assert!(has(
            failure,
            Step::SqlCommit(Phase::RegistrationPending),
            State::Confirmed
        ));
        assert!(has(
            failure,
            Step::SqlClose(Phase::RegistrationPending),
            State::Uncertain
        ));
        assert!(has(failure, Step::NativePopulation, State::NoNewEffect));
        assert!(!withheld.retained_native_ready_proof());
        assert!(withheld.pending_candidate_retained());
        withheld.test_revalidate_unregistered_root().unwrap();
        assert!(!failure.write_report.effects.iter().any(|effect| matches!(
            effect.step,
            Step::SqlOpen(Phase::ProviderPrepared) | Step::SqlCommit(Phase::ProviderPrepared)
        )));
    }
);

owned!(
    prepared_commit_fault_retains_native_ready_and_known_pending,
    240,
    {
        let f = Fixture::new();
        let published = f.published();
        preparation_hooks::stop(Step::SqlCommit(Phase::ProviderPrepared), true);
        let withheld = match published.prepare_provider([0x4e; 16]) {
            Outcome::Withheld(value) => value,
            Outcome::Prepared(_) => panic!("post-COMMIT checkpoint must withhold"),
        };
        let failure = withheld.failure();
        assert!(withheld.retained_native_ready_proof());
        assert!(withheld.pending_candidate_retained());
        assert!(withheld.prepared_candidate_retained());
        assert!(has(
            failure,
            Step::SqlCommit(Phase::RegistrationPending),
            State::Confirmed
        ));
        assert!(has(
            failure,
            Step::SqlCommit(Phase::ProviderPrepared),
            State::Confirmed
        ));
        assert!(has(
            failure,
            Step::SqlClose(Phase::ProviderPrepared),
            State::Confirmed
        ));
        assert!(has(failure, Step::NativePopulation, State::Confirmed));
        assert_eq!(state(&f), "provider_prepared");
    }
);

owned!(prepared_cas_refuses_changed_pending_row, 240, {
    let f = Fixture::new();
    let published = f.published();
    preparation_hooks::tamper_pending_after_prepared_open(f.metadata.join("projection.sqlite3"));
    let withheld = match published.prepare_provider([0x4f; 16]) {
        Outcome::Withheld(value) => value,
        Outcome::Prepared(_) => panic!("changed pending row prepared"),
    };
    let failure = withheld.failure();
    assert!(withheld.retained_native_ready_proof());
    assert!(has(
        failure,
        Step::SqlCommit(Phase::RegistrationPending),
        State::Confirmed
    ));
    assert!(has(
        failure,
        Step::SqlOpen(Phase::ProviderPrepared),
        State::Confirmed
    ));
    assert!(
        !failure
            .write_report
            .effects
            .iter()
            .any(|effect| effect.step == Step::SqlCommit(Phase::ProviderPrepared))
    );
    assert_eq!(state(&f), "control_published");
});

owned!(
    prepared_revalidate_refuses_changed_family_registry_with_retained_report,
    240,
    {
        let f = Fixture::new();
        let prepared = match f.published().prepare_provider([0x50; 16]) {
            Outcome::Prepared(value) => value,
            Outcome::Withheld(value) => panic!("native setup withheld: {:?}", value.failure()),
        };
        let count = prepared.report().effects.len();
        let catalog_path = f.family.root().join("metadata/catalog.sqlite3");
        let c = rusqlite::Connection::open_with_flags(
            &catalog_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        assert_eq!(
            c.execute("DELETE FROM worktrees WHERE id=?1", [f.worktree.as_str()])
                .unwrap(),
            1
        );
        c.close().unwrap();
        let error = prepared.revalidate().unwrap_err();
        assert_eq!(error.operation, "revalidate prepared provider");
        assert_eq!(error.database, Some(ProviderDatabaseScope::FamilyCatalog));
        assert_eq!(
            error.write_report.provenance,
            super::super::preparation::ProviderPreparationWriteReportProvenance::RetainedIssuance
        );
        assert_eq!(error.write_report.effects.len(), count);
    }
);
