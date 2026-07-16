use serde::{Deserialize, Serialize};
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
fn canonical_reconcile_deserialization_rejects_contradictory_states() {
    let both_payloads = serde_json::json!({
        "status": "auto_resolved",
        "summary": "contradictory",
        "resolved": {"exists": true, "bytes": [1]},
        "conflict": {"exists": true, "bytes": [2]},
        "resolutionMethods": [],
        "blocks": [],
        "segments": [],
        "fields": {}
    });
    let error = serde_json::from_value::<LensReconcileResult>(both_payloads)
        .expect_err("resolved plus conflict must be rejected");
    assert!(error.to_string().contains("forbids conflict content"));

    let unsupported_methods = serde_json::json!({
        "status": "unsupported",
        "summary": "contradictory",
        "resolutionMethods": [{"method": "manual", "scope": "file"}],
        "blocks": [],
        "segments": [],
        "fields": {}
    });
    let error = serde_json::from_value::<LensReconcileResult>(unsupported_methods)
        .expect_err("unsupported methods must be rejected");
    assert!(error.to_string().contains("forbids resolution methods"));

    let missing_conflict = serde_json::json!({
        "status": "conflicted",
        "summary": "missing conflict",
        "resolutionMethods": [],
        "blocks": [],
        "segments": [],
        "fields": {}
    });
    let error = serde_json::from_value::<LensReconcileResult>(missing_conflict)
        .expect_err("conflicted without conflict must be rejected");
    assert!(error.to_string().contains("requires conflict content"));
}

#[test]
fn reconcile_tombstones_require_coherent_bytes_size_hash_and_existence() {
    let present_empty = serde_json::json!({"exists": true, "bytes": [], "size": 0});
    serde_json::from_value::<LensReconcileSideContract>(present_empty)
        .expect("an empty present file is valid");

    let vectors = contract_negative_vectors();
    for vector in vectors.reconcile_sides {
        assert!(
            serde_json::from_value::<LensReconcileSideContract>(vector.value).is_err(),
            "{}",
            vector.name
        );
    }

    serde_json::from_value::<LensReconcileContent>(
        serde_json::json!({"exists": true, "bytes": []}),
    )
    .expect("an empty present result is valid");
    serde_json::from_value::<LensReconcileContent>(
        serde_json::json!({"exists": false, "bytes": []}),
    )
    .expect("a canonical tombstone is valid");
    for vector in vectors.reconcile_contents {
        assert!(
            serde_json::from_value::<LensReconcileContent>(vector.value).is_err(),
            "{}",
            vector.name
        );
    }

    let mut invalid_tombstone = LensReconcileSideContract::absent();
    invalid_tombstone.bytes.push(1);
    assert!(serde_json::to_value(invalid_tombstone).is_err());
}

#[test]
fn canonical_reconcile_rejects_unknown_methods_and_legacy_alias_fields() {
    let banana = serde_json::json!({
        "status": "conflicted",
        "summary": "unknown method",
        "conflict": {"exists": true, "bytes": []},
        "resolutionMethods": [{"method": "banana", "scope": "file"}],
        "blocks": [],
        "segments": [],
        "fields": {}
    });
    assert!(serde_json::from_value::<LensReconcileResult>(banana).is_err());

    let contradictory_alias = serde_json::json!({
        "status": "conflicted",
        "summary": "legacy alias",
        "conflict": {"exists": true, "bytes": []},
        "resolutionMethods": [],
        "blocks": [{
            "blockId": "block-1",
            "base": "base",
            "existing": "canonical",
            "ours": "legacy",
            "incoming": "incoming",
            "resolutionMethods": []
        }],
        "segments": [{"kind": "block", "blockId": "block-1"}],
        "fields": {}
    });
    let error = serde_json::from_value::<LensReconcileResult>(contradictory_alias)
        .expect_err("canonical Rust input must reject legacy alias fields");
    assert!(error.to_string().contains("unknown field `ours`"));
}

#[test]
fn reconcile_constructors_and_serialization_refuse_invalid_states() {
    let error =
        LensReconcileResult::auto_resolved("resolved", LensReconcileContent::present(Vec::new()))
            .try_with_file_methods(vec![ResolutionMethod::Manual])
            .expect_err("auto_resolved methods must be rejected");
    assert!(error.message().contains("forbids resolution methods"));

    let block = LensConflictBlock::new(
        "block-1",
        "base",
        "existing",
        "incoming",
        vec![ResolutionMethod::Manual],
    );
    assert!(
        LensReconcileResult::try_conflicted(
            "orphan block",
            LensReconcileContent::present(Vec::new()),
            vec![block],
            Vec::new(),
        )
        .is_err()
    );

    let mut invalid = LensReconcileResult::unsupported("unsupported");
    invalid.conflict = Some(LensReconcileContent::present(Vec::new()));
    assert!(serde_json::to_value(invalid).is_err());
}

#[test]
fn pins_require_strict_sha256_digests() {
    for vector in contract_negative_vectors().pins {
        assert!(
            serde_json::from_value::<LensPin>(vector.value).is_err(),
            "{}",
            vector.name
        );
    }
    assert!(LensPin::try_new("layrs.test", "1.0.0", "sha256:abc").is_err());
    assert!(
        LensPin::try_new(
            "layrs.test",
            "1.0.0",
            "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )
        .is_err()
    );

    let pin = serde_json::json!({
        "lensId": "layrs.test",
        "version": "1.0.0",
        "digest": "sha256:not-a-digest"
    });
    assert!(serde_json::from_value::<LensPin>(pin).is_err());

    let mut manifest = LensManifest::new(
        "layrs.test",
        "Test",
        "1.0.0",
        AnalyzerContract {
            supported_media_types: Vec::new(),
            file_extensions: Vec::new(),
            capabilities: Vec::new(),
        },
        ViewerContract::new("viewer", "Viewer", Vec::new(), Vec::new()),
    );
    manifest.digest = Some("present-but-invalid".to_string());
    assert!(manifest.pin().is_none());
    assert!(manifest.try_pin().is_err());
    assert!(serde_json::to_value(manifest).is_err());
}

#[test]
fn reconcile_responses_bind_to_the_validated_executed_implementation() {
    let mut manifest = LensManifest::new(
        "layrs.test.reconcile",
        "Reconcile",
        "1.0.0",
        AnalyzerContract {
            supported_media_types: vec!["text/plain".to_string()],
            file_extensions: vec!["txt".to_string()],
            capabilities: vec![LensCapability::Reconcile],
        },
        ViewerContract::new(
            "layrs.viewer.test.reconcile",
            "ReconcileViewer",
            vec![PreviewKind::Text],
            vec![DiffKind::TextLines],
        ),
    )
    .with_digest(LENS_DIGEST)
    .with_bundle(golden_bundle());
    manifest.resolution_methods = ResolutionMethods::file(vec![ResolutionMethod::Existing]);

    let implementation = b"immutable reconcile implementation";
    let mut executable_manifest = manifest.clone();
    executable_manifest
        .bundle
        .as_mut()
        .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
        .expect("reconcile entrypoint")
        .digest = sha256_digest(implementation);
    assert!(
        LoadedLensReconcileImplementation::try_load(&executable_manifest, b"different bytes")
            .is_err()
    );
    let loaded = LoadedLensReconcileImplementation::try_load(&executable_manifest, implementation)
        .expect("loader hashes the immutable reconcile implementation bytes");
    let execution = loaded.execution();
    assert_eq!(
        execution.implementation_digest(),
        sha256_digest(implementation)
    );

    let requested = manifest.try_pin().expect("valid manifest").expect("pin");
    let response = LensReconcileResponse::try_for_execution(
        execution,
        &requested,
        LensReconcileResult::unsupported("unsupported"),
    )
    .expect("matching response pin");
    assert_eq!(response.lens, requested);

    for vector in contract_negative_vectors().response_pin_mismatches {
        let mut mismatched = requested.clone();
        if let Some(lens_id) = vector.pin_override.lens_id {
            mismatched.lens_id = lens_id;
        }
        if let Some(version) = vector.pin_override.version {
            mismatched.version = version;
        }
        if let Some(digest) = vector.pin_override.digest {
            mismatched.digest = digest;
        }
        assert!(
            LensReconcileResponse::try_for_execution(
                execution,
                &mismatched,
                LensReconcileResult::unsupported("unsupported"),
            )
            .is_err(),
            "{}",
            vector.name
        );
    }
}

#[test]
fn loaded_execution_owns_the_bytes_it_proves() {
    let mut manifest = golden_contract().manifest;
    let mut source = b"implementation-v1".to_vec();
    manifest
        .bundle
        .as_mut()
        .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
        .expect("reconcile entrypoint")
        .digest = sha256_digest(&source);

    let loaded = LoadedLensReconcileImplementation::try_load(&manifest, &source)
        .expect("implementation loads");
    source[0] = b'X';

    assert_eq!(loaded.implementation_bytes(), b"implementation-v1");
    assert_ne!(
        loaded.execution().implementation_digest(),
        sha256_digest(&source)
    );
}

#[test]
fn rust_bundle_validation_matches_shared_vectors() {
    let vectors: BundleValidationVectors = serde_json::from_str(include_str!(
        "../../../packages/lens-sdk/test/bundle-validation.vectors.json"
    ))
    .expect("bundle validation vectors parse");

    for vector in vectors.paths {
        let mut manifest = golden_contract().manifest;
        manifest
            .bundle
            .as_mut()
            .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
            .expect("reconcile entrypoint")
            .path = vector.value;
        assert_eq!(manifest.validate().is_ok(), vector.valid, "{}", vector.name);
    }
    for vector in vectors.quotas {
        let mut manifest = golden_contract().manifest;
        manifest
            .bundle
            .as_mut()
            .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
            .expect("reconcile entrypoint")
            .resources
            .max_memory_bytes = vector.value;
        assert_eq!(manifest.validate().is_ok(), vector.valid, "{}", vector.name);
    }
}

#[test]
fn rust_contract_matches_the_shared_golden_fixture() {
    let actual = serde_json::to_value(golden_contract()).expect("contract serializes");
    let expected: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/lenses/v1/contract.golden.json"
    ))
    .expect("golden fixture parses");

    let _: GoldenContract = serde_json::from_value(expected.clone())
        .expect("golden fixture satisfies the Rust canonical contract");

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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GoldenContract {
    manifest: LensManifest,
    preview_window: LensPreviewWindow,
    diff_window: LensDiffWindow,
    reconcile_input: LensReconcileRequest,
    reconcile_results: GoldenReconcileResults,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GoldenReconcileResults {
    auto_resolved: LensReconcileResponse,
    conflicted: LensReconcileResponse,
    unsupported: LensReconcileResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractNegativeVectors {
    reconcile_sides: Vec<NamedValueVector>,
    reconcile_contents: Vec<NamedValueVector>,
    pins: Vec<NamedValueVector>,
    response_pin_mismatches: Vec<ResponsePinMismatchVector>,
}

#[derive(Deserialize)]
struct NamedValueVector {
    name: String,
    value: Value,
}

#[derive(Deserialize)]
struct ResponsePinMismatchVector {
    name: String,
    #[serde(rename = "override")]
    pin_override: PinOverride,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinOverride {
    lens_id: Option<String>,
    version: Option<String>,
    digest: Option<String>,
}

fn contract_negative_vectors() -> ContractNegativeVectors {
    serde_json::from_str(include_str!(
        "../../../packages/lens-sdk/test/contract-negative.vectors.json"
    ))
    .expect("shared negative contract vectors parse")
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

    let mut executable_manifest = manifest.clone();
    let implementation = b"golden reconcile implementation";
    executable_manifest
        .bundle
        .as_mut()
        .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
        .expect("reconcile entrypoint")
        .digest = sha256_digest(implementation);
    let loaded = LoadedLensReconcileImplementation::try_load(&executable_manifest, implementation)
        .expect("golden reconcile implementation matches its executable manifest");
    let auto_resolved =
        LensReconcileResponse::for_execution(loaded.execution(), &pin, auto_resolved);
    let conflicted = LensReconcileResponse::for_execution(loaded.execution(), &pin, conflicted);
    let unsupported = LensReconcileResponse::for_execution(loaded.execution(), &pin, unsupported);

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
            auto_resolved,
            conflicted,
            unsupported,
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

#[derive(Debug, Deserialize)]
struct BundleValidationVectors {
    paths: Vec<BundlePathVector>,
    quotas: Vec<BundleQuotaVector>,
}

#[derive(Debug, Deserialize)]
struct BundlePathVector {
    name: String,
    value: String,
    valid: bool,
}

#[derive(Debug, Deserialize)]
struct BundleQuotaVector {
    name: String,
    value: u64,
    valid: bool,
}
