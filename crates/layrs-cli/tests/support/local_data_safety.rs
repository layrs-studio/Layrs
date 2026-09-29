#![allow(dead_code)]

use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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

pub fn run_ok<I, S>(space: &TestSpace, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_ok_in(space, &space.space, args)
}

pub fn run_ok_with_stdin<I, S>(space: &TestSpace, args: I, stdin: &str) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<OsString>>();

    let output = run_layrs_with_stdin(space, &space.space, &args, Some(stdin.as_bytes()));
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

    format!("{stdout}{stderr}")
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

pub fn run_json_with_stdin<I, S>(space: &TestSpace, args: I, stdin: &str) -> Value
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    read_json(&run_ok_with_stdin(space, args, stdin))
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
    run_layrs_with_stdin(space, cwd, args, None)
}

fn run_layrs_with_stdin(
    space: &TestSpace,
    cwd: &Path,
    args: &[OsString],
    stdin: Option<&[u8]>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_layrs"));
    command.args(args);
    command.current_dir(cwd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
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
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }

    let mut child = command.spawn().unwrap_or_else(|error| {
        panic!(
            "failed to run layrs with args {:?} in {}: {error}",
            args,
            cwd.display()
        )
    });
    if let Some(stdin) = stdin {
        let mut child_stdin = child.stdin.take().expect("child stdin");
        child_stdin
            .write_all(stdin)
            .unwrap_or_else(|error| panic!("write layrs stdin: {error}"));
    }
    child.wait_with_output().unwrap_or_else(|error| {
        panic!(
            "failed waiting for layrs with args {:?} in {}: {error}",
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
