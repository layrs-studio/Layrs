//! Black-box format3 CLI routing and recovery. Remote parsing deliberately uses
//! an invalid HTTP origin: these cases make no network or credential-store calls.
use layrs_core::{
    projection::{EntryState, PathSemantics, ProjectionEntry, ProjectionSnapshot},
    scm::{
        FileId, ObjectId, RepoPath, SpaceId, StepId, StoreId, TreeEntryTarget, TreeId, WorktreeId,
    },
};
use layrs_store_local::projection::{
    ProjectionIdentity, ProjectionStore, RestoreFaults, RestorePoint, RestoreSource,
};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    outer: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> (Self, ProjectionStore) {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let outer = std::env::temp_dir().join(format!(
            "layrs-cli-partial-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let root = outer.join("project");
        fs::create_dir_all(&root).unwrap();
        let identity = ProjectionIdentity {
            store_id: StoreId::parse("store:cli-partial").unwrap(),
            worktree_id: WorktreeId::parse("worktree:cli-partial").unwrap(),
            space_id: SpaceId::parse("space:cli-partial").unwrap(),
            origin: "https://example.test".into(),
            principal_id: "principal:cli".into(),
        };
        let id = format!("blake3:{}", "1".repeat(64));
        let initial = ProjectionSnapshot {
            schema_version: "layrs.projection-snapshot.v1".into(),
            base_step_id: StepId::parse(&id).unwrap(),
            base_root_tree_id: TreeId::parse(&id).unwrap(),
            observed_policy_set_id: ObjectId::parse(&id).unwrap(),
            observed_authorization_epoch: 1,
            path_semantics: PathSemantics::PortableWindows,
            entries: vec![
                ProjectionEntry {
                    path: RepoPath::parse("allowed.txt").unwrap(),
                    target: Some(TreeEntryTarget::file(FileId::from_bytes(b"base"))),
                    executable: false,
                    writable: true,
                    state: EntryState::Unavailable,
                },
                ProjectionEntry {
                    path: RepoPath::parse("private.txt").unwrap(),
                    target: Some(TreeEntryTarget::file(FileId::from_bytes(b"secret"))),
                    executable: false,
                    writable: false,
                    state: EntryState::Reserved,
                },
            ],
        };
        let mut installed = initial.clone();
        installed.entries[0].state = EntryState::Materialized;
        let store = ProjectionStore::create(&root, identity, initial).unwrap();
        store
            .install_projection(0, &installed, &mut |path, out| {
                assert_eq!(path, "allowed.txt");
                out.write_all(b"base")
            })
            .unwrap();
        (Self { outer, root }, store)
    }
    fn run(&self, args: &[&str], success: bool) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(&self.root)
            .arg("--json")
            .args(args)
            .env_remove("LAYRS_SERVER_ENDPOINT")
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["ok"], success);
        result
    }
    fn document(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.outer.join(name);
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.outer.parent() == Some(std::env::temp_dir().as_path())
        {
            fs::remove_dir_all(&self.outer).unwrap();
        }
    }
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn partial_cli_status_capture_history_and_fsck_preserve_remote_base() {
    let (f, store) = Fixture::new();
    let initial = store.state().unwrap();
    let status = f.run(&["partial", "status"], true);
    assert_eq!(status["data"]["projection"]["generation"], 1);
    assert_eq!(
        status["data"]["projection"]["snapshot"]["entries"][1]["state"],
        "reserved"
    );
    assert!(status["data"]["latestCapture"].is_null());
    fs::write(f.root.join("allowed.txt"), b"one").unwrap();
    let first = f.run(&["partial", "capture", "1"], true);
    assert_eq!(first["data"]["sequence"], 1);
    assert_eq!(first["data"]["changes"][0]["path"], "allowed.txt");
    fs::write(f.root.join("allowed.txt"), b"two").unwrap();
    f.run(&["partial", "capture", "1"], true);
    let history = f.run(&["partial", "history"], true);
    assert_eq!(history["data"]["captures"][0]["sequence"], 2);
    assert_eq!(history["data"]["captures"][1]["sequence"], 1);
    let earlier = f.run(&["partial", "history", "2"], true);
    assert_eq!(earlier["data"]["captures"].as_array().unwrap().len(), 1);
    assert_eq!(earlier["data"]["captures"][0]["sequence"], 1);
    let report = f.run(&["partial", "fsck"], true);
    assert_eq!(report["data"]["captures"], 2);
    assert_eq!(store.state().unwrap(), initial);
    f.run(&["partial", "capture", "0"], false);
    f.run(&["partial", "capture", "invalid"], false);
    f.run(&["partial", "history", "invalid"], false);
    f.run(&["status"], false); // Format3 never enters the complete Workspace.
    assert!(!f.root.join("private.txt").exists());
}

#[test]
fn partial_cli_refuses_external_reserved_collision_without_deleting_bytes() {
    let (f, store) = Fixture::new();
    fs::write(f.root.join("PRIVATE.TXT"), b"external user file").unwrap();
    f.run(&["partial", "capture", "1"], false);
    assert!(store.latest_captured_patch().unwrap().is_none());
    assert_eq!(
        fs::read(f.root.join("PRIVATE.TXT")).unwrap(),
        b"external user file"
    );
    assert_eq!(f.run(&["partial", "fsck"], true)["data"]["captures"], 0);
}

struct StopRestore;
impl RestoreFaults for StopRestore {
    fn check(&self, point: RestorePoint) -> io::Result<()> {
        if point == RestorePoint::PathQuarantined {
            Err(io::Error::other("simulated crash"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn partial_cli_resumes_a_restore_and_keeps_an_exact_inverse() {
    let (f, store) = Fixture::new();
    fs::write(f.root.join("allowed.txt"), b"captured").unwrap();
    let capture = store.capture_patch(1).unwrap();
    fs::write(f.root.join("allowed.txt"), b"unrecorded current bytes").unwrap();
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: capture.sequence,
                paths: vec![RepoPath::parse("allowed.txt").unwrap()],
            },
        )
        .unwrap();
    assert!(
        store
            .apply_restore_with_faults(&preview, &StopRestore)
            .is_err()
    );
    f.run(&["partial", "status"], false);
    let resumed = f.run(&["partial", "resume-restore"], true);
    assert_eq!(
        resumed["data"]["restoration"]["operationId"],
        preview.operation_id
    );
    assert_eq!(fs::read(f.root.join("allowed.txt")).unwrap(), b"captured");
    assert_eq!(
        f.run(&["partial", "fsck"], true)["data"]["restoreOperations"],
        1
    );
    let undo = store
        .preview_restore(
            1,
            &RestoreSource::Inverse {
                operation_id: preview.operation_id,
            },
        )
        .unwrap();
    store.apply_restore(&undo).unwrap();
    assert_eq!(
        fs::read(f.root.join("allowed.txt")).unwrap(),
        b"unrecorded current bytes"
    );
    f.run(&["partial", "resume-restore"], false);
}

#[test]
fn remote_restore_files_parse_both_raw_and_cli_envelopes_without_bypassing_https() {
    let (f, store) = Fixture::new();
    fs::write(f.root.join("allowed.txt"), b"captured").unwrap();
    let capture = store.capture_patch(1).unwrap();
    fs::write(f.root.join("allowed.txt"), b"current").unwrap();
    let local = store
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: capture.sequence,
                paths: vec![RepoPath::parse("allowed.txt").unwrap()],
            },
        )
        .unwrap();
    let preview = json!({"schemaVersion":"layrs.projection-restore-preview.v1","identity":store.identity(),"local":local});
    for (name, value) in [
        ("raw.json", preview.clone()),
        ("envelope.json", json!({"ok":true,"data":preview})),
    ] {
        let file = f.document(name, &value);
        let refused = f.run(
            &[
                "remote",
                "partial-restore-apply",
                text(&file),
                "--endpoint",
                "http://example.test",
            ],
            false,
        );
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("HTTPS"),
            "{refused}"
        );
    }
    let output = f.outer.join("preview-output.json");
    for source in [
        json!({"kind":"capture","sequence":capture.sequence,"paths":["allowed.txt"]}),
        json!({"kind":"inverse","operationId":"123-456"}),
    ] {
        let request = f.document(
            "request.json",
            &json!({"expectedGeneration":1,"source":source}),
        );
        let refused = f.run(
            &[
                "remote",
                "partial-restore-preview",
                text(&request),
                "--output",
                text(&output),
                "--endpoint",
                "http://example.test",
            ],
            false,
        );
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("HTTPS"),
            "{refused}"
        );
        assert!(!output.exists());
    }
    assert_eq!(fs::read(f.root.join("allowed.txt")).unwrap(), b"current");
    assert_eq!(store.fsck().unwrap().restore_operations, 0);
}

#[test]
fn remote_restore_rejects_action_injection_and_malformed_previews_without_output() {
    let (f, _) = Fixture::new();
    let output = f.outer.join("preview-output.json");
    let injected = f.document("injected.json", &json!({"action":"logout","expectedGeneration":1,"source":{"kind":"capture","sequence":1,"paths":["allowed.txt"]}}));
    let refused = f.run(
        &[
            "remote",
            "partial-restore-preview",
            text(&injected),
            "--output",
            text(&output),
            "--endpoint",
            "http://example.test",
        ],
        false,
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("must not supply an action")
    );
    let malformed = f.document("malformed.json", &json!({"preview":"not a preview"}));
    f.run(
        &[
            "remote",
            "partial-restore-apply",
            text(&malformed),
            "--endpoint",
            "http://example.test",
        ],
        false,
    );
    assert!(!output.exists());
    assert_eq!(fs::read(f.root.join("allowed.txt")).unwrap(), b"base");
}
