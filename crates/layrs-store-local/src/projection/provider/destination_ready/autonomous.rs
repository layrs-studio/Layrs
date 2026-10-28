//! Complete terminal managed Ready SQL DATA from the actual destination and
//! family. No source connection, original source owner or physical byte proof.
use super::super::{
    checkout_journal::{CheckoutJournal, ExpectedCheckout},
    import_validation::{
        read_destination_prefix, DestinationOriginKind, VerifiedDestinationPrefix,
    },
    managed_admission::RequiredRepresentation,
    managed_budget,
};
use super::*;
use crate::projection::shared::exact_content::{self, ExactBinding, ExactInstallState};

mod family_read;
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
        || plan.companion_id.is_some()
        || plan.companion_files.count != 0
        || plan.companion_deletions.count != 0
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
    let family = family_read::read(family_c, actual_marker, ready, &claim, cancel)?;
    let copied = prefix.copied_content_mappings(c, cancel)?;
    let lengths = match origin {
        DestinationOriginKind::Provider => {
            inventory::require_union(
                prefix.source()?,
                &copied,
                &expected,
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
                &rows,
                cancel,
            )?
        }
    };
    let overlay = inventory::overlay(&capture, &expected, cancel)?;
    inventory::require_image(&capture, &expected, &version.effective_image_digest, cancel)?;
    let (observation, placement) = match origin {
        DestinationOriginKind::Provider => (
            prefix.source()?.initial().clone(),
            super::super::records::activation::activation_snapshot(prefix.source()?)?,
        ),
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
    let installation = match origin {
        DestinationOriginKind::Provider => exact_content::validate_protected_installations(
            family_c,
            &family.binding,
            &family.required,
            &family.protection,
            cancel,
        )?,
        DestinationOriginKind::Ordinary => {
            crate::projection::shared::admitted_catalog::validate_terminal_ready_catalog(
                family_c,
                &family.marker_bytes,
                actual_marker,
                &family.binding,
                &family.required,
                &family.protection,
                cancel,
            )?
        }
    };
    let (source_size, destination_size) = managed_budget::original_staging_sizes(ready)?;
    let base = managed_budget::base_metadata_floor(
        prefix.metadata_bytes(c, cancel)?,
        &observation,
        &placement,
        &capture,
        plan,
        view.source.selection(),
        source_size,
        destination_size,
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
    let floor = managed_budget::complete_metadata_floor(
        base,
        ready,
        prefix.receipt(),
        &claims[0],
        &family.required,
        cancel,
    )?;
    let retained = floor
        .checked_add(expected.maximum_encoded_bytes())
        .ok_or_else(|| invalid("Ready retained metadata envelope overflow"))?;
    let remaining = installation
        .remaining
        .checked_sub(expected.header().file_bytes)
        .ok_or_else(|| invalid("Ready retained checkout envelope insufficient"))?;
    family.protection.require_envelope(remaining, retained)?;
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
        budget.require_remaining(
            remaining,
            family.protection.payload().len() as u64,
            family.required.len() as u64,
        )?;
    }
    cancellation::check(cancel)?;
    Ok(VerifiedManagedReady {
        prefix,
        family_connection: family_c,
        marker: actual_marker.clone(),
        expected,
        journal,
        required: family.required,
        binding: family.binding,
        installation,
        metadata_floor: floor,
    })
}
