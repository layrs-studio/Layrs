//! Deterministic history pages pinned to immutable roots. Captures between page
//! requests cannot insert rows into an existing traversal. No rename inference.
use super::{NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::{
    ActorId, Operation, OperationId, RepoPath, Step, StepId, StepObservation, Timestamp, TreeEntry,
    TreeEntryTarget, TreeId,
};
use layrs_store::scm::{ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::{self, NativeScmReader, ObservationSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepHistoryCursor {
    pub anchor: StepId,
    pub offset: usize,
    pub path: Option<String>,
    pub filter: StepHistoryFilter,
    pub observation_snapshot: ObservationSnapshot,
}
/// All constraints must match the same local observation. These are claims
/// recorded locally, not authenticated authorship of the immutable Step.
pub type StepHistoryFilter = OperationHistoryFilter;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepHistoryPage {
    pub anchor: StepId,
    pub steps: Vec<Step>,
    pub observation_snapshot: ObservationSnapshot,
    pub observations: BTreeMap<StepId, Vec<StepObservation>>,
    pub next: Option<StepHistoryCursor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationHistoryCursor {
    pub anchor: OperationId,
    pub offset: usize,
    pub search: String,
    #[serde(default)]
    pub filter: OperationHistoryFilter,
}
/// Exact actor and inclusive instants. Timestamps belong to Operations, never Steps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationHistoryFilter {
    pub actor: Option<ActorId>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryPage {
    pub anchor: OperationId,
    pub operations: Vec<Operation>,
    pub next: Option<OperationHistoryCursor>,
}

impl NativeWorkspace {
    /// The selector is consulted only for the first page. A cursor retains that
    /// root even when its Layer moves. Exact paths; no speculative rename links.
    pub fn step_history(
        &self,
        selector: &str,
        cursor: Option<&StepHistoryCursor>,
        limit: usize,
        path: Option<&str>,
    ) -> Result<StepHistoryPage> {
        self.step_history_filtered(selector, cursor, limit, path, &StepHistoryFilter::default())
    }

    pub fn step_history_filtered(
        &self,
        selector: &str,
        cursor: Option<&StepHistoryCursor>,
        limit: usize,
        path: Option<&str>,
        filter: &StepHistoryFilter,
    ) -> Result<StepHistoryPage> {
        validate_page(limit, cursor.map_or(0, |c| c.offset))?;
        let since = filter.since.as_ref().map(instant).transpose()?;
        let until = filter.until.as_ref().map(instant).transpose()?;
        if since.zip(until).is_some_and(|(a, b)| a > b) {
            return Err(NativeClientError::InvalidState(
                "history since must precede until",
            ));
        }
        if let Some(path) = path {
            RepoPath::parse(path)?;
        }
        if cursor.is_some_and(|c| c.path.as_deref() != path || c.filter != *filter) {
            return Err(NativeClientError::InvalidState(
                "history cursor path differs from query",
            ));
        }
        let anchor = match cursor {
            Some(c) => c.anchor.clone(),
            None => self.resolve_step_name(selector)?.step_id,
        };
        let offset = cursor.map_or(0, |c| c.offset);
        let state = self.state()?;
        let store = scm_v1::open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let projection = reader
            .observation_projection(state.space.id(), cursor.map(|c| &c.observation_snapshot))?;
        let filtered = *filter != StepHistoryFilter::default();
        let mut observations = BTreeMap::new();
        let mut pending = vec![anchor.clone()];
        let mut visited = BTreeSet::new();
        let mut rows = vec![];
        let mut seen = 0;
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            if visited.len() > 100_000 {
                return Err(NativeClientError::InvalidState(
                    "history traversal limit exceeded",
                ));
            }
            if !reader
                .object_admitted_to_space(&StoreObjectId::Step(id.clone()), state.space.id())?
            {
                return Err(NativeClientError::Missing("Step admitted to this Space"));
            }
            let step = reader
                .get_step(&id)?
                .ok_or(NativeClientError::Missing("history Step"))?;
            pending.extend(step.parent_step_ids().iter().rev().cloned());
            let step_observations = projection.by_step.get(&id).cloned().unwrap_or_default();
            let mut observation_matches = !filtered;
            for observation in &step_observations {
                let observed = instant(observation.observed_at())?;
                observation_matches |= filter
                    .actor
                    .as_ref()
                    .is_none_or(|actor| observation.actor_id() == Some(actor))
                    && since.is_none_or(|start| observed >= start)
                    && until.is_none_or(|end| observed <= end);
            }
            if !observation_matches {
                continue;
            }
            let matches = if let Some(path) = path {
                let current = path_entry(&reader, step.root_tree_id(), path)?;
                let mut changed = step.parent_step_ids().is_empty() && current.is_some();
                for parent in step.parent_step_ids() {
                    if !reader.object_admitted_to_space(
                        &StoreObjectId::Step(parent.clone()),
                        state.space.id(),
                    )? {
                        return Err(NativeClientError::Missing("parent admitted to Space"));
                    }
                    let parent = reader
                        .get_step(parent)?
                        .ok_or(NativeClientError::Missing("history parent"))?;
                    changed |= current != path_entry(&reader, parent.root_tree_id(), path)?;
                }
                changed
            } else {
                true
            };
            if !matches {
                continue;
            }
            if seen >= offset {
                if rows.len() == limit {
                    return Ok(StepHistoryPage {
                        anchor: anchor.clone(),
                        steps: rows,
                        observation_snapshot: projection.snapshot.clone(),
                        observations,
                        next: Some(StepHistoryCursor {
                            anchor,
                            offset: seen,
                            path: path.map(str::to_owned),
                            filter: filter.clone(),
                            observation_snapshot: projection.snapshot,
                        }),
                    });
                }
                observations.insert(id, step_observations);
                rows.push(step);
            }
            seen += 1;
        }
        Ok(StepHistoryPage {
            anchor,
            steps: rows,
            observation_snapshot: projection.snapshot,
            observations,
            next: None,
        })
    }

    pub fn operation_history(
        &self,
        cursor: Option<&OperationHistoryCursor>,
        limit: usize,
        search: &str,
    ) -> Result<OperationHistoryPage> {
        self.operation_history_filtered(cursor, limit, search, &OperationHistoryFilter::default())
    }

    pub fn operation_history_filtered(
        &self,
        cursor: Option<&OperationHistoryCursor>,
        limit: usize,
        search: &str,
        filter: &OperationHistoryFilter,
    ) -> Result<OperationHistoryPage> {
        validate_page(limit, cursor.map_or(0, |c| c.offset))?;
        let since = filter.since.as_ref().map(instant).transpose()?;
        let until = filter.until.as_ref().map(instant).transpose()?;
        if since.zip(until).is_some_and(|(a, b)| a > b) {
            return Err(NativeClientError::InvalidState(
                "history since must precede until",
            ));
        }
        if search.len() > 1024 || cursor.is_some_and(|c| c.search != search || c.filter != *filter)
        {
            return Err(NativeClientError::InvalidState(
                "invalid history search or cursor",
            ));
        }
        let state = self.state()?;
        let anchor = cursor.map_or_else(|| state.operation.id().clone(), |c| c.anchor.clone());
        let offset = cursor.map_or(0, |c| c.offset);
        let store = scm_v1::open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let mut pending = vec![anchor.clone()];
        let mut visited = BTreeSet::new();
        let mut rows = vec![];
        let mut seen = 0;
        let query = search.to_lowercase();
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            if visited.len() > 100_000 {
                return Err(NativeClientError::InvalidState(
                    "history traversal limit exceeded",
                ));
            }
            let operation = reader
                .get_operation(&id)?
                .ok_or(NativeClientError::Missing("history Operation"))?;
            if operation.space_id() != state.space.id() {
                return Err(NativeClientError::InvalidState(
                    "Operation belongs to another Space",
                ));
            }
            pending.extend(operation.parent_operation_ids().iter().rev().cloned());
            let created = instant(operation.created_at())?;
            if filter
                .actor
                .as_ref()
                .is_some_and(|actor| operation.actor_id() != Some(actor))
                || since.is_some_and(|start| created < start)
                || until.is_some_and(|end| created > end)
            {
                continue;
            }
            if !query.is_empty()
                && !operation.summary().to_lowercase().contains(&query)
                && !operation.id().as_str().contains(&query)
                && !operation
                    .actor_id()
                    .is_some_and(|a| a.as_str().contains(&query))
            {
                continue;
            }
            if seen >= offset {
                if rows.len() == limit {
                    return Ok(OperationHistoryPage {
                        anchor: anchor.clone(),
                        operations: rows,
                        next: Some(OperationHistoryCursor {
                            anchor,
                            offset: seen,
                            search: search.into(),
                            filter: filter.clone(),
                        }),
                    });
                }
                rows.push(operation);
            }
            seen += 1;
        }
        Ok(OperationHistoryPage {
            anchor,
            operations: rows,
            next: None,
        })
    }
}
fn instant(timestamp: &Timestamp) -> Result<time::OffsetDateTime> {
    time::OffsetDateTime::parse(
        timestamp.as_str(),
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| NativeClientError::InvalidState("invalid history timestamp"))
}
fn validate_page(limit: usize, offset: usize) -> Result<()> {
    if !(1..=500).contains(&limit) || offset > 100_000 {
        return Err(NativeClientError::InvalidState(
            "history page requires limit 1..500 and offset <=100000",
        ));
    }
    Ok(())
}
fn path_entry(reader: &impl ScmReader, root: &TreeId, path: &str) -> Result<Option<TreeEntry>> {
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() > 128 {
        return Err(NativeClientError::InvalidState("history path depth limit"));
    }
    let mut id = root.clone();
    for (index, part) in parts.iter().enumerate() {
        let tree = reader
            .get_tree(&id)?
            .ok_or(NativeClientError::Missing("history Tree"))?;
        let Some(entry) = tree
            .entries()
            .iter()
            .find(|entry| entry.name().as_str() == *part)
        else {
            return Ok(None);
        };
        if index + 1 == parts.len() {
            return Ok(Some(entry.clone()));
        }
        let TreeEntryTarget::Tree { tree_id } = entry.target() else {
            return Ok(None);
        };
        id = tree_id.clone();
    }
    Ok(None)
}
