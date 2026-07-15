fn assemble_text_resolution(
    row: &WeaveConflictRow,
    reconcile: &LensReconcileResult,
    payload: &Value,
) -> Result<LensReconcileContent, ApiError> {
    let blocks = reconcile
        .blocks
        .iter()
        .map(|block| {
            let block_payload = block_payload(payload, &row.conflict_id, &block.block_id)
                .ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "Text conflict block {} is not resolved",
                        block.block_id
                    ))
                })?;
            let method = block_payload
                .get("method")
                .and_then(Value::as_str)
                .and_then(ResolutionMethod::from_label)
                .ok_or_else(|| ApiError::bad_request("stored text block resolution is invalid"))?;
            Ok(LensBlockResolutionInput {
                block_id: block.block_id.as_str(),
                base: block.base.as_str(),
                existing: block.ours.as_str(),
                incoming: block.theirs.as_str(),
                method,
                manual_text: None,
                resolved_text: block_payload.get("resolvedText").and_then(Value::as_str),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let segments = reconcile
        .segments
        .iter()
        .map(|segment| LensResolutionSegment {
            kind: segment.kind,
            text: segment.text.as_deref(),
            block_id: segment.block_id.as_deref(),
        })
        .collect::<Vec<_>>();
    layrs_lens_text::resolve_text_conflict(&blocks, &segments)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

fn lens_resolution_methods(lens_id: &str) -> ResolutionMethods {
    crate::lenses::built_in_lens_manifest(lens_id)
        .filter(|manifest| manifest.supports_capability(&layrs_lens_sdk::LensCapability::Reconcile))
        .map(|manifest| manifest.resolution_methods.clone())
        .unwrap_or_else(ResolutionMethods::none)
}

fn resolution_method_labels(methods: &[ResolutionMethod]) -> Vec<String> {
    layrs_lens_sdk::resolution_method_labels(methods)
}

fn ensure_resolution_method_allowed(
    lens_id: &str,
    scope: &str,
    target: &str,
    method: ResolutionMethod,
    methods: &[ResolutionMethod],
) -> Result<(), ApiError> {
    if methods.contains(&method) {
        return Ok(());
    }
    if methods.is_empty() {
        return Err(unsupported_lens_reconcile_error(lens_id, scope, target));
    }
    let available = resolution_method_labels(methods);
    let available = if available.is_empty() {
        "none".to_string()
    } else {
        available.join(", ")
    };
    Err(ApiError::bad_request(format!(
        "Lens {lens_id} does not declare {scope}-level resolution method `{}` for {target}. Available methods: {available}.",
        method.as_str()
    )))
}

fn unsupported_lens_reconcile_error(lens_id: &str, scope: &str, target: &str) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "lens_reconcile_unsupported",
        format!("Lens {lens_id} does not support {scope}-level Weave resolution for {target}"),
    )
}

fn conflict_media_type(
    row: &WeaveConflictRow,
    base: &WeaveConflictSide,
    ours: &WeaveConflictSide,
    theirs: &WeaveConflictSide,
) -> String {
    let stored = ours
        .media_type
        .as_deref()
        .or(theirs.media_type.as_deref())
        .or(base.media_type.as_deref())
        .unwrap_or("application/octet-stream");
    preview_media_type_for_path(&row.logical_path, stored).to_string()
}

fn side_text_json(side: &WeaveConflictSide) -> Value {
    if !side.exists {
        return json!("");
    }
    match std::str::from_utf8(&side.bytes) {
        Ok(text) => json!(text),
        Err(_) => Value::Null,
    }
}

fn normalize_weave_block_id(block_id: &str) -> String {
    let block_id = block_id.trim();
    if block_id.starts_with("block-") {
        block_id.to_string()
    } else {
        format!("block-{block_id}")
    }
}

fn conflict_payload<'a>(payload: &'a Value, conflict_id: &str) -> Option<&'a Value> {
    payload
        .get("conflictResolutions")
        .and_then(|conflicts| conflicts.get(conflict_id))
}

fn block_payload<'a>(payload: &'a Value, conflict_id: &str, block_id: &str) -> Option<&'a Value> {
    conflict_payload(payload, conflict_id)
        .and_then(|conflict| conflict.get("blocks"))
        .and_then(|blocks| blocks.get(block_id))
}

fn set_block_resolution_payload(
    payload: &mut Value,
    conflict_id: &str,
    block_id: &str,
    method: ResolutionMethod,
    resolved_text: &str,
) {
    let block = block_payload_mut(payload, conflict_id, block_id);
    block.insert("status".to_string(), json!("resolved"));
    block.insert("method".to_string(), json!(method.as_str()));
    block.insert("resolvedText".to_string(), json!(resolved_text));
}

fn set_conflict_resolution_payload(
    payload: &mut Value,
    conflict_id: &str,
    resolution: &str,
    resolved_file_object_id: Option<&str>,
) {
    let conflict = conflict_payload_mut(payload, conflict_id);
    conflict.insert("status".to_string(), json!("resolved"));
    conflict.insert("resolution".to_string(), json!(resolution));
    conflict.insert(
        "resolvedFileObjectId".to_string(),
        resolved_file_object_id
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
}

fn conflict_payload_mut<'a>(
    payload: &'a mut Value,
    conflict_id: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let root = ensure_json_object(payload);
    let conflicts = root
        .entry("conflictResolutions".to_string())
        .or_insert_with(|| json!({}));
    let conflicts = ensure_json_object(conflicts);
    let conflict = conflicts
        .entry(conflict_id.to_string())
        .or_insert_with(|| json!({}));
    ensure_json_object(conflict)
}

fn block_payload_mut<'a>(
    payload: &'a mut Value,
    conflict_id: &str,
    block_id: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let conflict = conflict_payload_mut(payload, conflict_id);
    let blocks = conflict
        .entry("blocks".to_string())
        .or_insert_with(|| json!({}));
    let blocks = ensure_json_object(blocks);
    let block = blocks
        .entry(block_id.to_string())
        .or_insert_with(|| json!({}));
    ensure_json_object(block)
}

fn ensure_json_object(value: &mut Value) -> &mut serde_json::Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("JSON value is an object")
}

fn text_resolution_has_manual(row: &WeaveConflictRow, payload: &Value) -> bool {
    conflict_payload(payload, &row.conflict_id)
        .and_then(|conflict| conflict.get("blocks"))
        .and_then(Value::as_object)
        .map(|blocks| {
            blocks
                .values()
                .any(|block| block.get("method").and_then(Value::as_str) == Some("manual"))
        })
        .unwrap_or(false)
}

fn resolution_kind_for_method(method: ResolutionMethod) -> &'static str {
    match method {
        ResolutionMethod::Existing => "ours",
        ResolutionMethod::Incoming => "theirs",
        ResolutionMethod::Both => "file",
        ResolutionMethod::Manual => "manual",
    }
}

#[cfg(test)]
#[path = "weave_conflict_resolution_tests.rs"]
mod canonical_lens_host_tests;
