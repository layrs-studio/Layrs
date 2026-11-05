//! Closed Provider8 -> Provider9 frozen denial DATA. No source/family authority,
//! current manifest bodies, mutable operation dispatch or admission capability.
use super::*;
use crate::projection::{offline, provider::managed_budget};
use layrs_core::projection::ProjectionSelection;

pub(super) struct Snapshots {
    pub observation: ProjectionSnapshot,
    pub placement: ProjectionSnapshot,
    pub selected: ProjectionSelection,
    pub selection_digest: String,
}

pub(super) fn read_snapshots(
    c: &Connection,
    prefix: &VerifiedDestinationPrefix<'_>,
    permit: &AdmittedSchemaPermit<'_>,
    cancel: Option<&Cancellation>,
) -> Result<Option<Snapshots>> {
    let ready = prefix.ready();
    let plan = &ready.plan;
    if plan.denial_coverage.is_none() {
        return Ok(None);
    }
    if prefix.origin() != DestinationOriginKind::Provider
        || permit.layout() != AdmittedLayout::Provider9
        || !matches!(
            prefix.prefix().source,
            ImportSource::Provider {
                private_schema: 8,
                ..
            }
        )
    {
        return Err(invalid("frozen denial requires Provider8 to Provider9"));
    }
    prefix.require_destination_connection(c)?;
    let version = prefix.prefix().source.version();
    if version.generation.checked_add(1) != Some(plan.observation.generation)
        || plan.observation.generation.checked_add(1) != Some(plan.placement_generation)
        || version.capture_count.checked_add(1) != Some(plan.capture_sequence)
    {
        return Err(invalid("frozen denial local boundaries differ"));
    }
    let observation = offline::snapshot_at(
        c,
        plan.observation.generation,
        Some(&plan.observation.snapshot_digest),
    )?
    .snapshot;
    let placement =
        offline::snapshot_at(c, plan.placement_generation, Some(&plan.placement_digest))?.snapshot;
    let selected = selection::at(c, plan.observation.generation)?;
    if selection::at(c, plan.placement_generation)? != selected
        || !selected.is_expansion_of(prefix.source()?.selection())
    {
        return Err(invalid("frozen denial selection differs or shrank"));
    }
    selected
        .validate_snapshot(&observation)
        .map_err(|e| invalid(e.to_string()))?;
    selected
        .validate_snapshot(&placement)
        .map_err(|e| invalid(e.to_string()))?;
    let selection_digest = database::digest(
        plan.observation.generation,
        Some(&plan.observation.snapshot_digest),
        &selection::encode(&selected)?,
    );
    let body = super::super::super::destination_coverage::read_current(c, ready, cancel)?
        .ok_or_else(|| invalid("frozen denial current body absent"))?;
    // The current canonical body must describe these actual rows, including
    // denied reservations. A valid plan/reference alone is insufficient.
    body.require_observation_rows(
        &observation,
        &selected,
        plan.observation.generation,
        &plan.observation.snapshot_digest,
        &selection_digest,
        cancel,
    )?;
    Ok(Some(Snapshots {
        observation,
        placement,
        selected,
        selection_digest,
    }))
}

pub(super) fn local_preserved(
    c: &Connection,
    prefix: &VerifiedDestinationPrefix<'_>,
    cancel: Option<&Cancellation>,
) -> Result<Vec<RequiredRepresentation>> {
    use crate::projection::provider::{
        records::companion_v2::CompanionHeaderRecord, transition_reconcile::source_sql,
    };
    let Some(id) = &prefix.ready().plan.companion_id else {
        return Ok(Vec::new());
    };
    let (header, _, _) = source_sql::read_group(c, id, cancel)?;
    let CompanionHeaderRecord::V2(header) = header else {
        return Err(invalid(
            "frozen denial companion requires real v2 provenance",
        ));
    };
    header.require_prefix(prefix.prefix())?;
    header.require_receipt(prefix.receipt())?;
    Ok(header
        .preserved_representations
        .into_iter()
        .map(|r| RequiredRepresentation {
            file_id: r.file_id,
            manifest_id: r.manifest_id,
            bytes: r.bytes,
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_retained(
    c: &Connection,
    marker: &format::Marker,
    prefix: &VerifiedDestinationPrefix<'_>,
    required: &[RequiredRepresentation],
    expected: &ExpectedCheckout,
    snapshots: Option<&Snapshots>,
    cancel: Option<&Cancellation>,
) -> Result<Option<super::super::retained_companion::RetainedCompanionReadyData>> {
    let Some(snapshots) = snapshots else {
        return Ok(None);
    };
    if prefix.ready().plan.companion_id.is_some() {
        let retained =
            super::super::retained_companion::RetainedCompanionReadyData::read_representations(
                c, marker, prefix, required, expected, cancel,
            )?;
        retained.require_frozen(
            prefix.ready(),
            prefix.receipt(),
            prefix.prefix(),
            &snapshots.observation,
            &snapshots.placement,
            cancel,
        )?;
        retained.require_rows(c, cancel)?;
        Ok(Some(retained))
    } else {
        let retained = super::super::denial::RetainedDenialReadyData::read_representations(
            c, prefix, required, expected, cancel,
        )?;
        retained.require_frozen(
            prefix.ready(),
            &snapshots.observation,
            &snapshots.placement,
            cancel,
        )?;
        Ok(None)
    }
}

/// Same pre-completion basis as the autonomous Ready reader. Copied metadata
/// charges inherited documents; only this transition's document is added here.
#[allow(clippy::too_many_arguments)]
pub(super) fn metadata_basis_floor(
    c: &Connection,
    prefix: &VerifiedDestinationPrefix<'_>,
    observation: &ProjectionSnapshot,
    placement: &ProjectionSnapshot,
    selected: &ProjectionSelection,
    capture: &CapturedPatch,
    companion: Option<&super::super::retained_companion::RetainedCompanionReadyData>,
    cancel: Option<&Cancellation>,
) -> Result<u64> {
    let ready = prefix.ready();
    let (source_size, destination_size) = managed_budget::original_staging_sizes(ready)?;
    let body = super::super::super::destination_coverage::read_current(c, ready, cancel)?;
    let coverage = body
        .as_ref()
        .map(|body| managed_budget::DeniedCoverageCost::new(ready, body, cancel))
        .transpose()?;
    let base = managed_budget::base_metadata_floor_with_coverage(
        prefix.metadata_bytes(c, cancel)?,
        observation,
        placement,
        capture,
        &ready.plan,
        selected,
        source_size,
        destination_size,
        coverage.as_ref(),
    )?;
    match companion {
        Some(rows) => managed_budget::add_metadata(base, rows.metadata_bytes()?),
        None => Ok(base),
    }
}
