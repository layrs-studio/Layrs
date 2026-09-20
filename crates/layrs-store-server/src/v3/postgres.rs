use std::sync::Arc;

use sqlx::{PgPool, Row};

use async_trait::async_trait;
use bytes::Bytes;
use layrs_sync::v3::{ObjectDescriptor, ObjectKind};

use super::{CapabilityCodec, PolicyAuthorizer, ServerCas, V3StoreError, V3StoreResult, schema};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCasObject {
    pub descriptor: ObjectDescriptor,
    pub store_id: String,
    pub storage_key: String,
    pub backend_etag: Option<String>,
}

/// Backend-specific CAS/catalog bridge. The CAS implementation must finish
/// and verify durable installation before returning a catalog binding. A later
/// PostgreSQL rollback may therefore leave a safe, unreachable CAS orphan.
#[async_trait]
pub trait CatalogedCas: ServerCas + Send + Sync {
    async fn verify_existing(
        &self,
        descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<PreparedCasObject>;

    async fn prepare_generated(
        &self,
        kind: ObjectKind,
        canonical_bytes: Bytes,
    ) -> V3StoreResult<PreparedCasObject>;
}

#[derive(Clone)]
pub struct PgV3Repository {
    pool: PgPool,
    cas: Arc<dyn CatalogedCas>,
    capabilities: CapabilityCodec,
    authorizer: Arc<dyn PolicyAuthorizer>,
    quotas: super::CasQuotas,
    budget_writer: Arc<tokio::sync::Mutex<()>>,
}

impl PgV3Repository {
    pub fn new(
        pool: PgPool,
        cas: Arc<dyn CatalogedCas>,
        capabilities: CapabilityCodec,
        authorizer: Arc<dyn PolicyAuthorizer>,
    ) -> Self {
        Self {
            pool,
            cas,
            capabilities,
            authorizer,
            quotas: super::CasQuotas::default(),
            budget_writer: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn with_quotas(mut self, quotas: super::CasQuotas) -> V3StoreResult<Self> {
        self.quotas = quotas.validate()?;
        Ok(self)
    }

    pub fn quotas(&self) -> super::CasQuotas {
        self.quotas
    }

    pub(crate) async fn cas_budget_writer(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.budget_writer.clone().lock_owned().await
    }

    pub fn cas(&self) -> &Arc<dyn CatalogedCas> {
        &self.cas
    }

    pub fn capabilities(&self) -> &CapabilityCodec {
        &self.capabilities
    }

    pub fn authorizer(&self) -> &Arc<dyn PolicyAuthorizer> {
        &self.authorizer
    }

    /// Refuses a connection that is not explicitly a member of the baseline's
    /// NOLOGIN writer role. Deployments should call this once during startup;
    /// tests using the migration owner can intentionally omit it.
    pub async fn verify_writer_membership(&self) -> V3StoreResult<()> {
        self.verify_schema().await?;
        let row = sqlx::query(
            r#"
            SELECT current_user AS current_login,
                   m.writer_role_name,
                   pg_has_role(current_user, m.writer_role_name, 'MEMBER') AS is_member
            FROM schema_metadata m
            WHERE m.singleton_id = 1
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| V3StoreError::Backend(error.to_string()))?;
        let role: String = row.get("writer_role_name");
        let member: bool = row.get("is_member");
        if !member || !role.starts_with("layrs_v3_w_") {
            return Err(V3StoreError::Backend(
                "database login is not a member of the Sync V3 writer role".into(),
            ));
        }
        Ok(())
    }

    pub async fn verify_schema(&self) -> V3StoreResult<()> {
        let row = sqlx::query(schema::METADATA_PROBE_SQL)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| V3StoreError::Backend(error.to_string()))?;
        let metadata = row.as_ref().map(|row| schema::SchemaMetadata {
            schema_version: row.get("schema_version"),
            protocol_version: row.get("protocol_version"),
            baseline_migration: row.get("baseline_migration"),
            schema_contract_checksum: row.get("schema_contract_checksum"),
            catalog_fingerprint: row.get("catalog_fingerprint"),
            actual_catalog_fingerprint: row.get("actual_catalog_fingerprint"),
        });
        schema::verify_metadata(metadata).map_err(|error| V3StoreError::Backend(error.to_string()))
    }
}
