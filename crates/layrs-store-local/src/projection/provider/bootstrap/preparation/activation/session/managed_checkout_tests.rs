//! Native checkout under retained owners. The injected fault withholds only
//! an acknowledgement after a successful close; it does not simulate BUSY.
use super::*;
use crate::projection::provider::{
    checkout_journal::{CheckoutEntry, CheckoutRow, FileState},
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    process_profile::tests::child,
    records::{
        Record,
        transition::{ProviderTransitionIntentRecord, TransitionState},
    },
};
use rusqlite::{Connection, OpenFlags, ffi};
use std::fs;

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
    refuse_at: Option<usize>,
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
            "checkout fixture requires a positive physical close"
        );
        self.closes += 1;
        (self.refuse_at == Some(self.closes)).then(|| invalid("checkout acknowledgement withheld"))
    }
}

fn case(refuse_owned_ack: bool) {
    super::managed_staging_tests::with_content_planned_checkout(
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
                    .unwrap_or_else(|e| panic!("active read: {}", e.cause))
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
            assert_eq!(image.files().len(), 2);
            let current: Vec<_> = image
                .files()
                .iter()
                .map(|f| f.manifest.manifest().id().clone())
                .collect();
            let required =
                super::managed_staging_tests::manifests_for(&active.active.source, &image);
            let mut staging =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            staging.state = TransitionState::Staging;
            staging.reservation = Some(staging.reservation_operation().unwrap());
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
                &required,
                &mut setup,
                &cancel,
            )
            .unwrap();
            let mut lease = super::super::managed_content::ManagedImportLease::open(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &destination,
                &mut image,
                &staging,
                &required,
                &fresh,
                &cancel,
            )
            .unwrap();
            lease.reserve_plan(&mut setup, &cancel).unwrap();
            for id in current.iter().chain(required.iter().map(|m| m.id())) {
                lease.install_exact(id, &mut setup, &cancel).unwrap();
            }
            let protected = lease.protect_exact(&mut setup, &cancel).unwrap();
            let source_path = fixture.metadata.join("projection.sqlite3");
            let family_path = fixture.family.root().join("metadata/catalog.sqlite3");
            let stable = || {
                [
                    database_io::tests::support::witness(&source_path),
                    database_io::tests::support::witness(&family_path),
                ]
            };
            let before = stable();
            let mut close = Close {
                refuse_at: refuse_owned_ack.then_some(4),
                ..Close::default()
            };
            let result = lease.copy_checkout(&protected, &mut close, &cancel);
            if refuse_owned_ack {
                let error = result
                    .err()
                    .expect("ownership acknowledgement must be refused");
                assert!(
                    error
                        .to_string()
                        .contains("checkout acknowledgement withheld"),
                    "{error}"
                );
                assert_eq!((close.opens, close.closes), (4, 4));
                assert!(!destination.root().join("README.md").exists());
                let temp = destination.root().join(format!(
                    ".layrs/checkout-{}/file-000000",
                    staging.transaction_id
                ));
                assert_eq!(
                    fs::metadata(&temp).unwrap().len(),
                    0,
                    "no producer bytes before ownership acknowledgement"
                );
                let c = Connection::open_with_flags(
                    destination.destination_database().path(),
                    OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let payload: Vec<u8> = c
                    .query_row(
                        "SELECT payload FROM projection_prejournal WHERE path=?1",
                        [format!(
                            "provider-checkout:{}:file:000000",
                            staging.transaction_id
                        )],
                        |r| r.get(0),
                    )
                    .unwrap();
                let row = CheckoutRow::decode(&payload).unwrap();
                assert!(matches!(
                    row.entry,
                    CheckoutEntry::File {
                        state: FileState::Owned { .. },
                        ..
                    }
                ));
                c.close().unwrap();
            } else {
                let completed = result.unwrap();
                completed.require_physical(&lease, &cancel).unwrap();
                assert_eq!(
                    fs::read(destination.root().join("README.md")).unwrap(),
                    b"provider base content"
                );
                assert_eq!(
                    fs::read(destination.root().join("dir/extra.txt")).unwrap(),
                    b"nested checkout content"
                );
                assert_eq!(close.opens, close.closes);
                let blocked = fs::OpenOptions::new()
                    .write(true)
                    .open(destination.root().join("README.md"))
                    .unwrap_err();
                assert_eq!(blocked.raw_os_error(), Some(32));
                drop(completed);
            }
            assert_eq!(
                stable(),
                before,
                "checkout does not mutate source or family accounting"
            );
            drop(protected);
            drop(lease);
            // Same process and same checkpoint/allocation owners, but fresh lease,
            // fresh protection rehash and typed journal observation.
            let mut lease = super::super::managed_content::ManagedImportLease::open(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &destination,
                &mut image,
                &staging,
                &required,
                &fresh,
                &cancel,
            )
            .unwrap();
            let protected = lease.reopen_protection(&cancel).unwrap();
            let database_before =
                database_io::tests::support::witness(destination.destination_database().path());
            let mut replay = Close::default();
            let completed = lease
                .copy_checkout(&protected, &mut replay, &cancel)
                .unwrap();
            completed.require_physical(&lease, &cancel).unwrap();
            assert_eq!(
                fs::read(destination.root().join("README.md")).unwrap(),
                b"provider base content"
            );
            assert_eq!(
                fs::read(destination.root().join("dir/extra.txt")).unwrap(),
                b"nested checkout content"
            );
            if !refuse_owned_ack {
                assert_eq!((replay.opens, replay.closes), (0, 0));
                assert_eq!(
                    database_io::tests::support::witness(destination.destination_database().path()),
                    database_before
                );
            } else {
                assert!(replay.opens > 0);
                assert_eq!(replay.opens, replay.closes);
            }
            assert_eq!(stable(), before);
            let c = Connection::open_with_flags(
                destination.destination_database().path(),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            assert_eq!(
                c.query_row(
                    "SELECT state FROM projection_provider_transitions",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "staging"
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM projection_claims", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                0
            );
            c.close().unwrap();
            let stage = destination
                .root()
                .join(format!(".layrs/checkout-{}", staging.transaction_id));
            assert!(fs::read_dir(stage).unwrap().next().is_none());
            if !refuse_owned_ack {
                // A later unexpected file cannot be adopted or cleaned up by a
                // completed replay, even while the known final files stay pinned.
                let foreign = destination.root().join("unowned.txt");
                fs::write(&foreign, b"preserve unowned checkout entry").unwrap();
                let witness = database_io::tests::support::witness(&foreign);
                let database =
                    database_io::tests::support::witness(destination.destination_database().path());
                let mut refusal = Close::default();
                let error = lease
                    .copy_checkout(&protected, &mut refusal, &cancel)
                    .err()
                    .expect("foreign checkout entry must refuse replay");
                assert!(error.to_string().contains("checkout namespace"), "{error}");
                assert_eq!((refusal.opens, refusal.closes), (0, 0));
                assert_eq!(database_io::tests::support::witness(&foreign), witness);
                assert_eq!(
                    database_io::tests::support::witness(destination.destination_database().path()),
                    database
                );
            }
            drop(completed);
            drop(protected);
            drop(lease);
            drop(image);
            drop(checkpoint);
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned managed checkout recipe"]
fn managed_checkout_copies_nested_image_and_replays_without_writer() {
    child::run(
        concat!(
            module_path!(),
            "::managed_checkout_copies_nested_image_and_replays_without_writer"
        ),
        240,
        || {
            child::declare();
            case(false);
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned managed checkout recipe"]
fn managed_checkout_owned_ack_refusal_reopens_before_producer_bytes() {
    child::run(
        concat!(
            module_path!(),
            "::managed_checkout_owned_ack_refusal_reopens_before_producer_bytes"
        ),
        240,
        || {
            child::declare();
            case(true);
        },
    );
}
