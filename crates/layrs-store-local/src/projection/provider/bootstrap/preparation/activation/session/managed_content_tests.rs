//! Retained image/owners and actual family reservation; ROOT releases execution.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::fixture::Fixture,
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    process_profile::tests::child,
    records::{
        transition::{ProviderTransitionIntentRecord, TransitionState},
        Record,
    },
};
use crate::projection::shared::provider_access::provider_import::content::ImportReservationEffect;
use crate::projection::ProjectionStoreError;
use layrs_core::scm::FileManifest;
use rusqlite::{ffi, Connection, OpenFlags};
use std::path::PathBuf;

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
    refuse_acknowledgement: bool,
    busy_source_close: bool,
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, _: *mut ffi::sqlite3) -> CauseResult<()> {
        if point == WriterOpenPoint::BeforeRaw {
            self.opens += 1;
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "owned managed reservation writer closes physically"
        );
        self.closes += 1;
        if self.busy_source_close {
            assert_eq!(self.closes, 1);
            // No SQLite call occurs between this successful family close and
            // the retained source RO close: only namespace/pin checks remain.
            database_io::test_hooks::busy_next_read_close();
        }
        self.refuse_acknowledgement
            .then(|| invalid("managed reservation close acknowledgement withheld"))
    }
}

#[derive(Clone, Copy)]
enum Case {
    Exact,
    Acknowledgement,
    ForgedUnion,
    InsufficientMetadata,
    SourceCloseBusy,
}

fn assert_genuine_read_close_busy(error: Cause) {
    let Cause::Cleanup { primary, close } = error else {
        panic!("source close must report genuine Cleanup, got {error:?}");
    };
    assert!(
        matches!(
            primary.as_ref(),
            Cause::Validation(error)
                if matches!(error.as_ref(), ProjectionStoreError::Invalid(message)
                    if message == "observable SQLite close failed")
        ),
        "source action must have completed before its failed close: {primary:?}"
    );
    assert!(
        matches!(
            close.as_ref(),
            Cause::Sqlite(rusqlite::Error::SqliteFailure(code, _))
                if code.code == rusqlite::ErrorCode::DatabaseBusy
                    && code.extended_code == ffi::SQLITE_BUSY
        ),
        "close must retain actual SQLITE_BUSY: {close:?}"
    );
}

fn witnesses(paths: &[PathBuf]) -> Vec<(String, u64, String)> {
    paths
        .iter()
        .map(|path| database_io::tests::support::witness(path))
        .collect()
}

fn assert_reservation(
    fixture: &Fixture,
    session: &ProviderActiveSession,
    staging: &ProviderTransitionIntentRecord,
) {
    let held = &session.source.held;
    let _locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .unwrap();
    let catalog = Connection::open_with_flags(
        fixture.family.root().join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    catalog.execute_batch("BEGIN DEFERRED").unwrap();
    let owner = staging
        .plan
        .source_prefix
        .destination_identity
        .worktree_id
        .as_str();
    let operation = staging.reservation_operation().unwrap();
    let mut query = catalog
        .prepare(
            "SELECT id,operation,remaining FROM reservations WHERE owner=?1 ORDER BY id LIMIT 2",
        )
        .unwrap();
    let rows = query
        .query_map([owner], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![(operation.clone(), operation, staging.plan.quota_bytes)]
    );
    drop(query);
    let state: String = catalog
        .query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(state, "initializing");
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM exact_installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
    ] {
        assert!(!catalog.query_row(sql, [owner], |row| row.get::<_, bool>(0)).unwrap());
    }
    catalog.close().unwrap();
}

fn case(kind: Case) {
    super::managed_staging_tests::with_content_planned(
        !matches!(kind, Case::InsufficientMetadata),
        matches!(kind, Case::ForgedUnion),
        |fixture, session, attempt, planned| {
            let cancel = Cancellation::default();
            let (mut destination, copying) =
                super::managed_allocation_tests::bound_holder(session, attempt, planned);
            destination
                .publish_format(
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &copying,
                    &cancel,
                )
                .unwrap();
            let held = &session.source.held;
            let mut setup = Close::default();
            super::super::managed_destination::register_and_copy(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &destination,
                &mut setup,
                &cancel,
            )
            .unwrap();
            let active = {
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                capture_state::read(held, Some(&session.source.root), Some(&cancel))
                    .unwrap_or_else(|error| panic!("active read: {}", error.cause))
            };
            let fresh = active.active.source.initial().clone();
            let mut checkpoint = session.session.export_checkpoint().unwrap();
            let mut image = super::super::managed_image::plan(
                &mut checkpoint,
                session.source.as_ref(),
                &active.active.source,
                &active.active.activated.snapshot,
                &cancel,
            )
            .unwrap();
            let manifests =
                super::managed_staging_tests::manifests_for(&active.active.source, &image);
            assert_eq!(
                manifests.len(),
                2,
                "fixture retains base and native replacement"
            );
            let mut staging =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            staging.state = TransitionState::Staging;
            staging.reservation = Some(staging.reservation_operation().unwrap());
            let supplied: &[FileManifest] = if matches!(kind, Case::ForgedUnion) {
                &[]
            } else {
                &manifests
            };
            super::super::managed_staging::stage(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &staging,
                &destination,
                &mut image,
                supplied,
                &mut setup,
                &cancel,
            )
            .unwrap();
            assert_eq!(setup.opens, 4);
            assert_eq!(setup.closes, 4);
            let private_paths = [
                fixture.metadata.join("projection.sqlite3"),
                destination.destination_database().path().to_owned(),
            ];
            let private_before = witnesses(&private_paths);
            let family_path = fixture.family.root().join("metadata/catalog.sqlite3");
            let family_before = database_io::tests::support::witness(&family_path);
            let opened = super::super::managed_content::ManagedImportLease::open(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &destination,
                &mut image,
                &staging,
                supplied,
                &fresh,
                &cancel,
            );
            if matches!(kind, Case::ForgedUnion | Case::InsufficientMetadata) {
                let error = opened
                    .err()
                    .expect("semantic plan preflight must refuse before reservation");
                let expected = if matches!(kind, Case::ForgedUnion) {
                    "managed protected manifest coverage differs"
                } else {
                    "managed reservation metadata envelope insufficient"
                };
                assert!(
                    error.to_string().contains(expected),
                    "wrong refusal: {error}"
                );
                assert_eq!(
                    database_io::tests::support::witness(&family_path),
                    family_before
                );
                assert_eq!(witnesses(&private_paths), private_before);
            } else {
                let mut lease =
                    opened.unwrap_or_else(|error| panic!("managed lease refused: {error}"));
                assert_eq!(
                    database_io::tests::support::witness(&family_path),
                    family_before,
                    "opening the lease is read-only"
                );
                let mut close = Close {
                    refuse_acknowledgement: matches!(kind, Case::Acknowledgement),
                    busy_source_close: matches!(kind, Case::SourceCloseBusy),
                    ..Close::default()
                };
                if matches!(kind, Case::SourceCloseBusy) {
                    let owned_name = concat!(
                        module_path!(),
                        "::managed_reservation_source_close_busy_stops_after_family_commit"
                    );
                    let owned_name = owned_name
                        .strip_prefix("layrs_store_local::")
                        .unwrap_or(owned_name);
                    assert_eq!(
                        std::env::var("LAYRS_PROVIDER_OWNED_CASE").as_deref(),
                        Ok(owned_name)
                    );
                }
                if matches!(kind, Case::SourceCloseBusy) {
                    // Keep every session/checkpoint holder outside the caught
                    // closure. A failed assertion after quarantine must also
                    // terminate without unwinding through those finalizers.
                    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let effect = lease.reserve_plan(&mut close, &cancel);
                        assert_genuine_read_close_busy(effect.unwrap_err());
                        assert_eq!((close.opens, close.closes), (1, 1));
                        // After quarantine observe raw files only, without
                        // SQLite/provider re-entry or rollback.
                        assert_eq!(witnesses(&private_paths), private_before);
                        let family_after = database_io::tests::support::witness(&family_path);
                        assert_eq!(family_after.0, family_before.0);
                        assert_ne!(family_after.2, family_before.2);
                    }));
                    // Exit only this child already owned by child::run, after
                    // success OR failed assertion. Skip all fixture finalizers: a
                    // native Session disconnect could otherwise drain callbacks.
                    // Parent reaps the exact handle; exit 1 preserves failure.
                    std::process::exit(if checked.is_ok() { 0 } else { 1 });
                }
                let effect = lease.reserve_plan(&mut close, &cancel);
                if matches!(kind, Case::Acknowledgement) {
                    let error = effect.unwrap_err();
                    assert!(error
                        .to_string()
                        .contains("managed reservation close acknowledgement withheld"));
                } else {
                    assert_eq!(effect.unwrap(), ImportReservationEffect::Inserted);
                    let durable = database_io::tests::support::witness(&family_path);
                    assert_eq!(
                        lease.reserve_plan(&mut close, &cancel).unwrap(),
                        ImportReservationEffect::AlreadyPresent
                    );
                    assert_eq!(database_io::tests::support::witness(&family_path), durable);
                }
                assert_eq!((close.opens, close.closes), (1, 1));
                drop(lease);
                assert_eq!(witnesses(&private_paths), private_before);
                let durable = database_io::tests::support::witness(&family_path);
                let mut replay = super::super::managed_content::ManagedImportLease::open(
                    held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &destination,
                    &mut image,
                    &staging,
                    supplied,
                    &fresh,
                    &cancel,
                )
                .unwrap_or_else(|error| panic!("managed replay lease refused: {error}"));
                let mut replay_close = Close::default();
                assert_eq!(
                    replay.reserve_plan(&mut replay_close, &cancel).unwrap(),
                    ImportReservationEffect::AlreadyPresent
                );
                assert_eq!((replay_close.opens, replay_close.closes), (0, 0));
                drop(replay);
                assert_eq!(database_io::tests::support::witness(&family_path), durable);
                assert_eq!(witnesses(&private_paths), private_before);
                assert_reservation(fixture, session, &staging);
            }
            drop(image);
            drop(checkpoint);
        },
    );
}

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned managed-reservation recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                240,
                || {
                    child::declare();
                    $body
                },
            );
        }
    };
}

owned!(managed_reservation_is_exact_and_replay_opens_no_writer, {
    case(Case::Exact);
});
owned!(managed_reservation_ack_refusal_replays_without_writer, {
    case(Case::Acknowledgement);
});
owned!(
    forged_union_and_insufficient_metadata_refuse_before_reservation,
    {
        case(Case::ForgedUnion);
        case(Case::InsufficientMetadata);
    }
);
owned!(
    managed_reservation_source_close_busy_stops_after_family_commit,
    {
        case(Case::SourceCloseBusy);
    }
);
