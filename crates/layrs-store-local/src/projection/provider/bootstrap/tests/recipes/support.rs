//! Shared fault, phase and retention helpers of the bootstrap recipes; the test declarations stay in the parent.
use super::*;

pub(super) fn invalid(cause: &Cause, message: &str) {
    match cause {
        Cause::Validation(e) => match e.as_ref() {
            ProjectionStoreError::Invalid(s) => assert_eq!(s, message),
            e => panic!("wrong validation error {e:?}"),
        },
        e => panic!("wrong cause {e:?}"),
    }
}

pub(super) fn ineligible_inspection_refusal(
    outer: &ProviderBootstrapFailure,
    operation: &str,
    database: Option<ProviderDatabaseScope>,
    message: &str,
) {
    use recovery::ProviderBootstrapFailureKind::NeedsRepair;
    assert_eq!(outer.kind, NeedsRepair);
    assert_eq!(outer.operation, operation);
    assert_eq!(outer.database, database);
    let inner = match &outer.cause {
        Cause::Validation(error) => match error.as_ref() {
            ProjectionStoreError::ProviderBootstrap(inner) => inner,
            error => panic!("expected nested provider inspection failure: {error:?}"),
        },
        cause => panic!("expected exact inspection validation wrapper: {cause:?}"),
    };
    assert_eq!(inner.operation, "reject ineligible writer inspection");
    assert_eq!(inner.kind, NeedsRepair);
    assert_eq!(inner.database, database);
    assert!(inner.write_report.is_none());
    invalid(&inner.cause, message);
}

pub(super) fn synthetic() -> std::io::Error {
    std::io::Error::other("synthetic exact writer boundary")
}

pub(super) fn stop(step: Step, after: bool) {
    hooks::observe(move |e| {
        if e.step == step && e.after == after {
            Err(synthetic())
        } else {
            Ok(())
        }
    });
}

pub(super) fn no_later_writes(f: &ProviderBootstrapFailure, forbidden: impl Fn(Step) -> bool) {
    assert!(
        !f.write_report
            .as_ref()
            .unwrap()
            .effects
            .iter()
            .any(|e| forbidden(e.step))
    );
}

pub(super) fn raw_sql(path: &Path, body: impl FnOnce(&rusqlite::Connection)) {
    let c =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .unwrap();
    body(&c);
    c.close().unwrap();
}

pub(super) fn phase(path: &Path) -> String {
    let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let p = c
        .query_row("SELECT phase FROM projection_provider_mounts", [], |r| {
            r.get(0)
        })
        .unwrap();
    c.close().unwrap();
    p
}

pub(super) fn stages(f: &Fixture) -> AllocationRecord {
    AllocationRecord::decode(&fs::read(f.receipt(Receipt::Allocation)).unwrap()).unwrap()
}

pub(super) fn prepared(f: &Fixture) -> PreparedRecord {
    PreparedRecord::decode(&fs::read(f.receipt(Receipt::Prepared)).unwrap()).unwrap()
}

pub(super) fn stop_at_prepared(f: &Fixture) {
    hooks::stop_publish(layrs_vfs::ControlDirectoryPublishPoint::PreRename);
    let e = failure(f.begin());
    assert_synthetic(&e.cause, "synthetic writer publisher cut");
    assert!(has(
        &e,
        Step::SqlCommit(Sql::PrepareControl),
        State::Confirmed
    ));
    hooks::reset();
    assert_eq!(
        phase(&f.metadata.join("projection.sqlite3")),
        "control_prepared"
    );
}

pub(super) fn assert_retained(
    p: &ProviderControlPublished,
    f: &ProviderBootstrapFailure,
    operation: ProviderBootstrapWriterOperation,
) {
    let a = p.inspection().write_report.as_ref().unwrap();
    let b = f.write_report.as_ref().unwrap();
    assert_eq!(
        a.provenance,
        ProviderBootstrapWriteReportProvenance::CurrentInvocation { operation }
    );
    assert_eq!(
        b.provenance,
        ProviderBootstrapWriteReportProvenance::RetainedIssuance { operation }
    );
    assert_eq!(a.attempt.family_root, b.attempt.family_root);
    assert_eq!(a.attempt.checkout_root, b.attempt.checkout_root);
    assert_eq!(a.attempt.stage_parent, b.attempt.stage_parent);
    assert_eq!(a.attempt.worktree_id, b.attempt.worktree_id);
    assert_eq!(a.attempt.transaction_id, b.attempt.transaction_id);
    assert_eq!(a.effects.len(), b.effects.len());
    for (a, b) in a.effects.iter().zip(&b.effects) {
        assert_eq!((a.step, a.state, &a.target), (b.step, b.state, &b.target));
    }
    assert_eq!(f.operation, "revalidate provider control publication");
    assert!(hooks::events().is_empty());
}

pub(super) fn pre_a(which: Create) {
    let f = Fixture::new();
    let before = f.witness();
    stop(Step::ExclusiveCreate(which), false);
    let e = failure(f.begin());
    assert_synthetic(&e.cause, "synthetic exact writer boundary");
    no_later_writes(&e, |s| {
        matches!(
            s,
            Step::ReceiptWrite(_) | Step::ReceiptRename(_) | Step::SqlOpen(_) | Step::FormatWrite
        )
    });
    assert_eq!(before, f.witness());
    assert!(!f.receipt(Receipt::Allocation).exists());
}

pub(super) fn receipt_fault(kind: Receipt, step: Step, after: bool, prefix: bool) {
    let f = Fixture::new();
    if kind == Receipt::Abandoned {
        drop(f.published());
    }
    if prefix {
        hooks::prefix(Some(kind), None);
    } else {
        stop(step, after);
    }
    let e = if kind == Receipt::Abandoned {
        let mut recovery = f.recovery();
        failure(recovery.abandon())
    } else {
        failure(f.begin())
    };
    assert_synthetic(
        &e.cause,
        if prefix {
            "synthetic short writer payload after real prefix write"
        } else {
            "synthetic exact writer boundary"
        },
    );
    let r = e.write_report.as_ref().unwrap();
    let committed = r
        .effects
        .iter()
        .any(|e| e.step == Step::ReceiptRename(kind) && e.state == State::Confirmed);
    assert_eq!(f.receipt(kind).exists(), committed);
    let forbidden = match kind {
        Receipt::Allocation => Sql::RegisterInitializing,
        Receipt::Prepared => Sql::PrepareControl,
        Receipt::Abandoned => Sql::RemoveInitializing,
    };
    no_later_writes(&e, |s| s == Step::SqlOpen(forbidden));
    if prefix {
        assert!(
            !r.effects
                .iter()
                .any(|e| e.step == Step::ReceiptRename(kind))
        );
    }
    // Retained temporary names are diagnostic only; discovery never scans them.
}

pub(super) fn commit_refusal(purpose: Sql) {
    let f = Fixture::new();
    hooks::commit_busy(purpose);
    let e = failure(f.begin());
    assert!(
        matches!(e.cause,Cause::Sqlite(rusqlite::Error::SqliteFailure(ref code,_))if code.code==rusqlite::ErrorCode::DatabaseBusy)
            || cause_sqlite(&e.cause, rusqlite::ffi::SQLITE_BUSY)
    );
    assert!(has(&e, Step::SqlCommit(purpose), State::Uncertain));
    assert!(!has(&e, Step::SqlCommit(purpose), State::Confirmed));
    assert!(has(&e, Step::SqlClose(purpose), State::Confirmed));
    hooks::release_reader();
}

pub(super) fn cause_sqlite(c: &Cause, code: i32) -> bool {
    use std::error::Error;
    if let Cause::Cleanup { primary, close } = c {
        return cause_sqlite(primary, code) || cause_sqlite(close, code);
    }
    let mut current: Option<&(dyn Error + 'static)> = Some(c);
    while let Some(e) = current {
        if let Some(actual) = e.downcast_ref::<Cause>() {
            if !std::ptr::eq(actual, c) {
                return cause_sqlite(actual, code);
            }
        }
        if let Some(rusqlite::Error::SqliteFailure(e, _)) = e.downcast_ref::<rusqlite::Error>() {
            if e.extended_code == code {
                return true;
            }
        }
        current = e.source();
    }
    false
}

pub(super) fn close_busy(purpose: Sql) {
    let f = Fixture::new();
    hooks::close_busy(purpose);
    let e = failure(f.begin());
    assert!(cause_sqlite(&e.cause, rusqlite::ffi::SQLITE_BUSY));
    assert!(has(&e, Step::SqlCommit(purpose), State::Confirmed));
    assert!(has(&e, Step::SqlClose(purpose), State::Uncertain));
    if purpose == Sql::InitializePrivate {
        no_later_writes(&e, |s| s == Step::FormatWrite);
    }
}

pub(super) fn abandon_control(staged: bool) {
    let f = Fixture::new();
    if staged {
        stop_at_prepared(&f);
    } else {
        drop(f.published());
    }
    let before = f.witness();
    let mut r = f.recovery();
    hooks::reset();
    let i = r.abandon().unwrap();
    assert_eq!(i.state, ProviderBootstrapState::Abandoned);
    let report = i.write_report.unwrap();
    assert_eq!(
        report.provenance,
        ProviderBootstrapWriteReportProvenance::CurrentInvocation {
            operation: ProviderBootstrapWriterOperation::Abandon
        }
    );
    let terminal = report
        .effects
        .iter()
        .position(|e| {
            e.step == Step::ReceiptFinalize(Receipt::Abandoned) && e.state == State::Confirmed
        })
        .unwrap();
    let remove = report
        .effects
        .iter()
        .position(|e| e.step == Step::SqlOpen(Sql::RemoveInitializing))
        .unwrap();
    assert!(terminal < remove);
    for (id, len, digest) in before {
        if id
            == super::super::super::super::database_io::tests::support::witness(
                &f.family.root().join("metadata/catalog.sqlite3"),
            )
            .0
        {
            continue;
        }
        assert!(f.witness().contains(&(id, len, digest)));
    }
    hooks::reset();
    let repeat = r.abandon().unwrap();
    assert_eq!(repeat.state, ProviderBootstrapState::Abandoned);
    assert!(repeat.write_report.unwrap().effects.is_empty());
    assert!(hooks::events().is_empty());
}

pub(super) fn dependent_private(table: &str) {
    let f = Fixture::new();
    drop(f.published());
    let mut r = f.recovery();
    let path = f.metadata.join("projection.sqlite3");
    raw_sql(&path, |c| {
        c.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;")
            .unwrap();
        let mut q = c.prepare(&format!("PRAGMA table_info({table})")).unwrap();
        let columns: Vec<(String, String)> = q
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        drop(q);
        let names = columns
            .iter()
            .map(|(name, _)| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(",");
        let values = columns
            .iter()
            .map(|(_, kind)| match kind.as_str() {
                "INTEGER" => "1",
                "BLOB" => "x'7b7d'",
                "TEXT" => "'uninterpreted-evidence'",
                _ => panic!("unexpected STRICT type"),
            })
            .collect::<Vec<_>>()
            .join(",");
        c.execute(
            &format!("INSERT INTO {table}({names}) VALUES({values})"),
            [],
        )
        .unwrap();
        // Deliberately uninterpreted dependency evidence, not a valid history/FK claim.
    });
    let before = f.witness();
    hooks::reset();
    let e = failure(r.abandon());
    invalid(
        &e.cause,
        &format!("provider bootstrap requires empty {table}"),
    );
    assert!(e.write_report.unwrap().effects.is_empty());
    assert!(hooks::events().is_empty());
    assert_eq!(before, f.witness());
    assert!(!f.receipt(Receipt::Abandoned).exists());
}

pub(super) fn family_dependency(which: &str) {
    let f = Fixture::new();
    drop(f.published());
    let mut r = f.recovery();
    raw_sql(&f.family.root().join("metadata/catalog.sqlite3"), |c| {
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        if which == "roots" {
            c.execute("INSERT INTO roots VALUES('writer-test-root',?1,'owned-test-db','writer-test-operation','digest',x'7b7d')",[f.worktree.as_str()]).unwrap();
        } else {
            c.execute("INSERT INTO reservations VALUES('writer-test-reservation',?1,'writer-test-operation',1)",[f.worktree.as_str()]).unwrap();
            if which == "installations" {
                c.execute("INSERT INTO installations VALUES('writer-test-reservation','test-file',x'7b7d',NULL,0)",[]).unwrap();
            }
        }
        let count: u64 = c
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    });
    let before = f.witness();
    hooks::reset();
    let e = failure(r.abandon());
    invalid(
        &e.cause,
        "provider bootstrap owner has accepted roots/reservations/installations",
    );
    assert_eq!(e.database, Some(ProviderDatabaseScope::FamilyCatalog));
    assert!(e.write_report.unwrap().effects.is_empty());
    assert_eq!(before, f.witness());
    assert!(!f.receipt(Receipt::Abandoned).exists());
    // Installations necessarily reference reservations: this qualifies the earlier
    // reservation fence plus dependency preservation, not an isolated later query.
}

pub(super) fn user_path_abandon(staged: bool) {
    let f = Fixture::new();
    if staged {
        stop_at_prepared(&f);
    } else {
        drop(f.published());
    }
    let user = f.checkout.join("retained-user.txt");
    fs::write(&user, b"user data is not bootstrap authority").unwrap();
    let before = super::super::super::super::database_io::tests::support::witness(&user);
    if staged {
        assert_eq!(f.inspect().state, ProviderBootstrapState::NeedsRepair);
        let e = failure(f.recovery().resume_control_publication());
        ineligible_inspection_refusal(
            &e,
            "resume provider control publication",
            None,
            "provider bootstrap is not eligible for publication-only resume",
        );
        assert!(e.write_report.unwrap().effects.is_empty());
    }
    let mut r = f.recovery();
    hooks::reset();
    let i = r.abandon().unwrap();
    assert_eq!(i.state, ProviderBootstrapState::Abandoned);
    assert_eq!(
        before,
        super::super::super::super::database_io::tests::support::witness(&user)
    );
}

pub(super) fn disk_rw_since(before: usize) -> usize {
    writer_observer::events()
        .into_iter()
        .skip(before)
        .filter(|(op, name, flags)| {
            op == "open"
                && !name.is_empty()
                && flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0
                && flags & rusqlite::ffi::SQLITE_OPEN_READWRITE != 0
        })
        .count()
}

pub(super) fn main_opens_since(before: usize) -> usize {
    writer_observer::events()
        .into_iter()
        .skip(before)
        .filter(|(op, name, flags)| {
            op == "open" && !name.is_empty() && flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0
        })
        .count()
}

pub(super) fn regular_boundary(
    f: &Fixture,
    omit_journal: bool,
) -> Vec<(PathBuf, Option<(String, u64, String)>)> {
    let a = stages(f);
    let mut paths = vec![
        f.family.root().join("FORMAT"),
        f.family.root().join("metadata/catalog.sqlite3"),
        f.metadata.join("projection.sqlite3"),
        f.receipt(Receipt::Allocation),
        f.receipt(Receipt::Prepared),
        f.receipt(Receipt::Abandoned),
    ];
    let stage = a.stage_identity().unwrap().stage().join("FORMAT");
    paths.push(if stage.try_exists().unwrap() {
        stage
    } else {
        f.checkout.join(".layrs/FORMAT")
    });
    for db in [
        f.family.root().join("metadata/catalog.sqlite3"),
        f.metadata.join("projection.sqlite3"),
    ] {
        for suffix in ["-journal", "-wal", "-shm"] {
            if omit_journal && suffix == "-journal" {
                continue;
            }
            let mut p = db.as_os_str().to_owned();
            p.push(suffix);
            paths.push(p.into());
        }
    }
    paths
        .into_iter()
        .map(|p| {
            let value = match fs::symlink_metadata(&p) {
                Ok(_) => Some(database_io::tests::support::witness(&p)),
                Err(e) if e.raw_os_error() == Some(2) => None,
                Err(e) => panic!("exact boundary witness unavailable {p:?}: {e}"),
            };
            (p, value)
        })
        .collect()
}

pub(super) fn cause_original<'a>(cause: &'a Cause) -> &'a Cause {
    match cause {
        Cause::Cleanup { primary, .. } => cause_original(primary),
        other => other,
    }
}

pub(super) fn io_fault_present(cause: &Cause, message: &str) -> bool {
    match cause {
        Cause::Io(e) => e.to_string() == message,
        Cause::Cleanup { primary, close } => {
            io_fault_present(primary, message) || io_fault_present(close, message)
        }
        _ => false,
    }
}

pub(super) fn check_no_allocation(
    f: &Fixture,
    e: &ProviderBootstrapFailure,
    before: &[(String, u64, String)],
    events: usize,
) {
    assert!(e.write_report.is_none());
    assert!(e.effects.is_empty());
    assert!(e.rollback_triggers.is_empty());
    f.assert_no_begin_allocation();
    assert_eq!(f.witness(), before);
    assert!(hooks::events().is_empty());
    assert_eq!(main_opens_since(events), 0);
}
