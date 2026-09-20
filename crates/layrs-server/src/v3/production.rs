//! Real V3 adapter over PostgreSQL, native CAS, policy and authenticated identity.
use super::{
    AuthenticatedPrincipal, ByteRange, DownloadResult, RequestedRange, StoredTransactionReceipt,
    UploadDisposition, UploadResult, UploadStream, V3Facade, V3ServiceError, resolve_range,
};
use crate::identity::BearerVerifier;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures_util::StreamExt;
use layrs_core::scm::SpaceId;
use layrs_store_server::v3 as store;
use layrs_sync::v3::*;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use store::RootedFetchAvailability;

pub struct ProductionV3Facade {
    repository: store::PgV3Repository,
    identity: Arc<dyn BearerVerifier>,
    cursors: store::EventCursorCodec,
    transfer_base_url: store::TransferBaseUrl,
    plan_lifetime_seconds: u64,
}

impl ProductionV3Facade {
    pub fn new(
        repository: store::PgV3Repository,
        identity: Arc<dyn BearerVerifier>,
        cursors: store::EventCursorCodec,
        transfer_base_url: store::TransferBaseUrl,
    ) -> Self {
        Self {
            repository,
            identity,
            cursors,
            transfer_base_url,
            plan_lifetime_seconds: 900,
        }
    }
    /// Shorten signed transfer grants without changing the maximum lifetime.
    /// Already persisted plans keep their original immutable expiry.
    pub fn with_plan_lifetime_seconds(mut self, seconds: u64) -> Result<Self, V3ServiceError> {
        if !(1..=900).contains(&seconds) {
            return Err(V3ServiceError::InvalidRequest {
                code: "invalid_plan_lifetime",
            });
        }
        self.plan_lifetime_seconds = seconds;
        Ok(self)
    }
    async fn principal(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<store::PrincipalContext, V3ServiceError> {
        store::load_principal_context(self.repository.pool(), &principal.principal_id)
            .await
            .map_err(map_error)
    }
    async fn authority(&self, space: &SpaceId) -> Result<store::SpaceAuthority, V3ServiceError> {
        store::PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load_authority(space)
            .await
            .map_err(map_error)
    }
    async fn context(
        &self,
        principal: &AuthenticatedPrincipal,
        space: &SpaceId,
        client: &ClientId,
        request: &RequestId,
        fetch: bool,
    ) -> Result<store::PlanContext, V3ServiceError> {
        let authority = self.authority(space).await?;
        let now = now()?;
        let query = if fetch {
            "SELECT principal_id,authorization_epoch,extract(epoch FROM expires_at)::BIGINT AS expiry FROM fetch_plans WHERE space_id=$1 AND client_id=$2 AND request_id=$3"
        } else {
            "SELECT principal_id,authorization_epoch,extract(epoch FROM expires_at)::BIGINT AS expiry FROM object_plans WHERE space_id=$1 AND client_id=$2 AND request_id=$3"
        };
        let row = sqlx::query(query)
            .bind(space.as_str())
            .bind(client.as_str())
            .bind(request.as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| V3ServiceError::Unavailable)?;
        let expiry = if let Some(row) = row {
            if row.get::<String, _>("principal_id") != principal.principal_id.as_str() {
                return Err(V3ServiceError::Forbidden);
            }
            if row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64 {
                return Err(V3ServiceError::AuthorizationEpochChanged);
            }
            let expiry =
                u64::try_from(row.get::<i64, _>("expiry")).map_err(|_| V3ServiceError::Internal)?;
            if expiry <= now {
                return Err(V3ServiceError::TransferUnavailable);
            }
            expiry
        } else {
            now + self.plan_lifetime_seconds
        };
        Ok(store::PlanContext {
            actor: store::CapabilityActor {
                principal_id: principal.principal_id.clone(),
                authorization_epoch: authority.authorization_epoch,
                space_id: space.clone(),
                client_id: client.clone(),
                request_id: request.clone(),
                plan_id: store::deterministic_plan_id(space, client, request).map_err(map_error)?,
            },
            now_unix_seconds: now,
            expires_at_unix_seconds: expiry,
            transfer_base_url: self.transfer_base_url.clone(),
        })
    }
    async fn transfer_authority(
        &self,
        actor: &store::CapabilityActor,
        action: store::V3Action,
    ) -> Result<(store::PrincipalContext, store::SpaceAuthority), V3ServiceError> {
        let principal = store::load_principal_context(self.repository.pool(), &actor.principal_id)
            .await
            .map_err(map_error)?;
        let authority = self.authority(&actor.space_id).await?;
        if authority.authorization_epoch != actor.authorization_epoch {
            return Err(V3ServiceError::AuthorizationEpochChanged);
        }
        let decision = self
            .repository
            .authorizer()
            .authorize(&store::AuthorizationCheck {
                principal: principal.clone(),
                action,
                target: store::AuthorizationTarget::Space(actor.space_id.clone()),
                policy_set_id: authority.policy_set_id.clone(),
                authorization_epoch: authority.authorization_epoch,
            })
            .await
            .map_err(map_error)?;
        if !decision.allowed {
            return Err(V3ServiceError::Forbidden);
        }
        Ok((principal, authority))
    }
}

#[async_trait]
impl V3Facade for ProductionV3Facade {
    async fn step_history(
        &self,
        principal: &AuthenticatedPrincipal,
        space: &SpaceId,
        request: store::StepHistoryRequest,
    ) -> Result<store::StepHistoryResponse, V3ServiceError> {
        let principal = self.principal(principal).await?;
        let authority = self.authority(space).await?;
        self.repository
            .step_history(&principal, &authority, request)
            .await
            .map_err(map_error)
    }
    async fn compare_steps(
        &self,
        principal: &AuthenticatedPrincipal,
        space: &SpaceId,
        request: store::CompareStepsRequest,
    ) -> Result<store::CompareStepsResponse, V3ServiceError> {
        let principal = self.principal(principal).await?;
        let authority = self.authority(space).await?;
        self.repository
            .compare_steps(&principal, &authority, request)
            .await
            .map_err(map_error)
    }
    async fn provision_space(
        &self,
        principal: &AuthenticatedPrincipal,
        request: store::ProvisionSpaceRequest,
    ) -> Result<store::ProvisionSpaceResponse, V3ServiceError> {
        self.repository
            .provision_space(&self.principal(principal).await?, &request)
            .await
            .map_err(map_error)
    }
    async fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<AuthenticatedPrincipal, V3ServiceError> {
        self.identity
            .verify_bearer(token)
            .await
            .map(|principal_id| AuthenticatedPrincipal { principal_id })
    }
    async fn bootstrap(
        &self,
        principal: &AuthenticatedPrincipal,
        space: &SpaceId,
    ) -> Result<SpaceBootstrap, V3ServiceError> {
        store::PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load(&self.principal(principal).await?, space)
            .await
            .map_err(map_error)
    }
    async fn object_plan(
        &self,
        principal: &AuthenticatedPrincipal,
        request: ObjectPlanRequest,
    ) -> Result<ObjectPlanResponse, V3ServiceError> {
        request
            .validate_v3()
            .map_err(|e| V3ServiceError::InvalidRequest { code: e.code })?;
        let context = self
            .context(
                principal,
                &request.space_id,
                &request.client_id,
                &request.request_id,
                false,
            )
            .await?;
        let plans = store::PgPlanStore::new(self.repository.clone(), store::PlanLimits::default());
        let plan = plans
            .create_object_plan(&self.principal(principal).await?, &context, request.objects)
            .await
            .map_err(map_error)?;
        plans
            .object_plan_response(&context, &plan)
            .map_err(map_error)
    }
    async fn fetch_plan(
        &self,
        principal: &AuthenticatedPrincipal,
        request: FetchPlanRequest,
    ) -> Result<FetchPlanResponse, V3ServiceError> {
        request
            .validate_v3()
            .map_err(|e| V3ServiceError::InvalidRequest { code: e.code })?;
        let context = self
            .context(
                principal,
                &request.space_id,
                &request.client_id,
                &request.request_id,
                true,
            )
            .await?;
        let (principal, authority) = self
            .transfer_authority(&context.actor, store::V3Action::PlanFetch)
            .await?;
        let resolver = store::PgRootedFetchResolver::new(
            self.repository.clone(),
            principal.clone(),
            authority.policy_set_id,
            authority.authorization_epoch,
        );
        let claims = request
            .claims
            .into_iter()
            .map(from_wire_claim)
            .collect::<Vec<_>>();
        let (snapshot, plan) = store::build_fetch_snapshot_page(
            &resolver,
            self.repository.capabilities(),
            store::PlanLimits::default(),
            &context,
            claims.clone(),
            request.page_cursor.as_ref().map(|v| v.as_str()),
            request.max_objects as usize,
        )
        .await
        .map_err(map_error)?;
        let plans = store::PgPlanStore::new(self.repository.clone(), store::PlanLimits::default());
        plans
            .persist_fetch_plan(&principal, &context, &claims, &snapshot)
            .await
            .map_err(map_error)?;
        plans
            .fetch_plan_response(&context, &plan)
            .map_err(map_error)
    }
    async fn transact(
        &self,
        principal: &AuthenticatedPrincipal,
        request: SyncTransactionRequest,
    ) -> Result<StoredTransactionReceipt, V3ServiceError> {
        let principal = self.principal(principal).await?;
        let receipt = store::PgTransactionStore::new(self.repository.clone(), self.cursors.clone())
            .transact(&principal, &request)
            .await
            .map_err(|e| match e {
                store::TransactionServiceError::Store(e) => map_error(e),
                store::TransactionServiceError::RequestDigestMismatch => V3ServiceError::Conflict {
                    code: "request_digest_mismatch",
                },
                store::TransactionServiceError::IdempotencyDigestMismatch => {
                    V3ServiceError::IdempotencyDigestMismatch
                }
                store::TransactionServiceError::Busy => V3ServiceError::Conflict {
                    code: "view_generation_conflict",
                },
            })?;
        StoredTransactionReceipt::new(receipt.receipt, Bytes::from(receipt.canonical_jcs))
    }
    async fn events(
        &self,
        principal: &AuthenticatedPrincipal,
        request: SpaceEventPageRequest,
    ) -> Result<SpaceEventPage, V3ServiceError> {
        request
            .validate_v3()
            .map_err(|e| V3ServiceError::InvalidRequest { code: e.code })?;
        store::PgEventStore::new(self.repository.clone(), self.cursors.clone())
            .page(
                &self.principal(principal).await?,
                &request.space_id,
                &request.cursor,
                request.max_events,
            )
            .await
            .map_err(map_error)
    }
    async fn upload(
        &self,
        token: &str,
        length: u64,
        body: UploadStream,
    ) -> Result<UploadResult, V3ServiceError> {
        let grant = self
            .repository
            .capabilities()
            .authenticate_upload(token, now()?)
            .map_err(map_error)?;
        if grant.descriptor.encoded_byte_len != length {
            return Err(V3ServiceError::LengthMismatch);
        }
        let (principal, _) = self
            .transfer_authority(&grant.actor, store::V3Action::CompleteUpload)
            .await?;
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upload_capabilities c JOIN object_plans p ON p.plan_id=c.plan_id WHERE c.token_digest=$1 AND c.plan_id=$2 AND c.object_id=$3 AND p.plan_state='finalized' AND c.expires_at>clock_timestamp() AND p.expires_at>clock_timestamp())")
            .bind(token_digest(token)).bind(grant.actor.plan_id.as_str()).bind(grant.descriptor.object_id.as_str()).fetch_one(self.repository.pool()).await.map_err(|_|V3ServiceError::Unavailable)?;
        if !valid {
            return Err(V3ServiceError::TransferUnavailable);
        }
        let body = Box::pin(body.map(|result| {
            result.map_err(|_| store::V3StoreError::Backend("upload body failed".into()))
        }));
        let outcome = self
            .repository
            .cas()
            .put_if_absent(&grant.descriptor, body)
            .await
            .map_err(map_error)?;
        store::PgCatalogStore::new(self.repository.clone())
            .complete_upload(store::CompleteUploadRequest {
                principal,
                actor: grant.actor,
                descriptor: grant.descriptor,
                capability_token: token.into(),
                now_unix_seconds: now()?,
            })
            .await
            .map_err(map_error)?;
        Ok(UploadResult {
            disposition: match outcome {
                store::PutOutcome::Installed => UploadDisposition::Installed,
                store::PutOutcome::AlreadyPresent => UploadDisposition::AlreadyPresent,
            },
            encoded_len: length,
        })
    }
    async fn download(
        &self,
        token: &str,
        requested: Option<RequestedRange>,
    ) -> Result<DownloadResult, V3ServiceError> {
        let grant = self
            .repository
            .capabilities()
            .authenticate_download(token, now()?)
            .map_err(map_error)?;
        let (principal, authority) = self
            .transfer_authority(&grant.actor, store::V3Action::FetchObject)
            .await?;
        let row=sqlx::query("SELECT c.claim_canonical_jcs FROM fetch_capabilities cap JOIN fetch_plans p ON p.plan_id=cap.plan_id JOIN fetch_plan_claims c ON c.plan_id=cap.plan_id AND c.ordinal=cap.claim_ordinal WHERE cap.token_digest=$1 AND cap.plan_id=$2 AND cap.object_id=$3 AND cap.resume_token_digest=$4 AND p.plan_state='finalized' AND c.decision='allowed' AND cap.expires_at>clock_timestamp() AND p.expires_at>clock_timestamp()")
            .bind(token_digest(token)).bind(grant.actor.plan_id.as_str()).bind(grant.descriptor.object_id.as_str()).bind(token_digest(&grant.resume_token)).fetch_optional(self.repository.pool()).await.map_err(|_|V3ServiceError::Unavailable)?.ok_or(V3ServiceError::TransferUnavailable)?;
        let claim: store::FetchClaim =
            serde_json::from_str(&row.get::<String, _>("claim_canonical_jcs"))
                .map_err(|_| V3ServiceError::Internal)?;
        let resolver = store::PgRootedFetchResolver::new(
            self.repository.clone(),
            principal,
            authority.policy_set_id,
            authority.authorization_epoch,
        );
        let resolved = resolver
            .resolve(&grant.actor.space_id, &claim)
            .await
            .map_err(map_error)?;
        if !matches!(resolved,store::ClaimResolution::Available {descriptors,..} if descriptors.contains(&grant.descriptor))
        {
            return Err(V3ServiceError::TransferUnavailable);
        }
        let total = grant.descriptor.encoded_byte_len;
        let range = match requested {
            Some(requested) => {
                resolve_range(requested, total).ok_or(V3ServiceError::RangeNotSatisfiable {
                    total_encoded_len: total,
                })?
            }
            None => ByteRange {
                start: 0,
                end_exclusive: total,
            },
        };
        if range.start < grant.range.start || range.end_exclusive > grant.range.end_exclusive {
            return Err(V3ServiceError::RangeNotSatisfiable {
                total_encoded_len: total,
            });
        }
        let stream = self
            .repository
            .cas()
            .verified_range(
                &grant.descriptor,
                store::ByteRange::new(range.start, range.end_exclusive).map_err(map_error)?,
            )
            .await
            .map_err(map_error)?;
        Ok(DownloadResult {
            total_encoded_len: total,
            served_range: ByteRange {
                start: range.start,
                end_exclusive: range.end_exclusive,
            },
            content_type: "application/octet-stream",
            body: Box::pin(stream.map(|r| r.map_err(map_error))),
        })
    }
}

fn from_wire_claim(claim: FetchClaim) -> store::FetchClaim {
    match claim {
        FetchClaim::Object {
            object,
            root,
            repo_path,
        } => store::FetchClaim::Object {
            object,
            root,
            repo_path: repo_path.0,
        },
        FetchClaim::File {
            file_id,
            preferred_manifest_ids,
            root,
            repo_path,
        } => store::FetchClaim::File {
            file_id,
            preferred_manifest_ids,
            root,
            repo_path,
        },
        FetchClaim::AuthorizationSnapshot {
            authorization_snapshot_id,
            purpose,
        } => store::FetchClaim::AuthorizationSnapshot {
            authorization_snapshot_id,
            purpose: match purpose {
                AuthorizationSnapshotPurpose::Audit => store::AuditPurpose::Audit,
                AuthorizationSnapshotPurpose::Administration => store::AuditPurpose::Administration,
            },
        },
    }
}
fn now() -> Result<u64, V3ServiceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| V3ServiceError::Internal)
}
fn token_digest(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}
fn map_error(error: store::V3StoreError) -> V3ServiceError {
    match error {
        store::V3StoreError::AuthorizationEpochChanged => V3ServiceError::AuthorizationEpochChanged,
        store::V3StoreError::InvalidCapability => V3ServiceError::Forbidden,
        store::V3StoreError::ExpiredCapability
        | store::V3StoreError::CapabilityBindingMismatch
        | store::V3StoreError::NotFound => V3ServiceError::TransferUnavailable,
        store::V3StoreError::LimitExceeded { limit, .. } => {
            V3ServiceError::PayloadTooLarge { limit }
        }
        store::V3StoreError::LengthMismatch { .. } => V3ServiceError::LengthMismatch,
        store::V3StoreError::Backend(_) | store::V3StoreError::Io(_) => V3ServiceError::Unavailable,
        error => V3ServiceError::InvalidRequest { code: error.code() },
    }
}
