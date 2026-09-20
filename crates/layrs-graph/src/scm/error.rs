use layrs_core::scm::{LayerId, OperationId, SpaceId, StepId};
use std::error::Error;
use std::fmt;

/// The kind of node involved in a generic graph invariant failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphObjectKind {
    Step,
    Operation,
}

impl fmt::Display for GraphObjectKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Step => "Step",
            Self::Operation => "Operation",
        })
    }
}

/// Failures produced while constructing or querying SCM graphs.
///
/// Every variant carries deterministic witnesses: when several nodes could
/// witness a failure, the lexicographically smallest identifier is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScmGraphError {
    DuplicateStep(StepId),
    DuplicateStepParent {
        step_id: StepId,
        parent_id: StepId,
    },
    MissingStep {
        step_id: StepId,
        referenced_id: StepId,
    },
    DuplicateOperation(OperationId),
    DuplicateOperationParent {
        operation_id: OperationId,
        parent_id: OperationId,
    },
    MissingOperation {
        operation_id: OperationId,
        referenced_id: OperationId,
    },
    SelfParent {
        kind: GraphObjectKind,
        id: String,
    },
    Cycle {
        kind: GraphObjectKind,
        id: String,
    },
    DuplicateHead(StepId),
    LayerHeadsNotAntichain {
        ancestor: StepId,
        descendant: StepId,
    },
    DuplicateLayerHeadSet(LayerId),
    InvalidInitializeParents(OperationId),
    DuplicateSpaceInitialization {
        space_id: SpaceId,
        first: OperationId,
        duplicate: OperationId,
    },
    OperationRequiresParent(OperationId),
    IntegrationRequiresMultipleParents(OperationId),
    IntegrationRequiresDedicatedAdmission(OperationId),
    OperationSpaceMismatch {
        operation_id: OperationId,
        parent_id: OperationId,
        operation_space: SpaceId,
        parent_space: SpaceId,
    },
    NotIntegrationOperation(OperationId),
    DuplicateIntegrationParent(OperationId),
    IntegrationParentSpaceMismatch {
        operation_id: OperationId,
        parent_id: OperationId,
    },
    IntegrationParentsMismatch {
        operation_id: OperationId,
        expected: Vec<OperationId>,
        actual: Vec<OperationId>,
    },
    InternalInvariant(&'static str),
}

impl ScmGraphError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DuplicateStep(_) => "duplicate_step",
            Self::DuplicateStepParent { .. } => "duplicate_step_parent",
            Self::MissingStep { .. } => "missing_step",
            Self::DuplicateOperation(_) => "duplicate_operation",
            Self::DuplicateOperationParent { .. } => "duplicate_operation_parent",
            Self::MissingOperation { .. } => "missing_operation",
            Self::SelfParent { .. } => "self_parent",
            Self::Cycle { .. } => "graph_cycle",
            Self::DuplicateHead(_) => "duplicate_head",
            Self::LayerHeadsNotAntichain { .. } => "layer_heads_not_antichain",
            Self::DuplicateLayerHeadSet(_) => "duplicate_layer_head_set",
            Self::InvalidInitializeParents(_) => "initialize_has_parents",
            Self::DuplicateSpaceInitialization { .. } => "duplicate_space_initialization",
            Self::OperationRequiresParent(_) => "operation_requires_parent",
            Self::IntegrationRequiresMultipleParents(_) => "integration_requires_multiple_parents",
            Self::IntegrationRequiresDedicatedAdmission(_) => {
                "integration_requires_dedicated_admission"
            }
            Self::OperationSpaceMismatch { .. } => "operation_space_mismatch",
            Self::NotIntegrationOperation(_) => "not_integration_operation",
            Self::DuplicateIntegrationParent(_) => "duplicate_integration_parent",
            Self::IntegrationParentSpaceMismatch { .. } => "integration_parent_space_mismatch",
            Self::IntegrationParentsMismatch { .. } => "integration_parents_mismatch",
            Self::InternalInvariant(_) => "internal_graph_invariant",
        }
    }
}

impl fmt::Display for ScmGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateStep(id) => write!(formatter, "duplicate Step {id}"),
            Self::DuplicateStepParent { step_id, parent_id } => {
                write!(formatter, "Step {step_id} repeats parent {parent_id}")
            }
            Self::MissingStep {
                step_id,
                referenced_id,
            } => write!(
                formatter,
                "Step {step_id} references missing Step {referenced_id}"
            ),
            Self::DuplicateOperation(id) => write!(formatter, "duplicate Operation {id}"),
            Self::DuplicateOperationParent {
                operation_id,
                parent_id,
            } => write!(
                formatter,
                "Operation {operation_id} repeats parent {parent_id}"
            ),
            Self::MissingOperation {
                operation_id,
                referenced_id,
            } => write!(
                formatter,
                "Operation {operation_id} references missing Operation {referenced_id}"
            ),
            Self::SelfParent { kind, id } => write!(formatter, "{kind} {id} parents itself"),
            Self::Cycle { kind, id } => {
                write!(formatter, "{kind} graph contains a cycle involving {id}")
            }
            Self::DuplicateHead(id) => write!(formatter, "duplicate Layer head {id}"),
            Self::LayerHeadsNotAntichain {
                ancestor,
                descendant,
            } => write!(
                formatter,
                "Layer head {ancestor} is an ancestor of head {descendant}"
            ),
            Self::DuplicateLayerHeadSet(id) => {
                write!(formatter, "duplicate head set for Layer {id}")
            }
            Self::InvalidInitializeParents(id) => {
                write!(formatter, "initialize Operation {id} must have no parents")
            }
            Self::DuplicateSpaceInitialization {
                space_id,
                first,
                duplicate,
            } => write!(
                formatter,
                "Space {space_id} has multiple initialize Operations: {first} and {duplicate}"
            ),
            Self::OperationRequiresParent(id) => {
                write!(formatter, "non-initialize Operation {id} requires a parent")
            }
            Self::IntegrationRequiresMultipleParents(id) => write!(
                formatter,
                "integration Operation {id} requires at least two parents"
            ),
            Self::IntegrationRequiresDedicatedAdmission(id) => write!(
                formatter,
                "integration Operation {id} requires atomic head-set admission"
            ),
            Self::OperationSpaceMismatch {
                operation_id,
                parent_id,
                operation_space,
                parent_space,
            } => write!(
                formatter,
                "Operation {operation_id} in {operation_space} has parent {parent_id} in {parent_space}"
            ),
            Self::NotIntegrationOperation(id) => {
                write!(formatter, "Operation {id} is not an integration")
            }
            Self::DuplicateIntegrationParent(id) => {
                write!(formatter, "duplicate expected integration parent {id}")
            }
            Self::IntegrationParentSpaceMismatch {
                operation_id,
                parent_id,
            } => write!(
                formatter,
                "expected parent {parent_id} is not a current head in the Space of Operation {operation_id}"
            ),
            Self::IntegrationParentsMismatch {
                operation_id,
                expected,
                actual,
            } => write!(
                formatter,
                "Operation {operation_id} integration parents differ: expected {expected:?}, got {actual:?}"
            ),
            Self::InternalInvariant(reason) => {
                write!(formatter, "internal graph invariant failed: {reason}")
            }
        }
    }
}

impl Error for ScmGraphError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn error_is_send_and_sync() {
        assert_send_sync::<ScmGraphError>();
    }
}
