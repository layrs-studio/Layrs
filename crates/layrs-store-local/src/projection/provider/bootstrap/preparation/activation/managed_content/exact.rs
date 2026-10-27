//! Closed adapter for exact installation. Only the engine receives a catalog
//! callback; native image bytes are copied after every scoped close succeeds.
use super::*;
use crate::projection::shared::exact_content::{self, ExactBinding, ExactCatalog, ExactStorage};
use layrs_core::scm::FileManifestId;

struct Catalog<'s, 'h, 'd, 'a, 'c, O: WriterCloseObserver> {
    lease: &'s ManagedImportLease<'h, 'd, 'a, 'c>,
    close: &'s mut O,
}

impl<'h, 'd, 'a, 'c> ManagedImportLease<'h, 'd, 'a, 'c> {
    /// Opening admits either untouched reservation state or typed installation
    /// progress. reserve_plan keeps its separate, strict reserve-only reader.
    pub(super) fn read_admission(
        &self,
        proof: &PrivateActiveCaptureValidation,
        destination_staging: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let inputs = self.inputs(proof, destination_staging)?;
        let binding = self.exact_binding()?;
        managed_destination::valid(database_io::read_cancellable(
            &self.held.family.catalog,
            cancel,
            |c| {
                let installing: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM exact_installations)",
                    [],
                    |r| r.get(0),
                )?;
                if installing {
                    self.held.family.validate_import_installation(
                        c,
                        &inputs,
                        &binding,
                        Some(cancel),
                    )?;
                } else {
                    self.held
                        .family
                        .validate_import_content(c, &inputs, Some(cancel))?;
                }
                Ok(())
            },
        )?)
    }

    fn exact_binding(&self) -> CauseResult<ExactBinding> {
        self.require_intent()?;
        let family = &self.held.artifacts.allocation.family;
        let destination = self.destination.proof();
        let member = destination
            .format
            .shared
            .as_ref()
            .ok_or_else(|| invalid("managed exact destination family binding absent"))?;
        let operation = self.staging.reservation_operation()?;
        let binding = ExactBinding {
            family_incarnation: family.incarnation.clone(),
            family_root_identity: family.root_identity.clone(),
            family_marker_digest: family.marker_digest.clone(),
            family_catalog_identity: family.catalog_file_identity.clone(),
            plan_digest: self.staging.plan.digest()?,
            prefix_id: self.staging.plan.source_prefix_id.clone(),
            destination_proof_digest: destination.digest(&self.staging.plan)?,
            transaction_id: self.staging.transaction_id.clone(),
            owner: destination.format.identity.worktree_id.clone(),
            database_identity: member.database_identity.clone(),
            reservation: operation.clone(),
            operation,
            quota_bytes: self.staging.plan.quota_bytes,
        };
        binding.validate()?;
        Ok(binding)
    }

    /// The returned manifest is DATA, not a destination protection or Ready
    /// capability. All physical owners remain borrowed for later finalization.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn install_exact(
        &mut self,
        id: &FileManifestId,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<FileManifest> {
        self.rehash(cancel)?;
        let mut attempt = exact_content::prepare(&mut Catalog { lease: self, close }, id, cancel)?;
        let manifest = attempt.manifest().clone();
        // A historical representation may already be fully resident. Missing
        // historical bytes require their separately authorized source; a cache
        // candidate with the same FileId cannot replace this ManifestId.
        let current_path = self
            .image
            .files()
            .iter()
            .find(|file| file.manifest.manifest() == &manifest)
            .map(|file| file.path.clone());
        if !attempt.published() {
            loop {
                let piece = exact_content::prepare_piece(
                    &mut Catalog { lease: self, close },
                    &mut attempt,
                    cancel,
                )?;
                let Some(mut piece) = piece else { break };
                if piece.needs_produce() {
                    let path = current_path.as_ref().ok_or_else(|| {
                        invalid("exact historical content unavailable from retained current image")
                    })?;
                    let ordinal = piece.ordinal();
                    piece.produce(
                        |sink| {
                            self.image
                                .copy_exact_chunk(path, &manifest, ordinal, sink, cancel)
                        },
                        cancel,
                    )?;
                }
                exact_content::finish_piece(
                    &mut Catalog { lease: self, close },
                    &mut attempt,
                    piece,
                    cancel,
                )?;
            }
        }
        // Publication performs the complete staged FileId verification itself.
        exact_content::publish(&mut Catalog { lease: self, close }, &mut attempt, cancel)?;
        self.boundary(cancel)?;
        if !attempt.published() {
            return Err(invalid(
                "exact installation acknowledgement lacks Published state",
            ));
        }
        Ok(manifest)
    }
}

impl<O: WriterCloseObserver> ExactCatalog for Catalog<'_, '_, '_, '_, '_, O> {
    fn expected_manifest(&self, id: &FileManifestId) -> CauseResult<FileManifest> {
        self.lease
            .manifests
            .iter()
            .find(|manifest| manifest.id() == id)
            .cloned()
            .ok_or_else(|| invalid("exact installation manifest is outside managed union"))
    }

    fn binding(&self) -> CauseResult<ExactBinding> {
        self.lease.exact_binding()
    }

    fn pin_storage(&self) -> CauseResult<ExactStorage> {
        let family = &self.lease.held.family;
        family.require_allocation(&self.lease.held.artifacts.allocation)?;
        let binding = self.binding()?;
        let storage = ExactStorage::open(
            family.root(),
            &binding.family_root_identity,
            family.writer_lifecycle(false)?,
        )?;
        family.verify()?;
        Ok(storage)
    }

    fn boundary(&self, cancel: &Cancellation) -> CauseResult<()> {
        self.lease.boundary(cancel)
    }

    fn with_catalog<T>(
        &mut self,
        write: bool,
        cancel: &Cancellation,
        action: impl FnOnce(&Connection) -> CauseResult<T>,
    ) -> CauseResult<T> {
        let lease = self.lease;
        let binding = lease.exact_binding()?;
        let close = &mut *self.close;
        let value = lease.with_authority(cancel, |proof, destination_staging| {
            let inputs = lease.inputs(proof, destination_staging)?;
            let file = &lease.held.family.catalog;
            let check = |c: &Connection| {
                lease
                    .held
                    .family
                    .validate_import_installation(c, &inputs, &binding, Some(cancel))
            };
            if !write {
                return managed_destination::valid(database_io::read_cancellable(
                    file,
                    cancel,
                    |c| {
                        check(c)?;
                        let value = action(c)?;
                        check(c)?;
                        lease.boundary(cancel)?;
                        Ok(value)
                    },
                )?);
            }
            database_io::validate_native_filename(file.path())?;
            let admission = database_io::clean_admission(file, false, |c| {
                check(c)?;
                Ok(())
            })?;
            lease.boundary(cancel)?;
            let bare = admission.open(file, close)?;
            let result = (|| {
                admission.require_geometry(&bare)?;
                let c = bare.connection();
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                check(c)?;
                let value = action(c)?;
                check(c)?;
                lease.boundary(cancel)?;
                c.execute_batch("COMMIT")?;
                Ok(value)
            })();
            bare.finish_writer(result, close)
        })?;
        // The source read close is now known positive. No acknowledgement can
        // escape an uncertain enclosing close or skip complete fresh readback.
        lease.with_authority(cancel, |proof, destination_staging| {
            let inputs = lease.inputs(proof, destination_staging)?;
            managed_destination::valid(database_io::read_cancellable(
                &lease.held.family.catalog,
                cancel,
                |c| {
                    lease.held.family.validate_import_installation(
                        c,
                        &inputs,
                        &binding,
                        Some(cancel),
                    )?;
                    Ok(())
                },
            )?)
        })?;
        lease.boundary(cancel)?;
        Ok(value)
    }
}
