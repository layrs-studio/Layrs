//! Pure serialized-envelope and boundary checks; no SQL or physical authority.
use super::*;
use crate::projection::provider::validation::tests::fixture::Fixture;

#[test]
fn copying_capacity_reserves_destination_before_record_limit() {
    let source = Fixture::new("G1", false);
    let extra = ordinary_destination_max_bytes(&source.allocation.format).unwrap();
    assert!(extra > TransitionDestinationProof::MAX_FORMAT_BYTES);
    let exact = ProviderTransitionIntentRecord::MAX_BYTES - extra;
    require_capacity(exact, extra, 0).unwrap();
    assert!(require_capacity(exact + 1, extra, 0).is_err());
    assert!(require_capacity(usize::MAX, extra, 0).is_err());
}

#[test]
fn copying_capacity_reserves_destination_before_inventory_limit() {
    let source = Fixture::new("G1", false);
    let extra = ordinary_destination_max_bytes(&source.allocation.format).unwrap();
    let used = MAX_TRANSITION_BYTES - extra as u64;
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
