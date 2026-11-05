//! Failure paths need no provider activation and must not fabricate a ready host.
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "layrs-cli-vfs-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn refused(&self, args: &[&str]) -> (i32, Value) {
        let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "multiple stdout envelopes: {text}");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["ok"], false);
        assert!(value["data"]["ticket"].is_null());
        (output.status.code().unwrap(), value)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[test]
fn vfs_parser_refuses_implicit_quiescence_and_unsupported_mutations() {
    let f = Fixture::new();
    for args in [
        vec!["vfs", "capture", "--ticket", "missing.json"],
        vec!["vfs", "export", "parent", "--ticket", "missing.json"],
        vec![
            "vfs",
            "capture",
            "--ticket",
            "missing.json",
            "--writers-quiesced=false",
        ],
        vec!["vfs", "restore", "base", "--ticket", "missing.json"],
        vec!["vfs", "start", "root", "--ready-timeout-seconds", "301"],
        vec!["vfs", "resume", "guessed-root"],
        vec!["vfs", "resume", "worktree:retained", "--step", "base"],
    ] {
        assert_eq!(f.refused(&args).0, 2);
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}

#[test]
fn resume_unknown_id_uses_explicit_source_without_creating_a_new_worktree() {
    use layrs_client_core::native_scm::NativeWorkspace;
    let f = Fixture::new();
    let source_root = f.0.join("ordinary source");
    fs::create_dir(&source_root).unwrap();
    let source = NativeWorkspace::initialize(&source_root, "Source").unwrap();
    let before = serde_json::to_value(source.list_worktrees().unwrap()).unwrap();
    let source_path = source_root.to_str().unwrap();
    f.refused(&["--space", source_path, "vfs", "resume", "worktree:unknown"]);
    let start = Instant::now();
    f.refused(&[
        "--space",
        source_path,
        "vfs",
        "resume",
        "worktree:unknown",
        "--background",
        "--ready-timeout-seconds",
        "1",
    ]);
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(
        serde_json::to_value(source.list_worktrees().unwrap()).unwrap(),
        before
    );
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}

#[test]
fn invalid_foreground_and_background_roots_never_claim_readiness_or_create_files() {
    let f = Fixture::new();
    f.refused(&["vfs", "serve", "missing root"]);
    let started = Instant::now();
    let (_, error) = f.refused(&[
        "vfs",
        "start",
        "missing root",
        "--ready-timeout-seconds",
        "1",
    ]);
    assert!(started.elapsed() < Duration::from_secs(10));
    #[cfg(windows)]
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("retained")
    );
    #[cfg(not(windows))]
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("require Windows")
    );
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}

#[test]
fn malformed_and_oversized_ticket_files_are_not_control_capabilities() {
    let f = Fixture::new();
    fs::write(f.0.join("bad.json"), b"not JSON").unwrap();
    fs::write(
        f.0.join("failed.json"),
        json!({"ok":false,"error":{"message":"failed"}}).to_string(),
    )
    .unwrap();
    fs::write(f.0.join("large.json"), vec![b' '; 64 * 1024 + 1]).unwrap();
    for ticket in ["bad.json", "failed.json", "large.json"] {
        f.refused(&["vfs", "status", "--ticket", ticket]);
        f.refused(&["vfs", "stop", "--ticket", ticket]);
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 3);
}
