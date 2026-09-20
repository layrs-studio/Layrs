mod chunks;
mod content_acl;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use layrs_core::scm::{ObjectId, SpaceId};
use layrs_sync::v3::{AuthorizationRoot, Compression, ObjectDescriptor, ObjectKind};
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::{
    AuditPurpose, AuthorizationCheck, AuthorizationTarget, CapabilityActor, ClaimResolution,
    DeniedCode, FetchClaim, FileManifestSelection, PgV3Repository, PreparedCasObject,
    PrincipalContext, RootedFetchAvailability, SpaceObjectAvailability, V3Action, V3StoreError,
    V3StoreResult, pg_error, require_allowed,
};

#[derive(Debug, Clone)]
pub struct CompleteUploadRequest {
    pub principal: PrincipalContext,
    pub actor: CapabilityActor,
    pub descriptor: ObjectDescriptor,
    pub capability_token: String,
    pub now_unix_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteUploadOutcome {
    Admitted,
    AlreadyAdmitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogInstallOutcome {
    Installed,
    AlreadyInstalled,
}

#[derive(Clone)]
pub struct PgCatalogStore {
    repository: PgV3Repository,
}

impl PgCatalogStore {
    pub fn new(repository: PgV3Repository) -> Self {
        Self { repository }
    }

    pub fn repository(&self) -> &PgV3Repository {
        &self.repository
    }

    /// Transaction-engine primitive: catalogues an already prepared CAS
    /// object without granting Space reachability. The caller owns the SQL
    /// transaction and may then insert typed projections before availability.
    pub async fn install_prepared(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        prepared: &PreparedCasObject,
    ) -> V3StoreResult<CatalogInstallOutcome> {
        if prepared.store_id.is_empty() || prepared.storage_key.is_empty() {
            return Err(V3StoreError::InvalidDescriptor(
                "CAS catalog binding is incomplete".into(),
            ));
        }
        let descriptor = &prepared.descriptor;
        self.repository
            .check_cas_budget(tx, None, std::slice::from_ref(descriptor))
            .await?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO canonical_objects (
                object_id, store_id, object_kind, canonical_byte_length,
                encoded_byte_length, compression, storage_key,
                verification_state, verified_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,'verified',transaction_timestamp())
            ON CONFLICT (object_id) DO NOTHING
            "#,
        )
        .bind(descriptor.object_id.as_str())
        .bind(&prepared.store_id)
        .bind(object_kind(descriptor.kind))
        .bind(i64_from_u64(descriptor.canonical_byte_len)?)
        .bind(i64_from_u64(descriptor.encoded_byte_len)?)
        .bind(compression(descriptor.compression))
        .bind(&prepared.storage_key)
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?
        .rows_affected()
            == 1;
        let exact: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM canonical_objects
                WHERE object_id=$1 AND store_id=$2 AND object_kind=$3
                  AND canonical_byte_length=$4 AND encoded_byte_length=$5
                  AND compression=$6 AND storage_key=$7 AND verification_state='verified'
            )
            "#,
        )
        .bind(descriptor.object_id.as_str())
        .bind(&prepared.store_id)
        .bind(object_kind(descriptor.kind))
        .bind(i64_from_u64(descriptor.canonical_byte_len)?)
        .bind(i64_from_u64(descriptor.encoded_byte_len)?)
        .bind(compression(descriptor.compression))
        .bind(&prepared.storage_key)
        .fetch_one(&mut **tx)
        .await
        .map_err(pg_error)?;
        if !exact {
            return Err(V3StoreError::CorruptObject(
                "ObjectId is already catalogued with different metadata".into(),
            ));
        }
        if descriptor.kind == ObjectKind::Chunk {
            sqlx::query(
                r#"
                INSERT INTO chunks (chunk_id, raw_byte_length, backend_etag)
                VALUES ($1,$2,$3) ON CONFLICT (chunk_id) DO NOTHING
                "#,
            )
            .bind(descriptor.object_id.as_str())
            .bind(i64_from_u64(descriptor.canonical_byte_len)?)
            .bind(&prepared.backend_etag)
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;
            let exact_chunk: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM chunks
                    WHERE chunk_id=$1 AND raw_byte_length=$2
                      AND backend_etag IS NOT DISTINCT FROM $3
                )
                "#,
            )
            .bind(descriptor.object_id.as_str())
            .bind(i64_from_u64(descriptor.canonical_byte_len)?)
            .bind(&prepared.backend_etag)
            .fetch_one(&mut **tx)
            .await
            .map_err(pg_error)?;
            if !exact_chunk {
                return Err(V3StoreError::CorruptObject(
                    "Chunk is already catalogued with different metadata".into(),
                ));
            }
        }
        Ok(if inserted {
            CatalogInstallOutcome::Installed
        } else {
            CatalogInstallOutcome::AlreadyInstalled
        })
    }

    pub async fn make_available(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        space_id: &SpaceId,
        object_id: &ObjectId,
    ) -> V3StoreResult<bool> {
        if self.repository.quotas().max_space_bytes.is_some()
            || self.repository.quotas().max_space_objects.is_some()
        {
            let row = sqlx::query("SELECT object_id, object_kind, canonical_byte_length, encoded_byte_length, compression FROM canonical_objects WHERE object_id=$1")
                .bind(object_id.as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
            let descriptor = super::plan_store::descriptor_from_row(&row)?;
            self.repository
                .check_cas_budget(tx, Some(space_id), &[descriptor])
                .await?;
        }
        Ok(sqlx::query(
            r#"
            INSERT INTO space_object_availability (space_id, object_id, availability_state)
            VALUES ($1,$2,'available')
            ON CONFLICT (space_id, object_id) DO NOTHING
            "#,
        )
        .bind(space_id.as_str())
        .bind(object_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?
        .rows_affected()
            == 1)
    }

    /// Bridges a fully verified CAS object into PostgreSQL. All failures that
    /// could reveal global CAS or cross-Space state collapse to capability
    /// failure; canonical metadata is installed before Space availability.
    pub async fn complete_upload(
        &self,
        request: CompleteUploadRequest,
    ) -> V3StoreResult<CompleteUploadOutcome> {
        if request.principal.principal_id != request.actor.principal_id {
            return Err(V3StoreError::InvalidCapability);
        }
        let descriptor_digest =
            super::capability::digest_json("layrs.sync.v3/descriptor", &request.descriptor)?;
        let token_digest = token_digest(&request.capability_token);
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        super::transaction_store::lock_identity(
            &mut tx,
            &request.principal,
            &request.actor.space_id,
        )
        .await?;
        sqlx::query("SELECT space_id FROM spaces WHERE space_id=$1 FOR UPDATE")
            .bind(request.actor.space_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;

        let binding = sqlx::query(
            r#"
            SELECT p.status AS principal_status,
                   s.lifecycle_state,
                   s.authorization_epoch,
                   s.current_policy_set_id,
                   d.store_id AS expected_store_id,
                   op.expires_at > to_timestamp($10) AS unexpired,
                   uc.expires_at > to_timestamp($10) AS capability_unexpired,
                   op.expires_at = uc.expires_at AS expiry_matches,
                   op.plan_state,
                   uc.token_digest,
                   opi.object_kind,
                   opi.canonical_byte_length,
                   opi.encoded_byte_length,
                   opi.compression,
                   wm.state AS membership_state,
                   soa.object_id IS NOT NULL AS already_admitted
            FROM object_plans op
            JOIN object_plan_items opi
              ON opi.plan_id = op.plan_id
             AND opi.object_id = $7
             AND opi.disposition = 'missing'
            JOIN upload_capabilities uc
              ON uc.plan_id = op.plan_id
             AND uc.item_ordinal = opi.ordinal
             AND uc.object_id = opi.object_id
            JOIN spaces s ON s.space_id = op.space_id
            JOIN workspaces w ON w.workspace_id = s.workspace_id
            JOIN deployments d ON d.deployment_id = w.deployment_id
            JOIN principals p ON p.principal_id = op.principal_id
            JOIN workspace_memberships wm
              ON wm.workspace_id = s.workspace_id
             AND wm.principal_id = p.principal_id
            LEFT JOIN space_object_availability soa
              ON soa.space_id = op.space_id
             AND soa.object_id = opi.object_id
             AND soa.availability_state = 'available'
            WHERE op.plan_id = $1
              AND op.space_id = $2
              AND op.client_id = $3
              AND op.request_id = $4
              AND op.principal_id = $5
              AND op.authorization_epoch = $6
              AND opi.descriptor_digest = $8
              AND uc.token_digest = $9
            FOR UPDATE OF op, uc
            "#,
        )
        .bind(request.actor.plan_id.as_str())
        .bind(request.actor.space_id.as_str())
        .bind(request.actor.client_id.as_str())
        .bind(request.actor.request_id.as_str())
        .bind(request.actor.principal_id.as_str())
        .bind(i64_from_u64(request.actor.authorization_epoch)?)
        .bind(request.descriptor.object_id.as_str())
        .bind(descriptor_digest.as_str())
        .bind(&token_digest)
        .bind(i64_from_u64(request.now_unix_seconds)?)
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_error)?;

        let Some(binding) = binding else {
            return Err(V3StoreError::InvalidCapability);
        };
        let epoch: i64 = binding.get("authorization_epoch");
        if epoch != i64_from_u64(request.actor.authorization_epoch)? {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        if binding.get::<String, _>("principal_status") != "active"
            || binding.get::<String, _>("lifecycle_state") != "ready"
            || binding.get::<String, _>("plan_state") != "finalized"
            || !binding.get::<bool, _>("unexpired")
            || !binding.get::<bool, _>("capability_unexpired")
            || !binding.get::<bool, _>("expiry_matches")
            || binding.get::<String, _>("membership_state") != "active"
        {
            return Err(V3StoreError::InvalidCapability);
        }
        verify_stored_descriptor(&binding, &request.descriptor)?;
        self.repository.capabilities().verify_upload(
            &request.capability_token,
            &request.actor,
            &request.descriptor,
            u64::try_from(epoch).map_err(|_| V3StoreError::InvalidCapability)?,
            request.now_unix_seconds,
        )?;

        let policy_set_id = ObjectId::parse(binding.get::<String, _>("current_policy_set_id"))
            .map_err(|_| V3StoreError::InvalidCapability)?;
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: request.principal.clone(),
                    action: V3Action::CompleteUpload,
                    target: AuthorizationTarget::Object {
                        space_id: request.actor.space_id.clone(),
                        object_id: request.descriptor.object_id.clone(),
                    },
                    policy_set_id,
                    authorization_epoch: request.actor.authorization_epoch,
                })
                .await?,
        )?;

        self.repository
            .check_cas_budget(&mut tx, None, std::slice::from_ref(&request.descriptor))
            .await?;
        self.repository
            .check_cas_budget(
                &mut tx,
                Some(&request.actor.space_id),
                std::slice::from_ref(&request.descriptor),
            )
            .await?;
        let prepared = self
            .repository
            .cas()
            .verify_existing(&request.descriptor)
            .await?;
        if prepared.descriptor != request.descriptor
            || prepared.store_id.is_empty()
            || prepared.storage_key.is_empty()
            || prepared.store_id != binding.get::<String, _>("expected_store_id")
        {
            return Err(V3StoreError::InvalidCapability);
        }

        let store_id = prepared.store_id;
        let storage_key = prepared.storage_key;
        sqlx::query(
            r#"
            INSERT INTO canonical_objects (
                object_id, store_id, object_kind, canonical_byte_length,
                encoded_byte_length, compression, storage_key,
                verification_state, verified_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'verified', transaction_timestamp())
            ON CONFLICT (object_id) DO NOTHING
            "#,
        )
        .bind(request.descriptor.object_id.as_str())
        .bind(&store_id)
        .bind(object_kind(request.descriptor.kind))
        .bind(i64_from_u64(request.descriptor.canonical_byte_len)?)
        .bind(i64_from_u64(request.descriptor.encoded_byte_len)?)
        .bind(compression(request.descriptor.compression))
        .bind(&storage_key)
        .execute(&mut *tx)
        .await
        .map_err(pg_error)?;

        let exact: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM canonical_objects
                WHERE object_id = $1 AND store_id = $2 AND object_kind = $3
                  AND canonical_byte_length = $4 AND encoded_byte_length = $5
                  AND compression = $6 AND storage_key = $7
                  AND verification_state = 'verified'
            )
            "#,
        )
        .bind(request.descriptor.object_id.as_str())
        .bind(&store_id)
        .bind(object_kind(request.descriptor.kind))
        .bind(i64_from_u64(request.descriptor.canonical_byte_len)?)
        .bind(i64_from_u64(request.descriptor.encoded_byte_len)?)
        .bind(compression(request.descriptor.compression))
        .bind(&storage_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        if !exact {
            return Err(V3StoreError::InvalidCapability);
        }

        if request.descriptor.kind == ObjectKind::Chunk {
            sqlx::query(
                r#"
                INSERT INTO chunks (chunk_id, raw_byte_length, backend_etag)
                VALUES ($1, $2, $3)
                ON CONFLICT (chunk_id) DO NOTHING
                "#,
            )
            .bind(request.descriptor.object_id.as_str())
            .bind(i64_from_u64(request.descriptor.canonical_byte_len)?)
            .bind(&prepared.backend_etag)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
            let exact_chunk: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM chunks
                    WHERE chunk_id=$1 AND raw_byte_length=$2
                      AND backend_etag IS NOT DISTINCT FROM $3
                )
                "#,
            )
            .bind(request.descriptor.object_id.as_str())
            .bind(i64_from_u64(request.descriptor.canonical_byte_len)?)
            .bind(&prepared.backend_etag)
            .fetch_one(&mut *tx)
            .await
            .map_err(pg_error)?;
            if !exact_chunk {
                return Err(V3StoreError::InvalidCapability);
            }
        }

        if request.descriptor.kind == ObjectKind::FileManifest {
            let manifest = super::closure::verified_manifest(
                &self.repository,
                &request.actor.space_id,
                &request.descriptor,
            )
            .await?;
            super::closure::insert_manifest(&mut tx, &manifest).await?;
        }

        let availability_inserted = sqlx::query(
            r#"
            INSERT INTO space_object_availability (space_id, object_id, availability_state)
            VALUES ($1, $2, 'available')
            ON CONFLICT (space_id, object_id) DO NOTHING
            "#,
        )
        .bind(request.actor.space_id.as_str())
        .bind(request.descriptor.object_id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(pg_error)?
        .rows_affected()
            == 1;
        let orphan_promoted = if availability_inserted {
            false
        } else {
            sqlx::query(
                r#"
                UPDATE space_object_availability SET availability_state='available'
                WHERE space_id=$1 AND object_id=$2 AND availability_state='leased_orphan'
                "#,
            )
            .bind(request.actor.space_id.as_str())
            .bind(request.descriptor.object_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?
            .rows_affected()
                == 1
        };
        let is_available: bool = sqlx::query_scalar(
            "SELECT availability_state='available' FROM space_object_availability WHERE space_id=$1 AND object_id=$2",
        )
        .bind(request.actor.space_id.as_str())
        .bind(request.descriptor.object_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        if !is_available {
            return Err(V3StoreError::InvalidCapability);
        }
        tx.commit().await.map_err(pg_error)?;

        Ok(if availability_inserted || orphan_promoted {
            CompleteUploadOutcome::Admitted
        } else {
            CompleteUploadOutcome::AlreadyAdmitted
        })
    }
}

#[async_trait]
impl SpaceObjectAvailability for PgCatalogStore {
    async fn is_verified_available(
        &self,
        space_id: &SpaceId,
        descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<bool> {
        sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM space_object_availability soa
                JOIN canonical_objects o ON o.object_id = soa.object_id
                WHERE soa.space_id = $1
                  AND soa.object_id = $2
                  AND soa.availability_state = 'available'
                  AND o.verification_state = 'verified'
                  AND o.object_kind = $3
                  AND o.canonical_byte_length = $4
                  AND o.encoded_byte_length = $5
                  AND o.compression = $6
            )
            "#,
        )
        .bind(space_id.as_str())
        .bind(descriptor.object_id.as_str())
        .bind(object_kind(descriptor.kind))
        .bind(i64_from_u64(descriptor.canonical_byte_len)?)
        .bind(i64_from_u64(descriptor.encoded_byte_len)?)
        .bind(compression(descriptor.compression))
        .fetch_one(self.repository.pool())
        .await
        .map_err(pg_error)
    }
}

#[derive(Clone)]
pub struct PgRootedFetchResolver {
    repository: PgV3Repository,
    principal: PrincipalContext,
    policy_set_id: ObjectId,
    authorization_epoch: u64,
}

impl PgRootedFetchResolver {
    pub fn new(
        repository: PgV3Repository,
        principal: PrincipalContext,
        policy_set_id: ObjectId,
        authorization_epoch: u64,
    ) -> Self {
        Self {
            repository,
            principal,
            policy_set_id,
            authorization_epoch,
        }
    }

    async fn authorize_claim(&self, space_id: &SpaceId, claim: &FetchClaim) -> V3StoreResult<bool> {
        if !matches!(claim,FetchClaim::AuthorizationSnapshot{..}) {
            let general=self.repository.authorizer().authorize(&AuthorizationCheck {
                principal:self.principal.clone(),action:V3Action::FetchObject,
                target:AuthorizationTarget::Space(space_id.clone()),policy_set_id:self.policy_set_id.clone(),authorization_epoch:self.authorization_epoch,
            }).await?;
            if !general.allowed { return Ok(false); }
            if general.content_restricted && matches!(claim,FetchClaim::File{..}|FetchClaim::Object{repo_path:Some(_),..}) {
                let Some(target)=self.restricted_target(space_id,claim).await? else { return Ok(false); };
                return Ok(self.repository.authorizer().authorize(&AuthorizationCheck { principal:self.principal.clone(),action:V3Action::FetchObject,target,policy_set_id:self.policy_set_id.clone(),authorization_epoch:self.authorization_epoch }).await?.allowed);
            }
        }
        let (action, target) = match claim {
            FetchClaim::Object {
                object, repo_path, ..
            } => (
                V3Action::FetchObject,
                match repo_path {
                    Some(path) => AuthorizationTarget::Path {
                        space_id: space_id.clone(),
                        repo_path: path.clone(),
                    },
                    None => AuthorizationTarget::Object {
                        space_id: space_id.clone(),
                        object_id: object.object_id.clone(),
                    },
                },
            ),
            FetchClaim::File { repo_path, .. } => (
                V3Action::FetchObject,
                AuthorizationTarget::Path {
                    space_id: space_id.clone(),
                    repo_path: repo_path.clone(),
                },
            ),
            FetchClaim::AuthorizationSnapshot {
                authorization_snapshot_id,
                purpose,
            } => (
                match purpose {
                    AuditPurpose::Audit | AuditPurpose::Administration => {
                        V3Action::ReadAuditEvidence
                    }
                },
                AuthorizationTarget::Object {
                    space_id: space_id.clone(),
                    object_id: authorization_snapshot_id.clone(),
                },
            ),
        };
        Ok(self
            .repository
            .authorizer()
            .authorize(&AuthorizationCheck {
                principal: self.principal.clone(),
                action,
                target,
                policy_set_id: self.policy_set_id.clone(),
                authorization_epoch: self.authorization_epoch,
            })
            .await?
            .allowed)
    }

    async fn resolve_root(
        &self,
        space_id: &SpaceId,
        root: &AuthorizationRoot,
    ) -> V3StoreResult<Option<ObjectId>> {
        let value = match root {
            AuthorizationRoot::Step { step_id } => sqlx::query_scalar::<_, String>(
                "SELECT step_id FROM space_steps WHERE space_id=$1 AND step_id=$2",
            )
            .bind(space_id.as_str())
            .bind(step_id.as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(pg_error)?,
            AuthorizationRoot::LayerHead {
                layer_id,
                generation,
                head_step_id,
            } => sqlx::query_scalar::<_, String>(
                r#"
                SELECT h.step_id
                FROM spaces s
                JOIN repository_view_layer_heads h
                  ON h.space_id=s.space_id
                 AND h.repository_view_id=s.current_repository_view_id
                WHERE s.space_id=$1 AND s.current_repository_view_generation=$2
                  AND h.layer_id=$3 AND h.step_id=$4
                "#,
            )
            .bind(space_id.as_str())
            .bind(i64_from_u64(*generation)?)
            .bind(layer_id.as_str())
            .bind(head_step_id.as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(pg_error)?,
            AuthorizationRoot::Product { product } => sqlx::query_scalar::<_, String>(
                "SELECT product_object_id FROM products WHERE space_id=$1 AND product_object_id=$2 AND logical_id=$3",
            )
            .bind(space_id.as_str())
            .bind(product.object_id().as_str())
            .bind(product.logical_id().as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(pg_error)?,
            AuthorizationRoot::RepositoryView { repository_view_id } => {
                sqlx::query_scalar::<_, String>(
                    "SELECT repository_view_id FROM repository_views WHERE space_id=$1 AND repository_view_id=$2",
                )
                .bind(space_id.as_str())
                .bind(repository_view_id.as_str())
                .fetch_optional(self.repository.pool())
                .await
                .map_err(pg_error)?
            }
            AuthorizationRoot::PolicySet { policy_set_id } => sqlx::query_scalar::<_, String>(
                "SELECT policy_set_id FROM policy_sets WHERE space_id=$1 AND policy_set_id=$2",
            )
            .bind(space_id.as_str())
            .bind(policy_set_id.as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(pg_error)?,
        };
        value
            .map(|value| {
                ObjectId::parse(value)
                    .map_err(|_| V3StoreError::CorruptObject("invalid root ObjectId".into()))
            })
            .transpose()
    }

    async fn reachable_object(
        &self,
        space_id: &SpaceId,
        root_id: &ObjectId,
        target: &ObjectId,
        repo_path: Option<&layrs_core::scm::RepoPath>,
    ) -> V3StoreResult<bool> {
        rooted_reachable(
            self.repository.pool(),
            space_id,
            root_id,
            target.as_str(),
            repo_path.map(|path| path.as_str()),
            false,
        )
        .await
    }

    async fn descriptor(
        &self,
        space_id: &SpaceId,
        object_id: &ObjectId,
    ) -> V3StoreResult<Option<ObjectDescriptor>> {
        let row = sqlx::query(
            r#"
            SELECT o.object_kind, o.canonical_byte_length, o.encoded_byte_length, o.compression
            FROM canonical_objects o
            JOIN space_object_availability a ON a.object_id=o.object_id
            WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available'
              AND o.verification_state='verified'
            "#,
        )
        .bind(space_id.as_str())
        .bind(object_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(pg_error)?;
        row.map(|row| {
            Ok(ObjectDescriptor {
                kind: super::plan_store::parse_kind(&row.get::<String, _>("object_kind"))?,
                object_id: object_id.clone(),
                canonical_byte_len: u64::try_from(row.get::<i64, _>("canonical_byte_length"))
                    .map_err(|_| V3StoreError::CorruptObject("invalid length".into()))?,
                encoded_byte_len: u64::try_from(row.get::<i64, _>("encoded_byte_length"))
                    .map_err(|_| V3StoreError::CorruptObject("invalid length".into()))?,
                compression: match row.get::<String, _>("compression").as_str() {
                    "identity" => Compression::Identity,
                    "zstd" => Compression::Zstd,
                    _ => return Err(V3StoreError::CorruptObject("invalid compression".into())),
                },
            })
        })
        .transpose()
    }
}

#[async_trait]
impl RootedFetchAvailability for PgRootedFetchResolver {
    async fn resolve(
        &self,
        space_id: &SpaceId,
        claim: &FetchClaim,
    ) -> V3StoreResult<ClaimResolution> {
        if !self.authorize_claim(space_id, claim).await? {
            return Ok(ClaimResolution::Denied(match claim {
                FetchClaim::AuthorizationSnapshot { .. } => DeniedCode::AuditDenied,
                FetchClaim::Object {
                    repo_path: Some(_), ..
                }
                | FetchClaim::File { .. } => DeniedCode::PathDenied,
                _ => DeniedCode::NotReachable,
            }));
        }
        match claim {
            FetchClaim::AuthorizationSnapshot {
                authorization_snapshot_id,
                ..
            } => Ok(
                match self.descriptor(space_id, authorization_snapshot_id).await? {
                    Some(descriptor) if descriptor.kind == ObjectKind::AuthorizationSnapshot => {
                        ClaimResolution::Available {
                            descriptors: vec![descriptor],
                            file_manifest_selection: None,
                        }
                    }
                    _ => ClaimResolution::Denied(DeniedCode::NotAvailable),
                },
            ),
            FetchClaim::Object {
                object,
                root,
                repo_path,
            } => {
                let Some(root_id) = self.resolve_root(space_id, root).await? else {
                    return Ok(ClaimResolution::Denied(DeniedCode::InvalidRoot));
                };
                if !self
                    .reachable_object(space_id, &root_id, &object.object_id, repo_path.as_ref())
                    .await?
                {
                    return Ok(ClaimResolution::Denied(DeniedCode::NotReachable));
                }
                Ok(match self.descriptor(space_id, &object.object_id).await? {
                    Some(descriptor) if descriptor.kind == object.kind => {
                        ClaimResolution::Available {
                            descriptors: vec![descriptor],
                            file_manifest_selection: None,
                        }
                    }
                    _ => ClaimResolution::Denied(DeniedCode::NotAvailable),
                })
            }
            FetchClaim::File {
                file_id,
                preferred_manifest_ids,
                root,
                repo_path,
            } => {
                let Some(root_id) = self.resolve_root(space_id, root).await? else {
                    return Ok(ClaimResolution::Denied(DeniedCode::InvalidRoot));
                };
                let reachable = rooted_reachable(
                    self.repository.pool(),
                    space_id,
                    &root_id,
                    file_id.as_str(),
                    Some(repo_path.as_str()),
                    true,
                )
                .await?;
                if !reachable {
                    return Ok(ClaimResolution::Denied(DeniedCode::NotReachable));
                }
                let preferred = preferred_manifest_ids
                    .iter()
                    .map(|id| id.as_str().to_owned())
                    .collect::<Vec<_>>();
                let row = sqlx::query(
                    r#"
                    SELECT m.file_manifest_id
                    FROM file_manifests m
                    JOIN space_object_availability a
                      ON a.space_id=$1 AND a.object_id=m.file_manifest_id
                     AND a.availability_state='available'
                    WHERE m.file_id=$2
                      AND NOT EXISTS (
                          SELECT 1 FROM file_manifest_chunks mc
                          LEFT JOIN space_object_availability ca
                            ON ca.space_id=$1 AND ca.object_id=mc.chunk_id
                           AND ca.availability_state='available'
                          WHERE mc.file_manifest_id=m.file_manifest_id AND ca.object_id IS NULL
                      )
                    ORDER BY CASE WHEN cardinality($3::TEXT[]) = 0 THEN 0
                                  ELSE array_position($3::TEXT[], m.file_manifest_id) END NULLS LAST,
                             m.reconstruction_verified_at DESC, m.file_manifest_id COLLATE "C"
                    LIMIT 1
                    "#,
                )
                .bind(space_id.as_str())
                .bind(file_id.as_str())
                .bind(&preferred)
                .fetch_optional(self.repository.pool())
                .await
                .map_err(pg_error)?;
                let Some(row) = row else {
                    return Ok(ClaimResolution::Denied(DeniedCode::NoVerifiedManifest));
                };
                let manifest_id = layrs_core::scm::FileManifestId::parse(
                    row.get::<String, _>("file_manifest_id"),
                )
                .map_err(|_| V3StoreError::CorruptObject("invalid FileManifestId".into()))?;
                let manifest_object_id = ObjectId::parse(manifest_id.as_str())
                    .map_err(|_| V3StoreError::CorruptObject("invalid manifest ObjectId".into()))?;
                let manifest_descriptor = self
                    .descriptor(space_id, &manifest_object_id)
                    .await?
                    .ok_or_else(|| V3StoreError::CorruptObject("missing manifest".into()))?;
                let chunk_descriptors =
                    chunks::descriptors(self.repository.pool(), space_id, &manifest_id).await?;
                let mut descriptors = vec![manifest_descriptor.clone()];
                descriptors.extend(chunk_descriptors.iter().cloned());
                Ok(ClaimResolution::Available {
                    descriptors,
                    file_manifest_selection: Some(FileManifestSelection {
                        file_id: file_id.clone(),
                        file_manifest_id: manifest_id,
                        manifest_descriptor,
                        chunk_descriptors,
                    }),
                })
            }
        }
    }
}

/// Traverses typed repository roots as well as content edges. Path components
/// accumulate through hierarchical Trees; a basename elsewhere never matches.
async fn rooted_reachable(
    pool: &sqlx::PgPool,
    space: &SpaceId,
    root: &ObjectId,
    target: &str,
    path: Option<&str>,
    file: bool,
) -> V3StoreResult<bool> {
    sqlx::query_scalar(r#"
        WITH RECURSIVE edges(source,target,component) AS (
            SELECT source_object_id,target_object_id,repo_path FROM canonical_object_edges WHERE target_object_id IS NOT NULL
            UNION ALL SELECT e.source_object_id,m.file_manifest_id,e.repo_path FROM canonical_object_edges e JOIN file_manifests m ON m.file_id=e.target_file_id
            UNION ALL SELECT step_id,root_tree_id,NULL::TEXT FROM steps
            UNION ALL SELECT step_id,parent_step_id,NULL::TEXT FROM step_parents WHERE $4::TEXT IS NULL
            UNION ALL SELECT repository_view_id,step_id,NULL::TEXT FROM repository_view_layer_heads WHERE space_id=$1
            UNION ALL SELECT repository_view_id,marker_product_object_id,NULL::TEXT FROM repository_view_markers WHERE space_id=$1
            UNION ALL SELECT repository_view_id,release_product_object_id,NULL::TEXT FROM repository_view_releases WHERE space_id=$1
            UNION ALL SELECT product_object_id,step_id,NULL::TEXT FROM product_step_dependencies WHERE space_id=$1
            UNION ALL SELECT product_object_id,dependency_product_object_id,NULL::TEXT FROM product_dependencies WHERE space_id=$1
            UNION ALL SELECT policy_set_id,policy_product_object_id,NULL::TEXT FROM policy_set_entries WHERE space_id=$1
        ), reachable(object_id,repo_path,depth) AS (
            SELECT $2::TEXT,''::TEXT,0
            UNION
            SELECT edge.target,
                CASE WHEN edge.component IS NULL THEN parent.repo_path
                     WHEN parent.repo_path='' THEN edge.component
                     ELSE parent.repo_path || '/' || edge.component END,
                parent.depth+1
            FROM reachable parent JOIN edges edge ON edge.source=parent.object_id
            JOIN space_object_availability a ON a.space_id=$1 AND a.object_id=edge.target AND a.availability_state='available'
            JOIN canonical_objects o ON o.object_id=edge.target AND o.verification_state='verified'
            WHERE parent.depth<256
        )
        SELECT CASE WHEN $5::BOOLEAN THEN EXISTS (
            SELECT 1 FROM reachable r JOIN canonical_object_edges e ON e.source_object_id=r.object_id
            WHERE e.target_file_id=$3 AND CASE WHEN e.repo_path IS NULL THEN r.repo_path WHEN r.repo_path='' THEN e.repo_path ELSE r.repo_path || '/' || e.repo_path END=$4
        ) ELSE EXISTS (SELECT 1 FROM reachable WHERE object_id=$3 AND ($4::TEXT IS NULL OR repo_path=$4)) END
    "#).bind(space.as_str()).bind(root.as_str()).bind(target).bind(path).bind(file).fetch_one(pool).await.map_err(pg_error)
}

pub(crate) fn object_kind(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Step => "step",
        ObjectKind::RepositoryView => "repository_view",
        ObjectKind::Chunk => "chunk",
        ObjectKind::FileManifest => "file_manifest",
        ObjectKind::Tree => "tree",
        ObjectKind::Conflict => "conflict",
        ObjectKind::Symlink => "symlink",
        ObjectKind::Tombstone => "tombstone",
        ObjectKind::Evidence => "evidence",
        ObjectKind::Product => "product",
        ObjectKind::PolicySet => "policy_set",
        ObjectKind::AuthorizationSnapshot => "authorization_snapshot",
    }
}

pub(crate) fn compression(value: Compression) -> &'static str {
    match value {
        Compression::Identity => "identity",
        Compression::Zstd => "zstd",
    }
}

pub(crate) fn i64_from_u64(value: u64) -> V3StoreResult<i64> {
    i64::try_from(value).map_err(|_| V3StoreError::InvalidCapability)
}

fn token_digest(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

fn verify_stored_descriptor(
    row: &sqlx::postgres::PgRow,
    descriptor: &ObjectDescriptor,
) -> V3StoreResult<()> {
    let canonical = i64_from_u64(descriptor.canonical_byte_len)?;
    let encoded = i64_from_u64(descriptor.encoded_byte_len)?;
    if row.get::<String, _>("object_kind") != object_kind(descriptor.kind)
        || row.get::<i64, _>("canonical_byte_length") != canonical
        || row.get::<i64, _>("encoded_byte_length") != encoded
        || row.get::<String, _>("compression") != compression(descriptor.compression)
    {
        return Err(V3StoreError::InvalidCapability);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_digests_do_not_store_bearer_secret() {
        let digest = token_digest("bearer-secret");
        assert_ne!(digest, "bearer-secret");
        assert_eq!(digest, token_digest("bearer-secret"));
    }
}
