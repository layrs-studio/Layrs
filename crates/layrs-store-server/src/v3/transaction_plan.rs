//! Deterministic View transition independent of PostgreSQL and CAS.
use layrs_core::scm::{LayerHeadSet, LayerId, ObjectId, RepositoryView, StepId};
use layrs_graph::scm::{StepDag, normalize_heads, validate_repository_view};
use layrs_sync::v3::{
    DecisionRecord, LayerMutationMode, LayerMutationResult, Nullable, Rejection, RejectionCode,
    RepositoryViewMutation, ViewOutcome,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn reject(code: RejectionCode, message: impl Into<String>) -> Rejection {
    Rejection {
        code,
        message: message.into(),
        target_id: None,
    }
}

pub(crate) struct PlannedView {
    pub view: RepositoryView,
    pub layers: Vec<LayerMutationResult>,
    pub outcome: ViewOutcome,
}

#[cfg(test)]
fn plan_view(
    graph: &StepDag,
    current: &RepositoryView,
    generation: u64,
    expected: &RepositoryView,
    proposed: &RepositoryView,
    mutation: &RepositoryViewMutation,
) -> Result<PlannedView, Rejection> {
    plan_view_with_decisions(
        graph,
        current,
        generation,
        expected,
        proposed,
        mutation,
        &BTreeMap::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_view_with_decisions(
    graph: &StepDag,
    current: &RepositoryView,
    generation: u64,
    expected: &RepositoryView,
    proposed: &RepositoryView,
    mutation: &RepositoryViewMutation,
    decisions: &BTreeMap<ObjectId, DecisionRecord>,
) -> Result<PlannedView, Rejection> {
    let invalid = |message| reject(RejectionCode::InvalidViewMutation, message);
    if current.space_id() != expected.space_id()
        || current.space_id() != proposed.space_id()
        || !proposed.worktrees().is_empty()
        || !current.worktrees().is_empty()
    {
        return Err(invalid("View scope or Worktree state is invalid"));
    }
    validate_repository_view(graph, proposed)
        .map_err(|_| invalid("proposed heads must be known antichains"))?;
    let heads = |view: &RepositoryView| -> BTreeMap<LayerId, Vec<StepId>> {
        view.layer_heads()
            .iter()
            .map(|h| (h.layer_id().clone(), h.step_ids().to_vec()))
            .collect()
    };
    let before = heads(expected);
    let wanted = heads(proposed);
    let mut result = heads(current);
    // Removing an empty Layer is still a reference mutation, never an implicit omission.
    if before.keys().any(|id| !wanted.contains_key(id)) {
        return Err(invalid(
            "Layer removal requires a separate explicit lifecycle operation",
        ));
    }
    if expected
        .marker_ids()
        .iter()
        .any(|id| !proposed.marker_ids().contains(id))
        || expected
            .release_ids()
            .iter()
            .any(|id| !proposed.release_ids().contains(id))
    {
        return Err(invalid(
            "published named references cannot be removed implicitly",
        ));
    }
    let changed: BTreeSet<_> = wanted
        .iter()
        .filter(|(id, h)| before.get(*id) != Some(*h))
        .map(|(id, _)| id.clone())
        .collect();
    let declared: BTreeSet<_> = mutation
        .layer_mutations
        .iter()
        .map(|m| m.layer_id.clone())
        .collect();
    if declared != changed || declared.len() != mutation.layer_mutations.len() {
        return Err(invalid(
            "Layer intents must exactly match proposed differences",
        ));
    }
    let stale = mutation.expected_generation != generation
        || mutation.expected_current_view_id != *current.id();
    if stale
        && (mutation.layer_mutations.is_empty()
            || mutation
                .layer_mutations
                .iter()
                .any(|m| m.mode != LayerMutationMode::Integrate))
    {
        return Err(reject(
            RejectionCode::ViewGenerationConflict,
            "expected View/generation is no longer current",
        ));
    }
    let mut layers = Vec::new();
    for intent in &mutation.layer_mutations {
        let observed = before.get(&intent.layer_id).cloned().unwrap_or_default();
        let target = wanted.get(&intent.layer_id).cloned().unwrap_or_default();
        let previous = result.get(&intent.layer_id).cloned().unwrap_or_default();
        let covered = |a: &StepId, b: &StepId| graph.is_ancestor_or_equal(a, b).unwrap_or(false);
        let resulting = match intent.mode {
            LayerMutationMode::Advance => {
                if previous
                    .iter()
                    .any(|old| !target.iter().any(|new| covered(old, new)))
                {
                    return Err(invalid("advance would discard a current head"));
                }
                target.clone()
            }
            LayerMutationMode::Integrate => {
                if !observed.is_empty()
                    && target
                        .iter()
                        .any(|new| !observed.iter().any(|old| covered(old, new)))
                {
                    return Err(invalid(
                        "integration head does not descend from observed history",
                    ));
                }
                let union: BTreeSet<_> = previous.iter().chain(&target).cloned().collect();
                normalize_heads(graph, &union.into_iter().collect::<Vec<_>>())
                    .map_err(|_| invalid("invalid integration heads"))?
                    .into_vec()
            }
            LayerMutationMode::Choose => {
                let record = intent
                    .decision_product_object_id
                    .as_ref()
                    .and_then(|id| decisions.get(id))
                    .ok_or_else(|| {
                        invalid("choose requires an exact verified Decision Product object")
                    })?;
                let discarded: Vec<_> = previous
                    .iter()
                    .filter(|old| !target.iter().any(|new| covered(old, new)))
                    .cloned()
                    .collect();
                if record.space_id != *current.space_id()
                    || record.layer_id != intent.layer_id
                    || record.chosen_head_step_ids != target
                    || record.discarded_head_step_ids != discarded
                {
                    return Err(invalid(
                        "Decision must name the exact Space, Layer, chosen heads and actually discarded current heads",
                    ));
                }
                target.clone()
            }
        };
        layers.push(LayerMutationResult {
            layer_id: intent.layer_id.clone(),
            mode: intent.mode,
            divergence_created: previous.len() <= 1 && resulting.len() > 1,
            previous_heads: previous,
            proposed_heads: target,
            resulting_heads: resulting.clone(),
            decision_product_object_id: Nullable(intent.decision_product_object_id.clone()),
        });
        result.insert(intent.layer_id.clone(), resulting);
    }
    let markers: BTreeSet<_> = current
        .marker_ids()
        .iter()
        .chain(proposed.marker_ids())
        .cloned()
        .collect();
    let releases: BTreeSet<_> = current
        .release_ids()
        .iter()
        .chain(proposed.release_ids())
        .cloned()
        .collect();
    let view = RepositoryView::new(
        current.space_id().clone(),
        result
            .into_iter()
            .map(|(id, heads)| LayerHeadSet::new(id, heads).expect("validated unique heads"))
            .collect(),
        markers.into_iter().collect(),
        releases.into_iter().collect(),
        vec![],
    )
    .map_err(|_| invalid("invalid resulting View"))?;
    let outcome = if view.id() == current.id() {
        ViewOutcome::NoOp
    } else if layers.iter().all(|l| l.mode == LayerMutationMode::Advance) {
        ViewOutcome::Advanced
    } else if layers
        .iter()
        .all(|l| l.mode == LayerMutationMode::Integrate)
    {
        ViewOutcome::Integrated
    } else if layers.iter().all(|l| l.mode == LayerMutationMode::Choose) {
        ViewOutcome::Chosen
    } else {
        ViewOutcome::Mixed
    };
    Ok(PlannedView {
        view,
        layers,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{MarkerId, SpaceId, Step, TreeId};
    use layrs_sync::v3::{LayerMutationIntent, MutationId, ProposedRepositoryView};
    fn step(seed: u64, parents: &[&Step]) -> Step {
        Step::new(
            parents.iter().map(|s| s.id().clone()).collect(),
            TreeId::parse(format!("blake3:{seed:064x}")).unwrap(),
        )
        .unwrap()
    }
    fn view(heads: &[&Step]) -> RepositoryView {
        RepositoryView::new(
            SpaceId::parse("space:a").unwrap(),
            vec![
                LayerHeadSet::new(
                    LayerId::parse("layer:main").unwrap(),
                    heads.iter().map(|s| s.id().clone()).collect(),
                )
                .unwrap(),
            ],
            vec![],
            vec![],
            vec![],
        )
        .unwrap()
    }
    fn mutation(
        expected: &RepositoryView,
        proposed: &RepositoryView,
        mode: LayerMutationMode,
    ) -> RepositoryViewMutation {
        RepositoryViewMutation {
            mutation_id: MutationId::parse("mutation:test").unwrap(),
            expected_current_view_id: expected.id().clone(),
            expected_generation: 1,
            proposed_view: ProposedRepositoryView::Inline {
                view: proposed.clone(),
            },
            layer_mutations: vec![LayerMutationIntent {
                layer_id: LayerId::parse("layer:main").unwrap(),
                mode,
                decision_product_object_id: None,
            }],
            expected_policy_version_object_ids: vec![],
        }
    }
    #[test]
    fn choose_binds_exact_discard_and_does_not_discard_a_retained_ancestor() {
        use layrs_core::scm::{ActorId, Timestamp};
        use layrs_sync::v3::{DecisionId, DecisionSchemaVersion};
        let base = step(1, &[]);
        let left = step(2, &[&base]);
        let right = step(3, &[&base]);
        let child = step(4, &[&left]);
        let graph = StepDag::from_steps([&base, &left, &right, &child]).unwrap();
        let current = view(&[&left, &right]);
        let proposed = view(&[&child]);
        let mut intent = mutation(&current, &proposed, LayerMutationMode::Choose);
        let id = ObjectId::parse(format!("blake3:{:064x}", 99)).unwrap();
        intent.layer_mutations[0].decision_product_object_id = Some(id.clone());
        let record = DecisionRecord {
            schema_version: DecisionSchemaVersion::V1,
            decision_id: DecisionId::parse("decision:test").unwrap(),
            space_id: current.space_id().clone(),
            layer_id: LayerId::parse("layer:main").unwrap(),
            discarded_head_step_ids: vec![right.id().clone()],
            chosen_head_step_ids: vec![child.id().clone()],
            reason: "Retain left through its descendant".into(),
            decided_by: ActorId::parse("actor:admin").unwrap(),
            decided_at: Timestamp::parse("2026-09-20T00:00:00Z").unwrap(),
        };
        let mut records = BTreeMap::from([(id.clone(), record)]);
        let plan =
            plan_view_with_decisions(&graph, &current, 1, &current, &proposed, &intent, &records)
                .unwrap();
        assert_eq!(plan.outcome, ViewOutcome::Chosen);
        assert_eq!(plan.view, proposed);
        assert_eq!(
            plan.layers[0].decision_product_object_id.as_ref(),
            Some(&id)
        );
        assert!(
            plan_view_with_decisions(&graph, &current, 2, &current, &proposed, &intent, &records)
                .is_err()
        );
        records.get_mut(&id).unwrap().discarded_head_step_ids =
            current.layer_heads()[0].step_ids().to_vec();
        assert!(
            plan_view_with_decisions(&graph, &current, 1, &current, &proposed, &intent, &records)
                .is_err()
        );
        let empty = view(&[]);
        intent.proposed_view = ProposedRepositoryView::Inline {
            view: empty.clone(),
        };
        records.get_mut(&id).unwrap().chosen_head_step_ids.clear();
        assert_eq!(
            plan_view_with_decisions(&graph, &current, 1, &current, &empty, &intent, &records)
                .unwrap()
                .view,
            empty
        );
        records.get_mut(&id).unwrap().layer_id = LayerId::parse("layer:wrong").unwrap();
        assert!(
            plan_view_with_decisions(&graph, &current, 1, &current, &empty, &intent, &records)
                .is_err()
        );
    }
    #[test]
    fn advance_preserves_ancestry_and_refuses_discard() {
        let root = step(1, &[]);
        let next = step(2, &[&root]);
        let other = step(3, &[]);
        let graph = StepDag::from_steps([&root, &next, &other]).unwrap();
        let current = view(&[&root]);
        let proposed = view(&[&next]);
        assert_eq!(
            plan_view(
                &graph,
                &current,
                1,
                &current,
                &proposed,
                &mutation(&current, &proposed, LayerMutationMode::Advance)
            )
            .unwrap()
            .view,
            proposed
        );
        let invalid = view(&[&other]);
        assert!(
            plan_view(
                &graph,
                &current,
                1,
                &current,
                &invalid,
                &mutation(&current, &invalid, LayerMutationMode::Advance)
            )
            .is_err()
        );
    }
    #[test]
    fn reference_only_requires_exact_cas_and_cannot_hide_heads_or_removals() {
        use layrs_sync::v3::ValidateV3;
        let root = step(1, &[]);
        let next = step(2, &[&root]);
        let graph = StepDag::from_steps([&root, &next]).unwrap();
        let current = view(&[&root]);
        let proposed = RepositoryView::new(
            current.space_id().clone(),
            current.layer_heads().to_vec(),
            vec![MarkerId::parse("marker:new").unwrap()],
            vec![],
            vec![],
        )
        .unwrap();
        let mut change = mutation(&current, &proposed, LayerMutationMode::Advance);
        change.layer_mutations.clear();
        change.validate_v3().unwrap();
        assert_eq!(
            plan_view(&graph, &current, 1, &current, &proposed, &change)
                .unwrap()
                .view,
            proposed
        );
        assert_eq!(
            plan_view(&graph, &current, 2, &current, &proposed, &change)
                .err()
                .unwrap()
                .code,
            RejectionCode::ViewGenerationConflict
        );
        assert!(plan_view(&graph, &current, 1, &current, &view(&[&next]), &change).is_err());
        let mut unchanged = change.clone();
        unchanged.proposed_view = ProposedRepositoryView::Inline {
            view: current.clone(),
        };
        assert!(unchanged.validate_v3().is_err());
        assert!(plan_view(&graph, &proposed, 1, &proposed, &current, &change).is_err());
    }
    #[test]
    fn stale_integration_keeps_concurrent_heads_and_named_points() {
        let root = step(1, &[]);
        let left = step(2, &[&root]);
        let right = step(3, &[&root]);
        let graph = StepDag::from_steps([&root, &left, &right]).unwrap();
        let expected = view(&[&root]);
        let proposed = view(&[&left]);
        let current = RepositoryView::new(
            expected.space_id().clone(),
            view(&[&right]).layer_heads().to_vec(),
            vec![MarkerId::parse("marker:concurrent").unwrap()],
            vec![],
            vec![],
        )
        .unwrap();
        let plan = plan_view(
            &graph,
            &current,
            2,
            &expected,
            &proposed,
            &mutation(&expected, &proposed, LayerMutationMode::Integrate),
        )
        .unwrap();
        assert_eq!(plan.view.layer_heads()[0].step_ids().len(), 2);
        assert_eq!(plan.view.marker_ids(), current.marker_ids());
        assert!(plan.layers[0].divergence_created);
    }
    #[test]
    fn stale_advance_and_unrelated_integration_are_rejected() {
        let root = step(1, &[]);
        let next = step(2, &[&root]);
        let other = step(3, &[]);
        let graph = StepDag::from_steps([&root, &next, &other]).unwrap();
        let current = view(&[&root]);
        let proposed = view(&[&next]);
        assert_eq!(
            plan_view(
                &graph,
                &current,
                2,
                &current,
                &proposed,
                &mutation(&current, &proposed, LayerMutationMode::Advance)
            )
            .err()
            .unwrap()
            .code,
            RejectionCode::ViewGenerationConflict
        );
        let invalid = view(&[&other]);
        assert!(
            plan_view(
                &graph,
                &current,
                1,
                &current,
                &invalid,
                &mutation(&current, &invalid, LayerMutationMode::Integrate)
            )
            .is_err()
        );
    }
    #[test]
    fn undeclared_mutations_and_non_antichains_are_rejected() {
        let root = step(1, &[]);
        let next = step(2, &[&root]);
        let graph = StepDag::from_steps([&root, &next]).unwrap();
        let current = view(&[&root]);
        let proposed = view(&[&next]);
        let mut request = mutation(&current, &proposed, LayerMutationMode::Advance);
        request.layer_mutations.clear();
        assert!(plan_view(&graph, &current, 1, &current, &proposed, &request).is_err());
        let invalid = view(&[&root, &next]);
        assert!(
            plan_view(
                &graph,
                &current,
                1,
                &current,
                &invalid,
                &mutation(&current, &invalid, LayerMutationMode::Advance)
            )
            .is_err()
        );
    }
    #[test]
    fn already_integrated_proposal_is_a_noop() {
        let root = step(1, &[]);
        let next = step(2, &[&root]);
        let graph = StepDag::from_steps([&root, &next]).unwrap();
        let expected = view(&[&root]);
        let current = view(&[&next]);
        let plan = plan_view(
            &graph,
            &current,
            2,
            &expected,
            &current,
            &mutation(&expected, &current, LayerMutationMode::Integrate),
        )
        .unwrap();
        assert_eq!(plan.outcome, ViewOutcome::NoOp);
        assert!(!plan.layers[0].divergence_created);
    }
}
