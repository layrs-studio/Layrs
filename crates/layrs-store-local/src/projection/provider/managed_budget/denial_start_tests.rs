//! Pure prospective sizing; no destination FORMAT/proof or SQL is fabricated.
use super::*;
use crate::projection::provider::{
    bootstrap::{ManagedPlanData, derive_reconciled_denial_data},
    transition_reconcile::companions::denial_tests::{TX, with_fixture},
};
fn planned(data: &ManagedPlanData) -> ProviderTransitionIntentRecord {
    ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v2".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: 0,
        plan: serde_json::from_slice(&serde_json::to_vec(&data.plan).unwrap()).unwrap(),
        destination: None,
        reservation: None,
        receipt_id: None,
    }
}
#[test]
fn prospective_denial_floor_is_before_allocation_and_independent_of_numeric_quota() {
    for expanded in [false, true] {
        with_fixture(expanded, |prefix, observed, owner, coverage| {
            let data = derive_reconciled_denial_data(
                prefix,
                owner,
                observed,
                prefix.destination_identity.clone(),
                "C:\\sizing-parent",
                "fs:v1:1111111111111111:2222222222222222",
                "00000000000000000000000000000001",
                TX,
                coverage,
            )
            .unwrap();
            let mut intent = planned(&data);
            let cost = DeniedCoverageCost::new(&intent, coverage.record(), None).unwrap();
            let floor = prospective_denial_metadata_floor(
                0,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent,
                observed.selection(),
                owner.manifests(),
                &cost,
                None,
            )
            .unwrap();
            assert!(intent.destination.is_none());
            assert!(
                floor
                    >= 3 * ProviderTransitionIntentRecord::MAX_BYTES as u64
                        + ProviderTransitionReceiptRecord::MAX_BYTES as u64
                        + cost.document_bytes()
            );
            let copied = prospective_denial_metadata_floor(
                123,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent,
                observed.selection(),
                owner.manifests(),
                &cost,
                None,
            )
            .unwrap();
            assert_eq!(copied - floor, 123);
            intent.plan.quota_bytes = 0;
            let zero = prospective_denial_metadata_floor(
                0,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent,
                observed.selection(),
                owner.manifests(),
                &cost,
                None,
            )
            .unwrap();
            assert_eq!(floor, zero);
            assert!(zero > intent.plan.quota_bytes);
            intent.plan.quota_bytes = 999_999_999;
            assert_eq!(
                floor,
                prospective_denial_metadata_floor(
                    0,
                    &data.observation,
                    &data.placement,
                    &data.capture,
                    &intent,
                    observed.selection(),
                    owner.manifests(),
                    &cost,
                    None
                )
                .unwrap()
            );
            // The exact current document is added once to the inherited base.
            let size = IntentSize {
                payload: 0,
                scalars: 0,
            };
            let with_current = base_metadata_floor_with_coverage(
                0,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent.plan,
                observed.selection(),
                size,
                size,
                Some(&cost),
            )
            .unwrap();
            intent.plan.denial_coverage = None;
            let legacy_base = base_metadata_floor(
                0,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent.plan,
                observed.selection(),
                size,
                size,
            )
            .unwrap();
            assert_eq!(with_current - legacy_base, cost.document_bytes());
        });
    }
}
#[test]
fn prospective_denial_floor_rejects_unbound_rows_union_cost_overflow_and_cancellation() {
    with_fixture(true, |prefix, observed, owner, coverage| {
        let data = derive_reconciled_denial_data(
            prefix,
            owner,
            observed,
            prefix.destination_identity.clone(),
            "C:\\sizing-parent",
            "fs:v1:1111111111111111:2222222222222222",
            "00000000000000000000000000000001",
            TX,
            coverage,
        )
        .unwrap();
        let intent = planned(&data);
        let cost = DeniedCoverageCost::new(&intent, coverage.record(), None).unwrap();
        assert!(
            prospective_denial_metadata_floor(
                0,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent,
                observed.selection(),
                &[],
                &cost,
                None
            )
            .is_err()
        );
        let mut changed = planned(&data);
        changed.plan.denial_coverage.as_mut().unwrap().path_bytes += 1;
        assert!(
            prospective_denial_metadata_floor(
                0,
                &data.observation,
                &data.placement,
                &data.capture,
                &changed,
                observed.selection(),
                owner.manifests(),
                &cost,
                None
            )
            .is_err()
        );
        let mut wrong = data.observation.clone();
        wrong.observed_authorization_epoch += 1;
        assert!(
            prospective_denial_metadata_floor(
                0,
                &wrong,
                &data.placement,
                &data.capture,
                &intent,
                observed.selection(),
                owner.manifests(),
                &cost,
                None
            )
            .is_err()
        );
        assert!(
            prospective_denial_metadata_floor(
                MAX_METADATA,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent,
                observed.selection(),
                owner.manifests(),
                &cost,
                None
            )
            .is_err()
        );
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            prospective_denial_metadata_floor(
                0,
                &data.observation,
                &data.placement,
                &data.capture,
                &intent,
                observed.selection(),
                owner.manifests(),
                &cost,
                Some(&cancel)
            ),
            Err(super::super::recovery::error::ProviderBootstrapCause::Vfs(
                layrs_vfs::Error::Cancelled
            ))
        ));
    });
}

#[test]
fn denial_admission_peak_counts_final_files_shared_parents_and_excludes_reserved_paths() {
    use crate::projection::capture::{CapturedChange, CapturedFile, CapturedTarget};
    use layrs_core::{
        projection::EntryState,
        scm::{RepoPath, TreeEntryTarget},
    };
    with_fixture(false, |prefix, observed, owner, coverage| {
        let data = derive_reconciled_denial_data(
            prefix,
            owner,
            observed,
            prefix.destination_identity.clone(),
            "C:\\sizing-parent",
            "fs:v1:1111111111111111:2222222222222222",
            "00000000000000000000000000000001",
            TX,
            coverage,
        )
        .unwrap();
        assert_eq!(owner.authorized_files().len(), 1);
        let mut placement = data.placement.clone();
        let template = placement
            .entries
            .iter()
            .find(|e| matches!(e.target, Some(TreeEntryTarget::File { .. })))
            .unwrap()
            .clone();
        for (path, reserved) in [
            ("dir/old", false),
            ("dir/keep", false),
            ("hidden/file", true),
        ] {
            let mut entry = template.clone();
            entry.path = RepoPath::parse(path).unwrap();
            if reserved {
                entry.state = EntryState::Reserved;
                entry.writable = false;
            }
            placement.entries.push(entry);
        }
        placement.entries.sort_by(|a, b| a.path.cmp(&b.path));
        let file = owner.manifests().first().unwrap();
        // These are sizing inputs only, not a fabricated accepted patch/proof.
        let mut capture = data.capture.clone();
        capture.changes = vec![
            CapturedChange {
                path: RepoPath::parse("dir/old").unwrap(),
                expected: Some(CapturedTarget {
                    file_id: file.file_id().clone(),
                    executable: false,
                }),
                replacement: None,
            },
            CapturedChange {
                path: RepoPath::parse("new/deep/create").unwrap(),
                expected: None,
                replacement: Some(CapturedFile {
                    file_id: file.file_id().clone(),
                    bytes: file.byte_length(),
                    executable: false,
                }),
            },
        ];
        let budget = admission::denial_admission_budget(
            1234,
            &placement,
            observed.selection(),
            &capture,
            None,
        )
        .unwrap();
        assert_eq!(budget, admission_budget(1234, 3, 3).unwrap());
        assert_eq!(budget.archive_rows(), 8);
        assert!(
            budget
                .require_metadata(budget.minimum_metadata() - 1)
                .is_err()
        );
        budget.require_metadata(budget.minimum_metadata()).unwrap();
        let mut alias = template.clone();
        alias.path = RepoPath::parse("DIR/other").unwrap();
        placement.entries.push(alias);
        assert!(
            admission::denial_admission_budget(
                1234,
                &placement,
                observed.selection(),
                &capture,
                None
            )
            .unwrap_err()
            .to_string()
            .contains("parent spelling aliases")
        );
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            admission::denial_admission_budget(
                1234,
                &data.placement,
                observed.selection(),
                &data.capture,
                Some(&cancel)
            ),
            Err(super::super::recovery::error::ProviderBootstrapCause::Vfs(
                layrs_vfs::Error::Cancelled
            ))
        ));
    });
}
