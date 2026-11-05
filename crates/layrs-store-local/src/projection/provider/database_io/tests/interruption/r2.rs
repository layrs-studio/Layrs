//! R2 actual-interruption orchestration tests. No synthetic hot DTO or journal.
use super::*;
use crate::projection::provider::recovery::{
    ProviderBootstrapCause as Cause, ProviderBootstrapFailure, ProviderBootstrapFailureKind,
    ProviderBootstrapState, ProviderDatabaseStatus,
};
use crate::projection::provider::{
    database_io,
    records::*,
    recovery::test_hooks::{self, Point},
};
use std::io::Write;
fn failure(error: ProjectionStoreError) -> Box<ProviderBootstrapFailure> {
    let ProjectionStoreError::ProviderBootstrap(f) = error else {
        panic!("provider typed failure required")
    };
    f
}
fn triggers(
    f: &ProviderBootstrapFailure,
    scopes: &[Scope],
    expected_shortcut_index: Option<usize>,
) {
    assert_eq!(
        f.rollback_triggers
            .iter()
            .map(|t| t.database)
            .collect::<Vec<_>>(),
        scopes
    );
    for t in &f.rollback_triggers {
        assert!(
            matches!(t.error(),rusqlite::Error::SqliteFailure(code,_) if code.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
        );
    }
    match expected_shortcut_index {
        None => assert!(f.readonly_rollback.is_none()),
        Some(index) => {
            let original = f
                .rollback_triggers
                .get(index)
                .expect("expected shortcut index within the exact trigger vector");
            let shortcut = f
                .readonly_rollback
                .as_ref()
                .expect("expected original rollback shortcut");
            assert!(std::ptr::eq(shortcut.as_ref(), original.error()));
        }
    }
}
fn complete(scope: Scope) -> recovery::ProviderRecoveryEffect {
    recovery::ProviderRecoveryEffect {
        database: scope,
        state: Effect::Completed,
    }
}
fn witness_files(paths: &[PathBuf]) -> Vec<(String, u64, String)> {
    paths.iter().map(|p| support::witness(p)).collect()
}

#[test]
#[ignore = "owned audited Windows/VFS host; real family hot + cold schema9 contradiction, zero provider RW"]
fn known_private_schema_contradiction_stops_family_before_rw() {
    child::run(
        concat!(
            module_path!(),
            "::known_private_schema_contradiction_stops_family_before_rw"
        ),
        180,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            f.terminal(RegistryAtDecision::Initializing);
            let recovery = bound(&f);
            let c = rusqlite::Connection::open(f.metadata.join("projection.sqlite3")).unwrap();
            c.execute_batch("PRAGMA user_version=9").unwrap();
            c.close().unwrap();
            let mut paths = hot(&f, true, "delete");
            paths.push(f.metadata.join("projection.sqlite3"));
            paths.push(f.metadata.join("provider-control/abandoned.json"));
            let before = witness_files(&paths);
            let opens = forward::RW_OPENS.load(Ordering::SeqCst);
            let error = failure(recovery.recover_databases().unwrap_err());
            assert_eq!(error.database, Some(Scope::PrivateMetadata));
            assert!(
                error
                    .cause
                    .to_string()
                    .contains("requires exact private schema8")
            );
            assert!(error.effects.is_empty());
            triggers(&error, &[Scope::FamilyCatalog], None);
            assert_eq!(forward::RW_OPENS.load(Ordering::SeqCst), opens);
            assert_eq!(witness_files(&paths), before);
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; actual family rollback then terminal registry contradiction"]
fn family_completed_preserves_trigger_on_terminal_registry_contradiction() {
    child::run(
        concat!(
            module_path!(),
            "::family_completed_preserves_trigger_on_terminal_registry_contradiction"
        ),
        180,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            f.terminal(RegistryAtDecision::Absent);
            let before = support::witness(&f.family.join("metadata/catalog.sqlite3"));
            hot(&f, true, "delete");
            let pending = f.inspect();
            assert_eq!(
                pending.state,
                ProviderBootstrapState::TerminalPendingValidation
            );
            assert!(pending.phase.is_none());
            let error = failure(bound(&f).recover_databases().unwrap_err());
            assert_eq!(error.effects, vec![complete(Scope::FamilyCatalog)]);
            assert_eq!(error.database, None);
            triggers(&error, &[Scope::FamilyCatalog], Some(0));
            assert!(error.cause.to_string().contains("terminal decided absent"));
            assert_eq!(
                support::witness(&f.family.join("metadata/catalog.sqlite3")),
                before
            );
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; actual private rollback then refreshed terminal mount contradiction"]
fn private_completed_preserves_trigger_on_terminal_mount_contradiction() {
    child::run(
        concat!(
            module_path!(),
            "::private_completed_preserves_trigger_on_terminal_mount_contradiction"
        ),
        180,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            f.terminal(RegistryAtDecision::Initializing);
            let tpath = f.metadata.join("provider-control/abandoned.json");
            let mut t = AbandonedRecord::decode(&fs::read(&tpath).unwrap()).unwrap();
            let PrivateState::InitialSchema8 { mount_digest, .. } =
                &mut t.eligibility.private_state
            else {
                panic!("initialized fixture")
            };
            *mount_digest = format!("blake3:{}", "0".repeat(64));
            t.eligibility_digest = t.eligibility.digest().unwrap();
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&tpath)
                .unwrap();
            file.write_all(&t.encode().unwrap()).unwrap();
            file.sync_all().unwrap();
            drop(file);
            let terminal = support::witness(&tpath);
            let before = support::witness(&f.metadata.join("projection.sqlite3"));
            hot(&f, false, "delete");
            assert_eq!(
                f.inspect().state,
                ProviderBootstrapState::TerminalPendingValidation
            );
            let error = failure(bound(&f).recover_databases().unwrap_err());
            assert_eq!(error.effects, vec![complete(Scope::PrivateMetadata)]);
            assert_eq!(error.database, None);
            triggers(&error, &[Scope::PrivateMetadata], Some(0));
            assert!(
                error
                    .cause
                    .to_string()
                    .contains("terminal initialized mount commitment differs")
            );
            assert_eq!(
                support::witness(&f.metadata.join("projection.sqlite3")),
                before
            );
            assert_eq!(support::witness(&tpath), terminal);
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; actual both-db rollback then concluding bound-receipt interference"]
fn concluding_binding_failure_keeps_two_completed_effects_and_triggers() {
    child::run(
        concat!(
            module_path!(),
            "::concluding_binding_failure_keeps_two_completed_effects_and_triggers"
        ),
        240,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            hot(&f, true, "delete");
            hot(&f, false, "delete");
            let recovery = bound(&f);
            let receipt = f.metadata.join("provider-control/allocation.json");
            let error = test_hooks::with(
                move |point| {
                    if point == Point::BeforeConcludingBoundary {
                        let mut file = fs::OpenOptions::new().append(true).open(&receipt)?;
                        file.write_all(b" ")?;
                        file.sync_all()?;
                    }
                    Ok(())
                },
                || recovery.recover_databases(),
            )
            .unwrap_err();
            let error = failure(error);
            assert_eq!(
                error.effects,
                vec![
                    complete(Scope::FamilyCatalog),
                    complete(Scope::PrivateMetadata)
                ]
            );
            assert_eq!(error.database, None);
            triggers(
                &error,
                &[Scope::FamilyCatalog, Scope::PrivateMetadata],
                Some(1),
            );
            // Appending exceeds the original read bound: this qualifies the
            // concluding receipt-size refusal, before the byte-comparison branch.
            let Cause::Validation(cause) = &error.cause else {
                panic!("typed receipt-size validation required: {:?}", error.cause)
            };
            let ProjectionStoreError::Invalid(message) = cause.as_ref() else {
                panic!("exact receipt-size validation required: {cause:?}")
            };
            assert_eq!(message, "provider receipt exceeds bound");
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; observable child-only VFS drift after actual family completion"]
fn post_family_profile_drift_keeps_prior_completion_and_original_triggers() {
    child::run(
        concat!(
            module_path!(),
            "::post_family_profile_drift_keeps_prior_completion_and_original_triggers"
        ),
        240,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            hot(&f, true, "delete");
            hot(&f, false, "delete");
            let recovery = bound(&f);
            let opens = forward::RW_OPENS.load(Ordering::SeqCst);
            let error = test_hooks::with(
                |point| {
                    if point == Point::AfterFamilyCompletion {
                        unsafe {
                            let v = rusqlite::ffi::sqlite3_vfs_find(c"win32".as_ptr());
                            assert!(!v.is_null());
                            (*v).mxPathname += 1;
                        }
                    }
                    Ok(())
                },
                || recovery.recover_databases(),
            )
            .unwrap_err();
            let error = failure(error);
            assert_eq!(
                error.kind,
                ProviderBootstrapFailureKind::ProcessProfileUnavailable
            );
            assert_eq!(
                error.effects,
                vec![
                    complete(Scope::FamilyCatalog),
                    recovery::ProviderRecoveryEffect {
                        database: Scope::PrivateMetadata,
                        state: Effect::Uncertain
                    }
                ]
            );
            assert_eq!(error.database, Some(Scope::PrivateMetadata));
            triggers(
                &error,
                &[Scope::FamilyCatalog, Scope::PrivateMetadata],
                Some(1),
            );
            assert_eq!(
                forward::RW_OPENS.load(Ordering::SeqCst) - opens,
                1,
                "private gate stops before RW open; potential attempt conservatively uncertain"
            );
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; real API close BUSY after family completion, quarantined until child exit"]
fn private_rw_close_busy_preserves_completed_family_and_stops_probing() {
    child::run(
        concat!(
            module_path!(),
            "::private_rw_close_busy_preserves_completed_family_and_stops_probing"
        ),
        240,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            hot(&f, true, "delete");
            hot(&f, false, "delete");
            let recovery = bound(&f);
            let start = forward::events().len();
            let error = test_hooks::with(
                |point| {
                    if point == Point::AfterFamilyCompletion {
                        database_io::test_hooks::busy_next_writable_close();
                    }
                    Ok(())
                },
                || recovery.recover_databases(),
            )
            .unwrap_err();
            let error = failure(error);
            assert_eq!(
                error.kind,
                ProviderBootstrapFailureKind::ObservableCloseFailure
            );
            assert_eq!(
                error.effects,
                vec![
                    complete(Scope::FamilyCatalog),
                    recovery::ProviderRecoveryEffect {
                        database: Scope::PrivateMetadata,
                        state: Effect::Uncertain
                    }
                ]
            );
            assert_eq!(error.database, Some(Scope::PrivateMetadata));
            triggers(
                &error,
                &[Scope::FamilyCatalog, Scope::PrivateMetadata],
                Some(1),
            );
            let Cause::Cleanup { close, .. } = &error.cause else {
                panic!("separate observable close cause")
            };
            assert!(
                matches!(close.as_ref(),Cause::Sqlite(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::DatabaseBusy)
            );
            let events = forward::events();
            let main: Vec<_> = events[start..]
                .iter()
                .filter(|(op, _, flags)| {
                    op == "open" && flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0
                })
                .collect();
            assert_eq!(
                main.len(),
                5,
                "initial2 + familyRW + familyRO + privateRW; no post-close DB probe"
            );
            assert!(main.last().unwrap().2 & rusqlite::ffi::SQLITE_OPEN_READWRITE != 0);
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; genuine one-page journal + captured live4096 geometry, no fabricated hot evidence"]
fn actual_512_page_1032_journal_initial_4096_refuses_before_rw() {
    child::run(
        concat!(
            module_path!(),
            "::actual_512_page_1032_journal_initial_4096_refuses_before_rw"
        ),
        180,
        || {
            let root = support::owned_root("initial-sector");
            let path = root.join("one-page.sqlite3");
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(
                "PRAGMA page_size=512; PRAGMA journal_mode=DELETE; PRAGMA user_version=1",
            )
            .unwrap();
            c.close().unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().len(),
                512,
                "UNQUALIFIED: expected actual one-page database"
            );
            forward::install(&path);
            forward::initial_sector_4096();
            child::declare();
            let paths = cut(&root, &path, false, "one-page");
            let journal = fs::read(&paths[1]).unwrap();
            assert_eq!(
                journal.len(),
                1032,
                "UNQUALIFIED: actual finite one-record journal extent"
            );
            assert_eq!(u32::from_be_bytes(journal[8..12].try_into().unwrap()), 1);
            assert_eq!(u32::from_be_bytes(journal[20..24].try_into().unwrap()), 512);
            assert_eq!(u32::from_be_bytes(journal[24..28].try_into().unwrap()), 512);
            let before = witness_files(&paths);
            let db = super::super::database(path.clone());
            let opens = forward::RW_OPENS.load(Ordering::SeqCst);
            let database_io::ReadState::Hot(hot) = database_io::read(&db, |_| Ok(())).unwrap()
            else {
                panic!("UNQUALIFIED: exact live RO rollback trigger required before admission")
            };
            assert!(
                matches!(hot.trigger.as_ref(),rusqlite::Error::SqliteFailure(code,_) if code.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
            );
            let error = match database_io::admit(&db, hot) {
                Err(e) => e,
                Ok(_) => panic!("initial geometry must refuse"),
            };
            assert!(matches!(
                error.cause,
                Cause::Journal(
                    crate::projection::provider::journal::Refusal::InitialHeaderDoesNotFit {
                        sector: 4096,
                        bytes: 1032
                    }
                )
            ));
            assert_eq!(forward::RW_OPENS.load(Ordering::SeqCst), opens);
            assert_eq!(witness_files(&paths), before);
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; terminal both-hot remains pending until actual independent recovery"]
fn terminal_both_hot_dispatch_requires_both_validated_databases() {
    child::run(
        concat!(
            module_path!(),
            "::terminal_both_hot_dispatch_requires_both_validated_databases"
        ),
        240,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            f.terminal(RegistryAtDecision::Initializing);
            hot(&f, true, "delete");
            hot(&f, false, "delete");
            let pending = f.inspect();
            assert_eq!(
                pending.state,
                ProviderBootstrapState::TerminalPendingValidation
            );
            assert!(pending.phase.is_none());
            assert_eq!(
                pending.family.status,
                ProviderDatabaseStatus::RollbackRequired
            );
            assert_eq!(
                pending.private.status,
                ProviderDatabaseStatus::RollbackRequired
            );
            assert_eq!(pending.rollback_triggers.len(), 2);
            let result = bound(&f).recover_databases().unwrap();
            assert_eq!(result.state, ProviderBootstrapState::AbandonIncomplete);
            assert!(result.phase.is_none());
            assert_eq!(
                result.effects,
                vec![
                    complete(Scope::FamilyCatalog),
                    complete(Scope::PrivateMetadata)
                ]
            );
            assert_eq!(result.rollback_triggers.len(), 2);
            for t in &result.rollback_triggers {
                assert!(
                    matches!(t.error(),rusqlite::Error::SqliteFailure(code,_) if code.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
                );
            }
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; healthy familyHot/privateValid positive control"]
fn family_hot_private_valid_recovers_one_effect_and_retains_trigger() {
    child::run(
        concat!(
            module_path!(),
            "::family_hot_private_valid_recovers_one_effect_and_retains_trigger"
        ),
        180,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            hot(&f, true, "delete");
            let pending = f.inspect();
            assert_eq!(
                pending.family.status,
                ProviderDatabaseStatus::RollbackRequired
            );
            assert_eq!(pending.private.status, ProviderDatabaseStatus::Validated);
            assert!(pending.phase.is_none());
            let result = bound(&f).recover_databases().unwrap();
            assert_eq!(result.state, ProviderBootstrapState::Allocating);
            assert_eq!(result.effects, vec![complete(Scope::FamilyCatalog)]);
            assert_eq!(result.rollback_triggers.len(), 1);
            assert_eq!(result.rollback_triggers[0].database, Scope::FamilyCatalog);
        },
    );
}

#[test]
#[ignore = "owned audited Windows/VFS host; terminal + real private sidecar sharing refusal is pending, not contradictory"]
fn terminal_unreadable_private_sidecar_stays_pending_without_effects() {
    child::run(
        concat!(
            module_path!(),
            "::terminal_unreadable_private_sidecar_stays_pending_without_effects"
        ),
        180,
        || {
            use std::os::windows::fs::OpenOptionsExt;
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            f.terminal(RegistryAtDecision::Initializing);
            let side = f.metadata.join("projection.sqlite3-journal");
            support::create(&side, &[]);
            let before = support::witness(&side);
            let exclusive = fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&side)
                .unwrap();
            let pending = f.inspect();
            assert_eq!(
                pending.state,
                ProviderBootstrapState::TerminalPendingValidation
            );
            assert!(pending.phase.is_none());
            assert_eq!(pending.private.status, ProviderDatabaseStatus::Unavailable);
            assert!(pending.rollback_triggers.is_empty());
            assert!(
                matches!(pending.private.cause,Some(Cause::Io(ref e)) if e.raw_os_error()==Some(32))
            );
            let opens = forward::RW_OPENS.load(Ordering::SeqCst);
            let result = bound(&f).recover_databases().unwrap();
            assert_eq!(
                result.state,
                ProviderBootstrapState::TerminalPendingValidation
            );
            assert!(result.effects.is_empty());
            assert!(result.rollback_triggers.is_empty());
            assert_eq!(forward::RW_OPENS.load(Ordering::SeqCst), opens);
            drop(exclusive);
            assert_eq!(support::witness(&side), before);
            assert_eq!(
                bound(&f).inspect().unwrap().state,
                ProviderBootstrapState::AbandonIncomplete
            );
            assert_eq!(support::witness(&side), before);
        },
    );
}
