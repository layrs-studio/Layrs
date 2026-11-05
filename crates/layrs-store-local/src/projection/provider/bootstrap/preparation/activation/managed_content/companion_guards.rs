//! Explicit native-recipe checks, never called by the production driver.
use super::*;

impl ManagedImportLease<'_, '_, '_, '_, '_> {
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn assert_unprotected_source_refuses_receipt(
        &self,
        protected: &protection::ManagedImportProtection,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.with_source(cancel, |source_c, proof, closure| {
            let owned = self.ready_data(self.rederive(source_c, closure, proof, cancel)?, protected.root(), cancel)?;
            let data = owned.as_data(self.manifests)?;
            let inputs = crate::projection::provider::destination_ready::ManagedReadyInputs {
                marker: &self.destination.proof().format, source: &proof.active.source,
                allocation: &self.held.artifacts.allocation,
                prepared: self.held.artifacts.prepared.as_ref().ok_or_else(|| invalid("test prepared source absent"))?,
                activation: &proof.active.activation, prefix: &self.staging.plan.source_prefix,
                staging: owned.destination_staging(), closure,
                family_marker_bytes: self.held.family.marker_bytes(), data: &data,
                companion: self.ready_companion(proof),
                denial: None,
            };
            managed_destination::valid(database_io::read_cancellable_strict(self.destination.destination_database(), cancel, |destination_c| {
                managed_destination::valid(database_io::read_cancellable_strict(&self.held.family.catalog, cancel, |family_c| {
                    let before = destination_c.total_changes();
                    let error = crate::projection::provider::destination_ready::validate_ready_data(source_c, destination_c, family_c, &inputs, Some(cancel))
                        .err().ok_or_else(|| invalid("C Ready accepted missing actual B protected root"))?;
                    assert!(error.to_string().contains("actual B protected root"), "{error}");
                    assert_eq!(destination_c.total_changes(), before);
                    assert!(!destination_c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transition_receipts WHERE transaction_id=?1)", [self.staging.transaction_id.as_str()], |r| r.get::<_, bool>(0))?);
                    Ok(())
                })?)
            })?)
        })
    }
}
