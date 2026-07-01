use layrs_lens_sdk::{
    AnalysisInput, AnalysisOutput, Analyzer, AnalyzerContract, ArtifactKind, ArtifactMetadata,
    DiffKind, DiffModel, InspectorField, InspectorValueType, LensCapability, LensManifest,
    LensResult, MetadataValue, PreviewKind, PreviewModel, ReconcileModel, ReconcileStatus,
    ViewerContract, content_hash, infer_media_type_from_path,
};

pub const RAW_LENS_ID: &str = "layrs.raw";

#[derive(Debug, Default, Clone, Copy)]
pub struct RawLens;

impl Analyzer for RawLens {
    fn manifest(&self) -> LensManifest {
        manifest()
    }

    fn analyze(&self, input: AnalysisInput<'_>) -> LensResult<AnalysisOutput> {
        Ok(analyze(input))
    }
}

pub fn manifest() -> LensManifest {
    let mut viewer = ViewerContract::new(
        "layrs.viewer.raw",
        "RawArtifactViewer",
        vec![PreviewKind::Raw],
        vec![DiffKind::Binary],
    );
    viewer.reconcile_statuses = reconcile_statuses_v1();
    viewer.inspector_fields = vec![
        InspectorField {
            key: "byte_len".to_string(),
            label: "Size".to_string(),
            value_type: InspectorValueType::Number,
        },
        InspectorField {
            key: "content_hash".to_string(),
            label: "Hash".to_string(),
            value_type: InspectorValueType::String,
        },
        InspectorField {
            key: "media_type".to_string(),
            label: "Type".to_string(),
            value_type: InspectorValueType::String,
        },
    ];

    LensManifest::new(
        RAW_LENS_ID,
        "Raw",
        "0.0.0",
        AnalyzerContract {
            supported_media_types: vec!["application/octet-stream".to_string()],
            file_extensions: Vec::new(),
            capabilities: vec![
                LensCapability::View,
                LensCapability::Diff,
                LensCapability::Reconcile,
                LensCapability::Metadata,
                LensCapability::Preview,
            ],
        },
        viewer,
    )
}

fn reconcile_statuses_v1() -> Vec<ReconcileStatus> {
    vec![
        ReconcileStatus::Unsupported,
        ReconcileStatus::NeedsManualResolution,
        ReconcileStatus::AutoResolvable,
    ]
}

pub fn analyze(input: AnalysisInput<'_>) -> AnalysisOutput {
    let media_type = input
        .media_type
        .or_else(|| infer_media_type_from_path(input.path))
        .unwrap_or("application/octet-stream");

    let mut metadata = ArtifactMetadata::new(
        input.artifact_id,
        RAW_LENS_ID,
        ArtifactKind::Raw,
        media_type,
        input.bytes,
    );
    metadata.fields.insert(
        "hash_algorithm".to_string(),
        MetadataValue::String("fnv1a64".to_string()),
    );

    if let Some(path) = input.path {
        metadata.fields.insert(
            "path".to_string(),
            MetadataValue::String(path.to_string_lossy().into_owned()),
        );
    }

    let mut preview = PreviewModel::new(PreviewKind::Raw, "Raw artifact", media_type);
    preview.fields.insert(
        "byte_len".to_string(),
        MetadataValue::Unsigned(input.bytes.len() as u64),
    );
    preview.fields.insert(
        "content_hash".to_string(),
        MetadataValue::String(metadata.content_hash.clone()),
    );

    let mut output = AnalysisOutput::new(metadata);
    output.preview = Some(preview);
    output.diff = input
        .previous_bytes
        .map(|previous| diff_binary(previous, input.bytes));
    output.reconcile = reconcile_for_raw_diff(output.diff.as_ref());
    output
}

pub fn diff_binary(previous: &[u8], current: &[u8]) -> DiffModel {
    let previous_hash = content_hash(previous);
    let current_hash = content_hash(current);
    let changed = previous_hash != current_hash;
    let mut diff = DiffModel::new(
        DiffKind::Binary,
        if changed {
            "Binary content changed"
        } else {
            "No binary changes"
        },
    );
    diff.fields.insert(
        "previous_byte_len".to_string(),
        MetadataValue::Unsigned(previous.len() as u64),
    );
    diff.fields.insert(
        "current_byte_len".to_string(),
        MetadataValue::Unsigned(current.len() as u64),
    );
    diff.fields.insert(
        "previous_hash".to_string(),
        MetadataValue::String(previous_hash),
    );
    diff.fields.insert(
        "current_hash".to_string(),
        MetadataValue::String(current_hash),
    );
    diff.fields
        .insert("changed".to_string(), MetadataValue::Bool(changed));
    diff
}

pub fn reconcile_for_raw_diff(diff: Option<&DiffModel>) -> ReconcileModel {
    match diff.and_then(|diff| diff.fields.get("changed")) {
        Some(MetadataValue::Bool(false)) => {
            ReconcileModel::auto_resolvable("No raw byte changes to reconcile")
        }
        Some(MetadataValue::Bool(true)) => {
            ReconcileModel::unsupported("Raw byte reconciliation is not implemented in V1")
        }
        _ => ReconcileModel::unsupported("Reconciliation requires a previous raw artifact"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_lens_sdk::ReconcileStatus;

    #[test]
    fn manifest_declares_core_lens_capabilities() {
        let manifest = manifest();

        assert!(
            manifest
                .analyzer
                .capabilities
                .contains(&LensCapability::View)
        );
        assert!(
            manifest
                .analyzer
                .capabilities
                .contains(&LensCapability::Diff)
        );
        assert!(
            manifest
                .analyzer
                .capabilities
                .contains(&LensCapability::Reconcile)
        );
        assert_eq!(manifest.viewer.diff_kinds, vec![DiffKind::Binary]);
    }

    #[test]
    fn unchanged_binary_is_auto_resolvable() {
        let output = analyze(AnalysisInput::new("artifact", b"same").with_previous_bytes(b"same"));

        assert_eq!(output.reconcile.status, ReconcileStatus::AutoResolvable);
    }
}
