//! Owned native recipes for ordered activation and explicit resume.
use super::super::preparation::activation::tests::hooks as activation_hooks;
use super::super::preparation::{
    ProviderActivationOutcome as Outcome, ProviderActivationSqlPhase as Phase,
    ProviderActivationWriteState as State, ProviderActivationWriteStep as Step,
    ProviderPreparationOutcome,
};
use super::*;

macro_rules! owned {
    ($name:ident,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host activation recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare(); hooks::reset(); activation_hooks::reset(); $body
            });
        }
    };
}

fn prepare(f: &Fixture, nonempty: bool) -> super::super::preparation::ProviderPrepared {
    let published = if nonempty {
        match f
            .family
            .begin_provider_bootstrap(
                &f.checkout,
                &f.stages,
                &f.worktree,
                fixture::TX,
                super::preparation::cold_nonempty_source(),
            )
            .unwrap()
        {
            ProviderControlPublicationOutcome::Published(value) => value,
            _ => panic!("activation nonempty source did not publish"),
        }
    } else {
        f.published()
    };
    match published.prepare_provider([0x79; 16]) {
        ProviderPreparationOutcome::Prepared(value) => value,
        ProviderPreparationOutcome::Withheld(value) => {
            panic!("activation prepare withheld: {:?}", value.failure())
        }
    }
}

fn sql_state(f: &Fixture) -> (u64, String, u64, String) {
    let private = rusqlite::Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let generation = private
        .query_row(
            "SELECT generation FROM projection_head WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let phase = private
        .query_row("SELECT phase FROM projection_provider_mounts", [], |r| {
            r.get(0)
        })
        .unwrap();
    let activations = private
        .query_row(
            "SELECT count(*) FROM projection_provider_activations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    private.close().unwrap();
    let family = rusqlite::Connection::open_with_flags(
        f.family.root().join("metadata/catalog.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let status = family
        .query_row(
            "SELECT state FROM worktrees WHERE id=?1",
            [f.worktree.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    family.close().unwrap();
    (generation, phase, activations, status)
}

owned!(healthy_cold_activation_is_generation_one_and_ready, {
    for nonempty in [false, true] {
        let f = Fixture::new();
        let active = match prepare(&f, nonempty).activate() {
            Outcome::Active(value) => value,
            Outcome::Withheld(value) => panic!("activation withheld: {:?}", value.failure()),
        };
        assert_eq!(active.generation(), 1);
        assert_eq!(active.root(), f.checkout.as_path());
        assert_eq!(sql_state(&f), (1, "active".into(), 1, "ready".into()));
        active.revalidate().unwrap();
    }
});

owned!(durably_reopened_prepared_owner_activates, {
    let f = Fixture::new();
    let prepared = prepare(&f, true);
    let expected = ProviderBootstrapBinding::decode(&prepared.binding().encode().unwrap()).unwrap();
    drop(prepared);
    let reopened = PartialStoreFamily::open_provider_prepared(
        f.family.root(),
        &f.worktree,
        fixture::TX,
        &expected,
    )
    .unwrap();
    let active = match reopened.activate() {
        Outcome::Active(value) => value,
        Outcome::Withheld(value) => panic!("reopened activation withheld: {:?}", value.failure()),
    };
    assert_eq!(sql_state(&f), (1, "active".into(), 1, "ready".into()));
    active.revalidate().unwrap();
});

owned!(
    private_commit_checkpoint_resumes_without_second_generation,
    {
        let f = Fixture::new();
        activation_hooks::stop(Step::SqlCommit(Phase::PrivateActivation), true);
        let withheld = match prepare(&f, false).activate() {
            Outcome::Withheld(value) => value,
            Outcome::Active(_) => panic!("commit checkpoint did not withhold"),
        };
        assert_eq!(
            sql_state(&f),
            (1, "active".into(), 1, "initializing".into())
        );
        assert!(
            withheld
                .report()
                .effects
                .iter()
                .any(|e| e.step == Step::SqlCommit(Phase::PrivateActivation)
                    && e.state == State::Confirmed)
        );
        activation_hooks::reset();
        let active = match withheld.resume_activation() {
            Outcome::Active(value) => value,
            Outcome::Withheld(value) => panic!("resume withheld: {:?}", value.failure()),
        };
        assert_eq!(sql_state(&f), (1, "active".into(), 1, "ready".into()));
        assert!(active.report().resumed);
        assert!(
            active
                .report()
                .effects
                .iter()
                .all(|e| e.step != Step::SqlCommit(Phase::PrivateActivation))
        );
    }
);

owned!(
    private_commit_before_sql_rolls_back_without_family_attempt,
    {
        let f = Fixture::new();
        activation_hooks::stop(Step::SqlCommit(Phase::PrivateActivation), false);
        let withheld = match prepare(&f, false).activate() {
            Outcome::Withheld(value) => value,
            Outcome::Active(_) => panic!("pre-commit fault activated"),
        };
        assert_eq!(
            sql_state(&f),
            (0, "provider_prepared".into(), 0, "initializing".into())
        );
        assert!(
            withheld
                .report()
                .effects
                .iter()
                .any(|e| e.step == Step::SqlCommit(Phase::PrivateActivation)
                    && e.state == State::NoNewEffect)
        );
        assert!(!withheld.report().effects.iter().any(|e| matches!(
            e.step,
            Step::SqlOpen(Phase::FamilyReady)
                | Step::SqlTransaction(Phase::FamilyReady)
                | Step::SqlCommit(Phase::FamilyReady)
                | Step::SqlClose(Phase::FamilyReady)
        )));
        activation_hooks::reset();
        let active = match withheld.resume_activation() {
            Outcome::Active(value) => value,
            Outcome::Withheld(value) => panic!("pre-commit retry withheld: {:?}", value.failure()),
        };
        assert_eq!(active.generation(), 1);
        assert_eq!(sql_state(&f), (1, "active".into(), 1, "ready".into()));
    }
);

owned!(
    private_busy_close_withholds_without_later_probe_or_family_attempt,
    {
        let f = Fixture::new();
        let prepared = prepare(&f, false);
        let family_path = f.family.root().join("metadata/catalog.sqlite3");
        let private_path = f.metadata.join("projection.sqlite3");
        let family_before =
            crate::projection::provider::database_io::tests::support::witness(&family_path);
        let private_before =
            crate::projection::provider::database_io::tests::support::witness(&private_path);
        activation_hooks::busy_close(Phase::PrivateActivation);
        let withheld = match prepared.activate() {
            Outcome::Withheld(value) => value,
            Outcome::Active(_) => panic!("private BUSY close activated"),
        };
        assert_eq!(
            withheld.failure().database,
            Some(ProviderDatabaseScope::PrivateMetadata)
        );
        assert_eq!(
            withheld.failure().kind,
            super::super::preparation::ProviderActivationFailureKind::ObservableCloseFailure
        );
        assert!(
            withheld
                .report()
                .effects
                .iter()
                .any(|e| e.step == Step::SqlCommit(Phase::PrivateActivation)
                    && e.state == State::Confirmed)
        );
        assert!(
            withheld
                .report()
                .effects
                .iter()
                .any(|e| e.step == Step::SqlClose(Phase::PrivateActivation)
                    && e.state == State::Uncertain)
        );
        assert!(!withheld.report().effects.iter().any(|e| matches!(
            e.step,
            Step::SqlOpen(Phase::FamilyReady)
                | Step::SqlTransaction(Phase::FamilyReady)
                | Step::SqlCommit(Phase::FamilyReady)
                | Step::SqlClose(Phase::FamilyReady)
        )));
        // A failed sqlite3_close leaves its raw handle quarantined. Observe only
        // the owned files here; opening SQLite again could disturb that evidence.
        assert_eq!(
            crate::projection::provider::database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_ne!(
            crate::projection::provider::database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(withheld.root(), f.checkout.as_path());
    }
);

owned!(
    family_commit_checkpoint_retains_ready_and_resume_is_idempotent,
    {
        let f = Fixture::new();
        activation_hooks::stop(Step::SqlCommit(Phase::FamilyReady), true);
        let withheld = match prepare(&f, true).activate() {
            Outcome::Withheld(value) => value,
            Outcome::Active(_) => panic!("family checkpoint did not withhold"),
        };
        assert_eq!(sql_state(&f), (1, "active".into(), 1, "ready".into()));
        activation_hooks::reset();
        let active = match withheld.resume_activation() {
            Outcome::Active(value) => value,
            Outcome::Withheld(value) => panic!("resume withheld: {:?}", value.failure()),
        };
        assert_eq!(sql_state(&f), (1, "active".into(), 1, "ready".into()));
        assert!(active.report().effects.is_empty());
    }
);

owned!(
    family_busy_close_retains_both_commits_without_later_probe,
    {
        let f = Fixture::new();
        let prepared = prepare(&f, false);
        let family_path = f.family.root().join("metadata/catalog.sqlite3");
        let private_path = f.metadata.join("projection.sqlite3");
        let family_before =
            crate::projection::provider::database_io::tests::support::witness(&family_path);
        let private_before =
            crate::projection::provider::database_io::tests::support::witness(&private_path);
        activation_hooks::busy_close(Phase::FamilyReady);
        let withheld = match prepared.activate() {
            Outcome::Withheld(value) => value,
            Outcome::Active(_) => panic!("close fault did not withhold"),
        };
        assert_eq!(
            withheld.failure().database,
            Some(ProviderDatabaseScope::FamilyCatalog)
        );
        assert_eq!(
            withheld.failure().kind,
            super::super::preparation::ProviderActivationFailureKind::ObservableCloseFailure
        );
        assert!(
            withheld
                .report()
                .effects
                .iter()
                .any(|e| e.step == Step::SqlCommit(Phase::PrivateActivation)
                    && e.state == State::Confirmed)
        );
        assert!(
            withheld
                .report()
                .effects
                .iter()
                .any(|e| e.step == Step::SqlCommit(Phase::FamilyReady)
                    && e.state == State::Confirmed)
        );
        assert!(
            withheld.report().effects.iter().any(
                |e| e.step == Step::SqlClose(Phase::FamilyReady) && e.state == State::Uncertain
            )
        );
        // A failed sqlite3_close quarantines its raw handle and process profile.
        // Read only bounded owned files; do not reopen either database or resume.
        assert_ne!(
            crate::projection::provider::database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_ne!(
            crate::projection::provider::database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(withheld.root(), f.checkout.as_path());
    }
);

owned!(corrupt_predecessor_refuses_before_any_activation_write, {
    let f = Fixture::new();
    let prepared = prepare(&f, false);
    let c = rusqlite::Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    c.execute(
        "UPDATE projection_provider_mounts SET digest=?1",
        [format!("blake3:{}", "0".repeat(64))],
    )
    .unwrap();
    c.close().unwrap();
    let before = f.witness();
    let withheld = match prepared.activate() {
        Outcome::Withheld(value) => value,
        Outcome::Active(_) => panic!("tampered predecessor activated"),
    };
    assert!(withheld.report().effects.is_empty());
    assert_eq!(f.witness(), before);
    assert_eq!(
        sql_state(&f),
        (0, "provider_prepared".into(), 0, "initializing".into())
    );
});

owned!(
    active_revalidation_reports_retained_issuance_without_new_writes,
    {
        let f = Fixture::new();
        let active = match prepare(&f, false).activate() {
            Outcome::Active(value) => value,
            Outcome::Withheld(value) => panic!("activation withheld: {:?}", value.failure()),
        };
        let prior_count = active.report().effects.len();
        let c = rusqlite::Connection::open_with_flags(
            f.metadata.join("projection.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        c.execute(
            "UPDATE projection_provider_mounts SET digest=?1",
            [format!("blake3:{}", "0".repeat(64))],
        )
        .unwrap();
        c.close().unwrap();
        let before = f.witness();
        let failure = active
            .revalidate()
            .err()
            .expect("tampered active mount accepted");
        assert_eq!(
            failure.write_report.provenance,
            super::super::preparation::ProviderActivationWriteReportProvenance::RetainedIssuance
        );
        assert_eq!(failure.write_report.effects.len(), prior_count);
        assert_eq!(f.witness(), before);
    }
);
