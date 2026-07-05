#![allow(dead_code)]

use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub struct TestSpace {
    pub root: PathBuf,
    pub space: PathBuf,
    appdata: PathBuf,
    xdg: PathBuf,
    home: PathBuf,
    cleanup: bool,
}

impl TestSpace {
    pub fn new(label: &str) -> Self {
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

    pub fn space_arg(&self) -> String {
        self.space.display().to_string()
    }

    pub fn pass(mut self) {
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

pub fn init_empty(label: &str) -> TestSpace {
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

pub fn run_ok<I, S>(space: &TestSpace, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_ok_in(space, &space.space, args)
}

pub fn run_ok_in<I, S>(space: &TestSpace, cwd: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<OsString>>();

    let output = run_layrs(space, cwd, &args);
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

pub fn run_err<I, S>(space: &TestSpace, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<OsString>>();
    let output = run_layrs(space, &space.space, &args);
    assert!(
        !output.status.success(),
        "layrs {:?} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}\nspace: {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        space.root.display()
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    format!("{stdout}\n{stderr}")
}

pub fn read_json(output: &str) -> Value {
    let envelope: Value = serde_json::from_str(output)
        .unwrap_or_else(|error| panic!("invalid JSON output: {error}\n{output}"));
    assert_eq!(
        envelope["ok"],
        Value::Bool(true),
        "CLI returned error JSON: {envelope}"
    );
    envelope["data"].clone()
}

pub fn run_json<I, S>(space: &TestSpace, args: I) -> Value
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    read_json(&run_ok(space, args))
}

pub fn space_size_bytes(path: &Path) -> u64 {
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

pub fn assert_file(root: &Path, relative: &str, expected: &str) {
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

pub fn assert_file_bytes(root: &Path, relative: &str, expected: &[u8]) {
    let path = root.join(relative);
    let actual = fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "file bytes mismatch at {}",
        path.display()
    );
}

pub fn json_steps(value: &Value) -> &[Value] {
    value["steps"].as_array().expect("timeline steps")
}

pub fn assert_json_array_contains(value: &Value, expected: &str) {
    let items = value.as_array().expect("JSON array");
    assert!(
        items.iter().any(|item| item.as_str() == Some(expected)),
        "expected JSON array to contain {expected:?}, got {items:?}"
    );
}

pub fn assert_json_array_not_contains(value: &Value, unexpected: &str) {
    let items = value.as_array().expect("JSON array");
    assert!(
        items.iter().all(|item| item.as_str() != Some(unexpected)),
        "expected JSON array to exclude {unexpected:?}, got {items:?}"
    );
}

pub type WorkingTreeSnapshot = BTreeMap<String, Vec<u8>>;

pub fn snapshot_working_tree(root: &Path) -> WorkingTreeSnapshot {
    let mut snapshot = BTreeMap::new();
    snapshot_working_tree_inner(root, root, &mut snapshot);
    snapshot
}

pub fn assert_working_tree_snapshot(root: &Path, expected: &WorkingTreeSnapshot) {
    let actual = snapshot_working_tree(root);
    assert_eq!(
        actual,
        *expected,
        "working tree changed under {}",
        root.display()
    );
}

pub fn loose_chunk_count(root: &Path) -> usize {
    let chunks = root.join(".layrs").join("objects").join("chunks");
    if !chunks.exists() {
        return 0;
    }
    fs::read_dir(&chunks)
        .unwrap_or_else(|error| panic!("read chunks {}: {error}", chunks.display()))
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("chunk"))
        .count()
}

pub fn timeline_step_by_origin<'a>(
    timeline: &'a Value,
    origin_layer_id: &str,
    origin_step_id: &str,
) -> &'a Value {
    json_steps(timeline)
        .iter()
        .find(|step| {
            step["origin_layer_id"].as_str() == Some(origin_layer_id)
                && step["origin_step_id"].as_str() == Some(origin_step_id)
        })
        .unwrap_or_else(|| {
            panic!(
                "timeline missing origin {origin_layer_id}/{origin_step_id}: {:?}",
                json_steps(timeline)
            )
        })
}

pub fn assert_timeline_has_origin(
    timeline: &Value,
    origin_layer_id: &str,
    origin_step_id: &str,
    step_kind: &str,
) {
    let step = timeline_step_by_origin(timeline, origin_layer_id, origin_step_id);
    assert_eq!(step["step_kind"].as_str(), Some(step_kind), "{step:?}");
}

pub fn assert_timeline_origin_order(timeline: &Value, expected: &[(&str, &str, &str)]) {
    let actual = json_steps(timeline)
        .iter()
        .rev()
        .map(|step| {
            (
                step["origin_layer_id"].as_str().unwrap_or("<missing>"),
                step["origin_step_id"].as_str().unwrap_or("<missing>"),
                step["step_kind"].as_str().unwrap_or("<missing>"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "timeline provenance order mismatch");
}

pub fn assert_latest_weave_conflict_files(root: &Path) {
    let weave_dir = latest_weave_dir(root);
    let conflicts_dir = weave_dir.join("conflicts");
    let mut conflicts = fs::read_dir(&conflicts_dir)
        .unwrap_or_else(|error| panic!("read conflicts {}: {error}", conflicts_dir.display()))
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect::<Vec<_>>();
    conflicts.sort_by_key(|entry| entry.file_name());
    let conflict = conflicts
        .first()
        .unwrap_or_else(|| panic!("no conflicts in {}", conflicts_dir.display()));
    for name in ["base", "ours", "theirs"] {
        assert!(
            conflict.path().join(name).exists(),
            "missing conflict file {} in {}",
            name,
            conflict.path().display()
        );
    }
}

pub fn deterministic_png_like_bytes() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    for index in 0..2048u32 {
        bytes.extend_from_slice(&index.rotate_left(index % 17).to_le_bytes());
    }
    bytes
}

fn snapshot_working_tree_inner(root: &Path, current: &Path, snapshot: &mut WorkingTreeSnapshot) {
    for entry in fs::read_dir(current)
        .unwrap_or_else(|error| panic!("read_dir {}: {error}", current.display()))
    {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.file_name().and_then(|value| value.to_str()) == Some(".layrs") {
            continue;
        }
        if path.is_dir() {
            snapshot_working_tree_inner(root, &path, snapshot);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path under root")
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let bytes =
                fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            snapshot.insert(relative, bytes);
        }
    }
}

fn latest_weave_dir(root: &Path) -> PathBuf {
    let weaves = root.join(".layrs").join("weaves");
    let mut dirs = fs::read_dir(&weaves)
        .unwrap_or_else(|error| panic!("read weaves {}: {error}", weaves.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    dirs.sort();
    dirs.pop()
        .unwrap_or_else(|| panic!("no weave directories in {}", weaves.display()))
}

pub fn first_chunk_path(root: &Path, layer_id: &str, file_path: &str) -> PathBuf {
    let layrs = root.join(".layrs");
    let state_path = layrs
        .join("layers")
        .join(layer_id)
        .join("working-state.json");
    let state: Value = serde_json::from_str(
        &fs::read_to_string(&state_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", state_path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", state_path.display()));
    let tree_id = state["rootTreeId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing rootTreeId in {}", state_path.display()));
    let tree_path = layrs
        .join("objects")
        .join("trees")
        .join(format!("{}.json", object_file_stem(tree_id)));
    let tree: Value = serde_json::from_str(
        &fs::read_to_string(&tree_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", tree_path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", tree_path.display()));
    let files = tree["files"]
        .as_array()
        .unwrap_or_else(|| panic!("missing files array in tree object {}", tree_path.display()));
    let file = files
        .iter()
        .find(|entry| entry["path"].as_str() == Some(file_path))
        .unwrap_or_else(|| panic!("missing {file_path} in tree {}", tree_path.display()));
    let object = file["object"]
        .as_str()
        .unwrap_or_else(|| panic!("missing object ref for {file_path}"));
    let manifest_path = layrs.join(object);
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", manifest_path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", manifest_path.display()));
    let chunk_id = manifest["chunks"]
        .as_array()
        .and_then(|chunks| chunks.first())
        .and_then(|chunk| chunk["chunkId"].as_str())
        .unwrap_or_else(|| panic!("missing first chunk in {}", manifest_path.display()));
    layrs
        .join("objects")
        .join("chunks")
        .join(format!("{}.chunk", object_file_stem(chunk_id)))
}

pub fn deterministic_large_content(step: usize) -> String {
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

fn run_layrs(space: &TestSpace, cwd: &Path, args: &[OsString]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_layrs"));
    command.args(args);
    command.current_dir(cwd);
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

    command.output().unwrap_or_else(|error| {
        panic!(
            "failed to run layrs with args {:?} in {}: {error}",
            args,
            cwd.display()
        )
    })
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

fn object_file_stem(id: &str) -> &str {
    id.strip_prefix("blake3:").unwrap_or(id)
}
