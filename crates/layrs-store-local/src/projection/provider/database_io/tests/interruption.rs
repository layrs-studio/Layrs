//! Real rollback-journal interruption recipes. Root runs each in an isolated
//! owned child. A missing native cut fails UNQUALIFIED; no synthetic journal is
//! substituted and all fixtures/logs are retained.
use super::super::super::process_profile::tests::child;
use super::{
    super::super::{
        records::MountPhase,
        recovery::{
            self, ProviderDatabaseScope as Scope, ProviderRecoveryEffectState as Effect,
            tests::fixture::NativeFixture,
        },
    },
    forward, support,
};
use crate::projection::{PartialStoreFamily, ProjectionStoreError};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

pub(super) fn hot(f: &NativeFixture, family: bool, mode: &str) -> Vec<PathBuf> {
    let target = if family {
        f.family.join("metadata/catalog.sqlite3")
    } else {
        f.metadata.join("projection.sqlite3")
    };
    cut(&f.outer, &target, family, mode)
}
pub(super) fn cut(root: &Path, target: &Path, family: bool, mode: &str) -> Vec<PathBuf> {
    let ack = root.join(format!("ack-{family}-{mode}.txt"));
    let name = "projection::provider::database_io::tests::interruption::owned_interruption_child";
    let owner = fs::read_to_string(root.join("fixture-owner.txt")).unwrap();
    assert!(!owner.is_empty() && owner.len() <= 256);
    let mut process = child::OwnedChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LAYRS_PROVIDER_CUT_TARGET", &target)
            .env("LAYRS_PROVIDER_CUT_ACK", &ack)
            .env("LAYRS_PROVIDER_CUT_MODE", mode)
            .env("LAYRS_PROVIDER_CUT_FAMILY", if family { "1" } else { "0" })
            .env("LAYRS_PROVIDER_CUT_ROOT", root)
            .env("LAYRS_PROVIDER_CUT_OWNER", owner)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .creation_flags(0x08000000)
            .spawn()
            .expect("owned hidden SQLite interruption child"),
    );
    let started = Instant::now();
    let selected = loop {
        if ack.try_exists().unwrap() {
            let bytes = fs::read(&ack).unwrap();
            assert!(!bytes.is_empty() && bytes.len() <= 4096);
            let selected = PathBuf::from(String::from_utf8(bytes).unwrap());
            assert!(
                selected.is_absolute() && selected.starts_with(root),
                "cut path is inside this owned fixture"
            );
            break selected;
        }
        if let Some(status) = process.try_wait().unwrap() {
            panic!(
                "UNQUALIFIED: actual {mode} delete cut not reached; child {status}; fixture {}",
                root.display()
            );
        }
        if started.elapsed() > Duration::from_secs(90) {
            let killed = process.kill();
            let reaped = process.wait();
            panic!(
                "UNQUALIFIED: cut timeout; kill={killed:?},reap={reaped:?}; {}",
                root.display()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let before_kill = process
        .try_wait()
        .expect("observe owned cut child after ACK");
    println!("owned cut handoff: pre_kill_status={before_kill:?}");
    assert!(
        before_kill.is_none(),
        "UNQUALIFIED: owned cut child exited before post-ACK termination"
    );
    process
        .kill()
        .expect("kill only the child launched here at its real delete barrier");
    let reaped = process.wait().expect("reap owned cut child");
    println!("owned cut handoff: reaped_status={reaped:?}");
    assert_eq!(
        reaped.code(),
        Some(1),
        "UNQUALIFIED: owned cut child did not reap the pinned Windows termination status"
    );
    let journal = PathBuf::from(format!("{}-journal", target.to_str().unwrap()));
    assert!(journal.is_file(), "actual rollback journal required");
    let mut files = vec![target.to_path_buf(), journal];
    if mode == "super" {
        assert!(
            selected
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("-mj")
        );
        files.extend([
            root.join("external.sqlite3"),
            root.join("external.sqlite3-journal"),
            selected,
        ]);
    } else {
        assert_eq!(selected, files[1]);
    }
    for file in &files {
        support::witness(file);
    }
    files
}

#[test]
#[ignore = "helper only; launched with exact owned target/ACK by interruption recipes"]
fn owned_interruption_child() {
    assert_eq!(
        std::env::var("LAYRS_PROVIDER_SQLITE_TEST_HOST").as_deref(),
        Ok("audited-layrs-test-host-v1")
    );
    let target =
        PathBuf::from(std::env::var_os("LAYRS_PROVIDER_CUT_TARGET").expect("owned target"));
    let root = PathBuf::from(std::env::var_os("LAYRS_PROVIDER_CUT_ROOT").expect("owned root"));
    let ack = PathBuf::from(std::env::var_os("LAYRS_PROVIDER_CUT_ACK").expect("owned ACK"));
    assert!(
        root.is_absolute() && target.starts_with(&root) && ack.parent() == Some(root.as_path())
    );
    let owner = std::env::var("LAYRS_PROVIDER_CUT_OWNER").expect("owned fixture nonce");
    assert!(!owner.is_empty() && owner.len() <= 256);
    assert_eq!(
        fs::read_to_string(root.join("fixture-owner.txt")).unwrap(),
        owner
    );
    let mode = std::env::var("LAYRS_PROVIDER_CUT_MODE").unwrap();
    let family = std::env::var("LAYRS_PROVIDER_CUT_FAMILY").unwrap() == "1";
    forward::install(&target);
    child::declare();
    if mode == "super" {
        let external = root.join("external.sqlite3");
        assert!(!external.exists());
        let c = rusqlite::Connection::open(&external).unwrap();
        c.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE witness(value TEXT); INSERT INTO witness VALUES('old');").unwrap();
        c.execute("ATTACH DATABASE ?1 AS target", [target.to_str().unwrap()])
            .unwrap();
        c.execute_batch("PRAGMA target.journal_mode=DELETE; PRAGMA target.synchronous=FULL; BEGIN IMMEDIATE; UPDATE witness SET value='new'; UPDATE target.projection_provider_mounts SET digest='blake3:0000000000000000000000000000000000000000000000000000000000000000';").unwrap();
        forward::arm_delete("super-journal", ack);
        c.execute_batch("COMMIT").unwrap();
    } else {
        let c = rusqlite::Connection::open(&target).unwrap();
        c.execute_batch("PRAGMA synchronous=FULL;").unwrap();
        if mode == "wal-delete" {
            assert_eq!(
                c.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                "wal"
            );
            forward::arm_delete(&format!("{}-journal", target.to_str().unwrap()), ack);
            let _: String = c
                .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
                .unwrap();
        } else {
            assert!(matches!(mode.as_str(), "delete" | "one-page"));
            assert_eq!(
                c.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                "delete"
            );
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            c.execute_batch(if mode=="one-page" {"PRAGMA user_version=2"} else if family{"UPDATE quota SET bytes=bytes-1"}else{"UPDATE projection_provider_mounts SET digest='blake3:0000000000000000000000000000000000000000000000000000000000000000'"}).unwrap();
            forward::arm_delete(&format!("{}-journal", target.to_str().unwrap()), ack);
            c.execute_batch("COMMIT").unwrap();
        }
    }
    panic!("UNQUALIFIED: real SQLite delete barrier returned instead of pausing");
}

pub(super) fn bound(f: &NativeFixture) -> recovery::ProviderBootstrapRecovery {
    let inspection = f.inspect();
    PartialStoreFamily::open_provider_bootstrap_recovery(
        &f.family,
        &f.allocation.format.identity.worktree_id,
        &f.allocation.transaction_id,
        &inspection.binding,
    )
    .unwrap()
}

#[test]
#[ignore = "native owned interruption; real finite DELETE journal; retained disposable fixtures"]
fn actual_delete_recovery_restores_committed_private_image() {
    child::run(
        concat!(
            module_path!(),
            "::actual_delete_recovery_restores_committed_private_image"
        ),
        180,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            let before = support::witness(&f.metadata.join("projection.sqlite3"));
            let files = hot(&f, false, "delete");
            let inspection = f.inspect();
            assert_eq!(
                inspection.private.status,
                recovery::ProviderDatabaseStatus::RollbackRequired
            );
            let opens = forward::RW_OPENS.load(Ordering::SeqCst);
            let recovery = bound(&f);
            let result = recovery.recover_databases().unwrap();
            assert_eq!(result.state, recovery::ProviderBootstrapState::Allocating);
            assert_eq!(
                result.effects,
                vec![recovery::ProviderRecoveryEffect {
                    database: Scope::PrivateMetadata,
                    state: Effect::Completed
                }]
            );
            assert_eq!(forward::RW_OPENS.load(Ordering::SeqCst) - opens, 1);
            assert_eq!(support::witness(&files[0]), before);
            assert!(!files[1].exists());
            assert!(recovery.recover_databases().unwrap().effects.is_empty());
        },
    );
}

#[test]
#[ignore = "native owned interruption; actual WAL-to-DELETE preimage and EOF super-journal; no synthetic replacement"]
fn actual_wal_preimage_and_super_journal_refuse_before_any_rw_or_external_access() {
    child::run(
        concat!(
            module_path!(),
            "::actual_wal_preimage_and_super_journal_refuse_before_any_rw_or_external_access"
        ),
        240,
        || {
            // One baseline per process. This first fixture also supplies the fixed
            // statement-trace target; refusal counters cover every main DB RW open.
            let first = NativeFixture::create(MountPhase::Allocating);
            forward::install(&first.metadata.join("projection.sqlite3"));
            child::declare();
            for (index, mode) in ["wal-delete", "super"].into_iter().enumerate() {
                let later;
                let f = if index == 0 {
                    &first
                } else {
                    later = NativeFixture::create(MountPhase::Allocating);
                    &later
                };
                let files = hot(f, false, mode);
                let before: Vec<_> = files.iter().map(|p| support::witness(p)).collect();
                let bytes = fs::read(&files[0]).unwrap();
                assert_eq!(&bytes[18..20], &[1, 1]);
                assert!(!PathBuf::from(format!("{}-wal", files[0].display())).exists());
                assert!(!PathBuf::from(format!("{}-shm", files[0].display())).exists());
                if mode == "super" {
                    let journal = fs::read(&files[1]).unwrap();
                    assert_eq!(
                        &journal[journal.len() - 8..],
                        &[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]
                    );
                }
                let opens = forward::RW_OPENS.load(Ordering::SeqCst);
                let event_start = forward::events().len();
                let recovery = bound(f);
                let error = recovery.recover_databases().unwrap_err();
                let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                    panic!("typed refusal expected")
                };
                assert!(failure.effects.is_empty());
                assert!(
                    matches!(failure.readonly_rollback.as_deref(),Some(rusqlite::Error::SqliteFailure(code,_)) if code.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
                );
                match (mode,failure.cause){
                ("super",recovery::ProviderBootstrapCause::Journal(crate::projection::provider::journal::Refusal::SuperJournalTrailerSignature))=>{},
                ("wal-delete",recovery::ProviderBootstrapCause::Journal(crate::projection::provider::journal::Refusal::PageOneVersions{write:2,read:2,..}))=>{},
                (_,cause)=>panic!("actual {mode} journal did not reach the required refusal: {cause}"),
            }
                assert_eq!(
                    forward::RW_OPENS.load(Ordering::SeqCst),
                    opens,
                    "NO writable open, not merely unchanged bytes"
                );
                assert!(
                    forward::events()[event_start..]
                        .iter()
                        .all(|(_, path, _)| !path.contains("external.sqlite3")),
                    "NO SQLite external database/MJ/child-journal path access"
                );
                assert_eq!(
                    files
                        .iter()
                        .map(|p| support::witness(p))
                        .collect::<Vec<_>>(),
                    before
                );
            }
        },
    );
}

#[test]
#[ignore = "native owned two-database interruption; family completion survives private journal refusal"]
fn family_completed_private_refused_preserves_effect_and_original_trigger() {
    child::run(
        concat!(
            module_path!(),
            "::family_completed_private_refused_preserves_effect_and_original_trigger"
        ),
        240,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            let family_before = support::witness(&f.family.join("metadata/catalog.sqlite3"));
            hot(&f, true, "delete");
            let files = hot(&f, false, "super");
            let before: Vec<_> = files.iter().map(|p| support::witness(p)).collect();
            let inspection = f.inspect();
            assert_eq!(
                inspection.family.status,
                recovery::ProviderDatabaseStatus::RollbackRequired
            );
            assert_eq!(
                inspection.private.status,
                recovery::ProviderDatabaseStatus::RollbackRequired
            );
            let error = bound(&f).recover_databases().unwrap_err();
            let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                panic!("typed recovery failure expected")
            };
            assert_eq!(
                failure.effects,
                vec![recovery::ProviderRecoveryEffect {
                    database: Scope::FamilyCatalog,
                    state: Effect::Completed
                }]
            );
            assert!(matches!(
                failure.cause,
                recovery::ProviderBootstrapCause::Journal(
                    crate::projection::provider::journal::Refusal::SuperJournalTrailerSignature
                )
            ));
            assert!(failure.readonly_rollback.is_some());
            assert_eq!(
                support::witness(&f.family.join("metadata/catalog.sqlite3")),
                family_before
            );
            assert_eq!(
                files
                    .iter()
                    .map(|p| support::witness(p))
                    .collect::<Vec<_>>(),
                before
            );
        },
    );
}

#[test]
#[ignore = "native owned interruption; post-open geometry mismatch must execute zero RW SQL"]
fn geometry_mismatch_after_one_bare_rw_open_is_uncertain_without_sql() {
    child::run(
        concat!(
            module_path!(),
            "::geometry_mismatch_after_one_bare_rw_open_is_uncertain_without_sql"
        ),
        180,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            let files = hot(&f, false, "delete");
            let before: Vec<_> = files.iter().map(|p| support::witness(p)).collect();
            let recovery = bound(&f);
            let opens = forward::RW_OPENS.load(Ordering::SeqCst);
            let statements = forward::WATCHED_RW_SQL.load(Ordering::SeqCst);
            forward::drift_geometry();
            let error = recovery.recover_databases().unwrap_err();
            let ProjectionStoreError::ProviderBootstrap(failure) = error else {
                panic!("typed geometry refusal expected")
            };
            assert!(
                failure
                    .cause
                    .to_string()
                    .contains("replay geometry differs before SQL")
            );
            assert_eq!(
                failure.effects,
                vec![recovery::ProviderRecoveryEffect {
                    database: Scope::PrivateMetadata,
                    state: Effect::Uncertain
                }]
            );
            assert_eq!(forward::RW_OPENS.load(Ordering::SeqCst) - opens, 1);
            assert_eq!(forward::WATCHED_RW_SQL.load(Ordering::SeqCst), statements);
            assert_eq!(
                files
                    .iter()
                    .map(|p| support::witness(p))
                    .collect::<Vec<_>>(),
                before
            );
        },
    );
}

#[test]
#[ignore = "owned forwarding VFS; demonstrates SQLite suppresses native xClose errors after actual close"]
fn native_xclose_error_suppression_is_not_observable_close_confirmation() {
    child::run(
        concat!(
            module_path!(),
            "::native_xclose_error_suppression_is_not_observable_close_confirmation"
        ),
        120,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            let bare =
                super::super::BareConnection::open(&f.metadata.join("projection.sqlite3"), false)
                    .unwrap();
            forward::inject_reported_close_error();
            bare.finish(Ok(())).unwrap();
            assert_eq!(
                forward::SUPPRESSED_CLOSES.load(Ordering::SeqCst),
                1,
                "real Win32 close ran, injected native return was suppressed by SQLite"
            );
            assert!(
                super::super::super::process_profile::gate().is_ok(),
                "no claim of detecting unobservable native-close failures"
            );
        },
    );
}

#[test]
#[ignore = "native owned independent family/private hot journals; exact family-first recovery order"]
fn both_hot_delete_journals_recover_in_family_then_private_order() {
    child::run(
        concat!(
            module_path!(),
            "::both_hot_delete_journals_recover_in_family_then_private_order"
        ),
        240,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            let family_before = support::witness(&f.family.join("metadata/catalog.sqlite3"));
            let private_before = support::witness(&f.metadata.join("projection.sqlite3"));
            hot(&f, true, "delete");
            hot(&f, false, "delete");
            let inspection = f.inspect();
            assert_eq!(
                inspection.family.status,
                recovery::ProviderDatabaseStatus::RollbackRequired
            );
            assert_eq!(
                inspection.private.status,
                recovery::ProviderDatabaseStatus::RollbackRequired
            );
            assert!(inspection.phase.is_none());
            let result = bound(&f).recover_databases().unwrap();
            assert_eq!(
                result.effects,
                vec![
                    recovery::ProviderRecoveryEffect {
                        database: Scope::FamilyCatalog,
                        state: Effect::Completed
                    },
                    recovery::ProviderRecoveryEffect {
                        database: Scope::PrivateMetadata,
                        state: Effect::Completed
                    }
                ]
            );
            assert_eq!(
                support::witness(&f.family.join("metadata/catalog.sqlite3")),
                family_before
            );
            assert_eq!(
                support::witness(&f.metadata.join("projection.sqlite3")),
                private_before
            );
        },
    );
}

#[test]
#[ignore = "native owned rollback restoring unsupported committed metadata; completion is not schema acceptance"]
fn completed_rollback_retains_unsupported_schema_cause_without_phase() {
    child::run(
        concat!(
            module_path!(),
            "::completed_rollback_retains_unsupported_schema_cause_without_phase"
        ),
        180,
        || {
            let f = NativeFixture::create(MountPhase::Allocating);
            forward::install(&f.metadata.join("projection.sqlite3"));
            child::declare();
            let c = rusqlite::Connection::open(f.metadata.join("projection.sqlite3")).unwrap();
            c.execute_batch("PRAGMA user_version=9").unwrap();
            c.close().unwrap();
            let before = support::witness(&f.metadata.join("projection.sqlite3"));
            hot(&f, false, "delete");
            let error = bound(&f).recover_databases().unwrap_err();
            let ProjectionStoreError::ProviderBootstrap(result) = error else {
                panic!("typed restored-schema failure")
            };
            assert_eq!(
                result.effects,
                vec![recovery::ProviderRecoveryEffect {
                    database: Scope::PrivateMetadata,
                    state: Effect::Completed
                }]
            );
            assert_eq!(result.rollback_triggers.len(), 1);
            assert!(result.readonly_rollback.is_some());
            assert!(
                result
                    .cause
                    .to_string()
                    .contains("requires exact private schema8")
            );
            assert_eq!(
                support::witness(&f.metadata.join("projection.sqlite3")),
                before
            );
        },
    );
}

mod r2;
