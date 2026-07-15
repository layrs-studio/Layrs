use super::*;
use layrs_lens_sdk::{
    LENS_CONTRACT_VERSION, LensReconcileInput, LensReconcileResultStatus, LensReconcileSide,
    ReconcileStatus,
};

#[test]
fn manifest_matches_text_golden_contract() {
    let manifest = manifest();
    let pin = manifest.pin().expect("built-in Lens pin");

    assert_eq!(manifest.contract_version, LENS_CONTRACT_VERSION);
    assert_eq!(manifest.id, TEXT_LENS_ID);
    assert_eq!(manifest.name, "Text");
    assert_eq!(manifest.version, TEXT_LENS_VERSION);
    assert_eq!(manifest.digest.as_deref(), Some(TEXT_LENS_DIGEST));
    assert_eq!(pin.lens_id, TEXT_LENS_ID);
    assert_eq!(pin.version, TEXT_LENS_VERSION);
    assert_eq!(pin.digest, TEXT_LENS_DIGEST);
    assert_eq!(
        manifest.analyzer.supported_media_types,
        vec!["text/plain", "text/markdown"]
    );
    assert_eq!(
        manifest.analyzer.file_extensions,
        vec!["txt", "md", "markdown", "rst", "log"]
    );
    assert_eq!(
        manifest.analyzer.capabilities,
        vec![
            LensCapability::View,
            LensCapability::Diff,
            LensCapability::Reconcile,
            LensCapability::Metadata,
            LensCapability::Preview,
            LensCapability::References,
        ]
    );
    assert_eq!(manifest.viewer.preview_kinds, vec![PreviewKind::Text]);
    assert_eq!(manifest.viewer.diff_kinds, vec![DiffKind::TextLines]);
    assert!(manifest.viewer.reconcile_statuses.is_empty());
    assert!(manifest.resolution_methods.file.is_empty());
    assert_eq!(
        manifest.resolution_methods.block,
        vec![
            ResolutionMethod::Existing,
            ResolutionMethod::Incoming,
            ResolutionMethod::Both,
            ResolutionMethod::Manual,
        ]
    );
}

#[test]
fn preview_and_diff_remain_available_offline() {
    let output =
        analyze(AnalysisInput::new("artifact", b"one\ntwo\n").with_previous_bytes(b"one\nold\n"));

    assert_eq!(output.preview.expect("preview").kind, PreviewKind::Text);
    assert_eq!(output.diff.expect("diff").kind, DiffKind::TextLines);
    assert_eq!(output.reconcile.status, ReconcileStatus::Unsupported);
    assert_eq!(output.reconcile.summary, LEGACY_ANALYSIS_RECONCILE_SUMMARY);
}

#[test]
fn extracts_relative_and_url_references() {
    let text = r#"
import "./widgets/card.ts";
background: url('../assets/bg.png');
mask-image: URL(icons/mask.svg);
docs: assets/readme.md
skip: /var/tmp/file.txt https://example.test/app.css data:image/png;base64,aa
"#;

    let refs = extract_references(text);

    assert!(refs.iter().any(|reference| {
        reference.kind == ReferenceKind::RelativePath && reference.target == "./widgets/card.ts"
    }));
    assert!(refs.iter().any(|reference| {
        reference.kind == ReferenceKind::UrlFunction && reference.target == "../assets/bg.png"
    }));
    assert!(refs.iter().any(|reference| {
        reference.kind == ReferenceKind::UrlFunction && reference.target == "icons/mask.svg"
    }));
    assert!(refs.iter().any(|reference| {
        reference.kind == ReferenceKind::RelativePath && reference.target == "assets/readme.md"
    }));
    assert!(
        !refs
            .iter()
            .any(|reference| reference.target == "/var/tmp/file.txt")
    );
    assert!(
        !refs
            .iter()
            .any(|reference| reference.target.starts_with("https://"))
    );
    assert!(
        !refs
            .iter()
            .any(|reference| reference.target.starts_with("data:"))
    );
}

#[test]
fn reconcile_auto_merges_non_overlapping_edits() {
    let result = reconcile_case(
        b"a\nb\nc\nd\n",
        b"a\nours-b\nc\nd\n",
        b"a\nb\ntheirs-c\nd\n",
    );

    assert_eq!(result.status, LensReconcileResultStatus::AutoResolved);
    assert_eq!(
        String::from_utf8(result.resolved.expect("resolved").bytes).expect("utf8"),
        "a\nours-b\ntheirs-c\nd\n"
    );
}

#[test]
fn reconcile_auto_merges_same_edit_on_both_sides() {
    let result = reconcile_case(b"a\nb\n", b"a\nshared\n", b"a\nshared\n");

    assert_eq!(result.status, LensReconcileResultStatus::AutoResolved);
    assert_eq!(
        String::from_utf8(result.resolved.expect("resolved").bytes).expect("utf8"),
        "a\nshared\n"
    );
}

#[test]
fn reconcile_reports_canonical_text_conflict_blocks() {
    let result = reconcile_case(
        b"a\nx\nb\ny\nc\n",
        b"a\nours-x\nb\nours-y\nc\n",
        b"a\ntheirs-x\nb\ntheirs-y\nc\n",
    );

    assert_eq!(result.status, LensReconcileResultStatus::Conflicted);
    assert_eq!(result.blocks.len(), 2);
    assert_eq!(result.blocks[0].block_id, "block-1");
    assert_eq!(result.blocks[1].block_id, "block-2");
    assert_eq!(
        result.blocks[0].methods,
        vec![
            ResolutionMethod::Existing,
            ResolutionMethod::Incoming,
            ResolutionMethod::Both,
            ResolutionMethod::Manual,
        ]
    );
    assert!(result.file_methods.is_empty());
    let marked = String::from_utf8(result.conflict.expect("conflict").bytes).expect("utf8");
    assert_eq!(marked.matches("<<<<<<< target:target").count(), 2);
    assert!(marked.contains("ours-x"));
    assert!(marked.contains("theirs-y"));
}

#[test]
fn reconcile_rejects_non_utf8_text() {
    let result = reconcile_case(b"base\n", b"existing\n", b"\xffincoming\n");

    assert_eq!(result.status, LensReconcileResultStatus::Unsupported);
    assert!(result.summary.contains("not valid UTF-8"));
}

#[test]
fn reconcile_preserves_crlf_and_trailing_newline() {
    let result = reconcile_case(b"a\r\nb\r\n", b"a\r\nours\r\n", b"a\r\nours\r\n");

    assert_eq!(result.status, LensReconcileResultStatus::AutoResolved);
    assert_eq!(result.resolved.expect("resolved").bytes, b"a\r\nours\r\n");
}

#[test]
fn text_lens_refuses_invalid_or_incomplete_block_methods() {
    assert_eq!(
        resolve_text_block_choice("base\n", "ours\n", "theirs\n", "both", None).expect("resolve"),
        "ours\ntheirs\n"
    );
    assert_eq!(
        resolve_text_block_choice("base\n", "ours\n", "theirs\n", "incoming", None)
            .expect("incoming"),
        "theirs\n"
    );
    assert!(resolve_text_block_choice("base\n", "ours\n", "theirs\n", "manual", None).is_err());
    assert!(resolve_text_block_choice("base\n", "ours\n", "theirs\n", "base", None).is_err());
}

#[test]
fn lens_assembles_text_resolution_from_segments() {
    let blocks = [layrs_lens_sdk::LensBlockResolutionInput {
        block_id: "block-1",
        base: "base\n",
        existing: "existing\n",
        incoming: "incoming\n",
        method: ResolutionMethod::Incoming,
        manual_text: None,
        resolved_text: None,
    }];
    let segments = [
        layrs_lens_sdk::LensResolutionSegment {
            kind: LensConflictSegmentKind::Text,
            text: Some("before\n"),
            block_id: None,
        },
        layrs_lens_sdk::LensResolutionSegment {
            kind: LensConflictSegmentKind::Block,
            text: None,
            block_id: Some("block-1"),
        },
    ];

    let content = resolve_text_conflict(&blocks, &segments).expect("resolved");

    assert_eq!(content.bytes, b"before\nincoming\n");
}

fn reconcile_case(base: &[u8], existing: &[u8], incoming: &[u8]) -> LensReconcileResult {
    reconcile_text(LensReconcileInput {
        path: None,
        media_type: Some("text/plain"),
        base: LensReconcileSide::present(base, None),
        ours: LensReconcileSide::present(existing, None),
        theirs: LensReconcileSide::present(incoming, None),
        ours_label: "target",
        theirs_label: "source",
    })
}
