//! Opaque, durable client merge drafts. A draft grants no authority and does not
//! block edits; clients must recompute and compare it before applying a merge.
use super::*;
use layrs_core::scm::canonical_json;
use rusqlite::OptionalExtension;

const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Draft {
    schema_version: String,
    generation: u64,
    projection_digest: String,
    capture_sequence: u64,
    capture_digest: String,
    document: serde_json::Value,
}
impl ProjectionStore {
    pub fn save_rebase_draft(
        &self,
        generation: u64,
        projection_digest: &str,
        sequence: u64,
        document: &[u8],
    ) -> Result<()> {
        if document.len() > MAX_BYTES {
            return Err(invalid("merge draft exceeds8MiB"));
        }
        let value: serde_json::Value = serde_json::from_slice(document).map_err(invalid)?;
        if canonical_json(&value).map_err(invalid)? != document {
            return Err(invalid("merge draft must be canonical JSON"));
        }
        let access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        let current = database::current(&access.connection)?;
        let capture = capture::storage::load_metadata(&access.connection, self, sequence)?;
        if current.generation != generation
            || current.digest != projection_digest
            || capture.projection_generation != generation
            || capture.projection_digest != projection_digest
        {
            return Err(invalid("merge draft capture or projection is stale"));
        }
        let draft = Draft {
            schema_version: "layrs.projection-rebase-draft.v1".into(),
            generation,
            projection_digest: projection_digest.into(),
            capture_sequence: sequence,
            capture_digest: capture.digest,
            document: value,
        };
        let payload = canonical_json(&draft).map_err(invalid)?;
        if payload.len() > MAX_BYTES {
            return Err(invalid("merge draft exceeds8MiB"));
        }
        access.verify()?;
        access.connection.execute("INSERT INTO projection_rebase_previews(singleton,payload) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET payload=excluded.payload", [payload])?;
        access.verify()?;
        Ok(())
    }
    pub fn rebase_draft(&self) -> Result<Option<Vec<u8>>> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_idle(&access.connection)?;
        let result = verify(&access.connection, self)?;
        access.verify()?;
        Ok(result)
    }
}
pub(super) fn verify(
    connection: &rusqlite::Connection,
    store: &ProjectionStore,
) -> Result<Option<Vec<u8>>> {
    let length: Option<i64> = connection
        .query_row(
            "SELECT length(payload) FROM projection_rebase_previews WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(length) = length else {
        return Ok(None);
    };
    if length < 0 || length as u64 > MAX_BYTES as u64 {
        return Err(invalid("stored merge draft exceeds 8 MiB"));
    }
    let payload: Vec<u8> = connection.query_row("SELECT payload FROM projection_rebase_previews WHERE singleton=1 AND length(payload)<=8388608", [], |row| row.get(0))?;
    let draft: Draft = serde_json::from_slice(&payload).map_err(invalid)?;
    let capture = capture::storage::load_metadata(connection, store, draft.capture_sequence)?;
    if draft.schema_version != "layrs.projection-rebase-draft.v1"
        || canonical_json(&draft).map_err(invalid)? != payload
        || capture.projection_generation != draft.generation
        || capture.projection_digest != draft.projection_digest
        || capture.digest != draft.capture_digest
    {
        return Err(invalid("merge draft historical binding changed"));
    }
    Ok(Some(canonical_json(&draft.document).map_err(invalid)?))
}
