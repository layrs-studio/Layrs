//! Immutable Ready history at an admitted boundary. Current ordinary heads,
//! claims, publications and operational prejournal belong to separate readers.
//! No family connection, canonical current manifest body or byte authority.
use super::super::{
    checkout_journal::{self, CheckoutJournal, ExpectedCheckout},
    import_validation::{DestinationOriginKind, VerifiedDestinationPrefix},
    managed_admission::{
        RequiredRepresentation,
        record::{ArchiveCommitment, ManagedAdmissionRecord},
        schema::{AdmittedLayout, AdmittedSchemaPermit},
    },
};
use super::*;
use serde::Serialize;

mod c14;

pub(in crate::projection::provider) struct FrozenReadyRows {
    pub required: Vec<RequiredRepresentation>,
    pub expected: ExpectedCheckout,
    pub journal: CheckoutJournal,
    pub archive: ArchiveCommitment,
    pub capture: CapturedPatch,
    pub new_content: Vec<RequiredRepresentation>,
    /// Family-independent Ready basis: base_metadata_floor_with_coverage plus
    /// actual local companion metadata. Excludes complete_metadata_floor's
    /// receipt/claim/current-Ready-intent/mapping columns and checkout Jmax. A caller
    /// must complete it with authenticated canonical manifests before budgeting.
    /// Present only for C14 terminal interpretation; legacy readers do not need it.
    pub metadata_basis_floor: Option<u64>,
}

pub(in crate::projection::provider) fn read_frozen_ready_rows(
    c: &Connection,
    marker: &format::Marker,
    prefix: &VerifiedDestinationPrefix<'_>,
    record: &ManagedAdmissionRecord,
    permit: &AdmittedSchemaPermit<'_>,
    outgoing: &super::super::ordinary_transition::OrdinaryOutgoing,
    cancel: Option<&Cancellation>,
) -> Result<FrozenReadyRows> {
    permit.require(c, marker, cancel)?;
    let ready = prefix.ready();
    let plan = &ready.plan;
    let version = prefix.prefix().source.version();
    if !matches!(
        (prefix.origin(), permit.layout()),
        (DestinationOriginKind::Provider, AdmittedLayout::Provider9)
            | (DestinationOriginKind::Ordinary, AdmittedLayout::Ordinary10)
    ) {
        return Err(invalid("admitted Ready frozen origin/layout differs"));
    }
    let current = c14::read_snapshots(c, prefix, permit, cancel)?;
    if current.is_none()
        && (plan.companion_id.is_some()
            || plan.companion_files.count != 0
            || plan.companion_deletions.count != 0)
    {
        return Err(invalid("admitted managed Ready companions unsupported"));
    }
    let (observation, placement, selected, selection_digest) = if let Some(rows) = &current {
        (
            &rows.observation,
            rows.placement.clone(),
            &rows.selected,
            rows.selection_digest.as_str(),
        )
    } else {
        match prefix.origin() {
            DestinationOriginKind::Provider => {
                let source = prefix.source()?;
                (
                    source.initial(),
                    super::super::records::activation::activation_snapshot(source)?,
                    source.selection(),
                    source.commitment().selection_digest.as_str(),
                )
            }
            DestinationOriginKind::Ordinary => {
                let source = prefix.ordinary_frozen()?;
                (
                    source.snapshot(),
                    source.snapshot().clone(),
                    source.selection(),
                    source.selection_digest(),
                )
            }
        }
    };
    let observation_bytes = observation
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?;
    let placement_bytes = placement
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?;
    if plan.observation.step_id != observation.base_step_id
        || plan.observation.root_tree_id != observation.base_root_tree_id
        || plan.observation.policy_set_id != observation.observed_policy_set_id
        || plan.observation.authorization_epoch != observation.observed_authorization_epoch
        || plan.observation.selection_digest != selection_digest
        || database::digest(
            plan.observation.generation,
            Some(&version.snapshot_digest),
            &observation_bytes,
        ) != plan.observation.snapshot_digest
        || database::digest(
            plan.placement_generation,
            Some(&plan.observation.snapshot_digest),
            &placement_bytes,
        ) != plan.placement_digest
    {
        return Err(invalid("admitted Ready snapshot commitments differ"));
    }
    for (generation, digest, parent, payload) in [
        (
            plan.observation.generation,
            &plan.observation.snapshot_digest,
            &version.snapshot_digest,
            &observation_bytes,
        ),
        (
            plan.placement_generation,
            &plan.placement_digest,
            &plan.observation.snapshot_digest,
            &placement_bytes,
        ),
    ] {
        cancellation::check(cancel)?;
        let selected_bytes = selection::encode(selected)?;
        let selected_digest = database::digest(generation, Some(digest), &selected_bytes);
        if !c.query_row("SELECT EXISTS(SELECT 1 FROM projection_snapshots WHERE generation=?1 AND digest=?2 AND parent_digest=?3 AND payload=?4)",params![generation,digest,parent,payload],|r|r.get::<_,bool>(0))?
            || !c.query_row("SELECT EXISTS(SELECT 1 FROM projection_selections WHERE generation=?1 AND digest=?2 AND payload=?3)",params![generation,selected_digest,selected_bytes],|r|r.get::<_,bool>(0))? {
            return Err(invalid("admitted Ready frozen snapshot or selection differs"));
        }
    }
    let closure = &prefix.prefix().closure;
    for (table, key, upper, expected) in [
        (
            "projection_snapshots",
            "generation",
            plan.placement_generation,
            closure.snapshots.count.checked_add(2),
        ),
        (
            "projection_selections",
            "generation",
            plan.placement_generation,
            closure.selections.count.checked_add(2),
        ),
        (
            "projection_captures",
            "sequence",
            plan.capture_sequence,
            closure.captures.count.checked_add(1),
        ),
        // C_ready is local and may acquire an ordinary completed publication.
        (
            "projection_publications",
            "sequence",
            version.capture_count,
            Some(closure.publications.count),
        ),
    ] {
        let expected = expected
            .filter(|n| *n <= 1_100_010)
            .ok_or_else(|| invalid("admitted frozen row bound overflow"))?;
        let count: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM {table} WHERE {key}<=?1 LIMIT ?2)"),
            params![upper, expected + 1],
            |r| r.get(0),
        )?;
        if count != expected {
            return Err(invalid("admitted frozen history has missing or extra rows"));
        }
    }
    for (table, expected) in [
        ("projection_provider_catalogs", closure.catalogs.count),
        ("projection_provider_descriptors", closure.descriptors.count),
        (
            "projection_provider_activations",
            closure.activation_maps.count,
        ),
        (
            "projection_provider_transition_receipts",
            closure
                .transition_receipts
                .count
                .checked_add(1)
                .and_then(|n| n.checked_add(outgoing.receipt_count()))
                .ok_or_else(|| invalid("admitted receipt count overflow"))?,
        ),
        (
            "projection_observations",
            closure
                .denial_observations
                .as_ref()
                .map_or(0, |rows| rows.count)
                .checked_add(u64::from(current.is_some()))
                .ok_or_else(|| invalid("admitted denial document count overflow"))?,
        ),
        (
            "projection_provider_companions",
            closure
                .companions
                .count
                .checked_add(u64::from(current.is_some() && plan.companion_id.is_some()))
                .ok_or_else(|| invalid("admitted companion count overflow"))?,
        ),
        (
            "projection_provider_companion_files",
            closure
                .companion_files
                .count
                .checked_add(if current.is_some() {
                    plan.companion_files.count
                } else {
                    0
                })
                .ok_or_else(|| invalid("admitted companion files count overflow"))?,
        ),
        (
            "projection_provider_companion_deletions",
            closure
                .companion_deletions
                .count
                .checked_add(if current.is_some() {
                    plan.companion_deletions.count
                } else {
                    0
                })
                .ok_or_else(|| invalid("admitted companion deletions count overflow"))?,
        ),
        ("projection_provider_mounts", 0),
        ("projection_provider_capture_transactions", 0),
        ("projection_provider_capture_receipts", 0),
        ("projection_conversion", 0),
    ] {
        // Ordinary offline observations are a mutable tail in legacy stores.
        // Only C14 terminal mode requires the complete table to be immutable.
        if table == "projection_observations" && current.is_none() {
            continue;
        }
        cancellation::check(cancel)?;
        let count: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT ?1)"),
            [expected
                .checked_add(1)
                .filter(|n| *n <= 1_100_011)
                .ok_or_else(|| invalid("admitted immutable row bound overflow"))?],
            |r| r.get(0),
        )?;
        if count != expected {
            return Err(invalid("admitted immutable provider inventory differs"));
        }
    }
    let head: (u64, Option<u64>) = c.query_row(
        "SELECT count(*),max(generation) FROM projection_head",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if head.0 != 1 || head.1.is_none_or(|g| g < plan.placement_generation) {
        return Err(invalid("admitted current head predates Ready"));
    }
    let capture = crate::projection::capture::storage::load_metadata_for_origin(
        c,
        plan.capture_sequence,
        &marker.identity,
        plan.placement_generation,
    )?;
    if capture.previous_patch_digest != version.capture_head
        || capture.projection_generation != plan.placement_generation
        || capture.projection_digest != plan.placement_digest
        || capture.digest != plan.capture_digest
    {
        return Err(invalid("admitted Ready frozen capture differs"));
    }
    let deletions: Vec<_> = capture
        .changes
        .iter()
        .filter(|change| change.replacement.is_none())
        .map(|change| change.path.clone())
        .collect();
    let (expected, journal, archive) =
        checkout_journal::read_archived(c, ready, &deletions, permit, cancel)?;
    if archive != record.archive {
        return Err(invalid("admitted checkout archive commitment differs"));
    }
    let copied = prefix.copied_content_mappings(c, cancel)?;
    let mut inherited = prefix.inherited_companion_representations(c, cancel)?;
    if current.is_some() {
        inherited.extend(c14::local_preserved(c, prefix, cancel)?);
    }
    let required = match prefix.origin() {
        DestinationOriginKind::Provider => {
            required(prefix.source()?, &copied, &expected, &inherited, cancel)?
        }
        DestinationOriginKind::Ordinary => {
            let rows =
                super::super::managed_admission::representations::read_import_representations(
                    c, permit, ready, cancel,
                )?;
            super::autonomous::inventory::require_ordinary_union(
                prefix.ordinary_frozen()?,
                &copied,
                &expected,
                &inherited,
                &rows,
                cancel,
            )?;
            rows
        }
    };
    if plan.protected_manifests.count != required.len() as u64
        || plan.protected_manifests.digest
            != rowset(
                b"layrs-provider-managed-protected-manifests-v1\0",
                &required,
                cancel,
            )?
    {
        return Err(invalid(
            "admitted exact required representation union differs",
        ));
    }
    let retained = c14::read_retained(
        c,
        marker,
        prefix,
        &required,
        &expected,
        current.as_ref(),
        cancel,
    )?;
    require_inventory(
        prefix,
        &placement,
        selected,
        current.is_some(),
        &capture,
        &expected,
        &required,
        cancel,
    )?;
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Overlay<'a> {
        change: &'a crate::projection::capture::CapturedChange,
        manifest_id: Option<&'a FileManifestId>,
    }
    let mut overlay = Vec::with_capacity(capture.changes.len());
    for change in &capture.changes {
        cancellation::check(cancel)?;
        let manifest_id = match &change.replacement {
            Some(_) => Some(
                &expected.files()[expected
                    .files()
                    .binary_search_by(|file| file.path.cmp(&change.path))
                    .map_err(|_| invalid("admitted overlay file absent"))?]
                .manifest_id,
            ),
            None => None,
        };
        overlay.push(Overlay {
            change,
            manifest_id,
        });
    }
    if plan.working_overlay.count != overlay.len() as u64
        || plan.working_overlay.digest
            != rowset(b"layrs-provider-managed-overlay-v1\0", &overlay, cancel)?
    {
        return Err(invalid("admitted Ready overlay commitment differs"));
    }
    if current.is_none() {
        super::autonomous::inventory::require_image(
            &capture,
            &expected,
            &version.effective_image_digest,
            cancel,
        )?;
    }
    let mut mapped = BTreeSet::new();
    let mut new_content = Vec::new();
    for row in &required {
        if !copied.contains_key(&row.file_id) && mapped.insert(&row.file_id) {
            cancellation::check(cancel)?;
            let row = retained
                .as_ref()
                .and_then(|local| local.mapping(&row.file_id))
                .map_or_else(
                    || row.clone(),
                    |(manifest_id, bytes)| RequiredRepresentation {
                        file_id: row.file_id.clone(),
                        manifest_id: manifest_id.clone(),
                        bytes: *bytes,
                    },
                );
            if required.binary_search(&row).is_err() {
                return Err(invalid("admitted companion sticky mapping outside union"));
            }
            if !c.query_row("SELECT EXISTS(SELECT 1 FROM projection_content WHERE file_id=?1 AND manifest_id=?2 AND bytes=?3)",params![row.file_id.as_str(),row.manifest_id.as_str(),row.bytes],|r|r.get::<_,bool>(0))? {return Err(invalid("admitted Ready content mapping differs"));}
            new_content.push(row.clone());
        }
    }
    // Preserve this exact claim among later ordinary claims. Never require a
    // singleton claim set or reinterpret an ordinary operation's prejournal.
    if !c.query_row("SELECT EXISTS(SELECT 1 FROM projection_claims WHERE root_id=?1 AND digest=?1 AND kind='provider-import' AND owner=?2)",params![record.root_id,record.transaction_id],|r|r.get::<_,bool>(0))? {return Err(invalid("admitted import retention claim differs"));}
    let metadata_basis_floor = if current.is_some() {
        Some(c14::metadata_basis_floor(
            c,
            prefix,
            observation,
            &placement,
            selected,
            &capture,
            retained.as_ref(),
            cancel,
        )?)
    } else {
        None
    };
    Ok(FrozenReadyRows {
        required,
        expected,
        journal,
        archive,
        capture,
        new_content,
        metadata_basis_floor,
    })
}

fn rowset<T: Serialize>(
    domain: &[u8],
    rows: &[T],
    cancel: Option<&Cancellation>,
) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(domain);
    hash.update(&(rows.len() as u64).to_be_bytes());
    for row in rows {
        cancellation::check(cancel)?;
        let bytes = canonical_json(row).map_err(|e| invalid(e.to_string()))?;
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(&bytes);
    }
    Ok(format!("blake3:{}", hash.finalize()))
}

fn required(
    source: &ProviderBootstrapSource,
    copied: &BTreeMap<FileId, (FileManifestId, u64)>,
    expected: &ExpectedCheckout,
    inherited: &[RequiredRepresentation],
    cancel: Option<&Cancellation>,
) -> Result<Vec<RequiredRepresentation>> {
    let mut rows = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut add = |file: &FileId, manifest: &FileManifestId, bytes: u64| -> Result<()> {
        cancellation::check(cancel)?;
        if bytes > 1073741824
            || lengths
                .insert(file.clone(), bytes)
                .is_some_and(|old| old != bytes)
        {
            return Err(invalid("admitted representation FileId length differs"));
        }
        let key = (file.clone(), manifest.clone());
        if rows.len() >= 100_000 && !rows.contains_key(&key) {
            return Err(invalid("admitted representation count exceeds bound"));
        }
        if rows.insert(key, bytes).is_some_and(|old| old != bytes) {
            return Err(invalid("admitted exact representation length differs"));
        }
        Ok(())
    };
    for d in source.descriptors() {
        add(&d.file_id, &d.manifest_id, d.bytes)?;
    }
    for (file, (manifest, bytes)) in copied {
        add(file, manifest, *bytes)?;
    }
    for file in expected.files() {
        add(&file.file_id, &file.manifest_id, file.bytes)?;
    }
    for row in inherited {
        add(&row.file_id, &row.manifest_id, row.bytes)?;
    }
    Ok(rows
        .into_iter()
        .map(|((file_id, manifest_id), bytes)| RequiredRepresentation {
            file_id,
            manifest_id,
            bytes,
        })
        .collect())
}

fn require_inventory(
    prefix: &VerifiedDestinationPrefix<'_>,
    placement: &ProjectionSnapshot,
    selected: &layrs_core::projection::ProjectionSelection,
    fresh: bool,
    capture: &CapturedPatch,
    expected: &ExpectedCheckout,
    required: &[RequiredRepresentation],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    use layrs_core::{projection::EntryState, scm::TreeEntryTarget};
    let lengths: BTreeMap<_, _> = required
        .iter()
        .map(|row| (&row.file_id, row.bytes))
        .collect();
    let mut files = 0usize;
    let mut deletions = 0usize;
    let mut changes = 0usize;
    for entry in &placement.entries {
        cancellation::check(cancel)?;
        if !selected.includes(&entry.path) || (fresh && entry.state == EntryState::Reserved) {
            continue;
        }
        let base = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => return Err(invalid("admitted Ready selected base differs")),
        };
        let changed = capture
            .changes
            .binary_search_by(|change| change.path.cmp(&entry.path))
            .ok();
        let actual = if let Some(n) = changed {
            changes += 1;
            let change = &capture.changes[n];
            if !entry.writable
                || change.expected.as_ref().map(|f| (&f.file_id, f.executable)) != base
            {
                return Err(invalid("admitted Ready patch preimage differs"));
            }
            match &change.replacement {
                Some(file) => Some((&file.file_id, file.executable, file.bytes)),
                None => {
                    if base.is_none() || expected.deletions().get(deletions) != Some(&entry.path) {
                        return Err(invalid("admitted Ready deletion inventory differs"));
                    }
                    deletions += 1;
                    None
                }
            }
        } else if let Some((file_id, executable)) = base {
            let bytes = match prefix.origin() {
                DestinationOriginKind::Provider if fresh => *lengths
                    .get(file_id)
                    .ok_or_else(|| invalid("admitted fresh unchanged representation absent"))?,
                DestinationOriginKind::Provider => {
                    let source = prefix.source()?;
                    let descriptor = source
                        .descriptors()
                        .binary_search_by(|d| d.path.cmp(&entry.path))
                        .ok()
                        .and_then(|n| source.descriptors().get(n))
                        .filter(|d| &d.file_id == file_id && d.executable == executable)
                        .ok_or_else(|| invalid("admitted Ready unchanged descriptor absent"))?;
                    descriptor.bytes
                }
                DestinationOriginKind::Ordinary => *lengths
                    .get(file_id)
                    .ok_or_else(|| invalid("admitted ordinary unchanged representation absent"))?,
            };
            Some((file_id, executable, bytes))
        } else {
            None
        };
        if let Some((file_id, executable, bytes)) = actual {
            let file = expected
                .files()
                .get(files)
                .ok_or_else(|| invalid("admitted Ready file inventory missing"))?;
            if file.path != entry.path
                || &file.file_id != file_id
                || file.executable != executable
                || file.bytes != bytes
            {
                return Err(invalid("admitted Ready file inventory differs"));
            }
            files += 1;
        }
    }
    if files != expected.files().len()
        || deletions != expected.deletions().len()
        || changes != capture.changes.len()
    {
        return Err(invalid(
            "admitted Ready inventory has missing or extra paths",
        ));
    }
    Ok(())
}
