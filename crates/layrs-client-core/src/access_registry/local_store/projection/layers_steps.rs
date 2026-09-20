use std::collections::{BTreeMap, BTreeSet};

use layrs_store_local::{StoreError, StoreResult};
use rusqlite::{params, Transaction};
use serde_json::Value;

use super::{
    array, bool_value, json_error, optional_string, seconds_to_ms, string, u64_to_i64, u64_value,
    Documents, ProjectedObject,
};

pub(super) fn insert_layers(
    transaction: &Transaction<'_>,
    documents: &Documents,
    space: &Value,
    space_id: &str,
    active_layer_id: Option<&str>,
    now_ms: i64,
) -> StoreResult<BTreeMap<String, u64>> {
    let layers = array(space, "layers")?;
    let created_at_ms = seconds_to_ms(u64_value(space, "createdAtUnix").unwrap_or_default())
        .min(now_ms)
        .max(0);
    let updated_at_ms = seconds_to_ms(u64_value(space, "updatedAtUnix").unwrap_or_default())
        .min(now_ms)
        .max(created_at_ms);
    let mut generations = BTreeMap::new();

    for layer in layers {
        let layer_id = string(layer, "layerId")?;
        let access_path = format!("layers/{}/access.json", safe_fragment(&layer_id));
        let access = documents.get(&access_path);
        let policy_epoch = access
            .and_then(|value| u64_value(value, "policyEpoch"))
            .unwrap_or_default();
        generations.insert(layer_id.clone(), policy_epoch);
        let payload = serde_json::to_string(layer).map_err(json_error)?;
        let access_payload = access
            .map(serde_json::to_string)
            .transpose()
            .map_err(json_error)?;

        transaction.execute(
            "INSERT INTO layers (
                 layer_id, space_id, name, layer_kind, state, base_layer_id,
                 timeline_active, generation, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, 'local', 'active', ?4, ?5, ?6, ?7, ?8)",
            params![
                layer_id,
                space_id,
                string(layer, "displayName")?,
                optional_string(layer, "parentLayerId"),
                active_layer_id == Some(layer_id.as_str()),
                u64_to_i64(policy_epoch)?,
                created_at_ms,
                updated_at_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO client_layer_details (
                 layer_id, parent_layer_id, lineage_status, access_kind,
                 can_open, policy_epoch, access_payload_json, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                layer_id,
                optional_string(layer, "parentLayerId"),
                optional_string(layer, "lineageStatus").unwrap_or_else(|| "linked".into()),
                optional_string(layer, "access").unwrap_or_else(|| "blocked".into()),
                bool_value(layer, "canOpen").unwrap_or(false),
                u64_to_i64(policy_epoch)?,
                access_payload,
                payload
            ],
        )?;
    }
    Ok(generations)
}

pub(super) fn insert_steps(
    transaction: &Transaction<'_>,
    documents: &Documents,
    space_id: &str,
    objects: &BTreeMap<String, ProjectedObject>,
    now_ms: i64,
) -> StoreResult<BTreeMap<String, (String, String, u64)>> {
    let mut by_layer: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for (path, value) in documents {
        if path.starts_with("layers/") && path.contains("/steps/") && path.ends_with(".json") {
            by_layer
                .entry(string(value, "layerId")?)
                .or_default()
                .push(value);
        }
    }
    let mut roots = BTreeMap::new();
    for (layer_id, mut steps) in by_layer {
        steps.sort_by(|left, right| {
            u64_value(left, "timelinePosition")
                .cmp(&u64_value(right, "timelinePosition"))
                .then_with(|| {
                    u64_value(left, "capturedAtUnix").cmp(&u64_value(right, "capturedAtUnix"))
                })
                .then_with(|| {
                    optional_string(left, "stepId").cmp(&optional_string(right, "stepId"))
                })
        });
        let mut used_sequences = BTreeSet::new();
        for (ordinal, step) in steps.into_iter().enumerate() {
            let step_id = string(step, "stepId")?;
            let root_id = string(step, "rootTreeId")?;
            if !objects.contains_key(&root_id) {
                return Err(StoreError::CorruptMetadata(format!(
                    "Step {step_id} references missing root object {root_id}"
                )));
            }
            let sequence = u64_value(step, "timelinePosition")
                .unwrap_or_else(|| u64::try_from(ordinal).unwrap_or(u64::MAX));
            if !used_sequences.insert(sequence) {
                return Err(StoreError::CorruptMetadata(format!(
                    "Layer {layer_id} contains duplicate timeline position {sequence}"
                )));
            }
            let parent = optional_string(step, "parentStepId").filter(|parent_id| {
                documents.iter().any(|(path, candidate)| {
                    path.starts_with("layers/")
                        && path.contains("/steps/")
                        && optional_string(candidate, "stepId").as_deref() == Some(parent_id)
                        && optional_string(candidate, "layerId").as_deref() == Some(&layer_id)
                })
            });
            let captured = u64_value(step, "capturedAtUnix").unwrap_or_default();
            transaction.execute(
                "INSERT INTO steps (
                     step_id, space_id, layer_id, parent_step_id, root_object_id,
                     sequence, created_at_ms, authored_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    step_id,
                    space_id,
                    layer_id,
                    parent,
                    root_id,
                    u64_to_i64(sequence)?,
                    seconds_to_ms(captured).min(now_ms).max(0),
                    captured.to_string()
                ],
            )?;
            let payload = serde_json::to_string(step).map_err(json_error)?;
            let changed_paths = serde_json::to_string(
                step.get("changedPaths")
                    .unwrap_or(&Value::Array(Vec::new())),
            )
            .map_err(json_error)?;
            transaction.execute(
                "INSERT INTO client_step_details (
                     step_id, base_layer_id, base_tree_id, timeline_position,
                     origin_layer_id, origin_layer_name, origin_step_id, step_kind,
                     changed_paths_json, payload_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    step_id,
                    optional_string(step, "baseLayerId"),
                    optional_string(step, "baseTreeId"),
                    u64_value(step, "timelinePosition")
                        .map(u64_to_i64)
                        .transpose()?,
                    optional_string(step, "originLayerId"),
                    optional_string(step, "originLayerName"),
                    optional_string(step, "originStepId"),
                    optional_string(step, "stepKind"),
                    changed_paths,
                    payload
                ],
            )?;
            let tree = objects
                .get(&root_id)
                .and_then(|object| object.payload.as_ref())
                .ok_or_else(|| {
                    StoreError::CorruptMetadata(format!(
                        "Step {step_id} root {root_id} has no projected tree payload"
                    ))
                })?;
            for file in array(tree, "files")? {
                let object_id = string(file, "hash")?;
                if !objects.contains_key(&object_id) {
                    return Err(StoreError::CorruptMetadata(format!(
                        "Step {step_id} path {} references missing object {object_id}",
                        string(file, "path")?
                    )));
                }
                transaction.execute(
                    "INSERT INTO step_paths (
                         step_id, path, object_id, entry_kind, executable
                     ) VALUES (?1, ?2, ?3, 'file', 0)",
                    params![step_id, string(file, "path")?, object_id],
                )?;
            }
            roots.insert(step_id, (layer_id.clone(), root_id, sequence));
        }
    }
    Ok(roots)
}

pub(super) fn insert_refs(
    transaction: &Transaction<'_>,
    space_id: &str,
    active_layer_id: Option<&str>,
    steps: &BTreeMap<String, (String, String, u64)>,
    generations: &BTreeMap<String, u64>,
    now_ms: i64,
) -> StoreResult<()> {
    let mut heads: BTreeMap<&str, (&str, u64)> = BTreeMap::new();
    for (step_id, (layer_id, _, sequence)) in steps {
        let replace = heads
            .get(layer_id.as_str())
            .is_none_or(|(_, current)| sequence >= current);
        if replace {
            heads.insert(layer_id, (step_id, *sequence));
        }
    }
    for (layer_id, (step_id, _)) in &heads {
        let generation = generations.get(*layer_id).copied().unwrap_or_default() + 1;
        transaction.execute(
            "INSERT INTO refs (
                 space_id, ref_name, ref_kind, layer_id, step_id, generation, updated_at_ms
             ) VALUES (?1, ?2, 'layer_head', ?3, ?4, ?5, ?6)",
            params![
                space_id,
                format!("layer/{layer_id}/head"),
                layer_id,
                step_id,
                u64_to_i64(generation)?,
                now_ms
            ],
        )?;
    }
    if let Some(active) = active_layer_id {
        if let Some((step_id, _)) = heads.get(active) {
            transaction.execute(
                "INSERT INTO refs (
                     space_id, ref_name, ref_kind, layer_id, step_id, generation, updated_at_ms
                 ) VALUES (?1, 'timeline/active', 'timeline', ?2, ?3, 1, ?4)",
                params![space_id, active, step_id, now_ms],
            )?;
        }
    }
    Ok(())
}

pub(super) fn insert_pending_publish(
    transaction: &Transaction<'_>,
    documents: &Documents,
    space_id: &str,
    steps: &BTreeMap<String, (String, String, u64)>,
    generations: &BTreeMap<String, u64>,
    now_ms: i64,
) -> StoreResult<()> {
    for (path, pending) in documents {
        if !path.starts_with("layers/")
            || !path.contains("/pending-publish/")
            || !path.ends_with(".json")
        {
            continue;
        }
        let step_id = string(pending, "stepId")?;
        let (layer_id, _, _) = steps.get(&step_id).ok_or_else(|| {
            StoreError::CorruptMetadata(format!(
                "pending publish {path} references missing Step {step_id}"
            ))
        })?;
        let idempotency = blake3::hash(format!("{space_id}\0{layer_id}\0{step_id}").as_bytes())
            .to_hex()
            .to_string();
        let created = seconds_to_ms(u64_value(pending, "createdAtUnix").unwrap_or_default())
            .min(now_ms)
            .max(0);
        transaction.execute(
            "INSERT INTO pending_publish (
                 publish_id, space_id, layer_id, step_id, operation_id,
                 idempotency_key, expected_generation, state, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, 'prepared', ?7, ?7)",
            params![
                format!("pending:{step_id}"),
                space_id,
                layer_id,
                step_id,
                idempotency,
                u64_to_i64(generations.get(layer_id).copied().unwrap_or_default())?,
                created
            ],
        )?;
    }
    Ok(())
}

fn safe_fragment(value: &str) -> String {
    let mut safe = String::new();
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            safe.push(character.to_ascii_lowercase());
        } else if character.is_whitespace() || matches!(character, '/' | '\\' | ':') {
            safe.push('-');
        } else {
            safe.push('_');
        }
    }
    let safe = safe.trim_matches('-');
    if safe.is_empty() {
        "layer".into()
    } else {
        safe.into()
    }
}
