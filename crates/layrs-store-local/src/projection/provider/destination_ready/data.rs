//! Bounded immutable expected DATA for the one managed Ready suffix.
use super::super::records::import::ProviderImportKind;
use super::*;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequiredRow<'a> {
    file_id: &'a FileId,
    manifest_id: &'a FileManifestId,
    bytes: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayRow<'a> {
    change: &'a crate::projection::capture::CapturedChange,
    manifest_id: Option<&'a FileManifestId>,
}

fn rowset<T: Serialize>(domain: &[u8], values: &[T]) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(domain);
    hash.update(&(values.len() as u64).to_be_bytes());
    for value in values {
        let bytes = canonical_json(value).map_err(|e| invalid(e.to_string()))?;
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(&bytes);
    }
    Ok(format!("blake3:{}", hash.finalize()))
}

pub(super) fn prepare(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<PreparedData> {
    prepare_at_member(
        source_c,
        destination,
        family_c,
        inputs,
        family::MemberPhase::Initializing,
        cancel,
    )
}

pub(super) fn prepare_at_member(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    member_phase: family::MemberPhase,
    cancel: Option<&Cancellation>,
) -> Result<PreparedData> {
    cancellation::check(cancel)?;
    if source_c.is_autocommit()
        || destination.is_autocommit()
        || family_c.is_autocommit()
        || std::ptr::eq(source_c, destination)
        || std::ptr::eq(source_c, family_c)
        || std::ptr::eq(destination, family_c)
    {
        return Err(invalid(
            "destination Ready requires three distinct retained transactions",
        ));
    }
    inputs.closure.require_source_connection(source_c)?;
    super::super::import_validation::require_schema(
        destination,
        inputs.marker,
        super::super::import_validation::ImportDatabaseKind::Provider,
        cancel,
    )?;
    let staging = inputs.staging;
    let ready = inputs.data.ready;
    staging.validate_fields()?;
    ready.require_transition(staging)?;
    if staging.role != TransitionRole::Destination
        || staging.state != TransitionState::Staging
        || ready.state != TransitionState::Ready
        || staging.receipt_id.is_some()
        || staging.reservation.as_deref() != Some(staging.reservation_operation()?.as_str())
        || staging.plan.kind != ProviderImportKind::ManagedOrdinary
        || staging.plan.source_prefix != *inputs.prefix
        || staging.plan.companion_id.is_some() != inputs.companion.is_some()
        || (inputs.denial.is_some() && inputs.companion.is_some())
        || (inputs.denial.is_some() && staging.plan.denial_coverage.is_none())
        || (staging.plan.denial_coverage.is_some()
            && inputs.companion.is_none()
            && inputs.denial.is_none())
    {
        return Err(invalid("destination Ready staged plan differs"));
    }
    // Reuse the strict FORMAT/source binding kernel without relaxing the
    // Staging reader or querying this destination as an earlier phase.
    let mut copying = ProviderTransitionIntentRecord::decode(&staging.encode()?)?;
    copying.state = TransitionState::Copying;
    copying.reservation = None;
    destination_validation::require_inputs(
        inputs.marker,
        inputs.source,
        inputs.allocation,
        inputs.prepared,
        inputs.activation,
        inputs.prefix,
        &copying,
    )?;
    if inputs.closure.prefix_id() != inputs.prefix.digest()?
        || inputs.closure.closure() != &inputs.prefix.closure
    {
        return Err(invalid("destination Ready source closure differs"));
    }
    ready.require_receipt(inputs.data.receipt)?;
    if let Some(body) =
        super::super::destination_coverage::read_current(destination, staging, cancel)?
    {
        body.require_observation_rows(
            inputs.data.observation,
            inputs.view().selection(),
            staging.plan.observation.generation,
            &staging.plan.observation.snapshot_digest,
            &staging.plan.observation.selection_digest,
            cancel,
        )?;
    }
    if let Some(companion) = &inputs.companion {
        inputs
            .closure
            .require_publication_inputs(companion.source_context.publication)?;
        let full = companion.source_context.publication;
        if !std::ptr::eq(inputs.source, &full.active.source)
            || !std::ptr::eq(inputs.activation, &full.active.activation)
        {
            return Err(invalid("companion Ready complete source proof differs"));
        }
        let root = companion
            .source_context
            .pending
            .ok_or_else(|| invalid("companion Ready exact pending B root absent"))?;
        let source_staging = full
            .source_transitions()
            .iter()
            .find(|i| i.transaction_id == staging.transaction_id)
            .ok_or_else(|| invalid("companion Ready original source transition absent"))?;
        if source_staging.plan != staging.plan {
            return Err(invalid("companion Ready original source plan differs"));
        }
        root.require_plan(inputs.allocation, source_staging, cancel)?;
        root.require_union(inputs.data.required, cancel)?;
        crate::projection::shared::source_companion::classify_source_groups(
            family_c,
            inputs.allocation,
            Some(&companion.source_context),
            cancel,
        )?;
        if root.inspect(family_c, cancel)?
            != crate::projection::shared::source_companion::SourceCompanionPresence::Protected
        {
            return Err(invalid("companion Ready requires actual B protected root"));
        }
    }
    if let Some(mode) = &inputs.denial {
        if mode.source_context.pending.is_some() {
            return Err(invalid("plain denial Ready refuses pending companion root"));
        }
        let full = mode.source_context.publication;
        inputs.closure.require_publication_inputs(full)?;
        if !std::ptr::eq(inputs.source, &full.active.source)
            || !std::ptr::eq(inputs.activation, &full.active.activation)
        {
            return Err(invalid("plain denial Ready complete source proof differs"));
        }
        let source = full
            .source_transitions()
            .iter()
            .find(|i| i.transaction_id == staging.transaction_id)
            .ok_or_else(|| invalid("plain denial Ready source transition absent"))?;
        if source.plan != staging.plan || source.destination != staging.destination {
            return Err(invalid("plain denial Ready source plan differs"));
        }
        mode.coverage.record().require_intent(source, cancel)?;
        let actual = super::super::denied_coverage_storage::read_required(
            source_c,
            source
                .plan
                .denial_coverage
                .as_ref()
                .ok_or_else(|| invalid("plain denial Ready source body reference absent"))?,
            cancel,
        )?;
        if actual.encode()? != mode.coverage.record().encode()? {
            return Err(invalid("plain denial Ready actual source body differs"));
        }
        let funding =
            crate::projection::shared::source_denial::SourceDeniedCoverageReservation::new(
                inputs.allocation,
                source,
                &actual,
                cancel,
            )?;
        if !full.capture.denial_reservations().contains(&funding) {
            return Err(invalid("plain denial Ready full source funding differs"));
        }
        funding.require_retained(family_c, cancel)?;
        crate::projection::shared::source_companion::classify_source_groups(
            family_c,
            inputs.allocation,
            Some(&mode.source_context),
            cancel,
        )?;
    }
    let copied = inputs
        .closure
        .copied_content_mappings(source_c, inputs.prefix, cancel)?;
    let prepared = prepare_suffix(&inputs.view(), &copied, cancel)?;
    match member_phase {
        family::MemberPhase::Initializing => family::require(family_c, inputs, cancel)?,
        family::MemberPhase::Ready => {
            family::require_at_member(family_c, inputs, member_phase, cancel)?
        }
    }
    Ok(prepared)
}

pub(super) fn prepare_suffix(
    inputs: &ReadyView<'_>,
    copied: &BTreeMap<FileId, (FileManifestId, u64)>,
    cancel: Option<&Cancellation>,
) -> Result<PreparedData> {
    cancellation::check(cancel)?;
    let ready = inputs.data.ready;
    ready.require_prefix(inputs.prefix)?;
    ready.require_receipt(inputs.data.receipt)?;
    if ready.role != TransitionRole::Destination
        || ready.state != TransitionState::Ready
        || ready.plan.kind != ProviderImportKind::ManagedOrdinary
        || ready.plan.companion_id.is_some() != inputs.has_companion()
        || ready
            .destination
            .as_ref()
            .is_none_or(|proof| &proof.format != inputs.marker)
    {
        return Err(invalid("destination Ready suffix binding differs"));
    }
    let version = inputs.source.version(inputs.prefix)?;
    inputs.require_companion(cancel)?;
    let plan = &inputs.data.ready.plan;
    // v1 commits the source selection; v2 commits the selection at the fresh
    // observation's own generation/snapshot, as derived by the normalizer.
    let selection_digest = if plan.denial_coverage.is_some() {
        database::digest(
            plan.observation.generation,
            Some(&plan.observation.snapshot_digest),
            &selection::encode(inputs.selection())?,
        )
    } else {
        inputs.source.selection_digest(inputs.prefix)?
    };
    if version.generation.checked_add(1) != Some(plan.observation.generation)
        || plan.observation.generation.checked_add(1) != Some(plan.placement_generation)
        || version.capture_count.checked_add(1) != Some(plan.capture_sequence)
        || plan.observation.step_id != inputs.data.observation.base_step_id
        || plan.observation.root_tree_id != inputs.data.observation.base_root_tree_id
        || plan.observation.policy_set_id != inputs.data.observation.observed_policy_set_id
        || plan.observation.authorization_epoch
            != inputs.data.observation.observed_authorization_epoch
        || plan.observation.selection_digest != selection_digest
    {
        return Err(invalid(
            "destination Ready local boundaries or snapshots differ",
        ));
    }
    let observation = inputs
        .data
        .observation
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?;
    let placement = inputs
        .data
        .placement
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?;
    if observation.len() > 64 * 1024 * 1024
        || placement.len() > 64 * 1024 * 1024
        || database::digest(
            plan.observation.generation,
            Some(&version.snapshot_digest),
            &observation,
        ) != plan.observation.snapshot_digest
        || database::digest(
            plan.placement_generation,
            Some(&plan.observation.snapshot_digest),
            &placement,
        ) != plan.placement_digest
    {
        return Err(invalid("destination Ready snapshot commitments differ"));
    }
    inputs
        .selection()
        .validate_snapshot(inputs.data.observation)
        .map_err(|e| invalid(e.to_string()))?;
    inputs
        .selection()
        .validate_snapshot(inputs.data.placement)
        .map_err(|e| invalid(e.to_string()))?;
    let patch = inputs.data.capture;
    if patch.schema_version != "layrs.projection-capture.v1"
        || patch.changes.len() > 1024
        || patch.changes.iter().any(|change| {
            change.path.as_str().len() > 4096
                || change
                    .replacement
                    .as_ref()
                    .is_some_and(|file| file.bytes > 1073741824)
        })
        || patch.sequence != plan.capture_sequence
        || patch.previous_patch_digest != version.capture_head
        || patch.projection_generation != plan.placement_generation
        || patch.projection_digest != plan.placement_digest
        || patch.identity != inputs.marker.identity
        || patch.digest != plan.capture_digest
        || patch.computed_digest()? != patch.digest
        || inputs.data.overlay.len() != patch.changes.len()
    {
        return Err(invalid("destination Ready capture commitment differs"));
    }
    let capture = canonical_json(patch).map_err(|e| invalid(e.to_string()))?;
    if capture.len() > 8 * 1024 * 1024 || inputs.data.required.len() > 100_000 {
        return Err(invalid("destination Ready DATA exceeds bound"));
    }
    let mut required = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    // Same inline-root frame as ExactProtection::new and the ordinary planner's
    // root_payload_bound: both already enforce 65536 + sum(payload + 1) <=16MiB.
    // The separate 32MiB source/cache bound does not enlarge a protectable union.
    let mut encoded_total = 65536usize;
    for manifest in inputs.data.required {
        cancellation::check(cancel)?;
        let payload = crate::projection::shared::exact_content::canonical_manifest_bytes(manifest)?;
        encoded_total = encoded_total
            .checked_add(payload.len() + 1)
            .filter(|n| *n <= 16 * 1024 * 1024)
            .ok_or_else(|| invalid("destination Ready protected union exceeds bound"))?;
        if payload.len() > 262144
            || manifest.byte_length() > 1073741824
            || manifest.chunks().len() > 1024
            || manifest.chunks().iter().any(|c| c.byte_length() > 1048576)
            || lengths
                .insert(manifest.file_id().clone(), manifest.byte_length())
                .is_some_and(|old| old != manifest.byte_length())
            || required
                .insert((manifest.file_id(), manifest.id()), manifest)
                .is_some()
        {
            return Err(invalid("destination Ready manifest representation differs"));
        }
    }
    let rows: Vec<_> = required
        .values()
        .map(|m| RequiredRow {
            file_id: m.file_id(),
            manifest_id: m.id(),
            bytes: m.byte_length(),
        })
        .collect();
    if plan.protected_manifests.count != rows.len() as u64
        || plan.protected_manifests.digest
            != rowset(b"layrs-provider-managed-protected-manifests-v1\0", &rows)?
    {
        return Err(invalid("destination Ready protected union differs"));
    }
    inputs.source.require_base_union(&required, cancel)?;
    let mut overlay = Vec::with_capacity(patch.changes.len());
    let mut prior = None;
    for (change, supplied) in patch.changes.iter().zip(inputs.data.overlay) {
        cancellation::check(cancel)?;
        if change.path != supplied.path || prior.is_some_and(|path: &RepoPath| path >= &change.path)
        {
            return Err(invalid("destination Ready overlay paths differ"));
        }
        prior = Some(&change.path);
        match (&change.replacement, &supplied.manifest_id) {
            (Some(file), Some(id))
                if required
                    .get(&(&file.file_id, id))
                    .is_some_and(|manifest| manifest.byte_length() == file.bytes) => {}
            (None, None) => {}
            _ => return Err(invalid("destination Ready overlay representation absent")),
        }
        overlay.push(OverlayRow {
            change,
            manifest_id: supplied.manifest_id.as_ref(),
        });
    }
    if plan.working_overlay.count != overlay.len() as u64
        || plan.working_overlay.digest != rowset(b"layrs-provider-managed-overlay-v1\0", &overlay)?
    {
        return Err(invalid("destination Ready overlay commitment differs"));
    }
    for (file, (manifest, bytes)) in copied {
        if !required
            .get(&(file, manifest))
            .is_some_and(|m| m.byte_length() == *bytes)
        {
            return Err(invalid(
                "destination Ready imported content representation absent",
            ));
        }
    }
    let mut new_content = Vec::new();
    let mut mapped = BTreeSet::new();
    for manifest in required.values() {
        if !copied.contains_key(manifest.file_id()) && mapped.insert(manifest.file_id()) {
            let (id, bytes) = inputs
                .companion_mapping(manifest.file_id())
                .map_or((manifest.id(), manifest.byte_length()), |(id, bytes)| {
                    (id, *bytes)
                });
            if !required
                .get(&(manifest.file_id(), id))
                .is_some_and(|m| m.byte_length() == bytes)
            {
                return Err(invalid("companion Ready sticky mapping outside full union"));
            }
            new_content.push((manifest.file_id().to_string(), id.to_string(), bytes));
        }
    }
    cancellation::check(cancel)?;
    Ok(PreparedData {
        observation,
        placement,
        capture,
        new_content,
    })
}
