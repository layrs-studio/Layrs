use layrs_client_core::native_scm::{execute_workspace, NativeWorkspace, WorkspaceAction};

#[test]
fn captured_files_remain_readable_after_reopen_without_changing_the_checkout() {
    let root = std::env::temp_dir().join(format!("layrs-step-diff-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let workspace = NativeWorkspace::initialize(&root, "Captured files").unwrap();
    std::fs::write(root.join("test.txt"), b"hello captured file").unwrap();
    execute_workspace(&root, WorkspaceAction::CaptureAutomatic).unwrap();
    let reopened = NativeWorkspace::open(&root).unwrap();
    let before = reopened.state().unwrap();
    assert!(reopened.working_diff().unwrap().is_empty());
    let step = before.view.layer_heads()[0].step_ids()[0].clone();
    let reply = execute_workspace(
        &root,
        WorkspaceAction::StepDiff {
            step: step.clone(),
            parent: None,
        },
    )
    .unwrap();
    assert_eq!(reply["stepId"], step.as_str());
    assert_eq!(reply["changes"][0]["path"], "test.txt");
    assert_eq!(reply["changes"][0]["status"], "added");
    assert_eq!(reopened.state().unwrap().operation, before.operation);
    assert_eq!(
        std::fs::read(root.join("test.txt")).unwrap(),
        b"hello captured file"
    );

    // A caller cannot substitute a Step that belongs only to another Space.
    let other = root.join("unrelated");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("private.txt"), b"private other project").unwrap();
    let foreign = NativeWorkspace::initialize(&other, "Other project").unwrap();
    let foreign_step = foreign.state().unwrap().view.layer_heads()[0].step_ids()[0].clone();
    assert!(execute_workspace(
        &root,
        WorkspaceAction::StepDiff {
            step: foreign_step.clone(),
            parent: None
        }
    )
    .is_err());
    assert!(execute_workspace(
        &root,
        WorkspaceAction::StepDiff {
            step,
            parent: Some(foreign_step)
        }
    )
    .is_err());
    assert_eq!(reopened.state().unwrap().operation, before.operation);
    drop(reopened);
    drop(workspace);
    drop(foreign);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
