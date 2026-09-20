use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{self, NativeScmReader, NativeScmRepository};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("layrs-rewrite-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("code.ts"), b"a\nb\nc\n").unwrap();
        Self(root)
    }
    fn workspace(&self) -> NativeWorkspace {
        NativeWorkspace::initialize(&self.0, "rewrite test").unwrap()
    }
    fn text(&self, bytes: &[u8]) {
        fs::write(self.0.join("code.ts"), bytes).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn head(workspace: &NativeWorkspace) -> StepId {
    workspace
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == workspace.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone()
}
fn step(workspace: &NativeWorkspace, id: &StepId) -> Step {
    let store = scm_v1::open(workspace.store_root()).unwrap();
    NativeScmReader::new(&store)
        .unwrap()
        .get_step(id)
        .unwrap()
        .unwrap()
}
fn branch(fixture: &Fixture, workspace: &NativeWorkspace, bytes: &[u8]) -> StepId {
    workspace.create_layer("topic").unwrap();
    workspace.switch_layer("topic").unwrap();
    fixture.text(bytes);
    workspace.capture().unwrap();
    let selected = head(workspace);
    workspace.switch_layer("main").unwrap();
    selected
}
fn import_step(workspace: &NativeWorkspace, value: Step) {
    let state = workspace.state().unwrap();
    let objects = vec![CanonicalObject::Step(value)];
    let admissions = objects
        .iter()
        .map(|object| SpaceObjectAdmission::new(state.space.id().clone(), object.object_id()))
        .collect();
    let batch = WriteBatch::new(
        MutationStamp::fresh().unwrap().transaction_id,
        objects,
        vec![],
        admissions,
        vec![],
    )
    .unwrap();
    NativeScmRepository::new(workspace.store_root())
        .apply_batch(&batch)
        .unwrap();
}

#[test]
fn replay_changes_only_selected_delta_records_map_and_keeps_honest_parent() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let selected = branch(&fixture, &workspace, b"A\nb\nc\n");
    fixture.text(b"a\nb\nC\n");
    fs::write(fixture.0.join("local.bin"), [0, 255]).unwrap();
    let plan = workspace.prepare_replay(&selected, None).unwrap();
    let parent = head(&workspace);
    assert_eq!(plan.summary().target_heads_before, vec![parent.clone()]);
    assert!(plan.conflicts().is_empty());
    assert_eq!(
        plan.summary().proposed_step.parent_step_ids(),
        std::slice::from_ref(&parent)
    );
    let rewritten = workspace.apply_rewrite(&plan).unwrap();
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"A\nb\nC\n");
    assert_eq!(fs::read(fixture.0.join("local.bin")).unwrap(), [0, 255]);
    let state = workspace.state().unwrap();
    assert_eq!(state.operation.kind(), OperationKind::RewriteSteps);
    assert_eq!(state.operation.rewrite_map()[0].old_step_id(), &selected);
    assert_eq!(
        state.operation.rewrite_map()[0].new_step_ids(),
        std::slice::from_ref(&rewritten)
    );
    assert_eq!(step(&workspace, &rewritten).parent_step_ids(), [parent]);
    let topic = state
        .layers
        .iter()
        .find(|layer| layer.name() == "topic")
        .unwrap();
    assert_eq!(
        state
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == topic.id())
            .unwrap()
            .step_ids(),
        [selected]
    );
    workspace.undo().unwrap();
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"a\nb\nC\n");
    workspace.redo(None).unwrap();
    assert_eq!(head(&workspace), rewritten);
}

#[test]
fn revert_preserves_later_work_and_records_additive_revert_operation() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    fixture.text(b"A\nb\nc\n");
    workspace.capture().unwrap();
    let selected = head(&workspace);
    fixture.text(b"A\nb\nC\n");
    workspace.capture().unwrap();
    let target = head(&workspace);
    let plan = workspace.prepare_revert(&selected, None).unwrap();
    assert!(plan.conflicts().is_empty());
    let result = workspace.apply_rewrite(&plan).unwrap();
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"a\nb\nC\n");
    assert_eq!(step(&workspace, &result).parent_step_ids(), [target]);
    let state = workspace.state().unwrap();
    assert_eq!(state.operation.kind(), OperationKind::Revert);
    assert!(state.operation.rewrite_map().is_empty());
}

#[test]
fn squash_preserves_named_points_old_history_and_can_be_undone_after_restart() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let root = head(&workspace);
    fixture.text(b"first");
    workspace.capture().unwrap();
    let first = head(&workspace);
    let marker = workspace.create_marker("first", "retained").unwrap();
    let release = workspace.create_release("1.0.0", "retained").unwrap();
    fixture.text(b"second");
    workspace.capture().unwrap();
    let second = head(&workspace);
    let plan = workspace.prepare_squash(&first).unwrap();
    assert_eq!(plan.summary().rewrite_map.len(), 2);
    assert_eq!(plan.summary().proposed_step.parent_step_ids(), [root]);
    let result = workspace.apply_rewrite(&plan).unwrap();
    assert_ne!(result, second);
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"second");
    let reopened = NativeWorkspace::open(&fixture.0).unwrap();
    assert_eq!(reopened.markers().unwrap()[0], marker);
    assert_eq!(reopened.releases().unwrap()[0], release);
    assert_eq!(step(&reopened, &first).id(), &first);
    assert_eq!(step(&reopened, &second).id(), &second);
    reopened.undo().unwrap();
    assert_eq!(head(&reopened), second);
    reopened.redo(None).unwrap();
    assert_eq!(head(&reopened), result);
}

#[test]
fn conflict_variants_are_previewed_and_never_written_as_clean_files() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let selected = branch(&fixture, &workspace, b"incoming");
    fixture.text(b"local");
    let plan = workspace.prepare_replay(&selected, None).unwrap();
    let state = workspace.state().unwrap();
    assert_eq!(plan.conflicts().len(), 1);
    assert_eq!(plan.summary().conflict_count, 1);
    let document = plan.conflicts()[0].document();
    assert_eq!(document["sides"].as_array().unwrap().len(), 2);
    let result = workspace.apply_rewrite(&plan).unwrap();
    let rewritten = workspace.state().unwrap();
    assert_ne!(rewritten.operation, state.operation);
    assert_eq!(
        rewritten.view.worktrees()[0].status(),
        WorktreeStatus::Conflicted
    );
    assert_eq!(
        step(&workspace, &result).parent_step_ids(),
        [plan.summary().proposed_step.parent_step_ids()[0].clone()]
    );
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"local");
    let conflict = &plan.conflicts()[0];
    let term = conflict.document()["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provenance"]["stepId"] == selected.as_str())
        .unwrap()["termId"]
        .as_str()
        .unwrap();
    let resolution = workspace
        .preview_conflict_resolution(
            &plan.summary().target_layer_id,
            &std::collections::BTreeMap::from([(conflict.id(), ObjectId::parse(term).unwrap())]),
            Default::default(),
        )
        .unwrap();
    workspace.accept_conflict_resolution(&resolution).unwrap();
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"incoming");
    workspace.undo().unwrap();
    assert_eq!(
        workspace.state().unwrap().view.worktrees()[0].status(),
        WorktreeStatus::Conflicted
    );
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"local");
}

#[test]
fn state_or_disk_change_after_preparation_invalidates_plan_without_overwrite() {
    for metadata in [false, true] {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let selected = branch(&fixture, &workspace, b"A\nb\nc\n");
        let plan = workspace.prepare_replay(&selected, None).unwrap();
        if metadata {
            workspace.create_marker("after-plan", "").unwrap();
        } else {
            fixture.text(b"new unsaved work");
        }
        let state = workspace.state().unwrap();
        assert!(workspace.apply_rewrite(&plan).is_err());
        assert_eq!(workspace.state().unwrap().operation, state.operation);
        assert_eq!(
            fs::read(fixture.0.join("code.ts")).unwrap(),
            if metadata {
                b"a\nb\nc\n".as_slice()
            } else {
                b"new unsaved work"
            }
        );
    }
}

#[test]
fn merge_replay_requires_actual_mainline_and_squash_rejects_noncontiguous_range() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let root = head(&workspace);
    let incoming = branch(&fixture, &workspace, b"A\nb\nc\n");
    fixture.text(b"a\nb\nC\n");
    workspace.capture().unwrap();
    let other = head(&workspace);
    let merge = Step::new(
        vec![incoming.clone(), other.clone()],
        step(&workspace, &incoming).root_tree_id().clone(),
    )
    .unwrap();
    let id = merge.id().clone();
    import_step(&workspace, merge);
    assert!(workspace.prepare_replay(&id, None).is_err());
    assert!(workspace.prepare_replay(&id, Some(&root)).is_err());
    let plan = workspace.prepare_replay(&id, Some(&other)).unwrap();
    assert!(plan.conflicts().is_empty());
    workspace.apply_rewrite(&plan).unwrap();
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"A\nb\nc\n");
    assert!(workspace.prepare_squash(&incoming).is_err());
    assert!(workspace.prepare_squash(&head(&workspace)).is_err());
}

#[test]
fn replay_uses_selected_parent_delta_and_does_not_import_other_source_history() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let root = head(&workspace);
    workspace.create_layer("topic").unwrap();
    workspace.switch_layer("topic").unwrap();
    fs::write(
        fixture.0.join("unselected.txt"),
        b"earlier unrelated change",
    )
    .unwrap();
    workspace.capture().unwrap();
    fixture.text(b"A\nb\nc\n");
    workspace.capture().unwrap();
    let selected = head(&workspace);
    workspace.switch_layer("main").unwrap();
    let first = workspace.prepare_replay(&selected, None).unwrap();
    let second = workspace.prepare_replay(&selected, None).unwrap();
    assert_eq!(
        first.summary().proposed_step,
        second.summary().proposed_step
    );
    workspace.apply_rewrite(&first).unwrap();
    assert!(!fixture.0.join("unselected.txt").exists());
    assert_eq!(fs::read(fixture.0.join("code.ts")).unwrap(), b"A\nb\nc\n");
    assert_eq!(
        step(&workspace, &head(&workspace)).parent_step_ids(),
        std::slice::from_ref(&root)
    );
    assert!(workspace.prepare_revert(&root, None).is_err());
    let other_fixture = Fixture::new();
    let other = other_fixture.workspace();
    other_fixture.text(b"foreign content");
    other.capture().unwrap();
    assert!(workspace.prepare_replay(&head(&other), None).is_err());
}
