//! Complete terminal managed Ready SQL DATA from the actual destination and
//! family. No source connection, original source owner or physical byte proof.
use super::super::{
    checkout_journal::{CheckoutJournal, ExpectedCheckout},
    import_validation::{read_destination_prefix, VerifiedDestinationPrefix},
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
        let fresh = read_managed_ready(c, &self.marker, family_c, cancel)?;
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
    cancellation::check(cancel)?;
    if c.is_autocommit() || family_c.is_autocommit() || std::ptr::eq(c, family_c) {
        return Err(invalid(
            "autonomous Ready requires distinct retained transactions",
        ));
    }
    let prefix = read_destination_prefix(c, actual_marker, cancel)?;
    let ready = prefix.ready();
    let plan = &ready.plan;
    let ImportSource::Provider { version, .. } = &prefix.prefix().source else {
        return Err(invalid("autonomous Ready requires provider prefix"));
    };
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
    let (expected, journal) = ExpectedCheckout::read_ready(c, ready, &deletions, cancel)?;
    let claim = family_read::read_claim(c, ready)?;
    let family = family_read::read(family_c, actual_marker, ready, &claim, cancel)?;
    let copied = prefix.copied_content_mappings(c, cancel)?;
    inventory::require_union(
        prefix.source(),
        &copied,
        &expected,
        &family.required,
        cancel,
    )?;
    let overlay = inventory::overlay(&capture, &expected, cancel)?;
    inventory::require_image(&capture, &expected, &version.effective_image_digest, cancel)?;
    let observation = prefix.source().initial().clone();
    let placement = super::super::records::activation::activation_snapshot(prefix.source())?;
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
        source: prefix.source(),
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
    let installation = exact_content::validate_protected_installations(
        family_c,
        &family.binding,
        &family.required,
        &family.protection,
        cancel,
    )?;
    let (source_size, destination_size) = managed_budget::original_staging_sizes(ready)?;
    let base = managed_budget::base_metadata_floor(
        prefix.metadata_bytes(c, cancel)?,
        &observation,
        &placement,
        &capture,
        plan,
        prefix.source().selection(),
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
    })
}
