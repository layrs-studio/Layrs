//! Owned native regression: family GC must not recover a provider owner's
//! private hot journal before it can prove that owner is ordinary.
use super::super::preparation::{ProviderActivationOutcome, ProviderPreparationOutcome};
use super::*;
use crate::projection::{ProjectionRecoveryReason, ProjectionStore};
use std::{
    fs,
    io::Read,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host maintenance recipe"]
fn active_private_hot_journal_blocks_family_gc_without_rollback() {
    child::run(
        concat!(
            module_path!(),
            "::active_private_hot_journal_blocks_family_gc_without_rollback"
        ),
        240,
        || {
            child::declare();
            let f = Fixture::new();
            let published = f.published();
            let prepared = match published.prepare_provider([0x72; 16]) {
                ProviderPreparationOutcome::Prepared(value) => value,
                ProviderPreparationOutcome::Withheld(value) => {
                    panic!("preparation withheld: {:?}", value.failure())
                }
            };
            let active = match prepared.activate() {
                ProviderActivationOutcome::Active(value) => value,
                ProviderActivationOutcome::Withheld(value) => {
                    panic!("activation withheld: {:?}", value.failure())
                }
            };
            assert_eq!(active.root(), f.checkout.as_path());
            drop(active); // GC observes a ready owner with no live provider holder.
            let db = f.metadata.join("projection.sqlite3");
            let ack = f.outer.join("hot-private-ready.txt");
            let owner = crate::projection::provider::database_io::tests::support::witness(
                &f.outer.join("fixture-owner.txt"),
            );
            let root_identity = crate::secure_fs::PinnedDirectory::open(&f.outer)
                .unwrap()
                .identity_token();
            let target = concat!(module_path!(), "::private_hot_journal_helper");
            let target = target.strip_prefix("layrs_store_local::").unwrap_or(target);
            let mut helper = child::OwnedChild::new(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        target,
                        "--ignored",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_DB", &db)
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_ACK", &ack)
                    .env("LAYRS_PROVIDER_MAINTENANCE_OWNER_DIGEST", &owner.2)
                    .env("LAYRS_PROVIDER_MAINTENANCE_ROOT_IDENTITY", &root_identity)
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_PHASE", "active")
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_KIND", "private")
                    .creation_flags(0x08000000)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("exact owned hot-journal helper"),
            );
            let started = Instant::now();
            while !ack.exists() {
                assert!(
                    helper.try_wait().unwrap().is_none(),
                    "hot helper exited before ACK"
                );
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "hot helper did not ACK"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(fs::read(&ack).unwrap(), b"uncommitted private write");
            helper.kill().expect("kill only exact owned helper handle");
            assert!(!helper.wait().expect("reap exact owned helper").success());
            let journal = PathBuf::from(format!("{}-journal", db.display()));
            assert!(fs::metadata(&journal).unwrap().len() > 0);
            let read = rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let error = read
                .query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
                    r.get::<_, u64>(0)
                })
                .unwrap_err();
            assert!(
                matches!(error, rusqlite::Error::SqliteFailure(code, _) if code.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
            );
            read.close().unwrap();
            let before = f.witness();
            let journal_before =
                crate::projection::provider::database_io::tests::support::witness(&journal);
            assert!(f.family.reconcile_retention().is_err());
            assert!(f.family.evict_unretained().is_err());
            assert_eq!(f.witness(), before);
            assert_eq!(
                crate::projection::provider::database_io::tests::support::witness(&journal),
                journal_before
            );
            assert!(fs::metadata(&f.checkout).unwrap().is_dir());
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host recovery recipe"]
fn control_published_private_hot_refuses_explicit_recovery_without_rollback() {
    child::run(
        concat!(
            module_path!(),
            "::control_published_private_hot_refuses_explicit_recovery_without_rollback"
        ),
        240,
        || {
            child::declare();
            let f = Fixture::new();
            let published = f.published();
            assert_eq!(published.root(), f.checkout.as_path());
            drop(published);
            let db = f.metadata.join("projection.sqlite3");
            let ack = f.outer.join("hot-private-ready.txt");
            let owner = crate::projection::provider::database_io::tests::support::witness(
                &f.outer.join("fixture-owner.txt"),
            );
            let root_identity = crate::secure_fs::PinnedDirectory::open(&f.outer)
                .unwrap()
                .identity_token();
            let target = concat!(module_path!(), "::private_hot_journal_helper");
            let target = target.strip_prefix("layrs_store_local::").unwrap_or(target);
            let mut helper = child::OwnedChild::new(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        target,
                        "--ignored",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_DB", &db)
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_ACK", &ack)
                    .env("LAYRS_PROVIDER_MAINTENANCE_OWNER_DIGEST", &owner.2)
                    .env("LAYRS_PROVIDER_MAINTENANCE_ROOT_IDENTITY", &root_identity)
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_PHASE", "control_published")
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_KIND", "private")
                    .creation_flags(0x08000000)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("exact owned hot-journal helper"),
            );
            let started = Instant::now();
            while !ack.exists() {
                assert!(
                    helper.try_wait().unwrap().is_none(),
                    "hot helper exited before ACK"
                );
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "hot helper did not ACK"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(fs::read(&ack).unwrap(), b"uncommitted private write");
            helper.kill().expect("kill only exact owned helper handle");
            assert!(!helper.wait().expect("reap exact owned helper").success());
            let journal = PathBuf::from(format!("{}-journal", db.display()));
            let before = f.witness();
            let journal_before =
                crate::projection::provider::database_io::tests::support::witness(&journal);
            let inspection = ProjectionStore::inspect_recovery(&f.checkout).unwrap();
            assert!(
                inspection
                    .recovery
                    .contains(&ProjectionRecoveryReason::WorktreeRollback)
            );
            assert!(ProjectionStore::recover_databases(&f.checkout, &inspection.binding).is_err());
            assert_eq!(f.witness(), before);
            assert_eq!(
                crate::projection::provider::database_io::tests::support::witness(&journal),
                journal_before
            );
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host recovery recipe"]
fn control_published_clean_private_hot_family_refuses_explicit_recovery() {
    child::run(
        concat!(
            module_path!(),
            "::control_published_clean_private_hot_family_refuses_explicit_recovery"
        ),
        240,
        || {
            child::declare();
            let f = Fixture::new();
            let published = f.published();
            assert_eq!(published.root(), f.checkout.as_path());
            drop(published);
            let db = f.family.root().join("metadata/catalog.sqlite3");
            let ack = f.outer.join("hot-private-ready.txt");
            let owner = crate::projection::provider::database_io::tests::support::witness(
                &f.outer.join("fixture-owner.txt"),
            );
            let root_identity = crate::secure_fs::PinnedDirectory::open(&f.outer)
                .unwrap()
                .identity_token();
            let target = concat!(module_path!(), "::private_hot_journal_helper");
            let target = target.strip_prefix("layrs_store_local::").unwrap_or(target);
            let mut helper = child::OwnedChild::new(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        target,
                        "--ignored",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_DB", &db)
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_ACK", &ack)
                    .env("LAYRS_PROVIDER_MAINTENANCE_OWNER_DIGEST", &owner.2)
                    .env("LAYRS_PROVIDER_MAINTENANCE_ROOT_IDENTITY", &root_identity)
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_PHASE", "control_published")
                    .env("LAYRS_PROVIDER_MAINTENANCE_HOT_KIND", "family")
                    .creation_flags(0x08000000)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("exact owned hot-journal helper"),
            );
            let started = Instant::now();
            while !ack.exists() {
                assert!(
                    helper.try_wait().unwrap().is_none(),
                    "hot helper exited before ACK"
                );
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "hot helper did not ACK"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(fs::read(&ack).unwrap(), b"uncommitted family write");
            helper.kill().expect("kill only exact owned helper handle");
            assert!(!helper.wait().expect("reap exact owned helper").success());
            let journal = PathBuf::from(format!("{}-journal", db.display()));
            let before = f.witness();
            let journal_before =
                crate::projection::provider::database_io::tests::support::witness(&journal);
            let inspection = ProjectionStore::inspect_recovery(&f.checkout).unwrap();
            assert_eq!(
                inspection.recovery,
                vec![ProjectionRecoveryReason::FamilyRollback]
            );
            assert!(ProjectionStore::recover_databases(&f.checkout, &inspection.binding).is_err());
            assert_eq!(f.witness(), before);
            assert_eq!(
                crate::projection::provider::database_io::tests::support::witness(&journal),
                journal_before
            );
        },
    );
}

#[test]
#[ignore = "PARENT-ONLY private hot-journal helper; requires exact fresh owned fixture paths"]
fn private_hot_journal_helper() {
    assert_eq!(
        std::env::var("LAYRS_PROVIDER_SQLITE_TEST_HOST").as_deref(),
        Ok("audited-layrs-test-host-v1")
    );
    child::declare();
    let parent = PathBuf::from(std::env::var_os("LAYRS_VFS_TEST_PARENT").expect("fresh parent"));
    let db = PathBuf::from(
        std::env::var_os("LAYRS_PROVIDER_MAINTENANCE_HOT_DB").expect("owned private DB"),
    );
    let ack =
        PathBuf::from(std::env::var_os("LAYRS_PROVIDER_MAINTENANCE_HOT_ACK").expect("owned ACK"));
    assert!(parent.is_absolute() && db.is_absolute() && ack.is_absolute());
    assert!(db.starts_with(&parent) && ack.starts_with(&parent));
    assert!(
        !db.components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    );
    assert!(
        !ack.components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    );
    let kind = std::env::var("LAYRS_PROVIDER_MAINTENANCE_HOT_KIND").unwrap();
    assert!(matches!(kind.as_str(), "private" | "family"));
    assert_eq!(
        db.file_name().unwrap(),
        if kind == "private" {
            "projection.sqlite3"
        } else {
            "catalog.sqlite3"
        }
    );
    assert_eq!(ack.file_name().unwrap(), "hot-private-ready.txt");
    assert_eq!(db.parent().unwrap().file_name().unwrap(), "metadata");
    let outer = ack.parent().unwrap();
    assert!(db.starts_with(outer));
    if kind == "family" {
        assert_eq!(db, outer.join("f/metadata/catalog.sqlite3"));
    }
    let root_identity = std::env::var("LAYRS_PROVIDER_MAINTENANCE_ROOT_IDENTITY").unwrap();
    assert!(!root_identity.is_empty() && root_identity.len() <= 512);
    assert_eq!(
        crate::secure_fs::PinnedDirectory::open(outer)
            .unwrap()
            .identity_token(),
        root_identity
    );
    let owner_digest = std::env::var("LAYRS_PROVIDER_MAINTENANCE_OWNER_DIGEST").unwrap();
    assert!(owner_digest.len() == 64 && owner_digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        crate::projection::provider::database_io::tests::support::witness(
            &outer.join("fixture-owner.txt")
        )
        .2,
        owner_digest
    );
    assert!(!ack.exists());
    let c = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .unwrap();
    assert_eq!(
        c.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    c.execute_batch("PRAGMA synchronous=FULL; BEGIN IMMEDIATE")
        .unwrap();
    let phase = std::env::var("LAYRS_PROVIDER_MAINTENANCE_HOT_PHASE").unwrap();
    assert!(matches!(phase.as_str(), "active" | "control_published"));
    let changed = if kind == "private" {
        c.execute(
            "UPDATE projection_provider_mounts SET digest=?1 WHERE phase=?2",
            (format!("blake3:{}", "0".repeat(64)), phase),
        )
        .unwrap()
    } else {
        assert_eq!(phase, "control_published");
        c.execute(
            "UPDATE worktrees SET state=CASE state WHEN 'ready' THEN 'initializing' ELSE 'ready' END WHERE id=(SELECT id FROM worktrees LIMIT 1)",
            [],
        )
        .unwrap()
    };
    assert_eq!(changed, 1);
    // Force a real nonzero rollback journal and an uncommitted database page
    // before acknowledging the parent. No COMMIT or SQLite close follows.
    let flushed = unsafe { rusqlite::ffi::sqlite3_db_cacheflush(c.handle()) };
    assert_eq!(flushed, rusqlite::ffi::SQLITE_OK);
    let journal = PathBuf::from(format!("{}-journal", db.display()));
    let mut journal_file = crate::layout::open_read_no_follow(&journal).unwrap();
    let journal_len = journal_file.metadata().unwrap().len();
    assert!((28..=16 * 1024 * 1024).contains(&journal_len));
    let mut header = [0u8; 8];
    journal_file.read_exact(&mut header).unwrap();
    assert!(header.iter().any(|byte| *byte != 0));
    crate::projection::provider::database_io::tests::support::create(
        &ack,
        if kind == "private" {
            b"uncommitted private write".as_slice()
        } else {
            b"uncommitted family write".as_slice()
        },
    );
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
