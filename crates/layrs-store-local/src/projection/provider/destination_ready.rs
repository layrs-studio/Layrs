//! Destination Ready SQL DATA only. The caller separately proves the copied
//! checkout and durable exact family content/protection, retains all pins and
//! gates, and owns transactions, rollback, COMMIT and positive closes. This
//! module grants no ordinary admission and never opens a business database.
//! WIP: deliberately not wired into provider; compilation, independent source
//! review and the planned owned memory-SQL recipes remain outstanding.
use super::{
    cancellation, database_io, destination_validation,
    import_validation::VerifiedImportClosure,
    records::{
        activation::ProviderActivationRecord,
        import::{ImportSource, ProviderImportPrefixRecord, ProviderTransitionReceiptRecord},
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        AllocationRecord, PreparedRecord, ProviderBootstrapSource, Record,
    },
    recovery::error::{invalid, Result},
};
use crate::projection::{capture::CapturedPatch, database, format, selection};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{canonical_json, FileId, FileManifest, FileManifestId, RepoPath},
};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};

mod data;
mod family;
mod read;

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
    pub data: &'a ManagedReadyData<'a>,
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
    read::ready(source_c, destination, inputs, &prepared, cancel)?;
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
        read::ready(source_c, destination, inputs, &prepared, cancel)?;
        return Ok(ReadyEffect::AlreadyReady);
    }
    if !staging {
        return Err(invalid("destination Ready predecessor phase differs"));
    }
    if destination.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u64>(0))? != 1 {
        return Err(invalid("destination Ready writer requires FK ON"));
    }
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
    let plan = &inputs.staging.plan;
    let ImportSource::Provider { version, .. } = &inputs.prefix.source else {
        return Err(invalid("destination Ready requires provider source"));
    };
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
        selection::insert(destination, generation, inputs.source.selection())?;
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
    let old = inputs.staging;
    let new = inputs.data.ready;
    if destination.execute(
        "UPDATE projection_provider_transitions SET state='ready',receipt_id=?1,payload=?2,digest=?3 \
         WHERE id=?4 AND role=?5 AND state=?6 AND local_generation=?7 \
         AND reservation IS ?8 AND receipt_id IS ?9 AND payload=?10 AND digest=?11",
        params![new.receipt_id,new.encode()?,new.digest()?,old.transaction_id,old.role.sql(),
            old.state.sql(),old.local_generation,old.reservation,old.receipt_id,old.encode()?,old.digest()?],
    )? != 1 {
        return Err(invalid("destination Ready exact intent CAS differs"));
    }
    read::ready(source_c, destination, inputs, &prepared, cancel)?;
    cancellation::check(cancel)?;
    Ok(ReadyEffect::Finalized)
}
