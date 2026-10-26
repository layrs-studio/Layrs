//! Exact-owned qualification for the published bootstrap metadata handoff.
use super::super::super::database_io::tests::writer_support as writer_observer;
use super::super::catalog::catalog_test_hooks as read_hooks;
use super::*;
use crate::projection::provider::records::Receipt as _;
use layrs_core::scm::RepoPath;
use read_hooks::CatalogReadPoint;
use std::fs;

macro_rules! owned_catalog {
    ($name:ident, $seconds:literal, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned-host catalog recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                $seconds,
                || {
                    child::declare();
                    hooks::reset();
                    $body
                },
            );
        }
    };
}

macro_rules! observed_catalog {
    ($name:ident, $seconds:literal, $fixture:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned-host catalog recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                $seconds,
                || {
                    hooks::reset();
                    writer_observer::clear_observer();
                    let $fixture = Fixture::new();
                    writer_observer::install(
                        &$fixture.family.root().join("metadata/catalog.sqlite3"),
                    );
                    child::declare();
                    $body
                },
            );
        }
    };
}

fn assert_source(
    view: &crate::projection::ProviderBootstrapCatalog<'_>,
    source: &ProviderBootstrapSource,
) {
    let expected = source.commitment();
    assert_eq!(view.source_generation(), expected.source_generation);
    assert_eq!(view.source_digest(), expected.snapshot_digest);
    assert_eq!(view.selection_digest(), expected.selection_digest);
    assert_eq!(view.catalog_id(), expected.catalog_id);
    assert_eq!(view.source_snapshot(), source.initial());
    assert_eq!(view.selection(), source.selection());
    assert_eq!(
        view.vfs_catalog().identity(),
        source.vfs_catalog().identity()
    );
    let actual: Vec<crate::projection::ProviderCatalogDescriptor<'_>> =
        view.descriptors().collect();
    let rows = source.descriptors();
    assert_eq!(actual.len(), rows.len());
    for (descriptor, row) in actual.into_iter().zip(rows) {
        assert_eq!(descriptor.path(), &row.path);
        assert_eq!(descriptor.file_id(), &row.file_id);
        assert_eq!(descriptor.manifest_id(), &row.manifest_id);
        assert_eq!(descriptor.manifest(), &row.manifest);
        assert_eq!(descriptor.byte_length(), row.bytes);
        assert_eq!(descriptor.executable(), row.executable);
        assert_eq!(
            view.descriptor(&row.path).unwrap().manifest_id(),
            &row.manifest_id
        );
    }
    assert!(
        view.descriptor(&RepoPath::parse("absent.txt").unwrap())
            .is_none()
    );
}

fn invalid_message(cause: &Cause) -> Option<&str> {
    match cause {
        Cause::Validation(error) => match error.as_ref() {
            ProjectionStoreError::Invalid(message) => Some(message),
            ProjectionStoreError::ProviderBootstrap(nested) => invalid_message(&nested.cause),
            _ => None,
        },
        Cause::Cleanup { primary, .. } => invalid_message(primary),
        _ => None,
    }
}

fn assert_retained_read_failure(
    published: &ProviderControlPublished,
    error: &ProviderBootstrapFailure,
    expected: &str,
) {
    assert_eq!(invalid_message(&error.cause), Some(expected));
    assert!(error.effects.is_empty());
    let report = error
        .write_report
        .as_ref()
        .expect("retained writer issuance");
    assert_eq!(
        report.provenance,
        ProviderBootstrapWriteReportProvenance::RetainedIssuance {
            operation: ProviderBootstrapWriterOperation::Begin
        }
    );
    let original = published.inspection().write_report.as_ref().unwrap();
    assert_eq!(report.effects.len(), original.effects.len());
    for (a, b) in report.effects.iter().zip(&original.effects) {
        assert_eq!(
            (&a.step, &a.state, &a.target),
            (&b.step, &b.state, &b.target)
        );
    }
}

fn sql(path: &Path, statement: &str) {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .unwrap();
    connection.execute_batch(statement).unwrap();
    connection.close().unwrap();
}

fn fixed_witness(
    f: &Fixture,
    stage_format: &Path,
) -> Vec<(PathBuf, Option<(String, u64, String)>)> {
    let mut paths = vec![
        f.family.root().join("FORMAT"),
        f.family.root().join("metadata/catalog.sqlite3"),
        f.metadata.join("projection.sqlite3"),
        f.receipt(ProviderBootstrapReceipt::Allocation),
        f.receipt(ProviderBootstrapReceipt::Prepared),
        f.receipt(ProviderBootstrapReceipt::Abandoned),
        f.checkout.join(".layrs/FORMAT"),
        stage_format.to_owned(),
    ];
    for database in [
        f.family.root().join("metadata/catalog.sqlite3"),
        f.metadata.join("projection.sqlite3"),
    ] {
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = database.as_os_str().to_owned();
            sidecar.push(suffix);
            paths.push(sidecar.into());
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let witness = match fs::symlink_metadata(&path) {
                Ok(_) => {
                    Some(crate::projection::provider::database_io::tests::support::witness(&path))
                }
                Err(error) if matches!(error.raw_os_error(), Some(2 | 3)) => None,
                Err(error) => panic!("fixed witness unavailable {path:?}: {error}"),
            };
            (path, witness)
        })
        .collect()
}

owned_catalog!(published_catalog_exact_and_cold, 180, {
    let f = Fixture::new();
    let make_source = || {
        use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, TreeEntryTarget};
        let chunk = Chunk::new(b"catalog metadata without resident bytes".to_vec()).unwrap();
        let manifest = FileManifest::from_chunks(ChunkingDescriptor::single(), &[chunk]).unwrap();
        assert!(manifest.byte_length() > 0);
        assert!(!manifest.chunks().is_empty());
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
    };
    let published = match f
        .family
        .begin_provider_bootstrap(
            &f.checkout,
            &f.stages,
            &f.worktree,
            fixture::TX,
            make_source(),
        )
        .unwrap()
    {
        ProviderControlPublicationOutcome::Published(published) => published,
        _ => panic!("nonempty metadata source must publish"),
    };
    let catalog_path = f.family.root().join("metadata/catalog.sqlite3");
    let content_counts = || {
        let db = rusqlite::Connection::open_with_flags(
            &catalog_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let counts: Vec<u64> = [
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
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        })
        .collect();
        db.close().unwrap();
        counts
    };
    assert_eq!(content_counts(), vec![0; 7]);
    let before = f.witness();
    let source = make_source();
    hooks::reset();
    let view = published.bootstrap_catalog().unwrap();
    assert_source(&view, &source);
    assert!(view.descriptors().next().unwrap().byte_length() > 0);
    assert!(
        !view
            .descriptors()
            .next()
            .unwrap()
            .manifest()
            .chunks()
            .is_empty()
    );
    assert_eq!(
        view.binding().encode().unwrap(),
        published.binding().encode().unwrap()
    );
    view.revalidate().unwrap();
    let second = published.bootstrap_catalog().unwrap();
    assert_source(&second, &source);
    assert_eq!(content_counts(), vec![0; 7]);
    assert_eq!(before, f.witness());
    assert!(hooks::events().is_empty());
});

owned_catalog!(resumed_catalog_is_same_without_reinitialization, 180, {
    let f = Fixture::new();
    let published = f.published();
    let original = published.bootstrap_catalog().unwrap();
    let catalog_id = original.catalog_id().to_owned();
    let source_digest = original.source_digest().to_owned();
    drop(original);
    drop(published);
    let before = f.witness();
    hooks::reset();
    let resumed = match f.recovery().resume_control_publication().unwrap() {
        ProviderControlPublicationOutcome::Published(published) => published,
        _ => panic!("published resume must retain published capability"),
    };
    let view = resumed.bootstrap_catalog().unwrap();
    assert_source(&view, &Fixture::source());
    assert_eq!(view.catalog_id(), catalog_id);
    assert_eq!(view.source_digest(), source_digest);
    assert!(
        resumed
            .inspection()
            .write_report
            .as_ref()
            .unwrap()
            .effects
            .is_empty()
    );
    assert_eq!(before, f.witness());
    assert!(hooks::events().is_empty());
});

owned_catalog!(two_family_members_keep_distinct_catalogs, 240, {
    let f = Fixture::new();
    let first = f.published();
    let second_checkout = f.outer.join("other-c");
    fs::create_dir(&second_checkout).unwrap();
    let second_id = WorktreeId::parse("worktree:writer-other-catalog").unwrap();
    let empty_source =
        super::super::super::validation::tests::fixture::Fixture::new("G0", false).source;
    let second = match f
        .family
        .begin_provider_bootstrap(
            &second_checkout,
            &f.stages,
            &second_id,
            "00000000000000000000000000000002",
            empty_source,
        )
        .unwrap()
    {
        ProviderControlPublicationOutcome::Published(published) => published,
        _ => panic!("second member publication must return its own holder"),
    };
    let before = f.witness();
    let second_metadata = f
        .family
        .root()
        .join("worktrees")
        .join(
            blake3::hash(second_id.as_str().as_bytes())
                .to_hex()
                .as_str(),
        )
        .join("metadata");
    let second_files = [
        second_metadata.join("projection.sqlite3"),
        second_metadata.join("provider-control/allocation.json"),
    ];
    let second_before: Vec<_> = second_files
        .iter()
        .map(|path| crate::projection::provider::database_io::tests::support::witness(path))
        .collect();
    hooks::reset();
    let first_view = first.bootstrap_catalog().unwrap();
    let second_view = second.bootstrap_catalog().unwrap();
    assert_source(&first_view, &Fixture::source());
    let empty_source =
        super::super::super::validation::tests::fixture::Fixture::new("G0", false).source;
    assert_source(&second_view, &empty_source);
    assert_ne!(first_view.catalog_id(), second_view.catalog_id());
    assert_ne!(
        first_view.binding().transaction_id,
        second_view.binding().transaction_id
    );
    let path = RepoPath::parse("README.md").unwrap();
    assert!(first_view.descriptor(&path).is_some());
    assert!(second_view.descriptor(&path).is_none());
    assert_eq!(second_view.descriptors().len(), 0);
    first_view.revalidate().unwrap();
    second_view.revalidate().unwrap();
    assert_eq!(before, f.witness());
    assert_eq!(
        second_before,
        second_files
            .iter()
            .map(|path| crate::projection::provider::database_io::tests::support::witness(path))
            .collect::<Vec<_>>()
    );
    assert!(hooks::events().is_empty());
});

owned_catalog!(
    descriptor_and_selection_contradictions_refuse_without_change,
    240,
    {
        for (statement, expected) in [
            (
                "DELETE FROM projection_provider_descriptors",
                "missing or unsupported selected source descriptor",
            ),
            (
                "INSERT INTO projection_provider_descriptors SELECT catalog_id,'zzz.md',file_id,bytes,executable,manifest_id,manifest FROM projection_provider_descriptors",
                "extra source descriptor",
            ),
            (
                "UPDATE projection_provider_descriptors SET manifest_id='blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                "provider descriptor row projection/order differs",
            ),
            (
                "UPDATE projection_provider_catalogs SET selection_digest=lower(hex(zeroblob(32)))",
                "provider catalog row projection differs",
            ),
        ] {
            let f = Fixture::new();
            let published = f.published();
            let view = published.bootstrap_catalog().unwrap();
            sql(&f.metadata.join("projection.sqlite3"), statement);
            let before = f.witness();
            hooks::reset();
            let error = failure(published.bootstrap_catalog());
            assert_retained_read_failure(&published, &error, expected);
            let error = failure(view.revalidate());
            assert_retained_read_failure(&published, &error, expected);
            assert_eq!(before, f.witness());
            assert!(hooks::events().is_empty());
        }
    }
);

owned_catalog!(pending_private_sidecars_refuse_without_open, 180, {
    for suffix in ["-wal", "-shm"] {
        let f = Fixture::new();
        let published = f.published();
        let sidecar = f.metadata.join(format!("projection.sqlite3{suffix}"));
        fs::write(&sidecar, b"retained unsupported sidecar").unwrap();
        let before = f.witness();
        hooks::reset();
        let error = failure(published.bootstrap_catalog());
        assert_retained_read_failure(
            &published,
            &error,
            "provider database WAL/SHM sidecar present before SQLite open",
        );
        assert_eq!(error.database, Some(ProviderDatabaseScope::PrivateMetadata));
        assert_eq!(fs::read(&sidecar).unwrap(), b"retained unsupported sidecar");
        assert_eq!(before, f.witness());
        assert!(hooks::events().is_empty());
    }
});

owned_catalog!(terminal_and_receipt_changes_refuse_metadata, 180, {
    for (terminal, expected) in [
        (true, "provider receipt appeared; fresh binding required"),
        (false, "provider receipt exceeds bound"),
    ] {
        let f = Fixture::new();
        let published = f.published();
        let allocation = AllocationRecord::decode(
            &fs::read(f.receipt(ProviderBootstrapReceipt::Allocation)).unwrap(),
        )
        .unwrap();
        let stage_format = allocation.stage_identity().unwrap().stage().join("FORMAT");
        if terminal {
            fs::write(
                f.receipt(ProviderBootstrapReceipt::Abandoned),
                b"incomplete terminal",
            )
            .unwrap();
        } else {
            use std::io::Write;
            let mut receipt = fs::OpenOptions::new()
                .append(true)
                .open(f.receipt(ProviderBootstrapReceipt::Allocation))
                .unwrap();
            receipt.write_all(b" ").unwrap();
            receipt.sync_all().unwrap();
        }
        let before = fixed_witness(&f, &stage_format);
        hooks::reset();
        let error = failure(published.bootstrap_catalog());
        assert_retained_read_failure(&published, &error, expected);
        assert_eq!(before, fixed_witness(&f, &stage_format));
        assert!(hooks::events().is_empty());
    }
});

owned_catalog!(second_read_must_still_be_published, 180, {
    let f = Fixture::new();
    let published = f.published();
    let path = f.metadata.join("projection.sqlite3");
    let seen = std::rc::Rc::new(std::cell::Cell::new(false));
    let reached = seen.clone();
    read_hooks::install(move |point, connection| {
        if point != CatalogReadPoint::AfterInspection {
            return;
        }
        assert!(connection.is_none());
        assert!(!reached.replace(true), "one deliberate phase race");
        let db = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        let bytes: Vec<u8> = db
            .query_row("SELECT payload FROM projection_provider_mounts", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut mount = MountRecord::decode(&bytes).unwrap();
        mount.phase = MountPhase::ControlPrepared;
        db.execute(
            "UPDATE projection_provider_mounts SET phase='control_prepared',payload=?1,digest=?2",
            rusqlite::params![mount.encode().unwrap(), mount.digest().unwrap()],
        )
        .unwrap();
        db.close().unwrap();
    });
    let error = failure(published.bootstrap_catalog());
    read_hooks::reset();
    assert!(seen.get());
    assert_retained_read_failure(
        &published,
        &error,
        "published provider catalog requires published private mount",
    );
    assert_eq!(error.database, Some(ProviderDatabaseScope::PrivateMetadata));
    let phase: String = rusqlite::Connection::open_with_flags(
        &f.metadata.join("projection.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row("SELECT phase FROM projection_provider_mounts", [], |r| {
        r.get(0)
    })
    .unwrap();
    assert_eq!(phase, "control_prepared");
});

owned_catalog!(catalog_locks_cover_both_read_passes_and_release, 240, {
    let f = std::rc::Rc::new(Fixture::new());
    let published = f.published();
    let points = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = points.clone();
    let fixture = f.clone();
    read_hooks::install(move |point, connection| {
        assert_eq!(
            connection.is_some(),
            point == CatalogReadPoint::AfterPrivateValidation
        );
        process::competing_catalog(&fixture, true);
        observed.borrow_mut().push(point);
    });
    let before = f.witness();
    let view = published.bootstrap_catalog().unwrap();
    read_hooks::reset();
    assert_eq!(
        *points.borrow(),
        [
            CatalogReadPoint::AfterInspection,
            CatalogReadPoint::AfterPrivateValidation
        ]
    );
    process::competing_catalog(&f, false);
    assert_source(&view, &Fixture::source());
    assert_eq!(before, f.witness());
});

observed_catalog!(actual_private_hot_refuses_without_recovery, 240, f, {
    let published = f.published();
    let private = f.metadata.join("projection.sqlite3");
    writer_observer::hot(&f.outer, &private, false);
    let before = f.witness();
    let writable = writer_observer::writable_opens();
    hooks::reset();
    let error = failure(published.bootstrap_catalog());
    assert_eq!(
        error.database,
        Some(ProviderDatabaseScope::PrivateMetadata),
        "{error:?}"
    );
    let trigger = error
        .readonly_rollback
        .as_ref()
        .expect("original private rollback trigger");
    assert!(
        matches!(trigger.as_ref(), rusqlite::Error::SqliteFailure(code, _) if code.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
    );
    assert!(error.rollback_triggers.iter().any(|entry| {
        entry.database == ProviderDatabaseScope::PrivateMetadata
            && std::ptr::eq(entry.error(), trigger.as_ref())
    }));
    assert_eq!(writer_observer::writable_opens(), writable);
    assert_eq!(before, f.witness());
    assert!(hooks::events().is_empty());
});

observed_catalog!(readonly_close_busy_preserves_cause_and_stops, 180, f, {
    let published = f.published();
    let points = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = points.clone();
    let close_boundary = std::rc::Rc::new(std::cell::Cell::new(None));
    let boundary = close_boundary.clone();
    read_hooks::install(move |point, connection| {
        observed.borrow_mut().push(point);
        if point == CatalogReadPoint::AfterPrivateValidation {
            boundary.set(Some(writer_observer::events().len()));
            let db = connection.expect("actual private read-only connection");
            let raw = unsafe { db.handle() };
            let mut statement = std::ptr::null_mut();
            assert_eq!(
                unsafe {
                    rusqlite::ffi::sqlite3_prepare_v2(
                        raw,
                        c"SELECT count(*) FROM projection_provider_descriptors".as_ptr(),
                        -1,
                        &mut statement,
                        std::ptr::null_mut(),
                    )
                },
                rusqlite::ffi::SQLITE_OK
            );
            assert!(!statement.is_null());
            // The child exits with the quarantined handle; there is no retry.
        }
    });
    let error = failure(published.bootstrap_catalog());
    read_hooks::reset();
    assert_eq!(
        error.database,
        Some(ProviderDatabaseScope::PrivateMetadata),
        "{error:?}"
    );
    assert_eq!(
        error.kind,
        recovery::ProviderBootstrapFailureKind::ObservableCloseFailure
    );
    assert_eq!(
        *points.borrow(),
        [
            CatalogReadPoint::AfterInspection,
            CatalogReadPoint::AfterPrivateValidation
        ]
    );
    let boundary = close_boundary
        .get()
        .expect("read-only connection reached close");
    assert!(
        writer_observer::events()
            .into_iter()
            .skip(boundary)
            .all(|(op, _, _)| op != "open")
    );
    let Cause::Cleanup { close, .. } = &error.cause else {
        panic!("observable close failure must preserve cleanup cause: {error:?}")
    };
    assert!(
        matches!(close.as_ref(), Cause::Sqlite(rusqlite::Error::SqliteFailure(code, _)) if code.code == rusqlite::ErrorCode::DatabaseBusy)
    );
    assert_eq!(
        error.write_report.as_ref().unwrap().provenance,
        ProviderBootstrapWriteReportProvenance::RetainedIssuance {
            operation: ProviderBootstrapWriterOperation::Begin
        }
    );
});
