use super::*;
impl PgTransactionStore {
    pub(super) async fn commit(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        request: &SyncTransactionRequest,
        prepared: Prepared,
        next_sequence: u64,
        scope: Option<&super::super::path_patch_scope::ScopedPatch>,
    ) -> V3StoreResult<PersistedReceipt> {
        let catalog = PgCatalogStore::new(self.repository.clone());
        catalog
            .install_prepared(tx, &prepared.snapshot_object)
            .await?;
        let snapshot = &prepared.snapshot;
        sqlx::query("INSERT INTO authorization_snapshots(authorization_snapshot_id,space_id,principal_id,policy_set_id,authorization_epoch,request_binding_jcs,decisions_jcs,canonical_record_jcs,evaluated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9::timestamptz) ON CONFLICT DO NOTHING")
            .bind(snapshot.authorization_snapshot_id.as_str()).bind(request.space_id.as_str()).bind(principal.principal_id.as_str()).bind(snapshot.policy_set_id.as_str()).bind(i64_from_u64(snapshot.authorization_epoch)?).bind(jcs(&snapshot.request_binding)?).bind(jcs(&snapshot.decisions)?).bind(jcs(snapshot)?).bind(snapshot.evaluated_at.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        catalog
            .make_available(tx, &request.space_id, &snapshot.authorization_snapshot_id)
            .await?;
        let transaction_id = TransactionId::parse(format!(
            "transaction:{}",
            &request.request_digest.as_str()[7..]
        ))
        .map_err(V3StoreError::Backend)?;
        let mut receipt = SyncTransactionReceipt {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionReceiptType::Receipt,
            principal_id: principal.principal_id.clone(),
            authorization_epoch: prepared.authority.authorization_epoch,
            resulting_authorization_epoch: prepared.authority.authorization_epoch,
            authorization_snapshot_id: snapshot.authorization_snapshot_id.clone(),
            current_policy_set_id: snapshot.policy_set_id.clone(),
            space_id: request.space_id.clone(),
            client_id: request.client_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            request_digest: request.request_digest.clone(),
            transaction_id: transaction_id.clone(),
            status: if prepared.rejection.is_some() {
                ReceiptStatus::Rejected
            } else {
                ReceiptStatus::Applied
            },
            accepted_step_ids: vec![],
            accepted_observation_ids: vec![],
            accepted_repository_view_ids: vec![],
            accepted_policy_set_ids: vec![],
            accepted_products: vec![],
            view_result: Nullable::null(),
            policy_root_result: Nullable::null(),
            policy_decisions: vec![],
            rejections: prepared.rejection.into_iter().collect(),
            event_ids: vec![],
            first_event_sequence: Nullable::null(),
            last_event_sequence: Nullable::null(),
            event_cursor: Nullable::null(),
            stored_at: prepared.timestamp.clone(),
        };
        let actor = EventActor {
            kind: ActorKind::Account,
            actor_id: principal.actor_id.clone(),
            device_id: Nullable::null(),
        };
        let mut events = Vec::new();
        let event_id = |sequence: u64| {
            EventId::parse(format!(
                "event:{}:{sequence:020}",
                &request.request_digest.as_str()[7..]
            ))
            .expect("valid deterministic event ID")
        };
        let mut sequence = next_sequence;
        if receipt.status == ReceiptStatus::Applied {
            let mut first_admitted_steps = Vec::new();
            for (step, object) in &prepared.steps {
                catalog.install_prepared(tx, object).await?;
                sqlx::query("INSERT INTO steps(step_id,root_tree_id,canonical_record_jcs) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(step.id().as_str()).bind(step.root_tree_id().as_str()).bind(jcs(step)?).execute(&mut **tx).await.map_err(pg_error)?;
                for (ordinal, parent) in step.parent_step_ids().iter().enumerate() {
                    sqlx::query("INSERT INTO step_parents(step_id,ordinal,parent_step_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(step.id().as_str()).bind(ordinal as i32).bind(parent.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
                }
                let newly_admitted = sqlx::query("INSERT INTO space_steps(space_id,step_id) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(request.space_id.as_str()).bind(step.id().as_str()).execute(&mut **tx).await.map_err(pg_error)?.rows_affected() == 1;
                if newly_admitted {
                    first_admitted_steps.push(step.id().clone());
                }
                catalog
                    .make_available(tx, &request.space_id, &object.descriptor.object_id)
                    .await?;
                receipt.accepted_step_ids.push(step.id().clone());
            }
            receipt.accepted_step_ids.sort();
            // Generated server observations are side effects of first Step
            // admission, not acknowledgements of client-supplied observations.
            let mut admission_observation_ids = super::super::observations::record_admissions(
                tx,
                principal,
                &request.space_id,
                &request.request_digest,
                &first_admitted_steps,
                &prepared.timestamp,
            )
            .await?;
            let (accepted_assertions, new_assertions) =
                super::super::observations::assertions::insert(
                    tx,
                    principal,
                    &request.space_id,
                    &request.request_digest,
                    &prepared.assertions,
                    &prepared.timestamp,
                )
                .await?;
            receipt.accepted_observation_ids = accepted_assertions;
            admission_observation_ids.extend(new_assertions);
            admission_observation_ids.sort();
            if let Some(closure) = &prepared.closure {
                insert_tree_edges(tx, closure).await?;
            }
            if !receipt.accepted_step_ids.is_empty() || !admission_observation_ids.is_empty() {
                events.push(SpaceEvent::StepsAccepted {
                    sequence,
                    event_id: event_id(sequence),
                    actor: actor.clone(),
                    transaction_id: transaction_id.clone(),
                    payload: StepsAcceptedPayload {
                        step_ids: receipt.accepted_step_ids.clone(),
                        observation_ids: admission_observation_ids,
                    },
                    created_at: prepared.timestamp.clone(),
                });
                sequence += 1;
            }
            receipt.accepted_products = super::super::transaction_products::insert_products(
                tx,
                &catalog,
                &request.space_id,
                &prepared.products,
            )
            .await?;
            receipt.accepted_products.extend(
                super::super::transaction_policy::install(
                    tx,
                    &catalog,
                    &request.space_id,
                    &prepared.policies,
                )
                .await?,
            );
            receipt.accepted_products.sort();
            if !receipt.accepted_products.is_empty() {
                events.push(SpaceEvent::ProductsAccepted {
                    sequence,
                    event_id: event_id(sequence),
                    actor: actor.clone(),
                    transaction_id: transaction_id.clone(),
                    payload: ProductsAcceptedPayload {
                        products: receipt.accepted_products.clone(),
                    },
                    created_at: prepared.timestamp.clone(),
                });
                sequence += 1;
            }
            for product in &prepared.products {
                if !product.existing {
                    if let ProductEnvelope::Layer {
                        product_id,
                        product_object_id,
                        ..
                    } = &product.envelope
                    {
                        events.push(SpaceEvent::LayerDefinitionChanged {
                            sequence,
                            event_id: event_id(sequence),
                            actor: actor.clone(),
                            transaction_id: transaction_id.clone(),
                            payload: LayerDefinitionChangedPayload {
                                layer_id: product_id.clone(),
                                previous_product_object_id: Nullable::null(),
                                product_object_id: product_object_id.clone(),
                            },
                            created_at: prepared.timestamp.clone(),
                        });
                        sequence += 1;
                    }
                }
            }
            if let Some((view, object)) = &prepared.proposed_view {
                insert_view(tx, &catalog, view, object).await?;
                receipt.accepted_repository_view_ids.push(view.id().clone());
            }
            if let (Some((view, object)), Some(plan), Some(mutation)) =
                (&prepared.view, &prepared.planned, &request.view_mutation)
            {
                insert_view(tx, &catalog, view, object).await?;
                receipt.accepted_repository_view_ids.push(view.id().clone());
                receipt.accepted_repository_view_ids.sort();
                receipt.accepted_repository_view_ids.dedup();
                let changed = view.id() != &prepared.authority.repository_view_id;
                let generation = prepared.authority.repository_view_generation + u64::from(changed);
                let result = RepositoryViewResult {
                    mutation_id: mutation.mutation_id.clone(),
                    outcome: plan.outcome,
                    old_repository_view_id: prepared.authority.repository_view_id.clone(),
                    proposed_repository_view_id: mutation.proposed_view.id().clone(),
                    new_repository_view_id: view.id().clone(),
                    old_generation: prepared.authority.repository_view_generation,
                    new_generation: generation,
                    layer_results: plan.layers.clone(),
                    event_id: if changed {
                        Nullable::some(event_id(sequence))
                    } else {
                        Nullable::null()
                    },
                };
                if changed {
                    events.push(SpaceEvent::RepositoryViewChanged {
                        sequence,
                        event_id: event_id(sequence),
                        actor: actor.clone(),
                        transaction_id: transaction_id.clone(),
                        payload: RepositoryViewChangedPayload {
                            mutation_id: result.mutation_id.clone(),
                            outcome: result.outcome,
                            old_repository_view_id: result.old_repository_view_id.clone(),
                            proposed_repository_view_id: result.proposed_repository_view_id.clone(),
                            new_repository_view_id: result.new_repository_view_id.clone(),
                            old_generation: result.old_generation,
                            new_generation: result.new_generation,
                            layer_results: result.layer_results.clone(),
                        },
                        created_at: prepared.timestamp.clone(),
                    });
                    sequence += 1;
                }
                receipt.view_result = Nullable::some(result);
            }
            if let (Some(mutation), Some((set, _))) =
                (&request.policy_root_mutation, &prepared.policies.proposed)
            {
                let changed = set.policy_set_id != prepared.authority.policy_set_id;
                let epoch = prepared
                    .authority
                    .authorization_epoch
                    .checked_add(u64::from(changed))
                    .ok_or_else(|| V3StoreError::Backend("authorization epoch overflow".into()))?;
                let result = PolicyRootResult {
                    mutation_id: mutation.mutation_id.clone(),
                    outcome: if changed {
                        PolicyRootOutcome::Changed
                    } else {
                        PolicyRootOutcome::NoOp
                    },
                    old_policy_set_id: prepared.authority.policy_set_id.clone(),
                    proposed_policy_set_id: set.policy_set_id.clone(),
                    new_policy_set_id: set.policy_set_id.clone(),
                    old_authorization_epoch: prepared.authority.authorization_epoch,
                    new_authorization_epoch: epoch,
                    event_id: if changed {
                        Nullable::some(event_id(sequence))
                    } else {
                        Nullable::null()
                    },
                };
                receipt
                    .accepted_policy_set_ids
                    .push(set.policy_set_id.clone());
                receipt.current_policy_set_id = set.policy_set_id.clone();
                receipt.resulting_authorization_epoch = epoch;
                if changed {
                    sqlx::query("INSERT INTO policy_root_history(space_id,authorization_epoch,policy_set_id) VALUES($1,$2,$3)").bind(request.space_id.as_str()).bind(i64_from_u64(epoch)?).bind(set.policy_set_id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
                    sqlx::query("UPDATE spaces SET current_policy_set_id=$2,authorization_epoch=$3 WHERE space_id=$1").bind(request.space_id.as_str()).bind(set.policy_set_id.as_str()).bind(i64_from_u64(epoch)?).execute(&mut **tx).await.map_err(pg_error)?;
                    events.push(SpaceEvent::PolicyRootChanged {
                        sequence,
                        event_id: event_id(sequence),
                        actor: actor.clone(),
                        transaction_id: transaction_id.clone(),
                        payload: PolicyRootChangedPayload {
                            mutation_id: mutation.mutation_id.clone(),
                            old_policy_set_id: result.old_policy_set_id.clone(),
                            new_policy_set_id: result.new_policy_set_id.clone(),
                            old_authorization_epoch: result.old_authorization_epoch,
                            new_authorization_epoch: epoch,
                        },
                        created_at: prepared.timestamp.clone(),
                    });
                    sequence += 1;
                }
                receipt.policy_root_result = Nullable::some(result);
            }
            receipt.event_cursor = Nullable::some(self.cursors.issue(
                &request.space_id,
                &principal.principal_id,
                receipt.resulting_authorization_epoch,
                sequence - 1,
            )?);
        }
        for event in &events {
            receipt.event_ids.push(match event {
                SpaceEvent::StepsAccepted { event_id, .. }
                | SpaceEvent::ProductsAccepted { event_id, .. }
                | SpaceEvent::LayerDefinitionChanged { event_id, .. }
                | SpaceEvent::RepositoryViewChanged { event_id, .. }
                | SpaceEvent::PolicyRootChanged { event_id, .. } => event_id.clone(),
            });
        }
        if !events.is_empty() {
            receipt.first_event_sequence = Nullable::some(next_sequence);
            receipt.last_event_sequence = Nullable::some(sequence - 1);
        }
        receipt
            .validate_v3()
            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
        let canonical_jcs = receipt.canonical_storage_bytes().map_err(json_error)?;
        sqlx::query("INSERT INTO sync_transactions(transaction_id,space_id,client_id,idempotency_key,request_digest,principal_id,authorization_snapshot_id,authorization_epoch,resulting_authorization_epoch,receipt_canonical_jcs,receipt_digest,first_event_sequence,last_event_sequence) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$13,$9,$10,$11,$12)")
            .bind(transaction_id.as_str()).bind(request.space_id.as_str()).bind(request.client_id.as_str()).bind(request.idempotency_key.as_str()).bind(request.request_digest.as_str()).bind(principal.principal_id.as_str()).bind(snapshot.authorization_snapshot_id.as_str()).bind(i64_from_u64(snapshot.authorization_epoch)?).bind(std::str::from_utf8(&canonical_jcs).map_err(|e|V3StoreError::Backend(e.to_string()))?).bind(receipt.computed_storage_digest().map_err(json_error)?.as_str()).bind(receipt.first_event_sequence.0.map(|v|v as i64)).bind(receipt.last_event_sequence.0.map(|v|v as i64)).bind(i64_from_u64(receipt.resulting_authorization_epoch)?).execute(&mut **tx).await.map_err(pg_error)?;
        if receipt.status == ReceiptStatus::Applied {
            if let Some(candidate) = scope.and_then(|scope| scope.candidate.as_ref()) {
                let source = &candidate.source_request;
                let step = request
                    .steps
                    .first()
                    .ok_or(V3StoreError::IdentityMismatch)?;
                if request.steps.len() != 1
                    || request.view_mutation.is_some()
                    || !receipt.accepted_step_ids.iter().all(|id| id == step.id())
                {
                    return Err(V3StoreError::IdentityMismatch);
                }
                let changed_paths: Vec<_> =
                    source.changes.iter().map(|change| &change.path).collect();
                sqlx::query("INSERT INTO layrs_collaboration.scoped_patch_candidates(space_id,candidate_id,candidate_step_id,candidate_root_tree_id,source_request_digest,capture_digest,author_principal_id,layer_id,base_step_id,base_root_tree_id,expected_view_id,expected_policy_set_id,expected_authorization_epoch,changed_paths_jcs,transaction_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
                    .bind(source.space_id.as_str()).bind(&candidate.candidate_id).bind(step.id().as_str()).bind(step.root_tree_id().as_str()).bind(source.request_digest.as_str()).bind(source.capture_digest.as_str()).bind(principal.principal_id.as_str()).bind(source.layer_id.as_str()).bind(source.base_step_id.as_str()).bind(source.base_root_tree_id.as_str()).bind(source.expected_repository_view_id.as_str()).bind(source.expected_policy_set_id.as_str()).bind(i64_from_u64(source.expected_authorization_epoch)?).bind(jcs(&changed_paths)?).bind(transaction_id.as_str())
                    .execute(&mut **tx).await.map_err(pg_error)?;
            }
        }
        if receipt.status == ReceiptStatus::Applied {
            super::super::transaction_products::install_layer_definitions(
                tx,
                &request.space_id,
                &prepared.products,
                &transaction_id,
            )
            .await?;
            if scope.and_then(|scope| scope.integration.as_ref()).is_none() {
                super::super::PgCollaborationStore::new(self.repository.clone())
                    .record_review_integrations(tx, principal, &receipt)
                    .await?;
            }
            if let Some(binding) = scope.and_then(|scope| scope.integration.as_ref()) {
                super::super::PgCollaborationStore::new(self.repository.clone())
                    .record_scoped_integration(
                        tx,
                        principal,
                        &prepared.authority,
                        &binding.review_id,
                        &binding.candidate_id,
                        &receipt,
                    )
                    .await?;
            }
        }
        if let Some(result) = receipt
            .view_result
            .as_ref()
            .filter(|r| r.new_generation != r.old_generation)
        {
            sqlx::query("UPDATE spaces SET current_repository_view_id=$2,current_repository_view_generation=$3 WHERE space_id=$1").bind(request.space_id.as_str()).bind(result.new_repository_view_id.as_str()).bind(i64_from_u64(result.new_generation)?).execute(&mut **tx).await.map_err(pg_error)?;
            sqlx::query("INSERT INTO repository_view_history(space_id,generation,repository_view_id,transaction_id) VALUES($1,$2,$3,$4)").bind(request.space_id.as_str()).bind(i64_from_u64(result.new_generation)?).bind(result.new_repository_view_id.as_str()).bind(transaction_id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
            sqlx::query("DELETE FROM current_layer_heads_projection WHERE space_id=$1")
                .bind(request.space_id.as_str())
                .execute(&mut **tx)
                .await
                .map_err(pg_error)?;
            sqlx::query("INSERT INTO current_layer_heads_projection(space_id,repository_view_id,repository_view_generation,layer_id,step_id) SELECT space_id,repository_view_id,$3,layer_id,step_id FROM repository_view_layer_heads WHERE space_id=$1 AND repository_view_id=$2").bind(request.space_id.as_str()).bind(result.new_repository_view_id.as_str()).bind(i64_from_u64(result.new_generation)?).execute(&mut **tx).await.map_err(pg_error)?;
        }
        for event in &events {
            let (id, kind) = match event {
                SpaceEvent::StepsAccepted { event_id, .. } => (event_id, "steps.accepted"),
                SpaceEvent::ProductsAccepted { event_id, .. } => (event_id, "products.accepted"),
                SpaceEvent::LayerDefinitionChanged { event_id, .. } => {
                    (event_id, "layer.definition_changed")
                }
                SpaceEvent::RepositoryViewChanged { event_id, .. } => {
                    (event_id, "repository_view.changed")
                }
                SpaceEvent::PolicyRootChanged { event_id, .. } => (event_id, "policy.root_changed"),
            };
            sqlx::query("INSERT INTO space_events(space_id,sequence,event_id,transaction_id,event_kind,actor_kind,actor_id,payload_canonical_jcs,created_at) VALUES($1,$2,$3,$4,$5,'account',$6,$7,$8::timestamptz)").bind(request.space_id.as_str()).bind(i64_from_u64(event.sequence())?).bind(id.as_str()).bind(transaction_id.as_str()).bind(kind).bind(principal.actor_id.as_str()).bind(jcs(event)?).bind(prepared.timestamp.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
        if !events.is_empty() {
            sqlx::query("UPDATE spaces SET next_event_sequence=$2 WHERE space_id=$1")
                .bind(request.space_id.as_str())
                .bind(i64_from_u64(sequence)?)
                .execute(&mut **tx)
                .await
                .map_err(pg_error)?;
        }
        Ok(PersistedReceipt {
            receipt,
            canonical_jcs,
        })
    }
}
