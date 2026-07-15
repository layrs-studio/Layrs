use serde::Serialize;
use serde_json::Value;

use super::*;

const LENS_DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ANALYZER_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RECONCILE_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const VIEWER_DIGEST: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

#[test]
fn default_output_declares_reconcile_unsupported() {
    let metadata = ArtifactMetadata::new(
        "artifact",
        "layrs.test",
        ArtifactKind::Raw,
        "application/octet-stream",
        b"bytes",
    );

    let output = AnalysisOutput::new(metadata);

    assert_eq!(output.reconcile.status, ReconcileStatus::Unsupported);
}

#[test]
fn viewer_contract_does_not_invent_reconcile_support() {
    let viewer = ViewerContract::new(
        "viewer",
        "Component",
        vec![PreviewKind::Text],
        vec![DiffKind::TextLines],
    );

    assert!(viewer.reconcile_statuses.is_empty());
}

#[test]
fn manifest_defaults_to_versioned_unpinned_contract() {
    let manifest = LensManifest::new(
        "lens",
        "Lens",
        "0.0.0",
        AnalyzerContract {
            supported_media_types: Vec::new(),
            file_extensions: Vec::new(),
            capabilities: Vec::new(),
        },
        ViewerContract::new("viewer", "Viewer", Vec::new(), Vec::new()),
    );

    assert_eq!(manifest.contract_version, LENS_CONTRACT_VERSION);
    assert_eq!(manifest.resolution_methods, ResolutionMethods::none());
    assert!(manifest.pin().is_none());
}

#[test]
fn resolution_methods_have_stable_wire_labels_and_aliases() {
    assert_eq!(ResolutionMethod::Existing.as_str(), "existing");
    assert_eq!(ResolutionMethod::Incoming.as_str(), "incoming");
    assert_eq!(ResolutionMethod::Both.as_str(), "both");
    assert_eq!(ResolutionMethod::Manual.as_str(), "manual");
    assert_eq!(
        ResolutionMethod::from_label("ours"),
        Some(ResolutionMethod::Existing)
    );
    assert_eq!(
        ResolutionMethod::from_label("theirs"),
        Some(ResolutionMethod::Incoming)
    );
    assert_eq!(
        ResolutionMethod::from_label("both_theirs_then_ours"),
        Some(ResolutionMethod::Both)
    );
    assert_eq!(ResolutionMethod::from_label("base"), None);
}

#[test]
fn reconcile_statuses_have_stable_wire_labels() {
    assert_eq!(ReconcileStatus::Unsupported.as_str(), "unsupported");
    assert_eq!(
        ReconcileStatus::NeedsManualResolution.as_str(),
        "needs_manual_resolution"
    );
    assert_eq!(ReconcileStatus::AutoResolvable.as_str(), "auto_resolvable");
}

#[test]
fn scoped_methods_reject_the_wrong_scope() {
    let value = serde_json::json!([{"method": "existing", "scope": "block"}]);
    let error = serde_json::from_value::<ResolutionMethods>(value)
        .expect("mixed manifest scopes remain valid")
        .file;
    assert!(error.is_empty());

    let result = serde_json::json!({
        "status": "conflicted",
        "summary": "conflict",
        "resolutionMethods": [{"method": "existing", "scope": "block"}],
        "blocks": [],
        "segments": [],
        "fields": {}
    });
    assert!(serde_json::from_value::<LensReconcileResult>(result).is_err());
}

#[test]
fn rust_contract_matches_the_shared_golden_fixture() {
    let actual = serde_json::to_value(golden_contract()).expect("contract serializes");
    let expected: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/lenses/v1/contract.golden.json"
    ))
    .expect("golden fixture parses");

    assert_eq!(actual, expected);
}

#[test]
fn canonical_schema_is_valid_json_with_expected_definitions() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../schemas/lenses/v1/lens-contract.schema.json"
    ))
    .expect("schema parses");

    assert_eq!(
        schema.get("$id").and_then(Value::as_str),
        Some("https://schemas.layrs.dev/lenses/v1/lens-contract.schema.json")
    );
    let definitions = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("schema definitions");
    for name in [
        "lensManifest",
        "previewWindow",
        "diffWindow",
        "reconcileRequest",
        "reconcileResponse",
    ] {
        assert!(definitions.contains_key(name), "missing {name}");
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoldenContract {
    manifest: LensManifest,
    preview_window: LensPreviewWindow,
    diff_window: LensDiffWindow,
    reconcile_input: LensReconcileRequest,
    reconcile_results: GoldenReconcileResults,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoldenReconcileResults {
    auto_resolved: LensReconcileResponse,
    conflicted: LensReconcileResponse,
    unsupported: LensReconcileResponse,
}

fn golden_contract() -> GoldenContract {
    let pin = LensPin::new("layrs.example.markdown", "1.2.3", LENS_DIGEST);
    let mut viewer = ViewerContract::new(
        "layrs.viewer.example.markdown",
        "MarkdownViewer",
        vec![PreviewKind::Text],
        vec![DiffKind::TextLines],
    );
    viewer.inspector_fields.push(InspectorField {
        key: "wordCount".to_string(),
        label: "Word count".to_string(),
        value_type: InspectorValueType::Number,
    });

    let mut manifest = LensManifest::new(
        "layrs.example.markdown",
        "Markdown",
        "1.2.3",
        AnalyzerContract {
            supported_media_types: vec!["text/markdown".to_string()],
            file_extensions: vec!["md".to_string(), "mdown".to_string()],
            capabilities: vec![
                LensCapability::View,
                LensCapability::Preview,
                LensCapability::Diff,
                LensCapability::Reconcile,
                LensCapability::Metadata,
            ],
        },
        viewer,
    )
    .with_digest(LENS_DIGEST)
    .with_bundle(golden_bundle());
    manifest.resolution_methods = ResolutionMethods {
        file: vec![ResolutionMethod::Existing, ResolutionMethod::Incoming],
        block: vec![
            ResolutionMethod::Existing,
            ResolutionMethod::Incoming,
            ResolutionMethod::Both,
            ResolutionMethod::Manual,
        ],
    };

    let mut auto_resolved = LensReconcileResult::auto_resolved(
        "Existing and incoming content converge",
        LensReconcileContent::present(b"merged\n".to_vec()),
    );
    auto_resolved.fields.insert(
        "strategy".to_string(),
        MetadataValue::String("threeWay".to_string()),
    );

    let mut conflicted = LensReconcileResult::conflicted(
        "One text block requires resolution",
        LensReconcileContent::present(b"ours\n".to_vec()),
        vec![LensConflictBlock::new(
            "block-1",
            "base\n",
            "ours\n",
            "theirs\n",
            vec![
                ResolutionMethod::Existing,
                ResolutionMethod::Incoming,
                ResolutionMethod::Both,
                ResolutionMethod::Manual,
            ],
        )],
        vec![
            LensConflictSegment {
                kind: LensConflictSegmentKind::Text,
                text: Some("before\n".to_string()),
                block_id: None,
            },
            LensConflictSegment {
                kind: LensConflictSegmentKind::Block,
                text: None,
                block_id: Some("block-1".to_string()),
            },
            LensConflictSegment {
                kind: LensConflictSegmentKind::Text,
                text: Some("after\n".to_string()),
                block_id: None,
            },
        ],
    )
    .with_file_methods(vec![ResolutionMethod::Existing, ResolutionMethod::Incoming]);
    conflicted.fields.insert(
        "strategy".to_string(),
        MetadataValue::String("textBlocks".to_string()),
    );

    let mut unsupported = LensReconcileResult::unsupported("Lens cannot reconcile this media type");
    unsupported.fields.insert(
        "reasonCode".to_string(),
        MetadataValue::String("mediaType".to_string()),
    );

    GoldenContract {
        manifest,
        preview_window: LensPreviewWindow::new(
            pin.clone(),
            "artifact-123",
            LensPreviewWindowUnit::Lines,
            LensWindowRange::new(10, 30, 120),
        ),
        diff_window: LensDiffWindow {
            contract_version: LENS_CONTRACT_VERSION.to_string(),
            lens: pin.clone(),
            path: "docs/readme.md".to_string(),
            lines: LensWindowRange::new(40, 60, 200),
            columns: Some(LensWindowRange::new(0, 120, 400)),
        },
        reconcile_input: golden_reconcile_request(pin.clone()),
        reconcile_results: GoldenReconcileResults {
            auto_resolved: LensReconcileResponse::new(pin.clone(), auto_resolved),
            conflicted: LensReconcileResponse::new(pin.clone(), conflicted),
            unsupported: LensReconcileResponse::new(pin, unsupported),
        },
    }
}

fn golden_reconcile_request(pin: LensPin) -> LensReconcileRequest {
    LensReconcileRequest::new(
        pin,
        "docs/readme.md",
        Some("text/markdown".to_string()),
        LensReconcileSideContract::present(
            b"base\n".to_vec(),
            Some(
                "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                    .to_string(),
            ),
        ),
        LensReconcileSideContract::present(
            b"ours\n".to_vec(),
            Some(
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                    .to_string(),
            ),
        ),
        LensReconcileSideContract::present(
            b"theirs\n".to_vec(),
            Some(
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
            ),
        ),
        LensReconcileLabelsContract {
            existing: "Current layer".to_string(),
            incoming: "Incoming layer".to_string(),
        },
    )
}

fn golden_bundle() -> LensBundleContract {
    let analyzer_resources = LensResourceQuotas {
        max_memory_bytes: 67_108_864,
        max_execution_ms: 2_000,
        max_input_bytes: 16_777_216,
        max_output_bytes: 8_388_608,
        max_fuel: Some(10_000_000),
    };
    let reconcile_resources = LensResourceQuotas {
        max_memory_bytes: 67_108_864,
        max_execution_ms: 3_000,
        max_input_bytes: 33_554_432,
        max_output_bytes: 33_554_432,
        max_fuel: Some(20_000_000),
    };
    let viewer_resources = LensResourceQuotas {
        max_memory_bytes: 134_217_728,
        max_execution_ms: 1_000,
        max_input_bytes: 8_388_608,
        max_output_bytes: 8_388_608,
        max_fuel: None,
    };

    LensBundleContract {
        format: LENS_BUNDLE_FORMAT.to_string(),
        entrypoints: LensBundleEntrypoints {
            analyzer: Some(WasiLensEntrypoint::new(
                "analyzer.wasm",
                "analyze",
                ANALYZER_DIGEST,
                analyzer_resources,
            )),
            reconcile: Some(WasiLensEntrypoint::new(
                "reconcile.wasm",
                "reconcile",
                RECONCILE_DIGEST,
                reconcile_resources,
            )),
            viewer: Some(EsmLensEntrypoint::new(
                "viewer.mjs",
                "default",
                VIEWER_DIGEST,
                viewer_resources,
            )),
        },
        signature: Some(LensSignatureMetadata {
            algorithm: "ed25519".to_string(),
            key_id: "did:key:example#lens-signing".to_string(),
            signed_digest: LENS_DIGEST.to_string(),
            signature: "base64:ZXhhbXBsZS1zaWduYXR1cmU=".to_string(),
        }),
    }
}
