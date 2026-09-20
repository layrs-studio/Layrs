use std::collections::BTreeMap;
use std::path::Path;

use layrs_store_local::{StoreError, StoreResult};
use rusqlite::Transaction;
use serde_json::Value;

mod documents;
mod layers_steps;
mod objects;
mod sync_weaves;

type Documents = BTreeMap<String, Value>;

#[derive(Clone, Debug)]
struct ProjectedObject {
    object_id: String,
    algorithm: &'static str,
    byte_len: u64,
    kind: &'static str,
    relative_path: String,
    payload: Option<Value>,
}

#[derive(Clone, Debug)]
struct ProjectedEdge {
    parent: String,
    child: String,
    kind: &'static str,
    ordinal: usize,
}

pub(super) fn rebuild(
    transaction: &Transaction<'_>,
    layrs_dir: &Path,
    now_ms: i64,
) -> StoreResult<()> {
    let documents = documents::load(transaction)?;
    let Some(space) = documents.get("local-space.json") else {
        return Ok(());
    };
    let local_space_id = string(space, "localSpaceId")?;
    let active_layer_id = documents
        .get("active-layer.json")
        .and_then(|active| optional_string(active, "layerId"));

    documents::clear_projection(transaction, &local_space_id)?;
    documents::insert_space(transaction, space, &local_space_id, now_ms)?;
    let layer_generations = layers_steps::insert_layers(
        transaction,
        &documents,
        space,
        &local_space_id,
        active_layer_id.as_deref(),
        now_ms,
    )?;

    let (projected_objects, edges) = objects::collect(layrs_dir, &documents)?;
    objects::insert(transaction, &projected_objects, &edges, now_ms)?;
    let step_roots = layers_steps::insert_steps(
        transaction,
        &documents,
        &local_space_id,
        &projected_objects,
        now_ms,
    )?;
    layers_steps::insert_refs(
        transaction,
        &local_space_id,
        active_layer_id.as_deref(),
        &step_roots,
        &layer_generations,
        now_ms,
    )?;
    layers_steps::insert_pending_publish(
        transaction,
        &documents,
        &local_space_id,
        &step_roots,
        &layer_generations,
        now_ms,
    )?;
    sync_weaves::insert_sync_cursors(transaction, &documents, &local_space_id, now_ms)?;
    sync_weaves::insert_scan_cache(
        transaction,
        &documents,
        &local_space_id,
        &projected_objects,
        now_ms,
    )?;
    sync_weaves::insert_weaves(
        transaction,
        &documents,
        &local_space_id,
        &step_roots,
        now_ms,
    )?;
    sync_weaves::insert_gc_roots(
        transaction,
        &documents,
        &local_space_id,
        &step_roots,
        now_ms,
    )?;
    Ok(())
}

fn string(value: &Value, key: &str) -> StoreResult<String> {
    optional_string(value, key).ok_or_else(|| {
        StoreError::CorruptMetadata(format!("metadata field {key} is missing or not a string"))
    })
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn u64_value(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn bool_value(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn array<'a>(value: &'a Value, key: &str) -> StoreResult<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| StoreError::CorruptMetadata(format!("metadata field {key} is not an array")))
}

fn json_field(value: &Value, key: &str) -> StoreResult<String> {
    serde_json::to_string(value.get(key).unwrap_or(&Value::Array(Vec::new()))).map_err(json_error)
}

fn seconds_to_ms(seconds: u64) -> i64 {
    i64::try_from(seconds.saturating_mul(1_000)).unwrap_or(i64::MAX)
}

fn u64_to_i64(value: u64) -> StoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidState("metadata integer exceeds SQLite INTEGER".into()))
}

fn usize_to_i64(value: usize) -> StoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidState("metadata ordinal exceeds SQLite INTEGER".into()))
}

fn json_error(error: serde_json::Error) -> StoreError {
    StoreError::CorruptMetadata(error.to_string())
}
