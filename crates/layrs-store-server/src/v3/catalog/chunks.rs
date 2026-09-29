//! One bounded metadata query per manifest, including repeated chunk occurrences.
use super::*;

pub(super) async fn descriptors(
    pool: &sqlx::PgPool,
    space: &SpaceId,
    manifest: &layrs_core::scm::FileManifestId,
) -> V3StoreResult<Vec<ObjectDescriptor>> {
    // The canonical manifest retains occurrence order and offsets. Capabilities
    // describe unique immutable objects, never one copy per occurrence.
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT c.chunk_id AS object_id,o.object_kind,o.canonical_byte_length,
            o.encoded_byte_length,o.compression,o.verification_state,a.availability_state
        FROM file_manifest_chunks c
        LEFT JOIN canonical_objects o ON o.object_id=c.chunk_id
        LEFT JOIN space_object_availability a ON a.object_id=c.chunk_id AND a.space_id=$1
        WHERE c.file_manifest_id=$2 ORDER BY object_id LIMIT 100001
    "#,
    )
    .bind(space.as_str())
    .bind(manifest.as_str())
    .fetch_all(pool)
    .await
    .map_err(pg_error)?;
    if rows.len() > 100_000 {
        return Err(V3StoreError::LimitExceeded {
            resource: "manifest chunk descriptors",
            limit: 100_000,
        });
    }
    rows.iter()
        .map(|row| {
            if row
                .get::<Option<String>, _>("availability_state")
                .as_deref()
                != Some("available")
                || row
                    .get::<Option<String>, _>("verification_state")
                    .as_deref()
                    != Some("verified")
                || row.get::<Option<String>, _>("object_kind").as_deref() != Some("chunk")
            {
                return Err(V3StoreError::CorruptObject(
                    "missing verified Space Chunk".into(),
                ));
            }
            super::super::plan_store::descriptor_from_row(row)
        })
        .collect()
}
