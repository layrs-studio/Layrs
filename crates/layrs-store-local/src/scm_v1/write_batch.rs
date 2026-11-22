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
    /// A pack consolidation after the batch registered its pack, before its
    /// retired files are removed (ADR 0028, section 6).
    AfterConsolidationCommit,
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

    /// Format 8's compact receipt (council s25, phase 2c): a version, the
    /// outcome and the four counts as LEB128; the transaction id and request
    /// digest are the row's key and digest columns.
    pub(crate) fn compact_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![1_u8, 1_u8];
        for count in [
            self.object_count,
            self.entity_count,
            self.admission_count,
            self.mutation_count,
        ] {
            let mut value = count as u64;
            loop {
                let byte = (value & 0x7f) as u8;
                value >>= 7;
                if value == 0 {
                    bytes.push(byte);
                    break;
                }
                bytes.push(byte | 0x80);
            }
        }
        bytes
    }

    /// The receipt a compact row stores (`compact_bytes`), or why not.
    pub(crate) fn from_compact(
        transaction_id: &str,
        request_digest: &str,
        bytes: &[u8],
    ) -> Result<Self, String> {
        let [1, 1, rest @ ..] = bytes else {
            return Err("unknown compact receipt version or outcome".into());
        };
        let mut counts = [0_usize; 4];
        let mut at = 0;
        for count in &mut counts {
            let mut value = 0_u64;
            let mut shift = 0;
            loop {
                let byte = *rest.get(at).ok_or("compact receipt is truncated")?;
                at += 1;
                let bits = u64::from(byte & 0x7f);
                // The tenth byte holds only bit 63.
                if shift > 63 || (shift == 63 && bits > 1) {
                    return Err("compact receipt count overflows".into());
                }
                value |= bits << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            *count = usize::try_from(value).map_err(|error| error.to_string())?;
        }
        if at != rest.len() {
            return Err("compact receipt has trailing bytes".into());
        }
        let receipt = Self {
            schema_version: "layrs.apply-batch-receipt.v1".into(),
            outcome: "applied".into(),
            transaction_id: transaction_id.to_owned(),
            request_digest: request_digest.to_owned(),
            object_count: counts[0],
            entity_count: counts[1],
            admission_count: counts[2],
            mutation_count: counts[3],
        };
        // Its encoding is the only one (no padded varints).
        if receipt.compact_bytes() != bytes {
            return Err("compact receipt is not canonical".into());
        }
        Ok(receipt)
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

#[cfg(test)]
mod tests {
    use super::ApplyBatchReceipt;

    fn receipt(counts: [usize; 4]) -> ApplyBatchReceipt {
        ApplyBatchReceipt {
            schema_version: "layrs.apply-batch-receipt.v1".into(),
            outcome: "applied".into(),
            transaction_id: "transaction:compact".into(),
            request_digest: format!("blake3:{}", "ab".repeat(32)),
            object_count: counts[0],
            entity_count: counts[1],
            admission_count: counts[2],
            mutation_count: counts[3],
        }
    }

    fn decode(bytes: &[u8]) -> Result<ApplyBatchReceipt, String> {
        ApplyBatchReceipt::from_compact(
            "transaction:compact",
            &format!("blake3:{}", "ab".repeat(32)),
            bytes,
        )
    }

    /// Format 8's compact receipt decodes to the same receipt, and only its
    /// one encoding decodes (phase 2c).
    #[test]
    fn compact_receipts_round_trip_and_refuse_other_encodings() {
        for counts in [
            [0, 0, 0, 0],
            [1, 2, 3, 4],
            [127, 128, 16_383, 16_384],
            [usize::MAX, 0, usize::MAX, 1],
        ] {
            let expected = receipt(counts);
            let bytes = expected.compact_bytes();
            assert!((6..=42).contains(&bytes.len()), "{counts:?}");
            assert_eq!(decode(&bytes).unwrap(), expected, "{counts:?}");
        }
        let bytes = receipt([1, 2, 3, 4]).compact_bytes();
        assert_eq!(bytes, [1, 1, 1, 2, 3, 4]);
        for damaged in [
            vec![2, 1, 1, 2, 3, 4],
            vec![1, 2, 1, 2, 3, 4],
            vec![1, 1, 1, 2, 3],
            vec![1, 1, 1, 2, 3, 4, 0],
            vec![1, 1, 1, 2, 3, 0x84, 0],
            vec![1, 1, 1, 2, 3, 0x84],
            [vec![1, 1, 1, 2, 3], vec![0xff; 10], vec![1]].concat(),
        ] {
            assert!(decode(&damaged).is_err(), "{damaged:?}");
        }
    }
}
