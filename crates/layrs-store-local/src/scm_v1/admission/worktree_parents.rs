//! A retained parent is usable only through a durable Worktree binding.
use super::*;
use layrs_core::scm::WorktreeStatus;
use layrs_store::scm::OperationHeadsCas;

impl CompositeAdmissionResolver<'_> {
    pub(super) fn validate_worktree_parent(
        &self,
        operation: &Operation,
        cas: &OperationHeadsCas,
    ) -> Result<()> {
        let changes = self
            .batch
            .mutations()
            .iter()
            .filter_map(|mutation| match mutation {
                StoreMutation::WorktreeActiveOperation(change)
                    if change.new_operation_id() == operation.id() =>
                {
                    Some(change)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for change in &changes {
            if !operation
                .parent_operation_ids()
                .contains(change.expected_operation_id())
            {
                return Err(invalid_operation_frontier(
                    operation,
                    "Worktree active CAS is not a parent of its published Operation",
                ));
            }
            self.validate_durable_worktree_binding(operation, change)?;
        }
        // Integration alone may advance multiple Worktrees. Its complete-head
        // parent set is checked by both the batch contract and frontier DAG.
        if operation.kind() == OperationKind::IntegrateOperations {
            return Ok(());
        }
        if !changes.is_empty()
            && (changes.len() != 1 || operation.parent_operation_ids().len() != 1)
        {
            return Err(invalid_operation_frontier(
                operation,
                "normal publication must advance exactly one own Worktree",
            ));
        }
        let outside_frontier = operation
            .parent_operation_ids()
            .iter()
            .any(|parent| !cas.expected_head_ids().contains(parent));
        if !outside_frontier && changes.is_empty() && cas.worktree_registration_source().is_none() {
            return Ok(());
        }
        let [parent_id] = operation.parent_operation_ids() else {
            return Err(invalid_operation_frontier(
                operation,
                "retained Worktree publication requires exactly one parent",
            ));
        };
        // Never authorize through the batch overlay, passive imports or a
        // fabricated same-batch Worktree/Operation. This parent must already be
        // durable and reachable from the complete durable publication frontier.
        if !self.durable_operation_dag.contains(parent_id) {
            return Err(invalid_operation_frontier(
                operation,
                "Worktree parent is not retained by the durable frontier",
            ));
        }
        let parent = self.reader.get_operation(parent_id)?.ok_or_else(|| {
            invalid_operation_frontier(operation, "missing durable Worktree parent")
        })?;
        if parent.space_id() != operation.space_id() {
            return Err(invalid_operation_frontier(
                operation,
                "cross-Space Worktree parent",
            ));
        }
        let Some(CanonicalObject::RepositoryView(result)) = self.object(
            &StoreObjectId::RepositoryView(operation.resulting_view_id().clone()),
        )?
        else {
            return Err(invalid_operation_frontier(
                operation,
                "missing resulting Worktree View",
            ));
        };
        let parent_view = self
            .reader
            .get_repository_view(parent.resulting_view_id())?
            .ok_or_else(|| invalid_operation_frontier(operation, "missing durable parent View"))?;

        if let Some(source_id) = cas.worktree_registration_source() {
            let source = self.reader.get_worktree(source_id)?.ok_or_else(|| {
                invalid_operation_frontier(
                    operation,
                    "missing durable registration source Worktree",
                )
            })?;
            if source.space_id() != operation.space_id()
                || !parent_view
                    .worktrees()
                    .iter()
                    .any(|state| state.worktree_id() == source_id)
                || !self
                    .durable_operation_dag
                    .is_ancestor_or_equal(parent_id, source.active_operation_id())
                    .map_err(|error| invalid_operation_frontier(operation, error.to_string()))?
            {
                return Err(invalid_operation_frontier(
                    operation,
                    "registration parent is not in the source Worktree's retained history",
                ));
            }
            let additions = self
                .batch
                .entity_records()
                .iter()
                .filter_map(|record| match record {
                    EntityRecord::Worktree(worktree)
                        if worktree.active_operation_id() == operation.id() =>
                    {
                        Some(worktree)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let [created] = additions.as_slice() else {
                return Err(invalid_operation_frontier(
                    operation,
                    "registration must bind exactly one new Worktree",
                ));
            };
            if operation.kind() != OperationKind::Restore
                || created.space_id() != operation.space_id()
                || self.reader.get_worktree(created.id())?.is_some()
                || !result.worktrees().iter().any(|state| {
                    state.worktree_id() == created.id() && state.status() == WorktreeStatus::Stale
                })
                || self
                    .batch
                    .mutations()
                    .iter()
                    .any(|m| matches!(m, StoreMutation::WorktreeActiveOperation(_)))
            {
                return Err(invalid_operation_frontier(
                    operation,
                    "registration must create a Stale Worktree without moving existing Worktrees",
                ));
            }
            return Ok(());
        }

        if changes.len() != 1 || changes[0].expected_operation_id() != parent_id {
            return Err(invalid_operation_frontier(
                operation,
                "retained-parent publication must advance exactly its one bound Worktree",
            ));
        }
        for change in changes {
            let Some(worktree) = self.reader.get_worktree(change.worktree_id())? else {
                continue;
            };
            if worktree.space_id() == operation.space_id()
                && worktree.active_operation_id() == parent_id
                && parent_view
                    .worktrees()
                    .iter()
                    .any(|state| state.worktree_id() == worktree.id())
                && result
                    .worktrees()
                    .iter()
                    .any(|state| state.worktree_id() == worktree.id())
            {
                return Ok(());
            }
        }
        Err(invalid_operation_frontier(
            operation,
            "retained parent is not the durable active Operation of the CAS-bound Worktree",
        ))
    }

    fn validate_durable_worktree_binding(
        &self,
        operation: &Operation,
        change: &layrs_store::scm::WorktreeActiveOperationCas,
    ) -> Result<()> {
        let worktree = self
            .reader
            .get_worktree(change.worktree_id())?
            .ok_or_else(|| {
                invalid_operation_frontier(operation, "missing durable Worktree CAS target")
            })?;
        let parent = self
            .reader
            .get_operation(change.expected_operation_id())?
            .ok_or_else(|| {
                invalid_operation_frontier(operation, "missing durable Worktree CAS parent")
            })?;
        let parent_view = self
            .reader
            .get_repository_view(parent.resulting_view_id())?
            .ok_or_else(|| {
                invalid_operation_frontier(operation, "missing durable Worktree parent View")
            })?;
        let Some(CanonicalObject::RepositoryView(result)) = self.object(
            &StoreObjectId::RepositoryView(operation.resulting_view_id().clone()),
        )?
        else {
            return Err(invalid_operation_frontier(
                operation,
                "missing resulting Worktree View",
            ));
        };
        if worktree.space_id() != operation.space_id()
            || parent.space_id() != operation.space_id()
            || worktree.active_operation_id() != change.expected_operation_id()
            || !self.durable_operation_dag.contains(parent.id())
            || !parent_view
                .worktrees()
                .iter()
                .any(|state| state.worktree_id() == worktree.id())
            || !result
                .worktrees()
                .iter()
                .any(|state| state.worktree_id() == worktree.id())
        {
            return Err(invalid_operation_frontier(
                operation,
                "publication does not preserve its durable own-active Worktree binding",
            ));
        }
        Ok(())
    }
}
