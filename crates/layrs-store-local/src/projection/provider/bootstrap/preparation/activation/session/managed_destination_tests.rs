//! Actual pinned source, published destination FORMAT and family/SQL copy.
use super::*;
use crate::projection::provider::{
    database_io::{self, WriterCloseObserver},
    process_profile::tests::child,
    records::transition::ProviderTransitionIntentRecord,
};
use rusqlite::{Connection, OpenFlags, ffi};
use std::fs;

#[derive(Default)]
struct Close {
    successful: u64,
}
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        if succeeded {
            self.successful += 1;
        }
        None
    }
}

#[derive(Default)]
struct RejectFirstClose {
    calls: u64,
}
impl WriterCloseObserver for RejectFirstClose {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "expected the committed family close to succeed physically"
        );
        self.calls += 1;
        (self.calls == 1).then(|| invalid("family close acknowledgement withheld"))
    }
}

#[test]
#[ignore = "owned audited Windows provider process; physical managed EmptyIngress to CopiedPrefix"]
fn published_managed_destination_registers_and_copies_without_ready() {
    child::run(
        concat!(
            module_path!(),
            "::published_managed_destination_registers_and_copies_without_ready"
        ),
        240,
        || {
            child::declare();
            super::managed_allocation_tests::with_planned(|fixture, session, attempt, planned| {
                let (mut destination, copying): (_, ProviderTransitionIntentRecord) =
                    super::managed_allocation_tests::bound_holder(session, attempt, planned);
                destination
                    .publish_format(
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &copying,
                        &Cancellation::default(),
                    )
                    .unwrap();
                destination.require_published(&copying.plan).unwrap();
                let source_db = fixture.metadata.join("projection.sqlite3");
                let source_before = database_io::tests::support::witness(&source_db);
                let mut close = Close::default();
                let first = super::super::managed_destination::register_and_copy(
                    &session.source.held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &copying,
                    &destination,
                    &mut close,
                    &Cancellation::default(),
                )
                .unwrap();
                assert_eq!(
                    first.family,
                    crate::projection::shared::provider_access::ImportRegistrationEffect::Inserted
                );
                assert_eq!(
                    first.destination,
                    super::super::managed_destination::DestinationSqlState::FreshCopied
                );
                assert_eq!(close.successful, 2);
                let second = super::super::managed_destination::register_and_copy(
                    &session.source.held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &copying,
                    &destination,
                    &mut close,
                    &Cancellation::default(),
                )
                .unwrap();
                assert_eq!(
                        second.family,
                        crate::projection::shared::provider_access::ImportRegistrationEffect::AlreadyPresent
                    );
                assert_eq!(
                    second.destination,
                    super::super::managed_destination::DestinationSqlState::ReplayedCopied
                );
                assert_eq!(close.successful, 2, "exact replay has no writer close");
                assert_eq!(
                    database_io::tests::support::witness(&source_db),
                    source_before
                );
                let catalog = Connection::open_with_flags(
                    fixture.family.root().join("metadata/catalog.sqlite3"),
                    OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let destination_state: String = catalog
                    .query_row(
                        "SELECT state FROM worktrees WHERE id=?1",
                        [copying
                            .plan
                            .source_prefix
                            .destination_identity
                            .worktree_id
                            .as_str()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(destination_state, "initializing");
                catalog.close().unwrap();
                let copied = Connection::open_with_flags(
                    destination.destination_database().path(),
                    OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let head: u64 = copied
                    .query_row("SELECT generation FROM projection_head", [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(head, 1);
                copied.close().unwrap();
            });
        },
    );
}

#[test]
#[ignore = "owned audited Windows provider process; family close refusal stops destination SQL"]
fn family_close_refusal_stops_before_destination_open() {
    child::run(
        concat!(
            module_path!(),
            "::family_close_refusal_stops_before_destination_open"
        ),
        240,
        || {
            child::declare();
            super::managed_allocation_tests::with_planned(|fixture, session, attempt, planned| {
                let (mut destination, copying) =
                    super::managed_allocation_tests::bound_holder(session, attempt, planned);
                destination
                    .publish_format(
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &copying,
                        &Cancellation::default(),
                    )
                    .unwrap();
                let mut close = RejectFirstClose::default();
                let refused = super::super::managed_destination::register_and_copy(
                    &session.source.held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &copying,
                    &destination,
                    &mut close,
                    &Cancellation::default(),
                );
                assert!(refused.is_err());
                assert_eq!(
                    close.calls, 1,
                    "no destination writer after family close refusal"
                );
                assert_eq!(
                    fs::metadata(destination.destination_database().path())
                        .unwrap()
                        .len(),
                    0
                );
                destination.require_published(&copying.plan).unwrap();
                let catalog = Connection::open_with_flags(
                    fixture.family.root().join("metadata/catalog.sqlite3"),
                    OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let state: String = catalog
                    .query_row(
                        "SELECT state FROM worktrees WHERE id=?1",
                        [copying
                            .plan
                            .source_prefix
                            .destination_identity
                            .worktree_id
                            .as_str()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    state, "initializing",
                    "the real family COMMIT remains visible"
                );
                catalog.close().unwrap();
            });
        },
    );
}

#[test]
#[ignore = "owned audited Windows provider process; foreign family registration refused"]
fn foreign_initializing_registration_refuses_without_destination_write() {
    child::run(
        concat!(
            module_path!(),
            "::foreign_initializing_registration_refuses_without_destination_write"
        ),
        240,
        || {
            child::declare();
            super::managed_allocation_tests::with_planned(|fixture, session, attempt, planned| {
                let (mut destination, copying) =
                    super::managed_allocation_tests::bound_holder(session, attempt, planned);
                destination
                    .publish_format(
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &copying,
                        &Cancellation::default(),
                    )
                    .unwrap();
                let catalog_path = fixture.family.root().join("metadata/catalog.sqlite3");
                let catalog = Connection::open(&catalog_path).unwrap();
                catalog
                    .execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                    .unwrap();
                let proof = destination.proof();
                let member = proof
                    .format
                    .shared
                    .as_ref()
                    .unwrap()
                    .metadata_path
                    .to_str()
                    .unwrap();
                catalog.execute(
                    "INSERT INTO worktrees(id,metadata_path,database_identity,marker,state) VALUES(?1,?2,?3,?4,'initializing')",
                    rusqlite::params![proof.format.identity.worktree_id.as_str(), member,
                        proof.private_database_identity, b"foreign family marker"],
                ).unwrap();
                catalog.execute_batch("COMMIT").unwrap();
                catalog.close().unwrap();
                let mut close = Close::default();
                let refused = super::super::managed_destination::register_and_copy(
                    &session.source.held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &copying,
                    &destination,
                    &mut close,
                    &Cancellation::default(),
                );
                assert!(refused.is_err());
                assert_eq!(close.successful, 0);
                assert_eq!(
                    fs::metadata(destination.destination_database().path())
                        .unwrap()
                        .len(),
                    0
                );
                destination.require_published(&copying.plan).unwrap();
            });
        },
    );
}
