//! Administrative selection of published heads. The serializable preview is the
//! exact retryable intent; callers persist it before applying network effects.
use super::{remote::RemoteResult, NativeRemote, NativeRemoteError};
use layrs_core::scm::{
    ActorId, LayerHeadSet, LayerId, ObjectId, RepositoryView, RepositoryViewId, SpaceId, Step,
    StepId, Timestamp,
};
use layrs_graph::scm::{normalize_heads, StepDag};
use layrs_sync::v3::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeChoosePreview {
    pub origin: String,
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub expected_view_id: RepositoryViewId,
    pub expected_generation: u64,
    pub expected_view: RepositoryView,
    pub previous_heads: Vec<StepId>,
    pub chosen_heads: Vec<StepId>,
    pub discarded_heads: Vec<StepId>,
    pub decision_product_object_id: ObjectId,
    pub request: SyncTransactionRequest,
}
#[derive(Deserialize)]
struct Identity {
    principal_id: PrincipalId,
    actor_id: ActorId,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct History {
    anchor: StepId,
    steps: Vec<Step>,
    next: Option<String>,
}
fn invalid(message: &'static str) -> NativeRemoteError {
    NativeRemoteError::Invalid(message)
}
impl NativeRemote {
    pub fn preview_choose(
        &self,
        space: &SpaceId,
        layer: &LayerId,
        chosen: &[StepId],
        reason: &str,
    ) -> RemoteResult<NativeChoosePreview> {
        if reason.trim().is_empty() || reason.len() > 4096 || chosen.len() > 64 {
            return Err(invalid(
                "choose requires a reason <=4096 bytes and at most64 heads",
            ));
        }
        let identity: Identity = serde_json::from_value(self.whoami()?)?;
        let bootstrap = self.bootstrap(space)?;
        let previous = bootstrap
            .current_repository_view
            .record
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == layer)
            .ok_or_else(|| invalid("Layer not published in this Space"))?
            .step_ids()
            .to_vec();
        if previous.len() > 64 {
            return Err(invalid("choose preview supports at most64 current heads"));
        }
        let requested: BTreeSet<_> = chosen.iter().cloned().collect();
        if requested.len() != chosen.len() {
            return Err(invalid("duplicate chosen head"));
        }
        let roots: BTreeSet<_> = previous.iter().chain(chosen).cloned().collect();
        let mut steps = BTreeMap::new();
        let mut pages = 0usize;
        let mut bytes = 0usize;
        for anchor in roots {
            if steps.contains_key(&anchor) {
                continue;
            }
            let mut cursor = None;
            let mut cursors = BTreeSet::new();
            loop {
                pages += 1;
                if pages > 1000 {
                    return Err(invalid("choose history page limit exceeded"));
                }
                let page: History = self.post(
                    &format!("v3/spaces/{space}/history"),
                    &serde_json::json!({"anchor":anchor,"cursor":cursor,"limit":100}),
                )?;
                if page.anchor != anchor || page.steps.len() > 100 || page.steps.is_empty() {
                    return Err(invalid("invalid choose history page"));
                }
                for step in page.steps {
                    bytes = bytes
                        .checked_add(serde_json::to_vec(&step)?.len())
                        .ok_or_else(|| invalid("choose graph byte overflow"))?;
                    if bytes > 64 * 1024 * 1024 {
                        return Err(invalid("choose graph byte limit exceeded"));
                    }
                    if let Some(old) = steps.insert(step.id().clone(), step.clone()) {
                        if old != step {
                            return Err(invalid("inconsistent immutable Step"));
                        }
                    }
                    if steps.len() > 100_000 {
                        return Err(invalid("choose graph node limit exceeded"));
                    }
                }
                match page.next {
                    None => break,
                    Some(next) => {
                        if next.len() > 4096 || !cursors.insert(next.clone()) {
                            return Err(invalid("invalid choose history continuation"));
                        }
                        cursor = Some(next);
                    }
                }
            }
        }
        let graph = StepDag::from_steps(steps.values())
            .map_err(|_| invalid("choose history closure incomplete"))?;
        let chosen = normalize_heads(&graph, &requested.into_iter().collect::<Vec<_>>())
            .map_err(|_| invalid("invalid chosen heads"))?
            .into_vec();
        let discarded = previous
            .iter()
            .filter(|old| {
                !chosen
                    .iter()
                    .any(|new| graph.is_ancestor_or_equal(old, new).unwrap_or(false))
            })
            .cloned()
            .collect();
        build(
            self.origin(),
            identity,
            &bootstrap,
            layer,
            previous,
            chosen,
            discarded,
            reason,
        )
    }

    /// Persist the returned preview before calling this method. On transport
    /// failure retry the same preview; a fresh preview is a new user decision.
    pub fn apply_choose(
        &self,
        preview: &NativeChoosePreview,
    ) -> RemoteResult<SyncTransactionReceipt> {
        validate_preview(preview)?;
        let identity: Identity = serde_json::from_value(self.whoami()?)?;
        if self.origin() != preview.origin
            || identity.principal_id != preview.principal_id
            || identity.actor_id != preview.actor_id
        {
            return Err(invalid(
                "choose preview belongs to another account or origin",
            ));
        }
        let receipt: SyncTransactionReceipt = self.post(
            &format!("v3/spaces/{}/transactions", preview.space_id),
            &preview.request,
        )?;
        receipt
            .validate_v3()
            .map_err(|_| invalid("invalid choose receipt"))?;
        if receipt.space_id != preview.space_id
            || receipt.client_id != preview.request.client_id
            || receipt.request_digest != preview.request.request_digest
            || receipt.idempotency_key != preview.request.idempotency_key
        {
            return Err(invalid("choose receipt request binding mismatch"));
        }
        if receipt.status == ReceiptStatus::Applied {
            let result = receipt
                .view_result
                .as_ref()
                .ok_or_else(|| invalid("choose receipt has no View result"))?;
            if result.outcome != ViewOutcome::Chosen
                || result.old_repository_view_id != preview.expected_view_id
                || result.old_generation != preview.expected_generation
                || result.new_repository_view_id
                    != *preview
                        .request
                        .view_mutation
                        .as_ref()
                        .unwrap()
                        .proposed_view
                        .id()
                || result.layer_results.len() != 1
            {
                return Err(invalid("choose receipt View differs from preview"));
            }
            let change = &result.layer_results[0];
            if change.layer_id != preview.layer_id
                || change.mode != LayerMutationMode::Choose
                || change.previous_heads != preview.previous_heads
                || change.resulting_heads != preview.chosen_heads
                || change.decision_product_object_id.as_ref()
                    != Some(&preview.decision_product_object_id)
            {
                return Err(invalid("choose receipt heads differ from preview"));
            }
        }
        Ok(receipt)
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    origin: &str,
    identity: Identity,
    bootstrap: &SpaceBootstrap,
    layer: &LayerId,
    previous: Vec<StepId>,
    chosen: Vec<StepId>,
    discarded: Vec<StepId>,
    reason: &str,
) -> RemoteResult<NativeChoosePreview> {
    if previous == chosen {
        return Err(invalid("choose would not change published heads"));
    }
    let nonce = uuid::Uuid::new_v4().to_string();
    let decision = DecisionId::parse(format!("decision:{nonce}"))
        .map_err(|_| invalid("invalid Decision ID"))?;
    let timestamp = Timestamp::parse(
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| invalid("invalid clock"))?,
    )
    .map_err(|_| invalid("invalid timestamp"))?;
    let mut product = ProductEnvelope::Decision {
        product_id: decision.clone(),
        product_object_id: ObjectId::parse(format!("blake3:{}", "0".repeat(64))).unwrap(),
        depends_on_step_ids: chosen
            .iter()
            .chain(&discarded)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        depends_on_product_ids: vec![ProductLogicalId::parse(layer.as_str()).unwrap()],
        record: DecisionRecord {
            schema_version: DecisionSchemaVersion::V1,
            decision_id: decision,
            space_id: bootstrap.space_id.clone(),
            layer_id: layer.clone(),
            discarded_head_step_ids: discarded.clone(),
            chosen_head_step_ids: chosen.clone(),
            reason: reason.into(),
            decided_by: identity.actor_id.clone(),
            decided_at: timestamp,
        },
    };
    let object = product
        .computed_object_id()
        .map_err(|_| invalid("invalid Decision identity"))?;
    if let ProductEnvelope::Decision {
        product_object_id, ..
    } = &mut product
    {
        *product_object_id = object.clone();
    }
    let mut heads = bootstrap
        .current_repository_view
        .record
        .layer_heads()
        .to_vec();
    let selected = heads
        .iter_mut()
        .find(|h| h.layer_id() == layer)
        .ok_or_else(|| invalid("unknown Layer"))?;
    *selected = LayerHeadSet::new(layer.clone(), chosen.clone())
        .map_err(|_| invalid("invalid chosen heads"))?;
    let view = RepositoryView::new(
        bootstrap.space_id.clone(),
        heads,
        bootstrap
            .current_repository_view
            .record
            .marker_ids()
            .to_vec(),
        bootstrap
            .current_repository_view
            .record
            .release_ids()
            .to_vec(),
        vec![],
    )
    .map_err(|_| invalid("invalid proposed View"))?;
    let mut request = SyncTransactionRequest {
        protocol: SyncV3Protocol::V3,
        message_type: TransactionRequestType::Request,
        space_id: bootstrap.space_id.clone(),
        client_id: ClientId::parse(format!("client:choose-{nonce}")).unwrap(),
        idempotency_key: IdempotencyKey::parse(format!("choose-{nonce}")).unwrap(),
        request_digest: object.clone(),
        known_event_cursor: Some(bootstrap.watermark_cursor.clone()),
        steps: vec![],
        observations: vec![],
        products: vec![product],
        view_mutation: Some(RepositoryViewMutation {
            mutation_id: MutationId::parse(format!("mutation:choose-{nonce}")).unwrap(),
            expected_current_view_id: bootstrap.current_repository_view.repository_view_id.clone(),
            expected_generation: bootstrap.current_repository_view_generation,
            proposed_view: ProposedRepositoryView::Inline { view },
            layer_mutations: vec![LayerMutationIntent {
                layer_id: layer.clone(),
                mode: LayerMutationMode::Choose,
                decision_product_object_id: Some(object.clone()),
            }],
            expected_policy_version_object_ids: captured_policy_version_object_ids(
                &bootstrap.current_policy_set.record,
            )
            .map_err(|_| invalid("invalid policy set"))?,
        }),
        policy_root_mutation: None,
    };
    request.request_digest = request
        .computed_request_digest()
        .map_err(|_| invalid("invalid choose digest"))?;
    let preview = NativeChoosePreview {
        origin: origin.into(),
        principal_id: identity.principal_id,
        actor_id: identity.actor_id,
        space_id: bootstrap.space_id.clone(),
        layer_id: layer.clone(),
        expected_view_id: bootstrap.current_repository_view.repository_view_id.clone(),
        expected_generation: bootstrap.current_repository_view_generation,
        expected_view: bootstrap.current_repository_view.record.clone(),
        previous_heads: previous,
        chosen_heads: chosen,
        discarded_heads: discarded,
        decision_product_object_id: object,
        request,
    };
    validate_preview(&preview)?;
    Ok(preview)
}

fn validate_preview(preview: &NativeChoosePreview) -> RemoteResult<()> {
    let request = &preview.request;
    request
        .validate_v3()
        .map_err(|_| invalid("invalid choose preview request"))?;
    if request
        .computed_request_digest()
        .map_err(|_| invalid("invalid choose digest"))?
        != request.request_digest
        || request.space_id != preview.space_id
        || request.products.len() != 1
        || !request.steps.is_empty()
        || !request.observations.is_empty()
        || request.policy_root_mutation.is_some()
    {
        return Err(invalid("choose preview request mismatch"));
    }
    let product = &request.products[0];
    let record = product
        .decision_record()
        .ok_or_else(|| invalid("choose preview requires Decision"))?;
    if product.object_id() != &preview.decision_product_object_id
        || record.decided_by != preview.actor_id
        || record.layer_id != preview.layer_id
        || record.space_id != preview.space_id
        || record.chosen_head_step_ids != preview.chosen_heads
        || record.discarded_head_step_ids != preview.discarded_heads
    {
        return Err(invalid("choose preview impact mismatch"));
    }
    let mutation = request
        .view_mutation
        .as_ref()
        .ok_or_else(|| invalid("missing choose mutation"))?;
    if preview.expected_view.id() != &preview.expected_view_id
        || preview.expected_view.space_id() != &preview.space_id
        || preview
            .expected_view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == &preview.layer_id)
            .map(|h| h.step_ids())
            != Some(preview.previous_heads.as_slice())
    {
        return Err(invalid("choose previous heads do not match anchored View"));
    }
    let mut heads = preview.expected_view.layer_heads().to_vec();
    *heads
        .iter_mut()
        .find(|h| h.layer_id() == &preview.layer_id)
        .unwrap() = LayerHeadSet::new(preview.layer_id.clone(), preview.chosen_heads.clone())
        .map_err(|_| invalid("invalid choose heads"))?;
    let expected = RepositoryView::new(
        preview.space_id.clone(),
        heads,
        preview.expected_view.marker_ids().to_vec(),
        preview.expected_view.release_ids().to_vec(),
        vec![],
    )
    .map_err(|_| invalid("invalid choose View"))?;
    if mutation.proposed_view.id() != expected.id() {
        return Err(invalid("choose preview changes unrelated references"));
    }
    if mutation.expected_current_view_id != preview.expected_view_id
        || mutation.expected_generation != preview.expected_generation
        || mutation.layer_mutations.len() != 1
        || mutation.layer_mutations[0].mode != LayerMutationMode::Choose
        || mutation.layer_mutations[0].layer_id != preview.layer_id
        || mutation.layer_mutations[0]
            .decision_product_object_id
            .as_ref()
            != Some(&preview.decision_product_object_id)
    {
        return Err(invalid("choose preview expectation mismatch"));
    }
    Ok(())
}
