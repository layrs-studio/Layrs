//! Fresh owned native allocations under a durable source Planned intent.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::{
        capture_family::active_holder_with_writable_file_private, fixture::Fixture,
        transition_capture::planned_for_image,
    },
    capture_lock::CaptureAttemptLock,
    database_io::{self, WriterCloseObserver},
    process_profile::tests::child,
    records::{
        transition::{ProviderTransitionIntentRecord, TransitionState},
        Record,
    },
    transition_sql::SourceIntentEffect,
    validation,
};
use rusqlite::{ffi, Connection, OpenFlags};
use std::fs;

#[derive(Default)]
struct Close {
    closed: bool,
}
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        self.closed = succeeded;
        None
    }
}

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned native managed-allocation recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare();
                $body
            });
        }
    };
}

fn remove_placeholder(path: &Path) {
    if fs::remove_file(path).is_err() {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
        fs::remove_file(path).unwrap();
    }
}

fn with_planned(
    test: impl FnOnce(
        &Fixture,
        &ProviderActiveSession,
        &CaptureAttemptLock,
        &ProviderTransitionIntentRecord,
    ),
) {
    let (fixture, holder, allocation, prepared, binding) =
        active_holder_with_writable_file_private();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
    session.connect().unwrap();
    let path = fixture.checkout.join("README.md");
    assert!(path.exists());
    remove_placeholder(&path); // do not hydrate the cold base
    fs::write(&path, b"ordinary managed destination replacement").unwrap();

    // Full source/family validation and its scoped locks end before hashing.
    let active = {
        let held = &session.source.held;
        let _locks = held
            .family
            .locks(&allocation.format.identity.worktree_id, false)
            .unwrap();
        capture_state::read(
            held,
            Some(&session.source.root),
            Some(&Cancellation::default()),
        )
        .unwrap_or_else(|error| panic!("active source read: {}", error.cause))
    };
    let mut checkpoint = session.session.export_checkpoint().unwrap();
    let mut image = super::super::managed_image::plan(
        &mut checkpoint,
        session.source.as_ref(),
        &active.active.source,
        &active.active.activated.snapshot,
        &Cancellation::default(),
    )
    .unwrap();
    let mut planned = planned_for_image(&fixture, &allocation, &prepared, &binding, image.digest());
    let parent = session.session.guard_export_parent(&fixture.outer).unwrap();
    planned.plan.destination_parent = parent.root().to_str().unwrap().into();
    planned.plan.destination_parent_identity = parent.filesystem_identity();
    drop(parent);
    planned.encode().unwrap();

    let attempt =
        CaptureAttemptLock::acquire(&session.source.held.artifacts, &session.source.held.family)
            .unwrap_or_else(|error| panic!("source attempt lock refused: {error:?}"));
    let mut close = Close::default();
    assert_eq!(
        super::super::managed_journal::begin(
            &session.source.held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &mut image,
            &planned,
            &mut close,
            &Cancellation::default(),
        )
        .unwrap(),
        SourceIntentEffect::Inserted,
    );
    assert!(close.closed, "source Planned writer must close observably");
    drop(image);
    drop(checkpoint);
    test(&fixture, &session, &attempt, &planned);
    drop(attempt);
    session.disconnect().unwrap();
}

fn durable_source(
    fixture: &Fixture,
    session: &ProviderActiveSession,
) -> ProviderTransitionIntentRecord {
    let held = &session.source.held;
    let _locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .unwrap();
    let c = Connection::open_with_flags(
        fixture.metadata.join("projection.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let checked = validation::private_active_transition_with_source(
        &c,
        &held.artifacts.allocation,
        held.artifacts.prepared.as_ref().unwrap(),
        &held.binding,
        Some(session.source.root.registration().root_id()),
        None,
    )
    .unwrap();
    assert_eq!(checked.transitions.len(), 1);
    c.close().unwrap();
    checked.transitions.into_iter().next().unwrap()
}

fn bound_holder<'a>(
    session: &'a ProviderActiveSession,
    attempt: &'a CaptureAttemptLock,
    planned: &'a ProviderTransitionIntentRecord,
) -> (
    super::super::managed_allocation::ManagedDestinationAllocation<'a>,
    ProviderTransitionIntentRecord,
) {
    let holder = super::super::managed_allocation::allocate(
        &session.session,
        &session.source.held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        planned,
        &Cancellation::default(),
    )
    .unwrap_or_else(|error| panic!("fresh managed allocation failed: {error:?}"));
    let mut copying = ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
    copying.state = TransitionState::Copying;
    copying.destination =
        Some(serde_json::from_slice(&serde_json::to_vec(holder.proof()).unwrap()).unwrap());
    copying.require_transition(planned).unwrap();
    let mut close = Close::default();
    assert_eq!(
        super::super::managed_journal::bind_allocation(
            &session.source.held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            attempt,
            planned,
            &copying,
            &holder,
            &mut close,
            &Cancellation::default(),
        )
        .unwrap(),
        SourceIntentEffect::Updated,
    );
    assert!(close.closed, "source Copying writer must close observably");
    (holder, copying)
}

owned!(
    fresh_allocation_binds_exact_copying_without_publishing_format,
    {
        with_planned(|fixture, session, attempt, planned| {
            let family_database = fixture.family.root().join("metadata/catalog.sqlite3");
            let family_before = database_io::tests::support::witness(&family_database);
            let holder = super::super::managed_allocation::allocate(
                &session.session,
                &session.source.held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                planned,
                &Cancellation::default(),
            )
            .unwrap_or_else(|error| panic!("fresh managed allocation failed: {error:?}"));
            holder.verify(&planned.plan).unwrap();
            assert_eq!(
                holder.root(),
                fixture.outer.join(&planned.plan.destination_leaf_nonce)
            );
            assert!(!holder.root().join(".layrs/FORMAT").exists());
            let temporary = holder.root().join(".layrs").join(format!(
                "FORMAT.pending-{}",
                planned.plan.destination_leaf_nonce
            ));
            assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
            assert_eq!(
                fs::metadata(holder.destination_database().path())
                    .unwrap()
                    .len(),
                0
            );
            assert_eq!(
                database_io::tests::support::witness(&family_database),
                family_before
            );

            let mut copying =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            copying.state = TransitionState::Copying;
            copying.destination =
                Some(serde_json::from_slice(&serde_json::to_vec(holder.proof()).unwrap()).unwrap());
            copying.require_transition(planned).unwrap();
            let mut close = Close::default();
            assert_eq!(
                super::super::managed_journal::bind_allocation(
                    &session.source.held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    planned,
                    &copying,
                    &holder,
                    &mut close,
                    &Cancellation::default(),
                )
                .unwrap(),
                SourceIntentEffect::Updated,
            );
            assert!(close.closed, "source Copying writer must close observably");
            assert_eq!(durable_source(fixture, session), copying);
            holder.verify(&copying.plan).unwrap(); // no Planned reread in holder.verify
            assert!(!holder.root().join(".layrs/FORMAT").exists());
            assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
            assert_eq!(
                fs::metadata(holder.destination_database().path())
                    .unwrap()
                    .len(),
                0
            );
            assert_eq!(
                database_io::tests::support::witness(&family_database),
                family_before
            );
            let family =
                Connection::open_with_flags(&family_database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .unwrap();
            let registered: u64 = family
                .query_row(
                    "SELECT count(*) FROM worktrees WHERE id=?1",
                    [&copying
                        .plan
                        .source_prefix
                        .destination_identity
                        .worktree_id
                        .as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                registered, 0,
                "allocation and Copying cannot register destination Ready"
            );
            family.close().unwrap();
        });
    }
);

owned!(occupied_root_is_never_adopted_or_deleted, {
    with_planned(|fixture, session, attempt, planned| {
        let target = fixture.outer.join(&planned.plan.destination_leaf_nonce);
        fs::create_dir(&target).unwrap();
        let valuable = target.join("valuable.txt");
        fs::write(&valuable, b"preexisting bytes").unwrap();
        let family_database = fixture.family.root().join("metadata/catalog.sqlite3");
        let family_before = database_io::tests::support::witness(&family_database);
        let before = durable_source(fixture, session);
        assert_eq!(&before, planned);
        let error = match super::super::managed_allocation::allocate(
            &session.session,
            &session.source.held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            attempt,
            planned,
            &Cancellation::default(),
        ) {
            Ok(_) => panic!("occupied destination was adopted"),
            Err(error) => error,
        };
        assert_eq!(error.attempted_root, target);
        assert_eq!(error.effects.len(), 1);
        assert_eq!(
            error.effects[0].step,
            super::super::managed_allocation::ManagedAllocationStep::Root
        );
        assert!(!error.effects[0].confirmed);
        assert!(
            error.retained_pin_count() >= 2,
            "parent/lifecycle pins retained on refusal"
        );
        assert_eq!(fs::read(&valuable).unwrap(), b"preexisting bytes");
        assert!(!target.join(".layrs").exists());
        assert_eq!(
            database_io::tests::support::witness(&family_database),
            family_before
        );
        assert_eq!(durable_source(fixture, session), before);
        drop(error); // only release pins; no cleanup
    });
});

owned!(copying_publishes_exact_inode_and_replay_is_idempotent, {
    with_planned(|fixture, session, attempt, planned| {
        let (mut holder, copying) = bound_holder(session, attempt, planned);
        let family_database = fixture.family.root().join("metadata/catalog.sqlite3");
        let family_before = database_io::tests::support::witness(&family_database);
        let temporary = holder.root().join(".layrs").join(format!(
            "FORMAT.pending-{}",
            planned.plan.destination_leaf_nonce,
        ));
        let final_path = holder.root().join(".layrs/FORMAT");
        assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
        let publication = holder
            .publish_format(
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(
            publication.state,
            super::super::managed_allocation::ManagedFormatState::Published
        );
        assert!(publication.effects.iter().any(|e| {
            e.step == super::super::managed_allocation::ManagedFormatStep::Rename && e.confirmed
        }));
        holder.require_published(&copying.plan).unwrap();
        assert!(!temporary.exists());
        assert_eq!(
            fs::read(&final_path).unwrap(),
            holder.proof().format.bytes().unwrap()
        );
        assert_eq!(
            fs::metadata(holder.destination_database().path())
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            database_io::tests::support::witness(&family_database),
            family_before
        );
        assert_eq!(durable_source(fixture, session), copying);

        let replay = holder
            .publish_format(
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(
            replay.state,
            super::super::managed_allocation::ManagedFormatState::AlreadyPublished
        );
        assert!(!replay.effects.iter().any(|e| matches!(
            e.step,
            super::super::managed_allocation::ManagedFormatStep::Append
                | super::super::managed_allocation::ManagedFormatStep::Rename,
        )));
        holder.require_published(&copying.plan).unwrap();
        assert_eq!(
            fs::read(&final_path).unwrap(),
            holder.proof().format.bytes().unwrap()
        );
        assert_eq!(
            database_io::tests::support::witness(&family_database),
            family_before
        );
        assert_eq!(durable_source(fixture, session), copying);
    });
});

owned!(foreign_final_format_collision_preserves_both_inodes, {
    with_planned(|fixture, session, attempt, planned| {
        let (mut holder, copying) = bound_holder(session, attempt, planned);
        let family_database = fixture.family.root().join("metadata/catalog.sqlite3");
        let family_before = database_io::tests::support::witness(&family_database);
        let temporary = holder.root().join(".layrs").join(format!(
            "FORMAT.pending-{}",
            planned.plan.destination_leaf_nonce,
        ));
        let final_path = holder.root().join(".layrs/FORMAT");
        let foreign = b"valuable unrelated FORMAT bytes";
        fs::write(&final_path, foreign).unwrap();
        let error = holder
            .publish_format(
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &Cancellation::default(),
            )
            .unwrap_err();
        assert!(
            error.cause.to_string().contains("both names")
                || error.cause.to_string().contains("inode"),
            "{error}"
        );
        assert!(
            error.effects.is_empty(),
            "collision is rejected before byte effects"
        );
        assert_eq!(fs::read(&final_path).unwrap(), foreign);
        assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
        assert_eq!(
            fs::metadata(holder.destination_database().path())
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            database_io::tests::support::witness(&family_database),
            family_before
        );
        assert_eq!(durable_source(fixture, session), copying);
    });
});
