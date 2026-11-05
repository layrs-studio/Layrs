//! DATA binding checks shared by source and autonomous destination readers.
//! The enclosing scope still authenticates the actual SQL owner and phase.
use super::*;
use crate::projection::provider::records::transition::ProviderTransitionIntentRecord;

#[cfg(test)]
#[path = "binding/tests.rs"]
mod tests;

impl TransitionDeniedCoverageRecord {
    /// The caller authenticates these actual SQL snapshot/selection rows and
    /// their chained digests. This binds the full retained negative namespace
    /// to them; it supplies neither a remote lease nor fresh access authority.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider) fn require_observation_rows(
        &self,
        snapshot: &ProjectionSnapshot,
        selected: &layrs_core::projection::ProjectionSelection,
        generation: u64,
        snapshot_digest: &str,
        selection_digest: &str,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        self.validate_fields()?;
        let policy = match &self.authority {
            DeniedAuthorityData::PreservedEpoch { policy_set_id, .. }
            | DeniedAuthorityData::ReobservedExpandedScope { policy_set_id, .. } => policy_set_id,
        };
        if self.path_semantics != snapshot.path_semantics
            || self.step_id != snapshot.base_step_id
            || self.root_tree_id != snapshot.base_root_tree_id
            || policy != &snapshot.observed_policy_set_id
            || self.authority.epoch() != snapshot.observed_authorization_epoch
            || self.observation_generation != generation
            || self.observation_snapshot_digest != snapshot_digest
            || self.observation_selection_digest != selection_digest
        {
            return Err(invalid("denied coverage actual observation rows differ"));
        }
        require_shared_namespace(
            snapshot,
            self.denied_paths.len(),
            self.denied_paths.iter(),
            cancel,
        )?;
        // Body validation and shared bounds precede the bounded path copy.
        let mut denied = Vec::with_capacity(self.denied_paths.len());
        for path in &self.denied_paths {
            cancellation::check(cancel)?;
            denied.push(
                crate::projection::provider::transition_observation::DeniedPathData {
                    path: path.clone(),
                },
            );
        }
        Ok(
            crate::projection::provider::transition_observation::require_stored_namespace(
                snapshot, selected, &denied, cancel,
            )?,
        )
    }

    /// Bind an already-read complete body to the actual transition, never just
    /// to an unowned compact reference. Planned may precede persistence of the
    /// destination observation snapshot; its commitments are checked here.
    pub(in crate::projection) fn require_intent(
        &self,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        intent.validate_fields()?;
        let plan = &intent.plan;
        let reference = plan
            .denial_coverage
            .as_ref()
            .ok_or_else(|| invalid("denied coverage body has no required transition link"))?;
        if self.reference()? != *reference {
            return Err(invalid("denied coverage body differs from transition link"));
        }
        let prefix = &plan.source_prefix;
        let (identity, version) = match &prefix.source {
            ImportSource::Provider {
                identity, version, ..
            }
            | ImportSource::Ordinary {
                identity, version, ..
            } => (identity, version),
        };
        let (policy, epoch) = match &self.authority {
            DeniedAuthorityData::PreservedEpoch {
                policy_set_id,
                authorization_epoch,
            }
            | DeniedAuthorityData::ReobservedExpandedScope {
                policy_set_id,
                authorization_epoch,
            } => (policy_set_id, *authorization_epoch),
        };
        if self.transaction_id != intent.transaction_id
            || self.source_prefix_id != plan.source_prefix_id
            || &self.source_identity != identity
            || self.destination_identity != prefix.destination_identity
            || &self.source_version != version
            || self.step_id != plan.observation.step_id
            || self.root_tree_id != plan.observation.root_tree_id
            || policy != &plan.observation.policy_set_id
            || epoch != plan.observation.authorization_epoch
            || self.observation_generation != plan.observation.generation
            || self.observation_snapshot_digest != plan.observation.snapshot_digest
            || self.observation_selection_digest != plan.observation.selection_digest
        {
            return Err(invalid("denied coverage actual transition binding differs"));
        }
        Ok(cancellation::check(cancel)?)
    }

    /// Reconstruct the entire document from the genuine bounded normalizer.
    /// This also binds path semantics, every negative path and shared namespace
    /// limits. A matching count/digest supplied independently is insufficient.
    pub(in crate::projection::provider) fn require_observation(
        &self,
        prefix: &ProviderImportPrefixRecord,
        transaction_id: &str,
        observation: &TransitionObservationData,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        let actual = Self::from_observation(prefix, transaction_id, observation, cancel)?;
        if self != &actual {
            return Err(invalid(
                "denied coverage differs from complete normalized observation",
            ));
        }
        Ok(cancellation::check(cancel)?)
    }
}
