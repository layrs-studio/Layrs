//! Native transaction admission. Preparation performs CAS I/O outside the final
//! transaction; the final transaction rechecks identity, membership and roots.
use super::closure::{VerifiedClosure, insert_tree_edges, verify_tree_closure};
use super::transaction_plan::{PlannedView, plan_view_with_decisions, reject};
use super::{
    EventCursorCodec, PgBootstrapStore, PgCatalogStore, PgV3Repository, PreparedCasObject,
    PrincipalContext, V3StoreError, V3StoreResult, i64_from_u64, pg_error,
};
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{ObjectId, RepositoryView, Step, Timestamp};
use layrs_graph::scm::StepDag;
use layrs_sync::v3::*;
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug)]
pub enum TransactionServiceError {
    Store(V3StoreError),
    RequestDigestMismatch,
    IdempotencyDigestMismatch,
    Busy,
}
impl From<V3StoreError> for TransactionServiceError {
    fn from(e: V3StoreError) -> Self {
        Self::Store(e)
    }
}

pub struct PersistedReceipt {
    pub receipt: SyncTransactionReceipt,
    pub canonical_jcs: Vec<u8>,
}

#[derive(Clone)]
pub struct PgTransactionStore {
    repository: PgV3Repository,
    cursors: EventCursorCodec,
}

mod commit;

struct Prepared {
    authority: super::SpaceAuthority,
    policy: PolicySet,
    snapshot: AuthorizationSnapshot,
    snapshot_object: PreparedCasObject,
    steps: Vec<(Step, PreparedCasObject)>,
    assertions: Vec<super::observations::assertions::PreparedAssertion>,
    products: Vec<super::transaction_products::PreparedProduct>,
    policies: super::transaction_policy::PreparedPolicies,
    view: Option<(RepositoryView, PreparedCasObject)>,
    proposed_view: Option<(RepositoryView, PreparedCasObject)>,
    planned: Option<PlannedView>,
    closure: Option<VerifiedClosure>,
    rejection: Option<Rejection>,
    timestamp: Timestamp,
}

impl PgTransactionStore {
    pub fn new(repository: PgV3Repository, cursors: EventCursorCodec) -> Self {
        Self {
            repository,
            cursors,
        }
    }

    pub async fn transact(
        &self,
        principal: &PrincipalContext,
        request: &SyncTransactionRequest,
    ) -> Result<PersistedReceipt, TransactionServiceError> {
        if request.computed_request_digest().map_err(json_error)? != request.request_digest {
            return Err(TransactionServiceError::RequestDigestMismatch);
        }
        // Retries only prepare against a fresh authority; they never repeat accepted effects.
        for _ in 0..3 {
            if let Some(stored) = self.replay(principal, request).await? {
                return Ok(stored);
            }
            let mut prepared = self.prepare(principal, request).await?;
            let _budget_writer = self.repository.cas_budget_writer().await;
            let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
            self.repository.lock_cas_budget(&mut tx).await?;
            let gate_principals =
                super::collaboration::gates::lock_reviewers(&mut tx, &request.space_id).await?;
            lock_identity(&mut tx, principal, &request.space_id).await?;
            let row = sqlx::query("SELECT current_repository_view_id,current_repository_view_generation,current_policy_set_id,authorization_epoch,next_event_sequence FROM spaces WHERE space_id=$1 AND lifecycle_state='ready' FOR UPDATE")
                .bind(request.space_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
            if let Some(stored) = replay_in(&mut tx, principal, request).await? {
                tx.commit().await.map_err(pg_error)?;
                return Ok(stored);
            }
            let same = row.get::<String, _>("current_repository_view_id")
                == prepared.authority.repository_view_id.as_str()
                && row.get::<i64, _>("current_repository_view_generation")
                    == i64_from_u64(prepared.authority.repository_view_generation)?
                && row.get::<String, _>("current_policy_set_id")
                    == prepared.authority.policy_set_id.as_str()
                && row.get::<i64, _>("authorization_epoch")
                    == i64_from_u64(prepared.authority.authorization_epoch)?;
            if !same {
                tx.rollback().await.map_err(pg_error)?;
                continue;
            }
            if prepared.rejection.is_none()
                && (super::transaction_choose::required(request)
                    || super::transaction_policy::required(request))
                && !super::transaction_choose::administrator(&mut tx, principal, &request.space_id)
                    .await?
            {
                prepared.rejection = Some(reject(
                    RejectionCode::PermissionDenied,
                    "administrative transaction requires active Workspace administrator role",
                ));
            }
            if prepared.rejection.is_none() {
                prepared.rejection = super::observations::assertions::recheck(
                    &mut tx,
                    principal,
                    &request.space_id,
                    &mut prepared.assertions,
                )
                .await?;
            }
            if prepared.rejection.is_none() {
                prepared.rejection = super::transaction_products::recheck_products(
                    &mut tx,
                    &request.space_id,
                    &mut prepared.products,
                )
                .await?;
            }
            if prepared.rejection.is_none() {
                prepared.rejection = super::transaction_policy::recheck(
                    &mut tx,
                    &request.space_id,
                    &prepared.policies,
                )
                .await?;
            }
            if prepared.rejection.is_none() {
                if let Some(planned) = &prepared.planned {
                    prepared.rejection = super::collaboration::gates::enforce(
                        self.repository.cas().as_ref(),
                        &mut tx,
                        &prepared.authority,
                        &planned.layers,
                        &gate_principals,
                    )
                    .await?;
                }
            }
            if prepared.rejection.is_none() {
                for product in &prepared.products {
                    if !product.existing {
                        if let ProductEnvelope::Release { record, .. } = &product.envelope {
                            prepared.rejection = super::collaboration::gates::enforce_release(
                                self.repository.cas().as_ref(),
                                &mut tx,
                                &prepared.authority,
                                record.target_step_id(),
                                &gate_principals,
                            )
                            .await?;
                            if prepared.rejection.is_some() {
                                break;
                            }
                        }
                    }
                }
            }
            let mut policy_objects = BTreeSet::from([prepared.authority.policy_set_id.clone()]);
            for entry in &prepared.policy.policy_versions {
                policy_objects.insert(entry.policy_product_object_id.clone());
                policy_objects.extend(entry.dependency_object_ids.iter().cloned());
            }
            for id in policy_objects {
                let valid: Option<String> = sqlx::query_scalar("SELECT a.object_id FROM space_object_availability a JOIN canonical_objects o ON o.object_id=a.object_id WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available' AND o.verification_state='verified' FOR SHARE OF a,o")
                    .bind(request.space_id.as_str()).bind(id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
                if valid.is_none() {
                    return Err(V3StoreError::AuthorizationEpochChanged.into());
                }
            }
            // Locks against quarantine/availability changes; CAS installation and hashing already finished.
            if let Some(closure) = &prepared.closure {
                for descriptor in closure.objects.values() {
                    let valid: Option<String> = sqlx::query_scalar("SELECT a.object_id FROM space_object_availability a JOIN canonical_objects o ON o.object_id=a.object_id WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available' AND o.verification_state='verified' FOR SHARE OF a,o")
                    .bind(request.space_id.as_str()).bind(descriptor.object_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
                    if valid.is_none() {
                        return Err(V3StoreError::NotFound.into());
                    }
                }
            }
            let result = self
                .commit(
                    &mut tx,
                    principal,
                    request,
                    prepared,
                    row.get::<i64, _>("next_event_sequence") as u64,
                )
                .await?;
            tx.commit().await.map_err(pg_error)?;
            return Ok(result);
        }
        Err(TransactionServiceError::Busy)
    }

    async fn replay(
        &self,
        principal: &PrincipalContext,
        request: &SyncTransactionRequest,
    ) -> Result<Option<PersistedReceipt>, TransactionServiceError> {
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        lock_identity(&mut tx, principal, &request.space_id).await?;
        let result = replay_in(&mut tx, principal, request).await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(result)
    }

    async fn prepare(
        &self,
        principal: &PrincipalContext,
        request: &SyncTransactionRequest,
    ) -> V3StoreResult<Prepared> {
        let authority = PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load_authority(&request.space_id)
            .await?;
        let mut cursor_epoch_changed = false;
        if let Some(cursor) = &request.known_event_cursor {
            let sequence = match self.cursors.verify(
                cursor,
                &request.space_id,
                &principal.principal_id,
                authority.authorization_epoch,
            ) {
                Ok(sequence) => sequence,
                Err(V3StoreError::AuthorizationEpochChanged) => {
                    cursor_epoch_changed = true;
                    0
                }
                Err(error) => return Err(error),
            };
            if sequence > authority.watermark_sequence {
                return Err(V3StoreError::InvalidCursor);
            }
        }
        let policy: PolicySet = read_json(
            self.repository.pool(),
            "SELECT canonical_record_jcs FROM policy_sets WHERE space_id=$1 AND policy_set_id=$2",
            &request.space_id,
            authority.policy_set_id.as_str(),
        )
        .await?;
        policy
            .validate_v3()
            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
        let timestamp = Timestamp::parse(
            OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|e| V3StoreError::Backend(e.to_string()))?,
        )
        .map_err(|e| V3StoreError::Backend(e.to_string()))?;
        let (snapshot, snapshot_object, authorized) = super::transaction_authorization::snapshot(
            &self.repository,
            principal,
            request,
            &authority,
            &policy,
            &timestamp,
        )
        .await?;
        let mut prepared = Prepared {
            authority,
            policy,
            snapshot,
            snapshot_object,
            steps: vec![],
            products: vec![],
            assertions: vec![],
            policies: Default::default(),
            view: None,
            proposed_view: None,
            planned: None,
            closure: None,
            rejection: None,
            timestamp,
        };
        if !authorized {
            prepared.rejection = Some(reject(
                RejectionCode::PermissionDenied,
                "transaction policy denied",
            ));
            return Ok(prepared);
        }
        if cursor_epoch_changed {
            // Persist a terminal outcome under the identity/Space locks. A
            // concurrent successful attempt still wins through replay_in;
            // clients can then release their exact pending request safely.
            prepared.rejection = Some(reject(
                RejectionCode::AuthorizationEpochChanged,
                "event cursor belongs to a previous authorization epoch; refresh before a new transaction",
            ));
            return Ok(prepared);
        }
        if let Err(error) = request.validate_v3() {
            prepared.rejection = Some(reject(RejectionCode::InvalidRequest, error.to_string()));
            return Ok(prepared);
        }
        prepared.assertions = match super::observations::assertions::prepare(principal, request) {
            Ok(assertions) => assertions,
            Err(rejection) => {
                prepared.rejection = Some(rejection);
                return Ok(prepared);
            }
        };
        if let Some(mutation) = &request.policy_root_mutation {
            if mutation.expected_policy_set_id != prepared.authority.policy_set_id
                || mutation.expected_authorization_epoch != prepared.authority.authorization_epoch
            {
                prepared.rejection = Some(reject(
                    RejectionCode::AuthorizationEpochChanged,
                    "PolicyRoot expected ID and epoch differ from current authority",
                ));
                return Ok(prepared);
            }
        }
        if super::transaction_policy::required(request) {
            match super::transaction_policy::prepare(&self.repository, request).await {
                Ok(policies) => prepared.policies = policies,
                Err(V3StoreError::NotFound) => {
                    prepared.rejection = Some(reject(
                        RejectionCode::MissingObject,
                        "Policy closure unavailable in this Space",
                    ));
                    return Ok(prepared);
                }
                Err(
                    V3StoreError::IdentityMismatch
                    | V3StoreError::CorruptObject(_)
                    | V3StoreError::InvalidDescriptor(_)
                    | V3StoreError::InvalidCapability,
                ) => {
                    prepared.rejection = Some(reject(
                        RejectionCode::PolicyRejected,
                        "Policy identity, closure or supported v1 rules failed validation",
                    ));
                    return Ok(prepared);
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(mutation) = &request.view_mutation {
            if mutation
                .verify_captured_policy_versions(&prepared.policy)
                .is_err()
            {
                prepared.rejection = Some(reject(
                    RejectionCode::AuthorizationEpochChanged,
                    "Policy versions differ from captured authority",
                ));
                return Ok(prepared);
            }
        }
        let rows: Vec<String> = sqlx::query_scalar("SELECT s.canonical_record_jcs FROM steps s JOIN space_steps ss ON ss.step_id=s.step_id WHERE ss.space_id=$1 ORDER BY s.step_id LIMIT 100001")
            .bind(request.space_id.as_str()).fetch_all(self.repository.pool()).await.map_err(pg_error)?;
        if rows.len() > 100_000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "Space graph",
                limit: 100_000,
            });
        }
        let mut known: Vec<Step> = rows
            .into_iter()
            .map(|s| serde_json::from_str(&s).map_err(json_error))
            .collect::<V3StoreResult<_>>()?;
        let known_ids: BTreeSet<_> = known.iter().map(|s| s.id().clone()).collect();
        let new_steps: Vec<_> = request
            .steps
            .iter()
            .filter(|s| !known_ids.contains(s.id()))
            .cloned()
            .collect();
        known.extend(new_steps.iter().cloned());
        if let Some(rejection) = super::observations::assertions::validate_steps(
            &prepared.assertions,
            &known.iter().map(|s| s.id().clone()).collect(),
        ) {
            prepared.rejection = Some(rejection);
            return Ok(prepared);
        }
        let graph = match StepDag::from_steps(&known) {
            Ok(g) => g,
            Err(_) => {
                prepared.rejection = Some(reject(
                    RejectionCode::MissingParent,
                    "Step parent closure is not available in this Space",
                ));
                return Ok(prepared);
            }
        };
        let roots = request
            .steps
            .iter()
            .map(|s| ObjectId::parse(s.root_tree_id().as_str()).expect("typed TreeId"))
            .collect();
        match verify_tree_closure(
            &self.repository,
            &request.space_id,
            roots,
            &known.iter().map(|s| (s.id().clone(), s.clone())).collect(),
            &new_steps.iter().map(|s| s.id().clone()).collect(),
        )
        .await
        {
            Ok(closure) => prepared.closure = Some(closure),
            Err(V3StoreError::NotFound) => {
                prepared.rejection = Some(reject(
                    RejectionCode::MissingObject,
                    "Step content closure is incomplete",
                ));
                return Ok(prepared);
            }
            Err(
                V3StoreError::IdentityMismatch
                | V3StoreError::CorruptObject(_)
                | V3StoreError::InvalidDescriptor(_),
            ) => {
                prepared.rejection = Some(reject(
                    RejectionCode::ImmutableObjectMismatch,
                    "Step content closure failed verification",
                ));
                return Ok(prepared);
            }
            Err(error) => return Err(error),
        }
        for step in &new_steps {
            let object = self.prepare_step(step, &request.space_id).await?;
            prepared.steps.push((step.clone(), object));
        }
        let mut products =
            match super::transaction_choose::products(&self.repository, request).await {
                Ok(products) => products,
                Err(V3StoreError::NotFound) => {
                    prepared.rejection = Some(reject(
                        RejectionCode::MissingObject,
                        "Decision Product unavailable in this Space",
                    ));
                    return Ok(prepared);
                }
                Err(V3StoreError::IdentityMismatch | V3StoreError::CorruptObject(_)) => {
                    prepared.rejection = Some(reject(
                        RejectionCode::ImmutableObjectMismatch,
                        "Decision Product failed CAS verification",
                    ));
                    return Ok(prepared);
                }
                Err(error) => return Err(error),
            };
        products.retain(|p| !matches!(p, ProductEnvelope::Policy { .. }));
        if products.iter().any(|p|matches!(p,ProductEnvelope::Decision{record,..} if record.decided_by!=principal.actor_id || record.reason.trim().is_empty() || record.reason.len()>4096)) {
            prepared.rejection=Some(reject(RejectionCode::PermissionDenied,"Decision requires the authenticated actor and an explicit bounded reason"));return Ok(prepared);
        }
        prepared.products = match super::transaction_products::prepare_products(
            &self.repository,
            &request.space_id,
            &products,
            &graph,
            &new_steps.iter().map(|s| s.id().clone()).collect(),
            &known.iter().map(|s| (s.id().clone(), s.clone())).collect(),
        )
        .await
        {
            Ok(Ok(products)) => products,
            Ok(Err(rejection)) => {
                prepared.rejection = Some(rejection);
                return Ok(prepared);
            }
            Err(V3StoreError::NotFound) => {
                prepared.rejection = Some(reject(
                    RejectionCode::MissingObject,
                    "Product dependency unavailable",
                ));
                return Ok(prepared);
            }
            Err(error) => return Err(error),
        };
        if let Err(error) = super::closure::lineage::validate(
            &self.repository,
            &request.space_id,
            &new_steps,
            &known.iter().map(|s| (s.id().clone(), s.clone())).collect(),
            &products,
            prepared.closure.as_mut().expect("verified closure"),
        )
        .await
        {
            match error {
                V3StoreError::NotFound
                | V3StoreError::IdentityMismatch
                | V3StoreError::CorruptObject(_)
                | V3StoreError::InvalidDescriptor(_) => {
                    prepared.rejection = Some(reject(
                        RejectionCode::ImmutableObjectMismatch,
                        "Conflict origin or partial resolution lineage failed verification",
                    ));
                    return Ok(prepared);
                }
                other => return Err(other),
            }
        }
        if let Some(mutation) = &request.view_mutation {
            let current: RepositoryView = read_json(self.repository.pool(),"SELECT canonical_record_jcs FROM repository_views WHERE space_id=$1 AND repository_view_id=$2",&request.space_id,prepared.authority.repository_view_id.as_str()).await?;
            let historical: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM repository_view_history WHERE space_id=$1 AND generation=$2 AND repository_view_id=$3)")
                .bind(request.space_id.as_str()).bind(i64_from_u64(mutation.expected_generation)?).bind(mutation.expected_current_view_id.as_str()).fetch_one(self.repository.pool()).await.map_err(pg_error)?;
            if !historical {
                prepared.rejection = Some(reject(
                    RejectionCode::ViewGenerationConflict,
                    "expected View is not in generation history",
                ));
                return Ok(prepared);
            }
            let expected: RepositoryView = read_json(self.repository.pool(),"SELECT canonical_record_jcs FROM repository_views WHERE space_id=$1 AND repository_view_id=$2",&request.space_id,mutation.expected_current_view_id.as_str()).await?;
            let proposed = match &mutation.proposed_view {
                ProposedRepositoryView::Inline { view }=>view.clone(),
                ProposedRepositoryView::Stored { repository_view_id }=>match read_json(self.repository.pool(),"SELECT canonical_record_jcs FROM repository_views WHERE space_id=$1 AND repository_view_id=$2",&request.space_id,repository_view_id.as_str()).await { Ok(view)=>view,Err(V3StoreError::NotFound)=>{prepared.rejection=Some(reject(RejectionCode::MissingObject,"proposed View unavailable"));return Ok(prepared);},Err(e)=>return Err(e) },
            };
            let decisions = prepared
                .products
                .iter()
                .filter_map(|p| {
                    if let ProductEnvelope::Decision { record, .. } = &p.envelope {
                        Some((p.envelope.object_id().clone(), record.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            let planned = match plan_view_with_decisions(
                &graph,
                &current,
                prepared.authority.repository_view_generation,
                &expected,
                &proposed,
                mutation,
                &decisions,
            ) {
                Ok(p) => p,
                Err(r) => {
                    prepared.rejection = Some(r);
                    return Ok(prepared);
                }
            };
            for heads in proposed.layer_heads() {
                let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM current_layer_definitions WHERE space_id=$1 AND layer_id=$2)").bind(request.space_id.as_str()).bind(heads.layer_id().as_str()).fetch_one(self.repository.pool()).await.map_err(pg_error)?;
                if !exists && !prepared.products.iter().any(|p|matches!(&p.envelope,ProductEnvelope::Layer{product_id,..} if product_id==heads.layer_id())) {
                    prepared.rejection = Some(reject(
                        RejectionCode::MissingObject,
                        "Layer definition unavailable",
                    ));
                    return Ok(prepared);
                }
            }
            for (kind, ids) in [
                (
                    "marker",
                    proposed
                        .marker_ids()
                        .iter()
                        .map(|id| id.as_str())
                        .collect::<Vec<_>>(),
                ),
                (
                    "release",
                    proposed
                        .release_ids()
                        .iter()
                        .map(|id| id.as_str())
                        .collect(),
                ),
            ] {
                for id in ids {
                    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products WHERE space_id=$1 AND product_kind=$2 AND logical_id=$3)").bind(request.space_id.as_str()).bind(kind).bind(id).fetch_one(self.repository.pool()).await.map_err(pg_error)?;
                    if !exists
                        && !prepared
                            .products
                            .iter()
                            .any(|p| p.envelope.logical_id() == id)
                    {
                        prepared.rejection = Some(reject(
                            RejectionCode::MissingObject,
                            "named reference unavailable",
                        ));
                        return Ok(prepared);
                    }
                }
            }
            let object = self
                .repository
                .cas()
                .prepare_generated(
                    ObjectKind::RepositoryView,
                    Bytes::from(serde_jcs::to_vec(&planned.view).map_err(json_error)?),
                )
                .await?;
            let proposed_object = self
                .repository
                .cas()
                .prepare_generated(
                    ObjectKind::RepositoryView,
                    Bytes::from(serde_jcs::to_vec(&proposed).map_err(json_error)?),
                )
                .await?;
            prepared.view = Some((planned.view.clone(), object));
            prepared.proposed_view = Some((proposed, proposed_object));
            prepared.planned = Some(planned);
        }
        Ok(prepared)
    }

    async fn prepare_step(
        &self,
        step: &Step,
        space: &layrs_core::scm::SpaceId,
    ) -> V3StoreResult<PreparedCasObject> {
        let bytes = serde_jcs::to_vec(step).map_err(json_error)?;
        let descriptor = ObjectDescriptor {
            kind: ObjectKind::Step,
            object_id: ObjectId::parse(step.id().as_str()).expect("typed StepId"),
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        };
        match super::load_space_descriptor(self.repository.pool(), space, &descriptor.object_id)
            .await
        {
            Ok(existing) => return self.repository.cas().verify_existing(&existing).await,
            Err(V3StoreError::NotFound) => {}
            Err(error) => return Err(error),
        }
        self.repository
            .cas()
            .put_if_absent(
                &descriptor,
                Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
            )
            .await?;
        self.repository.cas().verify_existing(&descriptor).await
    }
}

pub(crate) async fn lock_identity(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    space: &layrs_core::scm::SpaceId,
) -> V3StoreResult<()> {
    let active:Option<String>=sqlx::query_scalar("SELECT p.principal_id FROM principals p JOIN deployments d ON d.deployment_id=p.deployment_id WHERE p.principal_id=$1 AND p.actor_id=$2 AND p.deployment_id=$3 AND p.subject_digest=$4 AND p.status='active' AND d.status='ready' FOR SHARE OF p,d")
        .bind(principal.principal_id.as_str()).bind(principal.actor_id.as_str()).bind(&principal.deployment_id).bind(&principal.subject_digest).fetch_optional(&mut **tx).await.map_err(pg_error)?;
    if active.is_none() {
        return Err(V3StoreError::InvalidCapability);
    }
    let member:Option<String>=sqlx::query_scalar("SELECT wm.principal_id FROM workspace_memberships wm JOIN spaces s ON s.workspace_id=wm.workspace_id JOIN workspaces w ON w.workspace_id=s.workspace_id JOIN principals p ON p.principal_id=wm.principal_id AND p.deployment_id=w.deployment_id WHERE s.space_id=$1 AND wm.principal_id=$2 AND wm.state='active' AND s.lifecycle_state='ready' FOR SHARE OF wm")
        .bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
    if member.is_none() {
        return Err(V3StoreError::InvalidCapability);
    }
    Ok(())
}

async fn replay_in(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    request: &SyncTransactionRequest,
) -> Result<Option<PersistedReceipt>, TransactionServiceError> {
    let row=sqlx::query("SELECT request_digest,principal_id,receipt_canonical_jcs,receipt_digest FROM sync_transactions WHERE space_id=$1 AND client_id=$2 AND idempotency_key=$3")
        .bind(request.space_id.as_str()).bind(request.client_id.as_str()).bind(request.idempotency_key.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
    let Some(row) = row else { return Ok(None) };
    if row.get::<String, _>("principal_id") != principal.principal_id.as_str() {
        return Err(V3StoreError::InvalidCapability.into());
    }
    if row.get::<String, _>("request_digest") != request.request_digest.as_str() {
        return Err(TransactionServiceError::IdempotencyDigestMismatch);
    }
    let canonical_jcs = row.get::<String, _>("receipt_canonical_jcs").into_bytes();
    let receipt: SyncTransactionReceipt =
        serde_json::from_slice(&canonical_jcs).map_err(json_error)?;
    if receipt
        .computed_storage_digest()
        .map_err(json_error)?
        .as_str()
        != row.get::<String, _>("receipt_digest")
        || receipt.canonical_storage_bytes().map_err(json_error)? != canonical_jcs
    {
        return Err(V3StoreError::CorruptObject("stored receipt digest mismatch".into()).into());
    }
    receipt
        .authorize_replay(
            Some(&principal.principal_id),
            &request.client_id,
            &request.space_id,
            false,
        )
        .map_err(|_| V3StoreError::InvalidCapability)?;
    Ok(Some(PersistedReceipt {
        receipt,
        canonical_jcs,
    }))
}

async fn insert_view(
    tx: &mut Transaction<'_, Postgres>,
    catalog: &PgCatalogStore,
    view: &RepositoryView,
    object: &PreparedCasObject,
) -> V3StoreResult<()> {
    catalog.install_prepared(tx, object).await?;
    sqlx::query("INSERT INTO repository_views(repository_view_id,space_id,canonical_record_jcs) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(view.id().as_str()).bind(view.space_id().as_str()).bind(jcs(view)?).execute(&mut **tx).await.map_err(pg_error)?;
    catalog
        .make_available(tx, view.space_id(), &object.descriptor.object_id)
        .await?;
    for heads in view.layer_heads() {
        for step in heads.step_ids() {
            sqlx::query("INSERT INTO repository_view_layer_heads(space_id,repository_view_id,layer_id,step_id) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(view.space_id().as_str()).bind(view.id().as_str()).bind(heads.layer_id().as_str()).bind(step.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
    }
    for marker in view.marker_ids() {
        sqlx::query("INSERT INTO repository_view_markers(space_id,repository_view_id,marker_id,marker_product_object_id) SELECT $1,$2,$3,product_object_id FROM products WHERE space_id=$1 AND product_kind='marker' AND logical_id=$3 ON CONFLICT DO NOTHING").bind(view.space_id().as_str()).bind(view.id().as_str()).bind(marker.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
    }
    for release in view.release_ids() {
        sqlx::query("INSERT INTO repository_view_releases(space_id,repository_view_id,release_id,release_product_object_id) SELECT $1,$2,$3,product_object_id FROM products WHERE space_id=$1 AND product_kind='release' AND logical_id=$3 ON CONFLICT DO NOTHING").bind(view.space_id().as_str()).bind(view.id().as_str()).bind(release.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
    }
    Ok(())
}

fn json_error(error: serde_json::Error) -> V3StoreError {
    V3StoreError::CorruptObject(error.to_string())
}
fn jcs(value: &impl serde::Serialize) -> V3StoreResult<String> {
    serde_jcs::to_string(value).map_err(json_error)
}
async fn read_json<T: serde::de::DeserializeOwned>(
    pool: &sqlx::PgPool,
    query: &str,
    space: &layrs_core::scm::SpaceId,
    id: &str,
) -> V3StoreResult<T> {
    let value: Option<String> = sqlx::query_scalar(query)
        .bind(space.as_str())
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(pg_error)?;
    serde_json::from_str(&value.ok_or(V3StoreError::NotFound)?).map_err(json_error)
}
