use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use layrs_store_local::{LocalStore, LocalStoreConfig, SessionMode, StoreError};

const CHILD_MARKER: &str = "--layrs-lock-child";
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
        let root = required_path(&arguments, index + 1, "store root")?;
        let ready = required_path(&arguments, index + 2, "ready signal")?;
        let release = required_path(&arguments, index + 3, "release signal")?;
        hold_exclusive_lock(&root, &ready, &release)
    } else {
        verify_cross_process_contention()
    }
}

fn verify_cross_process_contention() -> FixtureResult {
    let root = unique_test_dir();
    let store = LocalStore::init(LocalStoreConfig::new(&root))?;
    let ready = root.join("tmp").join("child-lock-ready");
    let release = root.join("tmp").join("child-lock-release");
    let executable = std::env::current_exe()?;
    let mut child = ChildGuard::new(
        Command::new(executable)
            .arg(CHILD_MARKER)
            .arg(&root)
            .arg(&ready)
            .arg(&release)
            .spawn()?,
    );

    if !wait_for_path(&ready, Duration::from_secs(10)) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "child process did not acquire the store lock",
        )
        .into());
    }

    let contention = store.session(SessionMode::Exclusive);
    let lock_was_busy = matches!(contention, Err(StoreError::LockBusy(_)));
    drop(contention);
    fs::write(&release, b"release")?;
    let status = child.wait()?;
    drop(store);
    fs::remove_dir_all(&root)?;

    if !status.success() {
        return Err(io::Error::other(format!("lock helper exited with status {status}")).into());
    }
    if !lock_was_busy {
        return Err(
            io::Error::other("parent did not observe cross-process lock contention").into(),
        );
    }

    println!("process_lock fixture: cross-process exclusive contention verified");
    Ok(())
}

fn hold_exclusive_lock(root: &Path, ready: &Path, release: &Path) -> FixtureResult {
    let store = LocalStore::open(LocalStoreConfig::new(root))?;
    let _session = store.session(SessionMode::Exclusive)?;
    fs::write(ready, b"ready")?;
    if !wait_for_path(release, Duration::from_secs(15)) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "parent did not release the child lock",
        )
        .into());
    }
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
