//! Caller-directed recovery from durable journal evidence.

use crate::error::{StoreError, StoreResult};
use crate::journal::{JournalOperation, JournalPath, JournalState};
use crate::repository::MetadataRepository;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    Advance(JournalState),
    NeedsRepair(String),
    LeaveUnchanged,
}

pub trait RecoveryHandler {
    /// Decides using durable evidence only. The handler must not assume an
    /// external side effect happened merely because an operation was prepared.
    fn decide(
        &mut self,
        operation: &JournalOperation,
        paths: &[JournalPath],
    ) -> StoreResult<RecoveryAction>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryResult {
    pub operation_id: String,
    pub previous_state: JournalState,
    pub resulting_state: JournalState,
    pub changed: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub operations: Vec<RecoveryResult>,
}

pub fn recover_operations(
    repository: &mut MetadataRepository,
    handler: &mut dyn RecoveryHandler,
    updated_at_ms: i64,
) -> StoreResult<RecoveryReport> {
    let operations = repository.list_recoverable_operations()?;
    let mut report = RecoveryReport::default();

    for operation in operations {
        let paths = repository.load_operation_paths(&operation.operation_id)?;
        let action = handler.decide(&operation, &paths)?;
        let (resulting_state, changed) = match action {
            RecoveryAction::LeaveUnchanged => (operation.state, false),
            RecoveryAction::Advance(next) => {
                let transitioned = repository.transition_operation(
                    &operation.operation_id,
                    operation.state,
                    next,
                    updated_at_ms,
                    None,
                )?;
                (transitioned.state, true)
            }
            RecoveryAction::NeedsRepair(reason) => {
                if reason.trim().is_empty() {
                    return Err(StoreError::InvalidState(
                        "needs-repair recovery reason must not be empty".into(),
                    ));
                }
                let transitioned = repository.transition_operation(
                    &operation.operation_id,
                    operation.state,
                    JournalState::NeedsRepair,
                    updated_at_ms,
                    Some(&reason),
                )?;
                (transitioned.state, true)
            }
        };
        report.operations.push(RecoveryResult {
            operation_id: operation.operation_id,
            previous_state: operation.state,
            resulting_state,
            changed,
        });
    }
    Ok(report)
}
