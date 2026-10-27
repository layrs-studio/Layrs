//! Pure serialized-envelope and boundary checks; no SQL or physical authority.
use super::*;
use crate::projection::provider::validation::tests::fixture::Fixture;

#[test]
fn copying_capacity_reserves_destination_before_record_limit() {
    let source = Fixture::new("G1", false);
    let extra = ordinary_destination_max_bytes(&source.allocation.format).unwrap();
    assert!(extra > TransitionDestinationProof::MAX_FORMAT_BYTES);
    let exact = ProviderTransitionIntentRecord::MAX_BYTES - extra - LIFECYCLE_PAYLOAD_GROWTH;
    require_capacity(exact, extra, 0).unwrap();
    assert!(require_capacity(exact + 1, extra, 0).is_err());
    assert!(require_capacity(usize::MAX, extra, 0).is_err());
}

#[test]
fn copying_capacity_reserves_destination_before_inventory_limit() {
    let source = Fixture::new("G1", false);
    let extra = ordinary_destination_max_bytes(&source.allocation.format).unwrap();
    let used = MAX_TRANSITION_BYTES - extra as u64 - LIFECYCLE_NORMALIZED_GROWTH as u64;
    require_capacity(4096, extra, used).unwrap();
    assert!(require_capacity(4096, extra, used + 1).is_err());
    assert!(require_capacity(4096, extra, u64::MAX).is_err());
}

#[test]
fn copying_destination_envelope_includes_maximum_ascii_identities() {
    let source = Fixture::new("G1", false);
    let marker = &source.allocation.format;
    let max_file = format!("{}:{}:{}", u32::MAX, u32::MAX, u32::MAX);
    assert_eq!(max_file.len(), 32);
    let data = TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: marker.clone(),
        format_file_identity: max_file.clone(),
        private_database_identity: max_file,
        family_marker_digest: format!("blake3:{}", "f".repeat(64)),
        native_control_identity_hex: None,
        native_root_identity: None,
    };
    // Encoding here intentionally grants no valid destination proof. These
    // maximum-width fields check the pre-effect reserve independently of IDs
    // that the filesystem has not allocated yet.
    let actual = layrs_core::scm::canonical_json(&data).unwrap();
    let bound = ordinary_destination_max_bytes(marker).unwrap();
    assert_eq!(
        bound - actual.len(),
        TransitionDestinationProof::MAX_FORMAT_BYTES - marker.bytes().unwrap().len()
    );
    assert!(actual.len() <= bound);
}

#[test]
fn lifecycle_capacity_refuses_a_record_or_inventory_that_only_copying_can_fit() {
    let source = Fixture::new("G1", false);
    let extra = ordinary_destination_max_bytes(&source.allocation.format).unwrap();
    assert!(require_capacity(ProviderTransitionIntentRecord::MAX_BYTES - extra, extra, 0).is_err());
    assert!(require_capacity(4096, extra, MAX_TRANSITION_BYTES - extra as u64).is_err());
    assert_eq!(LIFECYCLE_NORMALIZED_GROWTH, 334);
    assert_eq!(LIFECYCLE_PAYLOAD_GROWTH, 158);
}

#[test]
fn lifecycle_growth_covers_canonical_phase_fields_and_normalized_columns() {
    // Only the changing fields are serialized. The actual record's plan, ID,
    // schema and punctuation are unchanged between phases, so they cancel from
    // these exact length differences. This sizing projection is NOT a Record,
    // source receipt, SQL witness or physical destination proof.
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Fields<'a> {
        role: TransitionRole,
        state: TransitionState,
        local_generation: u64,
        destination: Option<&'a TransitionDestinationProof>,
        reservation: Option<&'a str>,
        receipt_id: Option<&'a str>,
    }
    fn scalars(fields: &Fields<'_>) -> usize {
        // Enum strings are the codec's corresponding SQL labels. All values
        // are ASCII; remove the two JSON quote bytes from each scalar label.
        32 + layrs_core::scm::canonical_json(&fields.role).unwrap().len() - 2
            + layrs_core::scm::canonical_json(&fields.state)
                .unwrap()
                .len()
            - 2
            + 8
            + fields.reservation.map_or(0, str::len)
            + fields.receipt_id.map_or(0, str::len)
            + 71
    }
    let source = Fixture::new("G1", false);
    let max_file = format!("{}:{}:{}", u32::MAX, u32::MAX, u32::MAX);
    let destination = TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: source.allocation.format.clone(),
        format_file_identity: max_file.clone(),
        private_database_identity: max_file,
        family_marker_digest: format!("blake3:{}", "f".repeat(64)),
        native_control_identity_hex: None,
        native_root_identity: None,
    };
    let destination_bytes = layrs_core::scm::canonical_json(&destination).unwrap().len();
    let destination_bound = ordinary_destination_max_bytes(&destination.format).unwrap();
    let reservation = format!("provider-transition:{}", "f".repeat(64));
    let receipt = format!("blake3:{}", "f".repeat(64));
    assert_eq!((reservation.len(), receipt.len()), (84, 71));
    for generation in [0, 3, 10, 999_999, i64::MAX as u64] {
        let original = Fields {
            role: TransitionRole::Source,
            state: TransitionState::Planned,
            local_generation: generation,
            destination: None,
            reservation: None,
            receipt_id: None,
        };
        let original_payload = layrs_core::scm::canonical_json(&original).unwrap().len();
        let original_scalars = scalars(&original);
        for role in [TransitionRole::Source, TransitionRole::Destination] {
            for state in [
                TransitionState::Copying,
                TransitionState::Staging,
                TransitionState::Ready,
                TransitionState::Published,
                TransitionState::Cancelled,
            ] {
                let fields = Fields {
                    role,
                    state,
                    local_generation: if role == TransitionRole::Destination {
                        0
                    } else {
                        generation
                    },
                    destination: Some(&destination),
                    reservation: (state != TransitionState::Copying)
                        .then_some(reservation.as_str()),
                    receipt_id: matches!(
                        state,
                        TransitionState::Ready | TransitionState::Published
                    )
                    .then_some(receipt.as_str()),
                };
                let payload = layrs_core::scm::canonical_json(&fields).unwrap().len();
                let scalar = scalars(&fields);
                assert!(payload <= original_payload + destination_bound + LIFECYCLE_PAYLOAD_GROWTH);
                assert!(
                    payload + scalar
                        <= original_payload
                            + original_scalars
                            + destination_bound
                            + LIFECYCLE_NORMALIZED_GROWTH
                );
                if role == TransitionRole::Source && state == TransitionState::Published {
                    assert_eq!(payload - original_payload, destination_bytes - 4 + 153);
                    assert_eq!(scalar - original_scalars, 157);
                }
                if role == TransitionRole::Source && state == TransitionState::Staging {
                    assert_eq!(payload - original_payload, destination_bytes - 4 + 82);
                    assert_eq!(scalar - original_scalars, 84);
                }
            }
        }
    }
}
