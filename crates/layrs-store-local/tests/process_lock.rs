use std::error::Error;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Stdio;
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use layrs_core::scm::StoreId;
use layrs_store_local::StoreError;
use layrs_store_local::scm_v1::{NativeStoreError, NativeStoreLayout, init, open, open_exclusive};

const CHILD_MARKER: &str = "--layrs-lock-child";
const HOLD_SHARED: &str = "hold-shared";
const PROBE_FSCK: &str = "probe-fsck";
type FixtureResult<T = ()> = Result<T, Box<dyn Error>>;

fn main() {
    if let Err(error) = dispatch() {
        eprintln!("process_lock fixture failed: {error}");
        std::process::exit(1);
    }
}

fn dispatch() -> FixtureResult {
    let arguments: Vec<_> = std::env::args_os().collect();
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument == OsStr::new(CHILD_MARKER))
    {
        let operation = arguments
            .get(index + 1)
            .and_then(|value| value.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing operation"))?;
        let root = required_path(&arguments, index + 2, "store root")?;
        match operation {
            HOLD_SHARED => {
                let ready = required_path(&arguments, index + 3, "ready signal")?;
                let release = required_path(&arguments, index + 4, "release signal")?;
                hold_shared_lock(&root, &ready, &release)
            }
            PROBE_FSCK => {
                drop(open(&root)?);
                Ok(())
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown child operation {operation}"),
            )
            .into()),
        }
    } else {
        verify_process_lock_matrix()
    }
}

fn verify_process_lock_matrix() -> FixtureResult {
    let root = unique_test_dir();
    let store = init(&root, StoreId::parse("store:process-lock")?)?;
    let layout = store.layout().clone();
    drop(store);
    verify_shared_shared_and_shared_exclusive(&layout)?;
    verify_shared_open_with_read_only_lock(&layout)?;
    fs::remove_dir_all(&root)?;

    println!(
        "process_lock fixture: shared/shared, shared/exclusive, and read-only lock rights verified"
    );
    Ok(())
}

fn verify_shared_shared_and_shared_exclusive(layout: &NativeStoreLayout) -> FixtureResult {
    let ready = layout.root().join("child-shared-ready");
    let release = layout.root().join("child-shared-release");
    let mut child = spawn_child(HOLD_SHARED, layout.root(), Some((&ready, &release)))?;
    if !wait_for_path(&ready, Duration::from_secs(10)) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "child process did not acquire the shared lock",
        )
        .into());
    }

    let parent_shared = open(layout.root())?;
    drop(parent_shared);
    let contention = open_exclusive(layout.root());
    let exclusive_was_busy = matches!(
        contention,
        Err(NativeStoreError::Lock(StoreError::LockBusy(_)))
    );
    drop(contention);

    fs::write(&release, b"release")?;
    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::other(format!("shared holder exited with {status}")).into());
    }
    if !exclusive_was_busy {
        return Err(io::Error::other("shared process lock did not block exclusive access").into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn verify_shared_open_with_read_only_lock(layout: &NativeStoreLayout) -> FixtureResult {
    let lock_path = layout.lock_path();
    let original_permissions = fs::metadata(lock_path)?.permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(lock_path, read_only)?;

    let probe_result = (|| -> FixtureResult<ExitStatus> {
        let mut child = spawn_child(PROBE_FSCK, layout.root(), None)?;
        Ok(child.wait()?)
    })();
    fs::set_permissions(lock_path, original_permissions)?;
    let status = probe_result?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "shared fsck could not open a read-only lock file: {status}"
        ))
        .into());
    }
    Ok(())
}

#[cfg(windows)]
fn verify_shared_open_with_read_only_lock(layout: &NativeStoreLayout) -> FixtureResult {
    let lock_path = layout.lock_path();
    let sid = current_user_sid()?;
    let grant = format!("*{sid}:(R)");
    let restricted = Command::new("icacls")
        .arg(lock_path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(grant)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !restricted.success() {
        return Err(io::Error::other(format!(
            "icacls could not install read-only ACL: {restricted}"
        ))
        .into());
    }

    let probe_result = (|| -> FixtureResult<ExitStatus> {
        let mut child = spawn_child(PROBE_FSCK, layout.root(), None)?;
        Ok(child.wait()?)
    })();
    let restored = Command::new("icacls")
        .arg(lock_path)
        .arg("/reset")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !restored.success() {
        return Err(
            io::Error::other(format!("icacls could not restore lock ACL: {restored}")).into(),
        );
    }
    let status = probe_result?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "shared fsck could not open a read-only ACL lock file: {status}"
        ))
        .into());
    }
    Ok(())
}

#[cfg(windows)]
fn current_user_sid() -> FixtureResult<String> {
    let output = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("whoami exited with {}", output.status)).into());
    }
    let row = String::from_utf8(output.stdout)?;
    row.trim()
        .split("\",\"")
        .nth(1)
        .map(|sid| sid.trim_matches('"').to_string())
        .filter(|sid| sid.starts_with("S-"))
        .ok_or_else(|| io::Error::other(format!("could not parse user SID from {row:?}")))
        .map_err(Into::into)
}

fn spawn_child(
    operation: &str,
    root: &Path,
    signals: Option<(&Path, &Path)>,
) -> FixtureResult<ChildGuard> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command.arg(CHILD_MARKER).arg(operation).arg(root);
    if let Some((ready, release)) = signals {
        command.arg(ready).arg(release);
    }
    Ok(ChildGuard::new(command.spawn()?))
}

fn hold_shared_lock(root: &Path, ready: &Path, release: &Path) -> FixtureResult {
    let layout = NativeStoreLayout::new(root)?;
    let lock_path = layout.lock_path();
    let file = OpenOptions::new().read(true).open(lock_path)?;
    FileExt::try_lock_shared(&file)?;
    fs::write(ready, b"ready")?;
    if !wait_for_path(release, Duration::from_secs(15)) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "parent did not release the shared child lock",
        )
        .into());
    }
    FileExt::unlock(&file)?;
    Ok(())
}

fn required_path(
    arguments: &[std::ffi::OsString],
    index: usize,
    label: &str,
) -> FixtureResult<PathBuf> {
    arguments
        .get(index)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("missing {label}")))
        .map_err(Into::into)
}

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

fn unique_test_dir() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "layrs-store-local-process-lock-{}-{timestamp}",
        std::process::id()
    ))
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child
            .take()
            .expect("child guard already consumed")
            .wait()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
