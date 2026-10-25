//! Owned process-death and genuine retained-reader qualification. These tests
//! do not claim forced cache spill, power-loss durability, or old cached handles.
use super::*;
use crate::secure_fs::PinnedDirectory;
use rusqlite::types::Value;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const CHILD_SPEC: &str = "LAYRS_SCHEMA8_UPGRADE_CHILD_SPEC";
const CHILD_TEST: &str =
    "projection::tests::shared::provider_schema::process::upgrade_process_child";
const PROCESS_LIMIT: Duration = Duration::from_secs(20);
#[cfg(windows)]
const OUTPUT_LIMIT: u64 = 64 * 1024;

struct OwnedProcess {
    child: Child,
    reaped: bool,
    stdout: PathBuf,
    stderr: PathBuf,
}
impl OwnedProcess {
    fn spawn(command: &mut Command, logs: &Path, name: &str) -> Self {
        let stdout = logs.join(format!("{name}.stdout"));
        let stderr = logs.join(format!("{name}.stderr"));
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(new_file(&stdout)))
            .stderr(Stdio::from(new_file(&stderr)));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "cannot launch owned process ({error}); fixture: {}",
                logs.display()
            )
        });
        Self {
            child,
            reaped: false,
            stdout,
            stderr,
        }
    }
    fn poll(&mut self) -> Option<ExitStatus> {
        let status = self.child.try_wait().unwrap();
        if status.is_some() {
            self.reaped = true;
        }
        status
    }
    fn kill_and_reap(&mut self) {
        assert!(
            self.poll().is_none(),
            "child exited before requested kill: {}",
            self.diagnostics()
        );
        self.child.kill().unwrap();
        let status = self.child.wait().unwrap();
        self.reaped = true;
        assert!(!status.success(), "killed child unexpectedly succeeded");
    }
    fn diagnostics(&self) -> String {
        let prefix = |path: &Path| {
            let mut bytes = Vec::new();
            if let Ok(file) = File::open(path) {
                let _ = file.take(2048).read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        };
        format!(
            "logs {} / {}; stdout={}; stderr={}",
            self.stdout.display(),
            self.stderr.display(),
            prefix(&self.stdout),
            prefix(&self.stderr)
        )
    }
    #[cfg(windows)]
    fn finish(&mut self) -> (ExitStatus, Vec<u8>) {
        let deadline = Instant::now() + PROCESS_LIMIT;
        loop {
            if let Some(status) = self.poll() {
                assert!(
                    fs::metadata(&self.stdout).unwrap().len() <= OUTPUT_LIMIT,
                    "oversized output: {}",
                    self.diagnostics()
                );
                assert!(
                    fs::metadata(&self.stderr).unwrap().len() <= OUTPUT_LIMIT,
                    "oversized error output: {}",
                    self.diagnostics()
                );
                return (status, fs::read(&self.stdout).unwrap());
            }
            if Instant::now() >= deadline {
                self.kill_and_reap();
                panic!("owned process timed out: {}", self.diagnostics());
            }
            if [&self.stdout, &self.stderr]
                .into_iter()
                .any(|path| fs::metadata(path).unwrap().len() > OUTPUT_LIMIT)
            {
                self.kill_and_reap();
                panic!(
                    "owned process exceeded output limit: {}",
                    self.diagnostics()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if !self.reaped {
            // Never search for or terminate a process by executable name/PID.
            // This handle belongs only to the child launched by this instance.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn new_file(path: &Path) -> File {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap()
}
fn phase(point: ProviderSchemaUpgradePoint) -> &'static str {
    match point {
        ProviderSchemaUpgradePoint::SchemaCreated => "schema-created",
        ProviderSchemaUpgradePoint::BeforeCommit => "before-commit",
        ProviderSchemaUpgradePoint::AfterCommit => "after-commit",
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UpgradeSpec {
    root: PathBuf,
    acknowledgment: PathBuf,
    nonce: String,
    point: String,
    generation: u64,
}
impl UpgradeSpec {
    fn ack(&self) -> Vec<u8> {
        format!("schema8-upgrade:{}:{}", self.nonce, self.point).into_bytes()
    }
}
struct ParkAtBoundary(UpgradeSpec);
impl ProviderSchemaUpgradeFaults for ParkAtBoundary {
    fn check(&self, point: ProviderSchemaUpgradePoint) -> io::Result<()> {
        if phase(point) == self.0.point {
            let temporary = self.0.acknowledgment.with_extension("pending");
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&self.0.ack())?;
            file.sync_all()?;
            drop(file);
            // Both names are inside the verified, exclusively owned fixture.
            // Publish only after the complete nonce-bearing ACK is synced.
            assert!(!self.0.acknowledgment.exists());
            fs::rename(&temporary, &self.0.acknowledgment)?;
            loop {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(())
    }
}

/// Inert in an ordinary suite; the owning parent invokes this exact test name.
#[test]
fn upgrade_process_child() {
    let Some(spec_path) = std::env::var_os(CHILD_SPEC) else {
        return;
    };
    let spec_path = PathBuf::from(spec_path);
    assert!(fs::metadata(&spec_path).unwrap().len() <= 65536);
    let bytes = fs::read(&spec_path).unwrap();
    let spec: UpgradeSpec = serde_json::from_slice(&bytes).unwrap();
    let outer = spec_path.parent().unwrap();
    assert_eq!(spec.root.parent(), Some(outer));
    assert_eq!(spec.root.file_name().unwrap(), "worktree");
    assert_eq!(spec.acknowledgment, outer.join("upgrade.ack"));
    assert!(
        outer
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("layrs-projection-")
    );
    let outer_canonical = outer.canonicalize().unwrap();
    let temporary_canonical = std::env::temp_dir().canonicalize().unwrap();
    assert_eq!(
        outer_canonical.parent(),
        Some(temporary_canonical.as_path())
    );
    let _pin = PinnedDirectory::open(outer).unwrap();
    assert!(!spec.nonce.is_empty() && spec.nonce.len() <= 128);
    assert!(spec.nonce.bytes().all(|b| b.is_ascii_digit() || b == b'-'));
    assert!(["schema-created", "before-commit", "after-commit"].contains(&spec.point.as_str()));
    let store = ProjectionStore::open_for_recovery(&spec.root).unwrap();
    assert_eq!(version(&store), 7);
    store
        .upgrade_provider_schema_with_faults(spec.generation, &ParkAtBoundary(spec))
        .unwrap();
    panic!("requested upgrade boundary was not reached");
}

fn accepted_fixture(f: &Fixture) -> (PartialStoreFamily, ProjectionStore) {
    let family = family(f);
    let store = member(f, &family, "worktree", true);
    fs::write(
        store.checkout_root().join("a.txt"),
        b"accepted pre-upgrade work",
    )
    .unwrap();
    assert_eq!(store.capture_patch(1).unwrap().sequence, 1);
    store.cancel_shared_reservation().unwrap();
    // Existing journals leave empty unowned staging directories. Retain those
    // fixture-only directories outside the checkout; do not infer cleanup
    // ownership, remove a path, or teach the production upgrade to ignore it.
    for slot in [
        "projection-staging",
        "namespace-staging",
        "projection-quarantine",
    ] {
        let source = store.checkout_root().join(".layrs").join(slot);
        if !source.exists() {
            continue;
        }
        fn require_empty_tree(path: &Path) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                assert!(
                    entry.file_type().unwrap().is_dir(),
                    "fixture stage still contains data"
                );
                require_empty_tree(&entry.path());
            }
        }
        require_empty_tree(&source);
        let destination = f.0.join(format!("retained-{slot}"));
        let boundary = f.0.canonicalize().unwrap();
        assert!(source.canonicalize().unwrap().starts_with(&boundary));
        assert_eq!(
            destination.parent().unwrap().canonicalize().unwrap(),
            boundary
        );
        assert!(!destination.exists());
        fs::rename(&source, &destination).unwrap();
    }
    let ready = store
        .prepare_offline(&request(&store, "nested/b.txt"), &mut |_, _, _| {
            panic!("fixture must already be resident")
        })
        .unwrap();
    assert_eq!(ready.status, "ready");
    assert_eq!(version(&store), 7);
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    store.fsck().unwrap();
    (family, store)
}

type Payloads = BTreeMap<String, (u64, String)>;
fn payloads(c: &rusqlite::Connection, names: &[String]) -> Payloads {
    let mut result = BTreeMap::new();
    for table in names {
        assert!(table.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
        let columns = c
            .prepare(&format!("SELECT * FROM {table}"))
            .unwrap()
            .column_count();
        let order = (1..=columns)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut query = c
            .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
            .unwrap();
        let mut rows = query.query([]).unwrap();
        let mut count = 0u64;
        let mut hash = blake3::Hasher::new();
        hash.update(&(columns as u64).to_be_bytes());
        while let Some(row) = rows.next().unwrap() {
            count += 1;
            for ordinal in 0..columns {
                match row.get::<_, Value>(ordinal).unwrap() {
                    Value::Null => {
                        hash.update(&[0]);
                    }
                    Value::Integer(value) => {
                        hash.update(&[1]);
                        hash.update(&value.to_be_bytes());
                    }
                    Value::Real(value) => {
                        hash.update(&[2]);
                        hash.update(&value.to_bits().to_be_bytes());
                    }
                    Value::Text(value) => {
                        hash.update(&[3]);
                        hash.update(&(value.len() as u64).to_be_bytes());
                        hash.update(value.as_bytes());
                    }
                    Value::Blob(value) => {
                        hash.update(&[4]);
                        hash.update(&(value.len() as u64).to_be_bytes());
                        hash.update(&value);
                    }
                }
            }
        }
        result.insert(table.clone(), (count, hash.finalize().to_hex().to_string()));
    }
    result
}
#[derive(Debug, PartialEq, Eq)]
struct DiskEntry {
    identity: String,
    length: Option<u64>,
    digest: Option<String>,
}
fn disk(store: &ProjectionStore, family: &PartialStoreFamily) -> BTreeMap<PathBuf, DiskEntry> {
    fn visit(path: &Path, private_db: &Path, result: &mut BTreeMap<PathBuf, DiskEntry>) {
        assert!(result.len() < 1024, "unexpected fixture namespace growth");
        let metadata = fs::symlink_metadata(path).unwrap();
        let entry = if metadata.is_dir() {
            let pin = PinnedDirectory::open(path).unwrap();
            let entry = DiskEntry {
                identity: pin.identity_token(),
                length: None,
                digest: None,
            };
            for child in fs::read_dir(path).unwrap() {
                visit(&child.unwrap().path(), private_db, result);
            }
            pin.verify_path_identity().unwrap();
            entry
        } else {
            assert!(metadata.is_file() && !metadata.file_type().is_symlink());
            let pin = crate::projection::file::DatabasePin::open(path).unwrap();
            let bytes = if path == private_db {
                None
            } else {
                Some(fs::read(path).unwrap())
            };
            let entry = DiskEntry {
                identity: pin.identity().to_owned(),
                length: bytes.as_ref().map(|b| b.len() as u64),
                digest: bytes.as_ref().map(|b| blake3::hash(b).to_hex().to_string()),
            };
            pin.verify(path).unwrap();
            entry
        };
        result.insert(path.to_owned(), entry);
    }
    let mut result = BTreeMap::new();
    visit(store.checkout_root(), &db(store), &mut result);
    visit(family.root(), &db(store), &mut result);
    result
}
struct Snapshot {
    binding: RecoveryBinding,
    payloads: Payloads,
    disk: BTreeMap<PathBuf, DiskEntry>,
    private_bytes: Vec<u8>,
}
fn rollback_journal(store: &ProjectionStore) -> PathBuf {
    let mut path = db(store).into_os_string();
    path.push("-journal");
    PathBuf::from(path)
}
impl Snapshot {
    fn take(store: &ProjectionStore, family: &PartialStoreFamily) -> Self {
        let inspection = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
        assert!(inspection.recovery.is_empty());
        let c = database::connect(&db(store), false).unwrap();
        database::validate(&c, &store.marker).unwrap();
        let names = database::schema(&c)
            .unwrap()
            .into_iter()
            .filter(|(kind, _, _)| kind == "table")
            .map(|(_, name, _)| name)
            .collect::<Vec<_>>();
        Self {
            binding: inspection.binding,
            payloads: payloads(&c, &names),
            disk: disk(store, family),
            private_bytes: fs::read(db(store)).unwrap(),
        }
    }
    fn verify(
        &self,
        store: &ProjectionStore,
        family: &PartialStoreFamily,
        expected_schema: u32,
        raw_private: bool,
    ) {
        let inspection = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
        assert!(inspection.recovery.is_empty());
        assert_eq!(inspection.binding, self.binding);
        let c = database::connect(&db(store), false).unwrap();
        database::validate(&c, &store.marker).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            expected_schema
        );
        assert_eq!(
            payloads(&c, &self.payloads.keys().cloned().collect::<Vec<_>>()),
            self.payloads
        );
        let mut actual_disk = disk(store, family);
        if !raw_private {
            // A precommit kill can leave a non-hot rollback journal while all
            // accepted pages still equal schema7. It is transient SQLite data,
            // not an accepted payload. Never remove it to make the test pass.
            // Fresh old-reader no-write checks (raw_private=true) remain strict.
            actual_disk.remove(&rollback_journal(store));
        }
        assert_eq!(actual_disk, self.disk);
        if raw_private {
            assert_bytes(&db(store), &self.private_bytes);
        }
    }
}

#[test]
fn killed_upgrade_process_recovers_exact_schema_and_preserves_accepted_work() {
    for point in [
        ProviderSchemaUpgradePoint::SchemaCreated,
        ProviderSchemaUpgradePoint::BeforeCommit,
        ProviderSchemaUpgradePoint::AfterCommit,
    ] {
        let f = Fixture::new();
        eprintln!(
            "schema upgrade crash fixture: {}; phase: {}",
            f.0.display(),
            phase(point)
        );
        let (family, store) = accepted_fixture(&f);
        let before = Snapshot::take(&store, &family);
        let spec = UpgradeSpec {
            root: store.checkout_root().to_owned(),
            acknowledgment: f.0.join("upgrade.ack"),
            nonce: crate::projection::shared::unique_id().unwrap(),
            point: phase(point).into(),
            generation: 1,
        };
        let spec_path = f.0.join("upgrade-spec.json");
        let mut file = new_file(&spec_path);
        file.write_all(&serde_json::to_vec(&spec).unwrap()).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_SPEC, &spec_path);
        let mut child = OwnedProcess::spawn(&mut command, &f.0, "upgrade-child");
        let deadline = Instant::now() + PROCESS_LIMIT;
        loop {
            if spec.acknowledgment.exists() {
                assert!(fs::metadata(&spec.acknowledgment).unwrap().len() <= 4096);
                let ack = fs::read(&spec.acknowledgment).unwrap();
                assert_eq!(ack, spec.ack(), "wrong fixture/phase ACK");
                break;
            }
            assert!(
                child.poll().is_none(),
                "child exited before ACK: {}",
                child.diagnostics()
            );
            assert!(
                Instant::now() < deadline,
                "upgrade ACK timed out: {}",
                child.diagnostics()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        child.kill_and_reap();
        // Discovery must be the first database operation after death. It may
        // report typed rollback or already-complete ordinary metadata; it must
        // not silently repair the private DB as part of this inspection.
        let interrupted = fs::read(db(&store)).unwrap();
        let journal = rollback_journal(&store);
        let journal_bytes = match fs::read(&journal) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => panic!("cannot inspect interrupted journal: {error}"),
        };
        let inspection = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
        assert_bytes(&db(&store), &interrupted);
        if let Some(bytes) = &journal_bytes {
            assert_bytes(&journal, bytes);
        } else {
            assert!(!journal.exists());
        }
        assert_eq!(inspection.binding, before.binding);
        if !inspection.recovery.is_empty() {
            assert!(inspection.recovery.iter().all(|reason| matches!(
                reason,
                ProjectionRecoveryReason::FamilyRollback
                    | ProjectionRecoveryReason::WorktreeRollback
            )));
            let recovered =
                ProjectionStore::recover_databases(store.checkout_root(), &inspection.binding)
                    .unwrap();
            assert!(recovered.recovery.is_empty());
        }
        let expected = if point == ProviderSchemaUpgradePoint::AfterCommit {
            8
        } else {
            7
        };
        before.verify(&store, &family, expected, false);
        store.upgrade_provider_schema(1).unwrap();
        before.verify(&store, &family, 8, false);
        let accepted = Snapshot::take(&store, &family);
        store.upgrade_provider_schema(1).unwrap();
        accepted.verify(&store, &family, 8, true);
        store.fsck().unwrap();
        // Fixture's existing Drop preserves the entire tree/logs on panic.
    }
}

#[cfg(windows)]
const RETAINED_CLI_SHA256: &str =
    "1c23a1d6d2a8806e6edae769de26651c142448000b286a8181979fd03d488ee9";
#[cfg(windows)]
fn verify_retained_cli(path: &Path, logs: &Path, label: &str) {
    let powershell =
        PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot is required"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(powershell);
    // The executable path is passed as environment data, never interpolated
    // into shell code. Use the OS tool rather than add a test-only dependency.
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; (Get-FileHash -Algorithm SHA256 -LiteralPath $env:LAYRS_SCHEMA7_READER).Hash"])
        .env("LAYRS_SCHEMA7_READER", path);
    let mut child = OwnedProcess::spawn(&mut command, logs, label);
    let (status, bytes) = child.finish();
    assert!(
        status.success(),
        "artifact hash verification failed: {}",
        child.diagnostics()
    );
    assert_eq!(
        String::from_utf8(bytes)
            .unwrap()
            .trim()
            .to_ascii_lowercase(),
        RETAINED_CLI_SHA256,
        "wrong retained schema7 reader; logs: {}",
        logs.display()
    );
}
#[cfg(windows)]
fn retained_command(
    path: &Path,
    fixture: &Fixture,
    store: &ProjectionStore,
    label: &str,
    arguments: &[&str],
    success: bool,
) -> serde_json::Value {
    verify_retained_cli(path, &fixture.0, &format!("{label}-hash"));
    let mut command = Command::new(path);
    command
        .current_dir(store.checkout_root())
        .arg("--json")
        .args(arguments)
        .env("LAYRS_SERVER_ENDPOINT", "http://127.0.0.1:9");
    let mut child = OwnedProcess::spawn(&mut command, &fixture.0, label);
    let (status, bytes) = child.finish();
    assert_eq!(
        status.success(),
        success,
        "retained CLI result: {}",
        child.diagnostics()
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("retained CLI JSON: {error}; {}", child.diagnostics()));
    assert_eq!(
        value["ok"],
        success,
        "retained CLI envelope: {}",
        child.diagnostics()
    );
    if !success {
        assert!(
            String::from_utf8_lossy(&bytes).contains("projection schema mismatch"),
            "expected old schema refusal, not routing/argument/network failure: {}",
            child.diagnostics()
        );
    }
    value
}

#[cfg(windows)]
#[test]
#[ignore = "requires explicit LAYRS_SCHEMA7_READER pointing to the hash-pinned retained R5 CLI"]
fn retained_schema7_cli_accepts_seven_then_refuses_eight_without_writes() {
    let path = std::env::var_os("LAYRS_SCHEMA7_READER").expect(
        "set LAYRS_SCHEMA7_READER explicitly; a missing artifact must not skip qualification",
    );
    let path = std::path::absolute(PathBuf::from(path)).unwrap();
    let binary = crate::projection::file::DatabasePin::open(&path)
        .expect("retained CLI must exist and be a plain file");
    let f = Fixture::new();
    eprintln!(
        "retained reader fixture: {}; artifact: {}",
        f.0.display(),
        path.display()
    );
    let (family, store) = accepted_fixture(&f);
    let before = Snapshot::take(&store, &family);
    let value = retained_command(
        &path,
        &f,
        &store,
        "seven-status",
        &["partial", "offline", "status"],
        true,
    );
    assert_eq!(
        value["data"]["version"],
        serde_json::to_value(store.current_version().unwrap()).unwrap()
    );
    before.verify(&store, &family, 7, true);
    binary.verify(&path).unwrap();
    store.upgrade_provider_schema(1).unwrap();
    let upgraded = Snapshot::take(&store, &family);
    retained_command(
        &path,
        &f,
        &store,
        "eight-status",
        &["partial", "offline", "status"],
        false,
    );
    upgraded.verify(&store, &family, 8, true);
    let quota = family.usage().unwrap().limit_bytes.to_string();
    retained_command(
        &path,
        &f,
        &store,
        "eight-quota",
        &["partial", "offline", "quota", "--bytes", &quota],
        false,
    );
    upgraded.verify(&store, &family, 8, true);
    binary.verify(&path).unwrap();
    ProjectionStore::open(store.checkout_root())
        .unwrap()
        .fsck()
        .unwrap();
    // These launches are fresh old-binary readers. They establish no claim
    // about an old cached Store handle surviving an in-process schema upgrade.
}
