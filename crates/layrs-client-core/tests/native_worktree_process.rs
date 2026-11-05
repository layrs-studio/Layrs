mod worktree_support;
use layrs_client_core::native_scm::*;
use layrs_store::scm::{ScmReader, StoreMutation};
use layrs_store_local::scm_v1::NativeScmReader;
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use worktree_support::*;

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn wait_child(mut child: Child) {
    let deadline = Instant::now() + Duration::from_secs(75);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("capture process timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn synchronized_process_worker() {
    let Some(root) = std::env::var_os("LAYRS_CAPTURE_PROCESS_ROOT") else {
        return;
    };
    let control =
        std::path::PathBuf::from(std::env::var_os("LAYRS_CAPTURE_PROCESS_CONTROL").unwrap());
    let name = std::env::var("LAYRS_CAPTURE_PROCESS_NAME").unwrap();
    let workspace = NativeWorkspace::open(root).unwrap();
    let original = workspace.state().unwrap().operation.id().clone();
    let bytes = format!("{name} independent process bytes\0");
    fs::write(
        workspace.checkout_root().unwrap().join("same.txt"),
        bytes.as_bytes(),
    )
    .unwrap();
    let CapturePlan::Change(prepared) = workspace
        .client()
        .prepare_disk_capture(
            workspace.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected changed capture")
    };
    let heads = prepared
        .batch()
        .mutations()
        .iter()
        .find_map(|mutation| match mutation {
            StoreMutation::OperationHeads(heads) => Some(heads.expected_head_ids()),
            _ => None,
        })
        .unwrap();
    let ready = serde_json::json!({"heads":heads, "parent":original, "pid":std::process::id()});
    fs::write(
        control.join(format!("{name}.ready.tmp")),
        serde_json::to_vec(&ready).unwrap(),
    )
    .unwrap();
    fs::rename(
        control.join(format!("{name}.ready.tmp")),
        control.join(format!("{name}.ready")),
    )
    .unwrap();
    // Both processes have prepared against the same frontier before either accepts.
    wait_for(&control.join("release"));
    let raced = match workspace.client().accept(&prepared) {
        Ok(_) => false,
        Err(error) => {
            assert_eq!(error.code(), "cas_mismatch", "{error}");
            workspace.capture().unwrap();
            true
        }
    };
    let state = workspace.state().unwrap();
    assert_eq!(state.operation.parent_operation_ids(), &[original]);
    fs::write(
        control.join(format!("{name}.result")),
        serde_json::to_vec(&serde_json::json!({
            "raced":raced,"operation":state.operation.id(),"step":own(&workspace).base_step_id()
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn synchronized_two_process_same_path_captures_survive_both_process_lifetimes() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let initial = own(&primary);
    let base = initial.base_step_id().unwrap().clone();
    let root_a = primary.checkout_root().unwrap();
    let root_b = secondary.checkout_root().unwrap();
    let ids = (
        primary.worktree_id().clone(),
        secondary.worktree_id().clone(),
    );
    let parents = (
        primary.state().unwrap().operation.id().clone(),
        secondary.state().unwrap().operation.id().clone(),
    );
    let control = fixture.directory("control");
    drop(primary);
    drop(secondary);
    let spawn = |name: &str, root: &Path| {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "synchronized_process_worker", "--nocapture"])
            .env("LAYRS_CAPTURE_PROCESS_ROOT", root)
            .env("LAYRS_CAPTURE_PROCESS_CONTROL", &control)
            .env("LAYRS_CAPTURE_PROCESS_NAME", name)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let a = spawn("left", &root_a);
    let b = spawn("right", &root_b);
    wait_for(&control.join("left.ready"));
    wait_for(&control.join("right.ready"));
    let read = |name: &str| {
        serde_json::from_slice::<serde_json::Value>(&fs::read(control.join(name)).unwrap()).unwrap()
    };
    let ready_a = read("left.ready");
    let ready_b = read("right.ready");
    assert_ne!(ready_a["pid"], ready_b["pid"]);
    assert_eq!(ready_a["heads"], ready_b["heads"]);
    fs::write(control.join("release"), b"accept now").unwrap();
    wait_child(a);
    wait_child(b);
    let result_a = read("left.result");
    let result_b = read("right.result");
    assert_ne!(
        result_a["raced"], result_b["raced"],
        "one exact global CAS must reject the stale prepared batch"
    );
    let primary = NativeWorkspace::open(&root_a).unwrap();
    let secondary = NativeWorkspace::open(&root_b).unwrap();
    assert_eq!(
        (primary.worktree_id(), secondary.worktree_id()),
        (&ids.0, &ids.1)
    );
    let a = primary.state().unwrap();
    let b = secondary.state().unwrap();
    assert_eq!(a.operation.parent_operation_ids(), &[parents.0]);
    assert_eq!(b.operation.parent_operation_ids(), &[parents.1]);
    assert_eq!(
        a.operation.id().as_str(),
        result_a["operation"].as_str().unwrap()
    );
    assert_eq!(
        b.operation.id().as_str(),
        result_b["operation"].as_str().unwrap()
    );
    let left = own(&primary).base_step_id().unwrap().clone();
    let right = own(&secondary).base_step_id().unwrap().clone();
    assert_ne!(left, right);
    assert_eq!(left.as_str(), result_a["step"].as_str().unwrap());
    assert_eq!(right.as_str(), result_b["step"].as_str().unwrap());
    assert_eq!(
        fs::read(root_a.join("same.txt")).unwrap(),
        b"left independent process bytes\0"
    );
    assert_eq!(
        fs::read(root_b.join("same.txt")).unwrap(),
        b"right independent process bytes\0"
    );
    let store = primary.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let heads = reader.operation_heads(a.space.id()).unwrap();
    assert_eq!(heads.len(), 2);
    assert!(heads.contains(a.operation.id()) && heads.contains(b.operation.id()));
    for step in [&left, &right] {
        assert_eq!(
            reader.get_step(step).unwrap().unwrap().parent_step_ids(),
            &[base.clone()]
        );
    }
    drop(reader);
    drop(store);
    assert!(matches!(
        primary.capture().unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    assert!(matches!(
        secondary.capture().unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    assert_clean(&primary, &[base, left, right]);
}
