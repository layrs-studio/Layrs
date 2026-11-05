//! Individually ignored owned-host recipes. ROOT selects them serially after review.
use super::*;
use crate::projection::provider::records::Receipt as _;
use ProviderBootstrapCreate as Create;
use ProviderBootstrapReceipt as Receipt;
use ProviderBootstrapSqlWrite as Sql;
use ProviderBootstrapWriteState as State;
use ProviderBootstrapWriteStep as Step;
use fixture::{assert_synthetic, native_source};
use std::{fs, io::Write, sync::Arc};
macro_rules! owned {
    ($name:ident,$seconds:literal,$body:block)=>{
        #[test]
        #[ignore="UNQUALIFIED until ROOT authorizes this exact owned audited-host writer recipe and absolute disposable parent"]
        fn $name(){child::run(concat!(module_path!(),"::",stringify!($name)),$seconds,||{child::declare();hooks::reset();$body});}
    };
}
owned!(begin_success_full_sql_and_legacy_inspection_none, 180, {
    let f = Fixture::new();
    let p = f.published();
    assert_eq!(
        p.inspection().state,
        ProviderBootstrapState::ControlPublished
    );
    assert!(p.inspection().effects.is_empty());
    assert!(p.inspection().rollback_triggers.is_empty());
    let report = p.inspection().write_report.as_ref().unwrap();
    assert_eq!(
        report.provenance,
        ProviderBootstrapWriteReportProvenance::CurrentInvocation {
            operation: ProviderBootstrapWriterOperation::Begin
        }
    );
    for purpose in [
        Sql::RegisterInitializing,
        Sql::InitializePrivate,
        Sql::PrepareControl,
        Sql::PublishControl,
    ] {
        assert!(
            report
                .effects
                .iter()
                .any(|e| e.step == Step::SqlCommit(purpose) && e.state == State::Confirmed)
        );
    }
    let i = f.inspect();
    assert!(i.write_report.is_none());
    assert_eq!(i.state, ProviderBootstrapState::ControlPublished);
    let a = stages(&f);
    let prepared = prepared(&f);
    raw_sql(&f.metadata.join("projection.sqlite3"), |c| {
        assert!(
            matches!(validation::private(c,&a,Some(&prepared)).unwrap(),validation::PrivateValidation::Initialized(m)if m.phase==MountPhase::ControlPublished)
        );
    });
    let family = FamilyAccess::open(f.family.root()).unwrap();
    raw_sql(family.catalog.path(), |c| {
        assert_eq!(
            family.validate_catalog(c, &a).unwrap(),
            Registration::Initializing
        )
    });
});
owned!(source_refusals_precede_allocation, 180, {
    let f = Fixture::new();
    let before = f.witness();
    let source = Fixture::source();
    let mut initial = source.initial().clone();
    initial.entries[0].state = layrs_core::projection::EntryState::Materialized;
    let e = ProviderBootstrapSource::validate(initial, source.selection().clone(), vec![])
        .err()
        .expect("Materialized refusal");
    assert!(
        matches!(e,ProjectionStoreError::Invalid(ref s)if s=="fresh provider source cannot claim Materialized entries")
    );
    let e = ProviderBootstrapSource::validate(
        source.initial().clone(),
        source.selection().clone(),
        vec![],
    )
    .err()
    .expect("descriptor refusal");
    assert!(
        matches!(e,ProjectionStoreError::Invalid(ref s)if s=="missing or unsupported selected source descriptor")
    );
    assert!(hooks::events().is_empty());
    assert!(!f.metadata.exists());
    assert_eq!(before, f.witness());
});
#[test]
#[ignore = "UNQUALIFIED until ROOT selects this exact undeclared owned-host process; no implicit declaration"]
fn undeclared_writer_profile_has_zero_allocation() {
    child::run(
        concat!(
            module_path!(),
            "::undeclared_writer_profile_has_zero_allocation"
        ),
        180,
        || {
            hooks::reset();
            let f = Fixture::new();
            let before = f.witness();
            let e = failure(f.begin());
            assert!(matches!(e.cause, Cause::Profile(_)));
            assert!(e.write_report.is_none());
            assert!(hooks::events().is_empty());
            assert_eq!(before, f.witness());
            assert!(!f.metadata.exists());
        },
    );
}
owned!(invalid_stage_ancestry_has_zero_allocation, 180, {
    let f = Fixture::new();
    let before = f.witness();
    for parent in [&f.checkout, f.family.root()] {
        let e = failure(f.family.begin_provider_bootstrap(
            &f.checkout,
            parent,
            &f.worktree,
            fixture::TX,
            Fixture::source(),
        ));
        assert!(e.write_report.is_none());
        invalid(
            &e.cause,
            "provider checkout/family/stage native ancestry overlaps",
        );
    }
    assert!(hooks::events().is_empty());
    assert_eq!(before, f.witness());
    assert!(!f.metadata.exists());
});
owned!(
    member_collision_preserves_existing_and_consumes_worktree_id,
    180,
    {
        let f = Fixture::new();
        let member = f.metadata.parent().unwrap();
        fs::create_dir(member).unwrap();
        fs::write(member.join("retained"), b"unchanged").unwrap();
        let id = crate::secure_fs::PinnedDirectory::open(member)
            .unwrap()
            .identity_token();
        let before = f.witness();
        for tx in [fixture::TX, "00000000000000000000000000000002"] {
            let e = failure(f.family.begin_provider_bootstrap(
                &f.checkout,
                &f.stages,
                &f.worktree,
                tx,
                Fixture::source(),
            ));
            assert!(matches!(e.cause, Cause::Io(_)));
            assert_eq!(e.write_report.as_ref().unwrap().effects.len(), 1);
            no_later_writes(&e, |s| !matches!(s, Step::ExclusiveCreate(Create::Member)));
        }
        assert_eq!(
            id,
            crate::secure_fs::PinnedDirectory::open(member)
                .unwrap()
                .identity_token()
        );
        assert_eq!(fs::read(member.join("retained")).unwrap(), b"unchanged");
        assert_eq!(before, f.witness());
    }
);
owned!(pre_a_metadata_fault_retains_only_proven_creations, 180, {
    pre_a(Create::Metadata);
});
owned!(
    pre_a_lock_creation_fault_has_no_receipt_sql_or_format,
    180,
    {
        pre_a(Create::OwnerLock);
    }
);
owned!(pre_a_after_owner_fault_has_no_receipt_sql_or_format, 180, {
    let f = Fixture::new();
    let before = f.witness();
    hooks::fail_owner();
    let e = failure(f.begin());
    assert_synthetic(&e.cause, "synthetic writer stop after owner acquisition");
    no_later_writes(&e, |s| {
        matches!(
            s,
            Step::ReceiptWrite(_) | Step::SqlOpen(_) | Step::FormatWrite
        )
    });
    assert!(f.metadata.join("layrs.lock").exists());
    assert_eq!(before, f.witness());
});
owned!(
    pre_a_owner_busy_keeps_prior_creations_without_adoption,
    180,
    {
        // The owned companion process holds the actual existing owner lock. Its ACK
        // occurs after creator closure and before writer acquisition; no PID lookup.
        process::owner_contention();
    }
);
macro_rules! receipt_cases {
    ($short:ident,$pre:ident,$post:ident,$final:ident,$kind:ident) => {
        owned!($short, 240, {
            receipt_fault(
                Receipt::$kind,
                Step::ReceiptWrite(Receipt::$kind),
                false,
                true,
            );
        });
        owned!($pre, 240, {
            receipt_fault(
                Receipt::$kind,
                Step::ReceiptRename(Receipt::$kind),
                false,
                false,
            );
        });
        owned!($post, 240, {
            receipt_fault(
                Receipt::$kind,
                Step::ReceiptRename(Receipt::$kind),
                true,
                false,
            );
        });
        // Synthetic finalize-arm refusal only; separate R4 recipes reach the real final comparisons.
        owned!($final, 240, {
            receipt_fault(
                Receipt::$kind,
                Step::ReceiptFinalize(Receipt::$kind),
                false,
                false,
            );
        });
    };
}
receipt_cases!(
    allocation_short_write,
    allocation_before_rename,
    allocation_after_rename,
    allocation_final_check_fault,
    Allocation
);
receipt_cases!(
    prepared_short_write,
    prepared_before_rename,
    prepared_after_rename,
    prepared_final_check_fault,
    Prepared
);
receipt_cases!(
    terminal_short_write,
    terminal_before_rename,
    terminal_after_rename,
    terminal_final_check_fault,
    Abandoned
);
owned!(actual_family_commit_busy_stops_before_private_init, 240, {
    commit_refusal(Sql::RegisterInitializing);
});
owned!(actual_private_commit_busy_preserves_callback_cause, 240, {
    commit_refusal(Sql::PrepareControl);
});
owned!(family_commit_known_close_busy_retains_confirmation, 240, {
    close_busy(Sql::RegisterInitializing);
});
owned!(private_init_commit_known_close_busy_forbids_format, 240, {
    close_busy(Sql::InitializePrivate);
});
owned!(
    private_initialization_precommit_refusal_forbids_format_replay,
    240,
    {
        let f = Fixture::new();
        stop(Step::SqlCommit(Sql::InitializePrivate), false);
        let e = failure(f.begin());
        no_later_writes(&e, |s| s == Step::FormatWrite);
        assert_synthetic(&e.cause, "synthetic exact writer boundary");
        hooks::reset();
        let i = f.inspect();
        assert_eq!(i.format_state, ProviderFormatState::Empty);
        let before = f.witness();
        let e = failure(f.recovery().resume_control_publication());
        ineligible_inspection_refusal(
            &e,
            "resume provider control publication",
            Some(ProviderDatabaseScope::PrivateMetadata),
            "provider publication requires validated initialized databases",
        );
        assert!(e.write_report.unwrap().effects.is_empty());
        assert_eq!(before, f.witness());
    }
);
owned!(partial_format_restart_never_rewrites_or_initializes, 240, {
    let f = Fixture::new();
    hooks::prefix(None, None);
    let e = failure(f.begin());
    assert!(has(
        &e,
        Step::SqlCommit(Sql::InitializePrivate),
        State::Confirmed
    ));
    assert_synthetic(
        &e.cause,
        "synthetic short writer payload after real prefix write",
    );
    hooks::reset();
    let a = stages(&f);
    let format = a.stage_identity().unwrap().stage().join("FORMAT");
    let before = fs::read(&format).unwrap();
    assert!(!before.is_empty());
    assert!(before.len() < a.format_bytes as usize);
    let i = f.inspect();
    assert_eq!(i.format_state, ProviderFormatState::IncompletePrefix);
    let e = failure(f.recovery().resume_control_publication());
    assert!(e.write_report.unwrap().effects.is_empty());
    assert_eq!(before, fs::read(format).unwrap());
});
owned!(
    existing_prepared_allocating_callback_authenticates_without_rewrite,
    240,
    {
        let f = Fixture::new();
        stop(Step::SqlOpen(Sql::PrepareControl), false);
        let e = failure(f.begin());
        assert_synthetic(&e.cause, "synthetic exact writer boundary");
        assert_eq!(phase(&f.metadata.join("projection.sqlite3")), "allocating");
        let before = super::super::super::database_io::tests::support::witness(
            &f.receipt(Receipt::Prepared),
        );
        hooks::reset();
        let outcome = f.recovery().resume_control_publication().unwrap();
        let p = match outcome {
            ProviderControlPublicationOutcome::Published(p) => p,
            _ => panic!("expected real callback publication"),
        };
        assert_eq!(
            before,
            super::super::super::database_io::tests::support::witness(
                &f.receipt(Receipt::Prepared)
            )
        );
        let r = p.inspection().write_report.as_ref().unwrap();
        assert!(
            !r.effects
                .iter()
                .any(|e| e.step == Step::ReceiptWrite(Receipt::Prepared))
        );
        assert!(
            r.effects
                .iter()
                .any(|e| e.step == Step::SqlCommit(Sql::PrepareControl)
                    && e.state == State::Confirmed)
        );
    }
);
owned!(
    committed_prepared_resume_skips_callback_and_releases_observer,
    240,
    {
        let f = Fixture::new();
        stop_at_prepared(&f);
        let recovery = f.recovery();
        hooks::reset();
        let outcome = recovery.resume_control_publication().unwrap();
        let p = match outcome {
            ProviderControlPublicationOutcome::Published(p) => p,
            _ => panic!("expected preparedSome publication"),
        };
        assert!(
            p.inspection()
                .write_report
                .as_ref()
                .unwrap()
                .effects
                .iter()
                .all(|e| e.step != Step::SqlOpen(Sql::PrepareControl)
                    && e.step != Step::ReceiptWrite(Receipt::Prepared))
        );
        assert_eq!(
            p.inspection().state,
            ProviderBootstrapState::ControlPublished
        );
    }
);
owned!(
    held_strong_stage_observer_refuses_rename_then_explicit_retry,
    240,
    {
        let f = Fixture::new();
        stop_at_prepared(&f);
        let a = stages(&f);
        let observer =
            layrs_vfs::observe_allocated_control_stage(&a.stage_identity().unwrap()).unwrap();
        let p_before = fs::read(f.receipt(Receipt::Prepared)).unwrap();
        let e = failure(f.recovery().resume_control_publication());
        assert!(matches!(
            &e.cause,
            Cause::Vfs(layrs_vfs::Error::ControlDirectory {
                kind: layrs_vfs::ControlDirectoryFailure::RenameRefused,
                ..
            })
        ));
        assert_eq!(native_source(&e.cause), Some(5));
        assert_eq!(p_before, fs::read(f.receipt(Receipt::Prepared)).unwrap());
        assert!(a.stage_identity().unwrap().stage().exists());
        drop(observer);
        hooks::reset();
        match f.recovery().resume_control_publication().unwrap() {
            ProviderControlPublicationOutcome::Published(_) => (),
            _ => panic!("explicit exact preparedSome retry"),
        }
    }
);
owned!(
    final_guard_transfer_is_idempotent_and_has_empty_issuance,
    180,
    {
        let f = Fixture::new();
        drop(f.published());
        let before = f.witness();
        hooks::reset();
        let p = match f.recovery().resume_control_publication().unwrap() {
            ProviderControlPublicationOutcome::Published(p) => p,
            _ => panic!("idempotent transfer"),
        };
        assert!(
            p.inspection()
                .write_report
                .as_ref()
                .unwrap()
                .effects
                .is_empty()
        );
        assert_eq!(before, f.witness());
        assert!(hooks::events().is_empty());
    }
);

owned!(prepared_mismatch_refuses_without_rewrite, 240, {
    let f = Fixture::new();
    stop_at_prepared(&f);
    let p = f.receipt(Receipt::Prepared);
    let mut record = prepared(&f);
    let n = record.control_identity_hex.len();
    let replacement = if &record.control_identity_hex[n - 2..] == "30" {
        "31"
    } else {
        "30"
    };
    record
        .control_identity_hex
        .replace_range(n - 2.., replacement);
    let bytes = record.encode().unwrap();
    fs::write(&p, &bytes).unwrap();
    let before = f.witness();
    let e = failure(PartialStoreFamily::inspect_provider_bootstrap(
        f.family.root(),
        &f.worktree,
        fixture::TX,
    ));
    invalid(&e.cause, "prepared FORMAT size/content differs");
    assert_eq!(before, f.witness());
    assert!(hooks::events().is_empty());
});
owned!(allocating_final_present_never_promotes, 240, {
    let f = Fixture::new();
    let p = f.published();
    let a = stages(&f);
    let m = MountRecord {
        schema_version: "layrs.projection-provider-mount.v1".into(),
        transaction_id: fixture::TX.into(),
        catalog_id: a.source.catalog_id.clone(),
        phase: MountPhase::Allocating,
        allocation: a.data_reference().unwrap(),
        prepared: None,
    };
    drop(p);
    raw_sql(&f.metadata.join("projection.sqlite3"), |c| {
        c.execute(
            "UPDATE projection_provider_mounts SET phase='allocating',payload=?1,digest=?2",
            rusqlite::params![m.encode().unwrap(), m.digest().unwrap()],
        )
        .unwrap();
    });
    hooks::reset();
    let i = f.inspect();
    assert_eq!(i.state, ProviderBootstrapState::NeedsRepair);
    let before = f.witness();
    let e = failure(f.recovery().resume_control_publication());
    ineligible_inspection_refusal(
        &e,
        "resume provider control publication",
        None,
        "provider bootstrap is not eligible for publication-only resume",
    );
    assert!(e.write_report.unwrap().effects.is_empty());
    assert_eq!(before, f.witness());
});
owned!(
    publication_commit_known_close_busy_withholds_owned_guard,
    240,
    {
        let f = Fixture::new();
        hooks::close_busy(Sql::PublishControl);
        let w = match f.begin().unwrap() {
            ProviderControlPublicationOutcome::CommittedCapabilityWithheld(w) => w,
            _ => panic!("known COMMIT close failure must withhold"),
        };
        assert_eq!(
            w.commit_disposition(),
            ProviderControlCommitDisposition::Committed
        );
        assert!(cause_sqlite(w.cause(), rusqlite::ffi::SQLITE_BUSY));
        assert!(has(
            w.failure(),
            Step::SqlCommit(Sql::PublishControl),
            State::Confirmed
        ));
        assert!(has(
            w.failure(),
            Step::SqlClose(Sql::PublishControl),
            State::Uncertain
        ));
        let identity = prepared(&f).control_identity().unwrap();
        let refusal = layrs_vfs::ControlDirectoryGuard::open(&f.checkout, &identity)
            .err()
            .expect("withheld owns sole native guard");
        assert_eq!(native_source(&Cause::Vfs(refusal)), Some(32));
    }
);
owned!(
    publication_commit_refused_is_unknown_and_keeps_guard,
    240,
    {
        let f = Fixture::new();
        hooks::commit_busy(Sql::PublishControl);
        let w = match f.begin().unwrap() {
            ProviderControlPublicationOutcome::CommitOutcomeUnknown(w) => w,
            _ => panic!("COMMIT error must remain unknown"),
        };
        assert_eq!(
            w.commit_disposition(),
            ProviderControlCommitDisposition::Unknown
        );
        assert!(cause_sqlite(w.cause(), rusqlite::ffi::SQLITE_BUSY));
        assert!(has(
            w.failure(),
            Step::SqlCommit(Sql::PublishControl),
            State::Uncertain
        ));
        assert!(!has(
            w.failure(),
            Step::SqlCommit(Sql::PublishControl),
            State::Confirmed
        ));
        hooks::release_reader();
        let identity = prepared(&f).control_identity().unwrap();
        let e = layrs_vfs::ControlDirectoryGuard::open(&f.checkout, &identity)
            .err()
            .unwrap();
        assert_eq!(native_source(&Cause::Vfs(e)), Some(32));
        drop(w);
        assert_eq!(
            phase(&f.metadata.join("projection.sqlite3")),
            "control_prepared"
        );
    }
);
owned!(
    publication_postcommit_binding_refusal_keeps_confirmed_effect,
    240,
    {
        let f = Fixture::new();
        let path = f.receipt(Receipt::Allocation);
        hooks::observe(move |e| {
            if e.after && e.step == Step::SqlClose(Sql::PublishControl) {
                let mut file = fs::OpenOptions::new().append(true).open(&path)?;
                file.write_all(b" ")?;
                file.sync_all()?;
            }
            Ok(())
        });
        let w = match f.begin().unwrap() {
            ProviderControlPublicationOutcome::CommittedCapabilityWithheld(w) => w,
            _ => panic!("committed phase with invalid concluding receipt"),
        };
        invalid(w.cause(), "provider receipt exceeds bound");
        assert!(has(
            w.failure(),
            Step::SqlCommit(Sql::PublishControl),
            State::Confirmed
        ));
        assert!(has(
            w.failure(),
            Step::SqlClose(Sql::PublishControl),
            State::Confirmed
        ));
        assert!(w.last_inspection().is_some());
    }
);
owned!(
    publication_postcommit_terminal_appearance_is_not_success,
    240,
    {
        let f = Fixture::new();
        let path = f.receipt(Receipt::Abandoned);
        hooks::observe(move |e| {
            if e.after && e.step == Step::SqlClose(Sql::PublishControl) {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                file.write_all(b"unknown terminal")?;
                file.sync_all()?;
            }
            Ok(())
        });
        let w = match f.begin().unwrap() {
            ProviderControlPublicationOutcome::CommittedCapabilityWithheld(w) => w,
            _ => panic!("terminal appearance must withhold"),
        };
        assert!(has(
            w.failure(),
            Step::SqlCommit(Sql::PublishControl),
            State::Confirmed
        ));
        invalid(
            w.cause(),
            "provider receipt appeared; fresh binding required",
        );
        assert_eq!(
            fs::read(f.receipt(Receipt::Abandoned)).unwrap(),
            b"unknown terminal"
        );
    }
);
owned!(
    begin_issuance_first_lock_error_has_retained_provenance,
    180,
    {
        let f = Fixture::new();
        let p = f.published();
        let _lock = crate::lock::StoreLock::acquire_existing(
            &f.family.root().join("metadata/lifecycle.lock"),
            crate::lock::SessionMode::Exclusive,
        )
        .unwrap();
        hooks::reset();
        let e = failure(p.revalidate());
        assert!(matches!(
            e.cause,
            Cause::Store(crate::StoreError::ReentrantLock(_))
        ));
        assert_retained(&p, &e, ProviderBootstrapWriterOperation::Begin);
    }
);
owned!(
    resume_issuance_later_receipt_refusal_keeps_historical_entries,
    180,
    {
        let f = Fixture::new();
        stop_at_prepared(&f);
        let p = match f.recovery().resume_control_publication().unwrap() {
            ProviderControlPublicationOutcome::Published(p) => p,
            _ => panic!("resume publication"),
        };
        let path = f.receipt(Receipt::Allocation);
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(b" ").unwrap();
        file.sync_all().unwrap();
        drop(file);
        hooks::reset();
        let e = failure(p.revalidate());
        invalid(&e.cause, "provider receipt exceeds bound");
        assert_retained(
            &p,
            &e,
            ProviderBootstrapWriterOperation::ResumeControlPublication,
        );
    }
);
owned!(
    empty_resume_issuance_later_terminal_refusal_stays_empty,
    180,
    {
        let f = Fixture::new();
        drop(f.published());
        let p = match f.recovery().resume_control_publication().unwrap() {
            ProviderControlPublicationOutcome::Published(p) => p,
            _ => panic!("idempotent resume"),
        };
        assert!(
            p.inspection()
                .write_report
                .as_ref()
                .unwrap()
                .effects
                .is_empty()
        );
        fs::write(f.receipt(Receipt::Abandoned), b"incomplete terminal").unwrap();
        hooks::reset();
        let e = failure(p.revalidate());
        invalid(
            &e.cause,
            "provider receipt appeared; fresh binding required",
        );
        assert_retained(
            &p,
            &e,
            ProviderBootstrapWriterOperation::ResumeControlPublication,
        );
        assert!(e.write_report.unwrap().effects.is_empty());
    }
);
owned!(pending_ok_inspection_is_not_published_revalidation, 180, {
    let f = Fixture::new();
    let p = f.published();
    let wal = f.metadata.join("projection.sqlite3-wal");
    fs::write(&wal, b"retained unsupported sidecar").unwrap();
    hooks::reset();
    let i = f.inspect();
    assert_eq!(i.state, ProviderBootstrapState::SqlValidationPending);
    assert_eq!(i.private.status, ProviderDatabaseStatus::Unavailable);
    invalid(
        i.private.cause.as_ref().unwrap(),
        "provider database WAL/SHM sidecar present before SQLite open",
    );
    let e = failure(p.revalidate());
    ineligible_inspection_refusal(
        &e,
        "revalidate provider control publication",
        Some(ProviderDatabaseScope::PrivateMetadata),
        "provider database WAL/SHM sidecar present before SQLite open",
    );
    assert!(matches!(e.cause, Cause::Validation(_)));
    assert_retained(&p, &e, ProviderBootstrapWriterOperation::Begin);
    assert_eq!(fs::read(wal).unwrap(), b"retained unsupported sidecar");
});
owned!(
    successful_revalidation_has_zero_new_entries_and_writes,
    180,
    {
        let f = Fixture::new();
        let p = f.published();
        let issuance = p.inspection().write_report.as_ref().unwrap().clone();
        let before = f.witness();
        hooks::reset();
        p.revalidate().unwrap();
        p.revalidate().unwrap();
        assert!(hooks::events().is_empty());
        assert_eq!(before, f.witness());
        let r = p.inspection().write_report.as_ref().unwrap();
        assert_eq!(r.provenance, issuance.provenance);
        for (a, b) in r.effects.iter().zip(&issuance.effects) {
            assert_eq!((a.step, a.state, &a.target), (b.step, b.state, &b.target));
        }
        assert_eq!(r.effects.len(), issuance.effects.len());
    }
);
owned!(
    old_inspect_recover_constructors_keep_none_with_real_writer_state,
    180,
    {
        let f = Fixture::new();
        drop(f.published());
        hooks::reset();
        let r = f.recovery();
        for i in [r.inspect().unwrap(), r.recover_databases().unwrap()] {
            assert!(i.write_report.is_none());
            assert!(i.effects.is_empty());
            assert!(i.rollback_triggers.is_empty());
        }
        assert!(hooks::events().is_empty());
    }
);
owned!(
    staged_abandon_preserves_all_physical_data_and_terminal_first,
    240,
    {
        abandon_control(true);
    }
);
owned!(
    final_abandon_preserves_all_physical_data_and_terminal_first,
    240,
    {
        abandon_control(false);
    }
);
owned!(
    existing_terminal_initializing_completes_without_rewriting,
    240,
    {
        let f = Fixture::new();
        drop(f.published());
        let mut r = f.recovery();
        stop(Step::SqlOpen(Sql::RemoveInitializing), false);
        let e = failure(r.abandon());
        assert_synthetic(&e.cause, "synthetic exact writer boundary");
        let t = fs::read(f.receipt(Receipt::Abandoned)).unwrap();
        drop(r);
        hooks::reset();
        let mut r = f.recovery();
        assert_eq!(
            r.inspect().unwrap().state,
            ProviderBootstrapState::AbandonIncomplete
        );
        let i = r.abandon().unwrap();
        assert_eq!(i.state, ProviderBootstrapState::Abandoned);
        assert!(!i.write_report.unwrap().effects.iter().any(|e| matches!(
            e.step,
            Step::ReceiptWrite(Receipt::Abandoned) | Step::ReceiptRename(Receipt::Abandoned)
        )));
        assert_eq!(t, fs::read(f.receipt(Receipt::Abandoned)).unwrap());
    }
);
owned!(ambiguous_terminal_never_removes_registry_or_rebinds, 240, {
    let f = Fixture::new();
    drop(f.published());
    let mut r = f.recovery();
    let expected = r.binding().encode().unwrap();
    hooks::prefix(Some(Receipt::Abandoned), None);
    let e = failure(r.abandon());
    assert_synthetic(
        &e.cause,
        "synthetic short writer payload after real prefix write",
    );
    no_later_writes(&e, |s| s == Step::SqlOpen(Sql::RemoveInitializing));
    assert_eq!(r.binding().encode().unwrap(), expected);
    assert!(!f.receipt(Receipt::Abandoned).exists());
    assert_eq!(
        phase(&f.metadata.join("projection.sqlite3")),
        "control_published"
    );
});
owned!(
    mismatching_existing_terminal_refuses_without_effects,
    180,
    {
        let f = Fixture::new();
        drop(f.published());
        let mut r = f.recovery();
        r.abandon().unwrap();
        drop(r);
        let path = f.receipt(Receipt::Abandoned);
        let mut t = AbandonedRecord::decode(&fs::read(&path).unwrap()).unwrap();
        t.transaction_id = "00000000000000000000000000000002".into();
        let bytes = t.encode().unwrap();
        fs::write(&path, &bytes).unwrap();
        hooks::reset();
        let before = f.witness();
        let e = failure(PartialStoreFamily::inspect_provider_bootstrap(
            f.family.root(),
            &f.worktree,
            fixture::TX,
        ));
        invalid(&e.cause, "terminal allocation reference differs");
        assert!(hooks::events().is_empty());
        assert_eq!(before, f.witness());
    }
);
macro_rules! dependency {
    ($name:ident,$table:literal) => {
        owned!($name, 180, {
            dependent_private($table);
        });
    };
}

dependency!(abandon_refuses_projection_install, "projection_install");
dependency!(
    abandon_refuses_projection_refresh_pending,
    "projection_refresh_pending"
);
dependency!(
    abandon_refuses_projection_refresh_history,
    "projection_refresh_history"
);
dependency!(
    abandon_refuses_projection_capture_files,
    "projection_capture_files"
);
dependency!(
    abandon_refuses_projection_capture_chunks,
    "projection_capture_chunks"
);
dependency!(abandon_refuses_projection_captures, "projection_captures");
dependency!(
    abandon_refuses_projection_publication_pending,
    "projection_publication_pending"
);
dependency!(
    abandon_refuses_projection_publications,
    "projection_publications"
);
dependency!(
    abandon_refuses_projection_namespace_pending,
    "projection_namespace_pending"
);
dependency!(
    abandon_refuses_projection_namespace_operations,
    "projection_namespace_operations"
);
dependency!(
    abandon_refuses_projection_restore_pending,
    "projection_restore_pending"
);
dependency!(
    abandon_refuses_projection_restore_history,
    "projection_restore_history"
);
dependency!(
    abandon_refuses_projection_rebase_pending,
    "projection_rebase_pending"
);
dependency!(
    abandon_refuses_projection_rebase_history,
    "projection_rebase_history"
);
dependency!(
    abandon_refuses_projection_rebase_previews,
    "projection_rebase_previews"
);
dependency!(abandon_refuses_projection_content, "projection_content");
dependency!(abandon_refuses_projection_claims, "projection_claims");
dependency!(abandon_refuses_projection_budgets, "projection_budgets");
dependency!(
    abandon_refuses_projection_prejournal,
    "projection_prejournal"
);
dependency!(
    abandon_refuses_projection_preparations,
    "projection_preparations"
);
dependency!(
    abandon_refuses_projection_observations,
    "projection_observations"
);
dependency!(abandon_refuses_projection_imports, "projection_imports");
dependency!(
    abandon_refuses_projection_conversion,
    "projection_conversion"
);
dependency!(
    abandon_refuses_projection_provider_activations,
    "projection_provider_activations"
);
dependency!(
    abandon_refuses_projection_provider_capture_transactions,
    "projection_provider_capture_transactions"
);
dependency!(
    abandon_refuses_projection_provider_capture_receipts,
    "projection_provider_capture_receipts"
);
dependency!(
    abandon_refuses_projection_provider_transitions,
    "projection_provider_transitions"
);
dependency!(
    abandon_refuses_projection_provider_transition_receipts,
    "projection_provider_transition_receipts"
);
dependency!(
    abandon_refuses_projection_provider_companions,
    "projection_provider_companions"
);
dependency!(
    abandon_refuses_projection_provider_companion_files,
    "projection_provider_companion_files"
);
dependency!(
    abandon_refuses_projection_provider_companion_deletions,
    "projection_provider_companion_deletions"
);

owned!(abandon_refuses_target_family_roots, 180, {
    family_dependency("roots");
});
owned!(abandon_refuses_target_family_reservations, 180, {
    family_dependency("reservations");
});
owned!(
    abandon_refuses_installation_with_its_required_reservation,
    180,
    {
        family_dependency("installations");
    }
);
owned!(abandon_preserves_unrelated_protected_family_member, 180, {
    let f = Fixture::new();
    let other = f.outer.join("other");
    fs::create_dir(&other).unwrap();
    let input = super::super::super::validation::tests::fixture::Fixture::new("G1", false).source;
    let id = WorktreeId::parse("worktree:writer-unrelated-control").unwrap();
    let mut initial = input.initial().clone();
    initial.entries[0].writable = true;
    let store = f
        .family
        .create_worktree(
            &other,
            id.clone(),
            initial.clone(),
            input.selection().clone(),
        )
        .unwrap();
    let mut materialized = initial;
    materialized.entries[0].state = layrs_core::projection::EntryState::Materialized;
    store
        .install_projection(0, &materialized, &mut |_, _| Ok(()))
        .unwrap();
    fs::write(other.join("README.md"), b"independent accepted capture").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let kept = layrs_core::scm::RepoPath::parse("README.md").unwrap();
    let manifest = store
        .captured_file_manifest(capture.sequence, &kept)
        .unwrap();
    let mut protected_before = Vec::<(String, Vec<u8>)>::new();
    raw_sql(&f.family.root().join("metadata/catalog.sqlite3"), |c| {
        let mut q = c
            .prepare("SELECT id,payload FROM roots WHERE owner=?1 ORDER BY id")
            .unwrap();
        protected_before = q
            .query_map([id.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(!protected_before.is_empty());
    });
    drop(f.published());
    let mut r = f.recovery();
    r.abandon().unwrap();
    raw_sql(&f.family.root().join("metadata/catalog.sqlite3"), |c| {
        let mut q = c
            .prepare("SELECT id,payload FROM roots WHERE owner=?1 ORDER BY id")
            .unwrap();
        let after = q
            .query_map([id.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<(String, Vec<u8>)>>>()
            .unwrap();
        assert_eq!(protected_before, after);
    });
    assert_eq!(
        manifest.id(),
        store
            .captured_file_manifest(capture.sequence, &kept)
            .unwrap()
            .id()
    );
    assert_eq!(
        fs::read(other.join("README.md")).unwrap(),
        b"independent accepted capture"
    );
});
owned!(
    registered_final_root_refuses_abandon_without_terminal,
    180,
    {
        let f = Fixture::new();
        let p = f.published();
        let catalog = Arc::new(layrs_vfs::Catalog::new(0, Vec::new()).unwrap());
        // Actual qualified public VFS metadata constructor, no callbacks/connection.
        // Test-only external registration interference; never a Store writer feature.
        let registered = layrs_vfs::ProviderRootGuard::create_with_control_directory(
            &p.held.guard,
            [19; 16],
            catalog,
            |registration| {
                super::super::super::database_io::tests::support::create(
                    &f.outer.join("external-registration.bin"),
                    &registration.encode(),
                );
                Ok(())
            },
        )
        .expect("UNQUALIFIED if this host cannot create the owned disposable registration");
        drop(registered);
        drop(p);
        hooks::reset();
        let mut r = f.recovery();
        let before = f.witness();
        let e = failure(r.abandon());
        assert!(matches!(
            &e.cause,
            Cause::Vfs(layrs_vfs::Error::ControlDirectory {
                kind: layrs_vfs::ControlDirectoryFailure::NeedsRepair,
                operation: "revalidate unregistered root",
                ..
            })
        ));
        assert!(e.write_report.unwrap().effects.is_empty());
        assert_eq!(before, f.witness());
        assert!(!f.receipt(Receipt::Abandoned).exists());
    }
);

owned!(
    begin_first_lifecycle_lock_failure_keeps_empty_current_report,
    180,
    {
        let f = Fixture::new();
        let before = f.witness();
        let _lock = crate::lock::StoreLock::acquire_existing(
            &f.family.root().join("metadata/lifecycle.lock"),
            crate::lock::SessionMode::Exclusive,
        )
        .unwrap();
        let e = failure(f.begin());
        assert!(matches!(
            e.cause,
            Cause::Store(crate::StoreError::ReentrantLock(_))
        ));
        let report = e.write_report.unwrap();
        assert_eq!(
            report.provenance,
            ProviderBootstrapWriteReportProvenance::CurrentInvocation {
                operation: ProviderBootstrapWriterOperation::Begin
            }
        );
        assert!(report.effects.is_empty());
        assert!(!f.metadata.exists());
        assert_eq!(before, f.witness());
    }
);
owned!(
    pre_a_failed_creation_cannot_reuse_worktree_with_new_transaction,
    180,
    {
        let f = Fixture::new();
        hooks::fail_owner();
        let first = failure(f.begin());
        assert_synthetic(
            &first.cause,
            "synthetic writer stop after owner acquisition",
        );
        let member_id = crate::secure_fs::PinnedDirectory::open(f.metadata.parent().unwrap())
            .unwrap()
            .identity_token();
        let lock = super::super::super::database_io::tests::support::witness(
            &f.metadata.join("layrs.lock"),
        );
        let before = f.witness();
        hooks::reset();
        let second = failure(f.family.begin_provider_bootstrap(
            &f.checkout,
            &f.stages,
            &f.worktree,
            "00000000000000000000000000000002",
            Fixture::source(),
        ));
        assert!(matches!(second.cause,Cause::Io(ref e)if e.raw_os_error()==Some(183)));
        assert_eq!(second.write_report.as_ref().unwrap().effects.len(), 1);
        assert_eq!(
            member_id,
            crate::secure_fs::PinnedDirectory::open(f.metadata.parent().unwrap())
                .unwrap()
                .identity_token()
        );
        assert_eq!(
            lock,
            super::super::super::database_io::tests::support::witness(
                &f.metadata.join("layrs.lock")
            )
        );
        assert_eq!(before, f.witness());
        assert!(!f.receipt(Receipt::Allocation).exists());
    }
);

owned!(
    actual_callback_cleanup_preserves_arc_identity_and_both_causes,
    180,
    {
        use std::error::Error;
        let f = Fixture::new();
        hooks::close_busy(Sql::PrepareControl);
        let e = failure(f.begin());
        let original = hooks::take_callback_cause();
        assert!(matches!(
            &e.cause,
            Cause::Vfs(layrs_vfs::Error::ControlDirectory {
                kind: layrs_vfs::ControlDirectoryFailure::ReceiptDurabilityUnknown,
                ..
            })
        ));
        let mut next: Option<&(dyn Error + 'static)> = Some(&e.cause);
        let mut same = false;
        while let Some(error) = next {
            if let Some(cause) = error.downcast_ref::<Cause>() {
                if std::ptr::eq(cause, original.as_ref()) {
                    same = true;
                    break;
                }
            }
            next = error.source();
        }
        assert!(same);
        match original.as_ref() {
            Cause::Cleanup { primary, close } => {
                invalid(primary, "observable SQLite close failed");
                assert!(
                    matches!(close.as_ref(),Cause::Sqlite(rusqlite::Error::SqliteFailure(code,_))if code.extended_code==rusqlite::ffi::SQLITE_BUSY)
                );
                assert!(std::ptr::eq(
                    original.source().unwrap().downcast_ref::<Cause>().unwrap(),
                    primary.as_ref()
                ));
            }
            other => panic!("missing actual cleanup pair: {other:?}"),
        }
        assert!(has(
            &e,
            Step::SqlCommit(Sql::PrepareControl),
            State::Confirmed
        ));
        assert!(has(
            &e,
            Step::SqlClose(Sql::PrepareControl),
            State::Uncertain
        ));
        assert!(!f.checkout.join(".layrs").exists());
    }
);

owned!(
    ordinary_nonempty_staged_root_permits_only_proven_no_claims_abandon,
    180,
    {
        user_path_abandon(true);
    }
);
owned!(
    final_user_paths_are_preserved_by_unregistered_no_claims_abandon,
    180,
    {
        user_path_abandon(false);
    }
);

// R4 qualification: every native case still requires its exact owned child.
// Setup completes before counters are baselined; no historical result transfers.
use database_io::tests::writer_support as writer_observer;
use std::{cell::RefCell, rc::Rc};
macro_rules! observed_owned {
    ($name:ident,$seconds:literal,$f:ident,$setup:expr,$body:block)=>{
        #[test]
        #[ignore="UNQUALIFIED until ROOT authorizes this exact owned audited-host writer recipe and absolute disposable parent"]
        fn $name(){child::run(concat!(module_path!(),"::",stringify!($name)),$seconds,||{
            hooks::reset(); writer_observer::clear_observer();
            let $f=Rc::new($setup);
            writer_observer::install(&$f.family.root().join("metadata/catalog.sqlite3"));
            child::declare();
            $body
        });}
    };
}
observed_owned!(
    begin_rejects_overlong_family_incarnation_before_allocation,
    180,
    f,
    Fixture::new().with_incarnation(129),
    {
        let before = f.witness();
        let events = writer_observer::events().len();
        let e = failure(f.begin());
        invalid(&e.cause, "family incarnation length invalid");
        check_no_allocation(&f, &e, &before, events);
    }
);
observed_owned!(
    begin_rejects_family_filename_fragment_before_allocation,
    180,
    f,
    Fixture::with_family_name("family#one"),
    {
        let before = f.witness();
        let events = writer_observer::events().len();
        let e = failure(f.begin());
        invalid(
            &e.cause,
            "provider SQLite requires an exact absolute local native filename",
        );
        check_no_allocation(&f, &e, &before, events);
    }
);
observed_owned!(
    begin_context_bounds_valid_control,
    180,
    f,
    Fixture::new().with_incarnation(128),
    {
        let p = f.published();
        assert_eq!(
            p.inspection().state,
            ProviderBootstrapState::ControlPublished
        );
        assert_eq!(stages(&f).family.incarnation.len(), 128);
        assert!(p.revalidate().is_ok());
    }
);
observed_owned!(
    begin_registration_hot_family_refuses_without_rw,
    240,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Hot);
    }
);
observed_owned!(
    begin_registration_nonhot_journal_refuses_after_ro,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Nonhot);
    }
);
observed_owned!(
    begin_registration_wal_refuses_before_ro,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Wal);
    }
);
observed_owned!(
    begin_registration_shm_refuses_before_ro,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Shm);
    }
);
observed_owned!(
    begin_registration_unreadable_journal_is_not_absent,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Unreadable);
    }
);
observed_owned!(
    begin_registration_reparse_journal_is_not_absent,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Reparse);
    }
);
observed_owned!(
    begin_registration_unsupported_header_refuses_before_ro,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Unsupported);
    }
);
observed_owned!(
    begin_registration_corrupt_catalog_refuses_without_rw,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::Corrupt);
    }
);
observed_owned!(
    private_prepare_hot_journal_refuses_without_rw,
    240,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::PrivateHot);
    }
);
observed_owned!(
    initializer_zero_origin_sidecar_refuses_before_open,
    180,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::FreshSidecar);
    }
);
observed_owned!(
    terminal_removal_hot_family_keeps_terminal_and_registry,
    240,
    f,
    Fixture::new(),
    {
        admission_case(f, AdmissionCase::RemovalHot);
    }
);

observed_owned!(
    catalog_gate_continuous_across_read_to_write,
    240,
    f,
    Fixture::new(),
    {
        let phases = Rc::new(RefCell::new(Vec::new()));
        let seen = phases.clone();
        let ff = f.clone();
        let db = f.family.root().join("metadata/catalog.sqlite3");
        writer_observer::observe(move |path, point| {
            if path == db {
                process::competing_catalog(&ff, true);
                seen.borrow_mut().push(point);
            }
            Ok(())
        });
        let opened = Rc::new(std::cell::Cell::new(false));
        let reached = opened.clone();
        let ff = f.clone();
        hooks::observe(move |e| {
            if e.after && e.step == Step::SqlOpen(Sql::RegisterInitializing) {
                process::competing_catalog(&ff, true);
                reached.set(true);
            }
            Ok(())
        });
        let committed = Rc::new(std::cell::Cell::new(false));
        let reached = committed.clone();
        let ff = f.clone();
        hooks::on_sql_commit(move |after, purpose, _, _| {
            if !after && purpose == Sql::RegisterInitializing {
                process::competing_catalog(&ff, true);
                reached.set(true);
            }
            Ok(())
        });
        let published = f.published();
        assert_eq!(
            *phases.borrow(),
            vec![
                writer_observer::AdmissionPoint::ReadonlyActive,
                writer_observer::AdmissionPoint::ReadonlyClosed,
                writer_observer::AdmissionPoint::BeforeWritableOpen
            ]
        );
        assert!(opened.get() && committed.get());
        writer_observer::clear_observer();
        hooks::reset();
        process::competing_catalog(&f, false);
        published.revalidate().unwrap();
    }
);
observed_owned!(
    catalog_inode_replacement_before_rw_is_rejected,
    180,
    f,
    Fixture::new(),
    {
        let original =
            database_io::tests::support::witness(&f.family.root().join("metadata/catalog.sqlite3"));
        let saved = f.family.root().join("metadata/catalog-retained.sqlite3");
        let moved = saved.clone();
        let db = f.family.root().join("metadata/catalog.sqlite3");
        let watched = db.clone();
        let reached = Rc::new(std::cell::Cell::new(false));
        let seen = reached.clone();
        let baseline = writer_observer::events().len();
        writer_observer::observe(move |path, point| {
            if path == watched && point == writer_observer::AdmissionPoint::ReadonlyClosed {
                assert!(!seen.replace(true));
                let bytes = fs::read(path).unwrap();
                assert!(bytes.len() <= 16 * 1024 * 1024);
                fs::rename(path, &moved)
                    .expect("UNQUALIFIED retained inode rename; no handle/share fallback");
                database_io::tests::support::create(path, &bytes);
            }
            Ok(())
        });
        let e = failure(f.begin());
        assert!(reached.get());
        invalid(&e.cause, "projection database file replaced");
        assert_eq!(disk_rw_since(baseline), 0);
        let replacement = database_io::tests::support::witness(&db);
        assert_eq!(database_io::tests::support::witness(&saved), original);
        assert_ne!(replacement.0, original.0);
        assert_eq!((replacement.1, replacement.2), (original.1, original.2));
        assert!(has(
            &e,
            Step::ReceiptFinalize(Receipt::Allocation),
            State::Confirmed
        ));
        no_later_writes(&e, |s| matches!(s, Step::SqlOpen(_)));
    }
);
observed_owned!(
    catalog_gate_released_at_publisher_and_callback_boundaries,
    240,
    f,
    Fixture::new(),
    {
        let points = Rc::new(RefCell::new(Vec::new()));
        let seen = points.clone();
        let ff = f.clone();
        hooks::observe_publish(move |point| {
            if matches!(
                point,
                layrs_vfs::ControlDirectoryPublishPoint::StageValidated
                    | layrs_vfs::ControlDirectoryPublishPoint::AfterPersist
            ) {
                process::competing_catalog(&ff, false);
                seen.borrow_mut().push(point);
            }
            Ok(())
        });
        let callback = Rc::new(std::cell::Cell::new(false));
        let seen = callback.clone();
        let ff = f.clone();
        hooks::on_sql_commit(move |after, purpose, _, _| {
            if !after && purpose == Sql::PrepareControl {
                process::competing_catalog(&ff, false);
                seen.set(true);
            }
            Ok(())
        });
        let p = f.published();
        assert!(callback.get());
        assert_eq!(
            *points.borrow(),
            vec![
                layrs_vfs::ControlDirectoryPublishPoint::StageValidated,
                layrs_vfs::ControlDirectoryPublishPoint::AfterPersist
            ]
        );
        hooks::reset();
        p.revalidate().unwrap();
    }
);

observed_owned!(
    writer_null_open_error_has_no_close_entry,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::NullOpen, None);
    }
);
observed_owned!(
    writer_nonnull_open_error_records_successful_cleanup,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::NonnullOpen, None);
    }
);
observed_owned!(
    writer_adoption_error_records_successful_cleanup,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::Adoption, None);
    }
);
observed_owned!(
    writer_postopen_profile_refusal_records_cleanup,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::Profile, None);
    }
);
observed_owned!(
    writer_ro_rw_geometry_refusal_records_cleanup,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::Geometry, None);
    }
);
observed_owned!(
    writer_raw_cleanup_close_busy_retains_primary_and_quarantine,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::RawBusy, None);
    }
);
observed_owned!(
    writer_owned_cleanup_close_busy_retains_primary_and_quarantine,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::OwnedBusy, None);
    }
);
observed_owned!(
    writer_cleanup_arm_fault_still_closes_once,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::Geometry, Some(false));
    }
);
observed_owned!(
    writer_cleanup_confirmation_fault_preserves_confirmed_close,
    180,
    f,
    Fixture::new(),
    {
        cleanup_case(f, hooks::WriterFault::Geometry, Some(true));
    }
);

observed_owned!(receipt_allocation_temp_collision, 180, f, Fixture::new(), {
    receipt_mutation(f, Receipt::Allocation, ReceiptMutation::TempCollision);
});

observed_owned!(
    receipt_allocation_final_collision,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Allocation, ReceiptMutation::FinalCollision);
    }
);

observed_owned!(
    receipt_allocation_final_identity_replacement,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Allocation, ReceiptMutation::Identity);
    }
);

observed_owned!(
    receipt_allocation_final_length_change,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Allocation, ReceiptMutation::Length);
    }
);

observed_owned!(
    receipt_allocation_final_same_length_byte_change,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Allocation, ReceiptMutation::Bytes);
    }
);

observed_owned!(receipt_prepared_temp_collision, 180, f, Fixture::new(), {
    receipt_mutation(f, Receipt::Prepared, ReceiptMutation::TempCollision);
});

observed_owned!(receipt_prepared_final_collision, 180, f, Fixture::new(), {
    receipt_mutation(f, Receipt::Prepared, ReceiptMutation::FinalCollision);
});

observed_owned!(
    receipt_prepared_final_identity_replacement,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Prepared, ReceiptMutation::Identity);
    }
);

observed_owned!(
    receipt_prepared_final_length_change,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Prepared, ReceiptMutation::Length);
    }
);

observed_owned!(
    receipt_prepared_final_same_length_byte_change,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Prepared, ReceiptMutation::Bytes);
    }
);

observed_owned!(receipt_terminal_temp_collision, 180, f, Fixture::new(), {
    receipt_mutation(f, Receipt::Abandoned, ReceiptMutation::TempCollision);
});

observed_owned!(receipt_terminal_final_collision, 180, f, Fixture::new(), {
    receipt_mutation(f, Receipt::Abandoned, ReceiptMutation::FinalCollision);
});

observed_owned!(
    receipt_terminal_final_identity_replacement,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Abandoned, ReceiptMutation::Identity);
    }
);

observed_owned!(
    receipt_terminal_final_length_change,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Abandoned, ReceiptMutation::Length);
    }
);

observed_owned!(
    receipt_terminal_final_same_length_byte_change,
    180,
    f,
    Fixture::new(),
    {
        receipt_mutation(f, Receipt::Abandoned, ReceiptMutation::Bytes);
    }
);

observed_owned!(
    writer_nested_family_failure_survives_private_cleanup_success,
    240,
    f,
    Fixture::new(),
    {
        nested_cleanup_case(f, NestedCleanup::DirectSuccess);
    }
);
observed_owned!(
    writer_nested_family_failure_survives_callback_close_busy_and_arm_fault,
    240,
    f,
    Fixture::new(),
    {
        nested_cleanup_case(f, NestedCleanup::CallbackBusyArm);
    }
);
observed_owned!(
    writer_nested_family_failure_survives_callback_close_confirmation_fault,
    240,
    f,
    Fixture::new(),
    {
        nested_cleanup_case(f, NestedCleanup::CallbackConfirmedFault);
    }
);
observed_owned!(
    registration_cut_private_fence_rejects_initialized_schema8,
    180,
    f,
    Fixture::new(),
    {
        stop(Step::FormatWrite, false);
        let error = failure(f.begin());
        assert_synthetic(&error.cause, "synthetic exact writer boundary");
        assert!(has(
            &error,
            Step::SqlCommit(Sql::RegisterInitializing),
            State::Confirmed
        ));
        assert!(has(
            &error,
            Step::SqlCommit(Sql::InitializePrivate),
            State::Confirmed
        ));
        let a = stages(&f);
        let private = f.metadata.join("projection.sqlite3");
        let before = database_io::tests::support::witness(&private);
        assert_eq!(before.0, a.private.database_file_identity);
        assert!(before.1 > 0);
        assert_eq!(
            fs::metadata(a.stage_identity().unwrap().stage().join("FORMAT"))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(phase(&private), "allocating");
        let inspected = PartialStoreFamily::inspect_provider_bootstrap(
            f.family.root(),
            &f.worktree,
            fixture::TX,
        )
        .unwrap();
        assert_eq!(inspected.private.status, ProviderDatabaseStatus::Validated);
        let events = writer_observer::events().len();
        let refusal = process::registration_private_fence(&f, &inspected)
            .expect_err("actual schema8 must fail registration fence");
        assert!(matches!(refusal,process::RegistrationPrivateFence::Nonzero(n)if n==before.1));
        assert_eq!(
            main_opens_since(events),
            0,
            "negative fence must not open rejected nonzero DB via SQLite"
        );
        assert_eq!(before, database_io::tests::support::witness(&private));
        assert!(!f.receipt(Receipt::Prepared).try_exists().unwrap());
        assert!(!f.checkout.join(".layrs").try_exists().unwrap());
        assert!(!f.receipt(Receipt::Abandoned).try_exists().unwrap());
    }
);

#[path = "recipes/cleanup_receipts.rs"]
mod cleanup_receipts;
use cleanup_receipts::*;

#[path = "recipes/admission.rs"]
mod admission;
use admission::*;

#[path = "recipes/support.rs"]
mod support;
use support::*;
// The local helper shadows the glob-imported `invalid` of the parent.
use support::invalid;
