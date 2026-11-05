//! Destination Ready SQL DATA only. The caller separately proves the copied
//! checkout and durable exact family content/protection, retains all pins and
//! gates, and owns transactions, rollback, COMMIT and positive closes. This
//! module grants no ordinary admission and never opens a business database.
//! Existing writers/readers require an Initializing family member, including
//! Ready replay. A separate read-only entry interprets the exact Ready member
//! after external family publication; neither entry grants ordinary admission.
use super::{
    cancellation, database_io, destination_validation,
    import_validation::{OrdinaryFrozenBasis, VerifiedImportClosure},
    records::{
        AllocationRecord, PreparedRecord, ProviderBootstrapSource, Record,
        activation::ProviderActivationRecord,
        import::{ImportSource, ProviderImportPrefixRecord, ProviderTransitionReceiptRecord},
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    recovery::error::{Result, invalid},
};
use crate::projection::{capture::CapturedPatch, database, format, selection};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{FileId, FileManifest, FileManifestId, RepoPath, canonical_json},
};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

mod autonomous;
mod data;
mod denial;
mod family;
mod historical;
mod ordinary;
mod retained_companion;
pub(in crate::projection::provider) use historical::{FrozenReadyRows, read_frozen_ready_rows};
pub(in crate::projection::provider) use ordinary::{
    OrdinaryPublicationReady, OrdinaryReadyInputs, finalize_ordinary_checkout_or_replay,
    read_ordinary_checkout_finalizable, read_ordinary_checkout_progress,
    read_ordinary_checkout_ready, read_ordinary_checkout_ready_for_publication,
    read_ordinary_ready_for_source_publication,
};
mod read;
pub(in crate::projection::provider) use autonomous::{
    OrdinaryPrivateReadySnapshot, OwnedReadySnapshot, VerifiedManagedReady,
    VerifiedOrdinaryFamilyPublication, read_managed_ready, read_ordinary_family_publication,
    read_ordinary_managed_ready, read_ordinary_private_ready,
};

pub(in crate::projection::provider) struct ManagedReadyOverlay {
    pub path: RepoPath,
    pub manifest_id: Option<FileManifestId>,
}
pub(in crate::projection::provider) struct ManagedReadyClaim {
    pub root_id: String,
    pub digest: String,
    pub kind: String,
    pub owner: String,
}

/// Immutable expected DATA, not a content, filesystem or authorization proof.
pub(in crate::projection::provider) struct ManagedReadyData<'a> {
    pub observation: &'a ProjectionSnapshot,
    pub placement: &'a ProjectionSnapshot,
    pub capture: &'a CapturedPatch,
    pub overlay: &'a [ManagedReadyOverlay],
    pub required: &'a [FileManifest],
    pub receipt: &'a ProviderTransitionReceiptRecord,
    pub ready: &'a ProviderTransitionIntentRecord,
    pub claims: &'a [ManagedReadyClaim],
}
/// Closed reconciliation DATA plus the real pending B root. The caller still
/// owns physical C checkout/CAS and the complete source checkpoint.
#[derive(Clone, Copy)]
pub(in crate::projection::provider) struct ManagedCompanionReadyData<'a> {
    pub rows: &'a super::transition_reconcile::companions::OwnedCompanionData,
    pub observation: &'a super::transition_observation::TransitionObservationData,
    pub source_context:
        crate::projection::shared::source_companion::SourceCompanionFamilyContext<'a>,
}
/// Closed fresh observation without a new companion group. The full source
/// context authenticates d; it must not contain a pending preservation root s.
#[derive(Clone, Copy)]
pub(in crate::projection::provider) struct ManagedDenialReadyData<'a> {
    pub rows: &'a super::transition_reconcile::companions::OwnedCompanionData,
    pub observation: &'a super::transition_observation::TransitionObservationData,
    pub coverage: &'a super::records::denied_coverage::DerivedDeniedCoverage,
    pub source_context:
        crate::projection::shared::source_companion::SourceCompanionFamilyContext<'a>,
}
pub(in crate::projection::provider) struct ManagedReadyInputs<'a> {
    pub marker: &'a format::Marker,
    pub source: &'a ProviderBootstrapSource,
    pub allocation: &'a AllocationRecord,
    pub prepared: &'a PreparedRecord,
    pub activation: &'a ProviderActivationRecord,
    pub prefix: &'a ProviderImportPrefixRecord,
    pub staging: &'a ProviderTransitionIntentRecord,
    pub closure: &'a VerifiedImportClosure,
    pub family_marker_bytes: &'a [u8],
    pub companion: Option<ManagedCompanionReadyData<'a>>,
    pub denial: Option<ManagedDenialReadyData<'a>>,
    pub data: &'a ManagedReadyData<'a>,
}

/// Private suffix DATA, independent of the provenance-specific entry wrapper.
/// Construction never authenticates a source or promotes a protocol phase.
struct ReadyView<'a> {
    marker: &'a format::Marker,
    source: ReadySource<'a>,
    prefix: &'a ProviderImportPrefixRecord,
    data: &'a ManagedReadyData<'a>,
    companion: Option<&'a ManagedCompanionReadyData<'a>>,
    retained_companion: Option<&'a retained_companion::RetainedCompanionReadyData>,
    denial: Option<&'a ManagedDenialReadyData<'a>>,
    retained_denial: Option<&'a denial::RetainedDenialReadyData>,
}

impl ReadyView<'_> {
    fn selection(&self) -> &layrs_core::projection::ProjectionSelection {
        if let Some(data) = self.denial {
            return data.observation.selection();
        }
        if let Some(data) = self.retained_denial {
            return data.selection();
        }
        match (self.companion, self.retained_companion) {
            (Some(c), None) => c.observation.selection(),
            (None, Some(c)) => c.selection(),
            _ => self.source.selection(),
        }
    }
    fn has_companion(&self) -> bool {
        self.companion.is_some() || self.retained_companion.is_some()
    }
    fn has_fresh_observation(&self) -> bool {
        self.has_companion() || self.denial.is_some() || self.retained_denial.is_some()
    }
    fn companion_header(&self) -> Option<&super::records::companion_v2::CompanionHeaderV2> {
        self.companion
            .and_then(|c| c.rows.header())
            .or_else(|| self.retained_companion.map(|c| c.header()))
    }
    fn companion_mapping(&self, file: &FileId) -> Option<&(FileManifestId, u64)> {
        self.companion
            .and_then(|c| c.rows.source_mappings().get(file))
            .or_else(|| self.retained_companion.and_then(|c| c.mapping(file)))
    }
    fn require_companion(&self, cancel: Option<&Cancellation>) -> Result<()> {
        let modes = usize::from(self.companion.is_some())
            + usize::from(self.retained_companion.is_some())
            + usize::from(self.denial.is_some())
            + usize::from(self.retained_denial.is_some());
        if modes > 1 {
            return Err(invalid("Ready observation modes overlap"));
        }
        if let Some(data) = self.denial {
            return denial::require_live(data, self.data, self.prefix, cancel);
        }
        if let Some(data) = self.retained_denial {
            return data.require_data(self.data, self.prefix, cancel);
        }
        if let Some(retained) = self.retained_companion {
            if self.companion.is_some() {
                return Err(invalid("Ready companion modes overlap"));
            }
            return retained.require_data(self.data, self.prefix, cancel);
        }
        let Some(c) = self.companion else {
            if self.data.ready.plan.denial_coverage.is_some() {
                return Err(invalid(
                    "Ready denial plan requires explicit observation mode",
                ));
            }
            if self.data.ready.plan.companion_id.is_some()
                || self.data.ready.plan.companion_files.count != 0
                || self.data.ready.plan.companion_deletions.count != 0
            {
                return Err(invalid("strict Ready DATA refuses new companions"));
            }
            return self.source.require_snapshots(self.data);
        };
        cancellation::check(cancel)?;
        if self.data.ready.plan.denial_coverage.is_some() {
            // This is expected DATA only. SQL entry readers independently
            // require the stored body before accepting or writing Ready.
            let body =
                super::records::denied_coverage::TransitionDeniedCoverageRecord::from_observation(
                    self.prefix,
                    &self.data.ready.transaction_id,
                    c.observation,
                    cancel,
                )?;
            body.require_intent(self.data.ready, cancel)?;
        } else if c.observation.binding().denied_path_count != 0 {
            return Err(invalid(
                "companion Ready observation, overlay or full union differs",
            ));
        }
        if c.rows.manifests() != self.data.required
            || c.rows.overlay() != self.data.capture.changes
            || c.observation.snapshot() != self.data.observation
        {
            return Err(invalid(
                "companion Ready observation, overlay or full union differs",
            ));
        }
        if self.data.ready.plan.denial_coverage.is_some() {
            c.rows
                .require_denial_plan_observation(self.prefix, c.observation)?;
        } else {
            c.rows
                .require_plan_observation(self.prefix, c.observation)?;
        }
        let header = c
            .rows
            .header()
            .ok_or_else(|| invalid("companion Ready header absent"))?;
        header.require_prefix(self.prefix)?;
        header.require_receipt(self.data.receipt)?;
        let plan = &self.data.ready.plan;
        if plan.companion_id.as_deref() != Some(header.digest()?.as_str())
            || plan.companion_files.count != header.file_count
            || plan.companion_files.digest != header.files_digest
            || plan.companion_deletions.count != header.deletion_count
            || plan.companion_deletions.digest != header.deletions_digest
        {
            return Err(invalid("companion Ready plan rowset commitments differ"));
        }
        let mut placement = c.observation.snapshot().clone();
        for entry in &mut placement.entries {
            if c.observation.selection().includes(&entry.path)
                && entry.state != layrs_core::projection::EntryState::Reserved
                && matches!(
                    entry.target,
                    Some(layrs_core::scm::TreeEntryTarget::File { .. })
                )
            {
                entry.state = layrs_core::projection::EntryState::Materialized;
            }
        }
        if &placement != self.data.placement {
            return Err(invalid("companion Ready exact placement differs"));
        }
        Ok(())
    }
}

/// Each entry wrapper authenticates its own source before constructing this
/// private suffix view. Provider constructors never accept an ordinary basis.
enum ReadySource<'a> {
    Provider(&'a ProviderBootstrapSource),
    Ordinary(&'a ordinary::OrdinaryBasis),
    OrdinaryFrozen {
        source: &'a OrdinaryFrozenBasis,
        lengths: &'a BTreeMap<FileId, u64>,
    },
}

impl ReadySource<'_> {
    fn version<'p>(
        &self,
        prefix: &'p ProviderImportPrefixRecord,
    ) -> Result<&'p super::records::import::ImportVersion> {
        match (self, &prefix.source) {
            (Self::Provider(_), ImportSource::Provider { version, .. })
            | (
                Self::Ordinary(_) | Self::OrdinaryFrozen { .. },
                ImportSource::Ordinary {
                    private_schema: 9 | 10,
                    admission: Some(_),
                    version,
                    ..
                },
            ) => Ok(version),
            _ => Err(invalid("Ready source basis and prefix kind differ")),
        }
    }

    fn selection(&self) -> &layrs_core::projection::ProjectionSelection {
        match self {
            Self::Provider(source) => source.selection(),
            Self::Ordinary(source) => &source.selection,
            Self::OrdinaryFrozen { source, .. } => source.selection(),
        }
    }

    fn require_snapshots(&self, data: &ManagedReadyData<'_>) -> Result<()> {
        let valid = match self {
            Self::Provider(source) => {
                data.observation == source.initial()
                    && data.placement == &super::records::activation::activation_snapshot(source)?
            }
            Self::Ordinary(source) => {
                data.observation == &source.snapshot && data.placement == &source.snapshot
            }
            Self::OrdinaryFrozen { source, .. } => {
                data.observation == source.snapshot() && data.placement == source.snapshot()
            }
        };
        if !valid {
            return Err(invalid(
                "Ready snapshots differ from authenticated source basis",
            ));
        }
        Ok(())
    }

    fn selection_digest(&self, prefix: &ProviderImportPrefixRecord) -> Result<String> {
        Ok(match self {
            Self::Provider(source) => source.commitment().selection_digest.clone(),
            Self::Ordinary(source) => {
                let version = self.version(prefix)?;
                if source.selection_digest != version.selection_digest {
                    return Err(invalid("ordinary Ready source selection differs"));
                }
                source.selection_digest.clone()
            }
            Self::OrdinaryFrozen { source, .. } => {
                if source.selection_digest() != self.version(prefix)?.selection_digest {
                    return Err(invalid("ordinary frozen Ready source selection differs"));
                }
                source.selection_digest().to_owned()
            }
        })
    }

    fn require_base_union(
        &self,
        required: &BTreeMap<(&FileId, &FileManifestId), &FileManifest>,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        match self {
            Self::Provider(source) => {
                for descriptor in source.descriptors() {
                    cancellation::check(cancel)?;
                    if required
                        .get(&(&descriptor.file_id, descriptor.manifest.id()))
                        .copied()
                        != Some(&descriptor.manifest)
                    {
                        return Err(invalid("destination Ready source representation absent"));
                    }
                }
            }
            Self::Ordinary(source) => {
                for (file, (manifest, bytes)) in &source.base {
                    cancellation::check(cancel)?;
                    if !required
                        .get(&(file, manifest))
                        .is_some_and(|m| m.byte_length() == *bytes)
                    {
                        return Err(invalid(
                            "ordinary Ready exact private base representation absent",
                        ));
                    }
                }
            }
            Self::OrdinaryFrozen { source, lengths } => {
                // Only FileId coverage is recoverable here. The complete
                // committed union, not an arbitrary C mapping, supplies exact
                // representations; the old B base ManifestId is not inferred.
                for entry in &source.snapshot().entries {
                    cancellation::check(cancel)?;
                    if !source.selection().includes(&entry.path) {
                        continue;
                    }
                    if let Some(layrs_core::scm::TreeEntryTarget::File { file_id }) = &entry.target
                    {
                        if !lengths.contains_key(file_id) {
                            return Err(invalid("ordinary frozen Ready selected base absent"));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn unchanged_bytes(
        &self,
        path: &RepoPath,
        file: &FileId,
        executable: bool,
        required: &BTreeMap<&FileManifestId, &FileManifest>,
    ) -> Result<u64> {
        match self {
            Self::Provider(source) => source
                .descriptors()
                .binary_search_by(|d| d.path.cmp(path))
                .ok()
                .and_then(|n| source.descriptors().get(n))
                .filter(|d| &d.file_id == file && d.executable == executable)
                .map(|d| d.bytes)
                .ok_or_else(|| invalid("checkout Ready unchanged source descriptor absent")),
            Self::Ordinary(source) => ordinary::unchanged_bytes(&source.base, file, required),
            Self::OrdinaryFrozen { lengths, .. } => lengths
                .get(file)
                .copied()
                .ok_or_else(|| invalid("ordinary frozen unchanged representation absent")),
        }
    }
}
impl ManagedReadyInputs<'_> {
    fn view(&self) -> ReadyView<'_> {
        ReadyView {
            marker: self.marker,
            source: ReadySource::Provider(self.source),
            prefix: self.prefix,
            data: self.data,
            companion: self.companion.as_ref(),
            retained_companion: None,
            denial: self.denial.as_ref(),
            retained_denial: None,
        }
    }
}
pub(in crate::projection::provider) struct DestinationReady;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum ReadyEffect {
    Finalized,
    AlreadyReady,
}

struct PreparedData {
    observation: Vec<u8>,
    placement: Vec<u8>,
    capture: Vec<u8>,
    /// Only mappings absent from the immutable imported scope. Historical
    /// FileId-primary-key choices are never replaced by another representation.
    new_content: Vec<(String, String, u64)>,
}

pub(in crate::projection::provider) fn validate_ready_data(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    data::prepare(source_c, destination, family_c, inputs, cancel).map(|_| ())
}

/// Strict Ready suffix reader. The Staging reader remains unchanged and still
/// rejects any local observation, capture, claim or appended immutable row.
pub(in crate::projection::provider) fn read_ready(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<DestinationReady> {
    let prepared = data::prepare(source_c, destination, family_c, inputs, cancel)?;
    read::ready(source_c, destination, inputs, &prepared, None, cancel)?;
    Ok(DestinationReady)
}

/// Append the exact local suffix atomically in the caller's destination
/// transaction. Ready replay performs only reads and permits an RO transaction.
/// Any error after a mutation requires caller rollback; no cleanup is attempted.
pub(in crate::projection::provider) fn finalize_staged_or_replay(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<ReadyEffect> {
    finalize_inner(source_c, destination, family_c, inputs, None, cancel)
}

fn finalize_inner(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    checkout: Option<&super::checkout_journal::ExpectedCheckout>,
    cancel: Option<&Cancellation>,
) -> Result<ReadyEffect> {
    let prepared = data::prepare(source_c, destination, family_c, inputs, cancel)?;
    let count: u64 = destination.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 2)",
        [],
        |row| row.get(0),
    )?;
    if count != 1 {
        return Err(invalid("destination Ready intent cardinality differs"));
    }
    let (staging, ready): (bool, bool) = destination.query_row(
        "SELECT state='staging',state='ready' FROM projection_provider_transitions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if ready {
        read::ready(source_c, destination, inputs, &prepared, checkout, cancel)?;
        return Ok(ReadyEffect::AlreadyReady);
    }
    if !staging {
        return Err(invalid("destination Ready predecessor phase differs"));
    }
    if destination.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u64>(0))? != 1 {
        return Err(invalid("destination Ready writer requires FK ON"));
    }
    match checkout {
        None => {
            destination_validation::read_staging(
                destination,
                inputs.marker,
                inputs.source,
                inputs.allocation,
                inputs.prepared,
                inputs.activation,
                inputs.prefix,
                inputs.staging,
                inputs.closure,
                cancel,
            )?;
        }
        Some(expected) => {
            require_checkout_expected(inputs, expected, cancel)?;
            let journal = destination_validation::read_checkout_staging(
                destination,
                inputs.marker,
                inputs.source,
                inputs.allocation,
                inputs.prepared,
                inputs.activation,
                inputs.prefix,
                inputs.staging,
                inputs.closure,
                expected,
                cancel,
            )?;
            if !journal.is_complete() {
                return Err(invalid(
                    "destination Ready requires complete checkout journal",
                ));
            }
        }
    }
    append_suffix(
        destination,
        &inputs.view(),
        inputs.staging,
        &prepared,
        cancel,
    )?;
    read::ready(source_c, destination, inputs, &prepared, checkout, cancel)?;
    cancellation::check(cancel)?;
    Ok(ReadyEffect::Finalized)
}

/// Accept only a complete bound checkout journal and retain it byte-exact as
/// durable path-to-ManifestId provenance, including for read-only Ready replay.
/// The original readers/finalizer still require an empty prejournal.
///
/// This is DATA only: the caller must retain/revalidate the independently
/// authenticated image, copied checkout bytes/modes/namespace and native pins
/// through COMMIT and positive closes. A journal's inode strings are no physical
/// proof. This intermediate protocol grants no ordinary admission or future
/// history-import provenance. Migrating/retiring the retained journal requires
/// a separate typed protocol; no ordinary cleanup route may consume it.
/// Rollback is mandatory on every error after a mutation.
pub(in crate::projection::provider) fn finalize_checkout_or_replay(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    expected: &super::checkout_journal::ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<ReadyEffect> {
    finalize_inner(
        source_c,
        destination,
        family_c,
        inputs,
        Some(expected),
        cancel,
    )
}

/// Read only the distinct Ready form which retains complete exact checkout
/// provenance. This returns SQL DATA, never a physical checkout capability.
pub(in crate::projection::provider) fn read_checkout_ready(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    expected: &super::checkout_journal::ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<DestinationReady> {
    let prepared = data::prepare(source_c, destination, family_c, inputs, cancel)?;
    read::ready(
        source_c,
        destination,
        inputs,
        &prepared,
        Some(expected),
        cancel,
    )?;
    Ok(DestinationReady)
}

/// Dedicated post-family-publication DATA reader. All private prefix/suffix,
/// receipt, root, membership and complete checkout journal checks are shared
/// with read_checkout_ready; the family destination must now be exactly Ready.
/// This is not autonomous ordinary admission or a physical ownership proof.
pub(in crate::projection::provider) fn read_checkout_family_ready(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    expected: &super::checkout_journal::ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<DestinationReady> {
    let prepared = data::prepare_at_member(
        source_c,
        destination,
        family_c,
        inputs,
        family::MemberPhase::Ready,
        cancel,
    )?;
    read::ready(
        source_c,
        destination,
        inputs,
        &prepared,
        Some(expected),
        cancel,
    )?;
    Ok(DestinationReady)
}

fn require_checkout_expected(
    inputs: &ManagedReadyInputs<'_>,
    expected: &super::checkout_journal::ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let rebound = super::checkout_journal::ExpectedCheckout::new(
        inputs.staging,
        expected.files(),
        expected.deletions(),
        &expected.header().image_digest,
        cancel,
    )?;
    if rebound.header() != expected.header() || expected.marker() != inputs.marker {
        return Err(invalid("checkout journal destination binding differs"));
    }
    require_checkout_inventory(&inputs.view(), expected, cancel)
}

/// Bind every current leaf and explicit deletion to the base-relative Ready
/// patch. Unchanged files may use any required exact representation of those
/// same bytes; changed files must use the committed overlay representation.
/// This compares metadata, not the caller's actual retained image or bytes.
fn require_checkout_ready_expected(
    inputs: &ReadyView<'_>,
    expected: &super::checkout_journal::ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let rebound = match &inputs.source {
        ReadySource::Provider(_) => super::checkout_journal::ExpectedCheckout::from_ready(
            inputs.data.ready,
            expected.files(),
            expected.deletions(),
            &expected.header().image_digest,
            cancel,
        )?,
        ReadySource::Ordinary(source) => super::checkout_journal::ExpectedCheckout::ordinary(
            &source.staging,
            expected.files(),
            expected.deletions(),
            &expected.header().image_digest,
            cancel,
        )?,
        ReadySource::OrdinaryFrozen { .. } => {
            super::checkout_journal::ExpectedCheckout::from_ready_ordinary(
                inputs.data.ready,
                expected.files(),
                expected.deletions(),
                &expected.header().image_digest,
                cancel,
            )?
        }
    };
    if rebound.header() != expected.header() || expected.marker() != inputs.marker {
        return Err(invalid("checkout journal destination binding differs"));
    }
    require_checkout_inventory(inputs, expected, cancel)
}

fn require_checkout_inventory(
    inputs: &ReadyView<'_>,
    expected: &super::checkout_journal::ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    use layrs_core::{projection::EntryState, scm::TreeEntryTarget};
    let required: BTreeMap<_, _> = inputs.data.required.iter().map(|m| (m.id(), m)).collect();
    let patch = inputs.data.capture;
    let mut files = 0usize;
    let mut deletions = 0usize;
    let mut changes = 0usize;
    for entry in &inputs.data.placement.entries {
        cancellation::check(cancel)?;
        if !inputs.selection().includes(&entry.path)
            || (inputs.has_fresh_observation() && entry.state == EntryState::Reserved)
        {
            continue;
        }
        let base = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => {
                return Err(invalid(
                    "checkout Ready base is not a selected file or absence",
                ));
            }
        };
        let change = patch
            .changes
            .binary_search_by(|change| change.path.cmp(&entry.path))
            .ok();
        let actual = if let Some(index) = change {
            changes += 1;
            let change = &patch.changes[index];
            if !entry.writable
                || change.expected.as_ref().map(|f| (&f.file_id, f.executable)) != base
            {
                return Err(invalid("checkout Ready patch preimage differs"));
            }
            match &change.replacement {
                Some(file) => Some((
                    &file.file_id,
                    file.executable,
                    file.bytes,
                    inputs.data.overlay[index].manifest_id.as_ref(),
                )),
                None => {
                    if base.is_none() || expected.deletions().get(deletions) != Some(&entry.path) {
                        return Err(invalid("checkout Ready deletion inventory differs"));
                    }
                    deletions += 1;
                    None
                }
            }
        } else if let Some((file_id, executable)) = base {
            let bytes = if let Some(data) = inputs.denial {
                data.observation
                    .descriptors()
                    .iter()
                    .find(|d| {
                        d.path == entry.path && &d.file_id == file_id && d.executable == executable
                    })
                    .ok_or_else(|| invalid("plain denial Ready unchanged descriptor absent"))?
                    .bytes
            } else if let Some(data) = inputs.retained_denial {
                data.length(file_id)?
            } else {
                match (inputs.companion, inputs.retained_companion) {
                    (None, None) => inputs.source.unchanged_bytes(
                        &entry.path,
                        file_id,
                        executable,
                        &required,
                    )?,
                    (Some(c), None) => {
                        c.observation
                            .descriptors()
                            .iter()
                            .find(|d| {
                                d.path == entry.path
                                    && &d.file_id == file_id
                                    && d.executable == executable
                            })
                            .ok_or_else(|| invalid("companion Ready unchanged descriptor absent"))?
                            .bytes
                    }
                    (None, Some(c)) => c.length(file_id)?,
                    _ => return Err(invalid("Ready companion modes overlap")),
                }
            };
            Some((file_id, executable, bytes, None))
        } else {
            None
        };
        if let Some((file_id, executable, bytes, overlay)) = actual {
            let file = expected
                .files()
                .get(files)
                .ok_or_else(|| invalid("checkout Ready file inventory differs"))?;
            if file.path != entry.path
                || &file.file_id != file_id
                || file.executable != executable
                || file.bytes != bytes
                || overlay.is_some_and(|id| id != &file.manifest_id)
                || !required
                    .get(&file.manifest_id)
                    .is_some_and(|m| m.file_id() == file_id && m.byte_length() == bytes)
            {
                return Err(invalid("checkout Ready file inventory differs"));
            }
            files += 1;
        }
    }
    if files != expected.files().len()
        || deletions != expected.deletions().len()
        || changes != patch.changes.len()
    {
        return Err(invalid(
            "checkout Ready inventory has missing or extra paths",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod checkout_tests;

#[cfg(test)]
mod ordinary_tests;

// SQL append only. Each closed source entry first validates its full actual
// Staging ingress and afterwards reads the complete immutable prefix + suffix.
fn append_suffix(
    destination: &Connection,
    inputs: &ReadyView<'_>,
    staging: &ProviderTransitionIntentRecord,
    prepared: &PreparedData,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let plan = &staging.plan;
    let version = inputs.source.version(inputs.prefix)?;
    for (generation, digest, parent, payload) in [
        (
            plan.observation.generation,
            &plan.observation.snapshot_digest,
            &version.snapshot_digest,
            &prepared.observation,
        ),
        (
            plan.placement_generation,
            &plan.placement_digest,
            &plan.observation.snapshot_digest,
            &prepared.placement,
        ),
    ] {
        cancellation::check(cancel)?;
        if destination.execute(
            "INSERT INTO projection_snapshots(generation,digest,parent_digest,payload) VALUES(?1,?2,?3,?4)",
            params![generation,digest,parent,payload],
        )? != 1 {
            return Err(invalid("destination Ready snapshot insert differs"));
        }
        selection::insert(destination, generation, inputs.selection())?;
    }
    for (file_id, manifest_id, bytes) in &prepared.new_content {
        cancellation::check(cancel)?;
        if destination.execute(
            "INSERT INTO projection_content(file_id,manifest_id,bytes) VALUES(?1,?2,?3)",
            params![file_id, manifest_id, bytes],
        )? != 1
        {
            return Err(invalid("destination Ready content insert differs"));
        }
    }
    let patch = inputs.data.capture;
    destination.execute(
        "INSERT INTO projection_captures(sequence,digest,projection_generation,payload) VALUES(?1,?2,?3,?4)",
        params![patch.sequence,patch.digest,patch.projection_generation,prepared.capture],
    )?;
    let claim = &inputs.data.claims[0];
    destination.execute(
        "INSERT INTO projection_claims(root_id,digest,kind,owner) VALUES(?1,?2,?3,?4)",
        params![claim.root_id, claim.digest, claim.kind, claim.owner],
    )?;
    if let Some(companion) = inputs.companion {
        // Identical closed rows precede THIS database's own receipt, in this
        // same Ready transaction. B's private rows are still absent.
        companion
            .rows
            .insert_source_mappings_and_rows(destination, cancel)?;
    }
    let receipt = inputs.data.receipt;
    destination.execute(
        "INSERT INTO projection_provider_transition_receipts(id,owner_identity_digest,transaction_id,companion_id,payload) VALUES(?1,?2,?3,?4,?5)",
        params![receipt.digest()?,receipt.owner_identity_digest,receipt.transaction_id,
            receipt.companion_id,receipt.encode()?],
    )?;
    if destination.execute(
        "UPDATE projection_head SET generation=?1 WHERE singleton=1 AND generation=?2",
        params![plan.placement_generation, version.generation],
    )? != 1
    {
        return Err(invalid("destination Ready head CAS differs"));
    }
    let old = staging;
    let new = inputs.data.ready;
    if destination.execute(
        "UPDATE projection_provider_transitions SET state='ready',receipt_id=?1,payload=?2,digest=?3 \
         WHERE id=?4 AND role=?5 AND state=?6 AND local_generation=?7 \
         AND reservation IS ?8 AND receipt_id IS ?9 AND payload=?10 AND digest=?11",
        params![new.receipt_id,new.encode()?,new.digest()?,old.transaction_id,"destination",
            "staging",old.local_generation,old.reservation,old.receipt_id,old.encode()?,old.digest()?],
    )? != 1 {
        return Err(invalid("destination Ready exact intent CAS differs"));
    }
    cancellation::check(cancel)
}
