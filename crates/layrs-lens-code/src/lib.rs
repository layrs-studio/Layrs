use layrs_lens_sdk::{
    AnalysisInput, AnalysisOutput, Analyzer, AnalyzerContract, ArtifactKind, ArtifactMetadata,
    DiffKind, InspectorField, InspectorValueType, LensCapability, LensManifest, LensResult,
    MetadataValue, PreviewKind, PreviewModel, ReconcileModel, ReconcileStatus, ViewerContract,
    infer_media_type_from_path,
};
use layrs_lens_text::{diff_lines, extract_references, line_stats};
use std::path::Path;

pub const CODE_LENS_ID: &str = "layrs.code";

#[derive(Debug, Default, Clone, Copy)]
pub struct CodeLens;

impl Analyzer for CodeLens {
    fn manifest(&self) -> LensManifest {
        manifest()
    }

    fn analyze(&self, input: AnalysisInput<'_>) -> LensResult<AnalysisOutput> {
        Ok(analyze(input))
    }
}

pub fn manifest() -> LensManifest {
    let mut viewer = ViewerContract::new(
        "layrs.viewer.code",
        "CodeArtifactViewer",
        vec![PreviewKind::Code],
        vec![DiffKind::TextLines],
    );
    viewer.reconcile_statuses = reconcile_statuses_v1();
    viewer.inspector_fields = vec![
        InspectorField {
            key: "language".to_string(),
            label: "Language".to_string(),
            value_type: InspectorValueType::String,
        },
        InspectorField {
            key: "line_count".to_string(),
            label: "Lines".to_string(),
            value_type: InspectorValueType::Number,
        },
        InspectorField {
            key: "reference_count".to_string(),
            label: "References".to_string(),
            value_type: InspectorValueType::Number,
        },
    ];

    LensManifest::new(
        CODE_LENS_ID,
        "Code",
        "0.0.0",
        AnalyzerContract {
            supported_media_types: vec![
                "text/rust".to_string(),
                "text/typescript".to_string(),
                "text/javascript".to_string(),
                "text/css".to_string(),
                "text/html".to_string(),
                "application/json".to_string(),
                "application/toml".to_string(),
                "application/yaml".to_string(),
                "text/x-go".to_string(),
                "text/x-python".to_string(),
            ],
            file_extensions: code_extensions()
                .iter()
                .map(|extension| (*extension).to_string())
                .collect(),
            capabilities: vec![
                LensCapability::View,
                LensCapability::Diff,
                LensCapability::Reconcile,
                LensCapability::Metadata,
                LensCapability::Preview,
                LensCapability::References,
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
    let text = String::from_utf8_lossy(input.bytes);
    let utf8_lossy = matches!(text, std::borrow::Cow::Owned(_));
    let text = text.as_ref();
    let stats = line_stats(text, utf8_lossy);
    let references = extract_references(text);
    let media_type = input
        .media_type
        .or_else(|| infer_media_type_from_path(input.path))
        .unwrap_or("text/plain");
    let language = input.path.and_then(language_from_path);

    let mut metadata = ArtifactMetadata::new(
        input.artifact_id,
        CODE_LENS_ID,
        ArtifactKind::Code,
        media_type,
        input.bytes,
    );
    metadata.fields.insert(
        "line_count".to_string(),
        MetadataValue::Unsigned(stats.line_count as u64),
    );
    metadata.fields.insert(
        "non_empty_line_count".to_string(),
        MetadataValue::Unsigned(stats.non_empty_line_count as u64),
    );
    metadata.fields.insert(
        "max_line_length".to_string(),
        MetadataValue::Unsigned(stats.max_line_length as u64),
    );
    metadata.fields.insert(
        "utf8_lossy".to_string(),
        MetadataValue::Bool(stats.utf8_lossy),
    );
    metadata.fields.insert(
        "reference_count".to_string(),
        MetadataValue::Unsigned(references.len() as u64),
    );
    if let Some(language) = language {
        metadata.fields.insert(
            "language".to_string(),
            MetadataValue::String(language.to_string()),
        );
    }

    let mut preview = PreviewModel::new(PreviewKind::Code, "Code artifact", media_type);
    preview.language = language.map(ToString::to_string);
    preview.body = Some(preview_body(text));
    preview.fields.insert(
        "line_count".to_string(),
        MetadataValue::Unsigned(stats.line_count as u64),
    );
    preview.fields.insert(
        "reference_count".to_string(),
        MetadataValue::Unsigned(references.len() as u64),
    );

    let diff = input.previous_bytes.map(|previous| {
        let previous = String::from_utf8_lossy(previous);
        diff_lines(previous.as_ref(), text)
    });
    let reconcile = reconcile_for_code_diff(diff.as_ref());

    let mut output = AnalysisOutput::new(metadata);
    output.preview = Some(preview);
    output.diff = diff;
    output.reconcile = reconcile;
    output.references = references;
    output
}

pub fn language_from_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some("rust"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "json" => Some("json"),
        "css" => Some("css"),
        "html" | "htm" => Some("html"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "py" => Some("python"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "swift" => Some("swift"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" => Some("cpp"),
        "cs" => Some("csharp"),
        "php" => Some("php"),
        "rb" => Some("ruby"),
        "sh" | "bash" | "zsh" | "ps1" => Some("shell"),
        "sql" => Some("sql"),
        _ => None,
    }
}

pub fn code_extensions() -> &'static [&'static str] {
    &[
        "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "css", "html", "htm", "toml", "yaml",
        "yml", "py", "go", "java", "kt", "kts", "swift", "c", "h", "cc", "cpp", "cxx", "hpp", "cs",
        "php", "rb", "sh", "bash", "zsh", "ps1", "sql",
    ]
}

pub fn reconcile_for_code_diff(diff: Option<&layrs_lens_sdk::DiffModel>) -> ReconcileModel {
    match diff {
        Some(diff) if diff.hunks.is_empty() => {
            ReconcileModel::auto_resolvable("No code changes to reconcile")
        }
        Some(_) => ReconcileModel::needs_manual_resolution(
            "Code changes require manual reconciliation in V1",
        ),
        None => ReconcileModel::unsupported("Reconciliation requires a previous code artifact"),
    }
}

fn preview_body(text: &str) -> String {
    const MAX_PREVIEW_BYTES: usize = 8 * 1024;
    if text.len() <= MAX_PREVIEW_BYTES {
        return text.to_string();
    }

    let mut end = MAX_PREVIEW_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_lens_sdk::ReconcileStatus;

    #[test]
    fn manifest_uses_code_identity_and_preview() {
        let manifest = manifest();

        assert_eq!(manifest.id, CODE_LENS_ID);
        assert_eq!(manifest.viewer.preview_kinds, vec![PreviewKind::Code]);
        assert!(
            manifest
                .analyzer
                .file_extensions
                .contains(&"rs".to_string())
        );
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
    }

    #[test]
    fn analyzes_code_with_line_diff_and_manual_reconcile_status() {
        let output = analyze(
            AnalysisInput::new("artifact", b"fn main() {}\n")
                .with_path(Path::new("src/main.rs"))
                .with_previous_bytes(b"fn old() {}\n"),
        );

        assert_eq!(output.metadata.kind, ArtifactKind::Code);
        assert_eq!(output.preview.as_ref().unwrap().kind, PreviewKind::Code);
        assert_eq!(
            output.reconcile.status,
            ReconcileStatus::NeedsManualResolution
        );
        assert!(output.diff.is_some());
    }
}
