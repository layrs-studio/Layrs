mod authority;
mod wire;
use authority::revalidate_plan_authority;
use wire::*;

use layrs_core::scm::ObjectId;
use layrs_sync::v3::{
    Compression, DeniedClaim as WireDeniedClaim, DownloadCapability as WireDownloadCapability,
    DownloadMethod, FetchPlanResponse, FetchPlanResponseType,
    FileManifestSelection as WireFileManifestSelection, Nullable, ObjectDescriptor, ObjectKind,
    ObjectPlanResponse, ObjectPlanResponseType, SyncV3Protocol,
    UploadCapability as WireUploadCapability, UploadMethod,
};
use sqlx::Row;

use super::{
    AuthorizationCheck, AuthorizationTarget, CapabilityActor, DeniedCode, FetchClaim, FetchPlan,
    ObjectPlan, ObjectPlanBuilder, PgCatalogStore, PgV3Repository, PlanContext, PlanLimits,
    PrincipalContext, UploadCapability, V3Action, V3StoreError, V3StoreResult, compression,
    i64_from_u64, object_kind, pg_error, require_allowed,
};

#[derive(Clone)]
pub struct PgPlanStore {
    repository: PgV3Repository,
    catalog: PgCatalogStore,
    limits: PlanLimits,
}

impl PgPlanStore {
    pub fn new(repository: PgV3Repository, limits: PlanLimits) -> Self {
        let catalog = PgCatalogStore::new(repository.clone());
        Self {
            repository,
            catalog,
            limits,
        }
    }

    pub async fn create_object_plan(
        &self,
        principal: &PrincipalContext,
        context: &PlanContext,
        requested: Vec<ObjectDescriptor>,
    ) -> V3StoreResult<ObjectPlan> {
        self.create_object_plan_inner(principal, context, requested, None)
            .await
    }

    pub(crate) async fn create_scoped_object_plan(
        &self,
        principal: &PrincipalContext,
        context: &PlanContext,
        requested: Vec<ObjectDescriptor>,
        scope: &super::path_patch_scope::ScopedPatch,
        transaction: &layrs_sync::v3::SyncTransactionRequest,
    ) -> V3StoreResult<ObjectPlan> {
        self.create_object_plan_inner(principal, context, requested, Some((scope, transaction)))
            .await
    }

    async fn create_object_plan_inner(
        &self,
        principal: &PrincipalContext,
        context: &PlanContext,
        requested: Vec<ObjectDescriptor>,
        scope: Option<(
            &super::path_patch_scope::ScopedPatch,
            &layrs_sync::v3::SyncTransactionRequest,
        )>,
    ) -> V3StoreResult<ObjectPlan> {
        if context.actor.plan_id
            != deterministic_plan_id(
                &context.actor.space_id,
                &context.actor.client_id,
                &context.actor.request_id,
            )?
        {
            return Err(V3StoreError::InvalidCapability);
        }
        let (policy_set_id, epoch) = self.space_authority(&context.actor).await?;
        if principal.principal_id != context.actor.principal_id
            || epoch != context.actor.authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        if let Some((scope, transaction)) = scope {
            let authority = super::SpaceAuthority {
                space_id: context.actor.space_id.clone(),
                repository_view_id: scope.request.expected_repository_view_id.clone(),
                repository_view_generation: scope.request.expected_generation,
                policy_set_id: policy_set_id.clone(),
                authorization_epoch: epoch,
                watermark_sequence: 0,
            };
            if scope
                .decisions(&self.repository, principal, &authority, transaction, &[])
                .await?
                .iter()
                .any(|d| !d.allowed)
            {
                return Err(V3StoreError::InvalidCapability);
            }
        } else {
            require_allowed(
                self.repository
                    .authorizer()
                    .authorize(&AuthorizationCheck {
                        principal: principal.clone(),
                        action: V3Action::PlanUpload,
                        target: AuthorizationTarget::Space(context.actor.space_id.clone()),
                        policy_set_id,
                        authorization_epoch: epoch,
                    })
                    .await?,
            )?;
        }

        let request_digest =
            super::capability::digest_json("layrs.sync.v3/object-plan-request", &requested)?;
        // Catalogue reads use another pooled connection. Do this before taking
        // the writer lock so queued writers cannot exhaust the pool while its
        // owner waits for a second connection. Authority is rechecked below.
        let plan =
            ObjectPlanBuilder::new(&self.catalog, self.repository.capabilities(), self.limits)
                .build(context, requested)
                .await?;
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        let lock_key = format!(
            "object-plan/{}/{}/{}",
            context.actor.space_id, context.actor.client_id, context.actor.request_id
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(lock_key)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        revalidate_plan_authority(&mut tx, principal, context).await?;

        if let Some(row) = sqlx::query(
            r#"
            SELECT plan_id, request_descriptors_digest,
                   extract(epoch FROM expires_at)::BIGINT AS expires_at
            FROM object_plans
            WHERE space_id = $1 AND client_id = $2 AND request_id = $3
            "#,
        )
        .bind(context.actor.space_id.as_str())
        .bind(context.actor.client_id.as_str())
        .bind(context.actor.request_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_error)?
        {
            if row.get::<String, _>("request_descriptors_digest") != request_digest.as_str() {
                return Err(V3StoreError::InvalidCapability);
            }
            let plan_id = layrs_sync::v3::PlanId::parse(row.get::<String, _>("plan_id"))
                .map_err(|_| V3StoreError::CorruptObject("invalid stored PlanId".into()))?;
            let expires_at = u64::try_from(row.get::<i64, _>("expires_at"))
                .map_err(|_| V3StoreError::CorruptObject("invalid plan expiry".into()))?;
            let plan = self
                .load_object_plan(
                    CapabilityActor {
                        plan_id,
                        ..context.actor.clone()
                    },
                    expires_at,
                    context.transfer_base_url.as_str(),
                    &mut tx,
                )
                .await?;
            tx.commit().await.map_err(pg_error)?;
            return Ok(plan);
        }

        // Availability remains Space-scoped. A concurrently installed object
        // may still be reported missing; its upload is exact and idempotent.
        let descriptors: Vec<_> = plan
            .present_objects
            .iter()
            .chain(&plan.missing_objects)
            .cloned()
            .collect();
        self.repository
            .check_cas_budget(&mut tx, None, &descriptors)
            .await?;
        self.repository
            .check_cas_budget(&mut tx, Some(&context.actor.space_id), &descriptors)
            .await?;
        self.insert_object_plan(&mut tx, context, &request_digest, &plan)
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(plan)
    }

    pub fn object_plan_response(
        &self,
        context: &PlanContext,
        plan: &ObjectPlan,
    ) -> V3StoreResult<ObjectPlanResponse> {
        let expires_at = timestamp(context.expires_at_unix_seconds)?;
        let mut capabilities = Vec::with_capacity(plan.upload_capabilities.len());
        for capability in &plan.upload_capabilities {
            let descriptor_digest =
                super::capability::digest_json("layrs.sync.v3/descriptor", &capability.descriptor)?;
            capabilities.push(WireUploadCapability {
                principal_id: capability.actor.principal_id.clone(),
                authorization_epoch: capability.actor.authorization_epoch,
                space_id: capability.actor.space_id.clone(),
                client_id: capability.actor.client_id.clone(),
                request_id: capability.actor.request_id.clone(),
                plan_id: capability.actor.plan_id.clone(),
                descriptor: capability.descriptor.clone(),
                descriptor_digest,
                method: UploadMethod::Put,
                upload_url: capability.upload_url.clone(),
                token: capability.token.clone(),
                expires_at: expires_at.clone(),
                max_canonical_byte_len: capability.descriptor.canonical_byte_len,
                max_encoded_byte_len: capability.descriptor.encoded_byte_len,
                compression: capability.descriptor.compression,
            });
        }
        Ok(ObjectPlanResponse {
            protocol: SyncV3Protocol::V3,
            message_type: ObjectPlanResponseType::Response,
            principal_id: context.actor.principal_id.clone(),
            authorization_epoch: context.actor.authorization_epoch,
            space_id: context.actor.space_id.clone(),
            client_id: context.actor.client_id.clone(),
            request_id: context.actor.request_id.clone(),
            plan_id: context.actor.plan_id.clone(),
            present_objects: plan.present_objects.clone(),
            missing_objects: plan.missing_objects.clone(),
            upload_capabilities: capabilities,
            expires_at,
        })
    }

    /// Persists a fully resolved fetch page as an immutable exact capability
    /// set. Resolution remains a separate policy/rooted-graph service, keeping
    /// PostgreSQL persistence reusable by the transaction engine and HTTP.
    pub async fn persist_fetch_plan(
        &self,
        principal: &PrincipalContext,
        context: &PlanContext,
        request_claims: &[FetchClaim],
        plan: &FetchPlan,
    ) -> V3StoreResult<()> {
        if context.actor.plan_id
            != deterministic_plan_id(
                &context.actor.space_id,
                &context.actor.client_id,
                &context.actor.request_id,
            )?
        {
            return Err(V3StoreError::InvalidCapability);
        }
        let (policy_set_id, epoch) = self.space_authority(&context.actor).await?;
        if principal.principal_id != context.actor.principal_id
            || epoch != context.actor.authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::PlanFetch,
                    target: AuthorizationTarget::Space(context.actor.space_id.clone()),
                    policy_set_id,
                    authorization_epoch: epoch,
                })
                .await?,
        )?;
        let request_digest =
            super::capability::digest_json("layrs.sync.v3/fetch-claims", &request_claims)?;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        let lock_key = format!(
            "fetch-plan/{}/{}/{}",
            context.actor.space_id, context.actor.client_id, context.actor.request_id
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(lock_key)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        revalidate_plan_authority(&mut tx, principal, context).await?;
        if let Some(row) = sqlx::query(
            r#"
            SELECT plan_id, request_claims_digest, authorization_epoch,
                   extract(epoch FROM expires_at)::BIGINT AS expires_at
            FROM fetch_plans
            WHERE space_id=$1 AND client_id=$2 AND request_id=$3
            "#,
        )
        .bind(context.actor.space_id.as_str())
        .bind(context.actor.client_id.as_str())
        .bind(context.actor.request_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_error)?
        {
            let exact = row.get::<String, _>("plan_id") == context.actor.plan_id.as_str()
                && row.get::<String, _>("request_claims_digest") == request_digest.as_str()
                && row.get::<i64, _>("authorization_epoch")
                    == i64_from_u64(context.actor.authorization_epoch)?
                && row.get::<i64, _>("expires_at")
                    == i64_from_u64(context.expires_at_unix_seconds)?;
            if !exact {
                return Err(V3StoreError::InvalidCapability);
            }
            self.verify_persisted_fetch_plan(&mut tx, context, request_claims, plan)
                .await?;
            tx.commit().await.map_err(pg_error)?;
            return Ok(());
        }
        self.insert_fetch_plan(&mut tx, context, &request_digest, request_claims, plan)
            .await?;
        tx.commit().await.map_err(pg_error)
    }

    pub fn fetch_plan_response(
        &self,
        context: &PlanContext,
        plan: &FetchPlan,
    ) -> V3StoreResult<FetchPlanResponse> {
        let expires_at = timestamp(context.expires_at_unix_seconds)?;
        Ok(FetchPlanResponse {
            protocol: SyncV3Protocol::V3,
            message_type: FetchPlanResponseType::Response,
            principal_id: context.actor.principal_id.clone(),
            authorization_epoch: context.actor.authorization_epoch,
            space_id: context.actor.space_id.clone(),
            client_id: context.actor.client_id.clone(),
            request_id: context.actor.request_id.clone(),
            plan_id: context.actor.plan_id.clone(),
            file_manifest_selections: plan
                .file_manifest_selections
                .iter()
                .map(|selection| WireFileManifestSelection {
                    file_id: selection.file_id.clone(),
                    file_manifest_id: selection.file_manifest_id.clone(),
                    manifest_descriptor: selection.manifest_descriptor.clone(),
                    chunk_descriptors: selection.chunk_descriptors.clone(),
                })
                .collect(),
            download_capabilities: plan
                .download_capabilities
                .iter()
                .map(|capability| WireDownloadCapability {
                    principal_id: capability.actor.principal_id.clone(),
                    authorization_epoch: capability.actor.authorization_epoch,
                    space_id: capability.actor.space_id.clone(),
                    client_id: capability.actor.client_id.clone(),
                    request_id: capability.actor.request_id.clone(),
                    plan_id: capability.actor.plan_id.clone(),
                    claim: to_wire_claim(&capability.claim),
                    descriptor: capability.descriptor.clone(),
                    claim_digest: capability.claim_digest.clone(),
                    method: DownloadMethod::Get,
                    download_url: capability.download_url.clone(),
                    token: capability.token.clone(),
                    expires_at: expires_at.clone(),
                    resume_token: capability.resume_token.clone(),
                    accept_ranges: true,
                })
                .collect(),
            denied_claims: plan
                .denied_claims
                .iter()
                .map(|denied| WireDeniedClaim {
                    claim: to_wire_claim(&denied.claim),
                    code: to_wire_denied_code(denied.code),
                })
                .collect(),
            next_page_cursor: match &plan.next_page_cursor {
                Some(cursor) => Nullable::some(
                    layrs_sync::v3::Cursor::parse(cursor.clone())
                        .map_err(|_| V3StoreError::InvalidCursor)?,
                ),
                None => Nullable::null(),
            },
            has_more: plan.has_more,
            expires_at,
        })
    }

    async fn space_authority(&self, actor: &CapabilityActor) -> V3StoreResult<(ObjectId, u64)> {
        let row = sqlx::query(
            r#"
            SELECT current_policy_set_id, authorization_epoch
            FROM spaces
            WHERE space_id = $1 AND lifecycle_state = 'ready'
            "#,
        )
        .bind(actor.space_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(pg_error)?
        .ok_or(V3StoreError::InvalidCapability)?;
        let policy = ObjectId::parse(row.get::<String, _>("current_policy_set_id"))
            .map_err(|_| V3StoreError::CorruptObject("invalid PolicySet ID".into()))?;
        let epoch = u64::try_from(row.get::<i64, _>("authorization_epoch"))
            .map_err(|_| V3StoreError::CorruptObject("invalid authorization epoch".into()))?;
        Ok((policy, epoch))
    }

    async fn insert_object_plan(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        context: &PlanContext,
        request_digest: &ObjectId,
        plan: &ObjectPlan,
    ) -> V3StoreResult<()> {
        sqlx::query(
            r#"
            INSERT INTO object_plans (
                plan_id, request_id, space_id, client_id, principal_id,
                authorization_epoch, request_descriptors_digest, expires_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, to_timestamp($8))
            "#,
        )
        .bind(context.actor.plan_id.as_str())
        .bind(context.actor.request_id.as_str())
        .bind(context.actor.space_id.as_str())
        .bind(context.actor.client_id.as_str())
        .bind(context.actor.principal_id.as_str())
        .bind(i64_from_u64(context.actor.authorization_epoch)?)
        .bind(request_digest.as_str())
        .bind(i64_from_u64(context.expires_at_unix_seconds)?)
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;

        let mut all = Vec::with_capacity(plan.present_objects.len() + plan.missing_objects.len());
        all.extend(plan.present_objects.iter().cloned().map(|d| (d, "present")));
        all.extend(plan.missing_objects.iter().cloned().map(|d| (d, "missing")));
        all.sort_by(|left, right| left.0.cmp(&right.0));
        for (ordinal, (descriptor, disposition)) in all.iter().enumerate() {
            let digest = super::capability::digest_json("layrs.sync.v3/descriptor", descriptor)?;
            sqlx::query(
                r#"
                INSERT INTO object_plan_items (
                    plan_id, ordinal, object_id, object_kind,
                    canonical_byte_length, encoded_byte_length, compression,
                    descriptor_digest, disposition
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
                "#,
            )
            .bind(context.actor.plan_id.as_str())
            .bind(i32::try_from(ordinal).map_err(|_| V3StoreError::InvalidCapability)?)
            .bind(descriptor.object_id.as_str())
            .bind(object_kind(descriptor.kind))
            .bind(i64_from_u64(descriptor.canonical_byte_len)?)
            .bind(i64_from_u64(descriptor.encoded_byte_len)?)
            .bind(compression(descriptor.compression))
            .bind(digest.as_str())
            .bind(*disposition)
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;
            if *disposition == "missing" {
                let capability = plan
                    .upload_capabilities
                    .iter()
                    .find(|candidate| candidate.descriptor == *descriptor)
                    .ok_or_else(|| {
                        V3StoreError::CorruptObject("missing capability in plan".into())
                    })?;
                sqlx::query(
                    r#"
                    INSERT INTO upload_capabilities (
                        capability_id, plan_id, item_ordinal, object_id,
                        descriptor_digest, space_id, client_id, request_id,
                        principal_id, authorization_epoch, token_digest,
                        method, expires_at
                    ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'PUT',to_timestamp($12))
                    "#,
                )
                .bind(capability_id("upload", &context.actor.plan_id, ordinal))
                .bind(context.actor.plan_id.as_str())
                .bind(i32::try_from(ordinal).map_err(|_| V3StoreError::InvalidCapability)?)
                .bind(descriptor.object_id.as_str())
                .bind(digest.as_str())
                .bind(context.actor.space_id.as_str())
                .bind(context.actor.client_id.as_str())
                .bind(context.actor.request_id.as_str())
                .bind(context.actor.principal_id.as_str())
                .bind(i64_from_u64(context.actor.authorization_epoch)?)
                .bind(token_digest(&capability.token))
                .bind(i64_from_u64(context.expires_at_unix_seconds)?)
                .execute(&mut **tx)
                .await
                .map_err(pg_error)?;
            }
        }
        sqlx::query(
            "UPDATE object_plans SET plan_state='finalized', finalized_at=transaction_timestamp() WHERE plan_id=$1 AND plan_state='open'",
        )
        .bind(context.actor.plan_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;
        Ok(())
    }

    async fn load_object_plan(
        &self,
        actor: CapabilityActor,
        expires_at: u64,
        transfer_base_url: &str,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> V3StoreResult<ObjectPlan> {
        let rows = sqlx::query(
            r#"
            SELECT i.object_id, i.object_kind, i.canonical_byte_length,
                   i.encoded_byte_length, i.compression, i.disposition,
                   c.token_digest, extract(epoch FROM c.expires_at)::BIGINT AS capability_expiry
            FROM object_plan_items i
            LEFT JOIN upload_capabilities c
              ON c.plan_id=i.plan_id AND c.item_ordinal=i.ordinal
            WHERE i.plan_id = $1
            ORDER BY i.ordinal
            "#,
        )
        .bind(actor.plan_id.as_str())
        .fetch_all(&mut **tx)
        .await
        .map_err(pg_error)?;
        let mut present_objects = Vec::new();
        let mut missing_objects = Vec::new();
        let mut upload_capabilities = Vec::new();
        for row in rows {
            let descriptor = descriptor_from_row(&row)?;
            match row.get::<String, _>("disposition").as_str() {
                "present" => present_objects.push(descriptor),
                "missing" => {
                    let token = self.repository.capabilities().issue_upload(
                        actor.clone(),
                        descriptor.clone(),
                        expires_at,
                    )?;
                    let stored_digest =
                        row.get::<Option<String>, _>("token_digest")
                            .ok_or_else(|| {
                                V3StoreError::CorruptObject(
                                    "missing persisted upload capability".into(),
                                )
                            })?;
                    if stored_digest != token_digest(&token)
                        || row.get::<Option<i64>, _>("capability_expiry")
                            != Some(i64_from_u64(expires_at)?)
                    {
                        return Err(V3StoreError::CorruptObject(
                            "persisted upload capability mismatch".into(),
                        ));
                    }
                    upload_capabilities.push(UploadCapability {
                        actor: actor.clone(),
                        descriptor: descriptor.clone(),
                        method: "PUT",
                        upload_url: format!("{transfer_base_url}/v3/transfers/uploads"),
                        token,
                        expires_at_unix_seconds: expires_at,
                        full_object_only: true,
                    });
                    missing_objects.push(descriptor);
                }
                _ => return Err(V3StoreError::CorruptObject("invalid disposition".into())),
            }
        }
        present_objects.sort();
        missing_objects.sort();
        upload_capabilities.sort_by(|a, b| a.descriptor.cmp(&b.descriptor));
        Ok(ObjectPlan {
            present_objects,
            missing_objects,
            upload_capabilities,
        })
    }

    async fn insert_fetch_plan(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        context: &PlanContext,
        request_digest: &ObjectId,
        request_claims: &[FetchClaim],
        plan: &FetchPlan,
    ) -> V3StoreResult<()> {
        let processed = processed_claims(request_claims, plan)?;
        let last_scanned = processed
            .last()
            .map(|value| value.0)
            .ok_or_else(|| V3StoreError::InvalidDescriptor("empty fetch page".into()))?;
        sqlx::query(
            r#"
            INSERT INTO fetch_plans (
                plan_id, request_id, space_id, client_id, principal_id,
                authorization_epoch, request_claims_digest, last_scanned_claim, expires_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,to_timestamp($9))
            "#,
        )
        .bind(context.actor.plan_id.as_str())
        .bind(context.actor.request_id.as_str())
        .bind(context.actor.space_id.as_str())
        .bind(context.actor.client_id.as_str())
        .bind(context.actor.principal_id.as_str())
        .bind(i64_from_u64(context.actor.authorization_epoch)?)
        .bind(request_digest.as_str())
        .bind(i32::try_from(last_scanned).map_err(|_| V3StoreError::InvalidCapability)?)
        .bind(i64_from_u64(context.expires_at_unix_seconds)?)
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;

        for (claim_ordinal, claim, decision) in processed {
            let claim_jcs = serde_jcs::to_string(claim)
                .map_err(|error| V3StoreError::Backend(error.to_string()))?;
            let (decision_name, denied_code) = match decision {
                ProcessedDecision::Allowed => ("allowed", None),
                ProcessedDecision::Denied(code) => ("denied", Some(denied_code(code))),
            };
            let selected_manifest = plan
                .file_manifest_selections
                .iter()
                .find(|selection| match claim {
                    FetchClaim::File { file_id, .. } => selection.file_id == *file_id,
                    _ => false,
                })
                .map(|selection| selection.file_manifest_id.as_str());
            sqlx::query(
                r#"
                INSERT INTO fetch_plan_claims (
                    plan_id, ordinal, claim_kind, claim_canonical_jcs,
                    decision, denied_code, selected_file_manifest_id
                ) VALUES ($1,$2,$3,$4,$5,$6,$7)
                "#,
            )
            .bind(context.actor.plan_id.as_str())
            .bind(i32::try_from(claim_ordinal).map_err(|_| V3StoreError::InvalidCapability)?)
            .bind(claim_kind(claim))
            .bind(claim_jcs)
            .bind(decision_name)
            .bind(denied_code)
            .bind(selected_manifest)
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;

            if decision == ProcessedDecision::Allowed {
                let capabilities = plan
                    .download_capabilities
                    .iter()
                    .filter(|capability| capability.claim == *claim)
                    .collect::<Vec<_>>();
                if capabilities.is_empty() {
                    return Err(V3StoreError::InvalidDescriptor(
                        "allowed fetch claim has no capability".into(),
                    ));
                }
                for (object_ordinal, capability) in capabilities.into_iter().enumerate() {
                    let descriptor_digest = super::capability::digest_json(
                        "layrs.sync.v3/descriptor",
                        &capability.descriptor,
                    )?;
                    sqlx::query(
                        r#"
                        INSERT INTO fetch_claim_objects (
                            plan_id, claim_ordinal, object_ordinal, object_id, descriptor_digest
                        ) VALUES ($1,$2,$3,$4,$5)
                        "#,
                    )
                    .bind(context.actor.plan_id.as_str())
                    .bind(
                        i32::try_from(claim_ordinal)
                            .map_err(|_| V3StoreError::InvalidCapability)?,
                    )
                    .bind(
                        i32::try_from(object_ordinal)
                            .map_err(|_| V3StoreError::InvalidCapability)?,
                    )
                    .bind(capability.descriptor.object_id.as_str())
                    .bind(descriptor_digest.as_str())
                    .execute(&mut **tx)
                    .await
                    .map_err(pg_error)?;
                    sqlx::query(
                        r#"
                        INSERT INTO fetch_capabilities (
                            capability_id, plan_id, claim_ordinal, object_id,
                            descriptor_digest, space_id, client_id, request_id,
                            principal_id, authorization_epoch, token_digest,
                            resume_token_digest, method, expires_at
                        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'GET',to_timestamp($13))
                        "#,
                    )
                    .bind(capability_id(
                        "fetch",
                        &context.actor.plan_id,
                        claim_ordinal.saturating_mul(10_001) + object_ordinal,
                    ))
                    .bind(context.actor.plan_id.as_str())
                    .bind(
                        i32::try_from(claim_ordinal)
                            .map_err(|_| V3StoreError::InvalidCapability)?,
                    )
                    .bind(capability.descriptor.object_id.as_str())
                    .bind(descriptor_digest.as_str())
                    .bind(context.actor.space_id.as_str())
                    .bind(context.actor.client_id.as_str())
                    .bind(context.actor.request_id.as_str())
                    .bind(context.actor.principal_id.as_str())
                    .bind(i64_from_u64(context.actor.authorization_epoch)?)
                    .bind(token_digest(&capability.token))
                    .bind(token_digest(&capability.resume_token))
                    .bind(i64_from_u64(context.expires_at_unix_seconds)?)
                    .execute(&mut **tx)
                    .await
                    .map_err(pg_error)?;
                }
            }
        }
        sqlx::query(
            "UPDATE fetch_plans SET plan_state='finalized', finalized_at=transaction_timestamp() WHERE plan_id=$1 AND plan_state='open'",
        )
        .bind(context.actor.plan_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;
        Ok(())
    }

    async fn verify_persisted_fetch_plan(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        context: &PlanContext,
        request_claims: &[FetchClaim],
        plan: &FetchPlan,
    ) -> V3StoreResult<()> {
        let processed = processed_claims(request_claims, plan)?;
        let expected_objects = plan.download_capabilities.len();
        let counts = sqlx::query(
            r#"
            SELECT
              (SELECT count(*) FROM fetch_plan_claims WHERE plan_id=$1) AS claim_count,
              (SELECT count(*) FROM fetch_claim_objects WHERE plan_id=$1) AS object_count,
              (SELECT count(*) FROM fetch_capabilities WHERE plan_id=$1) AS capability_count
            "#,
        )
        .bind(context.actor.plan_id.as_str())
        .fetch_one(&mut **tx)
        .await
        .map_err(pg_error)?;
        if counts.get::<i64, _>("claim_count")
            != i64::try_from(processed.len()).map_err(|_| V3StoreError::InvalidCapability)?
            || counts.get::<i64, _>("object_count")
                != i64::try_from(expected_objects).map_err(|_| V3StoreError::InvalidCapability)?
            || counts.get::<i64, _>("capability_count")
                != i64::try_from(expected_objects).map_err(|_| V3StoreError::InvalidCapability)?
        {
            return Err(V3StoreError::CorruptObject(
                "persisted fetch plan cardinality mismatch".into(),
            ));
        }
        for (claim_ordinal, claim, decision) in processed {
            let row = sqlx::query(
                r#"
                SELECT claim_canonical_jcs, decision, denied_code, selected_file_manifest_id
                FROM fetch_plan_claims WHERE plan_id=$1 AND ordinal=$2
                "#,
            )
            .bind(context.actor.plan_id.as_str())
            .bind(i32::try_from(claim_ordinal).map_err(|_| V3StoreError::InvalidCapability)?)
            .fetch_optional(&mut **tx)
            .await
            .map_err(pg_error)?
            .ok_or_else(|| V3StoreError::CorruptObject("missing fetch claim".into()))?;
            let expected_jcs = serde_jcs::to_string(claim)
                .map_err(|error| V3StoreError::Backend(error.to_string()))?;
            let expected_decision = match decision {
                ProcessedDecision::Allowed => "allowed",
                ProcessedDecision::Denied(_) => "denied",
            };
            let expected_denied = match decision {
                ProcessedDecision::Allowed => None,
                ProcessedDecision::Denied(code) => Some(denied_code(code)),
            };
            let expected_manifest = plan
                .file_manifest_selections
                .iter()
                .find(|selection| match claim {
                    FetchClaim::File { file_id, .. } => selection.file_id == *file_id,
                    _ => false,
                })
                .map(|selection| selection.file_manifest_id.as_str());
            if row.get::<String, _>("claim_canonical_jcs") != expected_jcs
                || row.get::<String, _>("decision") != expected_decision
                || row.get::<Option<String>, _>("denied_code").as_deref() != expected_denied
                || row
                    .get::<Option<String>, _>("selected_file_manifest_id")
                    .as_deref()
                    != expected_manifest
            {
                return Err(V3StoreError::CorruptObject(
                    "persisted fetch claim mismatch".into(),
                ));
            }
            for capability in plan
                .download_capabilities
                .iter()
                .filter(|capability| capability.claim == *claim)
            {
                let digest = super::capability::digest_json(
                    "layrs.sync.v3/descriptor",
                    &capability.descriptor,
                )?;
                let matches: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1 FROM fetch_capabilities
                        WHERE plan_id=$1 AND claim_ordinal=$2 AND object_id=$3
                          AND descriptor_digest=$4 AND token_digest=$5
                          AND resume_token_digest=$6
                          AND expires_at=to_timestamp($7)
                    )
                    "#,
                )
                .bind(context.actor.plan_id.as_str())
                .bind(i32::try_from(claim_ordinal).map_err(|_| V3StoreError::InvalidCapability)?)
                .bind(capability.descriptor.object_id.as_str())
                .bind(digest.as_str())
                .bind(token_digest(&capability.token))
                .bind(token_digest(&capability.resume_token))
                .bind(i64_from_u64(context.expires_at_unix_seconds)?)
                .fetch_one(&mut **tx)
                .await
                .map_err(pg_error)?;
                if !matches {
                    return Err(V3StoreError::CorruptObject(
                        "persisted fetch capability mismatch".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn deterministic_plan_id(
    space_id: &layrs_core::scm::SpaceId,
    client_id: &layrs_sync::v3::ClientId,
    request_id: &layrs_sync::v3::RequestId,
) -> V3StoreResult<layrs_sync::v3::PlanId> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs.sync.v3/plan-id\0");
    hasher.update(space_id.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(client_id.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(request_id.as_str().as_bytes());
    layrs_sync::v3::PlanId::parse(format!("plan:{}", hasher.finalize().to_hex()))
        .map_err(|_| V3StoreError::Backend("cannot construct deterministic PlanId".into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessedDecision {
    Allowed,
    Denied(DeniedCode),
}

fn processed_claims<'a>(
    claims: &'a [FetchClaim],
    plan: &FetchPlan,
) -> V3StoreResult<Vec<(usize, &'a FetchClaim, ProcessedDecision)>> {
    let mut result = Vec::new();
    for (ordinal, claim) in claims.iter().enumerate() {
        if let Some(denied) = plan.denied_claims.iter().find(|item| item.claim == *claim) {
            result.push((ordinal, claim, ProcessedDecision::Denied(denied.code)));
        } else if plan
            .download_capabilities
            .iter()
            .any(|capability| capability.claim == *claim)
        {
            result.push((ordinal, claim, ProcessedDecision::Allowed));
        }
    }
    if result.windows(2).any(|pair| pair[0].0 + 1 != pair[1].0) {
        return Err(V3StoreError::InvalidDescriptor(
            "fetch page skipped a claim".into(),
        ));
    }
    Ok(result)
}

pub(crate) fn descriptor_from_row(row: &sqlx::postgres::PgRow) -> V3StoreResult<ObjectDescriptor> {
    Ok(ObjectDescriptor {
        kind: parse_kind(&row.get::<String, _>("object_kind"))?,
        object_id: ObjectId::parse(row.get::<String, _>("object_id"))
            .map_err(|_| V3StoreError::CorruptObject("invalid ObjectId".into()))?,
        canonical_byte_len: u64::try_from(row.get::<i64, _>("canonical_byte_length"))
            .map_err(|_| V3StoreError::CorruptObject("invalid object length".into()))?,
        encoded_byte_len: u64::try_from(row.get::<i64, _>("encoded_byte_length"))
            .map_err(|_| V3StoreError::CorruptObject("invalid object length".into()))?,
        compression: match row.get::<String, _>("compression").as_str() {
            "identity" => Compression::Identity,
            "zstd" => Compression::Zstd,
            _ => return Err(V3StoreError::CorruptObject("invalid compression".into())),
        },
    })
}

pub(crate) fn parse_kind(value: &str) -> V3StoreResult<ObjectKind> {
    Ok(match value {
        "step" => ObjectKind::Step,
        "repository_view" => ObjectKind::RepositoryView,
        "chunk" => ObjectKind::Chunk,
        "file_manifest" => ObjectKind::FileManifest,
        "tree" => ObjectKind::Tree,
        "conflict" => ObjectKind::Conflict,
        "symlink" => ObjectKind::Symlink,
        "tombstone" => ObjectKind::Tombstone,
        "evidence" => ObjectKind::Evidence,
        "product" => ObjectKind::Product,
        "policy_set" => ObjectKind::PolicySet,
        "authorization_snapshot" => ObjectKind::AuthorizationSnapshot,
        _ => return Err(V3StoreError::CorruptObject("invalid object kind".into())),
    })
}

#[cfg(test)]
mod tests;
