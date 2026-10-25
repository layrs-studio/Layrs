use super::super::{database_io::tests::support, process_profile::tests::child};
use super::*;
pub(in crate::projection::provider) mod fixture;
use fixture::NativeFixture;

fn witnesses(f: &NativeFixture) -> Vec<(String, u64, String)> {
    let mut paths = vec![
        f.family.join("FORMAT"),
        f.family.join("metadata/catalog.sqlite3"),
        f.metadata.join("projection.sqlite3"),
        f.metadata.join("provider-control/allocation.json"),
    ];
    if f.prepared.is_some() {
        paths.push(f.metadata.join("provider-control/prepared.json"));
    }
    paths.into_iter().map(|p| support::witness(&p)).collect()
}
fn open(
    f: &NativeFixture,
    binding: &ProviderBootstrapBinding,
) -> Result<ProviderBootstrapRecovery> {
    PartialStoreFamily::open_provider_bootstrap_recovery(
        &f.family,
        &f.allocation.format.identity.worktree_id,
        &f.allocation.transaction_id,
        binding,
    )
}

#[test]
#[ignore = "owned audited Windows/VFS test host and disposable parent; native fixtures retained"]
fn initialized_phases_live_guard_clone_and_repeated_handle_locks() {
    child::run(
        concat!(
            module_path!(),
            "::initialized_phases_live_guard_clone_and_repeated_handle_locks"
        ),
        180,
        || {
            child::declare();
            for (phase, expected) in [
                (MountPhase::Allocating, ProviderBootstrapState::Allocating),
                (
                    MountPhase::ControlPrepared,
                    ProviderBootstrapState::ControlPrepared,
                ),
                (
                    MountPhase::ControlPublished,
                    ProviderBootstrapState::ControlPublished,
                ),
            ] {
                let mut f = NativeFixture::create(phase);
                let before = witnesses(&f);
                let inspection = f.inspect();
                assert_eq!(inspection.state, expected);
                assert!(inspection.effects.is_empty());
                if let Some(guard) = &f.guard {
                    let clone = guard.try_clone().unwrap();
                    assert_eq!(
                        f.inspect().state,
                        expected,
                        "compatible DATA inspection with both real owners retained"
                    );
                    let error = match open(&f, &inspection.binding) {
                        Ok(_) => panic!("second sole DELETE owner unexpectedly acquired"),
                        Err(e) => e,
                    };
                    let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                        panic!("typed native cause required")
                    };
                    assert_eq!(failure.kind, ProviderBootstrapFailureKind::ControlInUse);
                    assert_eq!(witnesses(&f), before);
                    drop(clone);
                }
                drop(f.guard.take());
                let recovery = open(&f, &inspection.binding).unwrap();
                assert_eq!(recovery.inspect().unwrap().state, expected);
                let after = recovery.recover_databases().unwrap();
                assert_eq!(after.state, expected);
                assert!(after.effects.is_empty());
                assert_eq!(
                    recovery.inspect().unwrap().state,
                    expected,
                    "no retained/reentrant StoreLock"
                );
                assert_eq!(witnesses(&f), before);
            }
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS test host; terminal appearance and registry effects are fixture-only"]
fn terminal_dominance_stale_binding_and_validated_registry_absence() {
    child::run(
        concat!(
            module_path!(),
            "::terminal_dominance_stale_binding_and_validated_registry_absence"
        ),
        180,
        || {
            child::declare();
            let f = NativeFixture::create(MountPhase::Allocating);
            let initial = f.inspect();
            let stale = open(&f, &initial.binding).unwrap();
            f.terminal(RegistryAtDecision::Initializing);
            let terminal_witness =
                support::witness(&f.metadata.join("provider-control/abandoned.json"));
            let error = stale.recover_databases().unwrap_err();
            let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                panic!("typed stale terminal failure required")
            };
            assert!(failure.cause.to_string().contains("receipt appeared"));
            assert!(failure.effects.is_empty());
            let current = f.inspect();
            assert_eq!(current.state, ProviderBootstrapState::AbandonIncomplete);
            let fresh = open(&f, &current.binding).unwrap();
            assert_eq!(
                fresh.inspect().unwrap().state,
                ProviderBootstrapState::AbandonIncomplete
            );
            assert_eq!(
                fresh.recover_databases().unwrap().state,
                ProviderBootstrapState::AbandonIncomplete,
                "recovery never deletes registry"
            );
            let c = rusqlite::Connection::open(f.family.join("metadata/catalog.sqlite3")).unwrap();
            c.execute(
                "DELETE FROM worktrees WHERE id=?1",
                [f.allocation.format.identity.worktree_id.as_str()],
            )
            .unwrap();
            c.close().unwrap();
            assert_eq!(
                fresh.inspect().unwrap().state,
                ProviderBootstrapState::Abandoned
            );
            assert_eq!(
                support::witness(&f.metadata.join("provider-control/abandoned.json")),
                terminal_witness
            );
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS test host; zero private file is never initialized by recovery"]
fn zero_private_and_expected_binding_refusal_preserve_bytes() {
    child::run(
        concat!(
            module_path!(),
            "::zero_private_and_expected_binding_refusal_preserve_bytes"
        ),
        180,
        || {
            child::declare();
            let f = NativeFixture::create(MountPhase::Allocating);
            let original = f.inspect();
            let mut wrong =
                ProviderBootstrapBinding::decode(&original.binding.encode().unwrap()).unwrap();
            wrong.recovery.database_identity = "1:0:99".into();
            let before = witnesses(&f);
            let error = match open(&f, &wrong) {
                Err(e) => e,
                Ok(_) => panic!("wrong DB binding accepted"),
            };
            let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                panic!("bound failure expected")
            };
            assert_eq!(failure.kind, ProviderBootstrapFailureKind::BindingChanged);
            assert!(failure.effects.is_empty());
            assert_eq!(witnesses(&f), before);
            let private = f.metadata.join("projection.sqlite3");
            std::fs::OpenOptions::new()
                .write(true)
                .open(&private)
                .unwrap()
                .set_len(0)
                .unwrap();
            let zero = support::witness(&private);
            let current = f.inspect();
            assert_eq!(
                current.state,
                ProviderBootstrapState::FilesystemUninitialized
            );
            let recovery = open(&f, &current.binding).unwrap();
            let after = recovery.recover_databases().unwrap();
            assert_eq!(after.state, ProviderBootstrapState::FilesystemUninitialized);
            assert_eq!(after.private.status, ProviderDatabaseStatus::Uninitialized);
            assert!(after.effects.is_empty());
            assert_eq!(support::witness(&private), zero);
        },
    );
}

#[test]
fn causal_chain_preserves_native_and_sqlite_classification() {
    use std::error::Error;
    let wrapped = Cause::Vfs(layrs_vfs::Error::ControlDirectory {
        kind: layrs_vfs::ControlDirectoryFailure::RenameRefused,
        point: layrs_vfs::ControlDirectoryPublishPoint::PreRename,
        operation: "owned test",
        root: "C:\\root".into(),
        stage: "C:\\stage".into(),
        cause: Box::new(layrs_vfs::Error::Io(std::io::Error::from_raw_os_error(32))),
    });
    assert_eq!(
        error::kind(&wrapped),
        ProviderBootstrapFailureKind::ControlInUse
    );
    assert!(
        wrapped
            .source()
            .unwrap()
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .is_some()
    );
    assert_eq!(
        error::kind(&Cause::Io(std::io::Error::from_raw_os_error(5))),
        ProviderBootstrapFailureKind::ControlUnavailable
    );
    let busy = Cause::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        None,
    ));
    assert_eq!(
        error::kind(&busy),
        ProviderBootstrapFailureKind::DatabaseBusy
    );
}

#[test]
#[ignore = "owned audited Windows/VFS test host; distinct FORMAT sharing and owner lock causes"]
fn format_writer_and_existing_owner_lock_refuse_without_database_effects() {
    child::run(
        concat!(
            module_path!(),
            "::format_writer_and_existing_owner_lock_refuse_without_database_effects"
        ),
        180,
        || {
            use std::os::windows::fs::OpenOptionsExt;
            child::declare();
            let mut f = NativeFixture::create(MountPhase::ControlPublished);
            let inspected = f.inspect();
            drop(f.guard.take());
            let before = witnesses(&f);
            let writer = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(7)
                .open(f.checkout.join(".layrs/FORMAT"))
                .unwrap();
            let error = match open(&f, &inspected.binding) {
                Err(e) => e,
                Ok(_) => panic!("strong FORMAT proof must refuse an existing writer"),
            };
            let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                panic!("typed FORMAT failure expected")
            };
            assert_eq!(failure.kind, ProviderBootstrapFailureKind::ControlInUse);
            assert!(failure.effects.is_empty());
            drop(writer);
            assert_eq!(witnesses(&f), before);
            let recovery = open(&f, &inspected.binding).unwrap();
            let owner = crate::lock::StoreLock::acquire_existing(
                &f.metadata.join("layrs.lock"),
                crate::lock::SessionMode::Exclusive,
            )
            .unwrap();
            let error = recovery.inspect().unwrap_err();
            let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                panic!("typed owner-lock failure expected")
            };
            assert!(matches!(
                failure.cause,
                Cause::Store(crate::StoreError::ReentrantLock(_))
            ));
            assert!(failure.effects.is_empty());
            drop(owner);
            assert_eq!(
                recovery.inspect().unwrap().state,
                ProviderBootstrapState::ControlPublished
            );
            assert_eq!(witnesses(&f), before);
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS test host; static final DATA cannot grant terminal registration proof"]
fn final_terminal_requires_owned_unregistered_guard_and_known_contradictions_refuse() {
    child::run(
        concat!(
            module_path!(),
            "::final_terminal_requires_owned_unregistered_guard_and_known_contradictions_refuse"
        ),
        180,
        || {
            child::declare();
            let mut f = NativeFixture::create(MountPhase::ControlPublished);
            f.terminal(RegistryAtDecision::Initializing);
            let observed = f.inspect();
            assert_eq!(
                observed.state,
                ProviderBootstrapState::TerminalPendingValidation
            );
            assert!(observed.phase.is_none());
            assert!(observed.effects.is_empty());
            let clone = f.guard.as_ref().unwrap().try_clone().unwrap();
            assert_eq!(
                f.inspect().state,
                ProviderBootstrapState::TerminalPendingValidation
            );
            drop(clone);
            drop(f.guard.take());
            let recovery = open(&f, &observed.binding).unwrap();
            let before = witnesses(&f);
            assert_eq!(
                recovery.inspect().unwrap().state,
                ProviderBootstrapState::AbandonIncomplete
            );
            assert_eq!(
                recovery.recover_databases().unwrap().state,
                ProviderBootstrapState::AbandonIncomplete
            );
            assert_eq!(witnesses(&f), before);

            // Deliberately contradictory DATA fixture: fresh registry is initializing
            // while terminal claims an absent decision. It must never gain effects.
            let g = NativeFixture::create(MountPhase::Allocating);
            g.terminal(RegistryAtDecision::Absent);
            let before = witnesses(&g);
            let error = PartialStoreFamily::inspect_provider_bootstrap(
                &g.family,
                &g.allocation.format.identity.worktree_id,
                &g.allocation.transaction_id,
            )
            .unwrap_err();
            let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                panic!("typed terminal contradiction expected")
            };
            assert!(
                failure
                    .cause
                    .to_string()
                    .contains("terminal decided absent")
            );
            assert!(failure.effects.is_empty());
            assert_eq!(witnesses(&g), before);
        },
    );
}

#[test]
fn original_trigger_ownership_survives_all_context_error_paths() {
    let mut context = Context::new("pure causal data test");
    let first = Arc::new(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY_ROLLBACK),
        Some("family original".into()),
    ));
    let second = Arc::new(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY_ROLLBACK),
        Some("private original".into()),
    ));
    context.rollback_triggers = vec![
        ProviderRollbackTrigger {
            database: ProviderDatabaseScope::FamilyCatalog,
            error: Arc::clone(&first),
        },
        ProviderRollbackTrigger {
            database: ProviderDatabaseScope::PrivateMetadata,
            error: Arc::clone(&second),
        },
    ];
    context.effects.push(ProviderRecoveryEffect {
        database: ProviderDatabaseScope::FamilyCatalog,
        state: ProviderRecoveryEffectState::Completed,
    });
    for error in [
        context.error(invalid("later semantic cause"), None, None),
        context.binding_changed(),
        context.error(
            Cause::Profile(super::super::process_profile::ProviderSqliteProfileError::Drift),
            None,
            None,
        ),
    ] {
        let ProjectionStoreError::ProviderBootstrap(failure) = error else {
            panic!("typed failure")
        };
        assert_eq!(failure.effects, context.effects);
        assert_eq!(failure.rollback_triggers.len(), 2);
        assert!(Arc::ptr_eq(&failure.rollback_triggers[0].error, &first));
        assert!(Arc::ptr_eq(&failure.rollback_triggers[1].error, &second));
        assert!(Arc::ptr_eq(
            failure.readonly_rollback.as_ref().unwrap(),
            &second
        ));
        assert_eq!(
            failure.rollback_triggers[0].error().to_string(),
            first.to_string()
        );
    }
    for (scope, explicit, expected) in [
        (Some(ProviderDatabaseScope::FamilyCatalog), None, &first),
        (Some(ProviderDatabaseScope::PrivateMetadata), None, &second),
        (None, None, &second),
        (None, Some(Arc::clone(&first)), &first),
    ] {
        let error = context.error(invalid("scoped causal DATA control"), scope, explicit);
        let ProjectionStoreError::ProviderBootstrap(failure) = error else {
            panic!("typed scoped failure")
        };
        assert_eq!(failure.database, scope);
        assert_eq!(failure.effects, context.effects);
        assert_eq!(failure.rollback_triggers.len(), 2);
        assert_eq!(
            failure.rollback_triggers[0].database,
            ProviderDatabaseScope::FamilyCatalog
        );
        assert_eq!(
            failure.rollback_triggers[1].database,
            ProviderDatabaseScope::PrivateMetadata
        );
        assert!(Arc::ptr_eq(&failure.rollback_triggers[0].error, &first));
        assert!(Arc::ptr_eq(&failure.rollback_triggers[1].error, &second));
        assert!(Arc::ptr_eq(
            failure.readonly_rollback.as_ref().unwrap(),
            expected
        ));
    }
    for (requested, observed, original) in [
        (
            ProviderDatabaseScope::FamilyCatalog,
            ProviderDatabaseScope::PrivateMetadata,
            &second,
        ),
        (
            ProviderDatabaseScope::PrivateMetadata,
            ProviderDatabaseScope::FamilyCatalog,
            &first,
        ),
    ] {
        let mut other = Context::new("only the other DB observed DATA control");
        other.rollback_triggers.push(ProviderRollbackTrigger {
            database: observed,
            error: Arc::clone(original),
        });
        other.effects.push(ProviderRecoveryEffect {
            database: observed,
            state: ProviderRecoveryEffectState::Completed,
        });
        let error = other.error(
            invalid("no trigger for requested scope"),
            Some(requested),
            None,
        );
        let ProjectionStoreError::ProviderBootstrap(failure) = error else {
            panic!("typed missing-scope failure")
        };
        assert_eq!(failure.database, Some(requested));
        assert_eq!(failure.effects, other.effects);
        assert_eq!(failure.rollback_triggers.len(), 1);
        assert_eq!(failure.rollback_triggers[0].database, observed);
        assert!(Arc::ptr_eq(&failure.rollback_triggers[0].error, original));
        assert!(failure.readonly_rollback.is_none());
    }
    for scope in [
        None,
        Some(ProviderDatabaseScope::FamilyCatalog),
        Some(ProviderDatabaseScope::PrivateMetadata),
    ] {
        let empty = Context::new("zero observation").error(invalid("before SQL"), scope, None);
        let ProjectionStoreError::ProviderBootstrap(empty) = empty else {
            panic!()
        };
        assert_eq!(empty.database, scope);
        assert!(empty.effects.is_empty());
        assert!(empty.rollback_triggers.is_empty());
        assert!(empty.readonly_rollback.is_none());
    }

    let unbound = ReadState::<()>::UnboundRollback {
        trigger: Arc::clone(&first),
        cause: Cause::Io(std::io::Error::from_raw_os_error(32)),
    };
    let mut pending = Context::new("unbound actual trigger DATA control");
    pending.remember_hot(&unbound, ProviderDatabaseScope::FamilyCatalog);
    assert!(matches!(
        refuse_known(unbound, ProviderDatabaseScope::FamilyCatalog, &pending).unwrap(),
        ReadState::UnboundRollback { .. }
    ));
    assert!(Arc::ptr_eq(&pending.rollback_triggers[0].error, &first));
    let close = Context::new("close failure before read returns state").error(
        Cause::Cleanup {
            primary: Box::new(Cause::ReadonlyRollback(Arc::clone(&first))),
            close: Box::new(Cause::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                None,
            ))),
        },
        Some(ProviderDatabaseScope::FamilyCatalog),
        None,
    );
    let ProjectionStoreError::ProviderBootstrap(close) = close else {
        panic!("typed cleanup")
    };
    assert_eq!(close.rollback_triggers.len(), 1);
    assert!(Arc::ptr_eq(&close.rollback_triggers[0].error, &first));
    assert_eq!(
        close.kind,
        ProviderBootstrapFailureKind::ObservableCloseFailure
    );
}

#[test]
fn semantic_refusal_does_not_invent_pager_completion() {
    for pager_read in [false, true] {
        let trigger = Arc::new(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY_ROLLBACK),
            Some("original pure-state control".into()),
        ));
        let mut context = Context::new("completion state control");
        context.rollback_triggers.push(ProviderRollbackTrigger {
            database: ProviderDatabaseScope::FamilyCatalog,
            error: Arc::clone(&trigger),
        });
        context.effects.push(ProviderRecoveryEffect {
            database: ProviderDatabaseScope::FamilyCatalog,
            state: ProviderRecoveryEffectState::Uncertain,
        });
        let state = ReadState::<()>::Rejected {
            cause: invalid("proven metadata contradiction"),
            pager_read,
        };
        let error = match accept_completed(
            state,
            ProviderDatabaseScope::FamilyCatalog,
            Arc::clone(&trigger),
            &mut context,
        ) {
            Ok(state) => {
                match refuse_known(state, ProviderDatabaseScope::FamilyCatalog, &context) {
                    Err(e) => e,
                    Ok(_) => panic!("known contradiction cannot be accepted"),
                }
            }
            Err(error) => error,
        };
        let ProjectionStoreError::ProviderBootstrap(error) = error else {
            panic!("typed state refusal")
        };
        assert_eq!(
            error.effects[0].state,
            if pager_read {
                ProviderRecoveryEffectState::Completed
            } else {
                ProviderRecoveryEffectState::Uncertain
            }
        );
        assert!(Arc::ptr_eq(&error.rollback_triggers[0].error, &trigger));
        assert!(
            error
                .cause
                .to_string()
                .contains("proven metadata contradiction")
        );
    }
}
