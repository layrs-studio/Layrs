//! Bounded dependency and combined-history closure validation.
use super::*;
pub(super) fn admission_dependencies(object: &CanonicalObject) -> Vec<StoreObjectId> {
    match object {
        CanonicalObject::Product(value) => value.direct_dependencies(),
        CanonicalObject::ChunkRaw(_)
        | CanonicalObject::Symlink(_)
        | CanonicalObject::Tombstone(_)
        | CanonicalObject::Conflict(_) => Vec::new(),
        CanonicalObject::FileManifest(value) => value
            .chunks()
            .iter()
            .map(|entry| StoreObjectId::ChunkRaw(entry.chunk_id().clone()))
            .collect(),
        CanonicalObject::Tree(value) => value
            .entries()
            .iter()
            .filter_map(|entry| {
                let target = entry.target();
                target
                    .tree_id()
                    .map(|id| StoreObjectId::Tree(id.clone()))
                    .or_else(|| {
                        target
                            .conflict_id()
                            .map(|id| StoreObjectId::Conflict(id.clone()))
                    })
                    .or_else(|| {
                        target
                            .symlink_id()
                            .map(|id| StoreObjectId::Symlink(id.clone()))
                    })
                    .or_else(|| {
                        target
                            .tombstone_id()
                            .map(|id| StoreObjectId::Tombstone(id.clone()))
                    })
            })
            .collect(),
        CanonicalObject::Step(value) => {
            let mut dependencies = vec![StoreObjectId::Tree(value.root_tree_id().clone())];
            dependencies.extend(
                value
                    .parent_step_ids()
                    .iter()
                    .cloned()
                    .map(StoreObjectId::Step),
            );
            dependencies
        }
        CanonicalObject::RepositoryView(value) => {
            let mut dependencies: Vec<_> = value
                .layer_heads()
                .iter()
                .flat_map(|heads| heads.step_ids().iter().cloned())
                .map(StoreObjectId::Step)
                .collect();
            for worktree in value.worktrees() {
                dependencies.extend(worktree.base_step_id().cloned().map(StoreObjectId::Step));
                dependencies.extend(
                    worktree
                        .materialized_tree_id()
                        .cloned()
                        .map(StoreObjectId::Tree),
                );
            }
            dependencies
        }
    }
}

pub(super) fn admission_dependency_count(object: &CanonicalObject) -> usize {
    match object {
        CanonicalObject::Product(value) => value.direct_dependencies().len(),
        CanonicalObject::ChunkRaw(_)
        | CanonicalObject::Symlink(_)
        | CanonicalObject::Tombstone(_)
        | CanonicalObject::Conflict(_) => 0,
        CanonicalObject::FileManifest(value) => value.chunks().len(),
        CanonicalObject::Tree(value) => value.entries().len(),
        CanonicalObject::Step(value) => 1 + value.parent_step_ids().len(),
        CanonicalObject::RepositoryView(value) => {
            value
                .layer_heads()
                .iter()
                .map(|heads| heads.step_ids().len())
                .sum::<usize>()
                + value
                    .worktrees()
                    .iter()
                    .map(|state| {
                        usize::from(state.base_step_id().is_some())
                            + usize::from(state.materialized_tree_id().is_some())
                    })
                    .sum::<usize>()
        }
    }
}

pub(super) fn admission_rejected(object: &StoreObjectId) -> StoreError {
    StoreError::AdmissionRejected {
        kind: "SpaceObjectAdmission",
        id: object.to_string(),
    }
}

pub(super) fn mutation_rejected(kind: &'static str, id: &impl std::fmt::Display) -> StoreError {
    StoreError::AdmissionRejected {
        kind,
        id: id.to_string(),
    }
}

pub(super) fn require_budget(resource: &'static str, actual: usize, limit: usize) -> Result<()> {
    if actual > limit {
        Err(StoreError::AdmissionBudgetExceeded { resource, limit })
    } else {
        Ok(())
    }
}

pub(super) fn load_step_closure(
    reader: &dyn ScmReader,
    batch: &WriteBatch,
) -> Result<BTreeMap<StepId, Step>> {
    let mut steps: BTreeMap<_, _> = batch
        .canonical_objects()
        .iter()
        .filter_map(|object| match object {
            CanonicalObject::Step(value) => Some((value.id().clone(), value.clone())),
            _ => None,
        })
        .collect();
    let mut referenced = BTreeSet::new();
    for object in batch.canonical_objects() {
        if let CanonicalObject::Product(product) = object {
            referenced.extend(product.envelope().dependencies().0.iter().cloned());
        }
    }
    for view in batch
        .canonical_objects()
        .iter()
        .filter_map(|object| match object {
            CanonicalObject::RepositoryView(view) => Some(view),
            _ => None,
        })
    {
        for id in view
            .layer_heads()
            .iter()
            .flat_map(|heads| heads.step_ids().iter())
            .chain(
                view.worktrees()
                    .iter()
                    .filter_map(|state| state.base_step_id()),
            )
        {
            referenced.insert(id.clone());
            require_budget("referenced Steps", referenced.len(), MAX_GRAPH_NODES)?;
        }
    }
    for id in referenced {
        if let std::collections::btree_map::Entry::Vacant(entry) = steps.entry(id.clone()) {
            let value = reader.get_step(&id)?.ok_or_else(|| StoreError::NotFound {
                kind: "Step reference",
                id: id.to_string(),
            })?;
            entry.insert(value);
        }
    }
    require_budget("Step graph nodes", steps.len(), MAX_GRAPH_NODES)?;
    let mut edge_count: usize = steps
        .values()
        .map(|value| value.parent_step_ids().len())
        .sum();
    require_budget("Step graph edges", edge_count, MAX_GRAPH_EDGES)?;
    let mut pending: Vec<_> = steps.values().cloned().collect();
    while let Some(step) = pending.pop() {
        for parent in step.parent_step_ids() {
            if steps.contains_key(parent) {
                continue;
            }
            let value = reader
                .get_step(parent)?
                .ok_or_else(|| StoreError::NotFound {
                    kind: "Step parent",
                    id: parent.to_string(),
                })?;
            require_budget("Step graph nodes", steps.len() + 1, MAX_GRAPH_NODES)?;
            edge_count = edge_count
                .checked_add(value.parent_step_ids().len())
                .ok_or(StoreError::AdmissionBudgetExceeded {
                    resource: "Step graph edges",
                    limit: MAX_GRAPH_EDGES,
                })?;
            require_budget("Step graph edges", edge_count, MAX_GRAPH_EDGES)?;
            pending.push(value.clone());
            steps.insert(parent.clone(), value);
        }
    }
    Ok(steps)
}

pub(super) fn load_operation_closure(
    reader: &dyn ScmReader,
    batch: &WriteBatch,
    durable_heads: &BTreeMap<SpaceId, Vec<OperationId>>,
) -> Result<BTreeMap<OperationId, Operation>> {
    let mut operations: BTreeMap<_, _> = batch
        .entity_records()
        .iter()
        .filter_map(|record| match record {
            EntityRecord::Operation(value) => Some((value.id().clone(), value.clone())),
            _ => None,
        })
        .collect();
    for id in durable_heads.values().flatten() {
        if operations.contains_key(id) {
            continue;
        }
        let value = reader
            .get_operation(id)?
            .ok_or_else(|| StoreError::Corruption {
                message: format!("durable Operation head {id} is missing"),
            })?;
        operations.insert(id.clone(), value);
    }
    require_budget("Operation graph nodes", operations.len(), MAX_GRAPH_NODES)?;
    let mut edge_count: usize = operations
        .values()
        .map(|value| value.parent_operation_ids().len())
        .sum();
    require_budget("Operation graph edges", edge_count, MAX_GRAPH_EDGES)?;
    let mut pending: Vec<_> = operations.values().cloned().collect();
    while let Some(operation) = pending.pop() {
        for parent in operation.parent_operation_ids() {
            if operations.contains_key(parent) {
                continue;
            }
            let value = reader
                .get_operation(parent)?
                .ok_or_else(|| StoreError::NotFound {
                    kind: "Operation parent",
                    id: parent.to_string(),
                })?;
            require_budget(
                "Operation graph nodes",
                operations.len() + 1,
                MAX_GRAPH_NODES,
            )?;
            edge_count = edge_count
                .checked_add(value.parent_operation_ids().len())
                .ok_or(StoreError::AdmissionBudgetExceeded {
                    resource: "Operation graph edges",
                    limit: MAX_GRAPH_EDGES,
                })?;
            require_budget("Operation graph edges", edge_count, MAX_GRAPH_EDGES)?;
            pending.push(value.clone());
            operations.insert(parent.clone(), value);
        }
    }
    Ok(operations)
}

/// Loads exactly the durable ancestry rooted at the persisted current heads.
/// Batch entities are intentionally invisible here: a durable Operation that
/// is referenced only by an imported historical branch is not a current head
/// and must never enter publication-frontier calculation.
pub(super) fn load_durable_frontier_closure(
    reader: &dyn ScmReader,
    durable_heads: &BTreeMap<SpaceId, Vec<OperationId>>,
) -> Result<BTreeMap<OperationId, Operation>> {
    let mut operations = BTreeMap::new();
    let mut pending = Vec::new();
    for (space_id, heads) in durable_heads {
        for id in heads {
            let operation = reader
                .get_operation(id)?
                .ok_or_else(|| StoreError::Corruption {
                    message: format!("durable Operation head {id} is missing"),
                })?;
            if operation.space_id() != space_id {
                return Err(StoreError::Corruption {
                    message: format!(
                        "durable Operation head {id} belongs to {}, not {space_id}",
                        operation.space_id()
                    ),
                });
            }
            if let Some(existing) = operations.insert(id.clone(), operation.clone()) {
                if existing != operation {
                    return Err(StoreError::Corruption {
                        message: format!("durable Operation {id} has conflicting payloads"),
                    });
                }
            } else {
                pending.push(operation);
            }
        }
    }
    require_budget(
        "durable frontier Operation graph nodes",
        operations.len(),
        MAX_GRAPH_NODES,
    )?;
    let mut edge_count: usize = operations
        .values()
        .map(|operation| operation.parent_operation_ids().len())
        .sum();
    require_budget(
        "durable frontier Operation graph edges",
        edge_count,
        MAX_GRAPH_EDGES,
    )?;
    while let Some(operation) = pending.pop() {
        for parent in operation.parent_operation_ids() {
            if operations.contains_key(parent) {
                continue;
            }
            let value = reader
                .get_operation(parent)?
                .ok_or_else(|| StoreError::Corruption {
                    message: format!(
                        "durable Operation {} has missing parent {parent}",
                        operation.id()
                    ),
                })?;
            require_budget(
                "durable frontier Operation graph nodes",
                operations.len() + 1,
                MAX_GRAPH_NODES,
            )?;
            edge_count = edge_count
                .checked_add(value.parent_operation_ids().len())
                .ok_or(StoreError::AdmissionBudgetExceeded {
                    resource: "durable frontier Operation graph edges",
                    limit: MAX_GRAPH_EDGES,
                })?;
            require_budget(
                "durable frontier Operation graph edges",
                edge_count,
                MAX_GRAPH_EDGES,
            )?;
            pending.push(value.clone());
            operations.insert(parent.clone(), value);
        }
    }
    Ok(operations)
}

/// Validates the combined immutable history without assigning current-head
/// meaning to its graph tips. Integrations are structural multi-parent nodes
/// here; their exact frontier semantics belong exclusively to publication.
pub(super) fn validate_global_operation_history(
    operations: &BTreeMap<OperationId, Operation>,
) -> Result<()> {
    let mut initializers = BTreeMap::<SpaceId, OperationId>::new();
    let mut indegrees = BTreeMap::<OperationId, usize>::new();
    let mut children = BTreeMap::<OperationId, BTreeSet<OperationId>>::new();

    for (id, operation) in operations {
        let parents: BTreeSet<_> = operation.parent_operation_ids().iter().cloned().collect();
        if parents.len() != operation.parent_operation_ids().len() {
            return Err(candidate_graph_rejected(format!(
                "Operation {id} has duplicate parents"
            )));
        }
        match operation.kind() {
            OperationKind::Initialize => {
                if !parents.is_empty() {
                    return Err(candidate_graph_rejected(format!(
                        "Initialize Operation {id} has parents"
                    )));
                }
                if let Some(first) = initializers.insert(operation.space_id().clone(), id.clone()) {
                    return Err(candidate_graph_rejected(format!(
                        "Space {} has multiple Initialize Operations: {first} and {id}",
                        operation.space_id()
                    )));
                }
            }
            OperationKind::IntegrateOperations if parents.len() < 2 => {
                return Err(candidate_graph_rejected(format!(
                    "IntegrateOperations {id} requires at least two parents"
                )));
            }
            _ if parents.is_empty() => {
                return Err(candidate_graph_rejected(format!(
                    "non-Initialize Operation {id} requires a parent"
                )));
            }
            _ => {}
        }
        indegrees.insert(id.clone(), parents.len());
        children.entry(id.clone()).or_default();
        for parent in parents {
            if parent == *id {
                return Err(candidate_graph_rejected(format!(
                    "Operation {id} is its own parent"
                )));
            }
            let parent_operation = operations.get(&parent).ok_or_else(|| {
                candidate_graph_rejected(format!("Operation {id} has missing parent {parent}"))
            })?;
            if parent_operation.space_id() != operation.space_id() {
                return Err(candidate_graph_rejected(format!(
                    "Operation {id} and parent {parent} belong to different Spaces"
                )));
            }
            children.entry(parent).or_default().insert(id.clone());
        }
    }

    let mut ready: BTreeSet<_> = indegrees
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
        .collect();
    let mut visited = 0_usize;
    while let Some(id) = ready.pop_first() {
        visited += 1;
        for child in children.get(&id).into_iter().flatten() {
            let degree = indegrees
                .get_mut(child)
                .ok_or_else(|| StoreError::Corruption {
                    message: format!("candidate Operation child {child} lost its indegree"),
                })?;
            *degree = degree
                .checked_sub(1)
                .ok_or_else(|| StoreError::Corruption {
                    message: format!("candidate Operation indegree underflow for {child}"),
                })?;
            if *degree == 0 {
                ready.insert(child.clone());
            }
        }
    }
    if visited != operations.len() {
        let witness = indegrees
            .into_iter()
            .find_map(|(id, degree)| (degree != 0).then_some(id))
            .ok_or_else(|| StoreError::Corruption {
                message: "candidate Operation cycle has no witness".into(),
            })?;
        return Err(candidate_graph_rejected(format!(
            "Operation history contains a cycle at {witness}"
        )));
    }
    Ok(())
}

pub(super) fn build_operation_dag(
    operations: &BTreeMap<OperationId, Operation>,
) -> Result<OperationDag> {
    OperationDag::from_history(operations.values()).map_err(graph_corruption)
}

pub(super) fn candidate_graph_rejected(error: impl std::fmt::Display) -> StoreError {
    StoreError::AdmissionRejected {
        kind: "Operation history",
        id: error.to_string(),
    }
}

pub(super) fn is_strictly_sorted<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

pub(super) fn invalid_operation_frontier(
    operation: &Operation,
    reason: impl Into<String>,
) -> StoreError {
    StoreError::InvalidOperationHeadsTransition {
        operation_id: operation.id().clone(),
        reason: reason.into(),
    }
}

pub(super) fn object_kind(object: &CanonicalObject) -> &'static str {
    match object {
        CanonicalObject::ChunkRaw(_) => "Chunk",
        CanonicalObject::FileManifest(_) => "FileManifest",
        CanonicalObject::Tree(_) => "Tree",
        CanonicalObject::Symlink(_) => "Symlink",
        CanonicalObject::Tombstone(_) => "Tombstone",
        CanonicalObject::Step(_) => "Step",
        CanonicalObject::RepositoryView(_) => "RepositoryView",
        CanonicalObject::Conflict(_) => "Conflict",
        CanonicalObject::Product(_) => "Product",
    }
}

pub(super) fn entity_kind(record: &EntityRecord) -> &'static str {
    match record {
        EntityRecord::Space(_) => "Space",
        EntityRecord::Layer(_) => "Layer",
        EntityRecord::StepObservation(_) => "StepObservation",
        EntityRecord::Operation(_) => "Operation",
        EntityRecord::Worktree(_) => "Worktree",
        EntityRecord::Marker(_) => "Marker",
        EntityRecord::Release(_) => "Release",
        EntityRecord::ReleasePublication(_) => "ReleasePublication",
    }
}

pub(super) fn graph_corruption(error: impl std::fmt::Display) -> StoreError {
    StoreError::Corruption {
        message: format!("invalid candidate graph: {error}"),
    }
}
