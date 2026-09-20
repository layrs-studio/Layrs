use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use layrs_store_local::{StoreError, StoreResult};
use rusqlite::{params, Transaction};
use serde_json::Value;

use crate::object_store::{read_file_object_bytes, FileObjectFile};

use super::{
    array, optional_string, string, u64_to_i64, u64_value, usize_to_i64, Documents, ProjectedEdge,
    ProjectedObject,
};

pub(super) fn collect(
    layrs_dir: &Path,
    documents: &Documents,
) -> StoreResult<(BTreeMap<String, ProjectedObject>, Vec<ProjectedEdge>)> {
    let mut objects = BTreeMap::new();
    let mut edges = Vec::new();
    let mut tree_ids = BTreeSet::new();
    let mut snapshots = Vec::new();

    for value in documents.values() {
        collect_snapshot_entries(value, &mut snapshots);
        for key in ["rootTreeId", "baseTreeId", "preWeaveTargetTreeId"] {
            if let Some(tree_id) = optional_string(value, key) {
                tree_ids.insert(tree_id);
            }
        }
    }
    for snapshot in &snapshots {
        if let Some(hash) = optional_string(snapshot, "hash") {
            insert_file_object(layrs_dir, snapshot, &hash, &mut objects, &mut edges)?;
        }
    }
    for tree_id in tree_ids {
        insert_tree_object(layrs_dir, &tree_id, &mut objects, &mut edges)?;
    }
    Ok((objects, edges))
}

fn collect_snapshot_entries(value: &Value, entries: &mut Vec<Value>) {
    if let Some(files) = value.get("files").and_then(Value::as_array) {
        for file in files {
            if file.get("path").is_some() && file.get("object").is_some() {
                entries.push(file.clone());
            }
        }
    }
    if let Some(array) = value.as_array() {
        for item in array {
            collect_snapshot_entries(item, entries);
        }
    } else if let Some(object) = value.as_object() {
        for child in object.values() {
            if child.is_object() || child.is_array() {
                collect_snapshot_entries(child, entries);
            }
        }
    }
}

fn insert_tree_object(
    layrs_dir: &Path,
    tree_id: &str,
    objects: &mut BTreeMap<String, ProjectedObject>,
    edges: &mut Vec<ProjectedEdge>,
) -> StoreResult<()> {
    if objects.contains_key(tree_id) {
        return Ok(());
    }
    let path = layrs_dir
        .join("objects")
        .join("trees")
        .join(format!("{}.json", object_stem(tree_id)));
    let value = read_json_file(&path)?;
    if optional_string(&value, "treeId").as_deref() != Some(tree_id) {
        return Err(StoreError::CorruptMetadata(format!(
            "tree object {} does not identify {tree_id}",
            path.display()
        )));
    }
    let mut tree_material = String::new();
    for file in array(&value, "files")? {
        tree_material.push_str(&string(file, "path")?);
        tree_material.push('\0');
        tree_material.push_str(&string(file, "hash")?);
        tree_material.push('\0');
        tree_material.push_str(&u64_value(file, "size").unwrap_or_default().to_string());
        tree_material.push('\n');
    }
    let computed_tree_id = format!("blake3:{}", blake3::hash(tree_material.as_bytes()).to_hex());
    if computed_tree_id != tree_id {
        return Err(StoreError::CorruptMetadata(format!(
            "tree object {} hash does not close to {tree_id}",
            path.display()
        )));
    }
    let relative = relative_path(layrs_dir, &path)?;
    objects.insert(
        tree_id.to_string(),
        ProjectedObject {
            object_id: tree_id.to_string(),
            algorithm: algorithm(tree_id),
            byte_len: fs::metadata(&path)?.len(),
            kind: "tree",
            relative_path: relative,
            payload: Some(value.clone()),
        },
    );
    for (ordinal, file) in array(&value, "files")?.iter().enumerate() {
        let child = string(file, "hash")?;
        insert_file_object(layrs_dir, file, &child, objects, edges)?;
        edges.push(ProjectedEdge {
            parent: tree_id.to_string(),
            child,
            kind: "entry",
            ordinal,
        });
    }
    Ok(())
}

fn insert_file_object(
    layrs_dir: &Path,
    snapshot: &Value,
    object_id: &str,
    objects: &mut BTreeMap<String, ProjectedObject>,
    edges: &mut Vec<ProjectedEdge>,
) -> StoreResult<()> {
    if objects.contains_key(object_id) {
        return Ok(());
    }
    let relative = string(snapshot, "object")?;
    let relative_object_path = Path::new(&relative);
    if relative_object_path.is_absolute()
        || relative_object_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(StoreError::CorruptMetadata(format!(
            "snapshot object path {relative} is invalid"
        )));
    }
    let path = layrs_dir.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !path.is_file() {
        return Err(StoreError::CorruptMetadata(format!(
            "snapshot object {} is missing",
            path.display()
        )));
    }
    let value = read_json_file(&path)?;
    let manifest: FileObjectFile = serde_json::from_value(value.clone()).map_err(|error| {
        StoreError::CorruptMetadata(format!(
            "file object {} is invalid: {error}",
            path.display()
        ))
    })?;
    if manifest.hash != object_id {
        return Err(StoreError::CorruptMetadata(format!(
            "file object {} identifies {}, expected {object_id}",
            path.display(),
            manifest.hash
        )));
    }
    read_file_object_bytes(layrs_dir, &manifest).map_err(|error| {
        StoreError::CorruptMetadata(format!(
            "file object {} does not close over its chunks: {error}",
            path.display()
        ))
    })?;
    let size = u64_value(&value, "size")
        .or_else(|| u64_value(snapshot, "size"))
        .unwrap_or_else(|| {
            fs::metadata(&path)
                .map(|item| item.len())
                .unwrap_or_default()
        });
    objects.insert(
        object_id.to_string(),
        ProjectedObject {
            object_id: object_id.to_string(),
            algorithm: algorithm(object_id),
            byte_len: size,
            kind: "file",
            relative_path: relative_path(layrs_dir, &path)?,
            payload: Some(value.clone()),
        },
    );
    if let Some(chunks) = value.get("chunks").and_then(Value::as_array) {
        for (ordinal, chunk) in chunks.iter().enumerate() {
            let chunk_id = string(chunk, "chunkId")?;
            // Legacy files and chunks both use the hash of raw content. A
            // one-chunk file therefore already represents this same identity
            // in the deduplicated projection. Its manifest closure was verified
            // above; adding a reflexive edge is neither useful nor valid SQL.
            for (part, storage_id) in insert_chunk_objects(layrs_dir, &chunk_id, objects)?
                .into_iter()
                .enumerate()
            {
                if storage_id != object_id {
                    edges.push(ProjectedEdge {
                        parent: object_id.to_string(),
                        child: storage_id,
                        kind: "chunk",
                        ordinal: ordinal * 2 + part,
                    });
                }
            }
        }
    }
    Ok(())
}

fn insert_chunk_objects(
    layrs_dir: &Path,
    chunk_id: &str,
    objects: &mut BTreeMap<String, ProjectedObject>,
) -> StoreResult<Vec<String>> {
    let paths = crate::object_store::chunk_storage_paths(layrs_dir, chunk_id)
        .map_err(StoreError::CorruptMetadata)?;
    // A pack contains multiple logical chunks: index the physical pack and its
    // index once, with hashes/lengths of their actual bytes. Unique SQL paths
    // cannot model one separate object at the same pack filename per chunk.
    let mut ids = Vec::new();
    for path in paths {
        let relative = relative_path(layrs_dir, &path)?;
        if let Some(existing) = objects.values().find(|o| o.relative_path == relative) {
            ids.push(existing.object_id.clone());
            continue;
        }
        let bytes = fs::read(&path)?;
        let id = format!("blake3:{}", blake3::hash(&bytes).to_hex());
        objects.entry(id.clone()).or_insert(ProjectedObject {
            object_id: id.clone(),
            algorithm: "blake3",
            byte_len: bytes.len() as u64,
            kind: "opaque",
            relative_path: relative,
            payload: None,
        });
        ids.push(id);
    }
    Ok(ids)
}

pub(super) fn insert(
    transaction: &Transaction<'_>,
    objects: &BTreeMap<String, ProjectedObject>,
    edges: &[ProjectedEdge],
    now_ms: i64,
) -> StoreResult<()> {
    for object in objects.values() {
        transaction.execute(
            "INSERT INTO object_index (
                 object_id, algorithm, byte_len, object_kind, relative_path,
                 state, verified_at_ms, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'available', ?6, ?6)",
            params![
                object.object_id,
                object.algorithm,
                u64_to_i64(object.byte_len)?,
                object.kind,
                object.relative_path,
                now_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO client_projected_objects (object_id) VALUES (?1)",
            [&object.object_id],
        )?;
    }
    let mut inserted_edges = BTreeSet::new();
    for edge in edges {
        // Several chunks can share one physical pack. Reachability is unique
        // by parent/child/kind; their logical ordering remains in the manifest.
        if !inserted_edges.insert((&edge.parent, &edge.child, edge.kind)) {
            continue;
        }
        transaction.execute(
            "INSERT INTO object_edges (
                 parent_object_id, child_object_id, edge_kind, ordinal
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                edge.parent,
                edge.child,
                edge.kind,
                usize_to_i64(edge.ordinal)?
            ],
        )?;
    }
    Ok(())
}

fn read_json_file(path: &Path) -> StoreResult<Value> {
    let body = fs::read(path)?;
    serde_json::from_slice(&body).map_err(|error| {
        StoreError::CorruptMetadata(format!(
            "JSON object {} is invalid: {error}",
            path.display()
        ))
    })
}

fn relative_path(root: &Path, path: &Path) -> StoreResult<String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        StoreError::InvalidState(format!(
            "object path {} escapes local store",
            path.display()
        ))
    })?;
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn object_stem(object_id: &str) -> &str {
    object_id.strip_prefix("blake3:").unwrap_or(object_id)
}

fn algorithm(object_id: &str) -> &'static str {
    if object_id
        .strip_prefix("blake3:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        "blake3"
    } else {
        "placeholder"
    }
}
