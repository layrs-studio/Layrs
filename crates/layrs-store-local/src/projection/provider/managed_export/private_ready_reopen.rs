//! Fresh unchanged B Staging + complete C Staging -> private C Ready only.
//! Actual Staging and actual Ready are distinct closed read branches. Calculated
//! Ready DATA never stands in for an observed suffix or terminal capability.
use super::{content, ready_data::OwnedOrdinaryReadyData, reservation, reserve_plan, *};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary,
            checkout_journal::{CheckoutJournal, ExpectedCheckout},
            database_io::{self, WriterCloseObserver},
            destination_ready::{self, OrdinaryReadyInputs, ReadyEffect},
            import_validation::{ImportDatabaseKind, verify_source},
            managed_ready_reopen::{
                AdmissionStep, OrdinaryExclusiveSource, OrdinaryPrivateReadyParts,
                OrdinaryPrivateReadyPhysical, OrdinaryPrivateReadyResources,
            },
            ordinary_transition,
            records::{
                Record,
                import::{
                    AdmittedSourceBinding, ImportSourceReferences, ProviderTransitionReceiptRecord,
                },
            },
            recovery::error::{
                ProviderBootstrapCause as Cause, Result as CauseResult, invalid as rejected,
            },
        },
        shared::{
            self,
            admitted_catalog::{
                AdmittedFamilyContext, ExportProtectionState, ExportPublicationMember,
                ExportReserveOnlyTarget,
            },
            exact_content::ExactBinding,
            exact_protection::ProtectionPresence,
        },
    },
};
use layrs_core::projection::{ProjectionSelection, ProjectionSnapshot};

pub(in crate::projection::provider) struct OrdinaryPrivateReadyFinalizationProgress {
    effect: ReadyEffect,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
}
impl OrdinaryPrivateReadyFinalizationProgress {
    pub(in crate::projection::provider) fn effect(&self) -> ReadyEffect {
        self.effect
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
}

pub(in crate::projection::provider) struct OrdinaryPrivateReadyFinalizationFailure {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: OrdinaryPrivateReadyFinalization,
}

struct SourceObservation {
    staging: ProviderTransitionIntentRecord,
    snapshot: ProjectionSnapshot,
    selection: ProjectionSelection,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Staging,
    Ready,
}

/// Detached reconstruction DATA, not a terminal Ready snapshot. The phase is
/// observed by its complete corresponding SQL reader. For Staging, expected_*
/// fields are calculated suffix commitments only; no suffix is claimed present.
struct PrivateReadyData {
    phase: Phase,
    expected_ready: ProviderTransitionIntentRecord,
    expected_receipt: ProviderTransitionReceiptRecord,
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    required: Vec<FileManifest>,
    binding: ExactBinding,
    family: ExportProtectionState,
}
impl PrivateReadyData {
    fn parts(&self) -> OrdinaryPrivateReadyParts<'_> {
        OrdinaryPrivateReadyParts {
            ready: &self.expected_ready,
            expected: &self.expected,
            journal: &self.journal,
            required: &self.required,
            binding: &self.binding,
        }
    }

    /// Phase changes require an explicit caller check. All immutable image,
    /// funding, journal and family protection DATA must remain identical.
    fn require_same_image(&self, current: &Self) -> CauseResult<()> {
        if self.expected_ready != current.expected_ready
            || self.expected_receipt != current.expected_receipt
            || self.expected.marker() != current.expected.marker()
            || self.expected.header() != current.expected.header()
            || self.expected.operation() != current.expected.operation()
            || self.expected.files() != current.expected.files()
            || self.expected.directories() != current.expected.directories()
            || self.expected.deletions() != current.expected.deletions()
            || self.expected.declared_encoded_bytes() != current.expected.declared_encoded_bytes()
            || self.expected.maximum_encoded_bytes() != current.expected.maximum_encoded_bytes()
            || self.journal.rows() != current.journal.rows()
            || self.required != current.required
            || self.binding != current.binding
            || self.family != current.family
        {
            return Err(rejected(
                "fresh private Ready immutable reconstruction changed",
            ));
        }
        Ok(())
    }

    fn require_same_phase(&self, current: &Self) -> CauseResult<()> {
        self.require_same_image(current)?;
        if self.phase != current.phase {
            return Err(rejected("fresh private Ready actual phase changed"));
        }
        Ok(())
    }
}

// Large detached summaries remain boxed through every nested reader result.
struct Validated {
    data: Box<PrivateReadyData>,
    effect: Option<ReadyEffect>,
}

/// Acquires B Exclusive, then distinct C Exclusive from the start. Failures
/// retain both owners and every B image/C CAS/checkout pin. Explicit retry
/// reobserves actual Staging/Ready; it never infers an uncertain COMMIT/ACK.
pub(in crate::projection::provider) struct OrdinaryPrivateReadyFinalization {
    image: OrdinaryExportImage,
    physical: OrdinaryPrivateReadyPhysical,
    destination: OrdinaryPrivateReadyResources,
    source: OrdinaryExclusiveSource,
    observation: Box<SourceObservation>,
    data: Box<PrivateReadyData>,
}

impl OrdinaryPrivateReadyFinalization {
    pub(in crate::projection::provider) fn open(
        source_root: &Path,
        destination_root: &Path,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        let source = OrdinaryExclusiveSource::open(source_root, cancel)?;
        let mut observation = None;
        database_io::clean_admission(source.database(), false, |c| {
            observation = Some(read_observation(&source, c, transaction_id, cancel)?);
            Ok(())
        })?;
        let observation =
            observation.ok_or_else(|| rejected("fresh private Ready source observation absent"))?;
        let destination = source.inspect_private_ready_destination(destination_root, cancel)?;
        // No SQL or catalog gate survives the B preflight or surrounds hashes.
        let image = OrdinaryExportImage::build_parts(
            source.root(),
            source.marker(),
            &observation.snapshot,
            &observation.selection,
            cancel,
        )
        .map_err(Cause::from)?;
        require_image(&observation, &image)?;
        let before = Reconstruction {
            source: &source,
            destination: &destination,
            observation: &observation,
            image: &image,
            expected: None,
            physical: None,
        }
        .read_validated(cancel)?;
        // Complete actual B/C/family interpretation and positive RO closes
        // precede pinning C; a complete Staging journal supplies physical DATA.
        let physical = destination.pin_checkout(&before.data.parts(), cancel)?;
        let mut held = Self {
            image,
            physical,
            destination,
            source,
            observation,
            data: before.data,
        };
        held.require_physical(cancel)?;
        let current = held.reconstruction().read_validated(cancel)?;
        held.data.require_same_phase(&current.data)?;
        held.require_physical(cancel)?;
        Ok(held)
    }

    pub(in crate::projection::provider) fn finalize_ready(
        mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<
        OrdinaryPrivateReadyFinalizationProgress,
        OrdinaryPrivateReadyFinalizationFailure,
    > {
        let mut step = AdmissionStep::Validating;
        let result = self.finalize_inner(close, cancel, &mut step);
        result.map_err(|cause| OrdinaryPrivateReadyFinalizationFailure {
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
            expected: Some(&self.data),
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
        let parts = self.data.parts();
        self.destination.require_parts(&parts, cancel)?;
        self.physical.require_physical(&parts, cancel)
    }

    fn finalize_inner(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> CauseResult<OrdinaryPrivateReadyFinalizationProgress> {
        self.require_physical(cancel)?;
        self.reconstruction().read_validated(cancel)?;
        self.require_physical(cancel)?;
        let reconstruction = self.reconstruction();
        let family = self.destination.family();
        let after = {
            let _catalog = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            // B and family remain strict read-only through C COMMIT/close.
            // No independently opened C reader survives to its writer.
            reservation::valid(database_io::read_cancellable_strict(
                self.source.database(),
                cancel,
                |source| {
                    reservation::valid(database_io::read_cancellable_strict(
                        &family.catalog,
                        cancel,
                        |catalog| {
                            let before = reservation::valid(database_io::read_cancellable_strict(
                                self.destination.database(),
                                cancel,
                                |destination| {
                                    reconstruction.validate(
                                        source,
                                        destination,
                                        catalog,
                                        None,
                                        cancel,
                                    )
                                },
                            )?)?;
                            if before.data.phase == Phase::Ready {
                                return Ok(Box::new(Validated {
                                    data: before.data,
                                    effect: Some(ReadyEffect::AlreadyReady),
                                }));
                            }
                            let admission = database_io::clean_admission(
                                self.destination.database(),
                                false,
                                |destination| {
                                    let current = reconstruction.validate(
                                        source,
                                        destination,
                                        catalog,
                                        None,
                                        cancel,
                                    )?;
                                    before.data.require_same_phase(&current.data)
                                },
                            )?;
                            reconstruction.require_identity(cancel)?;
                            *step = AdmissionStep::WriterOpenAttempted;
                            let bare = admission.open(self.destination.database(), close)?;
                            let written = (|| {
                                admission.require_geometry(&bare)?;
                                let destination = bare.connection();
                                destination.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                                let after = reconstruction.validate(
                                    source,
                                    destination,
                                    catalog,
                                    Some(&mut *step),
                                    cancel,
                                )?;
                                before.data.require_same_image(&after.data)?;
                                if after.data.phase != Phase::Ready {
                                    return Err(rejected(
                                        "fresh private Ready finalizer did not produce actual Ready",
                                    ));
                                }
                                reconstruction.require_identity(cancel)?;
                                cancel.check()?;
                                *step = AdmissionStep::CommitAttempted;
                                destination.execute_batch("COMMIT")?;
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
        // Errors above propagate immediately after mandatory rollback/closes;
        // no concluding filesystem or SQL probe runs on a failed boundary.
        *step = AdmissionStep::ReadScopesClosed;
        self.require_physical(cancel)?;
        let current = self.reconstruction().read_validated(cancel)?;
        after.data.require_same_phase(&current.data)?;
        if current.data.phase != Phase::Ready {
            return Err(rejected("fresh private Ready durable phase differs"));
        }
        self.require_physical(cancel)?;
        let effect = after
            .effect
            .ok_or_else(|| rejected("fresh private Ready finalization effect absent"))?;
        let data = current.data;
        *step = AdmissionStep::FreshVerified;
        Ok(OrdinaryPrivateReadyFinalizationProgress {
            effect,
            ready: data.expected_ready,
            receipt: data.expected_receipt,
        })
    }
}

struct Reconstruction<'a> {
    source: &'a OrdinaryExclusiveSource,
    destination: &'a OrdinaryPrivateReadyResources,
    observation: &'a SourceObservation,
    image: &'a OrdinaryExportImage,
    expected: Option<&'a PrivateReadyData>,
    physical: Option<&'a OrdinaryPrivateReadyPhysical>,
}

impl Reconstruction<'_> {
    fn require_identity(&self, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_identity(cancel)?;
        self.image.verify_identity(cancel).map_err(Cause::from)?;
        self.destination.require_identity(cancel)?;
        if let Some(data) = self.expected {
            let parts = data.parts();
            self.destination.require_parts(&parts, cancel)?;
            if let Some(physical) = self.physical {
                physical.require_identity(&parts, cancel)?;
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
                    &family.catalog,
                    cancel,
                    |catalog| {
                        reservation::valid(database_io::read_cancellable_strict(
                            self.destination.database(),
                            cancel,
                            |destination| self.validate(source, destination, catalog, None, cancel),
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
                "fresh private Ready requires actual unchanged B Staging without receipt",
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
                "fresh private Ready source frontier/snapshot/selection changed",
            ));
        }
        require_image(self.observation, self.image)
    }

    fn validate(
        &self,
        source: &Connection,
        destination: &Connection,
        family: &Connection,
        mutation: Option<&mut AdmissionStep>,
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
        // The bounded discriminant only selects a full actual-phase reader.
        let phase = read_phase(destination, self.destination.marker(), cancel)?;
        let historical = closure.copied_content_mappings(source, prefix, Some(cancel))?;
        let mappings = include_selected_bases(source, &historical, cancel).map_err(Cause::from)?;
        let retained = exact_manifests(family, &mappings, cancel)?;
        // Calculation DATA only. Neither this canonical Staging projection nor
        // the calculated Ready record is presented as actual C SQL evidence.
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
        let protection = content::expected_protection(&target, &plan, cancel)?;
        let data = OwnedOrdinaryReadyData::new(
            plan.data,
            staging,
            self.image,
            &plan.required,
            &protection,
            plan.actual_metadata_floor,
            cancel,
        )?;
        let expected = content::expected_checkout(staging, self.image, cancel)?;
        self.source.require_ready_source(data.ready())?;
        let calculated = data.as_data(&plan.required)?;
        let inputs = OrdinaryReadyInputs {
            marker: self.destination.marker(),
            source_marker: self.source.marker(),
            source_staging: staging,
            closure: &closure,
            retained: &context,
            target: &target,
            protection: &protection,
            family_marker_bytes: self.destination.family().marker_bytes(),
            metadata_floor: data.metadata_floor(),
            data: &calculated,
        };
        // This complete actual retained+Protected walk explicitly requires C's
        // member Initializing even on actual Ready replay.
        let family_state = context
            .validate_export_protection(
                family,
                inputs.family_marker_bytes,
                &target,
                &plan.required,
                &protection,
                Some(cancel),
            )?
            .1;
        if family_state.root != ProtectionPresence::Present {
            return Err(rejected(
                "fresh private Ready requires committed exact C protection",
            ));
        }
        let current = read_actual(
            source,
            destination,
            family,
            &inputs,
            &expected,
            target.binding(),
            family_state,
            phase,
            cancel,
        )?;
        self.destination.require_parts(&current.parts(), cancel)?;
        if let Some(expected) = self.expected {
            expected.require_same_image(&current)?;
        }
        self.require_identity(cancel)?;
        let Some(step) = mutation else {
            return Ok(Box::new(Validated {
                data: current,
                effect: None,
            }));
        };
        if current.phase != Phase::Staging {
            return Err(rejected(
                "fresh private Ready writer requires actual Staging predecessor",
            ));
        }
        *step = AdmissionStep::MutationAttempted;
        let effect = destination_ready::finalize_ordinary_checkout_or_replay(
            source,
            destination,
            family,
            &inputs,
            &expected,
            Some(cancel),
        )?;
        if effect != ReadyEffect::Finalized {
            return Err(rejected(
                "fresh private Ready writer did not finalize actual Staging",
            ));
        }
        self.require_observation(source, cancel)?;
        let after = read_actual(
            source,
            destination,
            family,
            &inputs,
            &expected,
            target.binding(),
            family_state,
            Phase::Ready,
            cancel,
        )?;
        current.require_same_image(&after)?;
        self.destination.require_parts(&after.parts(), cancel)?;
        self.require_identity(cancel)?;
        Ok(Box::new(Validated {
            data: after,
            effect: Some(effect),
        }))
    }
}

/// Each branch completes its actual private SQL interpretation. An actual
/// Staging journal and calculated suffix commitments remain explicitly tagged.
#[allow(clippy::too_many_arguments)]
fn read_actual(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    binding: &ExactBinding,
    family_state: ExportProtectionState,
    phase: Phase,
    cancel: &Cancellation,
) -> CauseResult<Box<PrivateReadyData>> {
    match phase {
        Phase::Staging => {
            let journal = destination_ready::read_ordinary_checkout_finalizable(
                source,
                destination,
                family,
                inputs,
                expected,
                Some(cancel),
            )?;
            if !journal.is_complete() {
                return Err(rejected(
                    "fresh private Ready requires complete published checkout journal",
                ));
            }
            let mut required = inputs.data.required.to_vec();
            required.sort_by(|a, b| a.id().cmp(b.id()));
            Ok(Box::new(PrivateReadyData {
                phase,
                expected_ready: ProviderTransitionIntentRecord::decode(
                    &inputs.data.ready.encode()?,
                )?,
                expected_receipt: ProviderTransitionReceiptRecord::decode(
                    &inputs.data.receipt.encode()?,
                )?,
                expected: ExpectedCheckout::ordinary(
                    inputs.source_staging,
                    expected.files(),
                    expected.deletions(),
                    &expected.header().image_digest,
                    Some(cancel),
                )?,
                journal: *journal,
                required,
                binding: binding.clone(),
                family: family_state,
            }))
        }
        Phase::Ready => {
            let verified = destination_ready::read_ordinary_family_publication(
                source,
                destination,
                family,
                inputs,
                expected,
                Some(cancel),
            )?;
            let state = verified.state();
            if state.member != ExportPublicationMember::Initializing
                || state.protection != family_state
            {
                return Err(rejected(
                    "fresh private Ready replay requires unchanged Initializing family",
                ));
            }
            // Only this branch has actual Ready evidence. Move its genuinely
            // interpreted fields into local DATA, never use a terminal holder
            // or construct a terminal snapshot for Staging.
            let snapshot = verified.into_snapshot();
            Ok(Box::new(PrivateReadyData {
                phase,
                expected_ready: snapshot.ready,
                expected_receipt: snapshot.receipt,
                expected: snapshot.expected,
                journal: snapshot.journal,
                required: snapshot.required,
                binding: snapshot.binding,
                family: family_state,
            }))
        }
    }
}

fn read_phase(
    c: &Connection,
    marker: &format::Marker,
    cancel: &Cancellation,
) -> CauseResult<Phase> {
    super::super::import_validation::require_schema(
        c,
        marker,
        ImportDatabaseKind::Provider,
        Some(cancel),
    )?;
    let (count, bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(state AS BLOB))),0) FROM (SELECT state FROM projection_provider_transitions LIMIT 2)",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || bytes > 16 {
        return Err(rejected("fresh private Ready phase inventory differs"));
    }
    let state: String = c.query_row(
        "SELECT state FROM projection_provider_transitions",
        [],
        |r| r.get(0),
    )?;
    match state.as_str() {
        "staging" => Ok(Phase::Staging),
        "ready" => Ok(Phase::Ready),
        _ => Err(rejected("fresh private Ready actual C phase differs")),
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
            "fresh private Ready constructor requires actual B Staging without receipt",
        ));
    }
    let current = database::current(c).map_err(Cause::from)?;
    if current.snapshot.path_semantics != layrs_core::projection::PathSemantics::PortableWindows {
        return Err(rejected(
            "fresh private Ready export requires portable Windows path semantics",
        ));
    }
    let selected = selection::for_scan(c, &current.snapshot).map_err(Cause::from)?;
    let version = state.intent.plan.source_prefix.source.version();
    if current.generation != version.generation || current.digest != version.snapshot_digest {
        return Err(rejected(
            "fresh private Ready actual source frontier differs",
        ));
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
            "fresh Ordinary private Ready source image digest changed",
        ));
    }
    Ok(())
}

fn exact_manifests(
    family: &Connection,
    mappings: &Mappings,
    cancel: &Cancellation,
) -> CauseResult<BTreeMap<FileId, FileManifest>> {
    if mappings.len() > 100_000 {
        return Err(rejected(
            "fresh private Ready exact mapping count exceeds bound",
        ));
    }
    let mut retained = BTreeMap::new();
    let mut bytes = 0usize;
    for (file, (id, length)) in mappings {
        cancel.check()?;
        let manifest = shared::read_manifest_exact(family, id, Some(cancel))
            .map_err(Cause::from)?
            .ok_or_else(|| rejected("fresh private Ready exact family ManifestId missing"))?;
        if manifest.id() != id || manifest.file_id() != file || manifest.byte_length() != *length {
            return Err(rejected(
                "fresh private Ready exact family manifest mapping differs",
            ));
        }
        bytes = bytes
            .checked_add(
                canonical_json(&manifest)
                    .map_err(|e| rejected(e.to_string()))?
                    .len(),
            )
            .filter(|n| *n <= 32 * 1024 * 1024)
            .ok_or_else(|| {
                rejected("fresh private Ready retained canonical manifests exceed bound")
            })?;
        cancel.check()?;
        retained.insert(file.clone(), manifest);
    }
    Ok(retained)
}

fn rollback_failed_action<T>(c: &Connection, result: CauseResult<T>) -> CauseResult<T> {
    if c.is_autocommit() {
        return result;
    }
    reservation::combine(result, c.execute_batch("ROLLBACK").map_err(Cause::from))
}
