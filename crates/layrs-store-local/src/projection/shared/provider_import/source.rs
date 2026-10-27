//! Concrete source interpreters for the common destination registry kernel.
//! Borrowed private SQL evidence never grants physical source bytes or C pins.
use super::*;
use crate::projection::{
    format,
    provider::{
        admitted_ordinary, import_validation::VerifiedImportClosure,
        managed_admission::VerifiedAdmittedImport, records::import::AdmittedSourceBinding,
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
            private_schema: 9, ..
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
        // This verifies B's actual Ready registry membership, retained exact
        // import group and protections. It intentionally rejects active C
        // reservations until a separate export-scoped policy is implemented.
        self.retained
            .validate_catalog(c, family.marker_bytes(), cancel)?;
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
                if family.validate_capture_catalog(c, source.allocation, source.capture, cancel)?
                    != ActivationRegistration::Ready
                {
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
