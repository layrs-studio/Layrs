//! Schema-8 ordinary import ingress before any immutable prefix copy. This
//! validates DATA only; pinned files, family Initializing and source ownership
//! remain caller obligations. It grants neither ordinary admission nor Ready.
use super::{
    cancellation, checkout_journal, database_io,
    import_validation::{self, ImportDatabaseKind, VerifiedImportClosure},
    records::{
        AllocationRecord, PreparedRecord, ProviderBootstrapSource, Record,
        activation::ProviderActivationRecord,
        import::{
            ImportSource, ImportSourceReferences, ProviderImportKind, ProviderImportPrefixRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionRole,
            TransitionRowData, TransitionState,
        },
    },
    recovery::error::{Result, invalid},
};
use crate::projection::{database, format, selection};
use layrs_vfs::Cancellation;
use rusqlite::Connection;

pub(in crate::projection::provider) struct EmptyIngress;
pub(in crate::projection::provider) struct CopiedPrefix;
pub(in crate::projection::provider) struct CopiedPrefixStaging;

#[derive(Clone, Copy)]
enum IngressPhase<'a> {
    Copying,
    Staging,
    CheckoutStaging(&'a checkout_journal::ExpectedCheckout),
}

pub(super) fn require_inputs(
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
) -> Result<()> {
    require_inputs_for_phase(
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        IngressPhase::Copying,
    )
}

#[allow(clippy::too_many_arguments)]
fn require_inputs_for_phase(
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    phase: IngressPhase<'_>,
) -> Result<()> {
    marker.validate()?;
    let source_identity = &allocation.format.identity;
    let source_shared = allocation
        .format
        .shared
        .as_ref()
        .ok_or_else(|| invalid("source has no shared family"))?;
    let destination_shared = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("destination has no shared family"))?;
    if marker.identity == *source_identity
        || marker.identity.store_id != source_identity.store_id
        || marker.identity.space_id != source_identity.space_id
        || marker.identity.origin != source_identity.origin
        || marker.identity.principal_id != source_identity.principal_id
        || destination_shared.root != source_shared.root
        || destination_shared.incarnation != source_shared.incarnation
        || destination_shared.root_identity != source_shared.root_identity
        || marker.initial_snapshot_digest != source.commitment().snapshot_digest
        || marker.import_digest.as_deref() != Some(prefix.digest()?.as_str())
        || marker.identity != prefix.destination_identity
        || source.commitment() != &activation.source
        || source.commitment().catalog_id != activation.catalog_id
        || intent
            .destination
            .as_ref()
            .is_none_or(|proof| proof.family_marker_digest != allocation.family.marker_digest)
    {
        return Err(invalid("destination FORMAT/source family binding differs"));
    }
    marker.validate_initial_selection(source.selection())?;
    prefix.require_references(ImportSourceReferences::Provider {
        allocation,
        prepared,
        activation,
    })?;
    let ImportSource::Provider { version, .. } = &prefix.source else {
        return Err(invalid("destination ingress requires provider source"));
    };
    if prefix.kind != ProviderImportKind::ManagedOrdinary
        || version.generation == 0
        || version.generation > i64::MAX as u64
        || intent.plan.source_prefix != *prefix
        || intent.plan.source_prefix_id != prefix.digest()?
        || intent.role != TransitionRole::Destination
        || intent.state
            != match phase {
                IngressPhase::Copying => TransitionState::Copying,
                IngressPhase::Staging | IngressPhase::CheckoutStaging(_) => {
                    TransitionState::Staging
                }
            }
        || intent.local_generation != 0
        || intent.reservation.is_some() != !matches!(phase, IngressPhase::Copying)
        || intent.receipt_id.is_some()
        || intent.destination.as_ref().is_none_or(|proof| {
            proof.backend != TransitionBackendKind::Ordinary
                || proof.format != *marker
                || proof.private_database_identity != destination_shared.database_identity
                || proof.native_control_identity_hex.is_some()
                || proof.native_root_identity.is_some()
        })
    {
        return Err(invalid(
            "destination ingress intent or source boundary differs",
        ));
    }
    intent.validate_fields()?;
    source
        .selection()
        .validate_snapshot(source.initial())
        .map_err(|e| invalid(e.to_string()))?;
    Ok(())
}

/// The caller retains the same lock and read transaction across this read.
/// Every table is counted only up to two rows after the schema fingerprint.
pub(in crate::projection::provider) fn read_empty(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<EmptyIngress> {
    read_state(
        c,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        None,
        IngressPhase::Copying,
        cancel,
    )?;
    Ok(EmptyIngress)
}

/// A copied prefix is still Destination Copying, with its immutable tables
/// equal to the retained source proof. No Ready or ordinary admission follows.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn read_copied(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    proof: &VerifiedImportClosure,
    cancel: Option<&Cancellation>,
) -> Result<CopiedPrefix> {
    read_state(
        c,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        Some(proof),
        IngressPhase::Copying,
        cancel,
    )?;
    Ok(CopiedPrefix)
}

/// Exact copied immutable SQL while the destination reservation is durably
/// declared but neither installed nor Ready. This remains DATA only.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn read_staging(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    proof: &VerifiedImportClosure,
    cancel: Option<&Cancellation>,
) -> Result<CopiedPrefixStaging> {
    read_state(
        c,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        Some(proof),
        IngressPhase::Staging,
        cancel,
    )?;
    Ok(CopiedPrefixStaging)
}

/// The same immutable Staging prefix with a complete, exactly bound checkout
/// journal. The ordinary Staging reader still requires an empty prejournal.
/// Returned rows are DATA; native ownership and positive close remain caller
/// obligations. An absent or foreign journal is not admitted by this route.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn read_checkout_staging(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    proof: &VerifiedImportClosure,
    expected: &checkout_journal::ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<checkout_journal::CheckoutJournal> {
    // Do not accept a journal for a different transition merely because its
    // rows are internally canonical. Reconstruct the expectation from the
    // independently checked destination intent and its complete inventory.
    let rebound = checkout_journal::ExpectedCheckout::new(
        intent,
        expected.files(),
        expected.deletions(),
        &expected.header().image_digest,
        cancel,
    )?;
    if rebound.header() != expected.header() {
        return Err(invalid("checkout journal destination binding differs"));
    }
    read_state(
        c,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        Some(proof),
        IngressPhase::CheckoutStaging(expected),
        cancel,
    )?
    .ok_or_else(|| invalid("checkout Staging journal absent"))
}

#[allow(clippy::too_many_arguments)]
fn read_state(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    copied: Option<&VerifiedImportClosure>,
    phase: IngressPhase<'_>,
    cancel: Option<&Cancellation>,
) -> Result<Option<checkout_journal::CheckoutJournal>> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid("destination ingress requires retained transaction"));
    }
    import_validation::require_schema(c, marker, ImportDatabaseKind::Provider, cancel)?;
    require_inputs_for_phase(
        marker, source, allocation, prepared, activation, prefix, intent, phase,
    )?;
    for (kind, name, _) in database_io::schema_inventory(c)? {
        cancellation::check(cancel)?;
        if kind != "table" {
            continue;
        }
        // The name comes from the exact reference-schema fingerprint above.
        let observed: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM \"{name}\" LIMIT 2)"),
            [],
            |r| r.get(0),
        )?;
        let expected = match name.as_str() {
            "projection_prejournal" if matches!(phase, IngressPhase::CheckoutStaging(_)) => None,
            "projection_meta"
            | "projection_head"
            | "projection_backend"
            | "projection_imports"
            | "projection_provider_transitions" => Some(1),
            "projection_snapshots" | "projection_selections" if copied.is_some() => None,
            "projection_snapshots" | "projection_selections" => Some(1),
            "projection_observations" if copied.is_some() => None,
            "projection_observations" => Some(u64::from(intent.plan.denial_coverage.is_some())),
            "projection_captures"
            | "projection_publications"
            | "projection_provider_catalogs"
            | "projection_provider_descriptors"
            | "projection_provider_activations"
            | "projection_provider_transition_receipts"
            | "projection_provider_companions"
            | "projection_provider_companion_files"
            | "projection_provider_companion_deletions"
            | "projection_content"
                if copied.is_some() =>
            {
                None
            }
            _ => Some(0),
        };
        if expected.is_some_and(|expected| observed != expected) {
            return Err(invalid(
                "destination EmptyIngress table cardinality differs",
            ));
        }
    }
    cancellation::check(cancel)?;
    let (generation, digest_len, parent_null, payload_len): (u64, u64, bool, u64) =
        c.query_row("SELECT generation,length(CAST(digest AS BLOB)),parent_digest IS NULL,length(payload) FROM projection_snapshots WHERE generation=0", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if generation != 0 || digest_len != 64 || !parent_null || payload_len > 64 * 1024 * 1024 {
        return Err(invalid("destination initial snapshot shape differs"));
    }
    let (digest, payload): (String, Vec<u8>) = c.query_row(
        "SELECT digest,payload FROM projection_snapshots WHERE generation=0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if payload
        != source
            .initial()
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?
        || digest != database::digest(0, None, &payload)
        || digest != marker.initial_snapshot_digest
    {
        return Err(invalid(
            "destination initial snapshot differs from source/FORMAT",
        ));
    }
    let head: (u64, u64) = c.query_row(
        "SELECT singleton,generation FROM projection_head",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let source_generation = match &prefix.source {
        ImportSource::Provider { version, .. } => version.generation,
        _ => return Err(invalid("destination copied source is not provider")),
    };
    if head != (1, copied.map_or(0, |_| source_generation)) {
        return Err(invalid("destination EmptyIngress head differs"));
    }
    let (selection_generation, digest_len, selection_payload_len): (u64, u64, u64) = c.query_row(
            "SELECT generation,length(CAST(digest AS BLOB)),length(payload) FROM projection_selections WHERE generation=0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if selection_generation != 0 || digest_len != 64 || selection_payload_len > 16 * 1024 * 1024 {
        return Err(invalid("destination initial selection row exceeds bound"));
    }
    if selection::at(c, 0)? != *source.selection() {
        return Err(invalid("destination initial selection differs"));
    }
    let ordinary: bool = c.query_row(
        "SELECT backend='ordinary' AND mount_id IS NULL FROM projection_backend WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if !ordinary {
        return Err(invalid("destination backend differs"));
    }
    let (prefix_len, payload_len): (u64,u64) = c.query_row(
        "SELECT length(CAST(digest AS BLOB)),length(payload) FROM projection_imports WHERE singleton=1", [],
        |r| Ok((r.get(0)?,r.get(1)?)))?;
    if prefix_len != 71 || payload_len > ProviderImportPrefixRecord::MAX_BYTES as u64 {
        return Err(invalid("destination import ingress row exceeds bound"));
    }
    let (id, payload): (String, Vec<u8>) = c.query_row(
        "SELECT digest,payload FROM projection_imports WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    prefix.require_row(&id, &payload)?;
    let bad: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE \
         length(CAST(id AS BLOB))>128 OR length(CAST(role AS BLOB))>16 \
         OR length(CAST(state AS BLOB))>16 OR length(CAST(digest AS BLOB))!=71 \
         OR length(payload)>262144 OR length(CAST(reservation AS BLOB))>128 \
         OR length(CAST(receipt_id AS BLOB))>71)",
        [],
        |r| r.get(0),
    )?;
    if bad {
        return Err(invalid("destination intent row exceeds bound"));
    }
    let (id,role,state,local_generation,reservation,receipt_id,payload,digest):
        (String,String,String,u64,Option<String>,Option<String>,Vec<u8>,String) = c.query_row(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?;
    let observed = ProviderTransitionIntentRecord::decode(&payload)?;
    observed.require_row(TransitionRowData {
        id: &id,
        role: &role,
        state: &state,
        local_generation,
        reservation: reservation.as_deref(),
        receipt_id: receipt_id.as_deref(),
        payload: &payload,
        digest: &digest,
    })?;
    if &observed != intent {
        return Err(invalid("destination intent differs from expected phase"));
    }
    let current = super::destination_coverage::read_current(c, &observed, cancel)?;
    if let Some(body) = &current {
        if body.path_semantics != source.initial().path_semantics
            || body.step_id != source.initial().base_step_id
            || body.root_tree_id != source.initial().base_root_tree_id
        {
            return Err(invalid(
                "destination current document source namespace differs",
            ));
        }
    }
    if let Some(proof) = copied {
        import_validation::require_exact_copied_rows_with_current(
            c,
            marker,
            prefix,
            proof,
            current.as_ref(),
            cancel,
        )?;
    }
    cancellation::check(cancel)?;
    match phase {
        IngressPhase::CheckoutStaging(expected) => {
            Ok(Some(checkout_journal::read(c, expected, cancel)?))
        }
        IngressPhase::Copying | IngressPhase::Staging => Ok(None),
    }
}
