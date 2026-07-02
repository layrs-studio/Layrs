use serde_json::Value;
use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TestSpace {
    root: PathBuf,
    space: PathBuf,
    appdata: PathBuf,
    xdg: PathBuf,
    home: PathBuf,
    cleanup: bool,
}

impl TestSpace {
    fn new(label: &str) -> Self {
        let unique = format!(
            "layrs-cli-local-data-safety-{label}-{}-{}-{}",
            std::process::id(),
            unix_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let root = std::env::temp_dir().join(unique);
        let space = root.join("space");
        let appdata = root.join("appdata");
        let xdg = root.join("xdg");
        let home = root.join("home");

        fs::create_dir_all(&space).expect("create test space");
        fs::create_dir_all(&appdata).expect("create APPDATA");
        fs::create_dir_all(&xdg).expect("create XDG_CONFIG_HOME");
        fs::create_dir_all(&home).expect("create HOME");
        fs::create_dir_all(root.join("tmp")).expect("create test tmp");

        Self {
            root,
            space,
            appdata,
            xdg,
            home,
            cleanup: false,
        }
    }

    fn space_arg(&self) -> String {
        self.space.display().to_string()
    }

    fn pass(mut self) {
        self.cleanup = true;
    }
}

impl Drop for TestSpace {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_dir_all(&self.root);
        } else {
            eprintln!(
                "local_data_safety preserved failing test directory: {}",
                self.root.display()
            );
        }
    }
}

fn run_ok<I, S>(space: &TestSpace, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<OsString>>();

    let mut command = Command::new(env!("CARGO_BIN_EXE_layrs"));
    command.args(&args);
    command.current_dir(&space.space);
    command.env_clear();
    preserve_env(&mut command, "PATH");
    preserve_env(&mut command, "Path");
    preserve_env(&mut command, "PATHEXT");
    preserve_env(&mut command, "SystemRoot");
    preserve_env(&mut command, "SYSTEMROOT");
    preserve_env(&mut command, "WINDIR");
    preserve_env(&mut command, "COMSPEC");
    preserve_env(&mut command, "ComSpec");
    command.env("APPDATA", &space.appdata);
    command.env("LOCALAPPDATA", space.root.join("localappdata"));
    command.env("XDG_CONFIG_HOME", &space.xdg);
    command.env("HOME", &space.home);
    command.env("USERPROFILE", &space.home);
    command.env("TMP", space.root.join("tmp"));
    command.env("TEMP", space.root.join("tmp"));
    command.env("TMPDIR", space.root.join("tmp"));
    command.env("NO_COLOR", "1");

    let output = command.output().unwrap_or_else(|error| {
        panic!(
            "failed to run layrs with args {:?} in {}: {error}",
            args,
            space.space.display()
        )
    });
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(
        output.status.success(),
        "layrs {:?} failed with status {:?}\nstdout:\n{}\nstderr:\n{}\nspace: {}",
        args,
        output.status.code(),
        stdout,
        stderr,
        space.root.display()
    );

    stdout
}

fn read_json(output: &str) -> Value {
    let envelope: Value = serde_json::from_str(output)
        .unwrap_or_else(|error| panic!("invalid JSON output: {error}\n{output}"));
    assert_eq!(
        envelope["ok"],
        Value::Bool(true),
        "CLI returned error JSON: {envelope}"
    );
    envelope["data"].clone()
}

fn run_json<I, S>(space: &TestSpace, args: I) -> Value
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    read_json(&run_ok(space, args))
}

fn space_size_bytes(path: &Path) -> u64 {
    if path.is_file() {
        return fs::metadata(path).expect("metadata").len();
    }

    fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read_dir {}: {error}", path.display()))
        .map(|entry| {
            let entry = entry.expect("directory entry");
            space_size_bytes(&entry.path())
        })
        .sum()
}

fn assert_file(root: &Path, relative: &str, expected: &str) {
    let path = root.join(relative);
    let actual = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "file content mismatch at {}",
        path.display()
    );
}

#[test]
fn init_existing_folder_preserves_content_and_exposes_initial_pending_step() {
    let space = TestSpace::new("init-existing");
    fs::write(space.space.join("hello.txt"), "hello before layrs\n").expect("write hello");

    let init = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            "Existing",
            "--path",
            &space.space_arg(),
        ],
    );
    let step_id = init["initial_step_id"]
        .as_str()
        .expect("initial step id")
        .to_string();

    assert!(space.space.join(".layrs").is_dir());
    assert_eq!(init["pending_publish_count"].as_u64(), Some(1));
    assert_file(&space.space, "hello.txt", "hello before layrs\n");

    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(diff["source"].as_str(), Some("latestPendingStep"));
    assert_eq!(diff["step_id"].as_str(), Some(step_id.as_str()));
    assert_eq!(
        diff["message"].as_str(),
        Some(
            format!("No working tree changes; displaying latest pending Step {step_id}.").as_str()
        )
    );
    assert_json_array_contains(&diff["files"], "hello.txt");

    let timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert!(
        json_steps(&timeline)
            .iter()
            .any(|step| step["step_id"] == step_id)
    );

    space.pass();
}

#[test]
fn added_file_step_becomes_latest_pending_and_addressable_by_id() {
    let space = init_empty("added-file");
    fs::create_dir_all(space.space.join("src")).expect("create src");
    fs::write(space.space.join("src").join("new-file.txt"), "new file\n").expect("write new file");

    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(diff["source"].as_str(), Some("workingTree"));
    assert_json_array_contains(&diff["files"], "src/new-file.txt");

    let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let step_id = step["step_id"].as_str().expect("step id").to_string();
    assert_eq!(step["changed_files"].as_u64(), Some(1));

    let latest = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(latest["source"].as_str(), Some("latestPendingStep"));
    assert_eq!(latest["step_id"].as_str(), Some(step_id.as_str()));
    assert_eq!(
        latest["message"].as_str(),
        Some(
            format!("No working tree changes; displaying latest pending Step {step_id}.").as_str()
        )
    );

    let by_id = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "diff", &step_id],
    );
    assert_eq!(by_id["source"].as_str(), Some("step"));
    assert_eq!(by_id["step_id"].as_str(), Some(step_id.as_str()));
    assert_json_array_contains(&by_id["files"], "src/new-file.txt");

    space.pass();
}

#[test]
fn modifications_deletions_and_additions_step_cleanly_without_touching_files() {
    let space = init_empty("modify-delete-add");
    fs::write(space.space.join("keep.txt"), "keep v1\n").expect("write keep");
    fs::write(space.space.join("edit.txt"), "edit v1\n").expect("write edit");
    let base = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    assert_eq!(base["changed_files"].as_u64(), Some(2));

    fs::write(space.space.join("edit.txt"), "edit v2\n").expect("modify edit");
    fs::write(space.space.join("add.txt"), "add v1\n").expect("write add");
    fs::remove_file(space.space.join("keep.txt")).expect("delete keep");

    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(diff["source"].as_str(), Some("workingTree"));
    assert_json_array_contains(&diff["files"], "add.txt");
    assert_json_array_contains(&diff["files"], "edit.txt");
    assert_json_array_contains(&diff["files"], "keep.txt");

    let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    assert_eq!(step["changed_files"].as_u64(), Some(3));

    let status = run_json(&space, ["--json", "--space", &space.space_arg(), "status"]);
    assert_eq!(status["changed"].as_bool(), Some(false));
    assert_file(&space.space, "edit.txt", "edit v2\n");
    assert_file(&space.space, "add.txt", "add v1\n");
    assert!(
        !space.space.join("keep.txt").exists(),
        "deleted file was restored"
    );

    space.pass();
}

#[test]
fn stepped_layer_switches_restore_each_layer_and_keep_timelines_separate() {
    let space = init_empty("stepped-layers");
    fs::write(space.space.join("shared.txt"), "main version\n").expect("write main");
    let main_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let main_step_id = main_step["step_id"]
        .as_str()
        .expect("main step")
        .to_string();

    let feature = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "create",
            "Feature",
        ],
    );
    assert_eq!(feature["name"].as_str(), Some("Feature"));

    fs::write(space.space.join("shared.txt"), "feature version\n").expect("write feature");
    fs::write(space.space.join("feature-only.txt"), "feature only\n").expect("write feature only");
    let feature_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let feature_step_id = feature_step["step_id"]
        .as_str()
        .expect("feature step")
        .to_string();

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Main",
        ],
    );
    assert_file(&space.space, "shared.txt", "main version\n");
    assert!(!space.space.join("feature-only.txt").exists());
    let main_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert!(
        json_steps(&main_timeline)
            .iter()
            .any(|step| step["step_id"] == main_step_id)
    );
    assert!(
        !json_steps(&main_timeline)
            .iter()
            .any(|step| step["step_id"] == feature_step_id)
    );

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    assert_file(&space.space, "shared.txt", "feature version\n");
    assert_file(&space.space, "feature-only.txt", "feature only\n");
    let feature_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert!(
        json_steps(&feature_timeline)
            .iter()
            .any(|step| step["step_id"] == feature_step_id)
    );
    assert!(
        !json_steps(&feature_timeline)
            .iter()
            .any(|step| step["step_id"] == main_step_id)
    );

    space.pass();
}

#[test]
fn switching_away_from_unstepped_layer_work_auto_steps_and_restores_it() {
    let space = init_empty("unstepped-layer-switch");
    fs::write(space.space.join("note.txt"), "base\n").expect("write base");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "create",
            "Feature",
        ],
    );
    fs::write(space.space.join("note.txt"), "feature saved\n").expect("write feature");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    fs::write(space.space.join("scratch.txt"), "unstepped feature work\n").expect("write scratch");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Main",
        ],
    );
    assert_file(&space.space, "note.txt", "base\n");
    assert!(!space.space.join("scratch.txt").exists());

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    assert_file(&space.space, "note.txt", "feature saved\n");
    assert_file(&space.space, "scratch.txt", "unstepped feature work\n");

    let latest = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    let auto_step_id = latest["step_id"]
        .as_str()
        .expect("auto step id")
        .to_string();
    assert_eq!(latest["source"].as_str(), Some("latestPendingStep"));
    assert_eq!(
        latest["message"].as_str(),
        Some(
            format!("No working tree changes; displaying latest pending Step {auto_step_id}.")
                .as_str()
        )
    );
    assert_json_array_contains(&latest["files"], "scratch.txt");

    space.pass();
}

#[test]
fn compact_keeps_large_repeated_steps_small_and_diffable() {
    let space = TestSpace::new("compact-weight");
    let mut content = deterministic_large_content(0);
    fs::write(space.space.join("big.txt"), &content).expect("write big");
    let init = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            "Compact Weight",
            "--path",
            &space.space_arg(),
        ],
    );
    assert_eq!(init["pending_publish_count"].as_u64(), Some(1));

    let mut latest_step_id = init["initial_step_id"]
        .as_str()
        .expect("initial step")
        .to_string();
    let mut raw_cumulative = content.len() as u64;
    for index in 1..=10 {
        content = deterministic_large_content(index);
        raw_cumulative += content.len() as u64;
        fs::write(space.space.join("big.txt"), &content).expect("modify big");
        let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
        assert_eq!(step["changed_files"].as_u64(), Some(1));
        latest_step_id = step["step_id"].as_str().expect("step id").to_string();
    }

    let step_dir = space
        .space
        .join(".layrs")
        .join("layers")
        .join("local_layer_main")
        .join("steps");
    for entry in fs::read_dir(&step_dir).expect("read step dir") {
        let path = entry.expect("step entry").path();
        let body = fs::read_to_string(&path).expect("read step json");
        assert!(
            body.len() < 16 * 1024,
            "step JSON should stay small: {} has {} bytes",
            path.display(),
            body.len()
        );
        assert!(
            !body.contains(&"a".repeat(4096)),
            "step JSON unexpectedly contains full repeated file content: {}",
            path.display()
        );
    }

    let compact = run_json(&space, ["--json", "--space", &space.space_arg(), "compact"]);
    let threshold = raw_cumulative * 35 / 100;
    let stored_bytes = compact["stored_bytes"].as_u64().expect("stored_bytes");
    let layrs_bytes = space_size_bytes(&space.space.join(".layrs"));
    assert!(
        stored_bytes < threshold,
        "compact stored_bytes {stored_bytes} should be under 35% of raw cumulative {raw_cumulative}"
    );
    assert!(
        layrs_bytes < threshold,
        ".layrs size {layrs_bytes} should be under 35% of raw cumulative {raw_cumulative}"
    );

    let diff = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "diff",
            &latest_step_id,
        ],
    );
    assert_eq!(diff["source"].as_str(), Some("step"));
    assert_json_array_contains(&diff["files"], "big.txt");

    space.pass();
}

fn init_empty(label: &str) -> TestSpace {
    let space = TestSpace::new(label);
    let init = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            label,
            "--path",
            &space.space_arg(),
        ],
    );
    assert_eq!(init["pending_publish_count"].as_u64(), Some(0));
    assert!(init["initial_step_id"].is_null());
    space
}

fn preserve_env(command: &mut Command, key: &str) {
    if let Some(value) = std::env::var_os(key) {
        command.env(key, value);
    }
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX_EPOCH")
        .as_nanos()
}

fn json_steps(value: &Value) -> &[Value] {
    value["steps"].as_array().expect("timeline steps")
}

fn assert_json_array_contains(value: &Value, expected: &str) {
    let items = value.as_array().expect("JSON array");
    assert!(
        items.iter().any(|item| item.as_str() == Some(expected)),
        "expected JSON array to contain {expected:?}, got {items:?}"
    );
}

fn deterministic_large_content(step: usize) -> String {
    const TARGET_BYTES: usize = 187 * 1024;
    const MUTABLE_ROWS: [usize; 12] = [
        17, 149, 311, 487, 653, 829, 997, 1_181, 1_349, 1_517, 1_681, 1_859,
    ];

    let templates = [
        "component=canvas action=render status=stable",
        "component=timeline action=index status=stable",
        "component=sync action=queue status=stable",
        "component=policy action=check status=stable",
        "component=lens action=preview status=stable",
        "component=storage action=chunk status=stable",
        "component=layer action=materialize status=stable",
    ];
    let nouns = [
        "brush",
        "frame",
        "palette",
        "viewport",
        "snapshot",
        "workspace",
        "cursor",
        "selection",
    ];

    let mut content = String::with_capacity(TARGET_BYTES + 256);
    let mut row = 0usize;
    while content.len() < TARGET_BYTES {
        let template = templates[row % templates.len()];
        let noun = nouns[(row / templates.len()) % nouns.len()];
        let epoch = row % 97;
        let checksum = (row * 31 + 7) % 10_000;

        if MUTABLE_ROWS.contains(&row) {
            content.push_str(&format!(
                "{row:04} {template} item={noun}-{epoch:02} delta=local-edit-{step:02} checksum={checksum:04} note=small localized mutation\n"
            ));
        } else {
            content.push_str(&format!(
                "{row:04} {template} item={noun}-{epoch:02} delta=unchanged checkpoint checksum={checksum:04} note=reusable deterministic line\n"
            ));
        }

        row += 1;
    }

    content.truncate(TARGET_BYTES);
    content
}
