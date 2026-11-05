use super::*;
use layrs_store::scm::ScmRepository;
use layrs_store_local::scm_v1::NativeScmRepository;

fn reviewed_import(temp: &Temp) -> (NativeWorkspace, ImportPlan, ExpectedBridgeState) {
    let source = temp.directory("source");
    let git = bare(&source);
    let tree = tree(&git, &[(b"readme.txt", b"reviewed content", "100644")]);
    let commit = commit(&git, &tree, &[], "reviewed commit");
    reference(&git, "refs/heads/main", &commit);
    let plan = scan_git(&source, GitBridgeLimits::default()).unwrap();
    let workspace = NativeWorkspace::initialize(temp.directory("native"), "test").unwrap();
    let expected = ExpectedBridgeState::capture(&workspace).unwrap();
    (workspace, plan, expected)
}

#[test]
fn reviewed_import_refuses_changed_target_without_installing_references() {
    let temp = Temp::new();
    let (workspace, plan, expected) = reviewed_import(&temp);
    workspace.create_marker("changed-after-review", "").unwrap();
    let before = serde_json::to_value(workspace.state().unwrap()).unwrap();
    assert!(plan.install_expected(&workspace, &expected).is_err());
    assert_eq!(
        serde_json::to_value(workspace.state().unwrap()).unwrap(),
        before
    );
    assert!(
        !workspace
            .state()
            .unwrap()
            .layers
            .iter()
            .any(|layer| layer.name().starts_with("git/"))
    );

    let other = NativeWorkspace::initialize(temp.directory("other"), "other").unwrap();
    let before = serde_json::to_value(other.state().unwrap()).unwrap();
    assert!(plan.install_expected(&other, &expected).is_err());
    assert_eq!(
        serde_json::to_value(other.state().unwrap()).unwrap(),
        before
    );
}

#[test]
fn reviewed_import_prepared_batch_keeps_original_cas_expectations() {
    let temp = Temp::new();
    let (workspace, plan, expected) = reviewed_import(&temp);
    let result = plan.install_expected_with(&workspace, &expected, |batch| {
        workspace
            .create_marker("raced-after-preparation", "")
            .unwrap();
        NativeScmRepository::new(workspace.store_root())
            .with_store_identity(workspace.store_identity())
            .with_store_id(expected.state().worktree.store_id().clone())
            .apply_batch(batch)
            .map_err(invalid)
    });
    assert!(result.is_err());
    assert!(
        !workspace
            .state()
            .unwrap()
            .layers
            .iter()
            .any(|layer| layer.name().starts_with("git/"))
    );
    assert_eq!(workspace.markers().unwrap().len(), 1);
}

#[test]
fn reviewed_export_refuses_source_content_changes_before_any_destination_write() {
    let temp = Temp::new();
    let (workspace, _, expected) = reviewed_import(&temp);
    std::fs::write(
        workspace.checkout_root().unwrap().join("changed.txt"),
        b"new content after review",
    )
    .unwrap();
    workspace.capture().unwrap();
    let destination = temp.0.join("export.git");
    assert!(
        export_workspace_expected(
            &workspace,
            &expected,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert!(!destination.exists());
}

#[test]
fn prepared_import_refuses_replaced_store_at_same_path() {
    let temp = Temp::new();
    let (workspace, plan, expected) = reviewed_import(&temp);
    let checkout = workspace.checkout_root().unwrap();
    let result = plan.install_expected_with(&workspace, &expected, |batch| {
        // All paths are inside this fixture. Preserve the original and create a
        // different Store at its old path after the CAS batch has been prepared.
        std::fs::rename(
            workspace.store_root(),
            temp.0.join("retained-original-store"),
        )
        .unwrap();
        let replacement = NativeWorkspace::initialize(&checkout, "replacement").unwrap();
        let before = serde_json::to_value(replacement.state().unwrap()).unwrap();
        let outcome = NativeScmRepository::new(workspace.store_root())
            .with_store_identity(workspace.store_identity())
            .with_store_id(expected.state().worktree.store_id().clone())
            .apply_batch(batch)
            .map_err(invalid);
        assert!(outcome.is_err());
        assert_eq!(
            serde_json::to_value(replacement.state().unwrap()).unwrap(),
            before
        );
        outcome
    });
    assert!(result.is_err());
    let replacement = NativeWorkspace::open(&checkout).unwrap();
    let destination = temp.0.join("replacement-export.git");
    assert!(
        export_workspace_expected(
            &replacement,
            &expected,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert!(!destination.exists());
}

#[test]
fn export_rechecks_after_planning_and_never_substitutes_a_fresh_view() {
    let temp = Temp::new();
    let (workspace, _, expected) = reviewed_import(&temp);
    let destination = temp.0.join("export.git");
    let mut changed = false;
    let result = export_workspace_expected(
        &workspace,
        &expected,
        &destination,
        ExportMode::History,
        GitBridgeLimits::default(),
        &mut |progress| {
            if progress.phase == BridgePhase::ExportPlan && !changed {
                changed = true;
                workspace.create_marker("changed-in-callback", "").unwrap();
            }
            true
        },
    );
    assert!(result.is_err());
    assert!(!destination.exists());
}

pub(super) fn contents(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(contents(&path));
        } else {
            files.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    files
}

#[test]
fn reviewed_resume_refuses_changed_source_and_preserves_incomplete_export() {
    let temp = Temp::new();
    let (workspace, _, expected) = reviewed_import(&temp);
    let destination = temp.0.join("export.git");
    assert!(
        export_workspace_expected(
            &workspace,
            &expected,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |event| event.phase != BridgePhase::ExportCommits
        )
        .is_err()
    );
    assert!(destination.join("layrs-export.incomplete").exists());
    let before = contents(&destination);
    workspace
        .create_marker("changed-after-interruption", "")
        .unwrap();
    assert!(
        resume_export_workspace_expected(
            &workspace,
            &expected,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert_eq!(contents(&destination), before);
    let fresh = ExpectedBridgeState::capture(&workspace).unwrap();
    assert!(
        resume_export_workspace_expected(
            &workspace,
            &fresh,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert_eq!(contents(&destination), before);
}
