//! Fresh unchanged Ordinary B Staging + private C8 Ready -> family Ready only.
//! Fresh B bytes witness the durable image commitment. No allocation, Access,
//! old owner/pin, terminal holder, source Ready or admission is reconstructed.
use super::{content, ready_data::OwnedOrdinaryReadyData, reservation, reserve_plan, *};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary, checkout_journal,
            database_io::{self, WriterCloseObserver},
            destination_ready::{self, OrdinaryReadyInputs, OwnedReadySnapshot},
            import_validation::{verify_source, ImportDatabaseKind},
            managed_ready_reopen::{
                AdmissionStep, OrdinaryExclusiveSource, OrdinaryFamilyPhysical,
                OrdinaryFamilyResources,
            },
            ordinary_transition,
            records::{
                import::{
                    AdmittedSourceBinding, ImportSourceReferences, ProviderTransitionReceiptRecord,
                },
                Record,
            },
            recovery::error::{
                invalid as rejected, ProviderBootstrapCause as Cause, Result as CauseResult,
            },
        },
        shared::admitted_catalog::{
            AdmittedFamilyContext, ExportPublicationEffect, ExportPublicationMember,
            ExportPublicationState, ExportReserveOnlyTarget,
        },
    },
};
use layrs_core::projection::{ProjectionSelection, ProjectionSnapshot};

pub(in crate::projection::provider) struct OrdinaryFamilyReadyPublicationProgress {
    effect: ExportPublicationEffect,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    family_state: ExportPublicationState,
}
impl OrdinaryFamilyReadyPublicationProgress {
    pub(in crate::projection::provider) fn effect(&self) -> ExportPublicationEffect {
        self.effect
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(in crate::projection::provider) fn family_state(&self) -> ExportPublicationState {
        self.family_state
    }
}

pub(in crate::projection::provider) struct OrdinaryFamilyReadyPublicationFailure {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: OrdinaryFamilyReadyPublication,
}

struct SourceObservation {
    staging: ProviderTransitionIntentRecord,
    snapshot: ProjectionSnapshot,
    selection: ProjectionSelection,
}

/// Pointer-sized nested reader results avoid copying complete Ready records
/// and inventories into each debug reader's return storage.
struct Validated {
    snapshot: Box<OwnedReadySnapshot>,
    state: ExportPublicationState,
    effect: Option<ExportPublicationEffect>,
}

/// Owns B Exclusive before C Shared and retains every fresh image/CAS/checkout
/// proof through all mandatory closes. Explicit retry consumes a failed held
/// value and reobserves actual member phase; it never infers COMMIT or an ACK.
/// Native close quarantine, hot journals and changed B remain refusals.
pub(in crate::projection::provider) struct OrdinaryFamilyReadyPublication {
    image: OrdinaryExportImage,
    physical: OrdinaryFamilyPhysical,
    destination: OrdinaryFamilyResources,
    source: OrdinaryExclusiveSource,
    observation: Box<SourceObservation>,
    snapshot: Box<OwnedReadySnapshot>,
}

impl OrdinaryFamilyReadyPublication {
    pub(in crate::projection::provider) fn open(
        source_root: &Path,
        destination_root: &Path,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        let source = OrdinaryExclusiveSource::open(source_root, cancel)?;
        let mut observation = None;
        // This clean, real B RO connection positively closes before any
        // selected namespace scan/hash or C physical byte verification.
        database_io::clean_admission(source.database(), false, |c| {
            observation = Some(read_observation(&source, c, transaction_id, cancel)?);
            Ok(())
        })?;
        let observation =
            observation.ok_or_else(|| rejected("fresh family source observation absent"))?;
        let destination = source.inspect_family_destination(destination_root, cancel)?;
        let image = OrdinaryExportImage::build_parts(
            source.root(),
            source.marker(),
            &observation.snapshot,
            &observation.selection,
            cancel,
        )
        .map_err(Cause::from)?;
        require_image(&observation, &image)?;
        // Provisional C DATA is used only inside reconstruction. The complete
        // B/private/whole-family gate must pass and all RO scopes close before
        // any C CAS/checkout proof is acquired.
        let before = Reconstruction {
            source: &source,
            destination: &destination,
            observation: &observation,
            image: &image,
            expected: None,
            physical: None,
        }
        .read_validated(cancel)?;
        let physical = destination.pin_ready(&before.snapshot, cancel)?;
        let mut held = Self {
            image,
            physical,
            destination,
            source,
            observation,
            snapshot: before.snapshot,
        };
        held.require_physical(cancel)?;
        let current = held.reconstruction().read_validated(cancel)?;
        if current.state != before.state {
            return Err(rejected(
                "fresh family member/accounting changed during pinning",
            ));
        }
        held.require_physical(cancel)?;
        Ok(held)
    }

    pub(in crate::projection::provider) fn publish_family_ready(
        mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<
        OrdinaryFamilyReadyPublicationProgress,
        OrdinaryFamilyReadyPublicationFailure,
    > {
        let mut step = AdmissionStep::Validating;
        let result = self.publish_inner(close, cancel, &mut step);
        result.map_err(|cause| OrdinaryFamilyReadyPublicationFailure {
            step,
            cause,
            held: self,
        })
    }

    fn reconstruction(&self) -> Reconstruction<'_> {
        Reconstruction {
            source: &self.source,
            destination: &self.destination,
            observation: &self.observation,
            image: &self.image,
            expected: Some(&self.snapshot),
            physical: Some(&self.physical),
        }
    }

    fn require_physical(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_identity(cancel)?;
        self.image
            .revalidate_parts(
                self.source.root(),
                self.source.marker(),
                &self.observation.snapshot,
                &self.observation.selection,
                cancel,
            )
            .map_err(Cause::from)?;
        require_image(&self.observation, &self.image)?;
        self.destination.require_data(&self.snapshot, cancel)?;
        self.physical.require_physical(&self.snapshot, cancel)
    }

    fn publish_inner(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> CauseResult<OrdinaryFamilyReadyPublicationProgress> {
        self.require_physical(cancel)?;
        self.reconstruction().read_validated(cancel)?;
        self.require_physical(cancel)?;
        let reconstruction = self.reconstruction();
        let family = self.destination.family();
        let after = {
            let _catalog = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"),
                SessionMode::Exclusive,
            )?;
            // Actual B Staging and C Ready remain in strict RO transactions
            // throughout the family preflight, writer, COMMIT and close.
            reservation::valid(database_io::read_cancellable_strict(
                self.source.database(),
                cancel,
                |source| {
                    reservation::valid(database_io::read_cancellable_strict(
                        self.destination.database(),
                        cancel,
                        |destination| {
                            let before = reservation::valid(database_io::read_cancellable_strict(
                                &family.catalog,
                                cancel,
                                |catalog| {
                                    reconstruction.validate(
                                        source,
                                        destination,
                                        catalog,
                                        None,
                                        cancel,
                                    )
                                },
                            )?)?;
                            if before.state.member == ExportPublicationMember::Ready {
                                return Ok(Box::new(Validated {
                                    snapshot: before.snapshot,
                                    state: before.state,
                                    effect: Some(ExportPublicationEffect::AlreadyReady),
                                }));
                            }
                            // No independent family RO connection survives to
                            // family COMMIT. CleanWriteAdmission closes its own
                            // preflight positively before writer acquisition.
                            let admission =
                                database_io::clean_admission(&family.catalog, false, |catalog| {
                                    let current = reconstruction.validate(
                                        source,
                                        destination,
                                        catalog,
                                        None,
                                        cancel,
                                    )?;
                                    if current.state != before.state {
                                        return Err(rejected(
                                            "fresh family predecessor changed before writer",
                                        ));
                                    }
                                    Ok(())
                                })?;
                            reconstruction.require_identity(cancel)?;
                            *step = AdmissionStep::WriterOpenAttempted;
                            let bare = admission.open(&family.catalog, close)?;
                            let written = (|| {
                                admission.require_geometry(&bare)?;
                                let catalog = bare.connection();
                                catalog.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                                let after = reconstruction.validate(
                                    source,
                                    destination,
                                    catalog,
                                    Some((before.state, &mut *step)),
                                    cancel,
                                )?;
                                reconstruction.require_identity(cancel)?;
                                cancel.check()?;
                                *step = AdmissionStep::CommitAttempted;
                                catalog.execute_batch("COMMIT")?;
                                *step = AdmissionStep::CommitReturned;
                                reconstruction.require_identity(cancel)?;
                                Ok(after)
                            })();
                            let written = rollback_failed_action(bare.connection(), written);
                            let after = bare.finish_writer(written, close)?;
                            *step = AdmissionStep::WriterClosed;
                            Ok(after)
                        },
                    )?)
                },
            )?)?
        };
        // Every error above propagates through mandatory closes, then returns
        // immediately. There is no unconditional SQL/FS boundary on failure.
        *step = AdmissionStep::ReadScopesClosed;
        self.require_physical(cancel)?;
        let current = self.reconstruction().read_validated(cancel)?;
        if current.state.member != ExportPublicationMember::Ready || current.state != after.state {
            return Err(rejected("fresh family durable Ready/accounting changed"));
        }
        after.snapshot.require_same(&current.snapshot)?;
        self.require_physical(cancel)?;
        let effect = after
            .effect
            .ok_or_else(|| rejected("fresh family publication effect absent"))?;
        let snapshot = current.snapshot;
        *step = AdmissionStep::FreshVerified;
        Ok(OrdinaryFamilyReadyPublicationProgress {
            effect,
            ready: snapshot.ready,
            receipt: snapshot.receipt,
            family_state: current.state,
        })
    }
}

/// All members are genuine retained capabilities or immutable calculation
/// DATA. Full namespace/hash work is outside every method's SQL/catalog scope.
struct Reconstruction<'a> {
    source: &'a OrdinaryExclusiveSource,
    destination: &'a OrdinaryFamilyResources,
    observation: &'a SourceObservation,
    image: &'a OrdinaryExportImage,
    expected: Option<&'a OwnedReadySnapshot>,
    physical: Option<&'a OrdinaryFamilyPhysical>,
}

impl Reconstruction<'_> {
    fn require_identity(&self, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_identity(cancel)?;
        self.image.verify_identity(cancel).map_err(Cause::from)?;
        self.destination.require_identity(cancel)?;
        if let Some(expected) = self.expected {
            self.destination.require_data(expected, cancel)?;
            if let Some(physical) = self.physical {
                physical.require_identity(expected, cancel)?;
            }
        }
        Ok(())
    }

    fn read_validated(&self, cancel: &Cancellation) -> CauseResult<Box<Validated>> {
        self.require_identity(cancel)?;
        let family = self.destination.family();
        let _catalog = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        reservation::valid(database_io::read_cancellable_strict(
            self.source.database(),
            cancel,
            |source| {
                reservation::valid(database_io::read_cancellable_strict(
                    self.destination.database(),
                    cancel,
                    |destination| {
                        reservation::valid(database_io::read_cancellable_strict(
                            &family.catalog,
                            cancel,
                            |catalog| self.validate(source, destination, catalog, None, cancel),
                        )?)
                    },
                )?)
            },
        )?)
    }

    fn require_observation(&self, source: &Connection, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_snapshot(source, cancel)?;
        let actual = ordinary_transition::read_publication_state(
            source,
            self.source.marker(),
            &self.observation.staging.transaction_id,
            Some(cancel),
        )?;
        if actual.intent != self.observation.staging || actual.receipt.is_some() {
            return Err(rejected(
                "fresh family requires actual unchanged B Staging without receipt",
            ));
        }
        let current = database::current(source).map_err(Cause::from)?;
        let selected = selection::for_scan(source, &current.snapshot).map_err(Cause::from)?;
        let version = self.observation.staging.plan.source_prefix.source.version();
        if current.generation != version.generation
            || current.digest != version.snapshot_digest
            || current.snapshot != self.observation.snapshot
            || selected != self.observation.selection
        {
            return Err(rejected(
                "fresh family source frontier/snapshot/selection changed",
            ));
        }
        require_image(self.observation, self.image)
    }

    /// A mutation request must name the exact freshly observed predecessor.
    /// Every private/source/full-family gate runs before the sole CAS and again
    /// afterward in the same actual B/C/family transactions.
    fn validate(
        &self,
        source: &Connection,
        destination: &Connection,
        family: &Connection,
        mutation: Option<(ExportPublicationState, &mut AdmissionStep)>,
        cancel: &Cancellation,
    ) -> CauseResult<Box<Validated>> {
        self.require_observation(source, cancel)?;
        let staging = &self.observation.staging;
        let prefix = &staging.plan.source_prefix;
        let admitted = admitted_ordinary::inspect(source, self.source.marker(), Some(cancel))?;
        let source_binding = AdmittedSourceBinding {
            admission_digest: admitted.admission_digest().to_owned(),
            incoming_prefix_id: admitted.prefix_id().to_owned(),
            incoming_receipt_id: admitted.receipt().digest()?,
            database_identity: admitted.binding().database_identity.clone(),
        };
        let closure = verify_source(
            source,
            self.source.marker(),
            ImportDatabaseKind::AdmittedOrdinary,
            prefix,
            ImportSourceReferences::AdmittedOrdinary {
                marker: self.source.marker(),
                admission: &source_binding,
            },
            None,
            self.image.digest(),
            Some(cancel),
        )?;
        let private = destination_ready::read_ordinary_private_ready(
            destination,
            self.destination.marker(),
            family,
            Some(cancel),
        )?;
        self.destination.require_parts(&private, cancel)?;
        self.source.require_ready_source(private.ready())?;
        if private.ready().transaction_id != staging.transaction_id
            || private.ready().destination != staging.destination
            || private.ready().plan != staging.plan
            || private.family_marker_bytes() != self.destination.family().marker_bytes()
        {
            return Err(rejected(
                "fresh family actual C Ready differs from B stored destination/plan",
            ));
        }
        let historical = closure.copied_content_mappings(source, prefix, Some(cancel))?;
        let mappings = include_selected_bases(source, &historical, cancel).map_err(Cause::from)?;
        let retained = exact_manifests(&mappings, private.required(), cancel)?;
        // Canonical predecessor calculation only; actual C is decoded Ready.
        let mut destination_staging = ProviderTransitionIntentRecord::decode(&staging.encode()?)?;
        destination_staging.role = TransitionRole::Destination;
        destination_staging.local_generation = 0;
        destination_staging.validate_fields()?;
        let plan = reserve_plan::derive(
            source,
            prefix,
            &closure,
            self.image,
            &retained,
            staging,
            &destination_staging,
            cancel,
        )
        .map_err(Cause::from)?;
        require_union(&plan.required, private.required())?;
        let context = AdmittedFamilyContext::new(&admitted)?;
        let target = ExportReserveOnlyTarget::new(
            &context,
            staging,
            &destination_staging,
            &plan.required,
            plan.baseline_quota,
            plan.actual_metadata_floor,
            Some(cancel),
        )?;
        let data = OwnedOrdinaryReadyData::new(
            plan.data,
            staging,
            self.image,
            &plan.required,
            private.protection(),
            plan.actual_metadata_floor,
            cancel,
        )?;
        if data.ready() != private.ready()
            || data.receipt() != private.receipt()
            || data.metadata_floor() != private.metadata_floor()
        {
            return Err(rejected(
                "fresh family rederived Ready/receipt/fixed funding differs",
            ));
        }
        let expected = content::expected_checkout(staging, self.image, cancel)?;
        if expected.header() != private.expected().header()
            || expected.files() != private.expected().files()
            || expected.directories() != private.expected().directories()
            || expected.deletions() != private.expected().deletions()
        {
            return Err(rejected(
                "fresh family B image differs from actual C checkout",
            ));
        }
        let journal = checkout_journal::read(destination, &expected, Some(cancel))?;
        if journal.rows() != private.journal().rows() {
            return Err(rejected("fresh family actual complete C journal differs"));
        }
        let calculated = data.as_data(&plan.required)?;
        let inputs = OrdinaryReadyInputs {
            marker: private.marker(),
            source_marker: self.source.marker(),
            source_staging: staging,
            closure: &closure,
            retained: &context,
            target: &target,
            protection: private.protection(),
            family_marker_bytes: self.destination.family().marker_bytes(),
            metadata_floor: data.metadata_floor(),
            data: &calculated,
        };
        let verified = destination_ready::read_ordinary_family_publication(
            source,
            destination,
            family,
            &inputs,
            &expected,
            Some(cancel),
        )?;
        private.require_same(verified.snapshot())?;
        let before = verified.state();
        self.require_identity(cancel)?;
        let snapshot = verified.into_snapshot();
        if let Some(expected) = self.expected {
            expected.require_same(&snapshot)?;
        }
        let Some((predecessor, step)) = mutation else {
            return Ok(Box::new(Validated {
                snapshot,
                state: before,
                effect: None,
            }));
        };
        if predecessor != before || before.member != ExportPublicationMember::Initializing {
            return Err(rejected("fresh family actual writer predecessor changed"));
        }
        *step = AdmissionStep::MutationAttempted;
        let effect = context.publish_export_ready(
            family,
            self.destination.family().marker_bytes(),
            &inputs.publication_inputs(&expected),
            Some(cancel),
        )?;
        if effect != ExportPublicationEffect::Updated {
            return Err(rejected(
                "fresh family CAS did not update exact Initializing member",
            ));
        }
        // No new namespace/hash work beneath the SQL/catalog gate. Repeat the
        // complete actual-B-backed reader; only the member phase may change.
        self.require_observation(source, cancel)?;
        let after = destination_ready::read_ordinary_family_publication(
            source,
            destination,
            family,
            &inputs,
            &expected,
            Some(cancel),
        )?;
        let state = after.state();
        let current = after.into_snapshot();
        snapshot.require_same(&current)?;
        if state.member != ExportPublicationMember::Ready || state.protection != before.protection {
            return Err(rejected(
                "fresh family prospective Ready/protection changed",
            ));
        }
        self.require_identity(cancel)?;
        Ok(Box::new(Validated {
            snapshot: current,
            state,
            effect: Some(effect),
        }))
    }
}

fn read_observation(
    source: &OrdinaryExclusiveSource,
    c: &Connection,
    transaction_id: &str,
    cancel: &Cancellation,
) -> CauseResult<Box<SourceObservation>> {
    source.require_snapshot(c, cancel)?;
    let state = ordinary_transition::read_publication_state(
        c,
        source.marker(),
        transaction_id,
        Some(cancel),
    )?;
    if state.intent.state != TransitionState::Staging
        || state.intent.role != TransitionRole::Source
        || state.intent.receipt_id.is_some()
        || state.receipt.is_some()
    {
        return Err(rejected(
            "fresh family constructor requires actual B Staging without receipt",
        ));
    }
    let current = database::current(c).map_err(Cause::from)?;
    if current.snapshot.path_semantics != layrs_core::projection::PathSemantics::PortableWindows {
        return Err(rejected(
            "fresh family export requires portable Windows path semantics",
        ));
    }
    let selected = selection::for_scan(c, &current.snapshot).map_err(Cause::from)?;
    let version = state.intent.plan.source_prefix.source.version();
    if current.generation != version.generation || current.digest != version.snapshot_digest {
        return Err(rejected("fresh family actual source frontier differs"));
    }
    Ok(Box::new(SourceObservation {
        staging: state.intent,
        snapshot: current.snapshot,
        selection: selected,
    }))
}

fn require_image(observation: &SourceObservation, image: &OrdinaryExportImage) -> CauseResult<()> {
    if image.digest()
        != observation
            .staging
            .plan
            .source_prefix
            .source
            .version()
            .effective_image_digest
    {
        return Err(rejected(
            "fresh Ordinary family source image digest changed",
        ));
    }
    Ok(())
}

fn exact_manifests(
    mappings: &Mappings,
    union: &[FileManifest],
    cancel: &Cancellation,
) -> CauseResult<BTreeMap<FileId, FileManifest>> {
    let indexed: BTreeMap<_, _> = union
        .iter()
        .map(|m| ((m.file_id(), m.id(), m.byte_length()), m))
        .collect();
    let mut retained = BTreeMap::new();
    let mut bytes = 0usize;
    for (file, (id, length)) in mappings {
        cancel.check()?;
        let manifest = indexed.get(&(file, id, *length)).ok_or_else(|| {
            rejected("fresh family exact private representation absent from provisional C union")
        })?;
        bytes = bytes
            .checked_add(
                canonical_json(*manifest)
                    .map_err(|e| rejected(e.to_string()))?
                    .len(),
            )
            .filter(|n| *n <= 32 * 1024 * 1024)
            .ok_or_else(|| rejected("fresh family retained canonical manifests exceed bound"))?;
        retained.insert(file.clone(), (**manifest).clone());
    }
    Ok(retained)
}

fn require_union(expected: &[FileManifest], actual: &[FileManifest]) -> CauseResult<()> {
    let expected_map: BTreeMap<_, _> = expected.iter().map(|m| (m.id(), m)).collect();
    let actual_map: BTreeMap<_, _> = actual.iter().map(|m| (m.id(), m)).collect();
    if expected.len() != actual.len()
        || expected_map.len() != expected.len()
        || actual_map.len() != actual.len()
        || expected_map != actual_map
    {
        return Err(rejected(
            "fresh family complete exact manifest union differs",
        ));
    }
    Ok(())
}

fn rollback_failed_action<T>(c: &Connection, result: CauseResult<T>) -> CauseResult<T> {
    if c.is_autocommit() {
        return result;
    }
    // Transaction cleanup is mandatory even on validation/COMMIT failure.
    // Preserve the original action cause if ROLLBACK also fails.
    reservation::combine(result, c.execute_batch("ROLLBACK").map_err(Cause::from))
}
