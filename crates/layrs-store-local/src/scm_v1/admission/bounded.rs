//! Admission that reads no history (council s23): what the batch carries and
//! the durable records it names directly.
//!
//! Durably validated history is closed by induction: its Steps and
//! Operations are acyclic, their parents exist in their Space, and a durable
//! record never names one this batch introduces. So the batch's new Steps
//! and Operations are checked against their direct parents only, a cycle can
//! only lie among them, and the publication frontier moves algebraically:
//! `H' = (H - parents(o)) ∪ {o}`, or `H' = {o}` for an integration of exactly
//! `H`. Ancestry is asked only where a rule needs it, by walking parents from
//! the descendant (the durable frontier's membership is walked once, from
//! the heads, and remembered). The complete resolver stays for a Store with
//! an unreadable pack (council s18) and for Weave Products, whose merge bases
//! need the whole Step DAG.
use super::*;
use std::collections::VecDeque;

/// The batch's Steps that are not durable yet, after the checks the Step
/// DAG made on them: no duplicate or self parent, every parent in the batch
/// or durable, no cycle among them (Kahn's order, whose positions are
/// returned: parents first).
pub(super) fn new_batch_steps(
    reader: &dyn ScmReader,
    batch: &WriteBatch,
) -> Result<(BTreeMap<StepId, Step>, HashMap<StepId, usize>)> {
    let carried: BTreeMap<_, _> = batch
        .canonical_objects()
        .iter()
        .filter_map(|object| match object {
            CanonicalObject::Step(value) => Some((value.id().clone(), value)),
            _ => None,
        })
        .collect();
    let mut new_steps = BTreeMap::new();
    for (id, step) in &carried {
        if reader.get_step(id)?.is_some() {
            continue;
        }
        let mut seen = BTreeSet::new();
        for parent in step.parent_step_ids() {
            if !seen.insert(parent) {
                return Err(graph_corruption(format!(
                    "Step {id} has duplicate parent {parent}"
                )));
            }
            if parent == id {
                return Err(graph_corruption(format!("Step {id} is its own parent")));
            }
            if !carried.contains_key(parent) && reader.get_step(parent)?.is_none() {
                return Err(StoreError::NotFound {
                    kind: "Step parent",
                    id: parent.to_string(),
                });
            }
        }
        new_steps.insert(id.clone(), (*step).clone());
    }
    // Bounds on what the batch adds (council s23 review r1), not history.
    require_budget("Step graph nodes", new_steps.len(), MAX_GRAPH_NODES)?;
    let edges = new_steps
        .values()
        .try_fold(0_usize, |sum, step| {
            sum.checked_add(step.parent_step_ids().len())
        })
        .unwrap_or(usize::MAX);
    require_budget("Step graph edges", edges, MAX_GRAPH_EDGES)?;
    let mut indegrees: BTreeMap<&StepId, usize> = BTreeMap::new();
    let mut children: BTreeMap<&StepId, Vec<&StepId>> = BTreeMap::new();
    for (id, step) in &new_steps {
        let mut degree = 0;
        for parent in step.parent_step_ids() {
            if new_steps.contains_key(parent) {
                degree += 1;
                children.entry(parent).or_default().push(id);
            }
        }
        indegrees.insert(id, degree);
    }
    let mut ready: Vec<&StepId> = indegrees
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut ranks = HashMap::with_capacity(new_steps.len());
    while let Some(id) = ready.pop() {
        ranks.insert(id.clone(), ranks.len());
        for child in children.get(id).into_iter().flatten() {
            let degree = indegrees.get_mut(child).expect("a new Step");
            *degree -= 1;
            if *degree == 0 {
                ready.push(child);
            }
        }
    }
    if ranks.len() != new_steps.len() {
        return Err(graph_corruption("new Steps form a cycle"));
    }
    Ok((new_steps, ranks))
}

/// The checks `validate_global_operation_history` made, on the batch's new
/// Operations only: parent counts and duplicates, parents in the same Space
/// (in the batch or durable), one Initialize per Space, and no cycle among
/// them (a cycle cannot reach durable history, which never names them).
pub(super) fn validate_new_operations(
    reader: &dyn ScmReader,
    new_operations: &BTreeMap<OperationId, Operation>,
) -> Result<()> {
    require_budget(
        "Operation graph nodes",
        new_operations.len(),
        MAX_GRAPH_NODES,
    )?;
    let edges = new_operations
        .values()
        .try_fold(0_usize, |sum, operation| {
            sum.checked_add(operation.parent_operation_ids().len())
        })
        .unwrap_or(usize::MAX);
    require_budget("Operation graph edges", edges, MAX_GRAPH_EDGES)?;
    let mut initializers = BTreeMap::<&SpaceId, &OperationId>::new();
    let mut indegrees = BTreeMap::<&OperationId, usize>::new();
    let mut children = BTreeMap::<&OperationId, Vec<&OperationId>>::new();
    for (id, operation) in new_operations {
        let parents: BTreeSet<_> = operation.parent_operation_ids().iter().collect();
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
                if let Some(first) = initializers.insert(operation.space_id(), id) {
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
        let mut new_parents = 0;
        for parent in parents {
            if parent == id {
                return Err(candidate_graph_rejected(format!(
                    "Operation {id} is its own parent"
                )));
            }
            let space = match new_operations.get(parent) {
                Some(value) => {
                    new_parents += 1;
                    children.entry(parent).or_default().push(id);
                    value.space_id().clone()
                }
                None => reader
                    .get_operation(parent)?
                    .ok_or_else(|| StoreError::NotFound {
                        kind: "Operation parent",
                        id: parent.to_string(),
                    })?
                    .space_id()
                    .clone(),
            };
            if &space != operation.space_id() {
                return Err(candidate_graph_rejected(format!(
                    "Operation {id} has parent {parent} in another Space"
                )));
            }
        }
        indegrees.insert(id, new_parents);
    }
    // Kahn's order over the new Operations: what is left lies on a cycle.
    let mut ready: Vec<_> = indegrees
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut ordered = 0;
    while let Some(id) = ready.pop() {
        ordered += 1;
        for child in children.get(id).into_iter().flatten() {
            let degree = indegrees.get_mut(child).expect("a new Operation");
            *degree -= 1;
            if *degree == 0 {
                ready.push(child);
            }
        }
    }
    if ordered != new_operations.len() {
        return Err(candidate_graph_rejected("new Operations form a cycle"));
    }
    Ok(())
}

/// The durable publication frontier of each Space, answered without loading
/// its history: its heads, and the Operations proven reachable from them,
/// found by one remembered walk from the heads.
pub(super) struct LazyFrontier {
    heads: BTreeMap<SpaceId, Vec<OperationId>>,
    walks: Mutex<HashMap<SpaceId, FrontierWalk>>,
}

struct FrontierWalk {
    reached: HashSet<OperationId>,
    pending: VecDeque<OperationId>,
}

impl LazyFrontier {
    pub(super) fn new(heads: BTreeMap<SpaceId, Vec<OperationId>>) -> Self {
        Self {
            heads,
            walks: Mutex::new(HashMap::new()),
        }
    }

    /// Whether the durable `operation` is a head of its Space or one of
    /// their ancestors (the complete resolver's `durable_operation_dag`).
    /// Breadth first from the heads: recent Operations are found first.
    pub(super) fn retains(&self, reader: &dyn ScmReader, operation: &Operation) -> Result<bool> {
        let space = operation.space_id();
        let mut walks = self.walks.lock().map_err(|_| poisoned_frontier())?;
        let walk = walks.entry(space.clone()).or_insert_with(|| FrontierWalk {
            reached: HashSet::new(),
            pending: self
                .heads
                .get(space)
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
        });
        loop {
            if walk.reached.contains(operation.id()) {
                return Ok(true);
            }
            let Some(next) = walk.pending.pop_front() else {
                return Ok(false);
            };
            if !walk.reached.insert(next.clone()) {
                continue;
            }
            let value = reader
                .get_operation(&next)?
                .ok_or_else(|| StoreError::Corruption {
                    message: format!("durable frontier Operation {next} is missing"),
                })?;
            if value.space_id() != space {
                return Err(StoreError::Corruption {
                    message: format!("durable frontier Operation {next} belongs to another Space"),
                });
            }
            for parent in value.parent_operation_ids() {
                if !walk.reached.contains(parent) {
                    walk.pending.push_back(parent.clone());
                }
            }
        }
    }

    /// Whether `ancestor` is `descendant` or one of its durable ancestors.
    pub(super) fn is_ancestor_or_equal(
        &self,
        reader: &dyn ScmReader,
        ancestor: &OperationId,
        descendant: &OperationId,
    ) -> Result<bool> {
        if ancestor == descendant {
            return Ok(reader.get_operation(descendant)?.is_some());
        }
        let mut seen = HashSet::new();
        let mut pending = VecDeque::from([descendant.clone()]);
        while let Some(id) = pending.pop_front() {
            let Some(value) = reader.get_operation(&id)? else {
                // Retained Operations are durable: one missing is damage,
                // not a refusal (council s23 review r2).
                return Err(StoreError::Corruption {
                    message: format!("durable Operation {id} is missing"),
                });
            };
            for parent in value.parent_operation_ids() {
                if parent == ancestor {
                    return Ok(true);
                }
                if seen.insert(parent.clone()) {
                    pending.push_back(parent.clone());
                }
            }
        }
        Ok(false)
    }

    /// The Space's heads once the new `operation` is published over the
    /// complete durable `heads`, or why it cannot be (the complete DAG's
    /// `insert_batch` / `insert_integration` rules).
    pub(super) fn publish(
        &self,
        reader: &dyn ScmReader,
        operation: &Operation,
        heads: &[OperationId],
    ) -> Result<std::result::Result<Vec<OperationId>, String>> {
        let id = operation.id();
        let parents = operation.parent_operation_ids();
        if operation.kind() == OperationKind::IntegrateOperations {
            // The parents are exactly the complete current heads, in order.
            let expected: BTreeSet<_> = heads.iter().collect();
            if expected.len() != heads.len() {
                return Ok(Err(format!("duplicate integration parent in {heads:?}")));
            }
            let expected: Vec<_> = expected.into_iter().cloned().collect();
            if parents != expected.as_slice() {
                return Ok(Err(format!(
                    "integration parents {parents:?} differ from the current heads {expected:?}"
                )));
            }
            if parents.len() < 2 {
                return Ok(Err(format!("integration {id} requires multiple parents")));
            }
            return Ok(Ok(vec![id.clone()]));
        }
        match operation.kind() {
            OperationKind::Initialize if !parents.is_empty() => {
                return Ok(Err(format!("Initialize Operation {id} has parents")));
            }
            OperationKind::Initialize => {}
            _ if parents.is_empty() => {
                return Ok(Err(format!("Operation {id} requires a parent")));
            }
            _ => {}
        }
        for parent in parents {
            if parent == id {
                return Ok(Err(format!("Operation {id} is its own parent")));
            }
            let Some(value) = reader.get_operation(parent)? else {
                return Ok(Err(format!("Operation {id} references missing {parent}")));
            };
            if value.space_id() != operation.space_id() {
                return Ok(Err(format!(
                    "Operation {id} has parent {parent} in another Space"
                )));
            }
            if !self.retains(reader, &value)? {
                return Ok(Err(format!(
                    "Operation {id} references {parent}, outside the durable frontier"
                )));
            }
        }
        let mut next: BTreeSet<OperationId> = heads
            .iter()
            .filter(|head| !parents.contains(head))
            .cloned()
            .collect();
        next.insert(id.clone());
        Ok(Ok(next.into_iter().collect()))
    }
}

fn poisoned_frontier() -> StoreError {
    StoreError::Backend {
        message: "durable frontier walk poisoned".into(),
    }
}
