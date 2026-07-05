use layrs_lens_sdk::{
    AnalysisInput, AnalysisOutput, Analyzer, AnalyzerContract, ArtifactKind, ArtifactMetadata,
    DiffHunk, DiffKind, DiffLine, DiffModel, DiffOp, ExtractedReference, InspectorField,
    InspectorValueType, LensCapability, LensConflictBlock, LensConflictSegment,
    LensConflictSegmentKind, LensError, LensManifest, LensReconcileContent, LensReconcileInput,
    LensReconcileResult, LensResult, MetadataValue, PreviewKind, PreviewModel, ReconcileModel,
    ReconcileStatus, ReferenceKind, ViewerContract, infer_media_type_from_path, span_at,
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

#[derive(Clone)]
struct TextChangeRange {
    base_start: usize,
    base_end: usize,
    replacement: Vec<String>,
}

pub fn reconcile_text(input: LensReconcileInput<'_>) -> LensReconcileResult {
    let Ok(base_text) = side_text(input.base.bytes, input.base.exists) else {
        return LensReconcileResult::unsupported("Base text is not valid UTF-8");
    };
    let Ok(ours_text) = side_text(input.ours.bytes, input.ours.exists) else {
        return LensReconcileResult::unsupported("Target text is not valid UTF-8");
    };
    let Ok(theirs_text) = side_text(input.theirs.bytes, input.theirs.exists) else {
        return LensReconcileResult::unsupported("Source text is not valid UTF-8");
    };

    let base_lines = split_text_lines(base_text);
    let ours_lines = split_text_lines(ours_text);
    let theirs_lines = split_text_lines(theirs_text);
    let ours_changes = text_change_ranges(&base_lines, &ours_lines);
    let theirs_changes = text_change_ranges(&base_lines, &theirs_lines);
    let mut blocks = Vec::new();
    let mut merged = Vec::new();
    let mut segments = Vec::new();
    let mut base_index = 0usize;
    let mut ours_index = 0usize;
    let mut theirs_index = 0usize;

    while ours_index < ours_changes.len() || theirs_index < theirs_changes.len() {
        let next_ours = ours_changes.get(ours_index);
        let next_theirs = theirs_changes.get(theirs_index);
        match (next_ours, next_theirs) {
            (Some(ours_change), Some(theirs_change))
                if ours_change.base_end <= theirs_change.base_start =>
            {
                append_plain_lines_segment(
                    &mut merged,
                    &mut segments,
                    &base_lines,
                    base_index,
                    ours_change.base_start,
                );
                append_plain_text_segment(
                    &mut merged,
                    &mut segments,
                    join_lines(&ours_change.replacement),
                );
                base_index = ours_change.base_end;
                ours_index += 1;
            }
            (Some(ours_change), Some(theirs_change))
                if theirs_change.base_end <= ours_change.base_start =>
            {
                append_plain_lines_segment(
                    &mut merged,
                    &mut segments,
                    &base_lines,
                    base_index,
                    theirs_change.base_start,
                );
                append_plain_text_segment(
                    &mut merged,
                    &mut segments,
                    join_lines(&theirs_change.replacement),
                );
                base_index = theirs_change.base_end;
                theirs_index += 1;
            }
            (Some(_), Some(_)) => {
                let start = next_ours
                    .map(|change| change.base_start)
                    .into_iter()
                    .chain(next_theirs.map(|change| change.base_start))
                    .min()
                    .unwrap_or(base_index);
                let mut end = start;
                let mut ours_group = Vec::new();
                let mut theirs_group = Vec::new();
                loop {
                    let mut advanced = false;
                    while let Some(change) = ours_changes.get(ours_index) {
                        if change.base_start <= end {
                            end = end.max(change.base_end);
                            ours_group.push(change.clone());
                            ours_index += 1;
                            advanced = true;
                        } else {
                            break;
                        }
                    }
                    while let Some(change) = theirs_changes.get(theirs_index) {
                        if change.base_start <= end {
                            end = end.max(change.base_end);
                            theirs_group.push(change.clone());
                            theirs_index += 1;
                            advanced = true;
                        } else {
                            break;
                        }
                    }
                    if !advanced {
                        break;
                    }
                }

                append_plain_lines_segment(
                    &mut merged,
                    &mut segments,
                    &base_lines,
                    base_index,
                    start,
                );
                let base_text = join_lines(&base_lines[start..end]);
                let ours_text = apply_text_changes_to_region(&base_lines, start, end, &ours_group);
                let theirs_text =
                    apply_text_changes_to_region(&base_lines, start, end, &theirs_group);
                if ours_text == theirs_text {
                    append_plain_text_segment(&mut merged, &mut segments, ours_text);
                } else {
                    let block_id = format!("block-{}", blocks.len() + 1);
                    append_conflict_marker(
                        &mut merged,
                        input.ours_label,
                        input.theirs_label,
                        &ours_text,
                        &theirs_text,
                    );
                    segments.push(LensConflictSegment {
                        kind: LensConflictSegmentKind::Block,
                        text: None,
                        block_id: Some(block_id.clone()),
                    });
                    blocks.push(LensConflictBlock {
                        block_id,
                        base: base_text,
                        ours: ours_text,
                        theirs: theirs_text,
                        supported_resolutions: text_block_resolutions(),
                    });
                }
                base_index = end;
            }
            (Some(ours_change), None) => {
                append_plain_lines_segment(
                    &mut merged,
                    &mut segments,
                    &base_lines,
                    base_index,
                    ours_change.base_start,
                );
                append_plain_text_segment(
                    &mut merged,
                    &mut segments,
                    join_lines(&ours_change.replacement),
                );
                base_index = ours_change.base_end;
                ours_index += 1;
            }
            (None, Some(theirs_change)) => {
                append_plain_lines_segment(
                    &mut merged,
                    &mut segments,
                    &base_lines,
                    base_index,
                    theirs_change.base_start,
                );
                append_plain_text_segment(
                    &mut merged,
                    &mut segments,
                    join_lines(&theirs_change.replacement),
                );
                base_index = theirs_change.base_end;
                theirs_index += 1;
            }
            (None, None) => break,
        }
    }

    append_plain_lines_segment(
        &mut merged,
        &mut segments,
        &base_lines,
        base_index,
        base_lines.len(),
    );
    let merged_text = join_lines(&merged);
    if blocks.is_empty() {
        return LensReconcileResult::auto_resolved(
            "Text auto-merged without conflicts",
            LensReconcileContent::present(merged_text.into_bytes()),
        );
    }

    LensReconcileResult::conflicted(
        format!("Text merge has {} unresolved block(s)", blocks.len()),
        LensReconcileContent::present(merged_text.into_bytes()),
        blocks,
        segments,
    )
}

pub fn resolve_text_block_choice(
    base: &str,
    ours: &str,
    theirs: &str,
    resolution: &str,
    manual: Option<&str>,
) -> Result<String, LensError> {
    match resolution {
        "ours" => Ok(ours.to_string()),
        "theirs" => Ok(theirs.to_string()),
        "base" => Ok(base.to_string()),
        "both_ours_then_theirs" => Ok(format!("{}{}", ensure_trailing_newline(ours), theirs)),
        "both_theirs_then_ours" => Ok(format!("{}{}", ensure_trailing_newline(theirs), ours)),
        "manual" => manual.map(ToString::to_string).ok_or_else(|| {
            LensError::new(
                "missing_manual_resolution",
                "Manual text block resolution requires replacement text",
            )
        }),
        other => Err(LensError::new(
            "unsupported_text_resolution",
            format!(
                "Unsupported text block resolution `{other}`. Use ours, theirs, base, both_ours_then_theirs, both_theirs_then_ours, or manual."
            ),
        )),
    }
}

fn side_text(bytes: &[u8], exists: bool) -> Result<&str, std::str::Utf8Error> {
    if exists {
        std::str::from_utf8(bytes)
    } else {
        Ok("")
    }
}

fn text_block_resolutions() -> Vec<String> {
    [
        "ours",
        "theirs",
        "base",
        "both_ours_then_theirs",
        "both_theirs_then_ours",
        "manual",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect()
}

fn split_text_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split_inclusive('\n')
        .map(ToString::to_string)
        .collect()
}

fn append_base_lines(output: &mut Vec<String>, base: &[String], start: usize, end: usize) {
    if start < end {
        output.extend(base[start..end].iter().cloned());
    }
}

fn append_plain_lines_segment(
    output: &mut Vec<String>,
    segments: &mut Vec<LensConflictSegment>,
    base: &[String],
    start: usize,
    end: usize,
) {
    append_plain_text_segment(output, segments, join_lines(&base[start..end]));
}

fn append_plain_text_segment(
    output: &mut Vec<String>,
    segments: &mut Vec<LensConflictSegment>,
    text: String,
) {
    if text.is_empty() {
        return;
    }
    output.push(text.clone());
    segments.push(LensConflictSegment {
        kind: LensConflictSegmentKind::Text,
        text: Some(text),
        block_id: None,
    });
}

fn append_conflict_marker(
    output: &mut Vec<String>,
    ours_label: &str,
    theirs_label: &str,
    ours: &str,
    theirs: &str,
) {
    output.push(format!("<<<<<<< target:{ours_label}\n"));
    output.push(ensure_trailing_newline(ours));
    output.push("=======\n".to_string());
    output.push(ensure_trailing_newline(theirs));
    output.push(format!(">>>>>>> source:{theirs_label}\n"));
}

fn ensure_trailing_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

fn join_lines(lines: &[String]) -> String {
    lines.concat()
}

fn apply_text_changes_to_region(
    base: &[String],
    start: usize,
    end: usize,
    changes: &[TextChangeRange],
) -> String {
    let mut output = Vec::new();
    let mut cursor = start;
    let mut ordered = changes.to_vec();
    ordered.sort_by_key(|change| (change.base_start, change.base_end));
    for change in ordered {
        append_base_lines(&mut output, base, cursor, change.base_start);
        output.extend(change.replacement);
        cursor = change.base_end;
    }
    append_base_lines(&mut output, base, cursor, end);
    join_lines(&output)
}

fn text_change_ranges(base: &[String], variant: &[String]) -> Vec<TextChangeRange> {
    if base == variant {
        return Vec::new();
    }
    let Some(matches) = lcs_line_matches(base, variant) else {
        return vec![TextChangeRange {
            base_start: 0,
            base_end: base.len(),
            replacement: variant.to_vec(),
        }];
    };
    let mut ranges = Vec::new();
    let mut base_cursor = 0usize;
    let mut variant_cursor = 0usize;

    for (base_match, variant_match) in matches
        .into_iter()
        .chain(std::iter::once((base.len(), variant.len())))
    {
        if base_cursor != base_match || variant_cursor != variant_match {
            ranges.push(TextChangeRange {
                base_start: base_cursor,
                base_end: base_match,
                replacement: variant[variant_cursor..variant_match].to_vec(),
            });
        }
        base_cursor = base_match.saturating_add(1);
        variant_cursor = variant_match.saturating_add(1);
    }

    ranges
}

fn lcs_line_matches(base: &[String], variant: &[String]) -> Option<Vec<(usize, usize)>> {
    const MAX_LCS_CELLS: usize = 4_000_000;
    let cells = base.len().saturating_add(1) * variant.len().saturating_add(1);
    if cells > MAX_LCS_CELLS {
        return None;
    }
    let width = variant.len() + 1;
    let mut table = vec![0usize; (base.len() + 1) * width];
    for base_index in (0..base.len()).rev() {
        for variant_index in (0..variant.len()).rev() {
            let index = base_index * width + variant_index;
            table[index] = if base[base_index] == variant[variant_index] {
                table[(base_index + 1) * width + variant_index + 1] + 1
            } else {
                table[(base_index + 1) * width + variant_index]
                    .max(table[base_index * width + variant_index + 1])
            };
        }
    }

    let mut matches = Vec::new();
    let mut base_index = 0usize;
    let mut variant_index = 0usize;
    while base_index < base.len() && variant_index < variant.len() {
        if base[base_index] == variant[variant_index] {
            matches.push((base_index, variant_index));
            base_index += 1;
            variant_index += 1;
        } else if table[(base_index + 1) * width + variant_index]
            >= table[base_index * width + variant_index + 1]
        {
            base_index += 1;
        } else {
            variant_index += 1;
        }
    }
    Some(matches)
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
    use layrs_lens_sdk::{
        LensReconcileInput, LensReconcileResultStatus, LensReconcileSide, ReconcileStatus,
    };

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
    fn reconcile_reports_independent_text_conflict_blocks() {
        let result = reconcile_case(
            b"a\nx\nb\ny\nc\n",
            b"a\nours-x\nb\nours-y\nc\n",
            b"a\ntheirs-x\nb\ntheirs-y\nc\n",
        );

        assert_eq!(result.status, LensReconcileResultStatus::Conflicted);
        assert_eq!(result.blocks.len(), 2);
        assert_eq!(result.blocks[0].block_id, "block-1");
        assert_eq!(result.blocks[1].block_id, "block-2");
        let marked = String::from_utf8(result.conflict.expect("conflict").bytes).expect("utf8");
        assert_eq!(marked.matches("<<<<<<< target:target").count(), 2);
        assert!(marked.contains("ours-x"));
        assert!(marked.contains("theirs-y"));
    }

    #[test]
    fn reconcile_preserves_crlf_and_trailing_newline() {
        let result = reconcile_case(b"a\r\nb\r\n", b"a\r\nours\r\n", b"a\r\nours\r\n");

        assert_eq!(result.status, LensReconcileResultStatus::AutoResolved);
        assert_eq!(result.resolved.expect("resolved").bytes, b"a\r\nours\r\n");
    }

    #[test]
    fn resolves_text_blocks_with_lens_choices() {
        assert_eq!(
            resolve_text_block_choice(
                "base\n",
                "ours\n",
                "theirs\n",
                "both_ours_then_theirs",
                None
            )
            .expect("resolve"),
            "ours\ntheirs\n"
        );
        assert_eq!(
            resolve_text_block_choice("base\n", "ours\n", "theirs\n", "manual", Some("manual\n"))
                .expect("manual"),
            "manual\n"
        );
    }

    fn reconcile_case(base: &[u8], ours: &[u8], theirs: &[u8]) -> LensReconcileResult {
        reconcile_text(LensReconcileInput {
            path: None,
            media_type: Some("text/plain"),
            base: LensReconcileSide::present(base, None),
            ours: LensReconcileSide::present(ours, None),
            theirs: LensReconcileSide::present(theirs, None),
            ours_label: "target",
            theirs_label: "source",
        })
    }
}
