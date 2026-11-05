//! Pure denial-aware kernel; no source SQL or destination ownership authority.
use super::*;
use crate::projection::provider::{
    records::transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    transition_reconcile::companions::denial_tests::{TX, with_fixture},
};
const DIR: &str = "fs:v1:1111111111111111:2222222222222222";
fn data(
    prefix: &ProviderImportPrefixRecord,
    observed: &crate::projection::provider::transition_observation::TransitionObservationData,
    owner: &crate::projection::provider::transition_reconcile::companions::OwnedCompanionData,
    coverage: &DerivedDeniedCoverage,
) -> Result<ManagedPlanData> {
    derive_reconciled_denial_data(
        prefix,
        owner,
        observed,
        prefix.destination_identity.clone(),
        "C:\\denial-parent",
        DIR,
        "00000000000000000000000000000001",
        TX,
        coverage,
    )
}
#[test]
fn denial_kernel_commits_exact_selection_body_and_protected_union() {
    for expanded in [false, true] {
        with_fixture(expanded, |prefix, observed, owner, coverage| {
            let derived = data(prefix, observed, owner, coverage).unwrap();
            assert_eq!(
                derived.plan.denial_coverage,
                Some(coverage.record().reference().unwrap())
            );
            assert_eq!(
                derived.plan.observation.selection_digest,
                observed.binding().selection_digest
            );
            assert_ne!(
                derived.plan.observation.selection_digest,
                prefix.source.version().selection_digest
            );
            assert_eq!(derived.observation, *observed.snapshot());
            assert_eq!(derived.capture.changes, owner.overlay());
            assert_eq!(derived.quota_bytes, owner.quota_bytes());
            require_manifest_union(&derived.plan, owner.manifests()).unwrap();
            assert!(require_manifest_union(&derived.plan, &[]).is_err());
            let planned = ProviderTransitionIntentRecord {
                schema_version: "layrs.projection-provider-transition-intent.v2".into(),
                transaction_id: TX.into(),
                role: TransitionRole::Source,
                state: TransitionState::Planned,
                local_generation: 0,
                plan: derived.plan,
                destination: None,
                reservation: None,
                receipt_id: None,
            };
            coverage.record().require_intent(&planned, None).unwrap();
            planned.encode().unwrap();
            if !expanded {
                let legacy = derive_reconciled_data(
                    prefix,
                    owner,
                    observed,
                    prefix.destination_identity.clone(),
                    "C:\\denial-parent",
                    DIR,
                    "00000000000000000000000000000001",
                    TX,
                )
                .unwrap();
                assert!(legacy.plan.denial_coverage.is_none());
                assert_eq!(
                    legacy.plan.observation.selection_digest,
                    prefix.source.version().selection_digest
                );
                assert_eq!(legacy.capture, derived.capture);
            }
        });
    }
}
#[test]
fn denial_kernel_refuses_other_transaction_and_owner_observation_binding() {
    with_fixture(true, |prefix, observed, owner, coverage| {
        let other = DerivedDeniedCoverage::from_observation(
            prefix,
            "00000000000000000000000000000003",
            observed,
            None,
        )
        .unwrap();
        assert!(data(prefix, observed, owner, &other).is_err());
        let mut wrong = ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap();
        let ImportSource::Ordinary { version, .. } = &mut wrong.source else {
            unreachable!()
        };
        version.effective_image_digest = "b".repeat(64);
        assert!(data(&wrong, observed, owner, coverage).is_err());
    });
}
