use layrs_store_local::{StoreError, StoreResult};
use rusqlite::{params, Transaction};
use serde_json::Value;

use super::{json_error, optional_string, seconds_to_ms, string, u64_value, Documents};

pub(super) fn load(transaction: &Transaction<'_>) -> StoreResult<Documents> {
    let mut statement =
        transaction.prepare("SELECT path, payload_json FROM client_documents ORDER BY path")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut documents = Documents::new();
    for row in rows {
        let (path, body) = row?;
        let value = serde_json::from_str(&body).map_err(|error| {
            StoreError::CorruptMetadata(format!("document {path} is invalid: {error}"))
        })?;
        documents.insert(path, value);
    }
    Ok(documents)
}

pub(super) fn clear_projection(transaction: &Transaction<'_>, space_id: &str) -> StoreResult<()> {
    transaction.execute("DELETE FROM weave_sessions WHERE space_id = ?1", [space_id])?;
    transaction.execute(
        "DELETE FROM pending_publish WHERE space_id = ?1",
        [space_id],
    )?;
    transaction.execute(
        "DELETE FROM pending_layer_deletions WHERE space_id = ?1",
        [space_id],
    )?;
    transaction.execute("DELETE FROM refs WHERE space_id = ?1", [space_id])?;
    transaction.execute("DELETE FROM gc_roots WHERE space_id = ?1", [space_id])?;
    transaction.execute("DELETE FROM scan_cache WHERE space_id = ?1", [space_id])?;
    transaction.execute("DELETE FROM sync_cursors WHERE space_id = ?1", [space_id])?;
    transaction.execute("DELETE FROM steps WHERE space_id = ?1", [space_id])?;
    transaction.execute("DELETE FROM layers WHERE space_id = ?1", [space_id])?;
    transaction.execute(
        "DELETE FROM object_edges
         WHERE parent_object_id IN (SELECT object_id FROM client_projected_objects)
            OR child_object_id IN (SELECT object_id FROM client_projected_objects)",
        [],
    )?;
    transaction.execute(
        "DELETE FROM object_index
         WHERE object_id IN (SELECT object_id FROM client_projected_objects)",
        [],
    )?;
    Ok(())
}

pub(super) fn insert_space(
    transaction: &Transaction<'_>,
    space: &Value,
    local_space_id: &str,
    now_ms: i64,
) -> StoreResult<()> {
    let root = string(space, "rootPath")?;
    let name = string(space, "name")?;
    let created_at_ms = seconds_to_ms(u64_value(space, "createdAtUnix").unwrap_or_default());
    let updated_at_ms =
        seconds_to_ms(u64_value(space, "updatedAtUnix").unwrap_or_default()).max(created_at_ms);
    let server_space_id = optional_string(space, "serverSpaceId");
    let compatibility_space_id = optional_string(space, "spaceId");
    let identity = server_space_id.as_ref().or(compatibility_space_id.as_ref());
    let payload = serde_json::to_string(space).map_err(json_error)?;

    transaction.execute(
        "INSERT INTO local_spaces (
             space_id, canonical_root, display_name, case_sensitive, identity_token,
             state, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7)
         ON CONFLICT(space_id) DO UPDATE SET
             canonical_root = excluded.canonical_root,
             display_name = excluded.display_name,
             identity_token = excluded.identity_token,
             state = 'active',
             updated_at_ms = excluded.updated_at_ms",
        params![
            local_space_id,
            root,
            name,
            cfg!(not(windows)),
            identity,
            created_at_ms.min(now_ms).max(0),
            updated_at_ms.min(now_ms).max(created_at_ms).max(0)
        ],
    )?;
    transaction.execute(
        "INSERT INTO client_space_details (
             space_id, local_space_id, server_space_id, workspace_id,
             local_state, payload_json
         ) VALUES (?1, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(space_id) DO UPDATE SET
             server_space_id = excluded.server_space_id,
             workspace_id = excluded.workspace_id,
             local_state = excluded.local_state,
             payload_json = excluded.payload_json",
        params![
            local_space_id,
            server_space_id,
            optional_string(space, "workspaceId").unwrap_or_default(),
            optional_string(space, "state").unwrap_or_else(|| "linked".into()),
            payload
        ],
    )?;
    Ok(())
}
