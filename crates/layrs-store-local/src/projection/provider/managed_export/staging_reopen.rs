//! Fresh unchanged B Staging -> Ready only. New B image pins independently
//! witness the stored commitment; C's copied image cannot impersonate B.
//! No Access, ContentScope, allocation, old image, recovery or repair is adopted.
use super::{content, ready_data::OwnedOrdinaryReadyData, reservation, reserve_plan, *};
use crate::projection::shared::{
    admitted_catalog::{AdmittedFamilyContext, ExportReserveOnlyTarget},
    exact_protection::ExactProtection,
};
use crate::{
    lock::StoreLock,
    projection::provider::{
        admitted_ordinary, checkout_journal,
        database_io::{self, WriterCloseObserver},
        destination_ready::{self, OrdinaryReadyInputs},
        import_validation::{ImportDatabaseKind, verify_source},
        managed_ready_reopen::{AdmissionStep, OrdinaryExclusiveSource, OrdinaryReadyInspection},
        ordinary_publication::OrdinaryPublicationSource,
        ordinary_transition::{self, PublicationCapacity, PublicationState},
        records::{
            Record,
            import::{
                AdmittedSourceBinding, ImportSourceReferences, ProviderTransitionReceiptRecord,
            },
        },
        recovery::error::{
            ProviderBootstrapCause as Cause, Result as CauseResult, invalid as rejected,
        },
        transition_sql::SourceIntentEffect,
    },
};
use layrs_core::projection::{ProjectionSelection, ProjectionSnapshot};

pub(in crate::projection::provider) struct OrdinaryStagingPromotionProgress {
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    ready_digest: String,
}
impl OrdinaryStagingPromotionProgress {
    pub(in crate::projection::provider) fn effect(&self) -> SourceIntentEffect {
        SourceIntentEffect::Updated
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(in crate::projection::provider) fn ready_digest(&self) -> &str {
        &self.ready_digest
    }
}

pub(in crate::projection::provider) struct OrdinaryStagingPromotionFailure {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: OrdinaryStagingPromotion,
}

struct SourceObservation {
    staging: ProviderTransitionIntentRecord,
    snapshot: ProjectionSnapshot,
    selection: ProjectionSelection,
}

struct Validated {
    source: PublicationState,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    capacity: PublicationCapacity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Staging,
    Ready,
}

/// Only real Staging can construct this holder. Failure retains every new
/// B/C owner and image/CAS/checkout pin. There is no in-holder replay/resolve:
/// after an uncertain ACK, explicitly release it before using the distinct
/// Ready adapter. Failed native-close quarantine remains a refusal, not retry.
pub(in crate::projection::provider) struct OrdinaryStagingPromotion {
    image: OrdinaryExportImage,
    destination: OrdinaryReadyInspection,
    source: OrdinaryExclusiveSource,
    observation: SourceObservation,
    protection: ExactProtection,
}

impl OrdinaryStagingPromotion {
    pub(in crate::projection::provider) fn open(
        source_root: &Path,
        destination_root: &Path,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        #[cfg(test)]
        trace_stage("open/source-owner");
        let source = OrdinaryExclusiveSource::open(source_root, cancel)?;
        let mut observed = None;
        // Genuine clean B RO preflight: DATA escapes only after positive close.
        database_io::clean_admission(source.database(), false, |c| {
            observed = Some(read_observation(&source, c, transaction_id, cancel)?);
            Ok(())
        })?;
        let observation =
            observed.ok_or_else(|| rejected("fresh Staging source observation absent"))?;
        #[cfg(test)]
        trace_stage("open/destination-inspection");
        let destination = source.inspect_ready_destination(destination_root, cancel)?;
        source.require_ready_source(destination.ready())?;
        if destination.ready().transaction_id != observation.staging.transaction_id {
            return Err(rejected("fresh Staging destination transaction differs"));
        }
        // No SQL or catalog gate is held for selected namespace scans/hashes.
        #[cfg(test)]
        trace_stage("open/image");
        let image = OrdinaryExportImage::build_parts(
            source.root(),
            source.marker(),
            &observation.snapshot,
            &observation.selection,
            cancel,
        )
        .map_err(Cause::from)?;
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
                "fresh Ordinary Staging source image digest changed",
            ));
        }
        let binding = &destination.snapshot().binding;
        let protection = ExactProtection::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            destination.required(),
            Some(cancel),
        )?;
        let mut held = Self {
            image,
            destination,
            source,
            observation,
            protection,
        };
        held.require_physical(cancel)?;
        #[cfg(test)]
        trace_stage("open/validated-read");
        // Re-read exact B phase/snapshot/selection/history AFTER image work,
        // together with complete genuine C8/family Ready and its fixed budget.
        held.read_validated(Phase::Staging, cancel)?;
        held.require_physical(cancel)?;
        #[cfg(test)]
        trace_stage("open/complete");
        Ok(held)
    }

    pub(in crate::projection::provider) fn promote_ready(
        mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<OrdinaryStagingPromotionProgress, OrdinaryStagingPromotionFailure>
    {
        let mut step = AdmissionStep::Validating;
        let result = self.promote_inner(close, cancel, &mut step);
        result.map_err(|cause| OrdinaryStagingPromotionFailure {
            step,
            cause,
            held: self,
        })
    }

    fn require_identity(&self, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_identity(cancel)?;
        self.image.verify_identity(cancel).map_err(Cause::from)?;
        self.destination.require_identity(cancel)
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
        if self.image.digest()
            != self
                .observation
                .staging
                .plan
                .source_prefix
                .source
                .version()
                .effective_image_digest
        {
            return Err(rejected(
                "fresh Ordinary Staging source image digest changed",
            ));
        }
        self.destination.require_physical(cancel)
    }

    fn require_observation(
        &self,
        c: &Connection,
        phase: Phase,
        cancel: &Cancellation,
    ) -> CauseResult<PublicationState> {
        self.source.require_snapshot(c, cancel)?;
        let actual = ordinary_transition::read_publication_state(
            c,
            self.source.marker(),
            &self.observation.staging.transaction_id,
            Some(cancel),
        )?;
        match phase {
            Phase::Staging => {
                if actual.intent != self.observation.staging || actual.receipt.is_some() {
                    return Err(rejected(
                        "fresh promotion requires actual unchanged B Staging",
                    ));
                }
            }
            Phase::Ready => {
                if actual.intent.state != TransitionState::Ready
                    || actual.receipt.as_ref() != Some(self.destination.receipt())
                {
                    return Err(rejected(
                        "fresh promotion prospective/durable B Ready differs",
                    ));
                }
                actual
                    .intent
                    .require_transition(&self.observation.staging)?;
                actual.intent.require_receipt(self.destination.receipt())?;
            }
        }
        let current = database::current(c).map_err(Cause::from)?;
        let selected = selection::for_scan(c, &current.snapshot).map_err(Cause::from)?;
        let version = self.observation.staging.plan.source_prefix.source.version();
        if current.generation != version.generation
            || current.digest != version.snapshot_digest
            || current.snapshot != self.observation.snapshot
            || selected != self.observation.selection
        {
            return Err(rejected(
                "fresh Staging source snapshot/selection changed after image work",
            ));
        }
        Ok(actual)
    }

    /// All operations here are SQL, bounded calculation DATA or retained
    /// identity checks. Full image/CAS/namespace verification stays outside.
    fn validate(
        &self,
        source: &Connection,
        destination: &Connection,
        family: &Connection,
        phase: Phase,
        cancel: &Cancellation,
    ) -> CauseResult<Box<Validated>> {
        #[cfg(test)]
        trace_stage("validate/observation");
        let state = self.require_observation(source, phase, cancel)?;
        let staging = &self.observation.staging;
        let prefix = &staging.plan.source_prefix;
        let admitted = admitted_ordinary::inspect(source, self.source.marker(), Some(cancel))?;
        let binding = AdmittedSourceBinding {
            admission_digest: admitted.admission_digest().to_owned(),
            incoming_prefix_id: admitted.prefix_id().to_owned(),
            incoming_receipt_id: admitted.receipt().digest()?,
            database_identity: admitted.binding().database_identity.clone(),
        };
        // The digest is the independently hashed NEW B image, never the stored
        // prefix digest supplied as its own physical witness.
        let closure = verify_source(
            source,
            self.source.marker(),
            ImportDatabaseKind::AdmittedOrdinary,
            prefix,
            ImportSourceReferences::AdmittedOrdinary {
                marker: self.source.marker(),
                admission: &binding,
            },
            None,
            self.image.digest(),
            Some(cancel),
        )?;
        #[cfg(test)]
        trace_stage("validate/destination-ready");
        let actual = destination_ready::read_ordinary_managed_ready(
            destination,
            &self.destination.snapshot().marker,
            family,
            Some(cancel),
        )?;
        let capacity = actual.publication_capacity(Some(cancel))?;
        let actual = actual.into_snapshot()?;
        self.destination.snapshot().require_same(&actual)?;
        let historical = closure.copied_content_mappings(source, prefix, Some(cancel))?;
        let mappings = include_selected_bases(source, &historical, cancel).map_err(Cause::from)?;
        let retained = exact_manifests(&mappings, &actual.required, cancel)?;
        // This projection is calculation DATA only. Actual C remains Ready,
        // independently authenticated above and by the paired reader below.
        let mut destination_staging = ProviderTransitionIntentRecord::decode(&staging.encode()?)?;
        destination_staging.role = TransitionRole::Destination;
        destination_staging.local_generation = 0;
        destination_staging.validate_fields()?;
        #[cfg(test)]
        trace_stage("validate/reserve-plan");
        let plan = reserve_plan::derive(
            source,
            prefix,
            &closure,
            &self.image,
            &retained,
            staging,
            &destination_staging,
            cancel,
        )
        .map_err(Cause::from)?;
        require_union(&plan.required, &actual.required)?;
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
            &self.image,
            &plan.required,
            &self.protection,
            plan.actual_metadata_floor,
            cancel,
        )?;
        #[cfg(test)]
        trace_stage("validate/checkout");
        if data.ready() != &actual.ready || data.receipt() != &actual.receipt {
            return Err(rejected(
                "fresh Staging rederived Ready/receipt/immutable quota differs",
            ));
        }
        let expected = content::expected_checkout(staging, &self.image, cancel)?;
        if expected.header() != actual.expected.header()
            || expected.files() != actual.expected.files()
            || expected.directories() != actual.expected.directories()
            || expected.deletions() != actual.expected.deletions()
        {
            return Err(rejected(
                "fresh Staging source image differs from actual C checkout",
            ));
        }
        let journal = checkout_journal::read(destination, &expected, Some(cancel))?;
        if journal.rows() != actual.journal.rows() {
            return Err(rejected("fresh Staging actual complete C journal differs"));
        }
        let ready_floor = data
            .metadata_floor()
            .checked_add(expected.maximum_encoded_bytes())
            .ok_or_else(|| rejected("fresh Staging publication metadata overflow"))?;
        let remaining = actual
            .installation
            .remaining
            .checked_sub(expected.header().file_bytes)
            .ok_or_else(|| rejected("fresh Staging checkout exceeds reservation"))?;
        let rederived_capacity =
            PublicationCapacity::new(&actual.ready, &actual.receipt, ready_floor, remaining)?;
        if rederived_capacity.metadata_floor() != capacity.metadata_floor() {
            return Err(rejected(
                "fresh Staging true C metadata capacity differs from rederived B",
            ));
        }
        self.protection
            .require_envelope(remaining, capacity.metadata_floor())?;
        let publication = OrdinaryPublicationSource::new(
            source,
            self.source.marker(),
            &closure,
            staging,
            data.ready(),
            data.receipt(),
            Some(cancel),
        )?;
        let calculated = data.as_data(&plan.required)?;
        let inputs = OrdinaryReadyInputs {
            marker: &actual.marker,
            source_marker: self.source.marker(),
            source_staging: staging,
            closure: &closure,
            retained: &context,
            target: &target,
            protection: &self.protection,
            family_marker_bytes: self.destination.family().marker_bytes(),
            metadata_floor: data.metadata_floor(),
            data: &calculated,
        };
        #[cfg(test)]
        trace_stage("validate/paired-ready");
        let ready = destination_ready::read_ordinary_ready_for_source_publication(
            source,
            destination,
            family,
            &publication,
            &inputs,
            &expected,
            Some(cancel),
        )?;
        let ready_record = ProviderTransitionIntentRecord::decode(&ready.ready().encode()?)?;
        let receipt = ProviderTransitionReceiptRecord::decode(&ready.receipt().encode()?)?;
        if ready_record != actual.ready || receipt != actual.receipt {
            return Err(rejected("fresh Staging paired actual C Ready DATA differs"));
        }
        self.require_identity(cancel)?;
        #[cfg(test)]
        trace_stage("validate/complete");
        Ok(Box::new(Validated {
            source: state,
            ready: ready_record,
            receipt,
            capacity,
        }))
    }

    // Carry owned DATA through the nested reader/close results by pointer.
    // Keeping the complete records inline duplicates large return slots in
    // each debug reader frame. Proof borrows remain inside validate's scopes;
    // all three positive closes still precede returning this owned value.
    fn read_validated(&self, phase: Phase, cancel: &Cancellation) -> CauseResult<Box<Validated>> {
        self.require_identity(cancel)?;
        let family = self.destination.family();
        let _catalog = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        reservation::valid(database_io::read_cancellable_strict(
            self.destination.database(),
            cancel,
            |destination| {
                reservation::valid(database_io::read_cancellable_strict(
                    &family.catalog,
                    cancel,
                    |catalog| {
                        reservation::valid(database_io::read_cancellable_strict(
                            self.source.database(),
                            cancel,
                            |source| self.validate(source, destination, catalog, phase, cancel),
                        )?)
                    },
                )?)
            },
        )?)
    }

    fn promote_inner(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> CauseResult<OrdinaryStagingPromotionProgress> {
        self.require_physical(cancel)?;
        self.read_validated(Phase::Staging, cancel)?;
        self.require_physical(cancel)?;
        let family = self.destination.family();
        let after = {
            let _catalog = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            reservation::valid(database_io::read_cancellable_strict(
                self.destination.database(),
                cancel,
                |destination| {
                    reservation::valid(database_io::read_cancellable_strict(
                        &family.catalog,
                        cancel,
                        |catalog| {
                            let admission = database_io::clean_admission(
                                self.source.database(),
                                false,
                                |source| {
                                    self.validate(
                                        source,
                                        destination,
                                        catalog,
                                        Phase::Staging,
                                        cancel,
                                    )?;
                                    Ok(())
                                },
                            )?;
                            self.require_identity(cancel)?;
                            *step = AdmissionStep::WriterOpenAttempted;
                            let bare = admission.open(self.source.database(), close)?;
                            let result = (|| {
                                admission.require_geometry(&bare)?;
                                let source = bare.connection();
                                source.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                                let before = self.validate(
                                    source,
                                    destination,
                                    catalog,
                                    Phase::Staging,
                                    cancel,
                                )?;
                                *step = AdmissionStep::MutationAttempted;
                                if ordinary_transition::apply_ready(
                                    source,
                                    self.source.marker(),
                                    &before.source.intent,
                                    &before.ready,
                                    &before.receipt,
                                    &before.capacity,
                                    Some(cancel),
                                )? != SourceIntentEffect::Updated
                                {
                                    return Err(rejected(
                                        "fresh Staging promotion did not update actual predecessor",
                                    ));
                                }
                                let after = self.validate(
                                    source,
                                    destination,
                                    catalog,
                                    Phase::Ready,
                                    cancel,
                                )?;
                                self.require_identity(cancel)?;
                                cancel.check()?;
                                *step = AdmissionStep::CommitAttempted;
                                source.execute_batch("COMMIT")?;
                                *step = AdmissionStep::CommitReturned;
                                self.require_identity(cancel)?;
                                Ok(after.source)
                            })();
                            let result = rollback_failed_action(bare.connection(), result);
                            let after = bare.finish_writer(result, close)?;
                            *step = AdmissionStep::WriterClosed;
                            Ok(after)
                        },
                    )?)
                },
            )?)?
        };
        *step = AdmissionStep::ReadScopesClosed;
        // B writer -> family RO -> C RO all closed positively before any
        // full image/namespace check or fresh SQL. Errors stop at cleanup.
        self.require_physical(cancel)?;
        let current = self.read_validated(Phase::Ready, cancel)?.source;
        if current != after {
            return Err(rejected(
                "fresh Staging durable source Ready/receipt changed",
            ));
        }
        self.require_physical(cancel)?;
        let ready_digest = current.intent.digest()?;
        let receipt = current
            .receipt
            .ok_or_else(|| rejected("fresh Staging durable Ready receipt absent"))?;
        *step = AdmissionStep::FreshVerified;
        Ok(OrdinaryStagingPromotionProgress {
            ready: current.intent,
            receipt,
            ready_digest,
        })
    }
}

#[cfg(test)]
#[inline(never)]
fn trace_stage(stage: &str) {
    eprintln!("ORDINARY_STAGING_PHASE={stage}");
}

fn read_observation(
    source: &OrdinaryExclusiveSource,
    c: &Connection,
    transaction_id: &str,
    cancel: &Cancellation,
) -> CauseResult<SourceObservation> {
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
            "fresh promotion constructor requires actual B Staging without receipt",
        ));
    }
    let current = database::current(c).map_err(Cause::from)?;
    if current.snapshot.path_semantics != layrs_core::projection::PathSemantics::PortableWindows {
        return Err(rejected(
            "fresh Staging export requires portable Windows path semantics",
        ));
    }
    let selected = selection::for_scan(c, &current.snapshot).map_err(Cause::from)?;
    let version = state.intent.plan.source_prefix.source.version();
    if current.generation != version.generation || current.digest != version.snapshot_digest {
        return Err(rejected("fresh Staging actual source frontier differs"));
    }
    Ok(SourceObservation {
        staging: state.intent,
        snapshot: current.snapshot,
        selection: selected,
    })
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
            rejected("fresh Staging exact private representation absent from authenticated C union")
        })?;
        bytes = bytes
            .checked_add(
                canonical_json(*manifest)
                    .map_err(|e| rejected(e.to_string()))?
                    .len(),
            )
            .filter(|bytes| *bytes <= 32 * 1024 * 1024)
            .ok_or_else(|| rejected("fresh Staging retained canonical manifests exceed bound"))?;
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
            "fresh Staging complete exact manifest union differs",
        ));
    }
    Ok(())
}

fn rollback_failed_action<T>(source: &Connection, result: CauseResult<T>) -> CauseResult<T> {
    if result.is_ok() || source.is_autocommit() {
        return result;
    }
    match (
        result,
        source.execute_batch("ROLLBACK").map_err(Cause::from),
    ) {
        (Err(primary), Err(close)) => Err(Cause::Cleanup {
            primary: Box::new(primary),
            close: Box::new(close),
        }),
        (result, Ok(())) => result,
        (Ok(_), Err(close)) => Err(close),
    }
}
