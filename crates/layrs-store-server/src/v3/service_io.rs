//! Shared bounded reads for the native service and policy evaluator.
use super::{CatalogedCas, PrincipalContext, V3StoreError, V3StoreResult, pg_error};
use futures_util::StreamExt;
use layrs_core::scm::{ActorId, ObjectId, SpaceId};
use layrs_sync::v3::{Compression, ObjectDescriptor, PrincipalId};
use sqlx::{PgPool, Row};
use std::io::Read;

pub async fn load_principal_context(
    pool: &PgPool,
    id: &PrincipalId,
) -> V3StoreResult<PrincipalContext> {
    let row = sqlx::query("SELECT actor_id, deployment_id, subject_digest FROM principals WHERE principal_id=$1 AND status='active'")
        .bind(id.as_str()).fetch_optional(pool).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
    PrincipalContext::new(
        id.clone(),
        ActorId::parse(row.get::<String, _>("actor_id"))
            .map_err(|_| V3StoreError::InvalidCapability)?,
        row.get::<String, _>("deployment_id"),
        row.get::<String, _>("subject_digest"),
    )
}

pub async fn load_space_descriptor(
    pool: &PgPool,
    space: &SpaceId,
    id: &ObjectId,
) -> V3StoreResult<ObjectDescriptor> {
    let row = sqlx::query("SELECT o.object_id, o.object_kind, o.canonical_byte_length, o.encoded_byte_length, o.compression FROM canonical_objects o JOIN space_object_availability a ON a.object_id=o.object_id WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available' AND o.verification_state='verified'")
        .bind(space.as_str()).bind(id.as_str()).fetch_optional(pool).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
    super::plan_store::descriptor_from_row(&row)
}

/// Collects only explicitly bounded metadata/content. Large transfers use CAS streams.
pub async fn read_canonical_object(
    cas: &dyn CatalogedCas,
    descriptor: &ObjectDescriptor,
    limit: u64,
) -> V3StoreResult<Vec<u8>> {
    if descriptor.canonical_byte_len > limit || descriptor.encoded_byte_len > limit {
        return Err(V3StoreError::LimitExceeded {
            resource: "service object",
            limit,
        });
    }
    let mut stream = cas.verified_stream(descriptor).await?;
    let mut encoded = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if (encoded.len() as u64).saturating_add(chunk.len() as u64) > descriptor.encoded_byte_len {
            return Err(V3StoreError::CorruptObject(
                "encoded length overflow".into(),
            ));
        }
        encoded.extend_from_slice(&chunk);
    }
    if encoded.len() as u64 != descriptor.encoded_byte_len {
        return Err(V3StoreError::CorruptObject(
            "incomplete encoded object".into(),
        ));
    }
    let bytes = match descriptor.compression {
        Compression::Identity => encoded,
        Compression::Zstd => {
            let mut result = Vec::new();
            zstd::stream::read::Decoder::new(encoded.as_slice())?
                .take(limit.saturating_add(1))
                .read_to_end(&mut result)?;
            result
        }
    };
    if bytes.len() as u64 != descriptor.canonical_byte_len {
        return Err(V3StoreError::CorruptObject(
            "canonical length mismatch".into(),
        ));
    }
    Ok(bytes)
}
