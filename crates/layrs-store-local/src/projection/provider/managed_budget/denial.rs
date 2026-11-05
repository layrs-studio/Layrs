//! Exact current-document column sizing. This is DATA, not funding authority.
use super::*;
use crate::projection::provider::records::{
    denied_coverage::{DeniedCoverageRef, TransitionDeniedCoverageRecord},
    transition::TransitionPlan,
};

/// Construct from the complete body, never from path-byte/count hints or a
/// compact reference alone. Enclosing SQL/native readers authenticate owners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection::provider) struct DeniedCoverageCost {
    reference: DeniedCoverageRef,
    document_bytes: u64,
}
impl DeniedCoverageCost {
    pub(in crate::projection::provider) fn new(
        intent: &ProviderTransitionIntentRecord,
        body: &TransitionDeniedCoverageRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        body.require_intent(intent, cancel)?;
        let document_bytes = add_metadata(0, body.normalized_column_bytes()?)?;
        let reference = body.reference()?;
        cancellation::check(cancel)?;
        Ok(Self {
            reference,
            document_bytes,
        })
    }
    pub(in crate::projection::provider) fn document_bytes(&self) -> u64 {
        self.document_bytes
    }
}

pub(super) fn add_current_document(
    copied_bytes: u64,
    plan: &TransitionPlan,
    coverage: Option<&DeniedCoverageCost>,
) -> Result<u64> {
    match (&plan.denial_coverage, coverage) {
        (None, None) => add_metadata(copied_bytes, 0),
        (Some(reference), Some(actual)) if reference == &actual.reference => {
            add_metadata(copied_bytes, actual.document_bytes)
        }
        _ => Err(invalid(
            "current denial document sizing is missing or differs from plan",
        )),
    }
}
