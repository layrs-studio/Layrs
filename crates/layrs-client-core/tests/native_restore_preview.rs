use layrs_client_core::native_scm::*;
use layrs_core::scm::{OperationKind, StepId};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("layrs-restore-preview-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn init(&self) -> (NativeWorkspace, StepId) {
        fs::write(self.0.join("tracked"), b"original\0").unwrap();
        let workspace = NativeWorkspace::initialize(&self.0, "Preview").unwrap();
        let marker = workspace.create_marker("initial", "").unwrap();
        (workspace, marker.target_step_id().clone())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn guarded_preview_allows_tracked_file_directory_transitions() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("shape"), b"leaf").unwrap();
    let (workspace, leaf_step) = fixture.init();
    fs::remove_file(fixture.0.join("shape")).unwrap();
    fs::create_dir(fixture.0.join("shape")).unwrap();
    fs::write(fixture.0.join("shape/child"), b"nested").unwrap();
    workspace.capture().unwrap();
    let directory_step = workspace
        .create_marker("directory", "")
        .unwrap()
        .target_step_id()
        .clone();
    let preview = workspace
        .preview_restore_step(&leaf_step, Some("shape"))
        .unwrap();
    assert!(preview.collisions.is_empty());
    workspace
        .restore_step_guarded(&leaf_step, Some("shape"), &preview.expected)
        .unwrap();
    assert_eq!(fs::read(fixture.0.join("shape")).unwrap(), b"leaf");
    let preview = workspace
        .preview_restore_step(&directory_step, Some("shape"))
        .unwrap();
    assert!(preview.collisions.is_empty());
    workspace
        .restore_step_guarded(&directory_step, Some("shape"), &preview.expected)
        .unwrap();
    assert_eq!(fs::read(fixture.0.join("shape/child")).unwrap(), b"nested");
}

#[test]
fn ignored_directory_descendants_are_reported_before_any_capture_or_backup() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("cache"), b"historical leaf").unwrap();
    let (workspace, step) = fixture.init();
    fs::remove_file(fixture.0.join("cache")).unwrap();
    fs::write(fixture.0.join(".layrsignore"), "cache/\n").unwrap();
    workspace.capture().unwrap();
    fs::create_dir(fixture.0.join("cache")).unwrap();
    fs::write(fixture.0.join("cache/ignored"), b"must survive").unwrap();
    fs::create_dir(fixture.0.join("cache/empty")).unwrap();
    let preview = workspace
        .preview_restore_step(&step, Some("cache"))
        .unwrap();
    assert_eq!(
        preview
            .collisions
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        ["cache/empty", "cache/ignored"]
    );
    let before = workspace.state().unwrap().operation.id().clone();
    assert!(workspace
        .restore_step_guarded(&step, Some("cache"), &preview.expected)
        .is_err());
    assert_eq!(workspace.state().unwrap().operation.id(), &before);
    assert_eq!(
        fs::read(fixture.0.join("cache/ignored")).unwrap(),
        b"must survive"
    );
    assert!(!workspace.needs_repair().unwrap());
}

#[test]
fn preview_is_read_only_respects_ignored_tracked_paths_and_matches_partial_restore() {
    let fixture = Fixture::new();
    let (workspace, step) = fixture.init();
    fs::write(fixture.0.join(".layrsignore"), "tracked\n*.tmp\n").unwrap();
    fs::write(fixture.0.join("tracked"), b"changed\xff").unwrap();
    fs::write(fixture.0.join("cache.tmp"), b"ignored data").unwrap();
    fs::write(fixture.0.join("other"), b"dirty but outside selected path").unwrap();
    let before = workspace.state().unwrap();
    let preview = workspace
        .preview_restore_step(&step, Some("tracked"))
        .unwrap();
    assert!(preview.collisions.is_empty());
    assert_eq!(preview.changes.len(), 1);
    assert_eq!(preview.changes[0].path, "tracked");
    assert_eq!(preview.changes[0].before_bytes, Some(8));
    assert!(preview.changes[0].binary);
    assert_eq!(
        workspace.state().unwrap().operation.id(),
        before.operation.id()
    );
    // Ignored unrelated content is intentionally outside the projection guard.
    fs::write(fixture.0.join("cache.tmp"), b"new ignored data").unwrap();
    workspace
        .restore_step_guarded(&step, Some("tracked"), &preview.expected)
        .unwrap();
    assert_eq!(fs::read(fixture.0.join("tracked")).unwrap(), b"original\0");
    assert_eq!(
        fs::read(fixture.0.join("other")).unwrap(),
        b"dirty but outside selected path"
    );
    assert_eq!(
        fs::read(fixture.0.join("cache.tmp")).unwrap(),
        b"new ignored data"
    );
    let operations = workspace.operations(20).unwrap();
    assert_eq!(operations[0].kind(), OperationKind::Restore);
    assert_eq!(operations[0].parent_operation_ids().len(), 1);
    assert!(operations
        .iter()
        .any(|op| op.kind() == OperationKind::CaptureWorktree));
}

#[test]
fn changed_projection_or_operation_refuses_before_automatic_capture() {
    let fixture = Fixture::new();
    let (workspace, step) = fixture.init();
    fs::write(fixture.0.join("tracked"), b"preview dirty").unwrap();
    let preview = workspace.preview_restore_step(&step, None).unwrap();
    let before = workspace.state().unwrap();
    let count = workspace.operations(20).unwrap().len();
    fs::write(fixture.0.join("tracked"), b"edited after preview").unwrap();
    assert!(matches!(
        workspace.restore_step_guarded(&step, None, &preview.expected),
        Err(NativeClientError::StaleCapture)
    ));
    assert_eq!(
        workspace.state().unwrap().operation.id(),
        before.operation.id()
    );
    assert_eq!(workspace.operations(20).unwrap().len(), count);
    assert_eq!(
        fs::read(fixture.0.join("tracked")).unwrap(),
        b"edited after preview"
    );
    let preview = workspace.preview_restore_step(&step, None).unwrap();
    workspace.capture().unwrap();
    let captured = workspace.state().unwrap();
    assert!(matches!(
        workspace.restore_step_guarded(&step, None, &preview.expected),
        Err(NativeClientError::StaleCapture)
    ));
    assert_eq!(
        workspace.state().unwrap().operation.id(),
        captured.operation.id()
    );
    assert!(!workspace.needs_repair().unwrap());
}

#[test]
fn ignored_target_collision_is_reported_and_never_captured_or_overwritten() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("cache.tmp"), b"historically tracked").unwrap();
    let (workspace, step) = fixture.init();
    fs::remove_file(fixture.0.join("cache.tmp")).unwrap();
    fs::write(fixture.0.join(".layrsignore"), "*.tmp\n").unwrap();
    workspace.capture().unwrap();
    fs::write(fixture.0.join("cache.tmp"), b"ignored work must survive").unwrap();
    let before = workspace.state().unwrap();
    let preview = workspace
        .preview_restore_step(&step, Some("cache.tmp"))
        .unwrap();
    assert_eq!(preview.collisions.len(), 1);
    assert_eq!(preview.collisions[0].path, "cache.tmp");
    assert_eq!(preview.collisions[0].reason, "ignored_untracked_target");
    assert!(workspace
        .restore_step_guarded(&step, Some("cache.tmp"), &preview.expected)
        .is_err());
    assert_eq!(
        workspace.state().unwrap().operation.id(),
        before.operation.id()
    );
    assert_eq!(
        fs::read(fixture.0.join("cache.tmp")).unwrap(),
        b"ignored work must survive"
    );
    assert!(!workspace.needs_repair().unwrap());
}

#[test]
fn preview_and_apply_actions_bind_scope_and_restore_view() {
    let fixture = Fixture::new();
    let (workspace, step) = fixture.init();
    let initial = workspace.state().unwrap().operation.id().clone();
    fs::write(fixture.0.join("tracked"), b"second").unwrap();
    workspace.capture().unwrap();
    fs::write(fixture.0.join("added"), b"capture before full View restore").unwrap();
    let preview = execute_workspace(
        &fixture.0,
        WorkspaceAction::PreviewView {
            operation: initial.clone(),
        },
    )
    .unwrap();
    assert!(preview["layerHeads"].is_array());
    let expected: RestoreExpectation = serde_json::from_value(preview["expected"].clone()).unwrap();
    let before = workspace.state().unwrap().operation.id().clone();
    assert!(execute_workspace(
        &fixture.0,
        WorkspaceAction::ApplyRestore {
            step: step.clone(),
            path: None,
            expected: expected.clone()
        }
    )
    .is_err());
    assert_eq!(workspace.state().unwrap().operation.id(), &before);
    execute_workspace(
        &fixture.0,
        WorkspaceAction::ApplyView {
            operation: initial,
            expected,
        },
    )
    .unwrap();
    assert_eq!(fs::read(fixture.0.join("tracked")).unwrap(), b"original\0");
    assert!(!fixture.0.join("added").exists());
    let preview = execute_workspace(
        &fixture.0,
        WorkspaceAction::PreviewRestore {
            step: step.clone(),
            path: None,
        },
    )
    .unwrap();
    let expected = serde_json::from_value(preview["expected"].clone()).unwrap();
    execute_workspace(
        &fixture.0,
        WorkspaceAction::ApplyRestore {
            step,
            path: None,
            expected,
        },
    )
    .unwrap();
}

#[test]
fn guard_binds_selected_path_and_added_deleted_files() {
    let fixture = Fixture::new();
    let (workspace, step) = fixture.init();
    fs::remove_file(fixture.0.join("tracked")).unwrap();
    fs::write(fixture.0.join("extra"), b"new file").unwrap();
    let preview = workspace.preview_restore_step(&step, None).unwrap();
    assert_eq!(
        preview
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.status))
            .collect::<Vec<_>>(),
        [("extra", "deleted"), ("tracked", "added")]
    );
    let before = workspace.state().unwrap().operation.id().clone();
    assert!(matches!(
        workspace.restore_step_guarded(&step, Some("tracked"), &preview.expected),
        Err(NativeClientError::StaleCapture)
    ));
    assert_eq!(workspace.state().unwrap().operation.id(), &before);
    workspace
        .restore_step_guarded(&step, None, &preview.expected)
        .unwrap();
    assert!(!fixture.0.join("extra").exists());
    assert_eq!(fs::read(fixture.0.join("tracked")).unwrap(), b"original\0");
}

#[test]
fn missing_or_corrupted_historical_chunk_refuses_preview_without_capturing_dirty_work() {
    for missing in [false, true] {
        let fixture = Fixture::new();
        let (workspace, target) = fixture.init();
        fs::write(fixture.0.join("tracked"), b"captured current").unwrap();
        workspace.capture().unwrap();
        fs::write(fixture.0.join("tracked"), b"dirty work must survive").unwrap();
        let before = workspace.state().unwrap().operation.id().clone();
        let mut directories = vec![workspace.store_root().join("objects")];
        let mut historical = None;
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    directories.push(entry.path());
                } else if entry
                    .path()
                    .components()
                    .any(|part| part.as_os_str() == "chunk_raw")
                    && fs::read(entry.path()).unwrap() == b"original\0"
                {
                    historical = Some(entry.path());
                }
            }
        }
        let chunk = historical.expect("exact historical raw chunk");
        assert!(chunk.starts_with(&fixture.0));
        if missing {
            fs::remove_file(&chunk).unwrap();
        } else {
            fs::write(&chunk, b"corruption").unwrap();
        }
        assert!(workspace.preview_restore_step(&target, None).is_err());
        assert_eq!(workspace.state().unwrap().operation.id(), &before);
        assert_eq!(
            fs::read(fixture.0.join("tracked")).unwrap(),
            b"dirty work must survive"
        );
        assert!(!workspace.needs_repair().unwrap());
    }
}
