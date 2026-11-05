//! Complete terminal managed Ready SQL DATA from the actual destination and
//! family. No source connection, original source owner or physical byte proof.
use super::super::{
    checkout_journal::{CheckoutJournal, ExpectedCheckout},
    import_validation::{
        DestinationOriginKind, VerifiedDestinationPrefix, read_destination_prefix,
    },
    managed_admission::RequiredRepresentation,
    managed_budget,
};
use super::*;
use crate::projection::shared::admitted_catalog::ExportPublicationMember;
use crate::projection::shared::exact_content::{self, ExactBinding, ExactInstallState};

mod family_read;
mod ordinary_family_publication;
pub(in crate::projection::provider) use ordinary_family_publication::{
    OrdinaryPrivateReadySnapshot, VerifiedOrdinaryFamilyPublication,
    read_ordinary_family_publication, read_ordinary_private_ready,
};
pub(super) mod inventory;
#[cfg(test)]
mod tests;

pub(in crate::projection::provider) struct VerifiedManagedReady<'d, 'f> {
    prefix: VerifiedDestinationPrefix<'d>,
    family_connection: &'f Connection,
    marker: format::Marker,
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    required: Vec<FileManifest>,
    binding: ExactBinding,
    installation: ExactInstallState,
    // Actual complete Ready floor before checkout journal bytes. Retained
    // solely for autonomous source-publication capacity derivation.
    metadata_floor: u64,
}

/// Detached bounded SQL DATA. This owns no transaction, filesystem object or
/// byte authority. A physical consumer must re-read and compare it after I/O.
pub(in crate::projection::provider) struct OwnedReadySnapshot {
    pub(in crate::projection::provider) marker: format::Marker,
    pub(in crate::projection::provider) ready: ProviderTransitionIntentRecord,
    pub(in crate::projection::provider) receipt: ProviderTransitionReceiptRecord,
    pub(in crate::projection::provider) expected: ExpectedCheckout,
    pub(in crate::projection::provider) journal: CheckoutJournal,
    pub(in crate::projection::provider) required: Vec<FileManifest>,
    pub(in crate::projection::provider) binding: ExactBinding,
    pub(in crate::projection::provider) installation: ExactInstallState,
    /// Complete Ready metadata floor before checkout journal bytes. SQL DATA only.
    pub(in crate::projection::provider) metadata_floor: u64,
}
impl OwnedReadySnapshot {
    pub(in crate::projection::provider) fn require_same(&self, current: &Self) -> Result<()> {
        if self.marker != current.marker
            || self.ready != current.ready
            || self.receipt != current.receipt
            || self.expected.header() != current.expected.header()
            || self.journal.rows() != current.journal.rows()
            || self.required != current.required
            || self.binding != current.binding
            || self.installation != current.installation
            || self.metadata_floor != current.metadata_floor
        {
            return Err(invalid("detached managed Ready SQL snapshot changed"));
        }
        Ok(())
    }
}
impl VerifiedManagedReady<'_, '_> {
    /// Capacity DATA derived solely from this actual complete Ready snapshot:
    /// true floor + Jmax, net of actual checkout bytes. No nominated quota or
    /// caller-provided floor can manufacture publication funding.
    pub(in crate::projection::provider) fn publication_capacity(
        &self,
        cancel: Option<&Cancellation>,
    ) -> Result<super::super::ordinary_transition::PublicationCapacity> {
        cancellation::check(cancel)?;
        if self.origin() != DestinationOriginKind::Ordinary {
            return Err(invalid(
                "autonomous publication requires actual Ordinary Ready",
            ));
        }
        let remaining = self
            .installation
            .remaining
            .checked_sub(self.expected.header().file_bytes)
            .ok_or_else(|| invalid("autonomous publication checkout envelope insufficient"))?;
        let floor = self
            .metadata_floor
            .checked_add(self.expected.maximum_encoded_bytes())
            .ok_or_else(|| invalid("autonomous publication Ready metadata overflow"))?;
        let capacity = super::super::ordinary_transition::PublicationCapacity::new(
            self.ready(),
            self.receipt(),
            floor,
            remaining,
        )?;
        let protection = crate::projection::shared::exact_protection::ExactProtection::new(
            &self.binding.family_incarnation,
            &self.binding.owner,
            &self.binding.database_identity,
            &self.binding.transaction_id,
            &self.required,
            cancel,
        )?;
        protection.require_envelope(remaining, capacity.metadata_floor())?;
        Ok(capacity)
    }
    /// Move large inventories out of the retained SQL interpretation; only the
    /// two bounded canonical records are reconstructed. No authority escapes.
    pub(in crate::projection::provider) fn into_snapshot(self) -> Result<OwnedReadySnapshot> {
        let ready = ProviderTransitionIntentRecord::decode(&self.ready().encode()?)?;
        let receipt = ProviderTransitionReceiptRecord::decode(&self.receipt().encode()?)?;
        Ok(OwnedReadySnapshot {
            marker: self.marker,
            ready,
            receipt,
            expected: self.expected,
            journal: self.journal,
            required: self.required,
            binding: self.binding,
            installation: self.installation,
            metadata_floor: self.metadata_floor,
        })
    }
    /// These getters expose interpreted DATA, not physical authority.
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        self.prefix.receipt()
    }
    pub(in crate::projection::provider) fn origin(&self) -> DestinationOriginKind {
        self.prefix.origin()
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        self.prefix.ready()
    }
    pub(in crate::projection::provider) fn expected(&self) -> &ExpectedCheckout {
        &self.expected
    }
    pub(in crate::projection::provider) fn journal(&self) -> &CheckoutJournal {
        &self.journal
    }
    pub(in crate::projection::provider) fn required(&self) -> &[FileManifest] {
        &self.required
    }
    pub(in crate::projection::provider) fn installation(&self) -> ExactInstallState {
        self.installation
    }
    /// A Connection borrow does not prevent COMMIT/restart through &Connection.
    /// Every validating use therefore reinterprets both complete SQL snapshots.
    pub(in crate::projection::provider) fn require(
        &self,
        c: &Connection,
        family_c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.prefix.require_destination_connection(c)?;
        if family_c.is_autocommit() || !std::ptr::eq(family_c, self.family_connection) {
            return Err(invalid(
                "autonomous Ready family transaction differs or ended",
            ));
        }
        let fresh = read_ready_origin(c, &self.marker, family_c, self.origin(), cancel)?;
        if fresh.receipt() != self.receipt()
            || fresh.ready() != self.ready()
            || fresh.expected.header() != self.expected.header()
            || fresh.journal.rows() != self.journal.rows()
            || fresh.required != self.required
            || fresh.binding != self.binding
            || fresh.installation != self.installation
        {
            return Err(invalid("autonomous Ready retained DATA changed"));
        }
        Ok(())
    }
}

/// Both callers retain transactions and independently authenticate/pin FORMAT,
/// private DB and family. This interpreter never opens, commits or repairs.
pub(in crate::projection::provider) fn read_managed_ready<'d, 'f>(
    c: &'d Connection,
    actual_marker: &format::Marker,
    family_c: &'f Connection,
    cancel: Option<&Cancellation>,
) -> Result<VerifiedManagedReady<'d, 'f>> {
    read_ready_origin(
        c,
        actual_marker,
        family_c,
        DestinationOriginKind::Provider,
        cancel,
    )
}

/// Ordinary C8 Ready from copied immutable history and the complete real
/// family union. No B connection or historical B base mapping is consulted.
pub(in crate::projection::provider) fn read_ordinary_managed_ready<'d, 'f>(
    c: &'d Connection,
    actual_marker: &format::Marker,
    family_c: &'f Connection,
    cancel: Option<&Cancellation>,
) -> Result<VerifiedManagedReady<'d, 'f>> {
    read_ready_origin(
        c,
        actual_marker,
        family_c,
        DestinationOriginKind::Ordinary,
        cancel,
    )
}

fn read_ready_origin<'d, 'f>(
    c: &'d Connection,
    actual_marker: &format::Marker,
    family_c: &'f Connection,
    origin: DestinationOriginKind,
    cancel: Option<&Cancellation>,
) -> Result<VerifiedManagedReady<'d, 'f>> {
    let parts = read_private_ready_parts(
        c,
        actual_marker,
        family_c,
        origin,
        ExportPublicationMember::Ready,
        cancel,
    )?;
    // Terminal entry points always retain the original complete family gate.
    let installation = match origin {
        DestinationOriginKind::Provider => exact_content::validate_protected_installations(
            family_c,
            &parts.family.binding,
            &parts.family.required,
            &parts.family.protection,
            cancel,
        )?,
        DestinationOriginKind::Ordinary => {
            crate::projection::shared::admitted_catalog::validate_terminal_ready_catalog(
                family_c,
                &parts.family.marker_bytes,
                actual_marker,
                &parts.family.binding,
                &parts.family.required,
                &parts.family.protection,
                cancel,
            )?
        }
    };
    parts.require_installation(installation, cancel)?;
    cancellation::check(cancel)?;
    Ok(VerifiedManagedReady {
        prefix: parts.prefix,
        family_connection: family_c,
        marker: parts.marker,
        expected: parts.expected,
        journal: parts.journal,
        required: parts.family.required,
        binding: parts.family.binding,
        installation,
        metadata_floor: parts.metadata_floor,
    })
}

/// Internal private SQL interpretation. No complete family walk, installation
/// state, physical bytes, admission or publication capacity is inferred here.
/// Keep this large summary boxed through all nested readers.
struct PrivateReadyParts<'d> {
    prefix: VerifiedDestinationPrefix<'d>,
    marker: format::Marker,
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    family: family_read::FamilyData,
    metadata_floor: u64,
    member: ExportPublicationMember,
}

impl PrivateReadyParts<'_> {
    /// Only a complete family validator may supply this actual installation.
    fn require_installation(
        &self,
        installation: ExactInstallState,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        let retained = self
            .metadata_floor
            .checked_add(self.expected.maximum_encoded_bytes())
            .ok_or_else(|| invalid("Ready retained metadata envelope overflow"))?;
        let remaining = installation
            .remaining
            .checked_sub(self.expected.header().file_bytes)
            .ok_or_else(|| invalid("Ready retained checkout envelope insufficient"))?;
        self.family
            .protection
            .require_envelope(remaining, retained)?;
        if self.prefix.origin() == DestinationOriginKind::Ordinary {
            let budget = managed_budget::ordinary_admission_budget(
                self.metadata_floor,
                self.expected.files().len() as u64,
                self.expected.directories().len() as u64,
                self.family.required.len() as u64,
            )?;
            budget.require_remaining(
                remaining,
                self.family.protection.payload().len() as u64,
                self.family.required.len() as u64,
            )?;
        }
        cancellation::check(cancel)
    }
}

fn read_private_ready_parts<'d>(
    c: &'d Connection,
    actual_marker: &format::Marker,
    family_c: &Connection,
    origin: DestinationOriginKind,
    member: ExportPublicationMember,
    cancel: Option<&Cancellation>,
) -> Result<Box<PrivateReadyParts<'d>>> {
    cancellation::check(cancel)?;
    if c.is_autocommit() || family_c.is_autocommit() || std::ptr::eq(c, family_c) {
        return Err(invalid(
            "autonomous Ready requires distinct retained transactions",
        ));
    }
    let prefix = read_destination_prefix(c, actual_marker, cancel)?;
    if prefix.origin() != origin {
        return Err(invalid(
            "autonomous Ready origin differs from closed reader",
        ));
    }
    let ready = prefix.ready();
    let plan = &ready.plan;
    let version = prefix.prefix().source.version();
    if version.generation.checked_add(1) != Some(plan.observation.generation)
        || plan.observation.generation.checked_add(1) != Some(plan.placement_generation)
        || version.capture_count.checked_add(1) != Some(plan.capture_sequence)
        || (origin != DestinationOriginKind::Provider
            && (plan.companion_id.is_some()
                || plan.companion_files.count != 0
                || plan.companion_deletions.count != 0))
    {
        return Err(invalid("autonomous Ready local boundaries differ"));
    }
    // The prefix's conservative scalar preflight also covered these local
    // capture/snapshot/selection columns before their existing loader runs.
    let capture = crate::projection::capture::storage::load_metadata_for_origin(
        c,
        plan.capture_sequence,
        &actual_marker.identity,
        plan.placement_generation,
    )?;
    let deletions: Vec<_> = capture
        .changes
        .iter()
        .filter(|change| change.replacement.is_none())
        .map(|change| change.path.clone())
        .collect();
    let (expected, journal) = match origin {
        DestinationOriginKind::Provider => {
            ExpectedCheckout::read_ready(c, ready, &deletions, cancel)?
        }
        DestinationOriginKind::Ordinary => {
            ExpectedCheckout::read_ready_ordinary(c, ready, &deletions, cancel)?
        }
    };
    let claim = family_read::read_claim(c, ready)?;
    let family = match member {
        ExportPublicationMember::Ready => {
            family_read::read(family_c, actual_marker, ready, &claim, cancel)?
        }
        ExportPublicationMember::Initializing => {
            if origin != DestinationOriginKind::Ordinary {
                return Err(invalid(
                    "provisional private Ready requires Ordinary origin",
                ));
            }
            family_read::read_at_member(family_c, actual_marker, ready, &claim, member, cancel)?
        }
    };
    let copied = prefix.copied_content_mappings(c, cancel)?;
    let retained = if plan.companion_id.is_some() {
        Some(super::retained_companion::RetainedCompanionReadyData::read(
            c,
            actual_marker,
            &prefix,
            &family.required,
            &expected,
            cancel,
        )?)
    } else {
        None
    };
    let retained_denial = if plan.denial_coverage.is_some() && plan.companion_id.is_none() {
        Some(super::denial::RetainedDenialReadyData::read(
            c,
            &prefix,
            &family.required,
            &expected,
            cancel,
        )?)
    } else {
        None
    };

    let mut inherited = prefix.inherited_companion_representations(c, cancel)?;
    if let Some(local) = &retained {
        inherited.extend(local.header().preserved_representations.iter().map(|r| {
            RequiredRepresentation {
                file_id: r.file_id.clone(),
                manifest_id: r.manifest_id.clone(),
                bytes: r.bytes,
            }
        }));
    }
    let lengths = match origin {
        DestinationOriginKind::Provider => {
            inventory::require_union(
                prefix.source()?,
                &copied,
                &expected,
                &inherited,
                &family.required,
                cancel,
            )?;
            BTreeMap::new()
        }
        DestinationOriginKind::Ordinary => {
            let mut rows: Vec<_> = family
                .required
                .iter()
                .map(|manifest| RequiredRepresentation {
                    file_id: manifest.file_id().clone(),
                    manifest_id: manifest.id().clone(),
                    bytes: manifest.byte_length(),
                })
                .collect();
            // Family roots sort by ManifestId; this commitment uses FileId first.
            // Preserve root order and sort only the derived triple inventory.
            rows.sort_unstable();
            inventory::require_ordinary_union(
                prefix.ordinary_frozen()?,
                &copied,
                &expected,
                &inherited,
                &rows,
                cancel,
            )?
        }
    };
    let overlay = inventory::overlay(&capture, &expected, cancel)?;
    if retained.is_none() && retained_denial.is_none() {
        inventory::require_image(&capture, &expected, &version.effective_image_digest, cancel)?;
    }
    let (observation, placement) = match origin {
        DestinationOriginKind::Provider => match &retained {
            Some(rows) => (rows.observation().clone(), rows.placement().clone()),
            None => match &retained_denial {
                Some(rows) => (rows.observation().clone(), rows.placement().clone()),
                None => (
                    prefix.source()?.initial().clone(),
                    super::super::records::activation::activation_snapshot(prefix.source()?)?,
                ),
            },
        },
        DestinationOriginKind::Ordinary => (
            prefix.ordinary_frozen()?.snapshot().clone(),
            prefix.ordinary_frozen()?.snapshot().clone(),
        ),
    };
    let claims = [claim];
    let data = ManagedReadyData {
        observation: &observation,
        placement: &placement,
        capture: &capture,
        overlay: &overlay,
        required: &family.required,
        receipt: prefix.receipt(),
        ready,
        claims: &claims,
    };
    let view = ReadyView {
        marker: actual_marker,
        source: match origin {
            DestinationOriginKind::Provider => ReadySource::Provider(prefix.source()?),
            DestinationOriginKind::Ordinary => ReadySource::OrdinaryFrozen {
                source: prefix.ordinary_frozen()?,
                lengths: &lengths,
            },
        },
        prefix: prefix.prefix(),
        data: &data,
        companion: None,
        retained_companion: retained.as_ref(),
        denial: None,
        retained_denial: retained_denial.as_ref(),
    };
    let prepared = super::data::prepare_suffix(&view, &copied, cancel)?;
    super::read::ready_suffix(c, &view, &prepared, Some(&expected), cancel)?;
    super::family::require_protection(
        family_c,
        actual_marker,
        &ready.transaction_id,
        &family.required,
        &claims,
        cancel,
    )?;
    let (source_size, destination_size) = managed_budget::original_staging_sizes(ready)?;
    let coverage_cost = super::super::destination_coverage::read_current(c, ready, cancel)?
        .as_ref()
        .map(|body| managed_budget::DeniedCoverageCost::new(ready, body, cancel))
        .transpose()?;
    let base = managed_budget::base_metadata_floor_with_coverage(
        prefix.metadata_bytes(c, cancel)?,
        &observation,
        &placement,
        &capture,
        plan,
        view.selection(),
        source_size,
        destination_size,
        coverage_cost.as_ref(),
    )?;
    let baseline = managed_budget::quota(
        &family.required,
        expected.files().iter().map(|file| file.bytes),
        0,
    )?;
    let metadata = plan
        .quota_bytes
        .checked_sub(baseline)
        .filter(|n| *n <= 1073741824)
        .ok_or_else(|| invalid("managed reserved metadata envelope invalid"))?;
    if metadata < base {
        return Err(invalid(
            "managed reservation metadata envelope insufficient",
        ));
    }
    let retained_floor = match &retained {
        Some(rows) => managed_budget::add_metadata(base, rows.metadata_bytes()?)?,
        None => base,
    };
    let floor = managed_budget::complete_metadata_floor(
        retained_floor,
        ready,
        prefix.receipt(),
        &claims[0],
        &family.required,
        cancel,
    )?;
    if origin == DestinationOriginKind::Ordinary {
        // C8 must already fund its complete future C10 witness and archive
        // peak. Durable quotas remain immutable; a cheap current read cannot
        // borrow the separate family protection margin as private capacity.
        let budget = managed_budget::ordinary_admission_budget(
            floor,
            expected.files().len() as u64,
            expected.directories().len() as u64,
            family.required.len() as u64,
        )?;
        budget.require_quota(plan.quota_bytes, baseline)?;
    }
    cancellation::check(cancel)?;
    family_read::require_member(family_c, actual_marker, member)?;
    Ok(Box::new(PrivateReadyParts {
        prefix,
        marker: actual_marker.clone(),
        expected,
        journal,
        family,
        metadata_floor: floor,
        member,
    }))
}
