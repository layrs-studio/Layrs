//! ReserveOnly planning DATA, without ordinary lease/catalog lookup. The
//! physical caller authenticates actual B Staging and C through the appropriate
//! closed phase reader before any effect. A Ready driver may supply canonical
//! destination-Staging predecessor DATA here; that is never a claim about C's
//! actual phase. Cached manifest bodies must pass the export catalog and CAS
//! readers, and actual C must pass its complete Staging or Ready reader.
//! This never sets validated_metadata_floor or authorizes a reservation.
use super::*;
use crate::projection::provider::{managed_budget, records::Record};

pub(super) struct ReservePlan {
    pub(super) data: ManagedPlanData,
    pub(super) required: Vec<FileManifest>,
    pub(super) baseline_quota: u64,
    pub(super) actual_metadata_floor: u64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn derive(
    c: &Connection,
    prefix: &ProviderImportPrefixRecord,
    closure: &VerifiedImportClosure,
    image: &OrdinaryExportImage,
    retained: &BTreeMap<FileId, FileManifest>,
    source: &ProviderTransitionIntentRecord,
    destination: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> Result<ReservePlan> {
    check_cancel(cancel)?;
    source.validate_fields()?;
    destination.validate_fields()?;
    source.require_prefix(prefix)?;
    closure.require_source_connection(c)?;
    if c.is_autocommit()
        || source.role != TransitionRole::Source
        || destination.role != TransitionRole::Destination
        || source.state != TransitionState::Staging
        || destination.state != TransitionState::Staging
        || source.local_generation != prefix.source.version().generation
        || destination.local_generation != 0
        || source.transaction_id != destination.transaction_id
        || source.plan != destination.plan
        || source.destination != destination.destination
        || source.reservation != destination.reservation
        || source.receipt_id.is_some()
        || destination.receipt_id.is_some()
        || closure.prefix_id() != prefix.digest()?
        || closure.closure() != &prefix.closure
    {
        return Err(invalid("ordinary reserve planning pair/closure differs"));
    }
    let current = database::current(c)?;
    let selected = selection::at(c, current.generation)?;
    let mappings = closure.copied_content_mappings(c, prefix, Some(cancel))?;
    let expected = include_selected_bases(c, &mappings, cancel)?;
    if retained.len() != expected.len() || retained.len() > 100_000 {
        return Err(invalid("ordinary reserve cached manifest union differs"));
    }
    let mut bytes = 0usize;
    for (file, (manifest, length)) in &expected {
        check_cancel(cancel)?;
        let value = retained
            .get(file)
            .ok_or_else(|| invalid("ordinary reserve cached representation missing"))?;
        require_private_mapping(c, value)?;
        if value.file_id() != file || value.id() != manifest || value.byte_length() != *length {
            return Err(invalid("ordinary reserve cached representation differs"));
        }
        bytes = bytes
            .checked_add(canonical_json(value).map_err(invalid)?.len())
            .filter(|bytes| *bytes <= 32 * 1024 * 1024)
            .ok_or_else(|| invalid("ordinary reserve cached metadata exceeds bound"))?;
    }
    let base_ids: BTreeSet<_> = current
        .snapshot
        .entries
        .iter()
        .filter(|entry| selected.includes(&entry.path))
        .filter_map(|entry| match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some(file_id.clone()),
            _ => None,
        })
        .collect();
    let bases = base_ids
        .iter()
        .map(|id| {
            retained
                .get(id)
                .ok_or_else(|| invalid("ordinary reserve base lacks exact private representation"))
        })
        .collect::<Result<Vec<_>>>()?;
    let historical = mappings
        .iter()
        .map(|(file, (manifest, bytes))| RequiredManifest {
            file_id: file.clone(),
            manifest_id: manifest.clone(),
            bytes: *bytes,
        })
        .collect::<Vec<_>>();
    let files = image
        .files()
        .iter()
        .map(|file| ImageFile {
            path: &file.path,
            manifest: &file.manifest,
            executable: file.executable,
            expected: file.expected.clone(),
        })
        .collect::<Vec<_>>();
    let deletions = image
        .deletions()
        .iter()
        .map(|file| ImageDeletion {
            path: &file.path,
            base: &file.base,
            executable: file.executable,
        })
        .collect::<Vec<_>>();
    let mut union = BTreeMap::new();
    for manifest in retained
        .values()
        .chain(image.files().iter().map(|file| &file.manifest))
    {
        check_cancel(cancel)?;
        union.insert(
            (manifest.file_id().clone(), manifest.id().clone()),
            manifest.clone(),
        );
    }
    let required = union.into_values().collect::<Vec<_>>();
    let derive = |metadata| {
        bootstrap::derive_ordinary_data(
            prefix,
            &current.snapshot,
            &selected,
            &prefix.source.version().selection_digest,
            &bases,
            &files,
            &deletions,
            prefix.destination_identity.clone(),
            &source.plan.destination_parent,
            &source.plan.destination_parent_identity,
            &source.plan.destination_leaf_nonce,
            &source.transaction_id,
            &historical,
            &required,
            metadata,
        )
    };
    let baseline = derive(0)?;
    let copied_bytes = closure.metadata_bytes(c, prefix, Some(cancel))?;
    let envelope = budget::planning_envelope(
        &baseline,
        &selected,
        &required,
        &files,
        &deletions,
        copied_bytes,
        cancel,
    )?;
    let data = derive(envelope.metadata_bytes())?;
    if data.plan != source.plan || data.validated_metadata_floor.is_some() {
        return Err(invalid("ordinary reserve immutable funded plan differs"));
    }
    let actual_metadata_floor = managed_budget::base_metadata_floor(
        copied_bytes,
        &data.observation,
        &data.placement,
        &data.capture,
        &data.plan,
        &selected,
        managed_budget::intent_size(source)?,
        managed_budget::intent_size(destination)?,
    )?;
    if actual_metadata_floor > envelope.base_metadata_bound() {
        return Err(invalid(
            "ordinary reserve actual metadata exceeds funded envelope",
        ));
    }
    check_cancel(cancel)?;
    Ok(ReservePlan {
        data,
        required,
        baseline_quota: baseline.quota_bytes,
        actual_metadata_floor,
    })
}
