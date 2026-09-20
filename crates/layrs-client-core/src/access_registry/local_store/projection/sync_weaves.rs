use std::collections::{BTreeMap, BTreeSet};

use layrs_store_local::StoreResult;
use rusqlite::{params, Transaction};
use serde_json::Value;

use super::{
    array, json_error, json_field, optional_string, seconds_to_ms, string, u64_to_i64, u64_value,
    usize_to_i64, Documents, ProjectedObject,
};

pub(super) fn insert_sync_cursors(
    transaction: &Transaction<'_>,
    documents: &Documents,
    space_id: &str,
    now_ms: i64,
) -> StoreResult<()> {
    for (path, state) in documents {
        if !path.ends_with("/sync-state.json") {
            continue;
        }
        let Some(cursor) = optional_string(state, "serverCursor") else {
            continue;
        };
        let layer_id = string(state, "layerId")?;
        transaction.execute(
            "INSERT INTO sync_cursors (
                 space_id, remote_id, cursor, generation, updated_at_ms
             ) VALUES (?1, ?2, ?3, 1, ?4)",
            params![space_id, format!("studio:{layer_id}"), cursor, now_ms],
        )?;
    }
    Ok(())
}

pub(super) fn insert_scan_cache(
    transaction: &Transaction<'_>,
    documents: &Documents,
    space_id: &str,
    objects: &BTreeMap<String, ProjectedObject>,
    now_ms: i64,
) -> StoreResult<()> {
    let Some(cache) = documents.get("scan-cache.json") else {
        return Ok(());
    };
    for entry in array(cache, "entries")? {
        let snapshot = entry.get("snapshot").ok_or_else(|| {
            layrs_store_local::StoreError::CorruptMetadata(
                "scan cache entry has no snapshot".into(),
            )
        })?;
        let object_id = optional_string(snapshot, "hash").filter(|id| objects.contains_key(id));
        transaction.execute(
            "INSERT INTO scan_cache (
                 space_id, path_key, file_identity, byte_len, modified_at_ns,
                 mode, object_id, scanned_at_ms
             ) VALUES (?1, ?2, NULL, ?3, ?4, 0, ?5, ?6)",
            params![
                space_id,
                string(entry, "path")?,
                u64_to_i64(u64_value(entry, "size").unwrap_or_default())?,
                u64_to_i64(parse_time_marker(
                    optional_string(entry, "modifiedAt")
                        .as_deref()
                        .unwrap_or("0")
                ))?,
                object_id,
                now_ms
            ],
        )?;
    }
    Ok(())
}

pub(super) fn insert_weaves(
    transaction: &Transaction<'_>,
    documents: &Documents,
    space_id: &str,
    steps: &BTreeMap<String, (String, String, u64)>,
    now_ms: i64,
) -> StoreResult<()> {
    for (path, session) in documents {
        if !path.starts_with("weaves/") || !path.ends_with("/session.json") {
            continue;
        }
        let weave_id = string(session, "weaveId")?;
        let target_layer_id = string(session, "targetLayerId")?;
        let pre_step = optional_string(session, "preWeaveTargetStepId")
            .filter(|step_id| steps.contains_key(step_id));
        let status = weave_status(optional_string(session, "status").as_deref());
        let created = seconds_to_ms(u64_value(session, "createdAtUnix").unwrap_or_default())
            .min(now_ms)
            .max(0);
        let updated = seconds_to_ms(u64_value(session, "updatedAtUnix").unwrap_or_default())
            .min(now_ms)
            .max(created);
        transaction.execute(
            "INSERT INTO weave_sessions (
                 weave_id, space_id, target_layer_id, base_step_id, target_step_id,
                 result_step_id, state, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?4, NULL, ?5, ?6, ?7)",
            params![
                weave_id,
                space_id,
                target_layer_id,
                pre_step,
                status,
                created,
                updated
            ],
        )?;
        let payload = serde_json::to_string(session).map_err(json_error)?;
        let planned = json_field(session, "plannedSteps")?;
        let applied = json_field(session, "appliedSteps")?;
        transaction.execute(
            "INSERT INTO client_weave_details (
                 weave_id, source_layer_id, pre_target_tree_id, pre_target_step_id,
                 planned_steps_json, applied_steps_json, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                weave_id,
                string(session, "sourceLayerId")?,
                optional_string(session, "preWeaveTargetTreeId"),
                optional_string(session, "preWeaveTargetStepId"),
                planned,
                applied,
                payload
            ],
        )?;
        let mut ordinal = 0usize;
        for (role, key) in [("source", "plannedSteps"), ("result", "appliedSteps")] {
            for step in session
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(step_id) = step.as_str() else {
                    continue;
                };
                if steps.contains_key(step_id) {
                    transaction.execute(
                        "INSERT OR IGNORE INTO weave_steps (
                             weave_id, ordinal, step_id, step_role
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![weave_id, usize_to_i64(ordinal)?, step_id, role],
                    )?;
                    ordinal += 1;
                }
            }
        }
        for conflict in session
            .get("conflicts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            insert_weave_conflict(transaction, &weave_id, conflict, created, updated)?;
        }
    }
    Ok(())
}

fn insert_weave_conflict(
    transaction: &Transaction<'_>,
    weave_id: &str,
    conflict: &Value,
    created: i64,
    updated: i64,
) -> StoreResult<()> {
    // Legacy conflict IDs describe content and can recur in separate sessions.
    // SQL primary keys are global; preserve the original ID in payload_json.
    let conflict_id = format!("{weave_id}:{}", string(conflict, "conflictId")?);
    let state = if optional_string(conflict, "status").as_deref() == Some("resolved") {
        "resolved"
    } else {
        "open"
    };
    transaction.execute(
        "INSERT INTO weave_conflicts (
             conflict_id, weave_id, path, conflict_kind, state, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, 'content', ?4, ?5, ?6)",
        params![
            conflict_id,
            weave_id,
            string(conflict, "path")?,
            state,
            created,
            updated
        ],
    )?;
    transaction.execute(
        "INSERT INTO client_weave_conflict_details (
             conflict_id, lens_id, message, methods_json, segments_json, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            conflict_id,
            optional_string(conflict, "lensId").unwrap_or_default(),
            optional_string(conflict, "message").unwrap_or_default(),
            json_field(conflict, "methods")?,
            json_field(conflict, "segments")?,
            serde_json::to_string(conflict).map_err(json_error)?
        ],
    )?;
    for (ordinal, block) in conflict
        .get("blocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let block_id = format!("{conflict_id}:{}", string(block, "blockId")?);
        let digest = blake3::hash(serde_json::to_string(block).map_err(json_error)?.as_bytes())
            .to_hex()
            .to_string();
        transaction.execute(
            "INSERT INTO weave_blocks (
                 block_id, conflict_id, ordinal, side, object_id, content_hash
             ) VALUES (?1, ?2, ?3, 'merged', NULL, ?4)",
            params![block_id, conflict_id, usize_to_i64(ordinal)?, digest],
        )?;
    }
    if let Some(resolution) = optional_string(conflict, "resolution") {
        let kind = match resolution.as_str() {
            "ours" | "target" => "target",
            "theirs" | "source" => "source",
            "delete" => "delete",
            "rename" => "rename",
            _ => "manual",
        };
        transaction.execute(
            "INSERT INTO weave_resolutions (
                 resolution_id, conflict_id, resolution_kind, object_id,
                 payload_json, selected, created_at_ms
             ) VALUES (?1, ?2, ?3, NULL, ?4, 1, ?5)",
            params![
                format!("resolution:{conflict_id}"),
                conflict_id,
                kind,
                serde_json::to_string(&resolution).map_err(json_error)?,
                updated
            ],
        )?;
    }
    Ok(())
}

pub(super) fn insert_gc_roots(
    transaction: &Transaction<'_>,
    documents: &Documents,
    space_id: &str,
    steps: &BTreeMap<String, (String, String, u64)>,
    now_ms: i64,
) -> StoreResult<()> {
    let mut roots = BTreeSet::new();
    for (step_id, (_, root, _)) in steps {
        roots.insert((format!("ref:{step_id}"), root.clone(), "ref"));
    }
    for (path, pending) in documents {
        if path.contains("/pending-publish/") {
            if let Some(root) = optional_string(pending, "rootTreeId") {
                roots.insert((
                    format!("pending:{}", string(pending, "stepId")?),
                    root,
                    "pending",
                ));
            }
        }
    }
    let mut unique = BTreeSet::new();
    for (root_id, object_id, kind) in roots {
        if unique.insert((object_id.clone(), kind)) {
            transaction.execute(
                "INSERT INTO gc_roots (
                     root_id, space_id, object_id, root_kind, expires_at_ms, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                params![root_id, space_id, object_id, kind, now_ms],
            )?;
        }
    }
    Ok(())
}

fn parse_time_marker(value: &str) -> u64 {
    let mut parts = value.splitn(2, '.');
    let seconds: u64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .unwrap_or_default();
    let nanos: u64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .unwrap_or_default();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

fn weave_status(status: Option<&str>) -> &'static str {
    match status {
        Some("prepared") => "prepared",
        Some("conflicted") => "conflicted",
        Some("resolved") => "resolved",
        Some("applying") => "applying",
        Some("completed") | Some("applied") => "completed",
        Some("aborted") => "aborted",
        Some("needs-repair") | Some("needs_repair") => "needs_repair",
        _ => "in_progress",
    }
}
