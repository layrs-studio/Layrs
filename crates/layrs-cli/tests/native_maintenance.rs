use layrs_client_core::native_scm::NativeWorkspace;
use layrs_store_local::scm_v1::compaction::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn run(root: &Path, args: &[&str], success: bool) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{args:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], success);
    value["data"].clone()
}
struct Interrupt;
impl CompactionFaults for Interrupt {
    fn check(&self, point: CompactionFaultPoint) -> CompactionResult<()> {
        if point == CompactionFaultPoint::JournalDurable {
            return Err("simulated interruption".into());
        }
        Ok(())
    }
}

#[test]
fn cli_compacts_from_nested_checkout_and_resumes_a_fenced_store_without_losing_history() {
    let fixture = Fixture(std::env::temp_dir().join(format!(
            "layrs-cli-maintenance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    fs::create_dir(&fixture.0).unwrap();
    let nested = fixture.0.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(fixture.0.join("file"), b"initial").unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "Maintenance").unwrap();
    let marker = workspace.create_marker("initial", "").unwrap();
    fs::write(fixture.0.join("file"), b"released").unwrap();
    let release = workspace.create_release("v1", "").unwrap();
    let before = serde_json::to_value(workspace.state().unwrap()).unwrap();
    let marker_files = workspace.read_step_files(marker.target_step_id()).unwrap();
    let release_files = workspace.read_step_files(release.target_step_id()).unwrap();
    fs::write(fixture.0.join("file"), b"unrecorded").unwrap();
    let report = run(&nested, &["compact"], true);
    assert_eq!(report["historyDeleted"], false);
    assert_eq!(report["restartHistoryTraversal"], true);
    assert!(report["retainedRows"].as_u64().unwrap() > 0);
    assert!(
        Path::new(report["backupDirectory"].as_str().unwrap())
            .join("before.sqlite3")
            .is_file()
    );
    assert!(compact_metadata_with_faults(workspace.store_root(), &Interrupt).is_err());
    run(&fixture.0, &["status"], false);
    run(&nested, &["compact", "--resume"], true);
    let reopened = NativeWorkspace::open(&fixture.0).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.state().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        reopened.read_step_files(marker.target_step_id()).unwrap(),
        marker_files
    );
    assert_eq!(
        reopened.read_step_files(release.target_step_id()).unwrap(),
        release_files
    );
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"unrecorded");
    assert!(reopened.is_dirty().unwrap());
    run(&fixture.0, &["doctor"], true);
    run(&fixture.0, &["compact", "--resume"], false);
}
