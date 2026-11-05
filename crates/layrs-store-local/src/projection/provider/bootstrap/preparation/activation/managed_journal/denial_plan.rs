//! Reconstruct C before B funding. These closed DATA retain no SQL authority;
//! callers repeat this derivation inside the source snapshot before effects.
use super::super::{managed_image::complete::VerifiedCompleteManagedImage, managed_plan};
use crate::projection::provider::{
    import_validation::VerifiedImportClosure,
    managed_budget::{self, DeniedCoverageCost},
    records::{
        AllocationRecord, Record,
        denied_coverage::DerivedDeniedCoverage,
        transition::{
            ProviderTransitionIntentRecord, TransitionPlan, TransitionRole, TransitionState,
        },
    },
    recovery::error::{Result, invalid},
    source_companion::ProspectiveCompanionCost,
    transition_observation::TransitionObservationData,
    transition_reconcile::{companions::OwnedCompanionData, source_sql},
    validation::PrivateActiveCaptureValidation,
};
use layrs_core::scm::FileManifest;
use layrs_vfs::Cancellation;
use rusqlite::Connection;

pub(in crate::projection::provider) struct AdmissibleDenialPlan {
    data: managed_plan::ManagedPlanData,
    owner: OwnedCompanionData,
    cost: DeniedCoverageCost,
    companion_cost: Option<ProspectiveCompanionCost>,
}

impl AdmissibleDenialPlan {
    pub(in crate::projection::provider) fn plan(&self) -> &TransitionPlan {
        &self.data.plan
    }
    pub(in crate::projection::provider) fn manifests(&self) -> &[FileManifest] {
        self.owner.manifests()
    }
    pub(in crate::projection::provider) fn companion_cost(
        &self,
    ) -> Option<&ProspectiveCompanionCost> {
        self.companion_cost.as_ref()
    }
    pub(super) fn require_same(&self, other: &Self) -> Result<()> {
        if self.data.plan != other.data.plan
            || self.owner != other.owner
            || self.cost != other.cost
            || self.companion_cost != other.companion_cost
        {
            return Err(invalid("denial plan or complete source union changed"));
        }
        Ok(())
    }
}

/// Initial planning only. Recomputes the plan from immutable source, complete
/// image and observation; only destination parameters/transaction come from
/// the template. No reservation may use the template instead of this result.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn derive_candidate(
    source_c: &Connection,
    allocation: &AllocationRecord,
    closure: &VerifiedImportClosure,
    proof: &PrivateActiveCaptureValidation,
    image: &VerifiedCompleteManagedImage<'_, '_>,
    observed: &TransitionObservationData,
    planned: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    supplied: &[FileManifest],
    cancel: &Cancellation,
) -> Result<AdmissibleDenialPlan> {
    derive(
        source_c, allocation, closure, proof, image, observed, planned, coverage, supplied, false,
        cancel,
    )
}

/// Admission/replay: every plan field and the exact complete manifest union
/// must match the reconstruction. An insufficient quota fails before B exists.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn rederive(
    source_c: &Connection,
    allocation: &AllocationRecord,
    closure: &VerifiedImportClosure,
    proof: &PrivateActiveCaptureValidation,
    image: &VerifiedCompleteManagedImage<'_, '_>,
    observed: &TransitionObservationData,
    planned: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    supplied: &[FileManifest],
    cancel: &Cancellation,
) -> Result<AdmissibleDenialPlan> {
    derive(
        source_c, allocation, closure, proof, image, observed, planned, coverage, supplied, true,
        cancel,
    )
}

#[allow(clippy::too_many_arguments)]
fn derive(
    source_c: &Connection,
    allocation: &AllocationRecord,
    closure: &VerifiedImportClosure,
    proof: &PrivateActiveCaptureValidation,
    image: &VerifiedCompleteManagedImage<'_, '_>,
    observed: &TransitionObservationData,
    planned: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    supplied: &[FileManifest],
    compare: bool,
    cancel: &Cancellation,
) -> Result<AdmissibleDenialPlan> {
    cancel.check()?;
    planned.validate_fields()?;
    if planned.role != TransitionRole::Source
        || planned.state != TransitionState::Planned
        || planned.destination.is_some()
        || planned.reservation.is_some()
        || planned.receipt_id.is_some()
        || planned.plan.denial_coverage.is_none()
    {
        return Err(invalid("denial plan requires unallocated source Planned"));
    }
    coverage.record().require_intent(planned, Some(cancel))?;
    managed_plan::require_catalog_representable(supplied, cancel)?;
    let prefix = &planned.plan.source_prefix;
    let copied_bytes = closure.metadata_bytes(source_c, prefix, Some(cancel))?;
    source_sql::with_capture_source(
        source_c,
        &allocation.format,
        prefix,
        closure,
        proof,
        observed,
        Some(cancel),
        |source| {
            let derive_with =
                |metadata| -> Result<(managed_plan::ManagedPlanData, OwnedCompanionData)> {
                    let owner = image.reconcile_capture_retained(
                        source, observed, proof, supplied, metadata, cancel,
                    )?;
                    let data = managed_plan::derive_reconciled_denial_data(
                        prefix,
                        &owner,
                        observed,
                        prefix.destination_identity.clone(),
                        &planned.plan.destination_parent,
                        &planned.plan.destination_parent_identity,
                        &planned.plan.destination_leaf_nonce,
                        &planned.transaction_id,
                        coverage,
                    )?;
                    managed_plan::require_manifest_union(&data.plan, owner.manifests())?;
                    Ok((data, owner))
                };
            let (baseline, baseline_owner) = derive_with(0)?;
            let mut prospective = ProviderTransitionIntentRecord::decode(&planned.encode()?)?;
            prospective.plan = baseline.plan;
            let cost = DeniedCoverageCost::new(&prospective, coverage.record(), Some(cancel))?;
            let minimum = managed_budget::prospective_denial_metadata_floor(
                copied_bytes,
                &baseline.observation,
                &baseline.placement,
                &baseline.capture,
                &prospective,
                observed.selection(),
                baseline_owner.manifests(),
                &cost,
                Some(cancel),
            )?;
            let metadata = if compare {
                planned
                    .plan
                    .quota_bytes
                    .checked_sub(baseline.quota_bytes)
                    .filter(|bytes| *bytes >= minimum)
                    .ok_or_else(|| invalid("denial C metadata quota is insufficient"))?
            } else {
                minimum
            };
            let (mut data, owner) = derive_with(metadata)?;
            if compare && data.plan != planned.plan {
                return Err(invalid(
                    "denial plan differs from complete source and observation",
                ));
            }
            data.validated_metadata_floor = Some(minimum);
            // The unallocated Planned candidate is real planning DATA. Never
            // invent a destination proof or a terminal phase to size s.
            prospective.plan = serde_json::from_slice(
                &layrs_core::scm::canonical_json(&data.plan).map_err(|e| invalid(e.to_string()))?,
            )
            .map_err(|e| invalid(e.to_string()))?;
            let companion_cost =
                ProspectiveCompanionCost::derive(allocation, &prospective, &owner, Some(cancel))?;
            cancel.check()?;
            Ok(AdmissibleDenialPlan {
                data,
                owner,
                cost,
                companion_cost,
            })
        },
    )
}
