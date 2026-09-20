use super::error::{GraphObjectKind, ScmGraphError};
use layrs_core::scm::{Operation, OperationId, OperationKind, SpaceId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone)]
struct OperationNode {
    space_id: SpaceId,
    kind: OperationKind,
    parents: BTreeSet<OperationId>,
    children: BTreeSet<OperationId>,
}

#[derive(Debug, Clone)]
struct OperationSpec {
    space_id: SpaceId,
    kind: OperationKind,
    parents: BTreeSet<OperationId>,
}

/// A deterministic index of local Operations, partitioned by Space.
#[derive(Debug, Clone, Default)]
pub struct OperationDag {
    nodes: BTreeMap<OperationId, OperationNode>,
}

impl OperationDag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds an index from non-integration Operations. Historical
    /// integrations must be replayed in admission order with
    /// [`Self::insert_integration`].
    pub fn from_operations<'a>(
        operations: impl IntoIterator<Item = &'a Operation>,
    ) -> Result<Self, ScmGraphError> {
        let mut graph = Self::new();
        graph.insert_batch(operations)?;
        Ok(graph)
    }

    /// Builds a complete, closed historical Operation DAG in deterministic
    /// topological order. Unlike repeated insertion, this constructor validates
    /// integration frontiers without cloning the growing graph for every node.
    pub fn from_history<'a>(
        operations: impl IntoIterator<Item = &'a Operation>,
    ) -> Result<Self, ScmGraphError> {
        let empty = Self::new();
        let specifications = empty.collect_specifications(operations)?;

        for (id, specification) in &specifications {
            validate_parent_count(id, specification)?;
            for parent in &specification.parents {
                if parent == id {
                    return Err(ScmGraphError::SelfParent {
                        kind: GraphObjectKind::Operation,
                        id: id.to_string(),
                    });
                }
                let Some(parent_specification) = specifications.get(parent) else {
                    return Err(ScmGraphError::MissingOperation {
                        operation_id: id.clone(),
                        referenced_id: parent.clone(),
                    });
                };
                if parent_specification.space_id != specification.space_id {
                    return Err(ScmGraphError::OperationSpaceMismatch {
                        operation_id: id.clone(),
                        parent_id: parent.clone(),
                        operation_space: specification.space_id.clone(),
                        parent_space: parent_specification.space_id.clone(),
                    });
                }
            }
        }
        validate_single_initialization(&empty.nodes, &specifications)?;

        let mut graph = Self::new();
        for (id, specification) in &specifications {
            graph.nodes.insert(
                id.clone(),
                OperationNode {
                    space_id: specification.space_id.clone(),
                    kind: specification.kind,
                    parents: specification.parents.clone(),
                    children: BTreeSet::new(),
                },
            );
        }
        for (id, specification) in &specifications {
            for parent in &specification.parents {
                graph
                    .nodes
                    .get_mut(parent)
                    .ok_or(ScmGraphError::InternalInvariant(
                        "validated historical parent disappeared",
                    ))?
                    .children
                    .insert(id.clone());
            }
        }

        let order = graph.topological()?;
        let mut heads = BTreeMap::<SpaceId, BTreeSet<OperationId>>::new();
        for id in order {
            let node = graph.node(&id)?;
            let current = heads.entry(node.space_id.clone()).or_default();
            if node.kind == OperationKind::IntegrateOperations && node.parents != *current {
                return Err(ScmGraphError::IntegrationParentsMismatch {
                    operation_id: id,
                    expected: current.iter().cloned().collect(),
                    actual: node.parents.iter().cloned().collect(),
                });
            }
            for parent in &node.parents {
                current.remove(parent);
            }
            current.insert(id);
        }
        Ok(graph)
    }

    /// Atomically inserts non-integration Operations. References may point
    /// forward within the batch. `IntegrateOperations` is always rejected and
    /// must use [`Self::insert_integration`].
    pub fn insert_batch<'a>(
        &mut self,
        operations: impl IntoIterator<Item = &'a Operation>,
    ) -> Result<(), ScmGraphError> {
        let specifications = self.collect_specifications(operations)?;
        self.insert_specifications(specifications, false)
    }

    /// Atomically admits one integration against the caller's complete
    /// expected current head set. Validation and insertion happen under the
    /// same exclusive `&mut self` borrow and every failure leaves the DAG
    /// unchanged.
    pub fn insert_integration(
        &mut self,
        operation: &Operation,
        expected_heads: &[OperationId],
    ) -> Result<(), ScmGraphError> {
        if self.nodes.contains_key(operation.id()) {
            return Err(ScmGraphError::DuplicateOperation(operation.id().clone()));
        }
        self.validate_integration_parents(operation, expected_heads)?;
        let specifications = self.collect_specifications([operation])?;
        self.insert_specifications(specifications, true)
    }

    fn collect_specifications<'a>(
        &self,
        operations: impl IntoIterator<Item = &'a Operation>,
    ) -> Result<BTreeMap<OperationId, OperationSpec>, ScmGraphError> {
        let mut specifications = BTreeMap::new();
        for operation in operations {
            let id = operation.id().clone();
            if self.nodes.contains_key(&id) || specifications.contains_key(&id) {
                return Err(ScmGraphError::DuplicateOperation(id));
            }
            let parents: BTreeSet<_> = operation.parent_operation_ids().iter().cloned().collect();
            if parents.len() != operation.parent_operation_ids().len() {
                let duplicate = first_duplicate(operation.parent_operation_ids()).ok_or(
                    ScmGraphError::InternalInvariant("duplicate Operation parent has no witness"),
                )?;
                return Err(ScmGraphError::DuplicateOperationParent {
                    operation_id: id,
                    parent_id: duplicate,
                });
            }
            specifications.insert(
                id,
                OperationSpec {
                    space_id: operation.space_id().clone(),
                    kind: operation.kind(),
                    parents,
                },
            );
        }
        Ok(specifications)
    }

    fn insert_specifications(
        &mut self,
        specifications: BTreeMap<OperationId, OperationSpec>,
        allow_integration: bool,
    ) -> Result<(), ScmGraphError> {
        if !allow_integration {
            if let Some((id, _)) = specifications
                .iter()
                .find(|(_, specification)| specification.kind == OperationKind::IntegrateOperations)
            {
                return Err(ScmGraphError::IntegrationRequiresDedicatedAdmission(
                    id.clone(),
                ));
            }
        }

        for (id, specification) in &specifications {
            validate_parent_count(id, specification)?;
            for parent in &specification.parents {
                if parent == id {
                    return Err(ScmGraphError::SelfParent {
                        kind: GraphObjectKind::Operation,
                        id: id.to_string(),
                    });
                }
                let parent_space = if let Some(node) = self.nodes.get(parent) {
                    Some(&node.space_id)
                } else {
                    specifications.get(parent).map(|node| &node.space_id)
                };
                let Some(parent_space) = parent_space else {
                    return Err(ScmGraphError::MissingOperation {
                        operation_id: id.clone(),
                        referenced_id: parent.clone(),
                    });
                };
                if parent_space != &specification.space_id {
                    return Err(ScmGraphError::OperationSpaceMismatch {
                        operation_id: id.clone(),
                        parent_id: parent.clone(),
                        operation_space: specification.space_id.clone(),
                        parent_space: parent_space.clone(),
                    });
                }
            }
        }
        validate_single_initialization(&self.nodes, &specifications)?;

        let mut candidate = self.clone();
        for (id, specification) in &specifications {
            candidate.nodes.insert(
                id.clone(),
                OperationNode {
                    space_id: specification.space_id.clone(),
                    kind: specification.kind,
                    parents: BTreeSet::new(),
                    children: BTreeSet::new(),
                },
            );
        }
        for (id, specification) in specifications {
            candidate
                .nodes
                .get_mut(&id)
                .ok_or(ScmGraphError::InternalInvariant(
                    "inserted Operation disappeared",
                ))?
                .parents = specification.parents.clone();
            for parent in specification.parents {
                candidate
                    .nodes
                    .get_mut(&parent)
                    .ok_or(ScmGraphError::InternalInvariant(
                        "validated Operation parent disappeared",
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

    pub fn contains(&self, id: &OperationId) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn space_id(&self, id: &OperationId) -> Result<&SpaceId, ScmGraphError> {
        Ok(&self.node(id)?.space_id)
    }

    pub fn kind(&self, id: &OperationId) -> Result<OperationKind, ScmGraphError> {
        Ok(self.node(id)?.kind)
    }

    pub fn parents(&self, id: &OperationId) -> Result<Vec<OperationId>, ScmGraphError> {
        self.node(id)
            .map(|node| node.parents.iter().cloned().collect())
    }

    pub fn children(&self, id: &OperationId) -> Result<Vec<OperationId>, ScmGraphError> {
        self.node(id)
            .map(|node| node.children.iter().cloned().collect())
    }

    pub fn is_ancestor(
        &self,
        ancestor: &OperationId,
        descendant: &OperationId,
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
        ancestor: &OperationId,
        descendant: &OperationId,
    ) -> Result<bool, ScmGraphError> {
        self.node(ancestor)?;
        self.node(descendant)?;
        Ok(ancestor == descendant || self.is_ancestor(ancestor, descendant)?)
    }

    pub fn ancestors(&self, id: &OperationId) -> Result<Vec<OperationId>, ScmGraphError> {
        self.walk(id, Direction::Parents)
    }

    pub fn descendants(&self, id: &OperationId) -> Result<Vec<OperationId>, ScmGraphError> {
        self.walk(id, Direction::Children)
    }

    pub fn topological(&self) -> Result<Vec<OperationId>, ScmGraphError> {
        let mut indegrees: BTreeMap<_, _> = self
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.parents.len()))
            .collect();
        let mut ready: BTreeSet<_> = indegrees
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
            .collect();
        let mut result = Vec::with_capacity(self.nodes.len());
        while let Some(id) = ready.pop_first() {
            result.push(id.clone());
            for child in &self.node(&id)?.children {
                let degree = indegrees
                    .get_mut(child)
                    .ok_or(ScmGraphError::InternalInvariant(
                        "Operation child has no indegree",
                    ))?;
                *degree = degree
                    .checked_sub(1)
                    .ok_or(ScmGraphError::InternalInvariant(
                        "Operation indegree underflow",
                    ))?;
                if *degree == 0 {
                    ready.insert(child.clone());
                }
            }
        }
        if result.len() != self.nodes.len() {
            let remaining: BTreeSet<_> = indegrees
                .into_iter()
                .filter_map(|(id, degree)| (degree != 0).then_some(id))
                .collect();
            let witness = remaining
                .iter()
                .find_map(|id| self.reaches_itself(&remaining, id).then_some(id.clone()))
                .ok_or(ScmGraphError::InternalInvariant("cycle has no witness"))?;
            return Err(ScmGraphError::Cycle {
                kind: GraphObjectKind::Operation,
                id: witness.to_string(),
            });
        }
        Ok(result)
    }

    pub fn heads(&self) -> Vec<OperationId> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| node.children.is_empty().then_some(id.clone()))
            .collect()
    }

    pub fn heads_in_space(&self, space_id: &SpaceId) -> Vec<OperationId> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| {
                (node.space_id == *space_id && node.children.is_empty()).then_some(id.clone())
            })
            .collect()
    }

    /// Validates the compare-and-swap precondition for an integration. The
    /// caller's expected set must equal the complete current Space head set,
    /// and the Operation parents must equal that expected set exactly.
    pub fn validate_integration_parents(
        &self,
        operation: &Operation,
        expected_heads: &[OperationId],
    ) -> Result<(), ScmGraphError> {
        if operation.kind() != OperationKind::IntegrateOperations {
            return Err(ScmGraphError::NotIntegrationOperation(
                operation.id().clone(),
            ));
        }
        let mut expected = BTreeSet::new();
        for id in expected_heads {
            if !expected.insert(id.clone()) {
                return Err(ScmGraphError::DuplicateIntegrationParent(id.clone()));
            }
            let node = self.node_for(operation.id(), id)?;
            if node.space_id != *operation.space_id() {
                return Err(ScmGraphError::IntegrationParentSpaceMismatch {
                    operation_id: operation.id().clone(),
                    parent_id: id.clone(),
                });
            }
        }
        let expected: Vec<_> = expected.into_iter().collect();
        let current = self.heads_in_space(operation.space_id());
        if expected != current {
            return Err(ScmGraphError::IntegrationParentsMismatch {
                operation_id: operation.id().clone(),
                expected: current,
                actual: expected,
            });
        }
        let actual: Vec<_> = operation.parent_operation_ids().to_vec();
        if actual != expected {
            return Err(ScmGraphError::IntegrationParentsMismatch {
                operation_id: operation.id().clone(),
                expected,
                actual,
            });
        }
        if actual.len() < 2 {
            return Err(ScmGraphError::IntegrationRequiresMultipleParents(
                operation.id().clone(),
            ));
        }
        Ok(())
    }

    fn node(&self, id: &OperationId) -> Result<&OperationNode, ScmGraphError> {
        self.node_for(id, id)
    }

    fn node_for(
        &self,
        operation_id: &OperationId,
        referenced_id: &OperationId,
    ) -> Result<&OperationNode, ScmGraphError> {
        self.nodes
            .get(referenced_id)
            .ok_or_else(|| ScmGraphError::MissingOperation {
                operation_id: operation_id.clone(),
                referenced_id: referenced_id.clone(),
            })
    }

    fn walk(
        &self,
        id: &OperationId,
        direction: Direction,
    ) -> Result<Vec<OperationId>, ScmGraphError> {
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

    fn reaches_itself(&self, remaining: &BTreeSet<OperationId>, start: &OperationId) -> bool {
        let mut visited = BTreeSet::new();
        let mut pending = VecDeque::from([start.clone()]);
        while let Some(current) = pending.pop_front() {
            let Some(node) = self.nodes.get(&current) else {
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
}

#[derive(Clone, Copy)]
enum Direction {
    Parents,
    Children,
}

fn validate_parent_count(
    id: &OperationId,
    specification: &OperationSpec,
) -> Result<(), ScmGraphError> {
    if specification.kind == OperationKind::Initialize {
        if !specification.parents.is_empty() {
            return Err(ScmGraphError::InvalidInitializeParents(id.clone()));
        }
    } else if specification.parents.is_empty() {
        return Err(ScmGraphError::OperationRequiresParent(id.clone()));
    }
    if specification.kind == OperationKind::IntegrateOperations && specification.parents.len() < 2 {
        return Err(ScmGraphError::IntegrationRequiresMultipleParents(
            id.clone(),
        ));
    }
    Ok(())
}

fn first_duplicate(ids: &[OperationId]) -> Option<OperationId> {
    let mut seen = BTreeSet::new();
    ids.iter().find_map(|id| {
        if seen.insert(id.clone()) {
            None
        } else {
            Some(id.clone())
        }
    })
}

fn validate_single_initialization(
    existing: &BTreeMap<OperationId, OperationNode>,
    incoming: &BTreeMap<OperationId, OperationSpec>,
) -> Result<(), ScmGraphError> {
    let mut initializers = BTreeMap::<SpaceId, OperationId>::new();
    for (id, node) in existing {
        if node.kind == OperationKind::Initialize {
            initializers.insert(node.space_id.clone(), id.clone());
        }
    }
    for (id, node) in incoming {
        if node.kind == OperationKind::Initialize {
            if let Some(first) = initializers.insert(node.space_id.clone(), id.clone()) {
                return Err(ScmGraphError::DuplicateSpaceInitialization {
                    space_id: node.space_id.clone(),
                    first,
                    duplicate: id.clone(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{Metadata, RepositoryViewId, Timestamp};

    fn oid(seed: u64) -> String {
        format!("blake3:{seed:064x}")
    }

    fn space(name: &str) -> SpaceId {
        SpaceId::parse(format!("space:{name}")).unwrap()
    }

    fn operation(
        name: &str,
        space_id: &SpaceId,
        parents: &[&Operation],
        kind: OperationKind,
        seed: u64,
    ) -> Operation {
        Operation::new(
            OperationId::parse(format!("operation:{name}")).unwrap(),
            space_id.clone(),
            parents.iter().map(|parent| parent.id().clone()).collect(),
            RepositoryViewId::parse(oid(seed)).unwrap(),
            kind,
            name,
            vec![],
            None,
            None,
            Timestamp::parse("2026-07-19T00:00:00Z").unwrap(),
            Metadata::empty(),
        )
        .unwrap()
    }

    fn fork() -> (SpaceId, Operation, Operation, Operation) {
        let target_space = space("test");
        let root = operation("init", &target_space, &[], OperationKind::Initialize, 1);
        let left = operation(
            "left",
            &target_space,
            &[&root],
            OperationKind::CaptureWorktree,
            2,
        );
        let right = operation(
            "right",
            &target_space,
            &[&root],
            OperationKind::SwitchLayer,
            3,
        );
        (target_space, root, left, right)
    }

    #[test]
    fn forks_forward_refs_topology_and_heads_are_deterministic() {
        let (target_space, root, left, right) = fork();
        let first = OperationDag::from_operations([&right, &root, &left]).unwrap();
        let second = OperationDag::from_operations([&left, &right, &root]).unwrap();
        assert_eq!(first.topological().unwrap(), second.topological().unwrap());
        assert_eq!(first.heads_in_space(&target_space).len(), 2);
        assert!(first.is_ancestor(root.id(), left.id()).unwrap());
        assert_eq!(first.children(root.id()).unwrap().len(), 2);
    }

    #[test]
    fn integration_requires_exact_current_heads_then_replaces_them() {
        let (target_space, root, left, right) = fork();
        let mut graph = OperationDag::from_operations([&root, &left, &right]).unwrap();
        let integration = operation(
            "integrate",
            &target_space,
            &[&right, &left],
            OperationKind::IntegrateOperations,
            4,
        );
        let heads = graph.heads_in_space(&target_space);
        graph
            .validate_integration_parents(&integration, &heads)
            .unwrap();
        assert!(matches!(
            graph.validate_integration_parents(&integration, &[left.id().clone()]),
            Err(ScmGraphError::IntegrationParentsMismatch { .. })
        ));
        graph.insert_integration(&integration, &heads).unwrap();
        assert_eq!(
            graph.heads_in_space(&target_space),
            vec![integration.id().clone()]
        );
    }

    #[test]
    fn integration_admission_rejects_omitted_stale_and_duplicate_heads_atomically() {
        let (target_space, root, left, right) = fork();
        let third = operation("third", &target_space, &[&root], OperationKind::Undo, 7);
        let mut graph = OperationDag::from_operations([&root, &left, &right, &third]).unwrap();
        let integration = operation(
            "incomplete-integration",
            &target_space,
            &[&left, &right],
            OperationKind::IntegrateOperations,
            8,
        );
        let before = graph.topological().unwrap();

        assert!(matches!(
            graph.insert_integration(&integration, &[left.id().clone(), right.id().clone()]),
            Err(ScmGraphError::IntegrationParentsMismatch { .. })
        ));
        assert_eq!(graph.topological().unwrap(), before);
        assert!(!graph.contains(integration.id()));

        assert!(matches!(
            graph.insert_integration(
                &integration,
                &[root.id().clone(), left.id().clone(), right.id().clone()]
            ),
            Err(ScmGraphError::IntegrationParentsMismatch { .. })
        ));
        assert_eq!(graph.topological().unwrap(), before);

        assert!(matches!(
            graph.insert_integration(
                &integration,
                &[
                    left.id().clone(),
                    left.id().clone(),
                    right.id().clone(),
                    third.id().clone(),
                ]
            ),
            Err(ScmGraphError::DuplicateIntegrationParent(_))
        ));
        assert_eq!(graph.topological().unwrap(), before);
    }

    #[test]
    fn integrations_can_be_admitted_sequentially() {
        let (target_space, root, left, right) = fork();
        let mut graph = OperationDag::from_operations([&root, &left, &right]).unwrap();
        let first = operation(
            "integrate-first",
            &target_space,
            &[&left, &right],
            OperationKind::IntegrateOperations,
            10,
        );
        let first_heads = graph.heads_in_space(&target_space);
        graph.insert_integration(&first, &first_heads).unwrap();

        let next_left = operation(
            "next-left",
            &target_space,
            &[&first],
            OperationKind::Undo,
            11,
        );
        let next_right = operation(
            "next-right",
            &target_space,
            &[&first],
            OperationKind::Redo,
            12,
        );
        graph.insert_batch([&next_right, &next_left]).unwrap();
        let second = operation(
            "integrate-second",
            &target_space,
            &[&next_left, &next_right],
            OperationKind::IntegrateOperations,
            13,
        );
        let second_heads = graph.heads_in_space(&target_space);
        graph.insert_integration(&second, &second_heads).unwrap();
        assert_eq!(
            graph.heads_in_space(&target_space),
            vec![second.id().clone()]
        );
    }

    #[test]
    fn generic_apis_reject_integrations_and_forward_batches_atomically() {
        let (target_space, root, left, right) = fork();
        let integration = operation(
            "generic-integration",
            &target_space,
            &[&left, &right],
            OperationKind::IntegrateOperations,
            14,
        );
        let error =
            OperationDag::from_operations([&root, &left, &right, &integration]).unwrap_err();
        assert_eq!(error.code(), "integration_requires_dedicated_admission");

        let mut graph = OperationDag::from_operations([&root]).unwrap();
        let before = graph.topological().unwrap();
        assert_eq!(
            graph
                .insert_batch([&integration, &right, &left])
                .unwrap_err()
                .code(),
            "integration_requires_dedicated_admission"
        );
        assert_eq!(graph.topological().unwrap(), before);
        assert!(!graph.contains(left.id()));
        assert!(!graph.contains(right.id()));

        let second = operation(
            "generic-integration-second",
            &target_space,
            &[&left, &right],
            OperationKind::IntegrateOperations,
            15,
        );
        assert_eq!(
            graph
                .insert_batch([&left, &second, &right, &integration])
                .unwrap_err()
                .code(),
            "integration_requires_dedicated_admission"
        );
        assert_eq!(graph.topological().unwrap(), before);
    }

    #[test]
    fn cross_space_and_missing_parents_roll_back_atomically() {
        let (target_space, root, _, _) = fork();
        let mut graph = OperationDag::from_operations([&root]).unwrap();
        let other_space = space("other");
        let cross_space = operation(
            "cross",
            &other_space,
            &[&root],
            OperationKind::CaptureWorktree,
            8,
        );
        assert!(matches!(
            graph.insert_batch([&cross_space]),
            Err(ScmGraphError::OperationSpaceMismatch { .. })
        ));
        assert_eq!(graph.len(), 1);

        let absent = operation("absent", &target_space, &[&root], OperationKind::Undo, 9);
        let orphan = Operation::new(
            OperationId::parse("operation:orphan").unwrap(),
            target_space,
            vec![absent.id().clone()],
            RepositoryViewId::parse(oid(10)).unwrap(),
            OperationKind::Undo,
            "orphan",
            vec![],
            None,
            None,
            Timestamp::parse("2026-07-19T00:00:00Z").unwrap(),
            Metadata::empty(),
        )
        .unwrap();
        assert!(matches!(
            graph.insert_batch([&orphan]),
            Err(ScmGraphError::MissingOperation { .. })
        ));
        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn initialization_and_integration_parent_counts_are_enforced() {
        let (target_space, root, left, _) = fork();
        let malformed_init = operation(
            "bad-init",
            &target_space,
            &[&root],
            OperationKind::Initialize,
            5,
        );
        assert!(matches!(
            OperationDag::from_operations([&root, &malformed_init]),
            Err(ScmGraphError::InvalidInitializeParents(_))
        ));
        let malformed_integration = operation(
            "bad-integration",
            &target_space,
            &[&left],
            OperationKind::IntegrateOperations,
            6,
        );
        let mut graph = OperationDag::from_operations([&root, &left]).unwrap();
        assert!(matches!(
            graph.insert_integration(&malformed_integration, &[left.id().clone()]),
            Err(ScmGraphError::IntegrationRequiresMultipleParents(_))
        ));
        assert!(!graph.contains(malformed_integration.id()));
    }

    #[test]
    fn operation_cycles_are_rejected_with_a_deterministic_witness() {
        let target_space = space("cycle");
        let first_id = OperationId::parse("operation:cycle-a").unwrap();
        let second_id = OperationId::parse("operation:cycle-b").unwrap();
        let make = |id: OperationId, parent: OperationId, seed: u64| {
            Operation::new(
                id,
                target_space.clone(),
                vec![parent],
                RepositoryViewId::parse(oid(seed)).unwrap(),
                OperationKind::Undo,
                "cycle",
                vec![],
                None,
                None,
                Timestamp::parse("2026-07-19T00:00:00Z").unwrap(),
                Metadata::empty(),
            )
            .unwrap()
        };
        let first = make(first_id.clone(), second_id.clone(), 20);
        let second = make(second_id, first_id.clone(), 21);
        assert_eq!(
            OperationDag::from_operations([&second, &first]).unwrap_err(),
            ScmGraphError::Cycle {
                kind: GraphObjectKind::Operation,
                id: first_id.to_string(),
            }
        );
    }

    #[test]
    fn historical_constructor_handles_a_long_chain_in_one_deterministic_build() {
        let target_space = space("history-chain");
        let mut operations = Vec::with_capacity(5_000);
        let mut parent = None;
        for index in 0_u64..5_000 {
            let id = OperationId::parse(format!("operation:history-{index:05}")).unwrap();
            let value = Operation::new(
                id.clone(),
                target_space.clone(),
                parent.iter().cloned().collect(),
                RepositoryViewId::parse(oid(index + 100)).unwrap(),
                if parent.is_none() {
                    OperationKind::Initialize
                } else {
                    OperationKind::CaptureWorktree
                },
                "history",
                vec![],
                None,
                None,
                Timestamp::parse("2026-07-19T00:00:00Z").unwrap(),
                Metadata::empty(),
            )
            .unwrap();
            parent = Some(id);
            operations.push(value);
        }
        let graph = OperationDag::from_history(operations.iter().rev()).unwrap();
        assert_eq!(graph.len(), operations.len());
        assert_eq!(
            graph.heads_in_space(&target_space),
            parent.into_iter().collect::<Vec<_>>()
        );
    }
}
