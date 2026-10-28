//! Ordinary B9 Staging -> C8 Ready SQL DATA only. No physical authority,
//! opener, COMMIT, family publication or autonomous admission is granted.
use super::*;
use crate::projection::{
    provider::{
        admitted_ordinary,
        checkout_journal::{CheckoutJournal, ExpectedCheckout},
        import_validation::ImportDatabaseKind,
        managed_budget,
        ordinary_destination::{self, OrdinaryCopyInputs},
        ordinary_publication::OrdinaryPublicationSource,
    },
    shared::{
        admitted_catalog::{
            AdmittedFamilyContext, ExportPublicationInputs, ExportPublicationMember,
            ExportReserveOnlyTarget,
        },
        exact_content::ExactInstallState,
        exact_protection::{ExactProtection, ProtectionPresence},
    },
};
use layrs_core::{projection::ProjectionSelection, scm::TreeEntryTarget};

/// Descriptive borrowed inputs. Each entry reauthenticates B admission, actual
/// B Staging/current/selection, C FORMAT/schema/prefix/suffix and the complete
/// retained+Protected family. The physical driver separately compares its full
/// pinned current image/ExpectedCheckout/journal and retains CAS/checkout pins.
pub(in crate::projection::provider) struct OrdinaryReadyInputs<'a> {
    pub marker: &'a format::Marker,
    pub source_marker: &'a format::Marker,
    pub source_staging: &'a ProviderTransitionIntentRecord,
    pub closure: &'a VerifiedImportClosure,
    pub retained: &'a AdmittedFamilyContext,
    pub target: &'a ExportReserveOnlyTarget,
    pub protection: &'a ExactProtection,
    pub family_marker_bytes: &'a [u8],
    /// Complete Ready floor WITHOUT the checkout journal. Recomputed here;
    /// require_family adds ExpectedCheckout::maximum_encoded_bytes itself.
    pub metadata_floor: u64,
    pub data: &'a ManagedReadyData<'a>,
}

// Constructed solely from this entry's real retained B SQL. Never supplied by
// a caller or shared with a Provider entry as if it were a bootstrap source.
pub(super) struct OrdinaryBasis {
    pub(super) staging: ProviderTransitionIntentRecord,
    pub(super) snapshot: ProjectionSnapshot,
    pub(super) selection: ProjectionSelection,
    pub(super) selection_digest: String,
    pub(super) base: BTreeMap<FileId, (FileManifestId, u64)>,
}

impl OrdinaryReadyInputs<'_> {
    /// Expected publication DATA only; the outer reader still authenticates C.
    pub(in crate::projection::provider) fn publication_inputs(
        &self,
        expected: &ExpectedCheckout,
    ) -> ExportPublicationInputs<'_> {
        ExportPublicationInputs {
            target: self.target,
            destination_ready: self.data.ready,
            receipt: self.data.receipt,
            required: self.data.required,
            protection: self.protection,
            ready_metadata_floor: self.metadata_floor,
            checkout_journal_maximum_bytes: expected.maximum_encoded_bytes(),
            checkout_file_bytes: expected.header().file_bytes,
        }
    }

    fn view<'a>(&'a self, source: &'a OrdinaryBasis) -> ReadyView<'a> {
        ReadyView {
            marker: self.marker,
            source: ReadySource::Ordinary(source),
            prefix: &self.source_staging.plan.source_prefix,
            data: self.data,
        }
    }
}

// Closed entry choice. Existing Ready writer/replay always uses PrivateReady.
// Publication accepts only a complete actual C Ready under the full family walk.
#[derive(Clone, Copy)]
enum FamilyInterpretation {
    PrivateReady,
    Publication,
    SourcePublication,
}

// Closed actual-source authentication. Existing entries always select Staging.
enum SourceAuthentication<'a, 'source> {
    Staging,
    Publication(&'a OrdinaryPublicationSource<'source>),
}

struct OrdinaryPrepared {
    family: FamilyInterpretation,
    basis: OrdinaryBasis,
    suffix: PreparedData,
    installation: ExactInstallState,
    metadata_floor: u64,
}

pub(super) fn unchanged_bytes(
    base: &BTreeMap<FileId, (FileManifestId, u64)>,
    file: &FileId,
    required: &BTreeMap<&FileManifestId, &FileManifest>,
) -> Result<u64> {
    let (id, bytes) = base
        .get(file)
        .ok_or_else(|| invalid("checkout Ready exact B private mapping absent"))?;
    if !required
        .get(id)
        .is_some_and(|m| m.file_id() == file && m.byte_length() == *bytes)
    {
        return Err(invalid(
            "checkout Ready B private canonical representation differs",
        ));
    }
    Ok(*bytes)
}

fn prepare(
    family: FamilyInterpretation,
    authentication: SourceAuthentication<'_, '_>,
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<Box<OrdinaryPrepared>> {
    cancellation::check(cancel)?;
    if source_c.is_autocommit()
        || destination.is_autocommit()
        || family_c.is_autocommit()
        || std::ptr::eq(source_c, destination)
        || std::ptr::eq(source_c, family_c)
        || std::ptr::eq(destination, family_c)
    {
        return Err(invalid(
            "ordinary Ready requires three distinct retained transactions",
        ));
    }
    let staging = match authentication {
        SourceAuthentication::Staging => {
            let source = OrdinaryCopyInputs::new(
                source_c,
                inputs.source_marker,
                inputs.source_staging,
                inputs.closure,
            );
            source.authenticate_staging_source(destination, inputs.marker, cancel)?
        }
        SourceAuthentication::Publication(source) => {
            if !std::ptr::eq(source_c, source.connection())
                || source.marker() != inputs.source_marker
                || !std::ptr::eq(source.closure(), inputs.closure)
                || source.staging() != inputs.source_staging
                || source.destination_ready() != inputs.data.ready
                || source.receipt() != inputs.data.receipt
            {
                return Err(invalid(
                    "ordinary Ready publication inputs differ from closed source",
                ));
            }
            source.authenticate_destination(destination, inputs.marker, cancel)?
        }
    };
    // Source authentication does NOT read C. Its complete exact schema
    // must precede any C inventory/phase query, including on Ready replay.
    super::super::import_validation::require_schema(
        destination,
        inputs.marker,
        ImportDatabaseKind::Provider,
        cancel,
    )?;
    let actual = admitted_ordinary::inspect(source_c, inputs.source_marker, cancel)?;
    if inputs.retained.marker() != actual.marker()
        || inputs.retained.binding() != actual.binding()
        || inputs.retained.admission_digest() != actual.admission_digest()
    {
        return Err(invalid(
            "ordinary Ready retained context differs from actual B admission",
        ));
    }
    let prefix = &inputs.source_staging.plan.source_prefix;
    let ImportSource::Ordinary {
        private_schema: 9 | 10,
        admission: Some(_),
        version,
        ..
    } = &prefix.source
    else {
        return Err(invalid("ordinary Ready requires actual admitted B9 source"));
    };
    let current = database::current(source_c)?;
    let selected = selection::at(source_c, current.generation)?;
    // at() authenticates the selected row's canonical payload and its own
    // generation/snapshot framing. The import version commits that row digest,
    // not the unframed FORMAT initial-selection hash. The row may precede the
    // current snapshot when the selection has not changed.
    let selection_digest: String = source_c.query_row(
        "SELECT digest FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1",
        [current.generation],
        |row| row.get(0),
    )?;
    selected
        .validate_snapshot(&current.snapshot)
        .map_err(|e| invalid(e.to_string()))?;
    if current.generation != version.generation
        || current.digest != version.snapshot_digest
        || selection_digest != version.selection_digest
    {
        return Err(invalid(
            "ordinary Ready actual source head/selection differs",
        ));
    }
    staging.require_prefix(prefix)?;
    inputs.data.ready.require_transition(&staging)?;
    inputs.data.ready.require_receipt(inputs.data.receipt)?;
    let proof = staging
        .destination
        .as_ref()
        .ok_or_else(|| invalid("ordinary Ready destination proof absent"))?;
    let binding = inputs.target.binding();
    if inputs.data.ready.state != TransitionState::Ready
        || binding.transaction_id != staging.transaction_id
        || binding.plan_digest != staging.plan.digest()?
        || binding.prefix_id != staging.plan.source_prefix_id
        || binding.destination_proof_digest != proof.digest(&staging.plan)?
        || binding.owner != inputs.marker.identity.worktree_id
        || binding.database_identity != proof.private_database_identity
        || binding.quota_bytes != staging.plan.quota_bytes
        || binding.operation != staging.reservation_operation()?
        || staging.reservation.as_deref() != Some(binding.reservation.as_str())
        || inputs.target.marker() != inputs.marker
    {
        return Err(invalid(
            "ordinary Ready target or canonical destination Staging differs",
        ));
    }
    let copied = inputs
        .closure
        .copied_content_mappings(source_c, prefix, cancel)?;
    let mut base = BTreeMap::new();
    for entry in &current.snapshot.entries {
        cancellation::check(cancel)?;
        if !selected.includes(&entry.path) {
            continue;
        }
        let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
            continue;
        };
        let (id_bytes, bytes): (u64, u64) = source_c.query_row(
            "SELECT length(CAST(manifest_id AS BLOB)),bytes FROM projection_content WHERE file_id=?1",
            [file_id.as_str()], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if id_bytes != 71 || bytes > 1073741824 {
            return Err(invalid("ordinary Ready exact B mapping bounds differ"));
        }
        let id: String = source_c.query_row(
            "SELECT manifest_id FROM projection_content WHERE file_id=?1",
            [file_id.as_str()],
            |r| r.get(0),
        )?;
        let id = FileManifestId::parse(&id).map_err(|e| invalid(e.to_string()))?;
        if base
            .insert(file_id.clone(), (id.clone(), bytes))
            .is_some_and(|old| old != (id, bytes))
        {
            return Err(invalid(
                "ordinary Ready B mapping conflicts across selected paths",
            ));
        }
    }
    let basis = OrdinaryBasis {
        staging,
        snapshot: current.snapshot,
        selection: selected,
        selection_digest,
        base,
    };
    let view = inputs.view(&basis);
    let suffix = data::prepare_suffix(&view, &copied, cancel)?;
    require_checkout_ready_expected(&view, expected, cancel)?;
    require_complete_union(&basis, &copied, expected, inputs.data.required, cancel)?;
    autonomous::inventory::require_image(
        inputs.data.capture,
        expected,
        &version.effective_image_digest,
        cancel,
    )?;
    let base_floor = managed_budget::base_metadata_floor(
        inputs.closure.metadata_bytes(source_c, prefix, cancel)?,
        inputs.data.observation,
        inputs.data.placement,
        inputs.data.capture,
        &basis.staging.plan,
        &basis.selection,
        managed_budget::intent_size(inputs.source_staging)?,
        managed_budget::intent_size(&basis.staging)?,
    )?;
    if inputs.data.claims.len() != 1 {
        return Err(invalid("ordinary Ready exact claim cardinality differs"));
    }
    let floor = managed_budget::complete_metadata_floor(
        base_floor,
        inputs.data.ready,
        inputs.data.receipt,
        &inputs.data.claims[0],
        inputs.data.required,
        cancel,
    )?;
    if inputs.metadata_floor != floor {
        return Err(invalid(
            "ordinary Ready metadata floor differs from actual B/C pair",
        ));
    }
    let installation = require_family(family, family_c, inputs, expected, cancel)?;
    cancellation::check(cancel)?;
    Ok(Box::new(OrdinaryPrepared {
        family,
        basis,
        suffix,
        installation,
        metadata_floor: floor,
    }))
}

fn require_family(
    interpretation: FamilyInterpretation,
    family_c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<ExactInstallState> {
    let state = match interpretation {
        FamilyInterpretation::PrivateReady => {
            inputs
                .retained
                .validate_export_protection(
                    family_c,
                    inputs.family_marker_bytes,
                    inputs.target,
                    inputs.data.required,
                    inputs.protection,
                    cancel,
                )?
                .1
        }
        FamilyInterpretation::Publication | FamilyInterpretation::SourcePublication => {
            let state = inputs
                .retained
                .read_export_publication(
                    family_c,
                    inputs.family_marker_bytes,
                    &inputs.publication_inputs(expected),
                    cancel,
                )?
                .1;
            if matches!(interpretation, FamilyInterpretation::SourcePublication)
                && state.member != ExportPublicationMember::Ready
            {
                return Err(invalid(
                    "ordinary source publication requires actual family Ready",
                ));
            }
            state.protection
        }
    };
    if state.root != ProtectionPresence::Present {
        return Err(invalid("ordinary Ready requires committed C protection"));
    }
    // Authenticate the supplied one-claim DATA against this same exact root.
    family::require_protection(
        family_c,
        inputs.marker,
        &inputs.source_staging.transaction_id,
        inputs.data.required,
        inputs.data.claims,
        cancel,
    )?;
    let remaining = state
        .installation
        .remaining
        .checked_sub(expected.header().file_bytes)
        .ok_or_else(|| invalid("ordinary Ready checkout bytes exceed remaining reservation"))?;
    let floor = inputs
        .metadata_floor
        .checked_add(expected.maximum_encoded_bytes())
        .ok_or_else(|| invalid("ordinary Ready checkout metadata envelope overflow"))?;
    inputs.protection.require_envelope(remaining, floor)?;
    Ok(state.installation)
}

fn require_complete_union(
    basis: &OrdinaryBasis,
    copied: &BTreeMap<FileId, (FileManifestId, u64)>,
    expected: &ExpectedCheckout,
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let mut union = BTreeMap::new();
    for (file, (id, bytes)) in basis.base.iter().chain(copied) {
        cancellation::check(cancel)?;
        if union
            .insert((file, id), *bytes)
            .is_some_and(|old| old != *bytes)
        {
            return Err(invalid("ordinary Ready base/history length collision"));
        }
    }
    for file in expected.files() {
        cancellation::check(cancel)?;
        if union
            .insert((&file.file_id, &file.manifest_id), file.bytes)
            .is_some_and(|old| old != file.bytes)
        {
            return Err(invalid(
                "ordinary Ready current exact representation length collision",
            ));
        }
    }
    if required.len() != union.len() {
        return Err(invalid(
            "ordinary Ready complete exact union coverage differs",
        ));
    }
    for manifest in required {
        cancellation::check(cancel)?;
        if union.get(&(manifest.file_id(), manifest.id())) != Some(&manifest.byte_length()) {
            return Err(invalid("ordinary Ready complete exact union differs"));
        }
    }
    Ok(())
}

fn read_prepared(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    prepared: &OrdinaryPrepared,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let prefix = &inputs.source_staging.plan.source_prefix;
    // Scoped equality covers all thirteen immutable rowsets, including the
    // exact v2 restore history, while allowing only the checked local suffix.
    inputs.closure.require_copied_prefix_scope(
        source_c,
        destination,
        inputs.marker,
        prefix,
        cancel,
    )?;
    read::ready_suffix(
        destination,
        &inputs.view(&prepared.basis),
        &prepared.suffix,
        Some(expected),
        cancel,
    )?;
    require_family(prepared.family, family_c, inputs, expected, cancel).map(|_| ())
}

/// Actual C8 Staging with a complete published checkout journal and fixed
/// funding for private Ready and future C10. This read-only entry authenticates
/// B and the entire Initializing/protected family through the existing kernels.
/// The detached journal is DATA only; physical pins, writers and closes belong
/// to the driver. Actual private Ready has a separate complete reader.
pub(in crate::projection::provider) fn read_ordinary_checkout_finalizable(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<Box<CheckoutJournal>> {
    let prepared = prepare(
        FamilyInterpretation::PrivateReady,
        SourceAuthentication::Staging,
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        cancel,
    )?;
    let source = OrdinaryCopyInputs::new(
        source_c,
        inputs.source_marker,
        inputs.source_staging,
        inputs.closure,
    );
    let journal = ordinary_destination::read_checkout_staging(
        destination,
        inputs.marker,
        &source,
        expected,
        cancel,
    )?;
    if !journal.is_complete() {
        return Err(invalid(
            "ordinary finalizable checkout requires complete typed journal",
        ));
    }
    let budget = managed_budget::ordinary_admission_budget(
        prepared.metadata_floor,
        expected.files().len() as u64,
        expected.directories().len() as u64,
        inputs.data.required.len() as u64,
    )?;
    let baseline = managed_budget::quota(
        inputs.data.required,
        expected.files().iter().map(|file| file.bytes),
        0,
    )?;
    budget.require_quota(prepared.basis.staging.plan.quota_bytes, baseline)?;
    let remaining = prepared
        .installation
        .remaining
        .checked_sub(expected.header().file_bytes)
        .ok_or_else(|| invalid("ordinary finalizable checkout exceeds remaining reservation"))?;
    budget.require_remaining(
        remaining,
        inputs.protection.payload().len() as u64,
        inputs.data.required.len() as u64,
    )?;
    cancellation::check(cancel)?;
    Ok(Box::new(journal))
}

pub(in crate::projection::provider) fn read_ordinary_checkout_ready(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<DestinationReady> {
    let prepared = prepare(
        FamilyInterpretation::PrivateReady,
        SourceAuthentication::Staging,
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        cancel,
    )?;
    read_prepared(
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        &prepared,
        cancel,
    )?;
    Ok(DestinationReady)
}

/// Complete actual C Ready reader for the one-row family publication boundary.
/// The family member may be Initializing or Ready, but every retained group,
/// reservation, exact C root, prefix, suffix and checkout row is still checked.
/// This entry never writes C and never makes canonical Ready DATA an authority.
pub(in crate::projection::provider) fn read_ordinary_checkout_ready_for_publication(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<DestinationReady> {
    let prepared = prepare(
        FamilyInterpretation::Publication,
        SourceAuthentication::Staging,
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        cancel,
    )?;
    read_prepared(
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        &prepared,
        cancel,
    )?;
    Ok(DestinationReady)
}

/// Actual bounded, decoded C Ready/receipt DATA from this complete read. No
/// source, destination, family, physical pins or durable-close authority.
pub(in crate::projection::provider) struct OrdinaryPublicationReady {
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
}
impl OrdinaryPublicationReady {
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
}

/// Retained-owner/current-frontier publication reader. Authenticate actual B
/// Staging/Ready/Published independently of the original Staging calculation
/// DATA, then complete actual C Ready and the full family with member==Ready.
/// Historical Published handoff after B advances uses a different reader.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn read_ordinary_ready_for_source_publication(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    source: &OrdinaryPublicationSource<'_>,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<OrdinaryPublicationReady> {
    let prepared = prepare(
        FamilyInterpretation::SourcePublication,
        SourceAuthentication::Publication(source),
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        cancel,
    )?;
    // Bound and decode the actual publication rows before the shared suffix
    // comparator materializes those normalized receipt columns again.
    let actual = read_publication_ready_data(destination, inputs, cancel)?;
    read_prepared(
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        &prepared,
        cancel,
    )?;
    source.revalidate(cancel)?;
    Ok(actual)
}

fn read_publication_ready_data(
    c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<OrdinaryPublicationReady> {
    use super::super::records::{import::TransitionReceiptRowData, transition::TransitionRowData};
    cancellation::check(cancel)?;
    // Preflight every scalar and BLOB before materializing either row. The
    // complete suffix read below additionally proves incoming ancestry receipt
    // inventory; only C's own receipt is selected here.
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 2)",
        [],
        |row| row.get(0),
    )?;
    if count != 1 {
        return Err(invalid(
            "ordinary publication actual C transition cardinality differs",
        ));
    }
    let malformed: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE length(CAST(id AS BLOB))=0 OR length(CAST(id AS BLOB))>128 OR length(CAST(role AS BLOB))>16 OR length(CAST(state AS BLOB))>16 OR length(CAST(digest AS BLOB))!=71 OR length(payload)=0 OR length(payload)>262144 OR coalesce(length(CAST(reservation AS BLOB)),0)>128 OR coalesce(length(CAST(receipt_id AS BLOB)),0)!=71)",
        [], |row| row.get(0),
    )?;
    let receipt_id = inputs.data.receipt.digest()?;
    let (receipt_count, malformed_receipt): (u64, bool) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(id AS BLOB))!=71 OR length(CAST(owner_identity_digest AS BLOB))!=71 OR length(CAST(transaction_id AS BLOB))=0 OR length(CAST(transaction_id AS BLOB))>128 OR coalesce(length(CAST(companion_id AS BLOB)),71)!=71 OR length(payload)=0 OR length(payload)>65536),0) FROM projection_provider_transition_receipts WHERE id=?1",
        [&receipt_id], |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if malformed || malformed_receipt || receipt_count != 1 {
        return Err(invalid(
            "ordinary publication actual C Ready/receipt exceeds bounds",
        ));
    }
    let (id, role, state, generation, reservation, receipt_id, payload, digest):
        (String, String, String, u64, Option<String>, Option<String>, Vec<u8>, String) = c.query_row(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
    )?;
    let ready = ProviderTransitionIntentRecord::decode(&payload)?;
    ready.require_row(TransitionRowData {
        id: &id,
        role: &role,
        state: &state,
        local_generation: generation,
        reservation: reservation.as_deref(),
        receipt_id: receipt_id.as_deref(),
        payload: &payload,
        digest: &digest,
    })?;
    let (id, owner, transaction, companion, payload):
        (String, String, String, Option<String>, Vec<u8>) = c.query_row(
        "SELECT id,owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts WHERE id=?1",
        [inputs.data.receipt.digest()?], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    )?;
    let receipt = ProviderTransitionReceiptRecord::decode(&payload)?;
    receipt.require_row(TransitionReceiptRowData {
        id: &id,
        owner_identity_digest: &owner,
        transaction_id: &transaction,
        companion_id: companion.as_deref(),
        payload: &payload,
    })?;
    ready.require_receipt(&receipt)?;
    if ready.role != TransitionRole::Destination
        || ready.state != TransitionState::Ready
        || &ready != inputs.data.ready
        || &receipt != inputs.data.receipt
    {
        return Err(invalid(
            "ordinary publication actual C Ready/receipt differs",
        ));
    }
    cancellation::check(cancel)?;
    Ok(OrdinaryPublicationReady { ready, receipt })
}

/// Atomic destination SQL effect only. Ready replay accepts a read-only caller
/// transaction; every post-mutation error requires rollback. COMMIT/positive
/// closes and physical pinned checkout/CAS verification remain with the driver.
pub(in crate::projection::provider) fn finalize_ordinary_checkout_or_replay(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<ReadyEffect> {
    let prepared = prepare(
        FamilyInterpretation::PrivateReady,
        SourceAuthentication::Staging,
        source_c,
        destination,
        family_c,
        inputs,
        expected,
        cancel,
    )?;
    let (count, max_state): (u64, u64) = destination.query_row(
        "SELECT count(*),coalesce(max(length(CAST(state AS BLOB))),0) FROM (SELECT state FROM projection_provider_transitions LIMIT 2)",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || max_state > 16 {
        return Err(invalid("ordinary Ready phase inventory differs"));
    }
    let state: String = destination.query_row(
        "SELECT state FROM projection_provider_transitions",
        [],
        |r| r.get(0),
    )?;
    if state == "ready" {
        read_prepared(
            source_c,
            destination,
            family_c,
            inputs,
            expected,
            &prepared,
            cancel,
        )?;
        return Ok(ReadyEffect::AlreadyReady);
    }
    if state != "staging"
        || destination.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1
    {
        return Err(invalid(
            "ordinary Ready writer requires exact Staging and FK ON",
        ));
    }
    let source = OrdinaryCopyInputs::new(
        source_c,
        inputs.source_marker,
        inputs.source_staging,
        inputs.closure,
    );
    let journal = ordinary_destination::read_checkout_staging(
        destination,
        inputs.marker,
        &source,
        expected,
        cancel,
    )?;
    if !journal.is_complete() {
        return Err(invalid("ordinary Ready requires complete typed checkout"));
    }
    append_suffix(
        destination,
        &inputs.view(&prepared.basis),
        &prepared.basis.staging,
        &prepared.suffix,
        cancel,
    )?;
    // Reauthenticate actual B and the entire family as well as complete C; do
    // not reuse an earlier-phase whole-table reader after appending the suffix.
    read_ordinary_checkout_ready(source_c, destination, family_c, inputs, expected, cancel)?;
    cancellation::check(cancel)?;
    Ok(ReadyEffect::Finalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{Chunk, ChunkingDescriptor};

    #[test]
    fn unchanged_length_uses_exact_b_mapping_body_without_file_id_fallback() {
        let chunk = Chunk::new(b"unchanged exact bytes".to_vec()).unwrap();
        let base =
            FileManifest::from_chunks(ChunkingDescriptor::single(), &[chunk.clone()]).unwrap();
        let current = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(1048576).unwrap(),
            &[chunk],
        )
        .unwrap();
        assert_eq!(base.file_id(), current.file_id());
        assert_ne!(base.id(), current.id());
        let mut mapping = BTreeMap::from([(
            base.file_id().clone(),
            (base.id().clone(), base.byte_length()),
        )]);
        let only_current = BTreeMap::from([(current.id(), &current)]);
        assert!(unchanged_bytes(&mapping, base.file_id(), &only_current).is_err());
        let both = BTreeMap::from([(base.id(), &base), (current.id(), &current)]);
        assert_eq!(
            unchanged_bytes(&mapping, base.file_id(), &both).unwrap(),
            current.byte_length()
        );
        mapping.get_mut(base.file_id()).unwrap().1 += 1;
        assert!(unchanged_bytes(&mapping, base.file_id(), &both).is_err());
    }
}
