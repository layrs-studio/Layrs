use layrs_client_core::native_scm::*;
use std::{fs, path::PathBuf};
struct Checkout(PathBuf);

fn append_observation(workspace: &NativeWorkspace, step: layrs_core::scm::StepId, when: &str) {
    append_observation_as(workspace, step, when, None).unwrap();
}
fn append_observation_as(
    workspace: &NativeWorkspace,
    step: layrs_core::scm::StepId,
    when: &str,
    actor: Option<layrs_core::scm::ActorId>,
) -> layrs_store::scm::Result<layrs_store::scm::ApplyBatchOutcome> {
    use layrs_core::scm::{CaptureReason, Metadata, StepObservation, Timestamp};
    use layrs_store::scm::{EntityRecord, ScmRepository, WriteBatch};
    let state = workspace.state().unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let observation = StepObservation::new(
        stamp.observation_id,
        step,
        Some(state.space.id().clone()),
        Some(state.operation.id().clone()),
        Some(workspace.worktree_id().clone()),
        actor,
        None,
        Timestamp::parse(when).unwrap(),
        CaptureReason::Received,
        Metadata::empty(),
    );
    let batch = WriteBatch::new(
        stamp.transaction_id,
        vec![],
        vec![EntityRecord::StepObservation(observation)],
        vec![],
        vec![],
    )
    .unwrap();
    layrs_store_local::scm_v1::NativeScmRepository::new(workspace.store_root()).apply_batch(&batch)
}

#[test]
fn explicit_local_identity_is_persisted_and_captured_without_changing_step_identity() {
    use layrs_core::scm::{ActorId, DeviceId, Timestamp};
    let dir = Checkout::new();
    let workspace = NativeWorkspace::initialize(&dir.0, "Identities").unwrap();
    assert!(workspace.local_identity().unwrap().is_none());
    let alice = LocalIdentity {
        actor_id: ActorId::parse("actor:alice-local").unwrap(),
        device_id: Some(DeviceId::parse("device:alice-laptop").unwrap()),
        display_name: Some("Alice local".into()),
    };
    let bob = LocalIdentity {
        actor_id: ActorId::parse("actor:bob-local").unwrap(),
        device_id: None,
        display_name: None,
    };
    workspace.configure_identity(alice.clone()).unwrap();
    fs::write(dir.0.join("a"), "one").unwrap();
    let mut stamp = MutationStamp::fresh().unwrap();
    stamp.timestamp = Timestamp::parse("2020-01-01T00:00:00Z").unwrap();
    let CapturePlan::Change(prepared_alice) = workspace
        .client()
        .prepare_disk_capture(
            workspace.worktree_id(),
            SnapshotLimits::default(),
            stamp.clone(),
        )
        .unwrap()
    else {
        panic!("changed")
    };
    // Changing the declaration after prepare cannot relabel the frozen batch.
    workspace.configure_identity(bob.clone()).unwrap();
    let CapturePlan::Change(prepared_bob) = workspace
        .client()
        .prepare_disk_capture(workspace.worktree_id(), SnapshotLimits::default(), stamp)
        .unwrap()
    else {
        panic!("changed")
    };
    assert_eq!(prepared_alice.step_id(), prepared_bob.step_id());
    workspace.client().accept(&prepared_alice).unwrap();
    let accepted_step = prepared_alice.step_id().clone();
    let state = workspace.state().unwrap();
    assert_eq!(state.operation.actor_id(), Some(&alice.actor_id));
    assert_eq!(state.operation.device_id(), alice.device_id.as_ref());
    let reopened = NativeWorkspace::open(&dir.0).unwrap();
    assert_eq!(reopened.local_identity().unwrap(), Some(bob.clone()));
    append_observation_as(
        &workspace,
        accepted_step.clone(),
        "2021-01-01T00:00:00Z",
        Some(bob.actor_id.clone()),
    )
    .unwrap();
    let filter = StepHistoryFilter {
        actor: Some(alice.actor_id.clone()),
        since: None,
        until: None,
    };
    let page = workspace
        .step_history_filtered("@layer:main", None, 100, None, &filter)
        .unwrap();
    assert_eq!(page.steps.len(), 1);
    assert_eq!(page.observations[&accepted_step].len(), 2);
    // Alice matches one observation and 2021 matches another: no joined match.
    let paired = StepHistoryFilter {
        since: Some(Timestamp::parse("2021-01-01T00:00:00Z").unwrap()),
        ..filter
    };
    assert!(workspace
        .step_history_filtered("@layer:main", None, 100, None, &paired)
        .unwrap()
        .steps
        .is_empty());
    assert!(append_observation_as(
        &workspace,
        accepted_step,
        "2021-01-01T00:00:00Z",
        Some(ActorId::parse("actor:unregistered").unwrap())
    )
    .is_err());
    fs::write(dir.0.join("a"), "two").unwrap();
    workspace.capture().unwrap();
    assert_eq!(
        workspace.state().unwrap().operation.actor_id(),
        Some(&bob.actor_id)
    );
    assert_eq!(
        workspace
            .operation_history_filtered(
                None,
                100,
                "",
                &OperationHistoryFilter {
                    actor: Some(alice.actor_id),
                    since: None,
                    until: None
                }
            )
            .unwrap()
            .operations
            .len(),
        1
    );
}
impl Checkout {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("layrs-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("preserved {}", self.0.display());
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[test]
fn history_pages_keep_the_same_immutable_root_during_new_captures() {
    let dir = Checkout::new();
    fs::write(dir.0.join("a"), "0").unwrap();
    let workspace = NativeWorkspace::initialize(&dir.0, "Paged").unwrap();
    for n in 1..=4 {
        fs::write(dir.0.join("a"), n.to_string()).unwrap();
        workspace.capture().unwrap();
    }
    let expected = workspace.operations(100).unwrap();
    let first = workspace.operation_history(None, 2, "").unwrap();
    let mut ids: Vec<_> = first.operations.iter().map(|o| o.id().clone()).collect();
    fs::write(dir.0.join("a"), "later").unwrap();
    workspace.capture().unwrap();
    let mut cursor = first.next;
    while let Some(next) = cursor {
        let page = workspace.operation_history(Some(&next), 2, "").unwrap();
        assert_eq!(page.anchor, first.anchor);
        ids.extend(page.operations.into_iter().map(|o| o.id().clone()));
        cursor = page.next;
    }
    assert_eq!(
        ids,
        expected.iter().map(|o| o.id().clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        workspace
            .operation_history(None, 500, "capture worktree")
            .unwrap()
            .operations
            .len(),
        5
    );
    assert!(workspace.operation_history(None, 0, "").is_err());
}

#[test]
fn exact_path_history_and_step_pages_never_infer_renames() {
    let dir = Checkout::new();
    fs::write(dir.0.join("a"), "zero").unwrap();
    let workspace = NativeWorkspace::initialize(&dir.0, "Paths").unwrap();
    let baseline = workspace.create_marker("baseline", "").unwrap();
    fs::write(dir.0.join("b"), "other").unwrap();
    workspace.capture().unwrap();
    fs::write(dir.0.join("a"), "one").unwrap();
    workspace.capture().unwrap();
    let first = workspace
        .step_history("@layer:main", None, 1, Some("a"))
        .unwrap();
    assert_eq!(first.steps.len(), 1);
    fs::rename(dir.0.join("a"), dir.0.join("renamed")).unwrap();
    workspace.capture().unwrap();
    let next = workspace
        .step_history("@layer:main", first.next.as_ref(), 1, Some("a"))
        .unwrap();
    assert_eq!(next.anchor, first.anchor);
    assert_eq!(next.steps[0].id(), baseline.target_step_id());
    assert!(next.next.is_none());
    assert!(workspace
        .step_history("@layer:main", first.next.as_ref(), 1, Some("b"))
        .is_err());
    assert_eq!(
        workspace
            .step_history("@layer:main", None, 100, Some("a"))
            .unwrap()
            .steps
            .len(),
        3
    );
    assert_eq!(
        workspace
            .step_history("@layer:main", None, 100, Some("renamed"))
            .unwrap()
            .steps
            .len(),
        1
    );
    assert_eq!(
        workspace
            .resolve_step_name("@marker:baseline")
            .unwrap()
            .step_id,
        *baseline.target_step_id()
    );
    workspace.create_marker("baseline", "").unwrap();
    assert!(workspace.resolve_step_name("@marker:baseline").is_err());
}

#[test]
fn automatic_capture_records_its_reason_outside_step_identity() {
    use layrs_core::scm::CaptureReason;
    use layrs_store::scm::EntityRecord;
    let dir = Checkout::new();
    let workspace = NativeWorkspace::initialize(&dir.0, "Reasons").unwrap();
    fs::write(dir.0.join("a"), "idle").unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let automatic = workspace
        .client()
        .prepare_disk_capture_with_reason(
            workspace.worktree_id(),
            SnapshotLimits::default(),
            stamp.clone(),
            CaptureReason::AutomaticIdle,
        )
        .unwrap();
    let manual = workspace
        .client()
        .prepare_disk_capture(workspace.worktree_id(), SnapshotLimits::default(), stamp)
        .unwrap();
    let (CapturePlan::Change(automatic), CapturePlan::Change(manual)) = (automatic, manual) else {
        panic!("changes expected")
    };
    assert_eq!(automatic.step_id(), manual.step_id());
    assert!(automatic.batch().entity_records().iter().any(|e|matches!(e,EntityRecord::StepObservation(o) if o.capture_reason()==CaptureReason::AutomaticIdle)));
    workspace.client().accept(&automatic).unwrap();
    assert!(!workspace.is_dirty().unwrap());
}

#[test]
fn operation_periods_compare_instants_and_pin_all_filters_in_cursors() {
    use layrs_core::scm::{ActorId, Timestamp};
    let dir = Checkout::new();
    let workspace = NativeWorkspace::initialize(&dir.0, "Periods").unwrap();
    for hour in 0..3 {
        fs::write(dir.0.join("a"), hour.to_string()).unwrap();
        let mut stamp = MutationStamp::fresh().unwrap();
        stamp.timestamp = Timestamp::parse(format!("2020-01-01T0{hour}:00:00Z")).unwrap();
        let CapturePlan::Change(change) = workspace
            .client()
            .prepare_disk_capture(workspace.worktree_id(), SnapshotLimits::default(), stamp)
            .unwrap()
        else {
            panic!("changed")
        };
        workspace.client().accept(&change).unwrap();
    }
    let filter = OperationHistoryFilter {
        actor: None,
        since: Some(Timestamp::parse("2020-01-01T02:00:00+02:00").unwrap()),
        until: Some(Timestamp::parse("2019-12-31T21:00:00-05:00").unwrap()),
    };
    let first = workspace
        .operation_history_filtered(None, 1, "", &filter)
        .unwrap();
    assert_eq!(
        first.operations[0].created_at().as_str(),
        "2020-01-01T02:00:00Z"
    );
    let cursor = first.next.unwrap();
    let remaining = workspace
        .operation_history_filtered(Some(&cursor), 100, "", &filter)
        .unwrap();
    assert_eq!(remaining.operations.len(), 2);
    assert!(remaining.next.is_none());
    assert_eq!(
        remaining.operations[1].created_at().as_str(),
        "2020-01-01T00:00:00Z"
    );
    let changed = OperationHistoryFilter {
        until: None,
        ..filter.clone()
    };
    assert!(workspace
        .operation_history_filtered(Some(&cursor), 10, "", &changed)
        .is_err());
    let actor = OperationHistoryFilter {
        actor: Some(ActorId::parse("actor:missing").unwrap()),
        ..filter.clone()
    };
    assert!(workspace
        .operation_history_filtered(None, 10, "", &actor)
        .unwrap()
        .operations
        .is_empty());
    let inverted = OperationHistoryFilter {
        since: filter.until,
        until: Some(Timestamp::parse("2019-01-01T00:00:00Z").unwrap()),
        actor: None,
    };
    assert!(workspace
        .operation_history_filtered(None, 10, "", &inverted)
        .is_err());
}

#[test]
fn step_observation_projection_is_rebuilt_and_pages_exclude_backdated_appends() {
    use layrs_core::scm::{ActorId, Timestamp};
    let dir = Checkout::new();
    let workspace = NativeWorkspace::initialize(&dir.0, "Observations").unwrap();
    for hour in 0..3 {
        fs::write(dir.0.join("a"), hour.to_string()).unwrap();
        let mut stamp = MutationStamp::fresh().unwrap();
        stamp.timestamp = Timestamp::parse(format!("2020-01-01T0{hour}:00:00Z")).unwrap();
        let CapturePlan::Change(change) = workspace
            .client()
            .prepare_disk_capture(workspace.worktree_id(), SnapshotLimits::default(), stamp)
            .unwrap()
        else {
            panic!("change")
        };
        workspace.client().accept(&change).unwrap();
    }
    let filter = StepHistoryFilter {
        actor: None,
        since: Some(Timestamp::parse("2020-01-01T02:00:00+02:00").unwrap()),
        until: Some(Timestamp::parse("2019-12-31T21:00:00-05:00").unwrap()),
    };
    let expected = workspace
        .step_history_filtered("@layer:main", None, 100, None, &filter)
        .unwrap();
    assert_eq!(expected.steps.len(), 3);
    let first = workspace
        .step_history_filtered("@layer:main", None, 1, None, &filter)
        .unwrap();
    let next_step = expected.steps[1].id().clone();
    append_observation(&workspace, next_step.clone(), "2020-01-01T00:30:00Z");
    let initial = workspace
        .step_history("@layer:main", None, 100, None)
        .unwrap()
        .steps
        .last()
        .unwrap()
        .id()
        .clone();
    // The initial Step did not match the period. An appended old timestamp
    // must not introduce it into the already-started result set.
    append_observation(&workspace, initial, "2020-01-01T00:15:00Z");
    fs::write(dir.0.join("a"), "new capture").unwrap();
    workspace.capture().unwrap();
    let page = workspace
        .step_history_filtered("@layer:main", first.next.as_ref(), 100, None, &filter)
        .unwrap();
    assert_eq!(page.steps, expected.steps[1..]);
    assert_eq!(
        page.observations[&next_step],
        expected.observations[&next_step]
    );
    assert_eq!(page.observation_snapshot, first.observation_snapshot);
    let fresh = workspace
        .step_history_filtered("@layer:main", None, 100, None, &filter)
        .unwrap();
    assert_eq!(fresh.observations[&next_step].len(), 2);
    assert_eq!(fresh.steps.len(), 4);
    // A fresh rebuild from a newly opened workspace gives identical results.
    let reopened = NativeWorkspace::open(&dir.0).unwrap();
    let rebuilt = reopened
        .step_history_filtered("@layer:main", None, 100, None, &filter)
        .unwrap();
    assert_eq!(fresh.observation_snapshot, rebuilt.observation_snapshot);
    assert_eq!(fresh.observations, rebuilt.observations);
    let mut cursor = first.next.unwrap();
    let original_snapshot = cursor.observation_snapshot.clone();
    cursor.observation_snapshot.space_id = layrs_core::scm::SpaceId::parse("space:other").unwrap();
    assert!(workspace
        .step_history_filtered("@layer:main", Some(&cursor), 100, None, &filter)
        .is_err());
    cursor.observation_snapshot = original_snapshot;
    cursor.observation_snapshot.digest = "0".repeat(64);
    assert!(workspace
        .step_history_filtered("@layer:main", Some(&cursor), 100, None, &filter)
        .is_err());
    let missing = StepHistoryFilter {
        actor: Some(ActorId::parse("actor:missing").unwrap()),
        ..filter.clone()
    };
    assert!(workspace
        .step_history_filtered("@layer:main", None, 100, None, &missing)
        .unwrap()
        .steps
        .is_empty());
    let changed = StepHistoryFilter {
        since: None,
        ..filter.clone()
    };
    assert!(workspace
        .step_history_filtered("@layer:main", Some(&cursor), 100, None, &changed)
        .is_err());
    let inverted = StepHistoryFilter {
        since: filter.until,
        until: Some(Timestamp::parse("2010-01-01T00:00:00Z").unwrap()),
        actor: None,
    };
    assert!(workspace
        .step_history_filtered("@layer:main", None, 100, None, &inverted)
        .is_err());
}
