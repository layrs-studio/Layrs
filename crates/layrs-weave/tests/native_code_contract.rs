//! The native 1.1 reconciliation contract is distinct from immutable 1.0
//! analysis/viewer metadata. No historical pin acquires new capabilities.
use layrs_lens_sdk::{
    LensCapability, LensReconcileLabelsContract, LensReconcileRequest, LensReconcileResultStatus,
    LensReconcileSideContract as Side, sha256_digest,
};
use layrs_weave::reconciliation::*;

fn policy() -> FileMergePolicy {
    FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::Reject,
        limits: Default::default(),
    }
}
fn request(base: &[u8], existing: &[u8], incoming: &[u8]) -> LensReconcileRequest {
    let side = |bytes: &[u8]| Side::present(bytes, Some(sha256_digest(bytes)));
    LensReconcileRequest::new(
        native_manifest(NativeLens::Code).pin().unwrap(),
        "src/code.ts",
        None,
        side(base),
        side(existing),
        side(incoming),
        LensReconcileLabelsContract {
            existing: "A".into(),
            incoming: "B".into(),
        },
    )
}

#[test]
fn reconciliation_is_a_separate_version_and_the_historical_pin_stays_immutable() {
    let legacy = layrs_lens_code::manifest();
    let opt_in = layrs_lens_code::reconciliation_manifest();
    let native = native_manifest(NativeLens::Code);
    legacy.validate().unwrap();
    opt_in.validate().unwrap();
    native.validate().unwrap();
    assert_eq!(legacy.version, "1.0.0");
    assert_eq!(
        legacy.digest.as_deref(),
        Some(layrs_lens_code::CODE_LENS_DIGEST)
    );
    assert!(
        !legacy
            .analyzer
            .capabilities
            .contains(&LensCapability::Reconcile)
    );
    assert_eq!(opt_in.version, "1.1.0");
    assert!(
        opt_in
            .analyzer
            .capabilities
            .contains(&LensCapability::Reconcile)
    );
    assert!(
        opt_in.pin().is_none(),
        "unhosted declaration must not claim a verified implementation"
    );
    assert_eq!(
        opt_in.resolution_methods,
        layrs_lens_text::resolution_methods()
    );
    assert!(!native.resolution_methods.block.is_empty());
    assert_eq!(native.version, "1.1.0");
    assert_ne!(native.pin(), legacy.pin());
    // These fields describe the legacy analysis viewer, not the native
    // LensReconcileResultStatus wire protocol or canonical Conflict UI.
    assert_eq!(opt_in.viewer, legacy.viewer);
    assert!(opt_in.viewer.reconcile_statuses.is_empty());
    let mut request = request(b"before", b"left", b"right");
    request.lens = legacy.pin().unwrap();
    assert_eq!(
        reconcile_file(&request, policy()),
        Err(FileMergeError::PinMismatch)
    );
    request.lens.version = "1.1.0".into();
    assert_eq!(
        reconcile_file(&request, policy()),
        Err(FileMergeError::PinMismatch),
        "changing the version alone never authorizes an old implementation"
    );
}

#[test]
fn native_code_merges_exact_lines_and_keeps_conflicting_terms() {
    let request = request(
        "é\r\nconst middle = 1;\r\nend".as_bytes(),
        "É\r\nconst middle = 1;\r\nend".as_bytes(),
        "é\r\nconst middle = 1;\r\nEND".as_bytes(),
    );
    let result = reconcile_file(&request, policy()).unwrap();
    let FileMergeOutcome::Semantic(semantic) = result.outcome else {
        panic!("native code should execute")
    };
    assert_eq!(semantic.status, LensReconcileResultStatus::AutoResolved);
    assert_eq!(
        semantic.resolved.unwrap().bytes,
        "É\r\nconst middle = 1;\r\nEND".as_bytes()
    );
    let conflict = request_fixture();
    let result = reconcile_file(&conflict, policy()).unwrap();
    let FileMergeOutcome::Semantic(ref semantic) = result.outcome else {
        panic!("expected native code blocks")
    };
    assert_eq!(semantic.status, LensReconcileResultStatus::Conflicted);
    assert!(!semantic.blocks.is_empty());
    assert_eq!(
        result.choose_value(FileValueChoice::Base).bytes,
        conflict.base.bytes
    );
    assert_eq!(
        result.choose_value(FileValueChoice::Existing).bytes,
        conflict.existing.bytes
    );
    assert_eq!(
        result.choose_value(FileValueChoice::Incoming).bytes,
        conflict.incoming.bytes
    );
}
fn request_fixture() -> LensReconcileRequest {
    request(
        b"const value = 1;\n",
        b"const value = 2;\n",
        b"const value = 3;\n",
    )
}

#[test]
fn native_code_refuses_to_decode_binary_or_invalid_utf8() {
    for input in [
        request(b"\0\xff", b"\0\xfe", b"\0\xfd"),
        request(b"\xff", b"\xfe", b"\xfd"),
    ] {
        let result = reconcile_file(&input, policy()).unwrap();
        assert!(matches!(
            result.outcome,
            FileMergeOutcome::Structural(FallbackReason::UnsupportedContent)
        ));
        assert_eq!(
            result.choose_value(FileValueChoice::Base).bytes,
            input.base.bytes
        );
        assert_eq!(
            result.choose_value(FileValueChoice::Existing).bytes,
            input.existing.bytes
        );
        assert_eq!(
            result.choose_value(FileValueChoice::Incoming).bytes,
            input.incoming.bytes
        );
    }
}
