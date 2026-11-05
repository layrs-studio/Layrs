//! Owned native source-journal boundary and observable-close retry recipe.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::{capture_family, transition_capture},
    capture_lock::CaptureAttemptLock,
    database_io::{self, WriterCloseObserver},
    process_profile::tests::child,
    records::{
        Record,
        transition::{ProviderTransitionIntentRecord, TransitionState},
    },
    transition_sql::SourceIntentEffect,
};
use std::fs;

#[derive(Default)]
struct Close {
    calls: usize,
    closed: bool,
    refuse: bool,
}
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut rusqlite::ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        self.calls += 1;
        self.closed = succeeded;
        self.refuse
            .then(|| invalid("test post-close acknowledgement withheld"))
    }
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases exact owned managed source journal recipe"]
fn source_journal_retries_uncertain_ack_and_cancels_without_family_writes() {
    child::run(
        concat!(
            module_path!(),
            "::source_journal_retries_uncertain_ack_and_cancels_without_family_writes"
        ),
        240,
        || {
            child::declare();
            let (fixture, holder, allocation, prepared, binding) =
                capture_family::active_holder_with_writable_file_private();
            let mut session = holder
                .into_disconnected_session()
                .unwrap_or_else(|error| panic!("session withheld: {}", error.cause()));
            session.connect().unwrap();
            let path = fixture.checkout.join("README.md");
            fs::remove_file(&path).unwrap();
            fs::write(&path, b"managed journal retained image").unwrap();
            let held = &session.source.held;
            let attempt = CaptureAttemptLock::acquire(&held.artifacts, &held.family).unwrap();
            let active = {
                let _locks = held
                    .family
                    .locks(&allocation.format.identity.worktree_id, false)
                    .unwrap();
                capture_state::read(held, Some(&session.source.root), None)
                    .unwrap_or_else(|error| panic!("active read: {}", error.cause))
            };
            let cancel = Cancellation::default();
            let mut checkpoint = session.session.export_checkpoint().unwrap();
            let mut image = super::super::managed_image::plan(
                &mut checkpoint,
                session.source.as_ref(),
                &active.active.source,
                &active.active.activated.snapshot,
                &cancel,
            )
            .unwrap();
            // This fixture plan declares future destination fields. The journal
            // writes only source intent and cannot confer those future authorities.
            let planned = transition_capture::planned_for_image(
                &fixture,
                &allocation,
                &prepared,
                &binding,
                image.digest(),
            );
            let catalog_path = fixture.family.root().join("metadata/catalog.sqlite3");
            let family_before = database_io::tests::support::witness(&catalog_path);
            let mut close = Close {
                refuse: true,
                ..Default::default()
            };
            let error = super::super::managed_journal::begin(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                &attempt,
                &mut image,
                &planned,
                &mut close,
                &cancel,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("acknowledgement withheld"),
                "{error}"
            );
            assert!(close.closed);
            assert_eq!(close.calls, 1);
            // The first call must not acknowledge uncertain close. A distinct
            // attempt reopens read-only and recognizes its exact durable intent.
            let mut retry_close = Close::default();
            assert_eq!(
                super::super::managed_journal::begin(
                    held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    &attempt,
                    &mut image,
                    &planned,
                    &mut retry_close,
                    &cancel
                )
                .unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert!(retry_close.closed);
            assert_eq!(retry_close.calls, 1);
            assert_eq!(
                database_io::tests::support::witness(&catalog_path),
                family_before
            );
            drop(image);
            drop(checkpoint);
            // Cancellation remains possible even if the old complete image is no
            // longer valid. It preserves bytes and grants no cleanup permission.
            let unknown = fixture.checkout.join("later.txt");
            fs::write(&unknown, b"preserve after planning").unwrap();
            let mut cancelled =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            cancelled.state = TransitionState::Cancelled;
            let mut cancel_close = Close::default();
            assert_eq!(
                super::super::managed_journal::cancel(
                    held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    &attempt,
                    &planned,
                    &cancelled,
                    &mut cancel_close,
                    &cancel
                )
                .unwrap(),
                SourceIntentEffect::Updated
            );
            assert!(cancel_close.closed);
            assert_eq!(
                database_io::tests::support::witness(&catalog_path),
                family_before
            );
            assert_eq!(fs::read(&path).unwrap(), b"managed journal retained image");
            assert_eq!(fs::read(&unknown).unwrap(), b"preserve after planning");
            drop(attempt);
            session.disconnect().unwrap();
            drop(session);
            let reopened = crate::projection::shared::PartialStoreFamily::open_provider_active(
                fixture.family.root(),
                &fixture.worktree,
                &binding.transaction_id,
                &binding,
            )
            .unwrap();
            reopened.revalidate().unwrap();
        },
    );
}
