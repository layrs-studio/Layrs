async fn artifact_content_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    account_id: &str,
) -> Result<Value, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT artifact_id, logical_path, artifact_kind, state, current_file_object_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND artifact_id = $4 AND state <> 'deleted'
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("artifact not found"))?;
    let path = row.get::<String, _>("logical_path");
    let state = row.get::<String, _>("state");
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        &path,
        Some(artifact_id),
        account_id,
    )
    .await?;
    if state == "redacted" || !decision.can_read {
        return Err(ApiError::forbidden(
            "artifact content is redacted by layer access policy",
        ));
    }
    if let Ok(file_object_id) = row.try_get::<String, _>("current_file_object_id") {
        return artifact_v2_content_value(
            pool,
            workspace_id,
            space_id,
            layer_id,
            artifact_id,
            &path,
            row.get::<String, _>("artifact_kind").as_str(),
            &file_object_id,
        )
        .await;
    }

    let content_row = sqlx::query(
        r#"
        SELECT event_id, body_json,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM timeline_events
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = $3
          AND event_kind = 'artifact.published'
          AND body_json->>'artifactId' = $4
          AND body_json ? 'content'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("artifact content is not available"))?;
    let body = content_row.get::<Value, _>("body_json");

    Ok(json!({
        "artifactId": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "path": path,
        "type": artifact_type(row.get::<String, _>("artifact_kind").as_str()),
        "content": {
            "encoding": "json",
            "mediaType": body.get("mediaType").cloned().unwrap_or_else(|| json!("application/json")),
            "sha256": body.get("contentHash").cloned().unwrap_or(Value::Null),
            "value": body.get("content").cloned().unwrap_or(Value::Null)
        },
        "source": {
            "kind": "timeline_event",
            "eventId": content_row.get::<String, _>("event_id"),
            "createdAt": content_row.get::<String, _>("created_at")
        }
    }))
}

async fn artifact_v2_content_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    path: &str,
    artifact_kind: &str,
    file_object_id: &str,
) -> Result<Value, ApiError> {
    let file_row = sqlx::query(
        r#"
        SELECT f.file_object_id, f.digest, f.size_bytes, f.media_type
        FROM file_objects f
        JOIN space_file_objects sfo ON sfo.file_object_id = f.file_object_id
        WHERE sfo.workspace_id = $1
          AND sfo.space_id = $2
          AND f.file_object_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("file object not found"))?;
    let bytes = file_object_bytes(pool, workspace_id, space_id, file_object_id).await?;
    let chunks = chunk_values_for_file_object(pool, workspace_id, space_id, file_object_id).await?;
    let stored_media_type = file_row
        .try_get::<String, _>("media_type")
        .ok()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let media_type = preview_media_type_for_path(path, &stored_media_type);

    Ok(json!({
        "artifactId": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "path": path,
        "type": artifact_type(artifact_kind),
        "content": {
            "encoding": "base64",
            "mediaType": media_type,
            "digest": file_row.get::<String, _>("digest"),
            "value": BASE64.encode(bytes)
        },
        "fileObject": {
            "fileObjectId": file_row.get::<String, _>("file_object_id"),
            "sizeBytes": file_row.get::<i64, _>("size_bytes"),
            "chunks": chunks
        },
        "source": {
            "kind": "file_object",
            "storage": "file_object_chunks_v2"
        }
    }))
}

fn artifact_window_limit(limit: Option<usize>) -> Result<usize, ApiError> {
    let limit = limit.unwrap_or(DEFAULT_ARTIFACT_DIFF_WINDOW_LIMIT);
    if limit == 0 {
        return Err(ApiError::bad_request("limit must be greater than zero"));
    }
    Ok(limit.min(MAX_ARTIFACT_DIFF_WINDOW_LIMIT))
}

fn artifact_column_limit(limit: Option<usize>) -> Result<Option<usize>, ApiError> {
    match limit {
        None => Ok(None),
        Some(0) => Err(ApiError::bad_request(
            "columnLimit must be greater than zero",
        )),
        Some(limit) => Ok(Some(limit.min(MAX_DIFF_COLUMN_WINDOW_LIMIT))),
    }
}

#[derive(Clone, Copy, Debug)]
struct WindowRequest {
    start: usize,
    limit: usize,
    column_start: usize,
    column_limit: Option<usize>,
}

struct LensRuntimeDiffRender<'a> {
    workspace_id: &'a str,
    space_id: &'a str,
    layer_id: &'a str,
    artifact_id: Option<&'a str>,
    step_id: Option<&'a str>,
    base_layer_id: Option<&'a str>,
    path: &'a str,
    target: Option<&'a ArtifactTextWindow>,
    base: Option<&'a ArtifactTextWindow>,
    window_request: WindowRequest,
    summary: &'a str,
    mode: &'a str,
    limitation: Option<&'a str>,
}

fn lens_runtime_diff_value(input: LensRuntimeDiffRender<'_>) -> Value {
    let preview_window = input
        .target
        .or(input.base)
        .expect("lens runtime needs a target or base text window");
    let has_more = window_has_more(
        input.window_request.start,
        preview_window.lines.len(),
        preview_window.total_lines,
    );
    let has_long_lines =
        text_window_has_long_columns(input.target) || text_window_has_long_columns(input.base);
    let window_value = json!({
        "start": input.window_request.start,
        "limit": input.window_request.limit,
        "count": preview_window.lines.len(),
        "totalLines": preview_window.total_lines,
        "hasMore": has_more,
        "hasMoreBefore": input.window_request.start > 0,
        "hasMoreAfter": has_more
    });
    let column_window_value = json!({
        "columnStart": input.window_request.column_start,
        "columnLimit": input.window_request.column_limit,
        "hasLongLines": has_long_lines
    });
    let compare_missing_base_as_insert = input.step_id.is_some();
    let diff_lines = lens_runtime_diff_lines(
        input.target,
        input.base,
        input.window_request.start,
        compare_missing_base_as_insert,
    );
    let old_line_count = input
        .base
        .map(|window| window.lines.len())
        .unwrap_or_else(|| {
            if compare_missing_base_as_insert {
                0
            } else {
                preview_window.lines.len()
            }
        });
    let new_line_count = input
        .target
        .map(|window| window.lines.len())
        .unwrap_or_default();
    let new_line_count = if input.target.is_some() {
        new_line_count
    } else {
        0
    };
    let runtime_value = json!({
        "id": "layrs.server.lens-runtime.text",
        "lensId": lens_id_for_path_and_media_type(input.path, &preview_window.media_type)
    });
    let source_value = json!({
        "kind": "lens_runtime",
        "runtime": runtime_value,
        "target": input.target.map(|window| window.source.clone()),
        "base": input.base.map(|window| window.source.clone())
    });

    json!({
        "artifactId": input.artifact_id,
        "stepId": input.step_id,
        "workspaceId": input.workspace_id,
        "spaceId": input.space_id,
        "layerId": input.layer_id,
        "baseLayerId": input.base_layer_id,
        "path": input.path,
        "type": artifact_type(&preview_window.artifact_kind),
        "window": window_value,
        "preview": {
            "kind": preview_kind_for_path(input.path, &preview_window.media_type),
            "title": input.path,
            "body": preview_window.lines.iter().map(|line| line.text_segment.as_str()).collect::<Vec<_>>().join("\n"),
            "mediaType": preview_window.media_type,
            "fields": {
                "lines": preview_window.lines.iter().map(text_window_line_value).collect::<Vec<_>>(),
                "window": window_value,
                "columnWindow": column_window_value,
                "windowed": true,
                "contentHash": preview_window.content_hash,
                "baseLayerId": input.base_layer_id,
                "exactBaseDiff": false,
                "runtime": runtime_value,
                "source": source_value
            }
        },
        "diff": {
            "kind": "textLines",
            "summary": input.summary,
            "hunks": [{
                "oldStart": input.window_request.start + 1,
                "oldLines": old_line_count,
                "newStart": input.window_request.start + 1,
                "newLines": new_line_count,
                "lines": diff_lines
            }],
            "fields": {
                "mode": input.mode,
                "window": window_value,
                "columnWindow": column_window_value,
                "windowed": true,
                "totalLines": preview_window.total_lines,
                "oldTotalLines": input.base.map(|window| window.total_lines),
                "newTotalLines": input.target.map(|window| window.total_lines),
                "renderedLineCount": preview_window.lines.len(),
                "hasMore": has_more,
                "hasMoreBefore": input.window_request.start > 0,
                "hasMoreAfter": has_more,
                "hasLongLines": has_long_lines,
                "contentHash": preview_window.content_hash,
                "baseLayerId": input.base_layer_id,
                "exactBaseDiff": false,
                "limitation": input.limitation,
                "runtime": runtime_value
            }
        },
        "source": source_value
    })
}

async fn artifact_diff_window_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    account_id: &str,
    window_request: WindowRequest,
    base_layer_id: Option<&str>,
) -> Result<Value, ApiError> {
    let window = artifact_text_window(
        pool,
        workspace_id,
        space_id,
        layer_id,
        artifact_id,
        account_id,
        window_request,
    )
    .await?;
    let base_requested = base_layer_id.is_some();
    let summary = if base_requested {
        "Windowed artifact preview; exact base diff is not available in this server version"
    } else {
        "Windowed artifact preview"
    };

    Ok(lens_runtime_diff_value(LensRuntimeDiffRender {
        workspace_id,
        space_id,
        layer_id,
        artifact_id: Some(artifact_id),
        step_id: None,
        base_layer_id,
        path: &window.path,
        target: Some(&window),
        base: None,
        window_request,
        summary,
        mode: "preview",
        limitation: Some(
            "V1 returns a server-windowed artifact preview. Exact base comparison is not computed yet.",
        ),
    }))
}

async fn artifact_text_window(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    account_id: &str,
    window_request: WindowRequest,
) -> Result<ArtifactTextWindow, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT artifact_id, logical_path, artifact_kind, state, current_file_object_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND artifact_id = $4 AND state <> 'deleted'
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("artifact not found"))?;
    let path = row.get::<String, _>("logical_path");
    let state = row.get::<String, _>("state");
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        &path,
        Some(artifact_id),
        account_id,
    )
    .await?;
    if state == "redacted" || !decision.can_read {
        return Err(ApiError::forbidden(
            "artifact content is redacted by layer access policy",
        ));
    }
    let artifact_kind = row.get::<String, _>("artifact_kind");
    if let Ok(file_object_id) = row.try_get::<String, _>("current_file_object_id") {
        return file_object_text_window(
            pool,
            workspace_id,
            space_id,
            &path,
            artifact_kind,
            &file_object_id,
            window_request,
        )
        .await;
    }

    Err(ApiError::not_found(
        "artifact content is not available in the chunked store",
    ))
}

async fn file_object_text_window(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    path: &str,
    artifact_kind: String,
    file_object_id: &str,
    window_request: WindowRequest,
) -> Result<ArtifactTextWindow, ApiError> {
    let file_row = sqlx::query(
        r#"
        SELECT f.file_object_id, f.digest, f.size_bytes, f.media_type
        FROM file_objects f
        JOIN space_file_objects sfo ON sfo.file_object_id = f.file_object_id
        WHERE sfo.workspace_id = $1
          AND sfo.space_id = $2
          AND f.file_object_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("file object not found"))?;
    let stored_media_type = file_row
        .try_get::<String, _>("media_type")
        .ok()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if !is_textual_artifact(path, &stored_media_type) {
        return Err(ApiError::bad_request(
            "windowed diff preview is available for text artifacts only",
        ));
    }
    let media_type = preview_media_type_for_path(path, &stored_media_type).to_string();

    let chunk_rows = sqlx::query(
        r#"
        SELECT oc.content_bytes, oc.compression
        FROM file_object_chunks foc
        JOIN object_chunks oc ON oc.chunk_id = foc.chunk_id
        JOIN space_object_chunks soc ON soc.chunk_id = oc.chunk_id
        WHERE foc.file_object_id = $1
          AND soc.workspace_id = $2
          AND soc.space_id = $3
          AND oc.state = 'available'
        ORDER BY foc.chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?;
    let mut builder = TextLineWindowBuilder::new(window_request);
    for row in chunk_rows {
        let bytes = row
            .try_get::<Vec<u8>, _>("content_bytes")
            .ok()
            .ok_or_else(|| ApiError::not_found("chunk bytes are not available"))?;
        let compression = row
            .try_get::<String, _>("compression")
            .ok()
            .unwrap_or_else(|| CHUNK_COMPRESSION_IDENTITY.to_string());
        let raw = decode_chunk_bytes(&bytes, &compression)?;
        builder.push_lossy_utf8(&raw);
    }
    let text_window = builder.finish();

    Ok(ArtifactTextWindow {
        lines: text_window.lines,
        total_lines: text_window.total_lines,
        path: path.to_string(),
        artifact_kind,
        media_type,
        content_hash: Some(file_row.get::<String, _>("digest")),
        source: json!({
            "kind": "file_object",
            "storage": "file_object_chunks_v2",
            "fileObjectId": file_row.get::<String, _>("file_object_id"),
            "sizeBytes": file_row.get::<i64, _>("size_bytes")
        }),
    })
}

#[derive(Debug)]
struct ArtifactTextWindow {
    lines: Vec<TextWindowLine>,
    total_lines: usize,
    path: String,
    artifact_kind: String,
    media_type: String,
    content_hash: Option<String>,
    source: Value,
}

#[derive(Debug)]
struct TextLineWindow {
    lines: Vec<TextWindowLine>,
    total_lines: usize,
}

#[derive(Debug)]
struct TextWindowLine {
    text_segment: String,
    text_length: usize,
    column_start: usize,
    column_end: usize,
    has_more_columns: bool,
}

struct TextLineWindowBuilder {
    start: usize,
    limit: usize,
    column_start: usize,
    column_limit: Option<usize>,
    lines: Vec<TextWindowLine>,
    total_lines: usize,
    current_line_segment: String,
    current_line_segment_len: usize,
    current_line_len: usize,
    saw_content: bool,
}

impl TextLineWindowBuilder {
    fn new(request: WindowRequest) -> Self {
        Self {
            start: request.start,
            limit: request.limit,
            column_start: request.column_start,
            column_limit: request.column_limit,
            lines: Vec::new(),
            total_lines: 0,
            current_line_segment: String::new(),
            current_line_segment_len: 0,
            current_line_len: 0,
            saw_content: false,
        }
    }

    fn push_lossy_utf8(&mut self, bytes: &[u8]) {
        self.push_text(&String::from_utf8_lossy(bytes));
    }

    fn push_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.saw_content = true;
        }
        for character in text.chars() {
            if character == '\n' {
                self.flush_line();
            } else {
                if character != '\r' {
                    if self.total_lines >= self.start
                        && self.lines.len() < self.limit
                        && self.current_line_len >= self.column_start
                        && self
                            .column_limit
                            .map_or(true, |limit| self.current_line_segment_len < limit)
                    {
                        self.current_line_segment.push(character);
                        self.current_line_segment_len += 1;
                    }
                    self.current_line_len += 1;
                }
            }
        }
    }

    fn finish(mut self) -> TextLineWindow {
        if self.saw_content && (self.current_line_len > 0 || self.total_lines == 0) {
            self.flush_line();
        }
        TextLineWindow {
            lines: self.lines,
            total_lines: self.total_lines,
        }
    }

    fn flush_line(&mut self) {
        if self.total_lines >= self.start && self.lines.len() < self.limit {
            let segment_len = self.current_line_segment_len;
            let column_start = self.column_start.min(self.current_line_len);
            let column_end = column_start
                .saturating_add(segment_len)
                .min(self.current_line_len);
            self.lines.push(TextWindowLine {
                text_segment: self.current_line_segment.clone(),
                text_length: self.current_line_len,
                column_start,
                column_end,
                has_more_columns: column_end < self.current_line_len,
            });
        }
        self.total_lines += 1;
        self.current_line_segment.clear();
        self.current_line_segment_len = 0;
        self.current_line_len = 0;
    }
}

#[cfg(test)]
fn text_window_from_str(text: &str, start: usize, limit: usize) -> TextLineWindow {
    let mut builder = TextLineWindowBuilder::new(WindowRequest {
        start,
        limit,
        column_start: 0,
        column_limit: None,
    });
    builder.push_text(text);
    builder.finish()
}

fn window_has_more(start: usize, count: usize, total_lines: usize) -> bool {
    start.saturating_add(count) < total_lines
}

fn text_window_has_long_columns(window: Option<&ArtifactTextWindow>) -> bool {
    window.is_some_and(|window| {
        window
            .lines
            .iter()
            .any(|line| line.has_more_columns || line.column_start > 0)
    })
}

fn lens_runtime_diff_lines(
    target: Option<&ArtifactTextWindow>,
    base: Option<&ArtifactTextWindow>,
    start: usize,
    compare_missing_base_as_insert: bool,
) -> Vec<Value> {
    match base {
        None => target
            .into_iter()
            .flat_map(|window| {
                window.lines.iter().enumerate().map(move |(index, line)| {
                    let line_number = start + index + 1;
                    if compare_missing_base_as_insert {
                        lens_diff_line_value("insert", None, Some(line_number), line)
                    } else {
                        lens_diff_line_value("equal", Some(line_number), Some(line_number), line)
                    }
                })
            })
            .collect(),
        Some(base_window) => {
            let target_lines = target
                .map(|window| window.lines.as_slice())
                .unwrap_or_default();
            let max_count = base_window.lines.len().max(target_lines.len());
            let mut lines = Vec::new();
            for index in 0..max_count {
                let old_line_number = start + index + 1;
                let new_line_number = start + index + 1;
                match (base_window.lines.get(index), target_lines.get(index)) {
                    (Some(old_line), Some(new_line))
                        if text_window_lines_match(old_line, new_line) =>
                    {
                        lines.push(lens_diff_line_value(
                            "equal",
                            Some(old_line_number),
                            Some(new_line_number),
                            new_line,
                        ));
                    }
                    (Some(old_line), Some(new_line)) => {
                        lines.push(lens_diff_line_value(
                            "delete",
                            Some(old_line_number),
                            None,
                            old_line,
                        ));
                        lines.push(lens_diff_line_value(
                            "insert",
                            None,
                            Some(new_line_number),
                            new_line,
                        ));
                    }
                    (Some(old_line), None) => {
                        lines.push(lens_diff_line_value(
                            "delete",
                            Some(old_line_number),
                            None,
                            old_line,
                        ));
                    }
                    (None, Some(new_line)) => {
                        lines.push(lens_diff_line_value(
                            "insert",
                            None,
                            Some(new_line_number),
                            new_line,
                        ));
                    }
                    (None, None) => {}
                }
            }
            lines
        }
    }
}

fn text_window_lines_match(left: &TextWindowLine, right: &TextWindowLine) -> bool {
    left.text_segment == right.text_segment
        && left.text_length == right.text_length
        && left.column_start == right.column_start
        && left.column_end == right.column_end
        && left.has_more_columns == right.has_more_columns
}

fn text_window_line_value(line: &TextWindowLine) -> Value {
    json!({
        "textSegment": line.text_segment,
        "text": line.text_segment,
        "textLength": line.text_length,
        "columnStart": line.column_start,
        "columnEnd": line.column_end,
        "hasMoreColumns": line.has_more_columns
    })
}

fn lens_diff_line_value(
    op: &str,
    old_line: Option<usize>,
    new_line: Option<usize>,
    line: &TextWindowLine,
) -> Value {
    json!({
        "op": op,
        "oldLine": old_line,
        "newLine": new_line,
        "text": line.text_segment,
        "textSegment": line.text_segment,
        "textLength": line.text_length,
        "columnStart": line.column_start,
        "columnEnd": line.column_end,
        "hasMoreColumns": line.has_more_columns
    })
}

fn preview_kind_for_path(path: &str, media_type: &str) -> &'static str {
    if is_code_path(path) {
        return "code";
    }
    if is_text_path(path) || media_type == "text/markdown" || media_type.starts_with("text/") {
        return "text";
    }
    "raw"
}

fn preview_media_type_for_path<'a>(path: &str, stored_media_type: &'a str) -> &'a str {
    if stored_media_type != "application/octet-stream" {
        return stored_media_type;
    }

    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".mdx") || lower.ends_with(".markdown") {
        return "text/markdown";
    }
    if is_code_path(path) || is_text_path(path) {
        return "text/plain";
    }
    stored_media_type
}

fn is_textual_artifact(path: &str, media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json"
                | "application/javascript"
                | "application/typescript"
                | "application/xml"
                | "application/x-yaml"
                | "application/yaml"
        )
        || is_code_path(path)
        || is_text_path(path)
}

fn is_code_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".css", ".html", ".js", ".json", ".jsx", ".mjs", ".cjs", ".rs", ".toml", ".ts", ".tsx",
        ".xml", ".yaml", ".yml", ".py", ".go", ".java", ".kt", ".kts", ".swift", ".c", ".h", ".cc",
        ".cpp", ".cxx", ".hpp", ".cs", ".php", ".rb", ".sh", ".bash", ".zsh", ".ps1", ".sql",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension))
}

fn is_text_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".txt", ".md", ".mdx", ".markdown", ".rst", ".log"]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

