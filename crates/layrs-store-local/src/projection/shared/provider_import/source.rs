//! Concrete source interpreters for the common destination registry kernel.
//! Borrowed private SQL evidence never grants physical source bytes or C pins.
use super::*;
use crate::projection::{
    format,
    provider::{
        admitted_ordinary, import_validation::VerifiedImportClosure,
        managed_admission::VerifiedAdmittedImport, ordinary_publication::OrdinaryPublicationSource,
        records::import::AdmittedSourceBinding,
    },
    shared::admitted_catalog::AdmittedFamilyContext,
};

pub(in crate::projection) enum ImportSourceInputs<'p, 'db> {
    Provider(ProviderSourceInputs<'p>),
    Ordinary(OrdinarySourceInputs<'p, 'db>),
}

pub(in crate::projection) struct ProviderSourceInputs<'p> {
    allocation: &'p AllocationRecord,
    prepared: &'p PreparedRecord,
    activation: &'p ProviderActivationRecord,
    capture: &'p PrivateCaptureProof,
    source_context:
        Option<&'p crate::projection::shared::source_companion::SourceCompanionFamilyContext<'p>>,
}

/// Construct only inside the ordinary owner's retained source callback. None
/// of these references may be stored in the destination allocation holder.
/// The closure proves SQL completeness; the owner independently rederives the
/// physical image and retains source/destination ownership around effects.
#[derive(Clone, Copy)]
pub(in crate::projection) struct OrdinarySourceInputs<'p, 'db> {
    connection: &'db Connection,
    marker: &'p format::Marker,
    admitted: &'p VerifiedAdmittedImport<'db>,
    closure: &'p VerifiedImportClosure,
    retained: &'p AdmittedFamilyContext,
}

pub(super) struct ValidatedSource<'p> {
    marker: &'p format::Marker,
}
impl ValidatedSource<'_> {
    pub(super) fn marker(&self) -> &format::Marker {
        self.marker
    }
}

impl<'p, 'db> OrdinarySourceInputs<'p, 'db> {
    /// This constructor retains borrowed evidence only. Every catalog entry
    /// point revalidates the source before interpreting the destination row.
    pub(in crate::projection) fn new(
        connection: &'db Connection,
        marker: &'p format::Marker,
        admitted: &'p VerifiedAdmittedImport<'db>,
        closure: &'p VerifiedImportClosure,
        retained: &'p AdmittedFamilyContext,
    ) -> Self {
        Self {
            connection,
            marker,
            admitted,
            closure,
            retained,
        }
    }

    fn require_private(
        &self,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        self.admitted.require(self.connection, cancel)?;
        self.closure.require_source_connection(self.connection)?;
        if !self
            .admitted
            .outgoing()
            .iter()
            .any(|stored| stored == intent)
            || self.closure.prefix_id() != intent.plan.source_prefix.digest()?
            || self.closure.closure() != &intent.plan.source_prefix.closure
        {
            return Err(invalid("ordinary import source intent/SQL closure differs"));
        }
        // Reinterpret actual ingress in this same snapshot. A detached family
        // context alone cannot nominate a different valid incoming admission.
        let actual = admitted_ordinary::inspect(self.connection, self.marker, cancel)?;
        if self.retained.marker() != self.marker
            || actual.marker() != self.marker
            || self.retained.admission_digest() != actual.admission_digest()
            || self.retained.binding() != actual.binding()
        {
            return Err(invalid("ordinary import source retained admission differs"));
        }
        let shared = self
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary import source shared FORMAT absent"))?;
        let admission = AdmittedSourceBinding {
            admission_digest: actual.admission_digest().into(),
            incoming_prefix_id: actual.prefix_id().into(),
            incoming_receipt_id: actual.receipt().digest()?,
            database_identity: shared.database_identity.clone(),
        };
        intent
            .plan
            .source_prefix
            .require_references(ImportSourceReferences::AdmittedOrdinary {
                marker: self.marker,
                admission: &admission,
            })?;
        let ImportSource::Ordinary {
            private_schema: 9 | 10,
            ..
        } = &intent.plan.source_prefix.source
        else {
            return Err(invalid("ordinary import requires admitted private9 source"));
        };
        cancellation::check(cancel)
    }

    fn require_family(
        &self,
        family: &FamilyAccess,
        c: &Connection,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.require_family_identity(family, intent, cancel)?;
        // The ordinary/Provider wrapper stays strict: active C groups are
        // accepted only by the separately named export path below.
        self.retained
            .validate_catalog(c, family.marker_bytes(), cancel)?;
        family.verify()?;
        cancellation::check(cancel)
    }

    /// Actual B-private interpretation and physical family binding, followed
    /// by the complete retained walk plus one exact ReserveOnly target. The
    /// physical export driver separately authenticates C's actual Staging and
    /// independent pins before entering this catalog scope.
    pub(in crate::projection) fn validate_export_reserve_only(
        &self,
        family: &FamilyAccess,
        c: &Connection,
        intent: &ProviderTransitionIntentRecord,
        target: &crate::projection::shared::admitted_catalog::ExportReserveOnlyTarget,
        cancel: Option<&Cancellation>,
    ) -> Result<crate::projection::shared::admitted_catalog::ExportReserveOnlyState> {
        self.require_export_target(family, intent, target, cancel)?;
        let (_, state) =
            self.retained
                .validate_export_reserve_only(c, family.marker_bytes(), target, cancel)?;
        family.verify()?;
        cancellation::check(cancel)?;
        Ok(state)
    }

    /// Genuine B-private/family checks plus the complete retained-and-active
    /// Installing catalog. The physical driver still authenticates actual C
    /// Staging, complete image/funding and retained pins around every effect.
    pub(in crate::projection) fn validate_export_installing(
        &self,
        family: &FamilyAccess,
        c: &Connection,
        intent: &ProviderTransitionIntentRecord,
        target: &crate::projection::shared::admitted_catalog::ExportReserveOnlyTarget,
        required: &[layrs_core::scm::FileManifest],
        cancel: Option<&Cancellation>,
    ) -> Result<crate::projection::shared::exact_content::ExactInstallState> {
        self.require_export_target(family, intent, target, cancel)?;
        let (_, state) = self.retained.validate_export_installing(
            c,
            family.marker_bytes(),
            target,
            required,
            cancel,
        )?;
        family.verify()?;
        cancellation::check(cancel)?;
        Ok(state)
    }

    /// Complete Published/protection DATA under actual B Staging and the full
    /// retained-family gate. C pins, actual C Staging and CAS bytes remain the
    /// physical driver's separate responsibility before and after effects.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection) fn validate_export_protection(
        &self,
        family: &FamilyAccess,
        c: &Connection,
        intent: &ProviderTransitionIntentRecord,
        target: &crate::projection::shared::admitted_catalog::ExportReserveOnlyTarget,
        required: &[layrs_core::scm::FileManifest],
        protection: &crate::projection::shared::exact_protection::ExactProtection,
        cancel: Option<&Cancellation>,
    ) -> Result<crate::projection::shared::admitted_catalog::ExportProtectionState> {
        self.require_export_target(family, intent, target, cancel)?;
        let (_, state) = self.retained.validate_export_protection(
            c,
            family.marker_bytes(),
            target,
            required,
            protection,
            cancel,
        )?;
        family.verify()?;
        cancellation::check(cancel)?;
        Ok(state)
    }

    /// Full publication DATA interpretation under actual B Staging and the
    /// pinned physical family. Actual complete C Ready and its retained pins
    /// remain the publication driver's separate, simultaneous evidence.
    pub(in crate::projection) fn read_export_publication(
        &self,
        family: &FamilyAccess,
        c: &Connection,
        intent: &ProviderTransitionIntentRecord,
        inputs: &crate::projection::shared::admitted_catalog::ExportPublicationInputs<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<crate::projection::shared::admitted_catalog::ExportPublicationState> {
        self.require_export_target(family, intent, inputs.target, cancel)?;
        let (_, state) =
            self.retained
                .read_export_publication(c, family.marker_bytes(), inputs, cancel)?;
        family.verify()?;
        cancellation::check(cancel)?;
        Ok(state)
    }

    /// Phase-aware source-publication read for the same retained owner/current
    /// frontier. The actual B row may be Staging, Ready or Published; original
    /// Staging DATA is never presented as stored B. This is not the historical
    /// Published handoff reader after B advances. Complete C Ready and physical
    /// C/CAS/checkout pins remain independent simultaneous driver evidence.
    pub(in crate::projection) fn read_export_source_publication(
        &self,
        family: &FamilyAccess,
        c: &Connection,
        source: &OrdinaryPublicationSource<'_>,
        inputs: &crate::projection::shared::admitted_catalog::ExportPublicationInputs<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<crate::projection::shared::admitted_catalog::ExportPublicationState> {
        if !std::ptr::eq(self.connection, source.connection())
            || self.marker != source.marker()
            || !std::ptr::eq(self.closure, source.closure())
            || inputs.destination_ready != source.destination_ready()
            || inputs.receipt != source.receipt()
            || std::ptr::eq(self.connection, c)
        {
            return Err(invalid(
                "ordinary source publication source/Ready binding differs",
            ));
        }
        let actual = source.revalidate(cancel)?;
        Self::require_export_target_binding(&actual, inputs.target)?;
        // The driver supplies a fresh proof after every B CAS. require_private
        // reinterprets it in this snapshot and rejects a stale outgoing row;
        // shared code never bypasses the provider-private admission entry.
        self.require_family_identity(family, &actual, cancel)?;
        let (_, state) =
            self.retained
                .read_export_publication(c, family.marker_bytes(), inputs, cancel)?;
        if state.member
            != crate::projection::shared::admitted_catalog::ExportPublicationMember::Ready
        {
            return Err(invalid(
                "ordinary source publication requires actual family Ready",
            ));
        }
        family.verify()?;
        cancellation::check(cancel)?;
        Ok(state)
    }

    fn require_export_target(
        &self,
        family: &FamilyAccess,
        intent: &ProviderTransitionIntentRecord,
        target: &crate::projection::shared::admitted_catalog::ExportReserveOnlyTarget,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if intent.state != TransitionState::Staging {
            return Err(invalid("ordinary export source must be actual Staging"));
        }
        Self::require_export_target_binding(intent, target)?;
        self.require_family_identity(family, intent, cancel)
    }

    fn require_export_target_binding(
        intent: &ProviderTransitionIntentRecord,
        target: &crate::projection::shared::admitted_catalog::ExportReserveOnlyTarget,
    ) -> Result<()> {
        let proof = intent
            .destination
            .as_ref()
            .ok_or_else(|| invalid("ordinary export destination proof absent"))?;
        let target_binding = target.binding();
        if target_binding.transaction_id != intent.transaction_id
            || target_binding.plan_digest != intent.plan.digest()?
            || target_binding.prefix_id != intent.plan.source_prefix_id
            || target_binding.destination_proof_digest != proof.digest(&intent.plan)?
            || target_binding.owner != proof.format.identity.worktree_id
            || target_binding.database_identity != proof.private_database_identity
            || target_binding.quota_bytes != intent.plan.quota_bytes
            || intent.reservation.as_deref() != Some(target_binding.reservation.as_str())
        {
            return Err(invalid(
                "ordinary export target differs from actual source intent",
            ));
        }
        Ok(())
    }

    fn require_family_identity(
        &self,
        family: &FamilyAccess,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.require_private(intent, cancel)?;
        family.verify()?;
        let shared = self
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary import source shared FORMAT absent"))?;
        let binding = self.retained.binding();
        if shared.root != family.root()
            || shared.incarnation != family.family.marker.incarnation
            || shared.root_identity != family.root_pin.identity_token()
            || shared.metadata_path != family.member(&self.marker.identity.worktree_id)
            || shared.database_identity != binding.database_identity
            || binding.owner != self.marker.identity.worktree_id
            || binding.family_incarnation != family.family.marker.incarnation
            || binding.family_root_identity != family.root_pin.identity_token()
            || binding.family_catalog_identity != family.family.marker.database_identity
            || binding.family_marker_digest != family.marker_digest()
            || family.writer_identity(&self.marker.identity.worktree_id) != self.marker.identity
        {
            return Err(invalid("ordinary import source differs from pinned family"));
        }
        family.verify()?;
        cancellation::check(cancel)
    }
}

impl<'p, 'db> ImportSourceInputs<'p, 'db> {
    pub(super) fn provider(
        allocation: &'p AllocationRecord,
        prepared: &'p PreparedRecord,
        activation: &'p ProviderActivationRecord,
        capture: &'p PrivateCaptureProof,
    ) -> Self {
        Self::Provider(ProviderSourceInputs {
            allocation,
            prepared,
            activation,
            capture,
            source_context: None,
        })
    }

    pub(super) fn provider_with_companions(
        allocation: &'p AllocationRecord,
        prepared: &'p PreparedRecord,
        activation: &'p ProviderActivationRecord,
        capture: &'p PrivateCaptureProof,
        source_context: Option<
            &'p crate::projection::shared::source_companion::SourceCompanionFamilyContext<'p>,
        >,
    ) -> Self {
        Self::Provider(ProviderSourceInputs {
            allocation,
            prepared,
            activation,
            capture,
            source_context,
        })
    }
    pub(super) fn require_family(
        &self,
        family: &FamilyAccess,
        c: &Connection,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<ValidatedSource<'_>> {
        cancellation::check(cancel)?;
        if c.is_autocommit() {
            return Err(invalid("import family read requires retained transaction"));
        }
        let marker = match self {
            Self::Provider(source) => {
                family.require_allocation(source.allocation)?;
                let phase = if let Some(context) = source.source_context {
                    if !std::ptr::eq(source.capture, context.publication.capture_data()) {
                        return Err(invalid(
                            "source family context discarded the complete private proof",
                        ));
                    }
                    family.validate_source_companion_catalog(
                        c,
                        source.allocation,
                        context,
                        cancel,
                    )?
                } else {
                    family.validate_capture_catalog(c, source.allocation, source.capture, cancel)?
                };
                if phase != ActivationRegistration::Ready {
                    return Err(invalid("import source family is not Ready"));
                }
                intent
                    .plan
                    .source_prefix
                    .require_references(ImportSourceReferences::Provider {
                        allocation: source.allocation,
                        prepared: source.prepared,
                        activation: source.activation,
                    })?;
                let ImportSource::Provider { version, .. } = &intent.plan.source_prefix.source
                else {
                    return Err(invalid("import family source prefix is not provider"));
                };
                if version.capture_count != source.capture.patches.len() as u64
                    || version.capture_head.as_deref()
                        != source
                            .capture
                            .patches
                            .last()
                            .map(|patch| patch.digest.as_str())
                {
                    return Err(invalid(
                        "import source capture head differs from family proof",
                    ));
                }
                &source.allocation.format
            }
            Self::Ordinary(source) => {
                source.require_family(family, c, intent, cancel)?;
                source.marker
            }
        };
        cancellation::check(cancel)?;
        Ok(ValidatedSource { marker })
    }
}
