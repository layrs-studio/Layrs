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
/// Format 8 receipt keys (phase 3e, council s30 item 5a): a tag byte, then
/// the 16 bytes of the UUID of `transaction:` plus a canonical lowercase
/// UUID, or else the id's text.
const RECEIPT_KEY_TEXT: u8 = 0;
const RECEIPT_KEY_UUID: u8 = 1;
const TRANSACTION_PREFIX: &str = "transaction:";
/// The schema's check of a receipt key (tests/fixtures/scm-v4.sql), which
/// fsck runs to find rows forged past it; `receipt_key_check_matches_the_schema`
/// keeps the two equal.
pub(crate) const RECEIPT_KEY_CHECK: &str = "(substr(transaction_key, 1, 1) = x'01' AND length(transaction_key) = 17) \
    OR (substr(transaction_key, 1, 1) = x'00' \
    AND length(transaction_key) BETWEEN 2 AND 256 \
    AND instr(substr(transaction_key, 2), x'00') = 0 \
    AND CAST(substr(transaction_key, 2) AS TEXT) NOT GLOB '*[^A-Za-z0-9._:-]*' \
    AND CAST(substr(transaction_key, 2) AS TEXT) NOT GLOB 'transaction:[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]')";

/// The 16 bytes of a canonical lowercase hyphenated UUID, if `text` is one.
fn canonical_uuid(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut out = [0_u8; 16];
    let mut nibbles = 0;
    for (at, byte) in bytes.iter().enumerate() {
        if matches!(at, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return None;
            }
            continue;
        }
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => return None,
        };
        out[nibbles / 2] |= value << (4 * (1 - nibbles % 2));
        nibbles += 1;
    }
    Some(out)
}

/// The key of a transaction's receipt row in format 8: one encoding per id
/// (a text key never has the shape a UUID key encodes).
pub(crate) fn receipt_key(transaction_id: &str) -> Vec<u8> {
    match transaction_id
        .strip_prefix(TRANSACTION_PREFIX)
        .and_then(canonical_uuid)
    {
        Some(uuid) => [&[RECEIPT_KEY_UUID][..], &uuid].concat(),
        None => [&[RECEIPT_KEY_TEXT][..], transaction_id.as_bytes()].concat(),
    }
}

/// The transaction id a receipt key stands for, if the key is that id's
/// one encoding (`receipt_key`).
pub(crate) fn receipt_transaction(key: &[u8]) -> Option<String> {
    match key.split_first()? {
        (&RECEIPT_KEY_UUID, uuid) if uuid.len() == 16 => {
            let mut text = String::with_capacity(TRANSACTION_PREFIX.len() + 36);
            text.push_str(TRANSACTION_PREFIX);
            for (at, byte) in uuid.iter().enumerate() {
                if matches!(at, 4 | 6 | 8 | 10) {
                    text.push('-');
                }
                use std::fmt::Write as _;
                let _ = write!(text, "{byte:02x}");
            }
            Some(text)
        }
        (&RECEIPT_KEY_TEXT, text) => {
            let text = std::str::from_utf8(text).ok()?;
            (TransactionId::new(text).is_ok()
                && receipt_key(text).first() == Some(&RECEIPT_KEY_TEXT))
            .then(|| text.to_owned())
        }
        _ => None,
    }
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
    /// binds the receipt to both, so it decodes under no other id or digest
    /// (council s29: a request digest does not cover the transaction id).
    /// Replay compares the digest first, so it still reports a damaged
    /// digest as a replay mismatch; a damaged key or receipt is corruption.
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
    use super::{
        ApplyBatchReceipt, RECEIPT_APPLIED, RECEIPT_SCHEMA, receipt_key, receipt_transaction,
    };

    /// fsck's copy of the schema's key check is the schema's (whitespace
    /// aside).
    #[test]
    fn receipt_key_check_matches_the_schema() {
        let squash = |text: &str| text.split_whitespace().collect::<String>();
        let schema = crate::scm_v1::schema::SCHEMA_V4_SQL;
        let at = schema
            .find("transaction_key BLOB PRIMARY KEY CHECK (")
            .unwrap();
        let body = &schema[at + "transaction_key BLOB PRIMARY KEY CHECK (".len()..];
        let end = body.find("\n    ),").unwrap();
        assert_eq!(squash(&body[..end]), squash(super::RECEIPT_KEY_CHECK));
    }

    /// Phase 3e: a receipt key is 17 bytes for `transaction:` plus a
    /// canonical lowercase UUID, the tagged text otherwise; each id has one
    /// key, and a key stands for one id.
    #[test]
    fn receipt_keys_are_reversible_and_unique() {
        let uuid = "transaction:d9ca3a5f-9d45-4681-9fdc-6049560969ff";
        let key = receipt_key(uuid);
        assert_eq!(key.len(), 17);
        assert_eq!(key[0], 1);
        assert_eq!(key[1..3], [0xd9, 0xca]);
        assert_eq!(receipt_transaction(&key).as_deref(), Some(uuid));
        for text in [
            "transaction:compact",
            "transaction:D9CA3A5F-9d45-4681-9fdc-6049560969ff",
            "transaction:d9ca3a5f9d4546819fdc6049560969ff",
            "transaction:d9ca3a5f-9d45-4681-9fdc-6049560969f",
            "transaction:d9ca3a5f-9d45-4681-9fdc-6049560969ffa",
            "transaction:d9ca3a5f_9d45-4681-9fdc-6049560969ff",
            "d9ca3a5f-9d45-4681-9fdc-6049560969ff",
            "transaction:paging-0",
        ] {
            let key = receipt_key(text);
            assert_eq!(key[0], 0, "{text}");
            assert_eq!(&key[1..], text.as_bytes());
            assert_eq!(receipt_transaction(&key).as_deref(), Some(text));
        }
        // The text form of a UUID id, an empty text, a short UUID key, an
        // unknown tag: no id.
        assert_eq!(
            receipt_transaction(&[&[0][..], uuid.as_bytes()].concat()),
            None
        );
        assert_eq!(receipt_transaction(&[0]), None);
        assert_eq!(
            receipt_transaction(b"\0transaction:{d9ca3a5f-9d45-4681-9fdc-6049560969ff}"),
            None
        );
        assert_eq!(receipt_transaction(&key[..16]), None);
        assert_eq!(receipt_transaction(&[&[2][..], &key[1..]].concat()), None);
        assert_eq!(receipt_transaction(&[]), None);
        assert_eq!(receipt_transaction(&[0, 0xff]), None);
        // Text that is not a transaction id: no id either.
        assert_eq!(receipt_transaction(b"\0transaction:a b"), None);
        assert_eq!(
            receipt_transaction(&[&[0][..], "t".repeat(256).as_bytes()].concat()),
            None
        );
    }

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
