//! Counting-frame DATA only. No allocation/proof/terminal protocol value,
//! SQL, native fixture or source-publication authority is manufactured here.
use super::*;

#[cfg(test)]
impl ProspectiveCompanionCost {
    /// Mutate only scalar cost DATA cloned from the native recipe's authentic
    /// prospective derivation. The public C14 opener must refuse these before
    /// opening any writer. No protocol record or authority witness is created.
    /// Order: plan digest, source row bytes, receipt bytes, reservation bytes.
    pub(in crate::projection::provider) fn negative_test_cases(&self) -> Vec<(Self, &'static str)> {
        let mut plan = self.clone();
        let zero = format!("blake3:{}", "0".repeat(64));
        plan.plan_digest = if self.plan_digest == zero {
            format!("blake3:{}", "1".repeat(64))
        } else {
            zero
        };
        let mut rows = self.clone();
        rows.source_rows ^= 1;
        let mut receipt = self.clone();
        receipt.receipt_bytes ^= 1;
        let mut reservation = self.clone();
        reservation.reservation_bytes = 0;
        vec![
            (
                plan,
                "actual companion differs from prospective source plan",
            ),
            (
                rows,
                "actual companion differs from prospective source plan",
            ),
            (
                receipt,
                "actual companion exceeds prospective encoding envelope",
            ),
            (
                reservation,
                "actual companion exceeds prospective encoding envelope",
            ),
        ]
    }
}

fn invalid_message(
    error: crate::projection::provider::recovery::error::ProviderBootstrapCause,
    expected: &str,
) {
    assert!(
        matches!(&error,
        crate::projection::provider::recovery::error::ProviderBootstrapCause::Validation(value)
        if matches!(value.as_ref(), crate::projection::ProjectionStoreError::Invalid(message)
            if message == expected)),
        "{error}"
    );
}

#[test]
fn prospective_format_slot_uses_the_whole_codec_budget_without_a_marker() {
    assert_eq!(MAX_FORMAT, crate::projection::provider::records::transition::TransitionDestinationProof::MAX_FORMAT_BYTES as u64);
    assert_eq!(
        MAX_RECEIPT,
        ProviderTransitionReceiptRecord::MAX_BYTES as u64
    );
    assert_eq!(MAX_INTENT, ProviderTransitionIntentRecord::MAX_BYTES as u64);
    assert_eq!(
        SOURCE_OPERATION_BYTES as usize,
        crate::projection::shared::source_companion::NAMESPACE.len() + 64
    );
    assert_eq!(
        measure(&FormatBudget, MAX_FORMAT, "FORMAT bound").unwrap(),
        MAX_FORMAT
    );
    invalid_message(
        measure(&FormatBudget, MAX_FORMAT - 1, "FORMAT bound").unwrap_err(),
        "FORMAT bound",
    );
    let value = serde_json::to_value(FormatBudget).unwrap();
    let slot = value.as_str().unwrap();
    assert_eq!(slot.len() + 2, MAX_FORMAT as usize);
    assert!(slot.bytes().all(|b| b == b'?'));
}

#[test]
fn prospective_unknown_widths_cover_canonical_file_and_digest_encodings() {
    let max_file_identity = format!("{}:{}:{}", u32::MAX, u32::MAX, u32::MAX);
    assert_eq!(max_file_identity.len(), FILE_ID_BYTES);
    let max_digest = format!("blake3:{}", "a".repeat(64));
    assert_eq!(max_digest.len(), DIGEST_BYTES);
    for (actual, width) in [
        (&max_file_identity, FILE_ID_BYTES),
        (&max_digest, DIGEST_BYTES),
    ] {
        let expected = serde_json::to_vec(actual).unwrap().len() as u64;
        let counted = match width {
            FILE_ID_BYTES => {
                measure(&UnknownAscii::<FILE_ID_BYTES>, u64::MAX, "file width").unwrap()
            }
            DIGEST_BYTES => {
                measure(&UnknownAscii::<DIGEST_BYTES>, u64::MAX, "digest width").unwrap()
            }
            _ => unreachable!(),
        };
        assert_eq!(counted, expected);
    }
    let slot = serde_json::to_value(UnknownAscii::<DIGEST_BYTES>).unwrap();
    assert!(!slot.as_str().unwrap().starts_with("blake3:"));
    assert_eq!(slot.as_str().unwrap().len(), DIGEST_BYTES);
}

#[test]
fn prospective_count_accounts_for_known_json_escaping_and_exact_ceiling() {
    // Family incarnation is known before allocation and may contain JSON
    // escaping. Replacing its actual encoding with raw string length would
    // undercharge this frame. UTF-8 bytes and escapes are measured together.
    let owner = WorktreeId::parse("worktree:prospective-data").unwrap();
    let frame = SourceContextFrame {
        source_owner: &owner,
        source_database: "1:2:3",
        family_incarnation: "quote\" slash\\ newline\ncontrol\u{0001} é",
        family_root_identity: "fs:v1:1111111111111111:2222222222222222",
        family_catalog_identity: "4:5:6",
        family_marker_digest:
            "blake3:1111111111111111111111111111111111111111111111111111111111111111",
    };
    let canonical = layrs_core::scm::canonical_json(&frame).unwrap();
    let length = canonical.len() as u64;
    assert_eq!(
        measure(&frame, length, "known frame exceeds bound").unwrap(),
        length
    );
    invalid_message(
        measure(&frame, length - 1, "known frame exceeds bound").unwrap_err(),
        "known frame exceeds bound",
    );
    let encoded = String::from_utf8(canonical).unwrap();
    assert!(encoded.contains("\\\""));
    assert!(encoded.contains("\\\\"));
    assert!(encoded.contains("\\n"));
    assert!(encoded.contains("\\u0001"));
}

#[test]
fn prospective_receipt_ceiling_is_checked_before_the_larger_intent_budget() {
    // Synthetic counting DATA deliberately crosses the separate receipt
    // ceiling while fitting an intent. This is not a Receipt or Planned and
    // does not establish admission of such a protocol inventory.
    #[derive(Serialize)]
    struct CountedInput<'a> {
        source_prefix: &'a str,
    }
    let known = "quoted\"\\".repeat(MAX_RECEIPT as usize / 8);
    let frame = CountedInput {
        source_prefix: &known,
    };
    assert!(measure(&frame, MAX_INTENT, "intent").is_ok());
    invalid_message(
        measure(&frame, MAX_RECEIPT, "receipt").unwrap_err(),
        "receipt",
    );
}

#[test]
fn prospective_counter_and_reservation_math_refuse_overflow() {
    let mut count = Count {
        bytes: u64::MAX,
        limit: u64::MAX,
        error: "overflow",
    };
    assert!(count.write(b"x").is_err());
    assert!(add_cost(i64::MAX as u64, 1).is_err());
    assert!(reservation_bound(0, 0, u64::MAX, 1, 1).is_err());
}
