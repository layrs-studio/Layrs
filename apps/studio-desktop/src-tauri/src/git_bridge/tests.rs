use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let id = format!(
            "layrs-desktop-git-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(id);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir(&path).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let parent = std::env::temp_dir().canonicalize().ok();
        let resolved = self.0.canonicalize().ok();
        if let (Some(parent), Some(resolved)) = (parent, resolved) {
            if resolved.parent() == Some(parent.as_path())
                && resolved
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("layrs-desktop-git-")
            {
                let _ = std::fs::remove_dir_all(resolved);
            }
        }
    }
}

fn git(root: &Path, arguments: &[&str], input: &[u8]) -> String {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    let mut process = command.spawn().unwrap();
    process.stdin.take().unwrap().write_all(input).unwrap();
    let output = process.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn source(temp: &Temp, name: &str, special: &str) -> PathBuf {
    let source = temp.directory(name);
    git(&source, &["init", "--bare", "--template=", "."], b"");
    let content = if special == "lfs" {
        b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 3\n".as_slice()
    } else {
        b"reviewed content"
    };
    let blob = git(&source, &["hash-object", "-w", "--stdin"], content);
    let tree = git(
        &source,
        &["mktree"],
        format!("100644 blob {blob}\treadme.txt\n").as_bytes(),
    );
    let commit = |tree: &str| {
        git(&source, &["hash-object", "-t", "commit", "-w", "--stdin"], format!("tree {tree}\nauthor Test <test@example.invalid> 1000000000 +0000\ncommitter Test <test@example.invalid> 1000000000 +0000\n\nfixture\n").as_bytes())
    };
    let mut id = commit(&tree);
    if special == "submodule" {
        let tree = git(
            &source,
            &["mktree"],
            format!("160000 commit {id}\tsubmodule\n").as_bytes(),
        );
        id = commit(&tree);
    }
    git(&source, &["update-ref", "refs/heads/main", &id], b"");
    source
}
fn call(state: &GitBridgeState, root: &Path, request: Value) -> Result<Value, String> {
    execute(state, root, serde_json::from_value(request).unwrap())
}
fn fixture() -> (Temp, PathBuf, GitBridgeState, Value) {
    let temp = Temp::new();
    let root = temp.directory("native");
    NativeWorkspace::initialize(&root, "disposable").unwrap();
    let source = source(&temp, "source.git", "ordinary");
    let state = GitBridgeState::default();
    let checkpoint = call(&state, &root, json!({"action":"prepareImport","source":source,"checkpoint":temp.0.join("import.scan"),"references":[]})).unwrap();
    (temp, root, state, checkpoint["checkpoint"].clone())
}
fn review(state: &GitBridgeState, root: &Path, checkpoint: &Value) -> Value {
    call(state, root, json!({"action":"reviewImport","checkpoint":checkpoint["path"],"digest":checkpoint["digest"]})).unwrap()
}

#[test]
fn import_requires_native_review_and_revalidates_digest_path_and_target() {
    let (temp, root, state, checkpoint) = fixture();
    for change in ["digest", "path", "target"] {
        let preview = review(&state, &root, &checkpoint);
        let other = temp.0.join("other.scan");
        std::fs::copy(checkpoint["path"].as_str().unwrap(), &other).unwrap();
        let request = json!({"action":"applyImport","reviewId":preview["reviewId"],
            "checkpoint":if change=="path" {json!(other)} else {checkpoint["path"].clone()},
            "digest":if change=="digest" {json!(format!("sha256:{}","0".repeat(64)))} else {checkpoint["digest"].clone()}});
        if change == "target" {
            NativeWorkspace::open(&root)
                .unwrap()
                .create_marker("changed", "")
                .unwrap();
        }
        let before =
            serde_json::to_value(NativeWorkspace::open(&root).unwrap().state().unwrap()).unwrap();
        assert!(call(&state, &root, request).is_err());
        assert_eq!(
            serde_json::to_value(NativeWorkspace::open(&root).unwrap().state().unwrap()).unwrap(),
            before
        );
    }
}

#[test]
fn modified_checkpoint_is_refused_before_import_and_valid_review_installs() {
    let (_temp, root, state, checkpoint) = fixture();
    let preview = review(&state, &root, &checkpoint);
    let path = Path::new(checkpoint["path"].as_str().unwrap());
    let original = std::fs::read(path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"tampered")
        .unwrap();
    let before =
        serde_json::to_value(NativeWorkspace::open(&root).unwrap().state().unwrap()).unwrap();
    assert!(call(&state, &root, json!({"action":"applyImport","reviewId":preview["reviewId"],"checkpoint":path,"digest":checkpoint["digest"]})).is_err());
    assert_eq!(
        serde_json::to_value(NativeWorkspace::open(&root).unwrap().state().unwrap()).unwrap(),
        before
    );
    std::fs::write(path, original).unwrap();
    let preview = review(&state, &root, &checkpoint);
    let result = call(&state, &root, json!({"action":"applyImport","reviewId":preview["reviewId"],"checkpoint":path,"digest":checkpoint["digest"]})).unwrap();
    assert_eq!(result["import"]["alreadyImported"], false);
    assert!(
        NativeWorkspace::open(&root)
            .unwrap()
            .state()
            .unwrap()
            .layers
            .iter()
            .any(|layer| layer.name() == "git/main")
    );
    assert!(!root.join("readme.txt").exists());
}

#[test]
fn export_review_is_read_only_and_changed_destination_or_mode_cannot_write() {
    let (temp, root, state, _) = fixture();
    let destination = temp.0.join("output.git");
    for changed_mode in [false, true] {
        let preview = call(
            &state,
            &root,
            json!({"action":"reviewExport","destination":destination,"mode":{"kind":"history"}}),
        )
        .unwrap();
        assert_eq!(preview["review"]["contentPreview"], "unavailable");
        assert!(!destination.exists());
        let wrong = temp.0.join("wrong.git");
        assert!(call(&state, &root, json!({"action":"export","reviewId":preview["reviewId"],
            "destination":if changed_mode {destination.clone()} else {wrong.clone()},
            "mode":if changed_mode {json!({"kind":"selected_history","references":["refs/heads/main"]})} else {json!({"kind":"history"})}})).is_err());
        assert!(!destination.exists() && !wrong.exists());
    }
}

#[test]
fn exact_snapshot_review_refuses_another_admitted_step_or_export_mode_before_writes() {
    let (temp, root, state, _) = fixture();
    let workspace = NativeWorkspace::open(&root).unwrap();
    let before = workspace
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == workspace.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    std::fs::write(root.join("changed"), b"another admitted Step").unwrap();
    workspace.capture().unwrap();
    let after = workspace
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == workspace.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    assert_ne!(before, after);
    let destination = temp.0.join("snapshot.git");
    for changed in [
        json!({"kind":"snapshot","stepId":after,"reference":"refs/heads/exact"}),
        json!({"kind":"history"}),
    ] {
        let preview = call(&state, &root, json!({"action":"reviewExport","destination":destination,"mode":{"kind":"snapshot","stepId":before,"reference":"refs/heads/exact"}})).unwrap();
        assert!(call(&state, &root, json!({"action":"export","reviewId":preview["reviewId"],"destination":destination,"mode":changed})).is_err());
        assert!(!destination.exists());
    }
}

#[test]
fn export_and_resume_are_explicit_and_invalid_recovery_creates_nothing() {
    let (temp, root, state, _) = fixture();
    let destination = temp.0.join("output.git");
    let preview = call(
        &state,
        &root,
        json!({"action":"reviewExport","destination":destination,"mode":{"kind":"history"}}),
    )
    .unwrap();
    assert!(call(&state, &root, json!({"action":"resumeExport","reviewId":preview["reviewId"],"destination":destination,"mode":{"kind":"history"}})).is_err());
    assert!(!destination.exists());
    let preview = call(
        &state,
        &root,
        json!({"action":"reviewExport","destination":destination,"mode":{"kind":"history"}}),
    )
    .unwrap();
    let result = call(&state, &root, json!({"action":"export","reviewId":preview["reviewId"],"destination":destination,"mode":{"kind":"history"}})).unwrap();
    assert!(
        result["export"]["references"]
            .as_object()
            .is_some_and(|refs| !refs.is_empty())
    );
    assert!(!destination.join("layrs-export.incomplete").exists());
    let preview = call(
        &state,
        &root,
        json!({"action":"reviewExport","destination":destination,"mode":{"kind":"history"}}),
    )
    .unwrap();
    assert!(call(&state, &root, json!({"action":"resumeExport","reviewId":preview["reviewId"],"destination":destination,"mode":{"kind":"history"}})).is_err());
}

#[test]
fn lfs_and_submodule_refusals_reach_the_desktop_without_native_mutations() {
    let temp = Temp::new();
    let root = temp.directory("native");
    NativeWorkspace::initialize(&root, "disposable").unwrap();
    let state = GitBridgeState::default();
    let before =
        serde_json::to_value(NativeWorkspace::open(&root).unwrap().state().unwrap()).unwrap();
    for special in ["lfs", "submodule"] {
        let source = source(&temp, special, special);
        let prepared = call(
            &state,
            &root,
            json!({"action":"prepareImport","source":source,"checkpoint":temp.0.join(format!("{special}.scan")),"references":[]}),
        );
        let result = prepared.and_then(|value| call(&state, &root, json!({"action":"reviewImport","checkpoint":value["checkpoint"]["path"],"digest":value["checkpoint"]["digest"]})));
        let error = result.unwrap_err();
        assert!(error.contains("Unsupported"), "{error}");
        assert_eq!(
            serde_json::to_value(NativeWorkspace::open(&root).unwrap().state().unwrap()).unwrap(),
            before
        );
    }
}

#[test]
fn ipc_rejects_unknown_or_optimistic_fields_and_reviews_are_bounded() {
    assert!(serde_json::from_value::<GitBridgeAction>(
        json!({"action":"applyImport","reviewId":"x","checkpoint":"x","digest":"x","approved":true})
    )
    .is_err());
    assert!(serde_json::from_value::<GitBridgeAction>(json!({"action":"reviewExport","destination":"x","mode":{"kind":"snapshot","stepId":"@layer:main","reference":"refs/heads/main"}})).is_err());
    let (temp, root, state, _) = fixture();
    let mut first = String::new();
    for index in 0..17 {
        let review = call(&state, &root, json!({"action":"reviewExport","destination":temp.0.join(format!("{index}.git")),"mode":{"kind":"history"}})).unwrap();
        if index == 0 {
            first = review["reviewId"].as_str().unwrap().into();
        }
    }
    assert!(state.take(&first).is_err());
    assert_eq!(state.0.lock().unwrap().entries.len(), 16);
}

#[test]
fn export_scope_is_bounded_and_normalized_before_registry_retention() {
    let (temp, root, state, _) = fixture();
    let destination = temp.0.join("output.git");
    for mode in [
        json!({"kind":"selected_history","references":[]}),
        json!({"kind":"selected_history","references":["refs/heads/main","refs/heads/main"]}),
        json!({"kind":"selected_history","references":[format!("refs/heads/{}","x".repeat(16385))]}),
        json!({"kind":"selected_history","references":vec!["refs/heads/x";100001]}),
        json!({"kind":"selected_history","references":["refs/heads/a..b"]}),
        json!({"kind":"snapshot","stepId":format!("blake3:{}","a".repeat(64)),"reference":format!("refs/heads/{}","x".repeat(16385))}),
    ] {
        assert!(
            call(
                &state,
                &root,
                json!({"action":"reviewExport","destination":destination,"mode":mode})
            )
            .is_err()
        );
        assert!(state.0.lock().unwrap().entries.is_empty());
        assert!(!destination.exists());
    }
    let result = call(&state, &root, json!({"action":"reviewExport","destination":destination,"mode":{"kind":"selected_history","references":["refs/tags/v1","refs/heads/main"]}})).unwrap();
    assert_eq!(
        result["review"]["mode"]["references"],
        json!(["refs/heads/main", "refs/tags/v1"])
    );
    assert_eq!(state.0.lock().unwrap().entries.len(), 1);
}

#[test]
fn export_parent_replacement_after_native_review_does_not_write_to_replacement() {
    let (temp, root, state, _) = fixture();
    let parent = temp.directory("parent");
    let destination = parent.join("output.git");
    let review = call(
        &state,
        &root,
        json!({"action":"reviewExport","destination":destination,"mode":{"kind":"history"}}),
    )
    .unwrap();
    let original = temp.0.join("retained-parent");
    std::fs::rename(&parent, &original).unwrap();
    std::fs::create_dir(&parent).unwrap();
    assert!(call(&state, &root, json!({"action":"export","reviewId":review["reviewId"],"destination":destination,"mode":{"kind":"history"}})).is_err());
    assert!(!destination.exists());
    assert!(!original.join("output.git").exists());
}
