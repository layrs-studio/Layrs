use super::*;
use layrs_core::scm::{LayerId, ProofId, StepId};
use layrs_sync::v3::*;
use types::{invalid, validate_key};

impl PgCollaborationStore {
    pub async fn evaluate_gate(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        layer: &LayerId,
        step: &StepId,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::ReadReviews)
            .await?;
        let policies = self.policy_products(&authority).await?;
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        let locked = super::gates::lock_reviewers(&mut tx, space).await?;
        super::super::transaction_store::lock_identity(&mut tx, principal, space).await?;
        let row=sqlx::query("SELECT current_repository_view_id,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR UPDATE").bind(space.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("current_repository_view_id")
            != authority.repository_view_id.as_str()
            || row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let raw:Option<String>=sqlx::query_scalar("SELECT canonical_document_jcs FROM layrs_collaboration.layer_protections WHERE space_id=$1 AND layer_id=$2").bind(space.as_str()).bind(layer.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let protection: LayerProtection =
            serde_json::from_str(&raw.ok_or(V3StoreError::NotFound)?).map_err(bad)?;
        let gate = protection
            .gate_definition_object_id
            .ok_or(V3StoreError::NotFound)?;
        let planned = LayerMutationResult {
            layer_id: layer.clone(),
            mode: LayerMutationMode::Advance,
            previous_heads: vec![],
            proposed_heads: vec![step.clone()],
            resulting_heads: vec![step.clone()],
            divergence_created: false,
            decision_product_object_id: Nullable::null(),
        };
        let rejection = super::gates::enforce(
            self.repository.cas().as_ref(),
            &mut tx,
            &authority,
            &[planned],
            &locked,
        )
        .await?;
        let proof_rows=sqlx::query("SELECT r.proof_object_id,p.logical_id FROM layrs_collaboration.runs r JOIN products p ON p.product_object_id=r.proof_object_id WHERE r.space_id=$1 AND r.step_id=$2 AND r.policy_set_id=$3 AND r.authorization_epoch=$4 ORDER BY p.logical_id LIMIT 100").bind(space.as_str()).bind(step.as_str()).bind(authority.policy_set_id.as_str()).bind(authority.authorization_epoch as i64).fetch_all(&mut *tx).await.map_err(pg_error)?;
        let proofs: Vec<ObjectId> = proof_rows
            .iter()
            .map(|r| parse(r.get("proof_object_id")))
            .collect::<V3StoreResult<_>>()?;
        let gate_logical: String =
            sqlx::query_scalar("SELECT logical_id FROM products WHERE product_object_id=$1")
                .bind(gate.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(pg_error)?;
        let mut dependencies: Vec<ProductLogicalId> =
            policies.iter().map(|(_, id)| id.clone()).collect();
        dependencies.push(parse(gate_logical)?);
        for row in proof_rows {
            dependencies.push(parse(row.get("logical_id"))?)
        }
        dependencies.sort();
        dependencies.dedup();
        let timestamp = now()?;
        let evaluation_id: GateEvaluationId = parse(format!(
            "gate-evaluation:{}",
            digest(&(space, layer, step, &timestamp))?
        ))?;
        let outcome = if rejection.is_none() {
            GateOutcome::Passed
        } else {
            GateOutcome::Failed
        };
        let (product, object) = self
            .product(ProductEnvelope::GateEvaluation {
                product_id: evaluation_id.clone(),
                product_object_id: placeholder()?,
                depends_on_step_ids: vec![step.clone()],
                depends_on_product_ids: dependencies,
                record: GateEvaluationRecord {
                    schema_version: GateEvaluationSchemaVersion::V1,
                    gate_evaluation_id: evaluation_id,
                    space_id: space.clone(),
                    gate_definition_object_id: gate.clone(),
                    subject: ImmutableSubject::Step {
                        object_id: parse(step.as_str().into())?,
                    },
                    proof_object_ids: proofs.clone(),
                    policy_version_object_ids: policies.iter().map(|(id, _)| id.clone()).collect(),
                    outcome,
                    evaluated_at: timestamp,
                },
            })
            .await?;
        let references: Vec<ObjectId> = vec![gate, parse(step.as_str().into())?]
            .into_iter()
            .chain(proofs)
            .chain(policies.iter().map(|(id, _)| id.clone()))
            .collect();
        self.install_product(&mut tx, space, &product, &object, &references)
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(
            serde_json::json!({"outcome":outcome,"gateEvaluationObjectId":object.descriptor.object_id,"rejection":rejection,"product":product}),
        )
    }
    pub async fn get_run(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        run: &str,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::Bootstrap)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let row=sqlx::query("SELECT r.run_id,r.step_id,r.recipe_object_id,r.runner_principal_id,r.proof_object_id,r.outcome,to_char(r.expires_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS expires_at,to_char(r.completed_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS completed_at,p.canonical_envelope_jcs,c.evidence_object_id AS cancellation_object_id,c.canonical_document_jcs AS cancellation_document,CASE WHEN c.run_id IS NOT NULL THEN 'cancelled' WHEN r.proof_object_id IS NOT NULL THEN 'completed' WHEN r.expires_at<=clock_timestamp() THEN 'expired' WHEN r.execution_requested AND r.execution_started_at IS NULL THEN 'queued' ELSE 'running' END AS status FROM layrs_collaboration.runs r LEFT JOIN products p ON p.product_object_id=r.proof_object_id LEFT JOIN layrs_collaboration.run_cancellations c ON c.run_id=r.run_id AND c.space_id=r.space_id WHERE r.space_id=$1 AND r.run_id=$2").bind(space.as_str()).bind(run).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let cancellation = super::run_cancellation::cancellation_from_row(&row)?;
        let raw: Option<String> = row.get("canonical_envelope_jcs");
        let product = raw
            .map(|s| serde_json::from_str::<ProductEnvelope>(&s).map_err(bad))
            .transpose()?;
        let mut evidence = vec![];
        if let Some(ProductEnvelope::Proof { record, .. }) = &product {
            for id in &record.evidence_object_ids {
                let descriptor = load_space_descriptor(self.repository.pool(), space, id).await?;
                let bytes =
                    read_canonical_object(self.repository.cas().as_ref(), &descriptor, 1024 * 1024)
                        .await?;
                evidence.push(serde_json::from_slice::<serde_json::Value>(&bytes).map_err(bad)?);
            }
        }
        Ok(
            serde_json::json!({"runId":run,"stepId":row.get::<String,_>("step_id"),"recipeObjectId":row.get::<String,_>("recipe_object_id"),"runnerPrincipalId":row.get::<String,_>("runner_principal_id"),"proofObjectId":row.get::<Option<String>,_>("proof_object_id"),"outcome":row.get::<Option<String>,_>("outcome"),"expiresAt":row.get::<String,_>("expires_at"),"completedAt":row.get::<Option<String>,_>("completed_at"),"product":product,"evidence":evidence,"status":row.get::<String,_>("status"),"cancellation":cancellation}),
        )
    }
    async fn policy_products(
        &self,
        authority: &SpaceAuthority,
    ) -> V3StoreResult<Vec<(ObjectId, ProductLogicalId)>> {
        let rows=sqlx::query("SELECT e.policy_product_object_id,p.logical_id FROM policy_set_entries e JOIN products p ON p.product_object_id=e.policy_product_object_id WHERE e.policy_set_id=$1 ORDER BY p.logical_id").bind(authority.policy_set_id.as_str()).fetch_all(self.repository.pool()).await.map_err(pg_error)?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    parse(r.get("policy_product_object_id"))?,
                    parse(r.get("logical_id"))?,
                ))
            })
            .collect()
    }
    async fn product(
        &self,
        mut product: ProductEnvelope,
    ) -> V3StoreResult<(ProductEnvelope, PreparedCasObject)> {
        let id = product.computed_object_id().map_err(bad)?;
        match &mut product {
            ProductEnvelope::Proof {
                product_object_id, ..
            }
            | ProductEnvelope::GateDefinition {
                product_object_id, ..
            }
            | ProductEnvelope::GateEvaluation {
                product_object_id, ..
            } => *product_object_id = id.clone(),
            _ => return Err(invalid("unsupported_collaboration_product")),
        }
        let object = self
            .prepare(
                ObjectKind::Product,
                id,
                product.canonical_bytes().map_err(bad)?,
            )
            .await?;
        Ok((product, object))
    }
    async fn install_product(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: &SpaceId,
        product: &ProductEnvelope,
        object: &PreparedCasObject,
        references: &[ObjectId],
    ) -> V3StoreResult<()> {
        self.install(tx, space, object).await?;
        let (kind, logical) = match product {
            ProductEnvelope::Proof { product_id, .. } => ("proof", product_id.as_str()),
            ProductEnvelope::GateDefinition { product_id, .. } => {
                ("gateDefinition", product_id.as_str())
            }
            ProductEnvelope::GateEvaluation { product_id, .. } => {
                ("gateEvaluation", product_id.as_str())
            }
            _ => return Err(invalid("unsupported_collaboration_product")),
        };
        let id = object.descriptor.object_id.as_str();
        sqlx::query("INSERT INTO products(product_object_id,space_id,product_kind,logical_id,canonical_envelope_jcs) VALUES($1,$2,$3,$4,$5)").bind(id).bind(space.as_str()).bind(kind).bind(logical).bind(jcs(product)?).execute(&mut **tx).await.map_err(pg_error)?;
        for (ordinal, step) in product.dependencies().0.iter().enumerate() {
            sqlx::query("INSERT INTO product_step_dependencies(space_id,product_object_id,ordinal,step_id) VALUES($1,$2,$3,$4)").bind(space.as_str()).bind(id).bind(ordinal as i32).bind(step.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
        let mut unique = references.to_vec();
        unique.sort();
        unique.dedup();
        let mut dependency_ordinal = 0;
        for (ordinal, reference) in unique.iter().enumerate() {
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,$2,'product_dependency',$3)").bind(id).bind(ordinal as i32).bind(reference.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
            let product_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM products WHERE space_id=$1 AND product_object_id=$2)",
            )
            .bind(space.as_str())
            .bind(reference.as_str())
            .fetch_one(&mut **tx)
            .await
            .map_err(pg_error)?;
            if product_exists {
                sqlx::query("INSERT INTO product_dependencies(space_id,product_object_id,ordinal,dependency_product_object_id) VALUES($1,$2,$3,$4)").bind(space.as_str()).bind(id).bind(dependency_ordinal).bind(reference.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
                dependency_ordinal += 1;
            }
        }
        Ok(())
    }
    pub async fn configure_protection(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        layer: &LayerId,
        request: ProtectionRequest,
    ) -> V3StoreResult<LayerProtection> {
        request.validate()?;
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let recipe = match &request.recipe {
            Some(value) => Some(
                self.evidence(
                    &serde_json::json!({"schemaVersion":"layrs.recipe.v1","recipe":value}),
                )
                .await?,
            ),
            None => None,
        };
        let policies = self.policy_products(&authority).await?;
        let timestamp = now()?;
        let gate_id: GateDefinitionId = parse(format!(
            "gate-definition:{}",
            digest(&(space, layer, &request, &timestamp))?
        ))?;
        let recipes: Vec<ObjectId> = recipe
            .iter()
            .map(|o| o.descriptor.object_id.clone())
            .collect();
        let (gate, gate_object) = self
            .product(ProductEnvelope::GateDefinition {
                product_id: gate_id.clone(),
                product_object_id: placeholder()?,
                depends_on_step_ids: vec![],
                depends_on_product_ids: policies.iter().map(|(_, id)| id.clone()).collect(),
                previous_product_object_id: Nullable::null(),
                record: GateDefinitionRecord {
                    schema_version: GateDefinitionSchemaVersion::V1,
                    gate_definition_id: gate_id,
                    space_id: space.clone(),
                    name: format!("Protection {}", layer.as_str()),
                    recipe_object_ids: recipes.clone(),
                    policy_version_object_ids: policies.iter().map(|(id, _)| id.clone()).collect(),
                    created_at: timestamp.clone(),
                },
            })
            .await?;
        let protection = LayerProtection {
            schema_version: "layrs.layer-protection.v1".into(),
            space_id: space.clone(),
            layer_id: layer.clone(),
            minimum_approvals: request.minimum_approvals,
            allow_self_approval: request.allow_self_approval,
            reject_conflicts: request.reject_conflicts,
            recipe_object_id: recipes.first().cloned(),
            gate_definition_object_id: Some(gate_object.descriptor.object_id.clone()),
            trusted_runner_principal_ids: request.trusted_runner_principal_ids,
            maximum_proof_age_seconds: request.maximum_proof_age_seconds,
            configured_by: principal.principal_id.clone(),
            configured_at: timestamp,
        };
        let evidence = self.evidence(&protection).await?;
        let mut tx = self.locked(principal, &authority).await?;
        for runner in &protection.trusted_runner_principal_ids {
            let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM spaces s JOIN workspace_memberships m USING(workspace_id) JOIN principals p USING(principal_id) WHERE s.space_id=$1 AND p.principal_id=$2 AND p.principal_kind='service' AND p.status='active' AND m.state='active')").bind(space.as_str()).bind(runner.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if !active {
                return Err(invalid("trusted_runner_not_active_service"));
            }
        }
        if let Some(recipe) = &recipe {
            self.install(&mut tx, space, recipe).await?;
        }
        let references: Vec<ObjectId> = recipes
            .into_iter()
            .chain(policies.iter().map(|(id, _)| id.clone()))
            .collect();
        self.install_product(&mut tx, space, &gate, &gate_object, &references)
            .await?;
        self.install(&mut tx, space, &evidence).await?;
        sqlx::query("INSERT INTO layrs_collaboration.layer_protections(space_id,layer_id,protection_object_id,gate_definition_object_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5) ON CONFLICT(space_id,layer_id) DO UPDATE SET protection_object_id=EXCLUDED.protection_object_id,gate_definition_object_id=EXCLUDED.gate_definition_object_id,canonical_document_jcs=EXCLUDED.canonical_document_jcs").bind(space.as_str()).bind(layer.as_str()).bind(evidence.descriptor.object_id.as_str()).bind(gate_object.descriptor.object_id.as_str()).bind(jcs(&protection)?).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            space,
            "protection.changed",
            layer.as_str(),
            &evidence.descriptor.object_id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(protection)
    }
    pub async fn get_protection(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        layer: &LayerId,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::Bootstrap)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let raw:Option<String>=sqlx::query_scalar("SELECT canonical_document_jcs FROM layrs_collaboration.layer_protections WHERE space_id=$1 AND layer_id=$2").bind(space.as_str()).bind(layer.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        match raw {
            Some(raw) => serde_json::from_str(&raw).map_err(bad),
            None => Ok(serde_json::Value::Null),
        }
    }
    pub async fn start_run(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: StartRunRequest,
    ) -> V3StoreResult<serde_json::Value> {
        self.register_run(principal, space, request, false).await
    }
    pub async fn prepare_run(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: StartRunRequest,
    ) -> V3StoreResult<serde_json::Value> {
        self.register_run(principal, space, request, true).await
    }
    async fn register_run(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: StartRunRequest,
        awaiting_execution: bool,
    ) -> V3StoreResult<serde_json::Value> {
        validate_key(&request.idempotency_key)?;
        let authority = self.authority(principal, space, V3Action::RunProof).await?;
        let run_id = key_id("run", principal, space, &request.idempotency_key);
        let digest = if awaiting_execution {
            digest(&("awaiting_execution", &request))?
        } else {
            digest(&request)?
        };
        let mut tx = self.locked(principal, &authority).await?;
        let service: bool = sqlx::query_scalar(
            "SELECT principal_kind='service' FROM principals WHERE principal_id=$1",
        )
        .bind(principal.principal_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        if !service {
            return Err(invalid("proof_requires_service_identity"));
        }
        if let Some(row) = sqlx::query(
            "SELECT start_digest,expires_at::text FROM layrs_collaboration.runs WHERE run_id=$1",
        )
        .bind(&run_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_error)?
        {
            if row.get::<String, _>("start_digest") != digest {
                return Err(invalid("idempotency_digest_mismatch"));
            }
            return Ok(
                serde_json::json!({"runId":run_id,"expiresAt":row.get::<String,_>("expires_at")}),
            );
        }
        let protections:Vec<String>=sqlx::query_scalar("SELECT canonical_document_jcs FROM layrs_collaboration.layer_protections WHERE space_id=$1").bind(space.as_str()).fetch_all(&mut *tx).await.map_err(pg_error)?;
        let trusted = protections
            .iter()
            .map(|raw| serde_json::from_str::<LayerProtection>(raw).map_err(bad))
            .collect::<V3StoreResult<Vec<_>>>()?
            .iter()
            .any(|p| {
                p.recipe_object_id.as_ref() == Some(&request.recipe_object_id)
                    && p.trusted_runner_principal_ids
                        .contains(&principal.principal_id)
            });
        if !trusted {
            return Err(invalid("runner_or_recipe_not_trusted"));
        }
        let expiry:String=sqlx::query_scalar("INSERT INTO layrs_collaboration.runs(run_id,space_id,step_id,recipe_object_id,runner_principal_id,policy_set_id,authorization_epoch,start_digest,execution_requested) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING expires_at::text").bind(&run_id).bind(space.as_str()).bind(request.step_id.as_str()).bind(request.recipe_object_id.as_str()).bind(principal.principal_id.as_str()).bind(authority.policy_set_id.as_str()).bind(authority.authorization_epoch as i64).bind(digest).bind(awaiting_execution).fetch_one(&mut *tx).await.map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(serde_json::json!({"runId":run_id,"expiresAt":expiry}))
    }
    pub async fn complete_run(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        run: &str,
        request: CompleteRunRequest,
    ) -> V3StoreResult<serde_json::Value> {
        if request.log.len() > 256 * 1024 {
            return Err(invalid("proof_log_too_large"));
        }
        let authority = self.authority(principal, space, V3Action::RunProof).await?;
        let completion_digest = digest(&request)?;
        let policies = self.policy_products(&authority).await?;
        let mut tx = self.locked(principal, &authority).await?;
        let row=sqlx::query("SELECT step_id,recipe_object_id,policy_set_id,authorization_epoch,completion_digest,proof_object_id,execution_requested AND execution_started_at IS NULL AS awaiting_execution,expires_at>clock_timestamp() AS current FROM layrs_collaboration.runs WHERE run_id=$1 AND space_id=$2 AND runner_principal_id=$3 FOR UPDATE").bind(run).bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let cancelled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.run_cancellations WHERE space_id=$1 AND run_id=$2)").bind(space.as_str()).bind(run).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if cancelled {
            return Err(invalid("run_cancelled"));
        }
        if let Some(previous) = row.get::<Option<String>, _>("completion_digest") {
            if previous != completion_digest {
                return Err(invalid("completion_digest_mismatch"));
            }
            return Ok(
                serde_json::json!({"runId":run,"proofObjectId":row.get::<String,_>("proof_object_id"),"outcome":request.outcome}),
            );
        }
        if !row.get::<bool, _>("current")
            || row.get::<String, _>("policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(invalid("stale_run"));
        }
        if row.get::<bool, _>("awaiting_execution") {
            return Err(invalid("run_execution_not_started"));
        }
        let step: StepId = parse(row.get("step_id"))?;
        let recipe: ObjectId = parse(row.get("recipe_object_id"))?;
        let timestamp = now()?;
        let verdict=self.evidence(&serde_json::json!({"schemaVersion":"layrs.run-verdict.v1","runId":run,"stepId":step,"recipeObjectId":recipe,"runnerPrincipalId":principal.principal_id,"policySetId":authority.policy_set_id,"authorizationEpoch":authority.authorization_epoch,"outcome":request.outcome,"log":request.log,"completedAt":timestamp})).await?;
        let proof_id: ProofId = parse(format!("proof:{}", digest(&(run, &completion_digest))?))?;
        let (proof, object) = self
            .product(ProductEnvelope::Proof {
                product_id: proof_id.clone(),
                product_object_id: placeholder()?,
                depends_on_step_ids: vec![step.clone()],
                depends_on_product_ids: policies.iter().map(|(_, id)| id.clone()).collect(),
                record: ProofRecord {
                    schema_version: ProofSchemaVersion::V1,
                    proof_id,
                    space_id: space.clone(),
                    subject: ImmutableSubject::Step {
                        object_id: parse(step.as_str().to_owned())?,
                    },
                    recipe_object_id: recipe.clone(),
                    evidence_object_ids: vec![verdict.descriptor.object_id.clone()],
                    policy_version_object_ids: policies.iter().map(|(id, _)| id.clone()).collect(),
                    created_at: timestamp,
                },
            })
            .await?;
        self.install(&mut tx, space, &verdict).await?;
        let references: Vec<ObjectId> = vec![
            parse(step.as_str().into())?,
            recipe,
            verdict.descriptor.object_id.clone(),
        ]
        .into_iter()
        .chain(policies.iter().map(|(id, _)| id.clone()))
        .collect();
        self.install_product(&mut tx, space, &proof, &object, &references)
            .await?;
        sqlx::query("UPDATE layrs_collaboration.runs SET completion_digest=$2,proof_object_id=$3,outcome=$4,completed_at=clock_timestamp() WHERE run_id=$1").bind(run).bind(completion_digest).bind(object.descriptor.object_id.as_str()).bind(request.outcome.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            space,
            "proof.completed",
            run,
            &object.descriptor.object_id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(
            serde_json::json!({"runId":run,"proofObjectId":object.descriptor.object_id,"outcome":request.outcome}),
        )
    }
    pub async fn list_runs(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<serde_json::Value> {
        self.list_runs_page(principal, space, RunListQuery::default())
            .await
    }
}
fn placeholder() -> V3StoreResult<ObjectId> {
    parse(format!("blake3:{}", "0".repeat(64)))
}
