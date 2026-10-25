//! Opt-in real Windows CFAPI qualification. Run one exact recipe at a time with
//! LAYRS_VFS_TEST_PARENT set to an existing disposable local NTFS directory.
//! Never activates OS features. Every fixture, log, ticket and export is retained.
#![cfg(windows)]

use layrs_client_core::native_scm::{
    NativeWorkspace, VfsHostClient, VfsHostCommand, VfsHostResponse, VfsHostTicket, VfsWorktree,
    VfsWorktreeFaults, VfsWorktreeLocator,
};
use layrs_core::scm::{FileId, LayerId, Step, StepId};
use layrs_store_local::scm_v1::{
    checkout::{CheckoutContent, CheckoutEntry, CheckoutProjectionEntry, projection_tree_id},
    vfs_recovery::{VfsEvent, VfsJournalFaults, VfsRecovery},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    fs,
    io::{self, Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const BASE: &[u8] = b"original complete Store content\n";
const EDITED: &[u8] = b"private VFS CLI checkpoint\n";
const READY_SECONDS: &str = "30";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_OUTPUT: u64 = 64 * 1024;

fn pattern() -> Vec<u8> {
    (0..64 * 1024 + 19)
        .map(|n| ((n * 29 + n / 127) % 251) as u8)
        .collect()
}
fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 fixture path")
}

struct Fixture {
    root: PathBuf,
    ordinary: PathBuf,
    source: NativeWorkspace,
    layer: LayerId,
    step: StepId,
    command_number: Cell<u32>,
}
impl Fixture {
    fn new() -> Self {
        let parent =
            PathBuf::from(std::env::var_os("LAYRS_VFS_TEST_PARENT").expect(
                "set an explicit disposable LAYRS_VFS_TEST_PARENT; no default is permitted",
            ));
        assert!(
            parent.is_absolute() && parent.is_dir(),
            "parent must be an existing absolute directory"
        );
        let parent = std::path::absolute(parent).unwrap();
        let root = parent.join(format!(
            "cli-vfs-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        eprintln!("VFS CLI fixture retained: {}", root.display());
        let ordinary = root.join("ordinary source");
        fs::create_dir(&ordinary).unwrap();
        fs::write(ordinary.join("same.txt"), BASE).unwrap();
        fs::write(ordinary.join("untouched.bin"), pattern()).unwrap();
        let source = NativeWorkspace::initialize(&ordinary, "CLI VFS qualification").unwrap();
        let state = source.state().unwrap();
        let own = state
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == source.worktree_id())
            .unwrap();
        Self {
            root,
            ordinary,
            layer: own.active_layer_id().unwrap().clone(),
            step: own.base_step_id().unwrap().clone(),
            source,
            command_number: Cell::new(0),
        }
    }
    fn empty(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir(&path).unwrap();
        path
    }
    fn run(&self, label: &str, command: &mut Command, timeout: Duration) -> Vec<u8> {
        let sequence = self.command_number.get();
        self.command_number.set(sequence + 1);
        let stdout_path = self.root.join(format!("{sequence:02}-{label}.stdout"));
        let stderr_path = self.root.join(format!("{sequence:02}-{label}.stderr"));
        let stdout = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stdout_path)
            .unwrap();
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stderr_path)
            .unwrap();
        command
            .current_dir(&self.root)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        let mut child = OwnedChild(command.spawn().unwrap());
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "{label} timed out; only its owned command process will be terminated; logs retained in {}",
                self.root.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            status.success(),
            "{label} failed ({status}); inspect {} and {}",
            stdout_path.display(),
            stderr_path.display()
        );
        bounded_read(&stdout_path)
    }
    fn cli(&self, label: &str, args: &[&str], timeout: Duration) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_layrs"));
        command.arg("--json").args(args);
        let bytes = self.run(label, &mut command, timeout);
        assert_eq!(
            bytes
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            1,
            "CLI must return one JSON envelope"
        );
        let envelope: Value = serde_json::from_slice(&bytes).expect("valid CLI JSON envelope");
        assert_eq!(
            envelope["ok"], true,
            "successful CLI process must return ok=true"
        );
        envelope["data"].clone()
    }
    fn io(&self, root: &Path, action: &str) {
        assert!(root.starts_with(&self.root) && root != self.root);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "vfs_cli_owned_io_helper",
                "--nocapture",
            ])
            .env("LAYRS_CLI_VFS_OWNED_FIXTURE", &self.root)
            .env("LAYRS_CLI_VFS_OWNED_ROOT", root)
            .env("LAYRS_CLI_VFS_IO_ACTION", action);
        self.run(action, &mut command, Duration::from_secs(30));
    }
    fn assert_source(&self, witness: &Value, expected: &[u8]) {
        assert_eq!(json!(self.source.state().unwrap()), *witness);
        assert_eq!(fs::read(self.ordinary.join("same.txt")).unwrap(), expected);
        assert_eq!(
            fs::read(self.ordinary.join("untouched.bin")).unwrap(),
            pattern()
        );
    }
}

// A Child handle proves ownership. Never kill a process selected by a ticket PID.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = self.0.kill();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.0.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!(
            "termination requested for owned command PID {}; exit not observed",
            self.0.id()
        );
    }
}
fn bounded_read(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .take(MAX_OUTPUT + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(
        bytes.len() as u64 <= MAX_OUTPUT,
        "output exceeds bound: {}",
        path.display()
    );
    bytes
}

struct OwnedHost {
    ticket: VfsHostTicket,
    ticket_file: PathBuf,
    stopped: bool,
}
impl OwnedHost {
    fn from_launch(f: &Fixture, data: &Value, name: &str, root: &Path) -> Self {
        let ticket: VfsHostTicket = serde_json::from_value(data["ticket"].clone()).unwrap();
        // Validate fixture ownership before accepting any cleanup capability.
        assert_eq!(data["path"], json!(root));
        assert_eq!(ticket.store_root, f.source.store_root());
        assert_eq!(ticket.store_identity, f.source.store_identity());
        assert_eq!(ticket.store_metadata_identity, f.source.metadata_identity());
        assert_ne!(&ticket.worktree_id, f.source.worktree_id());
        let ticket_file = f.root.join(format!("{name}-ticket.json"));
        let host = Self {
            ticket,
            ticket_file,
            stopped: false,
        };
        // Establish the cleanup guard before further assertions or file writes.
        assert_eq!(data["status"], "ready");
        assert!(data["pid"].as_u64().is_some_and(|pid| pid > 0));
        let mut keys = data["ticket"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "endpointId",
                "storeId",
                "storeIdentity",
                "storeMetadataIdentity",
                "storeRoot",
                "worktreeId"
            ]
        );
        let endpoint = VfsRecovery::read_endpoint(
            &host.ticket.store_root,
            &host.ticket.store_identity,
            &host.ticket.store_metadata_identity,
            &host.ticket.store_id,
            &host.ticket.worktree_id,
            &host.ticket.endpoint_id,
        )
        .unwrap();
        let endpoint: Value = serde_json::from_slice(&endpoint).unwrap();
        let token = endpoint["token"].as_str().expect("private endpoint token");
        assert!(
            !token.is_empty() && !data.to_string().contains(token),
            "launch JSON leaked authentication token"
        );
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&host.ticket_file)
            .unwrap();
        file.write_all(&serde_json::to_vec_pretty(&host.ticket).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        host
    }
    fn status(&self, f: &Fixture, root: &Path, base: &StepId, accepted: &StepId) -> Value {
        let status = f.cli(
            "status",
            &["vfs", "status", "--ticket", text(&self.ticket_file)],
            CONTROL_TIMEOUT,
        );
        assert_eq!(status["worktreeId"], json!(self.ticket.worktree_id));
        assert_eq!(status["checkoutRoot"], json!(root));
        assert_eq!(status["sourceStepId"], json!(base));
        assert_eq!(status["acceptedStepId"], json!(accepted));
        assert_eq!(status["connected"], true);
        assert_eq!(status["captureAckPending"], false);
        assert_eq!(status["catalogFiles"], 2);
        status
    }
    fn stop(&mut self, f: &Fixture) {
        let reply = f.cli(
            "stop",
            &["vfs", "stop", "--ticket", text(&self.ticket_file)],
            CONTROL_TIMEOUT,
        );
        assert_eq!(reply, json!({"disconnected":true,"workRetained":true}));
        let client = VfsHostClient::open(&self.ticket).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if client
                .request_with_timeout(VfsHostCommand::Status {}, Duration::from_millis(250))
                .is_err()
            {
                self.stopped = true;
                return;
            }
            assert!(
                Instant::now() < deadline,
                "stopped host still accepts control requests"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for OwnedHost {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        let result = VfsHostClient::open(&self.ticket).and_then(|client| {
            client.request_with_timeout(VfsHostCommand::Stop {}, Duration::from_secs(3))
        });
        if !matches!(result, Ok(VfsHostResponse::Ok { .. })) {
            eprintln!(
                "owned host stop was not confirmed; retain root/journal and retry vfs stop --ticket {}",
                self.ticket_file.display()
            );
        }
    }
}

fn expected_files(same: &[u8]) -> Vec<CheckoutEntry> {
    vec![
        CheckoutEntry {
            path: "same.txt".into(),
            content: CheckoutContent::File {
                bytes: same.to_vec(),
                executable: false,
            },
        },
        CheckoutEntry {
            path: "untouched.bin".into(),
            content: CheckoutContent::File {
                bytes: pattern(),
                executable: false,
            },
        },
    ]
}

#[test]
#[ignore = "real CLI/CFAPI lifecycle; explicit disposable LAYRS_VFS_TEST_PARENT required"]
fn cli_vfs_background_create_capture_export_restart() {
    let f = Fixture::new();
    let witness = json!(f.source.state().unwrap());
    let root = f.empty("virtual root");
    let created = f.cli(
        "create",
        &[
            "--space",
            text(&f.ordinary),
            "vfs",
            "create",
            text(&root),
            "--layer",
            f.layer.as_str(),
            "--step",
            f.step.as_str(),
            "--background",
            "--ready-timeout-seconds",
            READY_SECONDS,
        ],
        COMMAND_TIMEOUT,
    );
    let mut host = OwnedHost::from_launch(&f, &created, "created", &root);
    let id = host.ticket.worktree_id.clone();
    host.status(&f, &root, &f.step, &f.step);
    let registrations = f.source.list_worktrees().unwrap();
    assert_eq!(registrations.len(), 2);
    let registered = registrations.iter().find(|w| w.id() == &id).unwrap();
    assert_eq!(Path::new(registered.checkout_root()), root);
    f.io(&root, "verify-base");
    f.io(&root, "edit"); // Waits for writer exit and flush before explicit quiescence.
    let capture = f.cli(
        "capture",
        &[
            "vfs",
            "capture",
            "--ticket",
            text(&host.ticket_file),
            "--writers-quiesced",
        ],
        COMMAND_TIMEOUT,
    );
    let step = StepId::parse(capture["stepId"].as_str().unwrap()).unwrap();
    let files = expected_files(EDITED);
    let tree = projection_tree_id(
        &files
            .iter()
            .map(CheckoutProjectionEntry::from)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(
        &step,
        Step::new(vec![f.step.clone()], tree.clone()).unwrap().id()
    );
    assert_eq!(capture["changed"], true);
    assert_eq!(f.source.read_step_files(&step).unwrap(), files);
    let status = host.status(&f, &root, &f.step, &step);
    assert_eq!(capture["activeOperationId"], status["activeOperationId"]);
    f.assert_source(&witness, BASE);

    let exports = f.empty("exports");
    let export = f.cli(
        "export",
        &[
            "vfs",
            "export",
            text(&exports),
            "--ticket",
            text(&host.ticket_file),
            "--writers-quiesced",
        ],
        COMMAND_TIMEOUT,
    );
    let tree_root = PathBuf::from(export["treeRoot"].as_str().unwrap());
    let manifest_path = PathBuf::from(export["manifestPath"].as_str().unwrap());
    assert!(tree_root.starts_with(&exports) && manifest_path.starts_with(&exports));
    assert_eq!(tree_root.parent(), manifest_path.parent());
    let mut names = fs::read_dir(&tree_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["same.txt", "untouched.bin"]);
    assert_eq!(fs::read(tree_root.join("same.txt")).unwrap(), EDITED);
    assert_eq!(
        fs::read(tree_root.join("untouched.bin")).unwrap(),
        pattern()
    );
    let total = EDITED.len() + pattern().len();
    assert_eq!(export["files"], 2);
    assert_eq!(export["bytes"], json!(total));
    assert_eq!(export["rootTreeId"], json!(tree));
    let manifest: Value = serde_json::from_slice(&bounded_read(&manifest_path)).unwrap();
    assert_eq!(manifest["schema"], "layrs.vfs-recovery-export.v1");
    assert_eq!(manifest["complete"], true);
    assert_eq!(manifest["worktreeId"], json!(id));
    assert_eq!(manifest["source"]["stepId"], json!(f.step));
    assert_eq!(manifest["source"]["storeId"], json!(host.ticket.store_id));
    assert_eq!(manifest["rootTreeId"], json!(tree));
    assert_eq!(manifest["bytes"], json!(total));
    assert_eq!(
        manifest["files"],
        json!([
            {"path":"same.txt","fileId":FileId::from_bytes(EDITED),"byteLength":EDITED.len()},
            {"path":"untouched.bin","fileId":FileId::from_bytes(&pattern()),"byteLength":pattern().len()},
        ])
    );
    host.stop(&f);
    let restarted = f.cli(
        "start",
        &[
            "vfs",
            "start",
            text(&root),
            "--ready-timeout-seconds",
            READY_SECONDS,
        ],
        COMMAND_TIMEOUT,
    );
    let mut reconnected = OwnedHost::from_launch(&f, &restarted, "restarted", &root);
    assert_eq!(reconnected.ticket.worktree_id, id);
    assert_ne!(reconnected.ticket.endpoint_id, host.ticket.endpoint_id);
    reconnected.status(&f, &root, &f.step, &step);
    f.io(&root, "verify-edited");
    reconnected.stop(&f);
    f.assert_source(&witness, BASE);
}

#[derive(Default)]
struct StopAfterReservation(RefCell<Option<VfsWorktreeLocator>>);
impl VfsJournalFaults for StopAfterReservation {
    fn after_event(&self, event: &VfsEvent) -> io::Result<()> {
        if let VfsEvent::Reserved { locator } = event {
            *self.0.borrow_mut() = Some(VfsWorktreeLocator::decode(locator).unwrap());
            return Err(io::Error::other(
                "CLI fixture stops after durable reservation",
            ));
        }
        Ok(())
    }
}
impl VfsWorktreeFaults for StopAfterReservation {}

#[test]
#[ignore = "real CLI/CFAPI exact-ID resume; explicit disposable LAYRS_VFS_TEST_PARENT required"]
fn cli_vfs_resume_reserved_id_preserves_original_base() {
    let f = Fixture::new();
    let root = f.empty("reserved virtual root");
    let fault = StopAfterReservation::default();
    assert!(VfsWorktree::create_with_faults(&f.source, &root, &f.layer, &f.step, &fault).is_err());
    let locator = fault
        .0
        .borrow()
        .clone()
        .expect("recorded durable reservation");
    assert_eq!(locator.checkout_root(), root);
    assert_eq!(locator.source.step_id, f.step);
    assert!(!root.join(".layrs").exists());
    assert_eq!(
        f.source.list_worktrees().unwrap().len(),
        1,
        "fault must precede registration"
    );
    fs::write(
        f.ordinary.join("same.txt"),
        b"ordinary source advanced after reservation",
    )
    .unwrap();
    f.source.capture().unwrap();
    let witness = json!(f.source.state().unwrap());
    assert_ne!(
        f.source.resolve_step_name("@layer:main").unwrap().step_id,
        f.step
    );
    let resumed = f.cli(
        "resume",
        &[
            "--space",
            text(&f.ordinary),
            "vfs",
            "resume",
            locator.worktree_id().as_str(),
            "--background",
            "--ready-timeout-seconds",
            READY_SECONDS,
        ],
        COMMAND_TIMEOUT,
    );
    let mut host = OwnedHost::from_launch(&f, &resumed, "resumed", locator.checkout_root());
    assert_eq!(&host.ticket.worktree_id, locator.worktree_id());
    host.status(
        &f,
        locator.checkout_root(),
        &locator.source.step_id,
        &locator.source.step_id,
    );
    f.io(locator.checkout_root(), "verify-base");
    let clean = f.cli(
        "capture-clean",
        &[
            "vfs",
            "capture",
            "--ticket",
            text(&host.ticket_file),
            "--writers-quiesced",
        ],
        COMMAND_TIMEOUT,
    );
    assert_eq!(clean["changed"], false);
    assert_eq!(clean["stepId"], json!(locator.source.step_id));
    assert_eq!(f.source.list_worktrees().unwrap().len(), 2);
    host.stop(&f);
    f.assert_source(&witness, b"ordinary source advanced after reservation");
}

#[test]
#[ignore = "bounded I/O helper invoked only by the exact CLI lifecycle recipes"]
fn vfs_cli_owned_io_helper() {
    let fixture = PathBuf::from(
        std::env::var_os("LAYRS_CLI_VFS_OWNED_FIXTURE").expect("owned fixture required"),
    );
    let root =
        PathBuf::from(std::env::var_os("LAYRS_CLI_VFS_OWNED_ROOT").expect("owned root required"));
    let parent =
        PathBuf::from(std::env::var_os("LAYRS_VFS_TEST_PARENT").expect("explicit parent required"));
    assert!(fixture.is_absolute() && fixture.starts_with(std::path::absolute(parent).unwrap()));
    assert!(root.starts_with(&fixture) && root != fixture);
    let action = std::env::var("LAYRS_CLI_VFS_IO_ACTION").unwrap();
    match action.as_str() {
        "edit" => {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(root.join("same.txt"))
                .unwrap();
            file.write_all(EDITED).unwrap();
            file.sync_all().unwrap();
        }
        "verify-base" | "verify-edited" => {
            assert_eq!(
                fs::read(root.join("same.txt")).unwrap(),
                if action == "verify-base" {
                    BASE
                } else {
                    EDITED
                }
            );
            assert_eq!(fs::read(root.join("untouched.bin")).unwrap(), pattern());
        }
        _ => panic!("unknown fixture I/O action"),
    }
}
