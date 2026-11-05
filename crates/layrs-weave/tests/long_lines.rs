use layrs_lens_sdk::{
    LensReconcileLabelsContract, LensReconcileRequest, LensReconcileResultStatus,
    LensReconcileSideContract as Side, sha256_digest,
};
use layrs_weave::reconciliation::*;

fn request(base: &[u8], existing: &[u8], incoming: &[u8]) -> LensReconcileRequest {
    let side = |bytes: &[u8]| Side::present(bytes, Some(sha256_digest(bytes)));
    LensReconcileRequest::new(
        native_manifest(NativeLens::Text).pin().unwrap(),
        "long.txt",
        Some("text/plain".into()),
        side(base),
        side(existing),
        side(incoming),
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
fn resolved(plan: &FileMergePlan) -> &[u8] {
    let FileMergeOutcome::Semantic(result) = &plan.outcome else {
        panic!("semantic text result expected")
    };
    assert_eq!(result.status, LensReconcileResultStatus::AutoResolved);
    &result.resolved.as_ref().unwrap().bytes
}

#[test]
fn very_long_disjoint_unicode_lines_keep_exact_crlf_and_missing_final_newline() {
    let long = "é".repeat(32_768);
    let base = format!("{long}\r\nmiddle\r\n{long}");
    let left = format!("A{long}\r\nmiddle\r\n{long}");
    let right = format!("{long}\r\nmiddle\r\n{long}B");
    let expected = format!("A{long}\r\nmiddle\r\n{long}B");
    let input = request(base.as_bytes(), left.as_bytes(), right.as_bytes());
    assert_eq!(
        resolved(&reconcile_file(&input, policy()).unwrap()),
        expected.as_bytes()
    );
    let reverse = request(base.as_bytes(), right.as_bytes(), left.as_bytes());
    assert_eq!(
        resolved(&reconcile_file(&reverse, policy()).unwrap()),
        expected.as_bytes()
    );
}

#[test]
fn concurrent_edits_of_one_long_line_keep_all_original_choices_and_enforce_byte_limits() {
    let base = "x".repeat(128 * 1024);
    let left = format!("A{base}");
    let right = format!("B{base}");
    let input = request(base.as_bytes(), left.as_bytes(), right.as_bytes());
    let plan = reconcile_file(&input, policy()).unwrap();
    let FileMergeOutcome::Semantic(result) = &plan.outcome else {
        panic!("text conflict expected")
    };
    assert_eq!(result.status, LensReconcileResultStatus::Conflicted);
    assert_eq!(
        plan.choose_value(FileValueChoice::Base).bytes,
        base.as_bytes()
    );
    assert_eq!(
        plan.choose_value(FileValueChoice::Existing).bytes,
        left.as_bytes()
    );
    assert_eq!(
        plan.choose_value(FileValueChoice::Incoming).bytes,
        right.as_bytes()
    );
    let mut limited = policy();
    limited.limits.max_input_bytes = base.len();
    assert_eq!(
        reconcile_file(&input, limited),
        Err(FileMergeError::Rejected(FallbackReason::InputLimit))
    );
}

#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn line_16k_real_wasm_matches_pure_bytes_with_explicit_budget() {
    let measured = std::time::Instant::now();
    use layrs_weave::{
        wasm_host::{self, SandboxLimits, WasmHost},
        worker::WorkerError,
    };
    let long = "q".repeat(16 * 1024);
    let base = format!("{long}\nmiddle\n{long}");
    let left = format!("a{long}\nmiddle\n{long}");
    let right = format!("{long}\nmiddle\n{long}z");
    let mut input = request(base.as_bytes(), left.as_bytes(), right.as_bytes());
    let expected = reconcile_file(&input, policy()).unwrap();
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_layrs-lens-worker"));
    let digest = sha256_digest(&std::fs::read(executable).unwrap());
    let host = WasmHost::open(executable, &digest, SandboxLimits::default()).unwrap();
    input.lens = wasm_host::manifest(NativeLens::Text).pin().unwrap();
    assert!(matches!(
        host.reconcile(&input, policy()),
        Err(WorkerError::FuelLimit)
    ));
    drop(host);
    let limits = SandboxLimits {
        fuel: 1_000_000_000,
        process: layrs_weave::worker::WorkerLimits {
            deadline_ms: 30_000,
            ..SandboxLimits::default().process
        },
        ..SandboxLimits::default()
    };
    let host = WasmHost::open(executable, &digest, limits).unwrap();
    let actual = host.reconcile(&input, policy()).unwrap();
    assert_eq!(actual.outcome, expected.outcome);
    drop(host);
    let host = WasmHost::open(
        executable,
        &digest,
        SandboxLimits {
            linear_memory_bytes: 2 * 1024 * 1024,
            ..limits
        },
    )
    .unwrap();
    assert!(matches!(
        host.reconcile(&input, policy()),
        Err(WorkerError::WasmGrowthLimit)
    ));
    eprintln!(
        "wasm_long_line_16k_default_refusal_configured_success_memory_refusal_ms={}",
        measured.elapsed().as_millis()
    );
}

#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn line_1k_real_wasm_matches_pure_bytes_with_default_budget() {
    let measured = std::time::Instant::now();
    use layrs_weave::wasm_host::{self, WasmHost};
    let long = "q".repeat(1024);
    let base = format!("{long}\nmiddle\n{long}");
    let left = format!("a{long}\nmiddle\n{long}");
    let right = format!("{long}\nmiddle\n{long}z");
    let mut input = request(base.as_bytes(), left.as_bytes(), right.as_bytes());
    let expected = reconcile_file(&input, policy()).unwrap();
    input.lens = wasm_host::manifest(NativeLens::Text).pin().unwrap();
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_layrs-lens-worker"));
    let digest = sha256_digest(&std::fs::read(executable).unwrap());
    let host = WasmHost::open(executable, &digest, Default::default()).unwrap();
    assert_eq!(
        host.reconcile(&input, policy()).unwrap().outcome,
        expected.outcome
    );
    eprintln!(
        "wasm_long_line_1k_default_success_ms={}",
        measured.elapsed().as_millis()
    );
}

#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn line_64k_real_wasm_matches_pure_bytes_with_explicit_budget() {
    let measured = std::time::Instant::now();
    use layrs_weave::{
        wasm_host::{self, SandboxLimits, WasmHost},
        worker::{WorkerError, WorkerLimits},
    };
    let long = "q".repeat(64 * 1024);
    let base = format!("{long}\nmiddle\n{long}");
    let left = format!("a{long}\nmiddle\n{long}");
    let right = format!("{long}\nmiddle\n{long}z");
    let mut input = request(base.as_bytes(), left.as_bytes(), right.as_bytes());
    let expected = reconcile_file(&input, policy()).unwrap();
    input.lens = wasm_host::manifest(NativeLens::Text).pin().unwrap();
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_layrs-lens-worker"));
    let digest = sha256_digest(&std::fs::read(executable).unwrap());
    let host = WasmHost::open(executable, &digest, SandboxLimits::default()).unwrap();
    assert!(matches!(
        host.reconcile(&input, policy()),
        Err(WorkerError::FuelLimit)
    ));
    drop(host);
    let limits = SandboxLimits {
        fuel: 1_000_000_000,
        process: WorkerLimits {
            deadline_ms: 30_000,
            ..SandboxLimits::default().process
        },
        ..SandboxLimits::default()
    };
    let host = WasmHost::open(executable, &digest, limits).unwrap();
    assert_eq!(
        host.reconcile(&input, policy()).unwrap().outcome,
        expected.outcome
    );
    eprintln!(
        "wasm_long_line_64k_default_refusal_configured_success_ms={}",
        measured.elapsed().as_millis()
    );
}
