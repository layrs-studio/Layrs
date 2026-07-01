use layrs_lens_sdk::{
    AnalysisInput, AnalysisOutput, Analyzer, AnalyzerContract, ArtifactKind, ArtifactMetadata,
    DiffHunk, DiffKind, DiffLine, DiffModel, DiffOp, ExtractedReference, InspectorField,
    InspectorValueType, LensCapability, LensManifest, LensResult, MetadataValue, PreviewKind,
    PreviewModel, ReconcileModel, ReconcileStatus, ReferenceKind, ViewerContract,
    infer_media_type_from_path, span_at,
};

pub const TEXT_LENS_ID: &str = "layrs.text";

#[derive(Debug, Default, Clone, Copy)]
pub struct TextLens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineStats {
    pub line_count: usize,
    pub non_empty_line_count: usize,
    pub max_line_length: usize,
    pub trailing_newline: bool,
    pub utf8_lossy: bool,
}

impl Analyzer for TextLens {
    fn manifest(&self) -> LensManifest {
        manifest()
    }

    fn analyze(&self, input: AnalysisInput<'_>) -> LensResult<AnalysisOutput> {
        Ok(analyze(input))
    }
}

pub fn manifest() -> LensManifest {
    let mut viewer = ViewerContract::new(
        "layrs.viewer.text",
        "TextArtifactViewer",
        vec![PreviewKind::Text],
        vec![DiffKind::TextLines],
    );
    viewer.reconcile_statuses = reconcile_statuses_v1();
    viewer.inspector_fields = vec![
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
        InspectorField {
            key: "utf8_lossy".to_string(),
            label: "UTF-8 lossy".to_string(),
            value_type: InspectorValueType::Boolean,
        },
    ];

    LensManifest::new(
        TEXT_LENS_ID,
        "Text",
        "0.0.0",
        AnalyzerContract {
            supported_media_types: vec!["text/plain".to_string(), "text/markdown".to_string()],
            file_extensions: vec![
                "txt".to_string(),
                "md".to_string(),
                "markdown".to_string(),
                "rst".to_string(),
                "log".to_string(),
            ],
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

    let mut metadata = ArtifactMetadata::new(
        input.artifact_id,
        TEXT_LENS_ID,
        ArtifactKind::Text,
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
        "trailing_newline".to_string(),
        MetadataValue::Bool(stats.trailing_newline),
    );
    metadata.fields.insert(
        "utf8_lossy".to_string(),
        MetadataValue::Bool(stats.utf8_lossy),
    );
    metadata.fields.insert(
        "reference_count".to_string(),
        MetadataValue::Unsigned(references.len() as u64),
    );

    let mut preview = PreviewModel::new(PreviewKind::Text, "Text artifact", media_type);
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
    let reconcile = reconcile_for_text_diff(diff.as_ref());

    let mut output = AnalysisOutput::new(metadata);
    output.preview = Some(preview);
    output.diff = diff;
    output.reconcile = reconcile;
    output.references = references;
    output
}

pub fn line_stats(text: &str, utf8_lossy: bool) -> LineStats {
    LineStats {
        line_count: text.lines().count(),
        non_empty_line_count: text.lines().filter(|line| !line.trim().is_empty()).count(),
        max_line_length: text
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0),
        trailing_newline: text.ends_with('\n'),
        utf8_lossy,
    }
}

pub fn extract_references(text: &str) -> Vec<ExtractedReference> {
    let mut references = Vec::new();
    let url_spans = extract_url_functions(text, &mut references);
    extract_relative_paths(text, &url_spans, &mut references);
    references
}

pub fn diff_lines(previous: &str, current: &str) -> DiffModel {
    let previous_lines: Vec<&str> = previous.lines().collect();
    let current_lines: Vec<&str> = current.lines().collect();

    let mut diff = DiffModel::new(DiffKind::TextLines, "No line changes");
    if previous_lines == current_lines {
        return diff;
    }

    let mut lines = Vec::new();
    let mut inserted = 0usize;
    let mut deleted = 0usize;
    let max_len = previous_lines.len().max(current_lines.len());

    for index in 0..max_len {
        match (previous_lines.get(index), current_lines.get(index)) {
            (Some(old), Some(new)) if old == new => lines.push(DiffLine {
                op: DiffOp::Equal,
                old_line: Some(index + 1),
                new_line: Some(index + 1),
                text: (*old).to_string(),
            }),
            (Some(old), Some(new)) => {
                deleted += 1;
                inserted += 1;
                lines.push(DiffLine {
                    op: DiffOp::Delete,
                    old_line: Some(index + 1),
                    new_line: None,
                    text: (*old).to_string(),
                });
                lines.push(DiffLine {
                    op: DiffOp::Insert,
                    old_line: None,
                    new_line: Some(index + 1),
                    text: (*new).to_string(),
                });
            }
            (Some(old), None) => {
                deleted += 1;
                lines.push(DiffLine {
                    op: DiffOp::Delete,
                    old_line: Some(index + 1),
                    new_line: None,
                    text: (*old).to_string(),
                });
            }
            (None, Some(new)) => {
                inserted += 1;
                lines.push(DiffLine {
                    op: DiffOp::Insert,
                    old_line: None,
                    new_line: Some(index + 1),
                    text: (*new).to_string(),
                });
            }
            (None, None) => {}
        }
    }

    diff.summary = format!("{inserted} inserted, {deleted} deleted");
    diff.hunks.push(DiffHunk {
        old_start: 1,
        old_lines: previous_lines.len(),
        new_start: 1,
        new_lines: current_lines.len(),
        lines,
    });
    diff.fields.insert(
        "inserted_lines".to_string(),
        MetadataValue::Unsigned(inserted as u64),
    );
    diff.fields.insert(
        "deleted_lines".to_string(),
        MetadataValue::Unsigned(deleted as u64),
    );
    diff
}

pub fn reconcile_for_text_diff(diff: Option<&DiffModel>) -> ReconcileModel {
    match diff {
        Some(diff) if diff.hunks.is_empty() => {
            ReconcileModel::auto_resolvable("No text changes to reconcile")
        }
        Some(_) => ReconcileModel::needs_manual_resolution(
            "Text changes require manual reconciliation in V1",
        ),
        None => ReconcileModel::unsupported("Reconciliation requires a previous text artifact"),
    }
}

fn extract_url_functions(
    text: &str,
    references: &mut Vec<ExtractedReference>,
) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut cursor = 0usize;

    while let Some(relative_start) = find_ascii_case_insensitive(&text[cursor..], "url(") {
        let function_start = cursor + relative_start;
        let inner_start = function_start + 4;
        let Some(relative_end) = text[inner_start..].find(')') else {
            break;
        };
        let inner_end = inner_start + relative_end;
        let inner = &text[inner_start..inner_end];
        let left_trimmed = inner.trim_start();
        let mut target_start = inner_start + (inner.len() - left_trimmed.len());
        let right_trimmed = left_trimmed.trim_end();
        let mut target_end = target_start + right_trimmed.len();
        let target = &text[target_start..target_end];
        let target = trim_wrapping_quotes(target, &mut target_start, &mut target_end);

        spans.push((function_start, inner_end + 1));

        if !target.is_empty() && !target.starts_with("data:") {
            references.push(ExtractedReference {
                target: target.to_string(),
                kind: ReferenceKind::UrlFunction,
                span: span_at(text, target_start, target_end),
            });
        }

        cursor = inner_end + 1;
    }

    spans
}

fn extract_relative_paths(
    text: &str,
    excluded_spans: &[(usize, usize)],
    references: &mut Vec<ExtractedReference>,
) {
    let mut token_start = None;

    for (offset, ch) in text.char_indices() {
        if is_token_delimiter(ch) {
            if let Some(start) = token_start.take() {
                push_relative_path_candidate(text, start, offset, excluded_spans, references);
            }
        } else if token_start.is_none() {
            token_start = Some(offset);
        }
    }

    if let Some(start) = token_start {
        push_relative_path_candidate(text, start, text.len(), excluded_spans, references);
    }
}

fn push_relative_path_candidate(
    text: &str,
    start: usize,
    end: usize,
    excluded_spans: &[(usize, usize)],
    references: &mut Vec<ExtractedReference>,
) {
    let mut candidate_start = start;
    let mut candidate_end = end;
    let candidate =
        trim_path_punctuation(&text[start..end], &mut candidate_start, &mut candidate_end);

    if candidate.is_empty()
        || excluded_spans.iter().any(|(span_start, span_end)| {
            candidate_start >= *span_start && candidate_end <= *span_end
        })
        || !looks_like_relative_path(candidate)
    {
        return;
    }

    references.push(ExtractedReference {
        target: candidate.to_string(),
        kind: ReferenceKind::RelativePath,
        span: span_at(text, candidate_start, candidate_end),
    });
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn trim_wrapping_quotes<'a>(value: &'a str, start: &mut usize, end: &mut usize) -> &'a str {
    let mut value = value;
    if let Some(stripped) = value.strip_prefix('"').or_else(|| value.strip_prefix('\'')) {
        *start += 1;
        value = stripped;
    }
    if let Some(stripped) = value.strip_suffix('"').or_else(|| value.strip_suffix('\'')) {
        *end -= 1;
        value = stripped;
    }
    value
}

fn trim_path_punctuation<'a>(value: &'a str, start: &mut usize, end: &mut usize) -> &'a str {
    let mut value = value;
    while let Some(first) = value.chars().next() {
        if matches!(first, '"' | '\'' | '`') {
            *start += first.len_utf8();
            value = &value[first.len_utf8()..];
        } else {
            break;
        }
    }

    while let Some(last) = value.chars().next_back() {
        if matches!(last, '"' | '\'' | '`' | '.' | ':' | '!') {
            *end -= last.len_utf8();
            value = &value[..value.len() - last.len_utf8()];
        } else {
            break;
        }
    }

    value
}

fn is_token_delimiter(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '<' | '>' | '[' | ']' | '{' | '}' | '(' | ')' | ',' | ';'
        )
}

fn looks_like_relative_path(candidate: &str) -> bool {
    if candidate.starts_with("http://")
        || candidate.starts_with("https://")
        || candidate.starts_with("data:")
        || candidate.starts_with('#')
        || candidate.starts_with('/')
        || candidate.starts_with('\\')
        || candidate.contains("://")
        || looks_like_windows_absolute_path(candidate)
    {
        return false;
    }

    if candidate.starts_with("./")
        || candidate.starts_with("../")
        || candidate.starts_with(".\\")
        || candidate.starts_with("..\\")
    {
        return true;
    }

    if !candidate.contains('/') && !candidate.contains('\\') {
        return false;
    }

    let leaf = candidate.rsplit(['/', '\\']).next().unwrap_or(candidate);
    leaf.contains('.') && !leaf.ends_with('.')
}

fn looks_like_windows_absolute_path(candidate: &str) -> bool {
    let bytes = candidate.as_bytes();
    bytes.len() >= 3
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
        && bytes[0].is_ascii_alphabetic()
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
    fn manifest_is_text_fallback_not_code() {
        let manifest = manifest();

        assert_eq!(manifest.name, "Text");
        assert_eq!(manifest.viewer.preview_kinds, vec![PreviewKind::Text]);
        assert!(
            !manifest
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
    fn text_diff_marks_reconcile_manual_when_changed() {
        let output = analyze(
            AnalysisInput::new("artifact", b"one\ntwo\n").with_previous_bytes(b"one\nold\n"),
        );

        assert_eq!(
            output.reconcile.status,
            ReconcileStatus::NeedsManualResolution
        );
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
}
