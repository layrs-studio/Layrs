use std::sync::Arc;

use serde::{Deserialize, Serialize};

use layrs_store::scm::{RequestDigest, StoreError, TransactionId, WriteBatch};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryFaultPoint {
    AfterEachCas,
    AfterAllCas,
    AfterWriterOpen,
    AfterBeginImmediate,
    AfterReplayCheck,
    AfterValidation,
    AfterObjects,
    AfterEntitiesAndProjections,
    AfterAdmissions,
    AfterRepositoryViewCas,
    AfterOperationHeadsCas,
    AfterWorktreeCas,
    BeforeReceipt,
    AfterReceiptBeforeCommit,
    AfterCommit,
}

pub trait RepositoryFaultInjector: Send + Sync {
    fn check(&self, point: RepositoryFaultPoint) -> std::result::Result<(), String>;
}

pub(crate) struct NoRepositoryFaults;

impl RepositoryFaultInjector for NoRepositoryFaults {
    fn check(&self, _point: RepositoryFaultPoint) -> std::result::Result<(), String> {
        Ok(())
    }
}

pub(crate) fn no_faults() -> Arc<dyn RepositoryFaultInjector> {
    Arc::new(NoRepositoryFaults)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApplyBatchReceipt {
    schema_version: String,
    outcome: String,
    transaction_id: String,
    request_digest: String,
    object_count: usize,
    entity_count: usize,
    admission_count: usize,
    mutation_count: usize,
}

impl ApplyBatchReceipt {
    pub(crate) fn new(batch: &WriteBatch) -> Self {
        Self {
            schema_version: "layrs.apply-batch-receipt.v1".into(),
            outcome: "applied".into(),
            transaction_id: batch.transaction_id().to_string(),
            request_digest: batch.request_digest().to_string(),
            object_count: batch.canonical_objects().len(),
            entity_count: batch.entity_records().len(),
            admission_count: batch.admissions().len(),
            mutation_count: batch.mutations().len(),
        }
    }

    pub(crate) fn validate(
        &self,
        transaction_id: &TransactionId,
        digest: RequestDigest,
        batch: &WriteBatch,
    ) -> Result<(), StoreError> {
        if self.schema_version != "layrs.apply-batch-receipt.v1"
            || self.outcome != "applied"
            || self.transaction_id != transaction_id.as_str()
            || self.request_digest != digest.to_string()
            || self.object_count != batch.canonical_objects().len()
            || self.entity_count != batch.entity_records().len()
            || self.admission_count != batch.admissions().len()
            || self.mutation_count != batch.mutations().len()
        {
            return Err(StoreError::Corruption {
                message: format!(
                    "applied transaction receipt for {transaction_id} is inconsistent"
                ),
            });
        }
        Ok(())
    }
}
