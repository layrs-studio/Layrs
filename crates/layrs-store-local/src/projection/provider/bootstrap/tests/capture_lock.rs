//! Isolated native capture-lane lock recipe. No capture transaction is written.
use super::*;
use crate::projection::provider::capture_lock::{CaptureAttemptLock, CaptureAttemptLockError};
use std::{
    fs,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::Storage::FileSystem::{LockFile, UnlockFile};

#[test]
#[ignore = "only the owned capture-lane recipe launches this exact external contender"]
fn external_contender() {
    let path = PathBuf::from(std::env::var_os("LAYRS_CAPTURE_LOCK_PATH").expect("lock path"));
    let expected = std::env::var("LAYRS_CAPTURE_LOCK_EXPECTED").expect("expected contention");
    let file = crate::layout::open_read_no_follow(&path).unwrap();
    // SAFETY: this local read handle stays alive through the matching unlock.
    let acquired = unsafe { LockFile(file.as_raw_handle(), 0, 1, 1, 0) } != 0;
    assert_eq!(acquired, expected == "free");
    if acquired {
        // SAFETY: same live handle and exact byte range as the successful lock.
        assert_ne!(unsafe { UnlockFile(file.as_raw_handle(), 0, 1, 1, 0) }, 0);
    }
}

fn external_probe(path: &std::path::Path, expected: &str) {
    let name = concat!(module_path!(), "::external_contender");
    let name = name.strip_prefix("layrs_store_local::").unwrap_or(name);
    let mut child = child::OwnedChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LAYRS_CAPTURE_LOCK_PATH", path)
            .env("LAYRS_CAPTURE_LOCK_EXPECTED", expected)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .creation_flags(0x08000000)
            .spawn()
            .expect("launch exact external capture contender"),
    );
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll contender") {
            assert!(status.success(), "external contender exited {status}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "contender timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact audited-host capture lock recipe"]
fn allocation_receipt_lane_excludes_processes_without_mutating_receipt() {
    child::run(
        concat!(
            module_path!(),
            "::allocation_receipt_lane_excludes_processes_without_mutating_receipt"
        ),
        120,
        || {
            child::declare();
            let fixture = Fixture::new();
            let published = fixture.published();
            let path = fixture.receipt(ProviderBootstrapReceipt::Allocation);
            let before = fs::read(&path).unwrap();
            let guard =
                CaptureAttemptLock::acquire(&published.held.artifacts, &published.held.family)
                    .unwrap();
            guard
                .verify(&published.held.artifacts, &published.held.family)
                .unwrap();
            assert!(matches!(
                CaptureAttemptLock::acquire(&published.held.artifacts, &published.held.family),
                Err(CaptureAttemptLockError::Contended)
            ));
            external_probe(&path, "busy");
            drop(guard);
            external_probe(&path, "free");
            let guard =
                CaptureAttemptLock::acquire(&published.held.artifacts, &published.held.family)
                    .unwrap();
            drop(guard);
            assert_eq!(fs::read(&path).unwrap(), before);
        },
    );
}
