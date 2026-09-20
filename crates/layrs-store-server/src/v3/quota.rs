//! Conservative logical CAS quotas. Reservations survive plan expiry until a
//! future verified GC can prove their possible durable CAS orphans are gone.
use super::{PgV3Repository, V3StoreError, V3StoreResult, pg_error};
use layrs_core::scm::SpaceId;
use layrs_sync::v3::ObjectDescriptor;
use serde::Serialize;
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CasQuotas {
    pub max_total_bytes: Option<u64>,
    pub max_total_objects: Option<u64>,
    pub max_space_bytes: Option<u64>,
    pub max_space_objects: Option<u64>,
}

impl CasQuotas {
    pub fn validate(self) -> V3StoreResult<Self> {
        for limit in [
            self.max_total_bytes,
            self.max_total_objects,
            self.max_space_bytes,
            self.max_space_objects,
        ]
        .into_iter()
        .flatten()
        {
            if limit == 0 || limit > i64::MAX as u64 {
                return Err(V3StoreError::InvalidDescriptor(
                    "CAS quota must be in 1..=i64::MAX".into(),
                ));
            }
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CasBudgetUsage {
    pub objects: u64,
    /// Sum of max(canonical length, encoded length), once per ObjectId.
    pub charged_bytes: u64,
    pub catalogued_bytes: u64,
    pub reserved_only_bytes: u64,
}

/// Database credentials are the authority for this maintenance-only read.
/// Do not expose it through an unauthenticated or ordinary-user HTTP endpoint.
pub async fn read_cas_budget(
    pool: &PgPool,
    space: Option<&SpaceId>,
) -> V3StoreResult<CasBudgetUsage> {
    let mut tx = pool.begin().await.map_err(pg_error)?;
    let result = usage(&mut tx, space, &[]).await?;
    tx.commit().await.map_err(pg_error)?;
    Ok(result)
}

/// Caller-owned transaction variant for authenticated administrator reads.
pub async fn read_cas_budget_in(
    tx: &mut Transaction<'_, Postgres>,
    space: Option<&SpaceId>,
) -> V3StoreResult<CasBudgetUsage> {
    usage(tx, space, &[]).await
}

impl PgV3Repository {
    /// Must precede identity, Space and catalogue row locks in writer paths.
    /// A transaction-scoped advisory lock works across processes and releases
    /// automatically on crash/rollback. All serving replicas need equal limits.
    /// Writers acquire the repository's local gate before opening a SQL
    /// transaction, so waiters cannot exhaust the pool needed by authorization.
    pub(crate) async fn lock_cas_budget(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> V3StoreResult<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(1818327410, 1903521652)")
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;
        Ok(())
    }

    pub(crate) async fn check_cas_budget(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: Option<&SpaceId>,
        additions: &[ObjectDescriptor],
    ) -> V3StoreResult<()> {
        let (bytes, objects) = if space.is_some() {
            (
                self.quotas().max_space_bytes,
                self.quotas().max_space_objects,
            )
        } else {
            (
                self.quotas().max_total_bytes,
                self.quotas().max_total_objects,
            )
        };
        if bytes.is_none() && objects.is_none() {
            return Ok(());
        }
        self.lock_cas_budget(tx).await?;
        let before = usage(tx, space, &[]).await?;
        let after = usage(tx, space, additions).await?;
        for (limit, old, new) in [
            (bytes, before.charged_bytes, after.charged_bytes),
            (objects, before.objects, after.objects),
        ] {
            // Exact retries remain possible after an operator reduces a quota.
            if limit.is_some_and(|limit| new > limit && new > old) {
                // No counters, object identities or cross-Space detail in errors.
                return Err(V3StoreError::LimitExceeded {
                    resource: "CAS storage quota",
                    limit: limit.unwrap(),
                });
            }
        }
        Ok(())
    }
}

async fn usage(
    tx: &mut Transaction<'_, Postgres>,
    space: Option<&SpaceId>,
    additions: &[ObjectDescriptor],
) -> V3StoreResult<CasBudgetUsage> {
    let mut ids = Vec::with_capacity(additions.len());
    let mut canonical = Vec::with_capacity(additions.len());
    let mut encoded = Vec::with_capacity(additions.len());
    let mut kinds = Vec::with_capacity(additions.len());
    let mut compressions = Vec::with_capacity(additions.len());
    for descriptor in additions {
        ids.push(descriptor.object_id.as_str());
        canonical.push(super::i64_from_u64(descriptor.canonical_byte_len)?);
        encoded.push(super::i64_from_u64(descriptor.encoded_byte_len)?);
        kinds.push(super::object_kind(descriptor.kind));
        compressions.push(super::compression(descriptor.compression));
    }
    // SQL aggregates avoid collecting an unbounded catalogue in Rust. All
    // states count, including quarantined objects and expired reservations.
    let row = sqlx::query(r#"
        WITH entries AS (
            SELECT o.object_id, o.canonical_byte_length AS canonical_len,
                o.encoded_byte_length AS encoded_len, o.object_kind, o.compression, true AS catalogued
            FROM canonical_objects o
            WHERE $1::TEXT IS NULL OR EXISTS (
                SELECT 1 FROM space_object_availability a
                WHERE a.space_id=$1 AND a.object_id=o.object_id)
            UNION ALL
            SELECT i.object_id,i.canonical_byte_length,i.encoded_byte_length,i.object_kind,i.compression,false
            FROM object_plan_items i JOIN object_plans p ON p.plan_id=i.plan_id
            WHERE $1::TEXT IS NULL OR p.space_id=$1
            UNION ALL
            SELECT object_id,canonical_len,encoded_len,object_kind,compression,false
            FROM UNNEST($2::TEXT[],$3::BIGINT[],$4::BIGINT[],$5::TEXT[],$6::TEXT[])
                AS incoming(object_id,canonical_len,encoded_len,object_kind,compression)
        ), objects AS (
            SELECT object_id, MAX(GREATEST(canonical_len,encoded_len)) AS charge,
                BOOL_OR(catalogued) AS catalogued,
                COUNT(DISTINCT (canonical_len,encoded_len,object_kind,compression)) AS variants
            FROM entries GROUP BY object_id
        )
        SELECT COUNT(*)::BIGINT AS objects, COALESCE(SUM(charge),0)::TEXT AS charged,
            COALESCE(SUM(charge) FILTER (WHERE catalogued),0)::TEXT AS catalogued,
            COALESCE(SUM(charge) FILTER (WHERE NOT catalogued),0)::TEXT AS reserved,
            COALESCE(BOOL_OR(variants<>1),false) AS inconsistent
        FROM objects
    "#).bind(space.map(SpaceId::as_str)).bind(ids).bind(canonical).bind(encoded)
        .bind(kinds).bind(compressions).fetch_one(&mut **tx).await.map_err(pg_error)?;
    if row.get::<bool, _>("inconsistent") {
        return Err(V3StoreError::CorruptObject(
            "inconsistent CAS reservation metadata".into(),
        ));
    }
    let number = |field| {
        row.get::<String, _>(field)
            .parse::<u64>()
            .map_err(|_| V3StoreError::CorruptObject("CAS quota accounting overflow".into()))
    };
    Ok(CasBudgetUsage {
        objects: u64::try_from(row.get::<i64, _>("objects"))
            .map_err(|_| V3StoreError::CorruptObject("CAS object count overflow".into()))?,
        charged_bytes: number("charged")?,
        catalogued_bytes: number("catalogued")?,
        reserved_only_bytes: number("reserved")?,
    })
}
