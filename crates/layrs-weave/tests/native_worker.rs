#![cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
use layrs_lens_sdk::{
    LensReconcileLabelsContract, LensReconcileRequest, LensReconcileSideContract as Side,
    sha256_digest,
};
use layrs_weave::{reconciliation::*, worker::*};
use std::path::Path;
fn request() -> LensReconcileRequest {
    let side = |bytes: &[u8]| Side::present(bytes, Some(sha256_digest(bytes)));
    LensReconcileRequest::new(
        native_manifest(NativeLens::Code).pin().unwrap(),
        "code.ts",
        Some("text/typescript".into()),
        side(b"a\nb\nc\n"),
        side(b"A\nb\nc\n"),
        side(b"a\nb\nC\n"),
        LensReconcileLabelsContract {
            existing: "A".into(),
            incoming: "B".into(),
        },
    )
}
fn policy() -> FileMergePolicy {
    FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::Reject,
        limits: FileMergeLimits::default(),
    }
}
#[test]
fn real_worker_matches_pure_engine_with_exact_pins_but_never_attests_ambient_isolation() {
    let executable = Path::new(env!("CARGO_BIN_EXE_layrs-lens-worker"));
    let input = request();
    let first = reconcile_in_worker(executable, &input, policy(), WorkerLimits::default()).unwrap();
    let second =
        reconcile_in_worker(executable, &input, policy(), WorkerLimits::default()).unwrap();
    assert_eq!(first.plan, reconcile_file(&input, policy()).unwrap());
    assert_eq!(first.plan, second.plan);
    assert!(!first.eligible_for_semantic_receipt());
    let mut invalid = input;
    invalid.lens.digest = format!("sha256:{}", "0".repeat(64));
    assert!(matches!(
        reconcile_in_worker(executable, &invalid, policy(), WorkerLimits::default()),
        Err(WorkerError::Reconciliation(FileMergeError::PinMismatch))
    ));
}

#[test]
fn real_process_wasm_host_matches_pure_output_and_checks_pinned_worker() {
    use layrs_weave::wasm_host::{self, SandboxLimits, WasmHost};
    let executable = Path::new(env!("CARGO_BIN_EXE_layrs-lens-worker"));
    let digest = sha256_digest(&std::fs::read(executable).unwrap());
    assert!(
        WasmHost::open(
            executable,
            &format!("sha256:{}", "0".repeat(64)),
            SandboxLimits::default()
        )
        .is_err()
    );
    let host = WasmHost::open(executable, &digest, SandboxLimits::default()).unwrap();
    let mut input = request();
    let expected = reconcile_file(&input, policy()).unwrap();
    input.lens = wasm_host::manifest(NativeLens::Code).pin().unwrap();
    let result = host.reconcile(&input, policy()).unwrap();
    assert_eq!(result.outcome, expected.outcome);
    assert_eq!(result.request, input);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn linux_pinned_worker_survives_replacement_and_rejects_symlink_paths() {
    use layrs_weave::wasm_host::{self, SandboxLimits, WasmHost};
    let executable = Path::new(env!("CARGO_BIN_EXE_layrs-lens-worker"));
    let digest = sha256_digest(&std::fs::read(executable).unwrap());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("layrs-worker-pin-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let copy = directory.join("worker");
    std::fs::copy(executable, &copy).unwrap();
    let host = WasmHost::open(&copy, &digest, SandboxLimits::default()).unwrap();
    let alias = directory.join("alias");
    std::os::unix::fs::symlink(&copy, &alias).unwrap();
    assert!(WasmHost::open(&alias, &digest, SandboxLimits::default()).is_err());
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&directory, &alias).unwrap();
    assert!(WasmHost::open(&alias.join("worker"), &digest, SandboxLimits::default()).is_err());
    std::fs::remove_file(&alias).unwrap();
    {
        use std::os::unix::ffi::OsStrExt;
        let fifo = std::ffi::CString::new(alias.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(WasmHost::open(&alias, &digest, SandboxLimits::default()).is_err());
        std::fs::remove_file(&alias).unwrap();
    }
    std::fs::write(&copy, b"file replaced after trusted pin").unwrap();
    std::fs::remove_file(copy).unwrap();
    std::fs::remove_dir(directory).unwrap();
    let mut input = request();
    let expected = reconcile_file(&input, policy()).unwrap();
    input.lens = wasm_host::manifest(NativeLens::Code).pin().unwrap();
    assert_eq!(
        host.reconcile(&input, policy()).unwrap().outcome,
        expected.outcome
    );
}
