use super::*;
use layrs_lens_sdk::{LensReconcileLabelsContract, LensReconcileSideContract as Side};

fn policy() -> FileMergePolicy {
    FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::StructuralFallback,
        limits: FileMergeLimits::default(),
    }
}

fn request(base: &[u8], existing: &[u8], incoming: &[u8]) -> LensReconcileRequest {
    LensReconcileRequest::new(
        native_manifest(NativeLens::Code).pin().unwrap(),
        "src/example.ts",
        Some("text/typescript".into()),
        Side::present(base, Some(sha256_digest(base))),
        Side::present(existing, Some(sha256_digest(existing))),
        Side::present(incoming, Some(sha256_digest(incoming))),
        LensReconcileLabelsContract {
            existing: "step-a".into(),
            incoming: "step-b".into(),
        },
    )
}

fn merged(plan: &FileMergePlan) -> &LensReconcileContent {
    match &plan.outcome {
        FileMergeOutcome::Semantic(result) => result.resolved.as_ref().expect("clean result"),
        outcome => panic!("expected semantic result: {outcome:?}"),
    }
}

#[test]
fn code_and_text_merge_disjoint_changes_without_modifying_input() {
    let mut input = request(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nC\n");
    let before = input.clone();
    let code = reconcile_file(&input, policy()).unwrap();
    assert_eq!(merged(&code).bytes, b"A\nb\nC\n");
    assert_eq!(input, before);
    assert_eq!(code, reconcile_file(&input, policy()).unwrap());
    input.lens = native_manifest(NativeLens::Text).pin().unwrap();
    let text = reconcile_file(&input, policy()).unwrap();
    assert_eq!(merged(&text), merged(&code));
    std::mem::swap(&mut input.existing, &mut input.incoming);
    assert_eq!(
        merged(&reconcile_file(&input, policy()).unwrap()),
        merged(&code)
    );
}

#[test]
fn line_endings_unicode_and_missing_final_newline_are_exact() {
    for (base, a, b, result) in [
        ("a\r\nb\r\nc", "é\r\nb\r\nc", "a\r\nb\r\n雪", "é\r\nb\r\n雪"),
        ("a\nb\nc", "é\nb\nc", "a\nb\n雪", "é\nb\n雪"),
    ] {
        let input = request(base.as_bytes(), a.as_bytes(), b.as_bytes());
        assert_eq!(
            merged(&reconcile_file(&input, policy()).unwrap()).bytes,
            result.as_bytes()
        );
    }
}

#[test]
fn competing_edits_retain_original_variants_and_semantic_blocks() {
    let input = request(b"a\n", b"left\n", b"right\n");
    let plan = reconcile_file(&input, policy()).unwrap();
    let FileMergeOutcome::Semantic(result) = &plan.outcome else {
        panic!("semantic conflict")
    };
    assert_eq!(result.status, LensReconcileResultStatus::Conflicted);
    assert_eq!(result.blocks.len(), 1);
    assert_eq!(plan.request, input);
    assert_eq!(
        plan.choose_value(FileValueChoice::Existing).bytes,
        b"left\n"
    );
    assert_eq!(
        plan.choose_value(FileValueChoice::Incoming).bytes,
        b"right\n"
    );
    assert_eq!(plan, reconcile_file(&input, policy()).unwrap());
}

#[test]
fn competing_insertions_never_accept_side_order_as_content_order() {
    let input = request(b"a\n", b"left\na\n", b"right\na\n");
    let plan = reconcile_file(&input, policy()).unwrap();
    assert_eq!(
        plan.outcome,
        FileMergeOutcome::Structural(FallbackReason::OrderDependentOutput)
    );
    assert_eq!(plan.request, input);
}

#[test]
fn binary_conflicts_preserve_exact_bytes_without_markers() {
    let input = request(&[0xff, 0], &[0xfe, 0], &[0xfd, 0]);
    let plan = reconcile_file(&input, policy()).unwrap();
    assert_eq!(
        plan.outcome,
        FileMergeOutcome::Structural(FallbackReason::UnsupportedContent)
    );
    assert_eq!(plan.choose_value(FileValueChoice::Base).bytes, [0xff, 0]);
    assert_eq!(
        plan.choose_value(FileValueChoice::Incoming).bytes,
        [0xfd, 0]
    );
}

#[test]
fn absent_is_not_an_empty_file_in_structural_resolution() {
    let mut input = request(b"original", b"", b"changed");
    input.existing = Side::absent();
    let plan = reconcile_file(&input, policy()).unwrap();
    assert_eq!(
        plan.outcome,
        FileMergeOutcome::Structural(FallbackReason::UnsupportedContent)
    );
    assert!(!plan.choose_value(FileValueChoice::Existing).exists);
    input.existing = Side::present(b"", None);
    let plan = reconcile_file(&input, policy()).unwrap();
    assert!(plan.choose_value(FileValueChoice::Existing).exists);
    assert!(!plan.choose_value(FileValueChoice::Delete).exists);
}

#[test]
fn unchanged_binary_and_deletion_use_exact_trivial_rules() {
    let mut input = request(&[0xff], &[0xff], &[0xfe]);
    assert_eq!(
        merged(&reconcile_file(&input, policy()).unwrap()).bytes,
        [0xfe]
    );
    input.incoming = Side::absent();
    assert!(!merged(&reconcile_file(&input, policy()).unwrap()).exists);
    input.base = Side::absent();
    input.existing = Side::present(b"", None);
    assert!(merged(&reconcile_file(&input, policy()).unwrap()).exists);
}

#[test]
fn missing_lens_fallback_is_explicit_and_reject_policy_is_honored() {
    let mut input = request(b"a", b"b", b"c");
    input.lens.lens_id = "vendor.unknown".into();
    let plan = reconcile_file(&input, policy()).unwrap();
    assert_eq!(
        plan.outcome,
        FileMergeOutcome::Structural(FallbackReason::UnsupportedLens)
    );
    let strict = FileMergePolicy {
        unsupported: FailureAction::Reject,
        ..policy()
    };
    assert_eq!(
        reconcile_file(&input, strict),
        Err(FileMergeError::Rejected(FallbackReason::UnsupportedLens))
    );
}

#[test]
fn invalid_input_and_pins_never_fall_back() {
    let mut input = request(b"a", b"b", b"c");
    input.lens.digest = sha256_digest(b"wrong implementation");
    assert_eq!(
        reconcile_file(&input, policy()),
        Err(FileMergeError::PinMismatch)
    );
    input = request(b"a", b"b", b"c");
    input.base.content_hash = Some(sha256_digest(b"wrong content"));
    assert_eq!(
        reconcile_file(&input, policy()),
        Err(FileMergeError::ContentDigestMismatch)
    );
    input = request(b"a", b"b", b"c");
    input.base.size = 99;
    assert!(matches!(
        reconcile_file(&input, policy()),
        Err(FileMergeError::InvalidInput(_))
    ));
    input = request(b"a", b"b", b"c");
    input.path = "../escape".into();
    assert!(matches!(
        reconcile_file(&input, policy()),
        Err(FileMergeError::InvalidInput(_))
    ));
}

#[test]
fn repository_paths_use_canonical_nfc_and_component_rules() {
    let mut input = request(b"a", b"a", b"b");
    for invalid in [
        "src/cafe\u{301}.ts",
        "src/../escape.ts",
        "../escape",
        "C:/drive.ts",
    ] {
        input.path = invalid.into();
        assert!(
            matches!(
                reconcile_file(&input, policy()),
                Err(FileMergeError::InvalidInput(_))
            ),
            "accepted non-canonical path {invalid:?}"
        );
    }
    input.path = format!("src/{}.ts", "a".repeat(256));
    assert!(matches!(
        reconcile_file(&input, policy()),
        Err(FileMergeError::InvalidInput(_))
    ));
    for valid in ["src/café.ts", "src/feature:variant.ts"] {
        input.path = valid.into();
        assert_eq!(
            merged(&reconcile_file(&input, policy()).unwrap()).bytes,
            b"b"
        );
    }
}

#[test]
fn quotas_are_enforced_before_execution_and_fallback_preserves_values() {
    let input = request(b"a\nb\nc", b"A\nb\nc", b"a\nb\nC");
    let mut bounded = policy();
    bounded.limits.max_diff_cells = 1;
    let plan = reconcile_file(&input, bounded).unwrap();
    assert_eq!(
        plan.outcome,
        FileMergeOutcome::Structural(FallbackReason::DiffWorkLimit)
    );
    assert_eq!(plan.request, input);
    bounded.quota = FailureAction::Reject;
    assert_eq!(
        reconcile_file(&input, bounded),
        Err(FileMergeError::Rejected(FallbackReason::DiffWorkLimit))
    );
    bounded = policy();
    bounded.limits.max_output_bytes = 1;
    assert_eq!(
        reconcile_file(&input, bounded).unwrap().outcome,
        FileMergeOutcome::Structural(FallbackReason::OutputLimit)
    );
    bounded.limits.max_input_bytes = 1;
    assert_eq!(
        reconcile_file(&input, bounded),
        Err(FileMergeError::Rejected(FallbackReason::InputLimit))
    );
}

#[test]
fn opted_in_manifests_are_valid_and_legacy_code_stays_compatible() {
    for lens in [NativeLens::Code, NativeLens::Text] {
        let manifest = native_manifest(lens);
        manifest.validate().unwrap();
        assert!(
            manifest
                .analyzer
                .capabilities
                .contains(&layrs_lens_sdk::LensCapability::Reconcile)
        );
        assert!(!manifest.resolution_methods.block.is_empty());
        assert!(manifest.pin().is_some());
    }
    assert_eq!(layrs_lens_code::manifest().version, "1.0.0");
    assert_eq!(native_manifest(NativeLens::Code).version, "1.1.0");
    assert_ne!(
        native_manifest(NativeLens::Code).pin(),
        layrs_lens_code::manifest().pin()
    );
}
