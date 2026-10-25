//! Process boundaries exercise validated secondary discovery and the shared Store.
use layrs_client_core::native_scm::{
    NativeWorkspace, WorkspaceAction, WorktreeCreationFaults, execute_workspace,
};
use layrs_store_local::scm_v1::materialize::{MaterializationFaults, MaterializationPoint};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "layrs-cli-worktrees-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir(&root).unwrap();
        root
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn run(root: &Path, args: &[&str], success: bool) -> Value {
    let result = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        result.status.success(),
        success,
        "{args:?}: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(envelope["ok"], success);
    if success {
        envelope["data"].clone()
    } else {
        envelope
    }
}
fn own_state(status: &Value) -> &Value {
    status["state"]["view"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["worktreeId"] == status["state"]["worktree"]["worktreeId"])
        .unwrap()
}

#[test]
fn secondary_commands_share_store_and_keep_independent_checkout_and_history() {
    let f = Fixture::new();
    let primary = f.directory("primary");
    let secondary = f.directory("secondary");
    fs::write(primary.join("shared.txt"), b"original").unwrap();
    run(&primary, &["init", "Worktrees"], true);
    let initial = run(&primary, &["status"], true);
    let step = own_state(&initial)["baseStepId"].as_str().unwrap();
    let created = run(
        &primary,
        &[
            "worktree",
            "create",
            text(&secondary),
            "--layer",
            "main",
            "--step",
            step,
        ],
        true,
    );
    assert_eq!(
        created["state"]["worktree"]["storeId"],
        initial["state"]["worktree"]["storeId"]
    );
    assert_ne!(
        created["worktreeId"],
        initial["state"]["worktree"]["worktreeId"]
    );
    assert_eq!(created["state"]["layers"].as_array().unwrap().len(), 1);
    assert!(secondary.join(".layrs").is_file());
    assert_eq!(fs::read(secondary.join("shared.txt")).unwrap(), b"original");
    let opened = run(&f.0, &["worktree", "open", text(&secondary)], true);
    assert_eq!(opened, created);
    assert_eq!(
        run(&secondary, &["worktree", "list"], true)["worktrees"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        run(&secondary, &["worktree", "list"], true)["currentWorktreeId"],
        created["worktreeId"]
    );
    let nested = secondary.join("nested");
    fs::create_dir(&nested).unwrap();
    assert_eq!(
        run(&nested, &["status"], true)["state"]["worktree"]["worktreeId"],
        created["worktreeId"]
    );
    run(&nested, &["layer", "create", "task"], true);
    run(&secondary, &["layer", "use", "task"], true);
    fs::write(secondary.join("shared.txt"), b"task change").unwrap();
    let captured = run(&nested, &["step"], true);
    assert_ne!(captured["stepId"], step);
    assert_eq!(run(&primary, &["status"], true), initial);
    assert_eq!(fs::read(primary.join("shared.txt")).unwrap(), b"original");
    assert_eq!(run(&secondary, &["status"], true)["dirty"], false);
    run(&secondary, &["layer", "use", "main"], true);
    assert_eq!(fs::read(secondary.join("shared.txt")).unwrap(), b"original");
    run(&secondary, &["layer", "use", "task"], true);
    assert_eq!(
        fs::read(secondary.join("shared.txt")).unwrap(),
        b"task change"
    );
    let human = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&secondary)
        .args(["worktree", "list"])
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stdout).contains(created["worktreeId"].as_str().unwrap())
    );
}

#[test]
fn exact_layer_ids_from_another_private_view_work_in_cli_and_desktop() {
    let f = Fixture::new();
    let primary_root = f.directory("primary");
    let secondary_root = f.directory("secondary");
    let primary = NativeWorkspace::initialize(&primary_root, "Worktrees").unwrap();
    let initial = primary.state().unwrap();
    let own = initial
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == primary.worktree_id())
        .unwrap();
    let base = own.base_step_id().unwrap();
    let secondary = primary
        .create_worktree(&secondary_root, own.active_layer_id().unwrap(), base)
        .unwrap();
    let layer = primary.create_layer("private-task").unwrap();
    assert!(
        !secondary
            .state()
            .unwrap()
            .layers
            .iter()
            .any(|visible| visible.id() == layer.id())
    );
    let before = run(&secondary_root, &["status"], true);
    let rejected_root = f.directory("rejected");
    for selector in [layer.name(), "@layer:private-task", "layer:missing"] {
        let request = WorkspaceAction::CreateWorktree {
            path: text(&rejected_root).into(),
            layer: selector.into(),
            step: base.to_string(),
        };
        assert!(execute_workspace(&secondary_root, request).is_err());
        run(
            &secondary_root,
            &[
                "worktree",
                "create",
                text(&rejected_root),
                "--layer",
                selector,
                "--step",
                base.as_str(),
            ],
            false,
        );
        assert_eq!(fs::read_dir(&rejected_root).unwrap().count(), 0);
    }
    let desktop_root = f.directory("desktop");
    let desktop = execute_workspace(
        &secondary_root,
        WorkspaceAction::CreateWorktree {
            path: text(&desktop_root).into(),
            layer: layer.id().to_string(),
            step: base.to_string(),
        },
    )
    .unwrap();
    assert_eq!(own_state(&desktop)["activeLayerId"], layer.id().as_str());
    assert_eq!(own_state(&desktop)["baseStepId"], base.as_str());
    let cli_root = f.directory("cli");
    let cli = run(
        &secondary_root,
        &[
            "worktree",
            "create",
            text(&cli_root),
            "--layer",
            layer.id().as_str(),
            "--step",
            base.as_str(),
        ],
        true,
    );
    assert_eq!(own_state(&cli)["activeLayerId"], layer.id().as_str());
    assert_eq!(own_state(&cli)["baseStepId"], base.as_str());
    assert_eq!(run(&secondary_root, &["status"], true), before);
}

#[test]
fn create_rejects_bad_selectors_flags_and_targets_without_registering_worktrees() {
    let f = Fixture::new();
    let primary = f.directory("primary");
    let target = f.directory("target");
    run(&primary, &["init", "Worktrees"], true);
    for suffix in [
        vec!["--layer", "main"],
        vec!["--layer", "unknown", "--step", "@layer:main"],
        vec!["--layer", "main", "--step", "@layer:unknown"],
        vec![
            "--layer",
            "main",
            "--step",
            "@layer:main",
            "--layer",
            "main",
        ],
        vec![
            "--layer",
            "main",
            "--step",
            "@layer:main",
            "--step",
            "@layer:main",
        ],
        vec!["--layer", "main", "--step", "@layer:main", "--unknown"],
    ] {
        let mut args = vec!["worktree", "create", text(&target)];
        args.extend(suffix);
        run(&primary, &args, false);
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        assert_eq!(
            run(&primary, &["worktree", "list"], true)["worktrees"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    fs::write(target.join("valuable"), b"untouched").unwrap();
    run(
        &primary,
        &[
            "worktree",
            "create",
            text(&target),
            "--layer",
            "main",
            "--step",
            "@layer:main",
        ],
        false,
    );
    assert_eq!(fs::read(target.join("valuable")).unwrap(), b"untouched");
    run(&primary, &["worktree", "resume"], false);
}

#[test]
fn copied_and_malformed_locators_are_errors_and_desktop_uses_the_same_contract() {
    let f = Fixture::new();
    let primary = f.directory("primary");
    let secondary = f.directory("secondary");
    let copied = f.directory("copied");
    run(&primary, &["init", "Worktrees"], true);
    let action: WorkspaceAction = serde_json::from_value(
        json!({"action":"createWorktree","path":secondary,"layer":"main","step":"@layer:main"}),
    )
    .unwrap();
    let created = execute_workspace(&primary, action).unwrap();
    assert_eq!(
        execute_workspace(
            &copied,
            WorkspaceAction::OpenWorktree {
                path: text(&secondary).into()
            }
        )
        .unwrap(),
        created
    );
    let list = execute_workspace(&secondary, WorkspaceAction::ListWorktrees).unwrap();
    assert_eq!(list["currentWorktreeId"], created["worktreeId"]);
    assert_eq!(list["worktrees"].as_array().unwrap().len(), 2);
    fs::copy(secondary.join(".layrs"), copied.join(".layrs")).unwrap();
    run(&copied, &["status"], false);
    run(&primary, &["worktree", "open", text(&copied)], false);
    fs::write(copied.join(".layrs"), b"not a locator").unwrap();
    run(&copied, &["status"], false);
    assert!(serde_json::from_value::<WorkspaceAction>(json!({"action":"createWorktree","path":secondary,"layer":"main","step":"@layer:main","createLayer":true})).is_err());
}

struct Stop;
struct StopBeforeLocatorPublication;
impl WorktreeCreationFaults for StopBeforeLocatorPublication {
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
    fn before_locator_publication(&self) -> io::Result<()> {
        Err(io::Error::other("interrupted before locator publication"))
    }
}

#[test]
fn nested_resume_discovers_staged_reservation_and_preserves_its_exact_base() {
    let f = Fixture::new();
    let primary = f.directory("primary");
    let secondary = f.directory("secondary");
    fs::write(primary.join("shared.txt"), b"base bytes").unwrap();
    let workspace = NativeWorkspace::initialize(&primary, "Worktrees").unwrap();
    let state = workspace.state().unwrap();
    let own = state
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == workspace.worktree_id())
        .unwrap();
    assert!(
        workspace
            .create_worktree_with_creation_faults(
                &secondary,
                own.active_layer_id().unwrap(),
                own.base_step_id().unwrap(),
                &layrs_store_local::scm_v1::materialize::NoMaterializationFaults,
                &StopBeforeLocatorPublication,
            )
            .is_err()
    );
    assert!(!secondary.join(".layrs").exists());
    let reservation: Value =
        serde_json::from_slice(&fs::read(secondary.join(".layrs.reservation")).unwrap()).unwrap();
    assert_eq!(workspace.list_worktrees().unwrap().len(), 1);
    let nested = secondary.join("nested/deep");
    fs::create_dir_all(&nested).unwrap();
    fs::write(primary.join("shared.txt"), b"new source bytes").unwrap();
    run(&primary, &["step"], true);
    let resumed = run(&nested, &["worktree", "resume"], true);
    assert_eq!(resumed["worktreeId"], reservation["worktreeId"]);
    assert_eq!(
        own_state(&resumed)["baseStepId"],
        own.base_step_id().unwrap().as_str()
    );
    assert_eq!(own_state(&resumed)["status"], "ready");
    assert!(secondary.join(".layrs").is_file());
    assert!(!secondary.join(".layrs.reservation").exists());
    assert_eq!(
        fs::read(secondary.join("shared.txt")).unwrap(),
        b"base bytes"
    );
    assert_eq!(
        fs::read(primary.join("shared.txt")).unwrap(),
        b"new source bytes"
    );
    assert_eq!(run(&nested, &["status"], true)["dirty"], false);
}

struct StopAfterLocator;
impl WorktreeCreationFaults for StopAfterLocator {
    fn after_locator(&self) -> io::Result<()> {
        Err(io::Error::other("interrupted before registration"))
    }
}

#[test]
fn resume_registers_durable_intent_even_when_ordinary_open_fails() {
    let f = Fixture::new();
    let primary = f.directory("primary");
    fs::write(primary.join("shared.txt"), b"base bytes").unwrap();
    run(&primary, &["init", "Worktrees"], true);
    let workspace = NativeWorkspace::open(&primary).unwrap();
    let state = workspace.state().unwrap();
    let own = state
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == workspace.worktree_id())
        .unwrap();
    for (directory, desktop) in [("cli-resume", false), ("desktop-resume", true)] {
        let secondary = f.directory(directory);
        assert!(
            workspace
                .create_worktree_with_creation_faults(
                    &secondary,
                    own.active_layer_id().unwrap(),
                    own.base_step_id().unwrap(),
                    &layrs_store_local::scm_v1::materialize::NoMaterializationFaults,
                    &StopAfterLocator,
                )
                .is_err()
        );
        assert!(secondary.join(".layrs").is_file());
        run(&secondary, &["status"], false);
        let metadata_path = workspace.store_root().join("metadata/layrs.sqlite3");
        let metadata_before = fs::read(&metadata_path).unwrap();
        for args in [
            &["compact"][..],
            &["compact", "--resume"][..],
            &["recover"][..],
        ] {
            run(&secondary, args, false);
            assert_eq!(fs::read(&metadata_path).unwrap(), metadata_before);
        }
        let resumed = if desktop {
            execute_workspace(&secondary, WorkspaceAction::ResumeWorktreeCreation).unwrap()
        } else {
            run(&secondary, &["worktree", "resume"], true)
        };
        assert_eq!(own_state(&resumed)["status"], "ready");
        assert_eq!(
            fs::read(secondary.join("shared.txt")).unwrap(),
            b"base bytes"
        );
        assert_eq!(run(&secondary, &["worktree", "resume"], true), resumed);
    }
}

impl MaterializationFaults for Stop {
    fn check(&self, point: MaterializationPoint) -> io::Result<()> {
        if point == MaterializationPoint::AfterJournal {
            Err(io::Error::other("interrupted creation"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn interrupted_secondary_creation_recovers_and_resumes_through_cli() {
    let f = Fixture::new();
    let primary = f.directory("primary");
    let secondary = f.directory("secondary");
    fs::write(primary.join("shared.txt"), b"base bytes").unwrap();
    run(&primary, &["init", "Worktrees"], true);
    let workspace = NativeWorkspace::open(&primary).unwrap();
    let state = workspace.state().unwrap();
    let own = state
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == workspace.worktree_id())
        .unwrap();
    assert!(
        workspace
            .create_worktree_with_faults(
                &secondary,
                own.active_layer_id().unwrap(),
                own.base_step_id().unwrap(),
                &Stop
            )
            .is_err()
    );
    assert_eq!(run(&secondary, &["status"], true)["needsRepair"], true);
    run(&secondary, &["worktree", "resume"], false);
    assert_eq!(run(&primary, &["recover"], true)["recovered"], 0);
    assert_eq!(run(&secondary, &["status"], true)["needsRepair"], true);
    assert_eq!(run(&secondary, &["recover"], true)["recovered"], 1);
    let resumed = run(&secondary, &["worktree", "resume"], true);
    assert_eq!(own_state(&resumed)["status"], "ready");
    assert_eq!(
        fs::read(secondary.join("shared.txt")).unwrap(),
        b"base bytes"
    );
    assert_eq!(run(&secondary, &["status"], true)["dirty"], false);
}
