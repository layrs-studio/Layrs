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

/// The receipt schema; a compact receipt's version byte 1 stands for it.
const RECEIPT_SCHEMA: &str = "layrs.apply-batch-receipt.v1";
/// The one outcome a receipt records; the compact outcome byte 1.
const RECEIPT_APPLIED: &str = "applied";
/// Compact receipt version and outcome bytes (format 8).
const COMPACT_RECEIPT_V1: u8 = 1;
const COMPACT_APPLIED: u8 = 1;
/// Bytes of the tag binding a compact receipt to its row (council s29 r2).
const COMPACT_TAG_LEN: usize = 8;

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
            schema_version: RECEIPT_SCHEMA.into(),
            outcome: RECEIPT_APPLIED.into(),
            transaction_id: batch.transaction_id().to_string(),
            request_digest: batch.request_digest().to_string(),
            object_count: batch.canonical_objects().len(),
            entity_count: batch.entity_records().len(),
            admission_count: batch.admissions().len(),
            mutation_count: batch.mutations().len(),
        }
    }

    /// Format 8's compact receipt (council s25, phase 2c): a version, the
    /// outcome, the four counts as LEB128 and an 8-byte tag. The transaction
    /// id and request digest are the row's key and digest columns; the tag
    /// binds the receipt to both, so damage to either column, or to the
    /// receipt, is corruption rather than another row's receipt (council
    /// s29: a request digest does not cover the transaction id).
    pub(crate) fn compact_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![COMPACT_RECEIPT_V1, COMPACT_APPLIED];
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
        let tag = self.compact_tag(&bytes);
        bytes.extend_from_slice(&tag);
        bytes
    }

    /// The tag of a compact receipt whose other bytes are `head`.
    fn compact_tag(&self, head: &[u8]) -> [u8; COMPACT_TAG_LEN] {
        let mut hasher = blake3::Hasher::new_derive_key("layrs compact receipt tag v1");
        for field in [
            self.transaction_id.as_bytes(),
            self.request_digest.as_bytes(),
        ] {
            hasher.update(&(field.len() as u64).to_le_bytes());
            hasher.update(field);
        }
        hasher.update(head);
        let mut tag = [0; COMPACT_TAG_LEN];
        tag.copy_from_slice(&hasher.finalize().as_bytes()[..COMPACT_TAG_LEN]);
        tag
    }

    /// The receipt a compact row stores (`compact_bytes`), or why not.
    pub(crate) fn from_compact(
        transaction_id: &str,
        request_digest: &str,
        bytes: &[u8],
    ) -> Result<Self, String> {
        let head = bytes
            .len()
            .checked_sub(COMPACT_TAG_LEN)
            .map(|at| &bytes[..at])
            .ok_or("compact receipt is truncated")?;
        let [COMPACT_RECEIPT_V1, COMPACT_APPLIED, rest @ ..] = head else {
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
            schema_version: RECEIPT_SCHEMA.into(),
            outcome: RECEIPT_APPLIED.into(),
            transaction_id: transaction_id.to_owned(),
            request_digest: request_digest.to_owned(),
            object_count: counts[0],
            entity_count: counts[1],
            admission_count: counts[2],
            mutation_count: counts[3],
        };
        // Its encoding is the only one (no padded varints), and its tag is
        // that of this row's transaction id and request digest.
        if receipt.compact_bytes() != bytes {
            return Err("compact receipt is not canonical or not this row's".into());
        }
        Ok(receipt)
    }

    pub(crate) fn validate(
        &self,
        transaction_id: &TransactionId,
        digest: RequestDigest,
        batch: &WriteBatch,
    ) -> Result<(), StoreError> {
        if self.schema_version != RECEIPT_SCHEMA
            || self.outcome != RECEIPT_APPLIED
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
    use super::{ApplyBatchReceipt, RECEIPT_APPLIED, RECEIPT_SCHEMA};

    fn receipt(counts: [usize; 4]) -> ApplyBatchReceipt {
        ApplyBatchReceipt {
            schema_version: RECEIPT_SCHEMA.into(),
            outcome: RECEIPT_APPLIED.into(),
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
            assert!((14..=50).contains(&bytes.len()), "{counts:?}");
            assert_eq!(decode(&bytes).unwrap(), expected, "{counts:?}");
        }
        let bytes = receipt([1, 2, 3, 4]).compact_bytes();
        assert_eq!(bytes[..6], [1, 1, 1, 2, 3, 4]);
        assert_eq!(bytes.len(), 14);
        let tag = &bytes[6..];
        let tagged = |head: &[u8]| [head, tag].concat();
        // Each damaged form keeps the tag of the exact receipt.
        for damaged in [
            tagged(&[2, 1, 1, 2, 3, 4]),
            tagged(&[1, 2, 1, 2, 3, 4]),
            tagged(&[1, 1, 1, 2, 3]),
            tagged(&[1, 1, 1, 2, 3, 4, 0]),
            tagged(&[1, 1, 1, 2, 3, 0x84, 0]),
            tagged(&[1, 1, 1, 2, 3, 0x84]),
            tagged(&[1, 1, 1, 2, 3, 5]),
            tagged(&[vec![1, 1, 1, 2, 3], vec![0xff; 10], vec![1]].concat()),
            [bytes.as_slice(), &[0]].concat(),
            bytes[..13].to_vec(),
            bytes[..7].to_vec(),
        ] {
            assert!(decode(&damaged).is_err(), "{damaged:?}");
        }
        // The tag binds the transaction id and the request digest.
        let digest = format!("blake3:{}", "ab".repeat(32));
        assert!(ApplyBatchReceipt::from_compact("transaction:compacu", &digest, &bytes).is_err());
        let other = format!("blake3:{}", "ac".repeat(32));
        assert!(ApplyBatchReceipt::from_compact("transaction:compact", &other, &bytes).is_err());
    }
}
