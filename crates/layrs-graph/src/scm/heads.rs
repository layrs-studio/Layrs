use super::{ScmGraphError, StepDag};
use layrs_core::scm::{RepositoryView, StepId};
use std::collections::BTreeSet;

/// A sorted, duplicate-free Step antichain.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NormalizedHeads {
    heads: Vec<StepId>,
}

impl NormalizedHeads {
    pub fn as_slice(&self) -> &[StepId] {
        &self.heads
    }

    pub fn into_vec(self) -> Vec<StepId> {
        self.heads
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &StepId> {
        self.heads.iter()
    }

    pub fn len(&self) -> usize {
        self.heads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }
}

/// Validates that `heads` is a duplicate-free antichain of known Steps.
pub fn validate_heads(graph: &StepDag, heads: &[StepId]) -> Result<(), ScmGraphError> {
    let sorted = checked_heads(graph, heads)?;
    for (index, first) in sorted.iter().enumerate() {
        for second in sorted.iter().skip(index + 1) {
            if graph.is_ancestor(first, second)? {
                return Err(ScmGraphError::LayerHeadsNotAntichain {
                    ancestor: first.clone(),
                    descendant: second.clone(),
                });
            }
            if graph.is_ancestor(second, first)? {
                return Err(ScmGraphError::LayerHeadsNotAntichain {
                    ancestor: second.clone(),
                    descendant: first.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Removes only redundant ancestor heads. Divergent heads are preserved.
pub fn normalize_heads(
    graph: &StepDag,
    heads: &[StepId],
) -> Result<NormalizedHeads, ScmGraphError> {
    let sorted = checked_heads(graph, heads)?;
    normalize_checked(graph, sorted)
}

/// Integrates an incoming head set with an already normalized one. A head
/// present in both sets is idempotent; duplicates inside `incoming` are not.
pub fn integrate_heads(
    graph: &StepDag,
    current: &NormalizedHeads,
    incoming: &[StepId],
) -> Result<NormalizedHeads, ScmGraphError> {
    validate_heads(graph, current.as_slice())?;
    let incoming = checked_heads(graph, incoming)?;
    let union: BTreeSet<_> = current.iter().cloned().chain(incoming).collect();
    normalize_checked(graph, union.into_iter().collect())
}

/// Validates each Layer head set in a RepositoryView against the Step DAG.
pub fn validate_repository_view(
    graph: &StepDag,
    view: &RepositoryView,
) -> Result<(), ScmGraphError> {
    let mut layers = BTreeSet::new();
    for head_set in view.layer_heads() {
        if !layers.insert(head_set.layer_id().clone()) {
            return Err(ScmGraphError::DuplicateLayerHeadSet(
                head_set.layer_id().clone(),
            ));
        }
        validate_heads(graph, head_set.step_ids())?;
    }
    Ok(())
}

fn checked_heads(graph: &StepDag, heads: &[StepId]) -> Result<Vec<StepId>, ScmGraphError> {
    let mut sorted = BTreeSet::new();
    for head in heads {
        if !graph.contains(head) {
            return Err(ScmGraphError::MissingStep {
                step_id: head.clone(),
                referenced_id: head.clone(),
            });
        }
        if !sorted.insert(head.clone()) {
            return Err(ScmGraphError::DuplicateHead(head.clone()));
        }
    }
    Ok(sorted.into_iter().collect())
}

fn normalize_checked(
    graph: &StepDag,
    sorted: Vec<StepId>,
) -> Result<NormalizedHeads, ScmGraphError> {
    let mut result = Vec::new();
    for candidate in &sorted {
        let mut redundant = false;
        for other in &sorted {
            if candidate != other && graph.is_ancestor(candidate, other)? {
                redundant = true;
                break;
            }
        }
        if !redundant {
            result.push(candidate.clone());
        }
    }
    Ok(NormalizedHeads { heads: result })
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{LayerHeadSet, LayerId, SpaceId, Step, TreeId};

    fn oid(seed: u64) -> String {
        format!("blake3:{seed:064x}")
    }

    fn step(parents: &[&Step], seed: u64) -> Step {
        Step::new(
            parents.iter().map(|parent| parent.id().clone()).collect(),
            TreeId::parse(oid(seed)).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn normalization_is_idempotent_and_preserves_divergence() {
        let root = step(&[], 1);
        let left = step(&[&root], 2);
        let right = step(&[&root], 3);
        let graph = StepDag::from_steps([&right, &root, &left]).unwrap();

        let normalized = normalize_heads(
            &graph,
            &[root.id().clone(), left.id().clone(), right.id().clone()],
        )
        .unwrap();
        assert_eq!(normalized.len(), 2);
        assert!(normalized.as_slice().contains(left.id()));
        assert!(normalized.as_slice().contains(right.id()));
        assert_eq!(
            normalize_heads(&graph, normalized.as_slice()).unwrap(),
            normalized
        );
        assert!(validate_heads(&graph, normalized.as_slice()).is_ok());
    }

    #[test]
    fn integration_is_idempotent_and_never_drops_divergence() {
        let root = step(&[], 1);
        let left = step(&[&root], 2);
        let right = step(&[&root], 3);
        let graph = StepDag::from_steps([&root, &left, &right]).unwrap();
        let current = normalize_heads(&graph, &[left.id().clone()]).unwrap();

        let divergent = integrate_heads(&graph, &current, &[right.id().clone()]).unwrap();
        assert_eq!(divergent.len(), 2);
        assert_eq!(
            integrate_heads(&graph, &divergent, &[right.id().clone()]).unwrap(),
            divergent
        );
    }

    #[test]
    fn duplicate_missing_and_non_antichain_heads_are_rejected() {
        let root = step(&[], 1);
        let child = step(&[&root], 2);
        let graph = StepDag::from_steps([&root, &child]).unwrap();
        assert!(matches!(
            validate_heads(&graph, &[root.id().clone(), root.id().clone()]),
            Err(ScmGraphError::DuplicateHead(_))
        ));
        let missing = StepId::parse(oid(99)).unwrap();
        assert!(matches!(
            validate_heads(&graph, &[missing]),
            Err(ScmGraphError::MissingStep { .. })
        ));
        let error = validate_heads(&graph, &[root.id().clone(), child.id().clone()]).unwrap_err();
        assert_eq!(error.code(), "layer_heads_not_antichain");
    }

    #[test]
    fn repository_view_is_validated_per_layer() {
        let root = step(&[], 1);
        let child = step(&[&root], 2);
        let graph = StepDag::from_steps([&root, &child]).unwrap();
        let view = RepositoryView::new(
            SpaceId::parse("space:test").unwrap(),
            vec![
                LayerHeadSet::new(
                    LayerId::parse("layer:test").unwrap(),
                    vec![root.id().clone(), child.id().clone()],
                )
                .unwrap(),
            ],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        assert_eq!(
            validate_repository_view(&graph, &view).unwrap_err().code(),
            "layer_heads_not_antichain"
        );
    }
}
