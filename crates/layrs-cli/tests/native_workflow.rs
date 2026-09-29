use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Checkout(PathBuf);
impl Checkout {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "layrs-cli-native-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str]) -> Value {
        self.run_env(args, &[])
    }
    fn run_env(&self, args: &[&str], environment: &[(&str, &str)]) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(&self.0)
            .arg("--json")
            .args(args)
            .envs(environment.iter().copied())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "args={args:?} stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], true);
        value["data"].clone()
    }
}

#[cfg(windows)]
#[test]
fn cli_configured_wasm_merges_independent_edits_and_rejects_a_wrong_pin() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("code.ts"), b"a\nb\nc\n").unwrap();
    checkout.run(&["init", "Sandbox"]);
    checkout.run(&["layer", "create", "topic"]);
    fs::write(checkout.0.join("code.ts"), b"A\nb\nc\n").unwrap();
    checkout.run(&["step"]);
    checkout.run(&["layer", "use", "topic"]);
    fs::write(checkout.0.join("code.ts"), b"a\nb\nC\n").unwrap();
    checkout.run(&["step"]);
    checkout.run(&["layer", "use", "main"]);
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_layrs"))
        .parent()
        .unwrap()
        .join("layrs-lens-worker.exe");
    let digest =
        layrs_lens_sdk::sha256_digest(&fs::read(&worker).expect("build layrs-lens-worker first"));
    let before = checkout.run(&["status"]);
    let refused = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&checkout.0)
        .args(["weave", "topic", "--target", "main"])
        .env("LAYRS_LENS_WORKER", &worker)
        .env(
            "LAYRS_LENS_WORKER_SHA256",
            format!("sha256:{}", "0".repeat(64)),
        )
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(before, checkout.run(&["status"]));
    let merged = checkout.run_env(
        &["weave", "topic", "--target", "main"],
        &[
            ("LAYRS_LENS_WORKER", worker.to_str().unwrap()),
            ("LAYRS_LENS_WORKER_SHA256", &digest),
        ],
    );
    assert_eq!(merged["weave"]["materialized"], true);
    assert_eq!(fs::read(checkout.0.join("code.ts")).unwrap(), b"A\nb\nC\n");
}

#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn cli_resolve_block_guards_preview_and_preserves_manual_fragment_across_commands() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("code.ts"), b"zero\r\nsame\r\nzero\r\n").unwrap();
    checkout.run(&["init", "Blocks"]);
    checkout.run(&["layer", "create", "topic"]);
    fs::write(checkout.0.join("code.ts"), b"A\r\nsame\r\nA\r\n").unwrap();
    checkout.run(&["step"]);
    checkout.run(&["layer", "use", "topic"]);
    fs::write(checkout.0.join("code.ts"), b"B\r\nsame\r\nB\r\n").unwrap();
    checkout.run(&["step"]);
    checkout.run(&["layer", "use", "main"]);
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_layrs"))
        .parent()
        .unwrap()
        .join(if cfg!(windows) {
            "layrs-lens-worker.exe"
        } else {
            "layrs-lens-worker"
        });
    let digest =
        layrs_lens_sdk::sha256_digest(&fs::read(&worker).expect("build layrs-lens-worker first"));
    checkout.run_env(
        &["weave", "topic", "--target", "main"],
        &[
            ("LAYRS_LENS_WORKER", worker.to_str().unwrap()),
            ("LAYRS_LENS_WORKER_SHA256", &digest),
        ],
    );
    let docs = checkout.run(&["weave", "conflicts"]);
    let doc = &docs["conflicts"][0]["document"];
    let conflict = doc["conflictId"].as_str().unwrap();
    let block = doc["blocks"][0]["blockId"].as_str().unwrap();
    let term = doc["blocks"][0]["baseValues"][0]["termId"]
        .as_str()
        .unwrap();
    let manual = checkout.0.parent().unwrap().join(format!(
        "layrs-block-edit-{}-{}.txt",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&manual, "manuel 🦀\r\n").unwrap();
    let file = manual.to_str().unwrap();
    let preview = checkout.run(&[
        "resolve-block",
        conflict,
        block,
        "--file",
        file,
        "--sources",
        term,
        "--layer",
        "main",
        "--preview",
    ]);
    let op = preview["expectedOperation"].as_str().unwrap();
    let step = preview["expectedStep"].as_str().unwrap();
    let before = checkout.run(&["status"]);
    fs::write(&manual, "different preview").unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&checkout.0)
        .args([
            "resolve-block",
            conflict,
            block,
            "--file",
            file,
            "--sources",
            term,
            "--expected-operation",
            op,
            "--expected-step",
            step,
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(checkout.run(&["status"]), before);
    fs::write(&manual, "manuel 🦀\r\n").unwrap();
    let partial = checkout.run(&[
        "resolve-block",
        conflict,
        block,
        "--file",
        file,
        "--sources",
        term,
        "--expected-operation",
        op,
        "--expected-step",
        step,
    ]);
    assert_eq!(partial["resolution"]["materialized"], false);
    assert_eq!(
        fs::read(checkout.0.join("code.ts")).unwrap(),
        b"A\r\nsame\r\nA\r\n"
    );
    let docs = checkout.run(&["weave", "conflicts"]);
    let doc = &docs["conflicts"][0]["document"];
    assert_eq!(doc["resolutionState"], "partial");
    let conflict = doc["conflictId"].as_str().unwrap();
    let block = doc["blocks"][1]["blockId"].as_str().unwrap();
    let term = doc["blocks"][1]["baseValues"][0]["termId"]
        .as_str()
        .unwrap();
    let done = checkout.run(&["resolve-block", conflict, block, "--term", term]);
    assert_eq!(done["resolution"]["materialized"], true);
    let bytes = fs::read(checkout.0.join("code.ts")).unwrap();
    assert!(
        bytes == "manuel 🦀\r\nsame\r\nzero\r\n".as_bytes()
            || bytes == "zero\r\nsame\r\nmanuel 🦀\r\n".as_bytes()
    );
    fs::remove_file(manual).unwrap();
}
impl Drop for Checkout {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("preserved {}", self.0.display());
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[test]
fn cli_git_cache_resumes_without_source_rejects_wrong_digest_and_exports_only_selected_refs() {
    let mut source = Checkout::new();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&source.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    git(&["init", "--initial-branch=main"]);
    fs::write(source.0.join("file"), [0, 255, 19]).unwrap();
    git(&["add", "file"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-m",
        "cache source",
    ]);
    git(&["branch", "excluded"]);
    let cache = Checkout::new();
    let checkpoint = cache.0.join("scan.cache");
    let descriptor = cache.run(&[
        "git",
        "scan-cache",
        source.0.to_str().unwrap(),
        "--output",
        checkpoint.to_str().unwrap(),
        "--ref",
        "refs/heads/main",
    ]);
    let digest = descriptor["checkpoint"]["digest"].as_str().unwrap();
    assert_eq!(
        descriptor["checkpoint"]["references"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let moved = source.0.with_extension("unavailable");
    assert_eq!(source.0.parent(), Some(std::env::temp_dir().as_path()));
    assert_eq!(moved.parent(), source.0.parent());
    fs::rename(&source.0, &moved).unwrap();
    source.0 = moved;
    let checkout = Checkout::new();
    let dry = checkout.run(&[
        "git",
        "import-cache",
        checkpoint.to_str().unwrap(),
        "--digest",
        digest,
        "--dry-run",
    ]);
    assert_eq!(dry["report"]["commits"].as_array().unwrap().len(), 1);
    assert!(!checkout.0.join(".layrs").exists());
    checkout.run(&["init", "Cache import"]);
    let before = checkout.run(&["status"]);
    let refused = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&checkout.0)
        .args([
            "--json",
            "git",
            "import-cache",
            checkpoint.to_str().unwrap(),
            "--digest",
            &format!("sha256:{}", "0".repeat(64)),
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(before, checkout.run(&["status"]));
    let imported = checkout.run(&[
        "git",
        "import-cache",
        checkpoint.to_str().unwrap(),
        "--digest",
        digest,
    ]);
    assert_eq!(imported["import"]["alreadyImported"], false);
    let replay = checkout.run(&[
        "git",
        "import-cache",
        checkpoint.to_str().unwrap(),
        "--digest",
        digest,
    ]);
    assert_eq!(replay["import"]["alreadyImported"], true);
    assert_eq!(imported["report"], replay["report"]);
    let destination = cache.0.join("exported.git");
    let exported = checkout.run(&[
        "git",
        "export",
        destination.to_str().unwrap(),
        "--ref",
        "refs/heads/main",
    ]);
    assert_eq!(
        exported["export"]["references"].as_object().unwrap().len(),
        1
    );
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&destination)
        .args(["show", "refs/heads/main:file"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, [0, 255, 19]);
}

#[test]
fn cli_operation_history_filters_and_cursor_keep_stdout_json() {
    let checkout = Checkout::new();
    checkout.run(&["init", "History"]);
    for n in 0..3 {
        fs::write(checkout.0.join("a"), n.to_string()).unwrap();
        checkout.run(&["step"]);
    }
    let first = checkout.run(&["operation-history", "--search", "capture", "--limit", "1"]);
    assert_eq!(first["operations"].as_array().unwrap().len(), 1);
    let cursor = serde_json::to_string(&first["next"]).unwrap();
    let second = checkout.run(&[
        "operation-history",
        "--search",
        "capture",
        "--limit",
        "10",
        "--cursor",
        &cursor,
    ]);
    assert_eq!(second["anchor"], first["anchor"]);
    assert_eq!(second["operations"].as_array().unwrap().len(), 2);
    let future = checkout.run(&["operation-history", "--since", "9999-01-01T00:00:00Z"]);
    assert!(future["operations"].as_array().unwrap().is_empty());
}

#[test]
fn preview_with_pending_edits_never_captures_or_changes_checkout() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("a"), "base").unwrap();
    checkout.run(&["init", "Preview"]);
    checkout.run(&["marker", "create", "base"]);
    checkout.run(&["layer", "create", "topic"]);
    fs::write(checkout.0.join("a"), "pending edit").unwrap();
    let before = checkout.run(&["status"]);
    for args in [
        vec!["weave", "topic", "--target", "main", "--preview"],
        vec!["replay", "@marker:base", "--preview"],
        vec!["revert", "@marker:base", "--preview"],
        vec!["squash", "@marker:base", "--preview"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(&checkout.0)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("read-only preview"));
        assert_eq!(checkout.run(&["status"]), before);
        assert_eq!(
            fs::read_to_string(checkout.0.join("a")).unwrap(),
            "pending edit"
        );
    }
}

#[test]
fn cli_weave_conflict_resolution_and_history_keep_both_parents() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("hello"), b"base\n").unwrap();
    checkout.run(&["init", "Weave"]);
    checkout.run(&["layer", "create", "topic"]);
    fs::write(checkout.0.join("hello"), b"main\n").unwrap();
    checkout.run(&["step"]);
    checkout.run(&["layer", "use", "topic"]);
    fs::write(checkout.0.join("hello"), b"topic\n").unwrap();
    let topic = checkout.run(&["step"])["stepId"]
        .as_str()
        .unwrap()
        .to_owned();
    checkout.run(&["layer", "use", "main"]);
    let preview = checkout.run(&["weave", "topic", "--target", "main", "--preview"]);
    assert_eq!(preview["conflicts"].as_array().unwrap().len(), 1);
    let accepted = checkout.run(&["weave", "topic", "--target", "main"]);
    assert_eq!(accepted["weave"]["materialized"], false);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"main\n");
    let conflicts = checkout.run(&["weave", "conflicts"]);
    let conflict = &conflicts["conflicts"][0];
    let term = conflict["document"]["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["provenance"]["stepId"] == topic)
        .unwrap()["termId"]
        .as_str()
        .unwrap();
    let other = conflict["document"]["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|side| side["termId"] != term)
        .unwrap()["termId"]
        .as_str()
        .unwrap();
    let variant = checkout.run(&[
        "conflict-variant",
        conflict["conflictId"].as_str().unwrap(),
        term,
        "--layer",
        "main",
    ]);
    assert_eq!(variant["termId"], term);
    assert_eq!(variant["text"], "topic\n");
    let comparison = checkout.run(&[
        "conflict-variant",
        conflict["conflictId"].as_str().unwrap(),
        term,
        "--layer",
        "main",
        "--against",
        other,
    ]);
    assert_eq!(comparison["available"], true);
    assert_eq!(comparison["changes"][0]["path"], "hello");
    let lines = comparison["changes"][0]["textDiff"].as_array().unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line["op"] == "delete" && line["text"] == "main\n")
    );
    assert!(
        lines
            .iter()
            .any(|line| line["op"] == "insert" && line["text"] == "topic\n")
    );
    let previous = checkout.run(&["timeline"]);
    for command in [
        ["weave", "continue"],
        ["weave", "abort"],
        ["conflict", "continue"],
        ["conflict", "abort"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(&checkout.0)
            .args(command)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("resolve-term") || message.contains("restore-view"),
            "{message}"
        );
    }
    assert_eq!(checkout.run(&["timeline"]), previous);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"main\n");
    let resolved = checkout.run(&[
        "resolve-term",
        conflict["conflictId"].as_str().unwrap(),
        term,
        "--layer",
        "main",
    ]);
    assert_eq!(resolved["resolution"]["materialized"], true);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"topic\n");
    assert!(
        checkout.run(&["weave", "conflicts"])["conflicts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let history = checkout.run(&["history", "@layer:main", "--limit", "2"]);
    assert_eq!(history["steps"].as_array().unwrap().len(), 2);
    assert!(history["next"].is_object());
    checkout.run(&["undo"]);
    let replacement = Checkout::new();
    let file = replacement.0.join("resolved.bin");
    fs::write(&file, [0, 255, 10, 13]).unwrap();
    let preview = checkout.run(&[
        "resolve-file",
        conflict["conflictId"].as_str().unwrap(),
        file.to_str().unwrap(),
        "--layer",
        "main",
        "--preview",
    ]);
    assert!(preview["conflicts"].as_array().unwrap().is_empty());
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"main\n");
    checkout.run(&[
        "resolve-file",
        conflict["conflictId"].as_str().unwrap(),
        file.to_str().unwrap(),
        "--layer",
        "main",
    ]);
    assert_eq!(
        fs::read(checkout.0.join("hello")).unwrap(),
        [0, 255, 10, 13]
    );
}

#[test]
fn cli_initializes_captures_switches_restores_and_reopens_native_history() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("hello"), b"first").unwrap();
    checkout.run(&["init", "Small team"]);
    assert!(checkout.0.join(".layrs/FORMAT").exists());
    assert_eq!(checkout.run(&["status"])["dirty"], false);
    let marker = checkout.run(&["marker", "create", "baseline"]);
    checkout.run(&["layer", "create", "draft"]);
    checkout.run(&["layer", "use", "draft"]);
    fs::write(checkout.0.join("hello"), [0, 255, 42]).unwrap();
    assert_eq!(checkout.run(&["step"])["status"], "captured");
    checkout.run(&["layer", "use", "main"]);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"first");
    checkout.run(&["layer", "use", "draft"]);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), [0, 255, 42]);
    checkout.run(&[
        "restore",
        marker["marker"]["targetStepId"].as_str().unwrap(),
        "--path",
        "hello",
    ]);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"first");
    checkout.run(&["undo"]);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), [0, 255, 42]);
    checkout.run(&["redo"]);
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"first");
    checkout.run(&["release", "create", "1.0.0"]);
    assert_eq!(
        checkout.run(&["releases"])["releases"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        checkout.run(&["timeline"])["operations"]
            .as_array()
            .unwrap()
            .len()
            >= 10
    );
    assert_eq!(checkout.run(&["recover"])["recovered"], 0);
}

#[test]
fn cli_replay_revert_squash_and_named_selection_preserve_points() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("a"), b"zero\n").unwrap();
    checkout.run(&["init", "Rewrite"]);
    checkout.run(&["layer", "create", "alternate"]);
    fs::write(checkout.0.join("a"), b"one\n").unwrap();
    let first = checkout.run(&["step"])["stepId"]
        .as_str()
        .unwrap()
        .to_owned();
    checkout.run(&["marker", "create", "one"]);
    fs::write(checkout.0.join("b"), b"independent").unwrap();
    checkout.run(&["step"]);
    let preview = checkout.run(&["revert", "@marker:one", "--preview"]);
    assert_eq!(preview["preview"]["conflictCount"], 0);
    assert_eq!(fs::read(checkout.0.join("a")).unwrap(), b"one\n");
    checkout.run(&["revert", "@marker:one"]);
    assert_eq!(fs::read(checkout.0.join("a")).unwrap(), b"zero\n");
    assert_eq!(fs::read(checkout.0.join("b")).unwrap(), b"independent");
    checkout.run(&["undo"]);
    checkout.run(&["squash", &first, "--preview"]);
    checkout.run(&["squash", &first]);
    assert_eq!(
        checkout.run(&["markers"])["markers"][0]["targetStepId"],
        first
    );
    checkout.run(&["layer", "use", "alternate"]);
    checkout.run(&["replay", "@marker:one", "--preview"]);
    checkout.run(&["replay", "@marker:one"]);
    assert_eq!(fs::read(checkout.0.join("a")).unwrap(), b"one\n");
    assert!(!checkout.0.join("b").exists());
    checkout.run(&["restore", "@layer:main", "--path", "b"]);
    assert_eq!(fs::read(checkout.0.join("b")).unwrap(), b"independent");
}

#[test]
fn cli_git_import_and_export_use_real_repositories() {
    let source = Checkout::new();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&source.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    git(&["init", "--initial-branch=main"]);
    fs::write(source.0.join("raw"), [0, 255, 42]).unwrap();
    git(&["add", "raw"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-m",
        "source",
    ]);
    let checkout = Checkout::new();
    git(&["branch", "excluded"]);
    let all = checkout.run(&["git", "import", source.0.to_str().unwrap(), "--dry-run"]);
    assert_eq!(all["report"]["references"].as_array().unwrap().len(), 2);
    let selected = checkout.run(&[
        "git",
        "import",
        source.0.to_str().unwrap(),
        "--dry-run",
        "--ref",
        "refs/heads/main",
    ]);
    assert_eq!(
        selected["report"]["references"].as_array().unwrap().len(),
        1
    );
    assert!(
        !checkout.0.join(".layrs").exists(),
        "dry run must not initialize a checkout"
    );
    checkout.run(&["init", "Imported"]);
    checkout.run(&[
        "git",
        "import",
        source.0.to_str().unwrap(),
        "--ref=refs/heads/main",
    ]);
    let destination = checkout.0.join(".layrs/test-export.git");
    checkout.run(&["git", "export", destination.to_str().unwrap()]);
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(destination)
        .args(["fsck", "--strict"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_remote_refuses_insecure_origin_before_reading_credentials() {
    let checkout = Checkout::new();
    let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&checkout.0)
        .args([
            "--json",
            "remote",
            "whoami",
            "--endpoint",
            "http://example.invalid",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert!(String::from_utf8_lossy(&output.stdout).contains("HTTPS"));
}

#[test]
fn restore_preview_refuses_changed_work_and_marker_visibility_preserves_it() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("file.txt"), b"original").unwrap();
    checkout.run(&["init", "Restore guards"]);
    let marker = checkout.run(&["marker", "create", "baseline"])["marker"].clone();
    let id = marker["markerId"].as_str().unwrap();
    let step = marker["targetStepId"].as_str().unwrap();
    checkout.run(&["marker", "hide", id]);
    fs::write(checkout.0.join("file.txt"), b"uncommitted").unwrap();
    let shown = checkout.run(&["marker", "show", id]);
    assert_eq!(shown["marker"], marker);
    assert_eq!(
        fs::read(checkout.0.join("file.txt")).unwrap(),
        b"uncommitted"
    );
    assert_eq!(checkout.run(&["status"])["dirty"], true);
    let before = checkout.run(&["timeline"]);
    let preview = checkout.run(&["restore", step, "--path", "file.txt", "--preview"]);
    let human = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&checkout.0)
        .args(["restore", step, "--path", "file.txt", "--preview"])
        .output()
        .unwrap();
    assert!(human.status.success());
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("Read-only restore preview") && text.contains("--expected"));
    assert!(text.contains("-uncommitted") && text.contains("+original"));
    let first_operation = before["operations"].as_array().unwrap().last().unwrap()["operationId"]
        .as_str()
        .unwrap();
    let view = checkout.run(&["restore-view", first_operation, "--preview"]);
    assert_eq!(view["preview"]["operationId"], first_operation);
    assert_eq!(view["preview"]["layerHeads"].as_array().unwrap().len(), 1);
    assert_eq!(
        before,
        checkout.run(&["timeline"]),
        "preview must not capture pending work"
    );
    assert!(
        preview["preview"]["collisions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let guard = serde_json::to_string(&preview["preview"]["expected"]).unwrap();
    fs::write(checkout.0.join("file.txt"), b"changed after preview").unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&checkout.0)
        .args([
            "--json",
            "restore",
            step,
            "--path",
            "file.txt",
            "--expected",
            &guard,
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(before, checkout.run(&["timeline"]));
    assert_eq!(
        fs::read(checkout.0.join("file.txt")).unwrap(),
        b"changed after preview"
    );
    let fresh = checkout.run(&["restore", step, "--path", "file.txt", "--preview"]);
    let guard = serde_json::to_string(&fresh["preview"]["expected"]).unwrap();
    checkout.run(&["restore", step, "--path", "file.txt", "--expected", &guard]);
    assert_eq!(fs::read(checkout.0.join("file.txt")).unwrap(), b"original");
    assert_eq!(checkout.run(&["markers"])["markers"][0], marker);
    checkout.run(&["undo"]);
    assert_eq!(
        fs::read(checkout.0.join("file.txt")).unwrap(),
        b"changed after preview"
    );
}

#[test]
fn doctor_reports_corruption_without_repairing_or_requiring_normal_open() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("payload"), b"retained bytes").unwrap();
    checkout.run(&["init", "Diagnosis"]);
    let healthy = checkout.run(&["doctor"]);
    assert_eq!(healthy["clean"], true);
    assert_eq!(healthy["complete"], true);
    assert!(healthy["checkedObjects"].as_u64().unwrap() > 0);
    let mut pending = vec![checkout.0.join(".layrs/objects")];
    let object = loop {
        let directory = pending.pop().expect("initialized CAS contains objects");
        let mut found = None;
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else if entry.file_type().unwrap().is_file() {
                found = Some(entry.path());
                break;
            }
        }
        if let Some(path) = found {
            break path;
        }
    };
    fs::write(&object, b"intentional corruption").unwrap();
    let format = fs::read(checkout.0.join(".layrs/FORMAT")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(&checkout.0)
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["data"]["clean"], false);
    assert!(!report["data"]["findings"].as_array().unwrap().is_empty());
    assert_eq!(fs::read(&object).unwrap(), b"intentional corruption");
    assert_eq!(
        fs::read(checkout.0.join("payload")).unwrap(),
        b"retained bytes"
    );
    assert_eq!(fs::read(checkout.0.join(".layrs/FORMAT")).unwrap(), format);
}

#[test]
fn local_identity_attributes_new_captures_and_both_history_filters() {
    let checkout = Checkout::new();
    checkout.run(&["init", "Attribution"]);
    assert!(checkout.run(&["identity"])["identity"].is_null());
    checkout.run(&[
        "identity",
        "--actor",
        "actor:local-alice",
        "--device",
        "device:laptop",
        "--name",
        "Alice",
    ]);
    fs::write(checkout.0.join("file"), b"attributed\n").unwrap();
    checkout.run(&["step"]);
    let identity = checkout.run(&["identity"]);
    assert_eq!(identity["identity"]["actorId"], "actor:local-alice");
    let operations = checkout.run(&["operation-history", "--actor", "actor:local-alice"]);
    assert_eq!(operations["operations"].as_array().unwrap().len(), 1);
    let history = checkout.run(&[
        "history",
        "@layer:main",
        "--actor",
        "actor:local-alice",
        "--since",
        "2020-01-01T00:00:00Z",
    ]);
    assert_eq!(history["steps"].as_array().unwrap().len(), 1);
    assert!(
        checkout.run(&["history", "@layer:main", "--actor", "actor:nobody"])["steps"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(checkout.run(&["doctor"])["clean"], true);
}
