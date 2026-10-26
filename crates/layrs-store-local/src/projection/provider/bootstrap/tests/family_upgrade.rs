//! Owned native source/family recipes; ROOT releases each pinned test host.
use super::*;
use crate::projection::provider::{
    database_io::{self, WriterCloseObserver},
    family_upgrade::upgrade_source_family,
    recovery::error::{invalid, ProviderBootstrapCause as Cause},
};
use crate::projection::shared::schema_v2::MigrationEffect;
use crate::projection::{ProjectionStoreError, SharedQuota};
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Close {
    calls: usize,
    refuse_acknowledgement: bool,
}
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(succeeded, "owned migration writer must close successfully");
        self.calls += 1;
        self.refuse_acknowledgement
            .then(|| invalid("family upgrade close acknowledgement withheld"))
    }
}

fn disconnected_source() -> (Fixture, AllocationRecord, ProviderBootstrapBinding) {
    let (fixture, holder, allocation, _, binding) = capture_family::active_holder_with_private();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
    session.connect().unwrap();
    session.disconnect().unwrap();
    drop(session); // Release the callback source and its shared lifecycle gate.
    (fixture, allocation, binding)
}

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned physical family-upgrade recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare();
                hooks::reset();
                $body
            });
        }
    };
}

owned!(
    quiescent_family_upgrade_preserves_bindings_and_reopens_active,
    {
        let (fixture, allocation, binding) = disconnected_source();
        let catalog = fixture.family.root().join("metadata/catalog.sqlite3");
        let private = fixture.metadata.join("projection.sqlite3");
        let before_catalog = database_io::tests::support::witness(&catalog);
        let before_private = database_io::tests::support::witness(&private);
        let marker = fs::read(fixture.family.root().join("FORMAT")).unwrap();
        let mut close = Close::default();
        assert_eq!(
            upgrade_source_family(
                fixture.family.root(),
                &fixture.worktree,
                &allocation.transaction_id,
                &mut close,
                &Cancellation::default(),
            )
            .unwrap(),
            MigrationEffect::Migrated,
        );
        assert_eq!(close.calls, 1);
        assert_eq!(
            database_io::tests::support::witness(&catalog).0,
            before_catalog.0,
            "catalog inode must be retained",
        );
        assert_eq!(
            database_io::tests::support::witness(&private),
            before_private
        );
        assert_eq!(
            fs::read(fixture.family.root().join("FORMAT")).unwrap(),
            marker
        );
        let reopened = PartialStoreFamily::open_provider_active(
            fixture.family.root(),
            &fixture.worktree,
            &binding.transaction_id,
            &binding,
        )
        .unwrap();
        reopened.revalidate().unwrap();
        drop(reopened);
        let after = database_io::tests::support::witness(&catalog);
        assert_eq!(
            upgrade_source_family(
                fixture.family.root(),
                &fixture.worktree,
                &allocation.transaction_id,
                &mut close,
                &Cancellation::default(),
            )
            .unwrap(),
            MigrationEffect::AlreadyV2,
        );
        assert_eq!(close.calls, 1, "exact replay must open no writer");
        assert_eq!(database_io::tests::support::witness(&catalog), after);
    }
);

owned!(active_session_blocks_family_upgrade_before_any_writer, {
    let (fixture, holder, allocation, _, _) = capture_family::active_holder_with_private();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
    session.connect().unwrap();
    let catalog = fixture.family.root().join("metadata/catalog.sqlite3");
    let private = fixture.metadata.join("projection.sqlite3");
    let before_catalog = database_io::tests::support::witness(&catalog);
    let before_private = database_io::tests::support::witness(&private);
    let mut close = Close::default();
    let error = upgrade_source_family(
        fixture.family.root(),
        &fixture.worktree,
        &allocation.transaction_id,
        &mut close,
        &Cancellation::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        Cause::Store(crate::StoreError::ReentrantLock(_))
            | Cause::Store(crate::StoreError::LockBusy(_))
    ));
    assert_eq!(close.calls, 0);
    assert_eq!(
        database_io::tests::support::witness(&catalog),
        before_catalog
    );
    assert_eq!(
        database_io::tests::support::witness(&private),
        before_private
    );
    session.disconnect().unwrap();
});

owned!(
    family_upgrade_acknowledgement_refusal_retries_without_writer,
    {
        let (fixture, allocation, binding) = disconnected_source();
        let private = fixture.metadata.join("projection.sqlite3");
        let before_private = database_io::tests::support::witness(&private);
        let mut refusal = Close {
            refuse_acknowledgement: true,
            ..Close::default()
        };
        let error = upgrade_source_family(
            fixture.family.root(),
            &fixture.worktree,
            &allocation.transaction_id,
            &mut refusal,
            &Cancellation::default(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("family upgrade close acknowledgement withheld"));
        assert_eq!(refusal.calls, 1);
        let mut retry = Close::default();
        assert_eq!(
            upgrade_source_family(
                fixture.family.root(),
                &fixture.worktree,
                &allocation.transaction_id,
                &mut retry,
                &Cancellation::default(),
            )
            .unwrap(),
            MigrationEffect::AlreadyV2,
        );
        assert_eq!(retry.calls, 0);
        assert_eq!(
            database_io::tests::support::witness(&private),
            before_private
        );
        let reopened = PartialStoreFamily::open_provider_active(
            fixture.family.root(),
            &fixture.worktree,
            &binding.transaction_id,
            &binding,
        )
        .unwrap();
        reopened.revalidate().unwrap();
    }
);

fn leave_family_journal_hot(fixture: &Fixture, database: &Path) -> PathBuf {
    let ack = fixture.outer.join("hot-private-ready.txt");
    assert!(!ack.exists());
    let owner = database_io::tests::support::witness(&fixture.outer.join("fixture-owner.txt"));
    let root_identity = crate::secure_fs::PinnedDirectory::open(&fixture.outer)
        .unwrap()
        .identity_token();
    // Reuse the existing audited native cut. Its "control_published" label
    // selects the family-only uncommitted worktree-state update; this recipe's
    // source is already Active and the helper never opens its private DB.
    let target = "projection::provider::bootstrap::tests::activation_maintenance::private_hot_journal_helper";
    let mut helper = child::OwnedChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                target,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LAYRS_PROVIDER_MAINTENANCE_HOT_DB", database)
            .env("LAYRS_PROVIDER_MAINTENANCE_HOT_ACK", &ack)
            .env("LAYRS_PROVIDER_MAINTENANCE_OWNER_DIGEST", &owner.2)
            .env("LAYRS_PROVIDER_MAINTENANCE_ROOT_IDENTITY", &root_identity)
            .env("LAYRS_PROVIDER_MAINTENANCE_HOT_PHASE", "control_published")
            .env("LAYRS_PROVIDER_MAINTENANCE_HOT_KIND", "family")
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("launch exact owned family hot-journal helper"),
    );
    let started = Instant::now();
    let expected_ack = b"uncommitted family write";
    loop {
        if ack.try_exists().unwrap() {
            assert!(fs::metadata(&ack).unwrap().len() <= expected_ack.len() as u64);
            let observed = fs::read(&ack).unwrap();
            assert!(expected_ack.starts_with(&observed));
            if observed == expected_ack {
                break;
            }
        }
        assert!(
            helper.try_wait().unwrap().is_none(),
            "owned family helper exited before ACK"
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "owned family helper did not reach its native cut"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(helper.try_wait().unwrap().is_none());
    helper
        .kill()
        .expect("terminate only this owned helper handle");
    assert!(!helper
        .wait()
        .expect("reap exact owned family helper")
        .success());
    let journal = PathBuf::from(format!("{}-journal", database.display()));
    assert!(fs::metadata(&journal).unwrap().len() > 28);
    let probe =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let error = probe
        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap_err();
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(code, _) if code.extended_code == ffi::SQLITE_READONLY_ROLLBACK),
        "the retained journal must require an actual rollback"
    );
    probe.close().unwrap();
    journal
}

fn assert_ordinary_readonly_rollback(error: ProjectionStoreError) {
    assert!(
        matches!(error, ProjectionStoreError::Database(rusqlite::Error::SqliteFailure(code, _)) if code.extended_code == ffi::SQLITE_READONLY_ROLLBACK),
        "ordinary admission must preserve the read-only rollback refusal"
    );
}

owned!(ordinary_hot_family_v1_and_v2_exact_stage_never_recovers, {
    for version_two in [false, true] {
        let (fixture, allocation, _) = disconnected_source();
        let catalog = fixture.family.root().join("metadata/catalog.sqlite3");
        if version_two {
            assert_eq!(
                upgrade_source_family(
                    fixture.family.root(),
                    &fixture.worktree,
                    &allocation.transaction_id,
                    &mut Close::default(),
                    &Cancellation::default(),
                )
                .unwrap(),
                MigrationEffect::Migrated,
            );
            // Establish a real, committed SQL-v2 stage before creating the hot
            // journal. Ordinary code must not recover first and inspect later.
            let family = crate::projection::shared::provider_access::FamilyAccess::open(
                fixture.family.root(),
            )
            .unwrap();
            let _locks = family.locks(&fixture.worktree, true).unwrap();
            let c = rusqlite::Connection::open_with_flags(
                &catalog,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                    .unwrap(),
                2
            );
            c.execute(
                "INSERT INTO reservations VALUES('ordinary-hot-stage',?1,'owned-stage',1024)",
                [fixture.worktree.as_str()],
            )
            .unwrap();
            let manifest = format!("blake3:{}", "a".repeat(64));
            let file = format!("blake3:{}", "b".repeat(64));
            c.execute(
                "INSERT INTO exact_installations VALUES('ordinary-hot-stage',?1,?2,0,?3,NULL,0)",
                rusqlite::params![manifest, file, b"owned-exact-intent".as_slice()],
            )
            .unwrap();
            assert!(!c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                    [],
                    |row| { row.get::<_, bool>(0) }
                )
                .unwrap());
            c.execute_batch("COMMIT").unwrap();
            c.close().unwrap();
        }
        let journal = leave_family_journal_hot(&fixture, &catalog);
        let paths = [
            catalog,
            journal,
            fixture.metadata.join("projection.sqlite3"),
            fixture.family.root().join("FORMAT"),
        ];
        let before: Vec<_> = paths
            .iter()
            .map(|path| database_io::tests::support::witness(path))
            .collect();
        let unchanged = || {
            let after: Vec<_> = paths
                .iter()
                .map(|path| database_io::tests::support::witness(path))
                .collect();
            assert_eq!(
                after, before,
                "refused ordinary operation changed retained evidence"
            );
        };
        assert_ordinary_readonly_rollback(fixture.family.usage().unwrap_err());
        unchanged();
        assert_ordinary_readonly_rollback(
            PartialStoreFamily::open(fixture.family.root())
                .err()
                .expect("ordinary open must refuse the retained hot journal"),
        );
        unchanged();
        assert_ordinary_readonly_rollback(
            fixture
                .family
                .set_quota(SharedQuota {
                    bytes: 128 * 1024 * 1024,
                })
                .unwrap_err(),
        );
        unchanged();
        // Deliberately no writable reopen, rollback, recovery or cleanup follows.
    }
});
