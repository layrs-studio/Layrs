use super::error::{GraphObjectKind, ScmGraphError};
use layrs_core::scm::{Step, StepId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Default)]
struct StepNode {
    parents: BTreeSet<StepId>,
    children: BTreeSet<StepId>,
}

/// An in-memory, deterministic index of the immutable Step DAG.
#[derive(Debug, Clone, Default)]
pub struct StepDag {
    nodes: BTreeMap<StepId, StepNode>,
}

impl StepDag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_steps<'a>(
        steps: impl IntoIterator<Item = &'a Step>,
    ) -> Result<Self, ScmGraphError> {
        let mut graph = Self::new();
        graph.insert_batch(steps)?;
        Ok(graph)
    }

    /// Atomically inserts a batch. Parent references may point forward within
    /// the batch, but every parent must exist in the resulting graph.
    pub fn insert_batch<'a>(
        &mut self,
        steps: impl IntoIterator<Item = &'a Step>,
    ) -> Result<(), ScmGraphError> {
        let mut specifications = BTreeMap::<StepId, BTreeSet<StepId>>::new();
        for step in steps {
            let id = step.id().clone();
            if self.nodes.contains_key(&id) || specifications.contains_key(&id) {
                return Err(ScmGraphError::DuplicateStep(id));
            }
            let parents: BTreeSet<_> = step.parent_step_ids().iter().cloned().collect();
            if parents.len() != step.parent_step_ids().len() {
                let duplicate = first_duplicate(step.parent_step_ids()).ok_or(
                    ScmGraphError::InternalInvariant("duplicate Step parent has no witness"),
                )?;
                return Err(ScmGraphError::DuplicateStepParent {
                    step_id: id,
                    parent_id: duplicate,
                });
            }
            specifications.insert(id, parents);
        }

        for (id, parents) in &specifications {
            for parent in parents {
                if parent == id {
                    return Err(ScmGraphError::SelfParent {
                        kind: GraphObjectKind::Step,
                        id: id.to_string(),
                    });
                }
                if !self.nodes.contains_key(parent) && !specifications.contains_key(parent) {
                    return Err(ScmGraphError::MissingStep {
                        step_id: id.clone(),
                        referenced_id: parent.clone(),
                    });
                }
            }
        }

        let mut candidate = self.clone();
        for id in specifications.keys() {
            candidate.nodes.insert(id.clone(), StepNode::default());
        }
        for (id, parents) in specifications {
            let node = candidate
                .nodes
                .get_mut(&id)
                .ok_or(ScmGraphError::InternalInvariant(
                    "inserted Step disappeared",
                ))?;
            node.parents = parents.clone();
            for parent in parents {
                candidate
                    .nodes
                    .get_mut(&parent)
                    .ok_or(ScmGraphError::InternalInvariant(
                        "validated parent disappeared",
                    ))?
                    .children
                    .insert(id.clone());
            }
        }
        candidate.topological()?;
        *self = candidate;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn contains(&self, id: &StepId) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn parents(&self, id: &StepId) -> Result<Vec<StepId>, ScmGraphError> {
        self.node(id)
            .map(|node| node.parents.iter().cloned().collect())
    }

    pub fn children(&self, id: &StepId) -> Result<Vec<StepId>, ScmGraphError> {
        self.node(id)
            .map(|node| node.children.iter().cloned().collect())
    }

    pub fn is_ancestor(
        &self,
        ancestor: &StepId,
        descendant: &StepId,
    ) -> Result<bool, ScmGraphError> {
        self.node(ancestor)?;
        self.node(descendant)?;
        if ancestor == descendant {
            return Ok(false);
        }
        let mut visited = BTreeSet::new();
        let mut pending = VecDeque::from([descendant.clone()]);
        while let Some(current) = pending.pop_front() {
            for parent in &self.node(&current)?.parents {
                if parent == ancestor {
                    return Ok(true);
                }
                if visited.insert(parent.clone()) {
                    pending.push_back(parent.clone());
                }
            }
        }
        Ok(false)
    }

    pub fn is_ancestor_or_equal(
        &self,
        ancestor: &StepId,
        descendant: &StepId,
    ) -> Result<bool, ScmGraphError> {
        self.node(ancestor)?;
        self.node(descendant)?;
        Ok(ancestor == descendant || self.is_ancestor(ancestor, descendant)?)
    }

    pub fn ancestors(&self, id: &StepId) -> Result<Vec<StepId>, ScmGraphError> {
        self.walk(id, Direction::Parents)
    }

    pub fn descendants(&self, id: &StepId) -> Result<Vec<StepId>, ScmGraphError> {
        self.walk(id, Direction::Children)
    }

    /// Returns every node in deterministic parent-before-child order.
    pub fn topological(&self) -> Result<Vec<StepId>, ScmGraphError> {
        topological_ids(&self.nodes)
    }

    /// Returns the complete parent closure of `heads`, including the heads,
    /// in deterministic parent-before-child order.
    pub fn parent_closure_topological(
        &self,
        heads: &[StepId],
    ) -> Result<Vec<StepId>, ScmGraphError> {
        let mut closure = BTreeSet::new();
        for head in checked_step_set(self, heads)? {
            closure.insert(head.clone());
            closure.extend(self.ancestors(&head)?);
        }
        Ok(self
            .topological()?
            .into_iter()
            .filter(|id| closure.contains(id))
            .collect())
    }

    /// Computes all maximal common ancestors of the supplied heads.
    pub fn merge_bases(&self, heads: &[StepId]) -> Result<Vec<StepId>, ScmGraphError> {
        let heads = checked_step_set(self, heads)?;
        let Some(first) = heads.first() else {
            return Ok(Vec::new());
        };

        let mut common = self.ancestors_including(first)?;
        for head in heads.iter().skip(1) {
            let ancestors = self.ancestors_including(head)?;
            common.retain(|candidate| ancestors.contains(candidate));
        }

        let candidates: Vec<_> = common.iter().cloned().collect();
        let mut maximal = Vec::new();
        for candidate in &candidates {
            let mut dominated = false;
            for other in &candidates {
                if candidate != other && self.is_ancestor(candidate, other)? {
                    dominated = true;
                    break;
                }
            }
            if !dominated {
                maximal.push(candidate.clone());
            }
        }
        Ok(maximal)
    }

    fn node(&self, id: &StepId) -> Result<&StepNode, ScmGraphError> {
        self.nodes
            .get(id)
            .ok_or_else(|| ScmGraphError::MissingStep {
                step_id: id.clone(),
                referenced_id: id.clone(),
            })
    }

    fn ancestors_including(&self, id: &StepId) -> Result<BTreeSet<StepId>, ScmGraphError> {
        let mut result: BTreeSet<_> = self.ancestors(id)?.into_iter().collect();
        result.insert(id.clone());
        Ok(result)
    }

    fn walk(&self, id: &StepId, direction: Direction) -> Result<Vec<StepId>, ScmGraphError> {
        self.node(id)?;
        let mut visited = BTreeSet::new();
        let mut pending = VecDeque::from([id.clone()]);
        while let Some(current) = pending.pop_front() {
            let node = self.node(&current)?;
            let neighbors = match direction {
                Direction::Parents => &node.parents,
                Direction::Children => &node.children,
            };
            for neighbor in neighbors {
                if visited.insert(neighbor.clone()) {
                    pending.push_back(neighbor.clone());
                }
            }
        }
        Ok(visited.into_iter().collect())
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Parents,
    Children,
}

fn checked_step_set(graph: &StepDag, ids: &[StepId]) -> Result<Vec<StepId>, ScmGraphError> {
    let mut result = BTreeSet::new();
    for id in ids {
        if !graph.contains(id) {
            return Err(ScmGraphError::MissingStep {
                step_id: id.clone(),
                referenced_id: id.clone(),
            });
        }
        if !result.insert(id.clone()) {
            return Err(ScmGraphError::DuplicateHead(id.clone()));
        }
    }
    Ok(result.into_iter().collect())
}

fn first_duplicate(ids: &[StepId]) -> Option<StepId> {
    let mut seen = BTreeSet::new();
    ids.iter().find_map(|id| {
        if seen.insert(id.clone()) {
            None
        } else {
            Some(id.clone())
        }
    })
}

fn topological_ids(nodes: &BTreeMap<StepId, StepNode>) -> Result<Vec<StepId>, ScmGraphError> {
    let mut indegrees: BTreeMap<_, _> = nodes
        .iter()
        .map(|(id, node)| (id.clone(), node.parents.len()))
        .collect();
    let mut ready: BTreeSet<_> = indegrees
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
        .collect();
    let mut result = Vec::with_capacity(nodes.len());

    while let Some(id) = ready.pop_first() {
        result.push(id.clone());
        let node = nodes.get(&id).ok_or(ScmGraphError::InternalInvariant(
            "topological Step disappeared",
        ))?;
        for child in &node.children {
            let degree = indegrees
                .get_mut(child)
                .ok_or(ScmGraphError::InternalInvariant(
                    "Step child has no indegree",
                ))?;
            *degree = degree
                .checked_sub(1)
                .ok_or(ScmGraphError::InternalInvariant("Step indegree underflow"))?;
            if *degree == 0 {
                ready.insert(child.clone());
            }
        }
    }

    if result.len() != nodes.len() {
        let remaining: BTreeSet<_> = indegrees
            .into_iter()
            .filter_map(|(id, degree)| (degree != 0).then_some(id))
            .collect();
        let witness = remaining
            .iter()
            .find_map(|id| step_reaches_itself(nodes, &remaining, id).then_some(id.clone()))
            .ok_or(ScmGraphError::InternalInvariant("cycle has no witness"))?;
        return Err(ScmGraphError::Cycle {
            kind: GraphObjectKind::Step,
            id: witness.to_string(),
        });
    }
    Ok(result)
}

fn step_reaches_itself(
    nodes: &BTreeMap<StepId, StepNode>,
    remaining: &BTreeSet<StepId>,
    start: &StepId,
) -> bool {
    let mut visited = BTreeSet::new();
    let mut pending = VecDeque::from([start.clone()]);
    while let Some(current) = pending.pop_front() {
        let Some(node) = nodes.get(&current) else {
            return false;
        };
        for child in &node.children {
            if child == start {
                return true;
            }
            if remaining.contains(child) && visited.insert(child.clone()) {
                pending.push_back(child.clone());
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::TreeId;

    fn oid(seed: u64) -> String {
        format!("blake3:{seed:064x}")
    }

    fn tree(seed: u64) -> TreeId {
        TreeId::parse(oid(seed)).unwrap()
    }

    fn step(parents: &[&Step], seed: u64) -> Step {
        Step::new(
            parents.iter().map(|parent| parent.id().clone()).collect(),
            tree(seed),
        )
        .unwrap()
    }

    #[test]
    fn diamond_supports_forward_refs_and_deterministic_traversal() {
        let root = step(&[], 1);
        let left = step(&[&root], 2);
        let right = step(&[&root], 3);
        let tip = step(&[&left, &right], 4);
        let graph = StepDag::from_steps([&tip, &right, &root, &left]).unwrap();

        assert_eq!(graph.parents(tip.id()).unwrap().len(), 2);
        assert_eq!(graph.children(root.id()).unwrap().len(), 2);
        assert!(graph.is_ancestor(root.id(), tip.id()).unwrap());
        assert!(!graph.is_ancestor(root.id(), root.id()).unwrap());
        assert!(graph.is_ancestor_or_equal(root.id(), root.id()).unwrap());
        let topo = graph.topological().unwrap();
        let root_position = topo.iter().position(|id| id == root.id()).unwrap();
        let tip_position = topo.iter().position(|id| id == tip.id()).unwrap();
        assert!(root_position < tip_position);
        assert_eq!(
            graph
                .merge_bases(&[left.id().clone(), right.id().clone()])
                .unwrap(),
            vec![root.id().clone()]
        );
    }

    #[test]
    fn missing_parent_and_duplicate_batches_roll_back_atomically() {
        let root = step(&[], 1);
        let mut graph = StepDag::from_steps([&root]).unwrap();
        let orphan_parent = step(&[], 9);
        let orphan = step(&[&orphan_parent], 10);
        let valid = step(&[&root], 2);

        assert!(matches!(
            graph.insert_batch([&valid, &orphan]),
            Err(ScmGraphError::MissingStep { .. })
        ));
        assert_eq!(graph.len(), 1);
        assert!(!graph.contains(valid.id()));
        assert!(matches!(
            graph.insert_batch([&root]),
            Err(ScmGraphError::DuplicateStep(_))
        ));
    }

    #[test]
    fn insertion_permutations_have_identical_topology() {
        let root = step(&[], 1);
        let left = step(&[&root], 2);
        let right = step(&[&root], 3);
        let tip = step(&[&left, &right], 4);
        let first = StepDag::from_steps([&root, &left, &right, &tip]).unwrap();
        let second = StepDag::from_steps([&tip, &right, &left, &root]).unwrap();
        assert_eq!(first.topological().unwrap(), second.topological().unwrap());
    }

    #[test]
    fn deep_history_uses_iterative_algorithms() {
        let mut steps = vec![step(&[], 0)];
        for seed in 1..4096 {
            let next = step(&[steps.last().unwrap()], seed);
            steps.push(next);
        }
        let references: Vec<_> = steps.iter().collect();
        let graph = StepDag::from_steps(references).unwrap();
        assert_eq!(
            graph.ancestors(steps.last().unwrap().id()).unwrap().len(),
            4095
        );
        assert_eq!(
            graph
                .descendants(steps.first().unwrap().id())
                .unwrap()
                .len(),
            4095
        );
        assert_eq!(
            graph
                .parent_closure_topological(&[steps.last().unwrap().id().clone()])
                .unwrap()
                .len(),
            4096
        );
    }

    #[test]
    fn criss_cross_returns_all_maximal_common_ancestors() {
        let root = step(&[], 1);
        let left_base = step(&[&root], 2);
        let right_base = step(&[&root], 3);
        let left_tip = step(&[&left_base, &right_base], 4);
        let right_tip = step(&[&left_base, &right_base], 5);
        let graph =
            StepDag::from_steps([&right_tip, &left_base, &root, &left_tip, &right_base]).unwrap();
        let expected: Vec<_> = BTreeSet::from([left_base.id().clone(), right_base.id().clone()])
            .into_iter()
            .collect();
        assert_eq!(
            graph
                .merge_bases(&[left_tip.id().clone(), right_tip.id().clone()])
                .unwrap(),
            expected
        );
        assert_eq!(
            graph
                .merge_bases(&[
                    left_tip.id().clone(),
                    right_tip.id().clone(),
                    left_base.id().clone(),
                ])
                .unwrap(),
            vec![left_base.id().clone()]
        );
    }

    #[test]
    fn corrupted_cycle_is_rejected_without_public_bypass() {
        let first = StepId::parse(oid(1)).unwrap();
        let second = StepId::parse(oid(2)).unwrap();
        let mut graph = StepDag::new();
        graph.nodes.insert(
            first.clone(),
            StepNode {
                parents: BTreeSet::from([second.clone()]),
                children: BTreeSet::from([second.clone()]),
            },
        );
        graph.nodes.insert(
            second.clone(),
            StepNode {
                parents: BTreeSet::from([first.clone()]),
                children: BTreeSet::from([first]),
            },
        );
        assert!(matches!(
            graph.topological(),
            Err(ScmGraphError::Cycle {
                kind: GraphObjectKind::Step,
                ..
            })
        ));
    }
}
