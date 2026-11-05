//! Separate Ordinary private Ready DATA and actual-B-backed complete family
//! publication interpretation. Neither result owns SQL or physical authority.
use super::*;
use crate::projection::shared::{
    admitted_catalog::ExportPublicationState, exact_protection::ExactProtection,
};

/// Provisional detached private SQL DATA for image/plan reconstruction only.
/// This has no complete-family, byte, admission or publication-capacity proof,
/// and deliberately contains no ExactInstallState or VerifiedManagedReady.
pub(in crate::projection::provider) struct OrdinaryPrivateReadySnapshot {
    marker: format::Marker,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    family: family_read::FamilyData,
    metadata_floor: u64,
    member: ExportPublicationMember,
}

impl OrdinaryPrivateReadySnapshot {
    pub(in crate::projection::provider) fn marker(&self) -> &format::Marker {
        &self.marker
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(in crate::projection::provider) fn expected(&self) -> &ExpectedCheckout {
        &self.expected
    }
    pub(in crate::projection::provider) fn journal(&self) -> &CheckoutJournal {
        &self.journal
    }
    pub(in crate::projection::provider) fn required(&self) -> &[FileManifest] {
        &self.family.required
    }
    pub(in crate::projection::provider) fn binding(&self) -> &ExactBinding {
        &self.family.binding
    }
    pub(in crate::projection::provider) fn protection(&self) -> &ExactProtection {
        &self.family.protection
    }
    pub(in crate::projection::provider) fn family_marker_bytes(&self) -> &[u8] {
        &self.family.marker_bytes
    }
    pub(in crate::projection::provider) fn metadata_floor(&self) -> u64 {
        self.metadata_floor
    }
    pub(in crate::projection::provider) fn member(&self) -> ExportPublicationMember {
        self.member
    }
    pub(in crate::projection::provider) fn require_same(&self, current: &Self) -> Result<()> {
        require_same_expected(&self.expected, &current.expected)?;
        if self.marker != current.marker
            || self.ready != current.ready
            || self.receipt != current.receipt
            || self.journal.rows() != current.journal.rows()
            || self.family.marker_bytes != current.family.marker_bytes
            || self.family.required != current.family.required
            || self.family.binding != current.family.binding
            || self.family.protection != current.family.protection
            || self.metadata_floor != current.metadata_floor
            || self.member != current.member
        {
            return Err(invalid("detached Ordinary private Ready DATA changed"));
        }
        Ok(())
    }
}

fn require_same_expected(actual: &ExpectedCheckout, expected: &ExpectedCheckout) -> Result<()> {
    if actual.marker() != expected.marker()
        || actual.header() != expected.header()
        || actual.operation() != expected.operation()
        || actual.files() != expected.files()
        || actual.directories() != expected.directories()
        || actual.deletions() != expected.deletions()
        || actual.declared_encoded_bytes() != expected.declared_encoded_bytes()
        || actual.maximum_encoded_bytes() != expected.maximum_encoded_bytes()
    {
        return Err(invalid(
            "Ordinary private Ready exact checkout DATA differs",
        ));
    }
    Ok(())
}

fn require_same_required(
    actual: &[FileManifest],
    supplied: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    cancellation::check(cancel)?;
    if actual.len() != supplied.len() {
        return Err(invalid(
            "Ordinary family publication exact required union differs",
        ));
    }
    let mut ordered: Vec<_> = supplied.iter().collect();
    ordered.sort_unstable_by(|left, right| left.id().cmp(right.id()));
    if ordered.windows(2).any(|pair| pair[0].id() == pair[1].id()) {
        return Err(invalid(
            "Ordinary family publication exact required union differs",
        ));
    }
    for (supplied, actual) in ordered.into_iter().zip(actual) {
        cancellation::check(cancel)?;
        if supplied != actual {
            return Err(invalid(
                "Ordinary family publication exact required union differs",
            ));
        }
    }
    cancellation::check(cancel)
}

fn read_parts_at_observed_member<'d>(
    destination: &'d Connection,
    actual_marker: &format::Marker,
    family: &Connection,
    cancel: Option<&Cancellation>,
) -> Result<Box<PrivateReadyParts<'d>>> {
    let member = family_read::read_member(family, actual_marker, cancel)?;
    read_private_ready_parts(
        destination,
        actual_marker,
        family,
        DestinationOriginKind::Ordinary,
        member,
        cancel,
    )
}

fn detach_private(parts: Box<PrivateReadyParts<'_>>) -> Result<Box<OrdinaryPrivateReadySnapshot>> {
    let ready = ProviderTransitionIntentRecord::decode(&parts.prefix.ready().encode()?)?;
    let receipt = ProviderTransitionReceiptRecord::decode(&parts.prefix.receipt().encode()?)?;
    Ok(Box::new(OrdinaryPrivateReadySnapshot {
        marker: parts.marker,
        ready,
        receipt,
        expected: parts.expected,
        journal: parts.journal,
        family: parts.family,
        metadata_floor: parts.metadata_floor,
        member: parts.member,
    }))
}

/// A provisional private read only. The exact actual member phase is observed
/// internally and required unchanged through the interpretation. No caller can
/// nominate a phase or obtain installation/capacity from this entry.
pub(in crate::projection::provider) fn read_ordinary_private_ready(
    destination: &Connection,
    actual_marker: &format::Marker,
    family: &Connection,
    cancel: Option<&Cancellation>,
) -> Result<Box<OrdinaryPrivateReadySnapshot>> {
    detach_private(read_parts_at_observed_member(
        destination,
        actual_marker,
        family,
        cancel,
    )?)
}

/// Complete actual B/C/family SQL DATA at one exact observed member phase.
/// Physical pins, owner scopes, mutation, COMMIT and positive closes belong to
/// the driver. This is distinct from every terminal Ready/admission type.
pub(in crate::projection::provider) struct VerifiedOrdinaryFamilyPublication {
    snapshot: Box<OrdinaryPrivateReadySnapshot>,
    state: ExportPublicationState,
}
impl VerifiedOrdinaryFamilyPublication {
    pub(in crate::projection::provider) fn snapshot(&self) -> &OrdinaryPrivateReadySnapshot {
        &self.snapshot
    }
    pub(in crate::projection::provider) fn state(&self) -> ExportPublicationState {
        self.state
    }
    pub(in crate::projection::provider) fn require_same(&self, current: &Self) -> Result<()> {
        self.snapshot.require_same(&current.snapshot)?;
        if self.state != current.state {
            return Err(invalid("Ordinary family publication accounting changed"));
        }
        Ok(())
    }
    /// Only the complete actual-B/full-family path can detach installation
    /// DATA. This creates no terminal reader, physical holder or admission.
    pub(in crate::projection::provider) fn into_snapshot(
        self: Box<Self>,
    ) -> Box<OwnedReadySnapshot> {
        let snapshot = self.snapshot;
        Box::new(OwnedReadySnapshot {
            marker: snapshot.marker,
            ready: snapshot.ready,
            receipt: snapshot.receipt,
            expected: snapshot.expected,
            journal: snapshot.journal,
            required: snapshot.family.required,
            binding: snapshot.family.binding,
            installation: self.state.protection.installation,
            metadata_floor: snapshot.metadata_floor,
        })
    }
}

pub(in crate::projection::provider) fn read_ordinary_family_publication(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<Box<VerifiedOrdinaryFamilyPublication>> {
    cancellation::check(cancel)?;
    let parts = read_parts_at_observed_member(destination, inputs.marker, family, cancel)?;
    require_same_expected(&parts.expected, expected)?;
    require_same_required(&parts.family.required, inputs.data.required, cancel)?;
    if parts.prefix.ready() != inputs.data.ready
        || parts.prefix.receipt() != inputs.data.receipt
        || &parts.family.binding != inputs.target.binding()
        || &parts.family.protection != inputs.protection
        || parts.family.marker_bytes != inputs.family_marker_bytes
        || parts.metadata_floor != inputs.metadata_floor
    {
        return Err(invalid(
            "Ordinary family publication supplied Ready DATA differs",
        ));
    }
    // Reuse the actual-B reader and its complete catalog policy: known B empty
    // roots, only the exact C root deferred, shared debit sets, all reservations.
    // Do not use or modify the autonomous terminal catalog walker here.
    super::super::ordinary::read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        cancel,
    )?;
    let state = inputs
        .retained
        .read_export_publication(
            family,
            inputs.family_marker_bytes,
            &inputs.publication_inputs(expected),
            cancel,
        )?
        .1;
    if state.member != parts.member {
        return Err(invalid(
            "Ordinary family publication actual member phase changed",
        ));
    }
    // This actual complete-family installation alone funds the final retained
    // envelope and the complete future C10 witness/archive peak.
    parts.require_installation(state.protection.installation, cancel)?;
    family_read::require_member(family, inputs.marker, parts.member)?;
    cancellation::check(cancel)?;
    Ok(Box::new(VerifiedOrdinaryFamilyPublication {
        snapshot: detach_private(parts)?,
        state,
    }))
}
