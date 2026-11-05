//! Separate immutable-prefix and exact local-suffix checks. Never reinterpret
//! an appended destination as the earlier whole-table CopiedPrefix state.
use super::super::records::{import::TransitionReceiptRowData, transition::TransitionRowData};
use super::*;

pub(super) fn ready(
    source_c: &Connection,
    c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    prepared: &PreparedData,
    checkout: Option<&super::super::checkout_journal::ExpectedCheckout>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    inputs.closure.require_copied_prefix_scope(
        source_c,
        c,
        inputs.marker,
        inputs.prefix,
        cancel,
    )?;
    ready_suffix(c, &inputs.view(), prepared, checkout, cancel)
}

pub(super) fn ready_suffix(
    c: &Connection,
    inputs: &ReadyView<'_>,
    prepared: &PreparedData,
    checkout: Option<&super::super::checkout_journal::ExpectedCheckout>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    // The current document is distinct from the inherited prefix. Authenticate
    // its complete body and the observation that the suffix stores, on every
    // Ready read; a compact v2 receipt alone is never sufficient.
    if let Some(body) =
        super::super::destination_coverage::read_current(c, inputs.data.ready, cancel)?
    {
        let plan = &inputs.data.ready.plan;
        body.require_observation_rows(
            inputs.data.observation,
            inputs.selection(),
            plan.observation.generation,
            &plan.observation.snapshot_digest,
            &plan.observation.selection_digest,
            cancel,
        )?;
    }
    let checkout_rows = match checkout {
        None => 0,
        Some(expected) => {
            require_checkout_ready_expected(inputs, expected, cancel)?;
            let journal = super::super::checkout_journal::read(c, expected, cancel)?;
            if !journal.is_complete() {
                return Err(invalid(
                    "destination Ready requires complete checkout journal",
                ));
            }
            journal.rows().len() as u64
        }
    };
    let closure = &inputs.prefix.closure;
    let counts = [
        (
            "projection_observations",
            add(
                closure
                    .denial_observations
                    .as_ref()
                    .map_or(0, |rows| rows.count),
                u64::from(inputs.data.ready.plan.denial_coverage.is_some()),
            )?,
        ),
        ("projection_snapshots", add(closure.snapshots.count, 2)?),
        ("projection_selections", add(closure.selections.count, 2)?),
        ("projection_captures", add(closure.captures.count, 1)?),
        ("projection_publications", closure.publications.count),
        (
            "projection_restore_history",
            closure
                .restore_history
                .as_ref()
                .map_or(0, |rows| rows.count),
        ),
        ("projection_provider_catalogs", closure.catalogs.count),
        ("projection_provider_descriptors", closure.descriptors.count),
        (
            "projection_provider_activations",
            closure.activation_maps.count,
        ),
        (
            "projection_provider_transition_receipts",
            add(closure.transition_receipts.count, 1)?,
        ),
        (
            "projection_provider_companions",
            add(closure.companions.count, u64::from(inputs.has_companion()))?,
        ),
        (
            "projection_provider_companion_files",
            add(
                closure.companion_files.count,
                inputs.companion_header().map_or(0, |h| h.file_count),
            )?,
        ),
        (
            "projection_provider_companion_deletions",
            add(
                closure.companion_deletions.count,
                inputs.companion_header().map_or(0, |h| h.deletion_count),
            )?,
        ),
        (
            "projection_content",
            add(
                closure.content_mappings.count,
                prepared.new_content.len() as u64,
            )?,
        ),
        ("projection_meta", 1),
        ("projection_head", 1),
        ("projection_backend", 1),
        ("projection_imports", 1),
        ("projection_provider_transitions", 1),
        ("projection_claims", 1),
        ("projection_prejournal", checkout_rows),
    ];
    for (kind, name, _) in database_io::schema_inventory(c)? {
        cancellation::check(cancel)?;
        if kind != "table" {
            continue;
        }
        // Names originate only from the exact reference schema already checked.
        let expected = counts
            .iter()
            .find(|(table, _)| *table == name)
            .map_or(0, |(_, n)| *n);
        let observed: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM \"{name}\" LIMIT ?1)"),
            [add(expected, 1)?],
            |row| row.get(0),
        )?;
        if observed != expected {
            return Err(invalid("destination Ready table has missing or extra rows"));
        }
    }
    let plan = &inputs.data.ready.plan;
    let version = inputs.source.version(inputs.prefix)?;
    let head: (u64, u64) = c.query_row(
        "SELECT singleton,generation FROM projection_head",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if head != (1, plan.placement_generation) {
        return Err(invalid("destination Ready head differs"));
    }
    if !c.query_row(
        "SELECT backend='ordinary' AND mount_id IS NULL FROM projection_backend WHERE singleton=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("destination Ready backend differs"));
    }
    let (digest, payload): (String, Vec<u8>) = c.query_row(
        "SELECT digest,payload FROM projection_imports WHERE singleton=1 AND length(payload)<=16777216",
        [], |r| Ok((r.get(0)?,r.get(1)?)),
    )?;
    inputs.prefix.require_row(&digest, &payload)?;
    for (generation, digest, parent, bytes) in [
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
        let valid: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_snapshots WHERE generation=?1 AND digest=?2 AND parent_digest=?3 AND payload=?4)",
            params![generation,digest,parent,bytes], |r| r.get(0),
        )?;
        let selection_payload = selection::encode(inputs.selection())?;
        let selection_digest = database::digest(generation, Some(digest), &selection_payload);
        let selected: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_selections WHERE generation=?1 AND digest=?2 AND payload=?3)",
            params![generation,selection_digest,selection_payload], |r| r.get(0),
        )?;
        if !valid || !selected {
            return Err(invalid(
                "destination Ready local snapshot or selection differs",
            ));
        }
    }
    for (file, manifest, bytes) in &prepared.new_content {
        cancellation::check(cancel)?;
        if !c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_content WHERE file_id=?1 AND manifest_id=?2 AND bytes=?3)",
            params![file,manifest,bytes], |r| r.get::<_,bool>(0),
        )? { return Err(invalid("destination Ready local content mapping differs")); }
    }
    let patch = crate::projection::capture::storage::load_metadata_for_origin(
        c,
        plan.capture_sequence,
        &inputs.marker.identity,
        plan.placement_generation,
    )?;
    if patch != *inputs.data.capture {
        return Err(invalid("destination Ready local capture differs"));
    }
    let claim = &inputs.data.claims[0];
    if !c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_claims WHERE root_id=?1 AND digest=?2 AND kind=?3 AND owner=?4)",
        params![claim.root_id,claim.digest,claim.kind,claim.owner], |r| r.get::<_,bool>(0),
    )? { return Err(invalid("destination Ready private import claim differs")); }
    if let Some(companion) = inputs.companion {
        companion.rows.require_new_rows(c, cancel)?;
        companion.rows.require_inherited_rows(c, cancel)?;
    }
    if let Some(retained) = inputs.retained_companion {
        retained.require_rows(c, cancel)?;
    }
    let receipt = inputs.data.receipt;
    let (id, owner, tx, companion, bytes): (String,String,String,Option<String>,Vec<u8>) = c.query_row(
        "SELECT id,owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts WHERE id=?1 AND length(payload)<=65536",
        [receipt.digest()?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    )?;
    receipt.require_row(TransitionReceiptRowData {
        id: &id,
        owner_identity_digest: &owner,
        transaction_id: &tx,
        companion_id: companion.as_deref(),
        payload: &bytes,
    })?;
    let oversized: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE \
         length(CAST(id AS BLOB))>128 OR length(CAST(role AS BLOB))>16 \
         OR length(CAST(state AS BLOB))>16 OR length(CAST(digest AS BLOB))!=71 \
         OR length(payload)>262144 OR length(CAST(reservation AS BLOB))>128 \
         OR length(CAST(receipt_id AS BLOB))>71)",
        [],
        |r| r.get(0),
    )?;
    if oversized {
        return Err(invalid("destination Ready intent exceeds bound"));
    }
    let (id,role,state,local_generation,reservation,receipt_id,payload,digest):
        (String,String,String,u64,Option<String>,Option<String>,Vec<u8>,String) = c.query_row(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)),
    )?;
    inputs.data.ready.require_row(TransitionRowData {
        id: &id,
        role: &role,
        state: &state,
        local_generation,
        reservation: reservation.as_deref(),
        receipt_id: receipt_id.as_deref(),
        payload: &payload,
        digest: &digest,
    })?;
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("destination Ready foreign key violation"));
    }
    cancellation::check(cancel)
}

fn add(value: u64, extra: u64) -> Result<u64> {
    value
        .checked_add(extra)
        .filter(|n| *n <= 1_100_010)
        .ok_or_else(|| invalid("destination Ready row count exceeds bound"))
}
