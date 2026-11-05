use super::*;

#[test]
fn ordinary_admission_funds_the_exact_bounded_witness_without_growing_quota() {
    let provider = admission_budget(4096, 1, 0).unwrap();
    let ordinary = ordinary_admission_budget(4096, 1, 0, 100_000).unwrap();
    assert_eq!(ordinary.witness_rows(), 100_000);
    assert_eq!(ordinary.witness_bound(), 15_000_000);
    assert_eq!(
        ordinary.minimum_metadata(),
        provider.minimum_metadata() + 15_000_000
    );
    assert!(
        ordinary
            .require_metadata(provider.minimum_metadata())
            .is_err()
    );
    assert!(
        ordinary
            .require_quota(65536 + provider.minimum_metadata(), 65536)
            .is_err()
    );
    assert!(ordinary_admission_budget(0, 0, 0, 100_001).is_err());
    assert!(ordinary_admission_budget(0, 0, 0, u64::MAX).is_err());
    let zero = ordinary_admission_budget(4096, 1, 0, 0).unwrap();
    assert_eq!(zero, provider);
}

#[test]
fn ordinary_admission_cost_rejects_missing_extra_or_mischarged_witness() {
    let budget = ordinary_admission_budget(1234, 1, 1, 2).unwrap();
    let actual = ordinary_admission_cost(100, 200, 4, 1024, 2, 300).unwrap();
    assert_eq!(actual.witness_bytes(), 300);
    assert_eq!(
        actual.copy_peak_bytes().unwrap(),
        2048 + actual.admission_bytes() + 300
    );
    budget
        .require_actual(budget.minimum_metadata(), &actual)
        .unwrap();
    let missing = ordinary_admission_cost(100, 200, 4, 1024, 1, 150).unwrap();
    let extra = ordinary_admission_cost(100, 200, 4, 1024, 3, 450).unwrap();
    let provider = admission_cost(100, 200, 4, 1024).unwrap();
    for other in [missing, extra, provider] {
        assert!(
            budget
                .require_actual(budget.minimum_metadata(), &other)
                .is_err()
        );
    }
    assert!(
        admission_budget(1234, 1, 1)
            .unwrap()
            .require_actual(budget.minimum_metadata(), &actual)
            .is_err()
    );
    assert!(ordinary_admission_cost(100, 200, 4, 1024, 2, 299).is_err());
    assert!(ordinary_admission_cost(100, 200, 4, 1024, 2, 301).is_err());
    assert!(ordinary_admission_cost(100, 200, 4, 1024, 100_001, 15_000_150).is_err());
}

#[test]
fn ordinary_admission_metadata_cap_includes_witness_separately_from_family_margin() {
    let peak = ordinary_admission_budget(0, 0, 0, 100_000)
        .unwrap()
        .minimum_metadata();
    let exact = ordinary_admission_budget(MAX_METADATA - peak, 0, 0, 100_000).unwrap();
    assert_eq!(exact.minimum_metadata(), MAX_METADATA);
    assert!(ordinary_admission_budget(MAX_METADATA - peak + 1, 0, 0, 100_000).is_err());
    assert!(exact.require_remaining(MAX_METADATA, 0, 0).is_err());
    exact.require_remaining(MAX_METADATA + 65536, 0, 0).unwrap();
}

#[test]
fn admission_budget_retains_both_journals_and_full_ack_capacity() {
    let ready = 4096;
    let budget = admission_budget(ready, 1, 0).unwrap();
    assert_eq!(budget.archive_rows(), 3); // file, header, stage
    assert_eq!(budget.admission_bound(), 327996);
    assert_eq!(budget.archive_bound(), 3 * 8460);
    assert_eq!(
        budget.minimum_metadata(),
        ready + 2 * budget.archive_bound() + budget.admission_bound()
    );
    let missing_copy = ready + budget.archive_bound() + budget.admission_bound();
    assert!(budget.require_metadata(missing_copy).is_err());
    budget.require_metadata(budget.minimum_metadata()).unwrap();
}

#[test]
fn admission_archive_bound_saturates_at_64mib_without_relaxing_row_limits() {
    let unsaturated_files = MAX_ARCHIVE_BYTES / MAX_ARCHIVE_ROW_BYTES - 2;
    let low = admission_budget(0, unsaturated_files, 0).unwrap();
    let high = admission_budget(0, unsaturated_files + 1, 0).unwrap();
    assert!(low.archive_bound() < MAX_ARCHIVE_BYTES);
    assert_eq!(high.archive_bound(), MAX_ARCHIVE_BYTES);
    let maximum = admission_budget(0, MAX_FILES, MAX_DIRECTORIES).unwrap();
    assert_eq!(maximum.archive_rows(), 200002);
    assert_eq!(maximum.archive_bound(), MAX_ARCHIVE_BYTES);
    assert!(admission_budget(0, MAX_FILES + 1, 0).is_err());
    assert!(admission_budget(0, 0, MAX_DIRECTORIES + 1).is_err());
    assert!(admission_budget(0, u64::MAX, u64::MAX).is_err());
}

#[test]
fn admission_metadata_cap_includes_the_entire_copy_peak() {
    let extra = admission_budget(0, 0, 0).unwrap().minimum_metadata();
    let exact = admission_budget(MAX_METADATA - extra, 0, 0).unwrap();
    assert_eq!(exact.minimum_metadata(), MAX_METADATA);
    exact.require_metadata(MAX_METADATA).unwrap();
    assert!(admission_budget(MAX_METADATA - extra + 1, 0, 0).is_err());
    assert!(admission_budget(u64::MAX, 0, 0).is_err());
    assert!(super::super::add_metadata(u64::MAX, 1).is_err());
    assert!(exact.require_metadata(MAX_METADATA + 1).is_err());
}

#[test]
fn admission_quota_is_exact_q0_plus_fixed_m_and_never_grows_old_reservations() {
    let budget = admission_budget(1000, 2, 1).unwrap();
    let q0 = super::super::quota(&[], [17, 23], 0).unwrap();
    let metadata = budget.minimum_metadata() + 128;
    let durable = super::super::quota(&[], [17, 23], metadata).unwrap();
    assert_eq!(durable, q0 + metadata);
    assert_eq!(budget.require_quota(durable, q0).unwrap(), metadata);
    // The historical Ready-only budget is not silently upgraded in memory.
    assert!(budget.require_quota(q0 + 1000, q0).is_err());
    assert!(budget.require_quota(q0 - 1, q0).is_err());
    assert!(budget.require_quota(q0 + MAX_METADATA + 1, q0).is_err());
    assert!(budget.require_quota(u64::MAX, q0).is_err());
    assert!(budget.require_quota(q0, u64::MAX).is_err());
}

#[test]
fn admission_actual_cost_preserves_peak_and_refuses_a_smaller_envelope() {
    let budget = admission_budget(1234, 1, 1).unwrap();
    let actual = admission_cost(100, 200, 4, 1024).unwrap();
    assert_eq!(actual.admission_bytes(), 4 * 71 + 4 * 8 + 100 + 200);
    assert_eq!(actual.archive_bytes(), 1024);
    assert_eq!(
        actual.copy_peak_bytes().unwrap(),
        2 * actual.archive_bytes() + actual.admission_bytes()
    );
    budget
        .require_actual(budget.minimum_metadata(), &actual)
        .unwrap();
    assert!(
        budget
            .require_actual(1234 + actual.copy_peak_bytes().unwrap(), &actual)
            .is_err()
    );
    let different_inventory = admission_cost(100, 200, 3, 1024).unwrap();
    assert!(
        budget
            .require_actual(budget.minimum_metadata(), &different_inventory)
            .is_err()
    );
}

#[test]
fn admission_actual_cost_refuses_oversized_columns_and_impossible_aggregates() {
    let maximum = admission_cost(
        MAX_SOURCE_PUBLISHED_BYTES,
        MAX_ADMISSION_RECORD_BYTES,
        MAX_ROWS,
        MAX_ARCHIVE_BYTES,
    )
    .unwrap();
    assert_eq!(maximum.admission_bytes(), 327996);
    assert_eq!(
        maximum.copy_peak_bytes().unwrap(),
        2 * MAX_ARCHIVE_BYTES + 327996
    );
    for (source, record, rows, bytes) in [
        (0, 1, 2, 158),
        (1, 0, 2, 158),
        (MAX_SOURCE_PUBLISHED_BYTES + 1, 1, 2, 158),
        (1, MAX_ADMISSION_RECORD_BYTES + 1, 2, 158),
        (1, 1, 1, 158),
        (1, 1, MAX_ROWS + 1, MAX_ARCHIVE_BYTES),
        (1, 1, 2, 157),
        (1, 1, 2, 2 * MAX_ARCHIVE_ROW_BYTES + 1),
        (1, 1, MAX_ROWS, MAX_ARCHIVE_BYTES + 1),
        (u64::MAX, u64::MAX, u64::MAX, u64::MAX),
    ] {
        assert!(admission_cost(source, record, rows, bytes).is_err());
    }
}

#[test]
fn admission_remaining_retains_the_existing_root_relations_and_frame_margin() {
    let budget = admission_budget(4096, 1, 0).unwrap();
    let root_bytes = 12345;
    let manifests = 7;
    let needed = budget.minimum_metadata() + root_bytes + 256 * manifests + 65536;
    assert_eq!(
        budget
            .require_remaining(needed, root_bytes, manifests)
            .unwrap(),
        needed
    );
    assert!(
        budget
            .require_remaining(needed - 1, root_bytes, manifests)
            .is_err()
    );
    assert!(
        budget
            .require_remaining(needed, 16 * 1024 * 1024 + 1, 0)
            .is_err()
    );
    assert!(budget.require_remaining(needed, 0, 100001).is_err());
    assert!(budget.require_remaining(u64::MAX, 0, 0).is_err());
}
