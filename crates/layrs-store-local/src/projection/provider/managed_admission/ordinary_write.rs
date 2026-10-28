//! Caller-transaction private8 Ordinary Ready -> private10 provenance. No COMMIT, close,
//! opener, lock acquisition or physical authority. Any failed write/postcheck
//! requires caller rollback before this connection may be reused.
use super::super::{
    destination_ready::{read_ordinary_managed_ready, ManagedReadyClaim, VerifiedManagedReady},
    import_validation::read_destination_prefix,
    managed_budget,
    published_handoff::OrdinaryPublishedForReady,
};
use super::write::{journal_commitment, same_representations, AdmissionWriteEffect};
use super::*;
use crate::projection::{
    capture::storage::load_metadata_for_origin, shared::exact_protection::ExactProtection,
};
use rusqlite::params;

/// The opaque SQL proofs are required only for an actual private8 transition.
/// Private10 replay interprets permanent local provenance before touching either
/// external connection. A fresh-process replay may simply call the exported
/// read_admitted_import without reconstructing these obsolete source holders.
///
/// OrdinaryPublishedForReady proves actual historical SQL, not durability: the physical
/// driver MUST obtain it from its own independent read-only source connection,
/// retain source/family/destination/FS proofs through destination COMMIT, observe
/// positive closes and re-read before acknowledging admission to its caller.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn admit_ordinary_ready(
    c: &Connection,
    marker: &format::Marker,
    family_c: &Connection,
    source_c: &Connection,
    ready: &VerifiedManagedReady<'_, '_>,
    published: &OrdinaryPublishedForReady<'_>,
    cancel: Option<&Cancellation>,
) -> Result<AdmissionWriteEffect> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid("managed admission requires caller transaction"));
    }
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 10 {
        read_admitted_import(c, marker, cancel)?;
        return Ok(AdmissionWriteEffect::AlreadyAdmitted);
    }
    if version != 8
        || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1
        || std::ptr::eq(c, source_c)
        || std::ptr::eq(c, family_c)
        || std::ptr::eq(source_c, family_c)
    {
        return Err(invalid(
            "managed admission requires private8, FK ON and distinct retained databases",
        ));
    }
    ready.require(c, family_c, cancel)?;
    published.require(source_c, cancel)?;
    let snapshot = read_ordinary_managed_ready(c, marker, family_c, cancel)?.into_snapshot()?;
    let prefix = read_destination_prefix(c, marker, cancel)?;
    let intent = &snapshot.ready;
    if published.receipt() != &snapshot.receipt
        || published.published().transaction_id != intent.transaction_id
        || published.published().plan != intent.plan
        || published.published().destination != intent.destination
        || published.published().reservation != intent.reservation
        || published.published().receipt_id != intent.receipt_id
    {
        return Err(invalid(
            "managed admission actual SourcePublished differs from Ready",
        ));
    }
    let source_body = published.published().encode()?;
    let archive = journal_commitment(c, &snapshot.expected, &snapshot.journal, cancel)?;
    // The complete Ready reader fenced and checked this unique canonical claim
    // before this fixed-length SELECT. Reconstruct only DATA for exact sizing.
    let claim = c.query_row(
        "SELECT root_id,digest,kind,owner FROM projection_claims",
        [],
        |r| {
            Ok(ManagedReadyClaim {
                root_id: r.get(0)?,
                digest: r.get(1)?,
                kind: r.get(2)?,
                owner: r.get(3)?,
            })
        },
    )?;
    let ImportSource::Ordinary {
        version: source_version,
        format_digest,
        admission: Some(source_admission),
        ..
    } = &prefix.prefix().source
    else {
        return Err(invalid(
            "managed admission requires admitted Ordinary source prefix",
        ));
    };
    let record = ManagedAdmissionRecord {
        schema_version: "layrs.managed-admission.v1".into(),
        format_digest: super::super::records::codec::hash(
            "layrs-managed-admission-format-v1",
            &marker.bytes()?,
        ),
        transaction_id: intent.transaction_id.clone(),
        prefix_id: prefix.prefix().digest()?,
        ready_intent_digest: intent.digest()?,
        receipt_id: snapshot.receipt.digest()?,
        source_published_digest: published.published().digest()?,
        source_database_identity: source_admission.database_identity.clone(),
        source_format_digest: format_digest.clone(),
        root_id: claim.root_id.clone(),
        binding: snapshot.binding.clone(),
        archive,
        boundaries: record::Boundaries {
            imported_generation: source_version.generation,
            imported_capture_count: source_version.capture_count,
            observation_generation: intent.plan.observation.generation,
            ready_generation: intent.plan.placement_generation,
            ready_capture_sequence: intent.plan.capture_sequence,
        },
        metadata_layout_version: 1,
    };
    record.require_links(
        schema::AdmittedLayout::Ordinary10,
        marker,
        prefix.prefix(),
        intent,
        &snapshot.receipt,
        published.published(),
    )?;
    let witness = representations::from_manifests(&snapshot.required, intent, cancel)?;
    let payload = record.encode()?;
    let basis = prefix.ordinary_frozen()?;
    let observation = basis.snapshot();
    let placement = basis.snapshot();
    let capture = load_metadata_for_origin(
        c,
        intent.plan.capture_sequence,
        &marker.identity,
        intent.plan.placement_generation,
    )?;
    let (source_size, destination_size) = managed_budget::original_staging_sizes(intent)?;
    let base = managed_budget::base_metadata_floor(
        prefix.metadata_bytes(c, cancel)?,
        observation,
        placement,
        &capture,
        &intent.plan,
        basis.selection(),
        source_size,
        destination_size,
    )?;
    let floor = managed_budget::complete_metadata_floor(
        base,
        intent,
        &snapshot.receipt,
        &claim,
        &snapshot.required,
        cancel,
    )?;
    let budget = managed_budget::ordinary_admission_budget(
        floor,
        snapshot.expected.files().len() as u64,
        snapshot.expected.directories().len() as u64,
        snapshot.required.len() as u64,
    )?;
    let baseline = managed_budget::quota(
        &snapshot.required,
        snapshot.expected.files().iter().map(|file| file.bytes),
        0,
    )?;
    let metadata = budget.require_quota(intent.plan.quota_bytes, baseline)?;
    let cost = managed_budget::ordinary_admission_cost(
        source_body.len() as u64,
        payload.len() as u64,
        record.archive.count,
        record.archive.bytes,
        snapshot.required.len() as u64,
        representations::column_bytes(snapshot.required.len() as u64)?,
    )?;
    budget.require_actual(metadata, &cost)?;
    let protection = ExactProtection::new(
        &snapshot.binding.family_incarnation,
        &snapshot.binding.owner,
        &snapshot.binding.database_identity,
        &intent.transaction_id,
        &snapshot.required,
        cancel,
    )?;
    if protection.id() != record.root_id {
        return Err(invalid("managed admission exact protection differs"));
    }
    let net_remaining = snapshot
        .installation
        .remaining
        .checked_sub(snapshot.expected.header().file_bytes)
        .ok_or_else(|| invalid("managed admission retained checkout envelope insufficient"))?;
    budget.require_remaining(
        net_remaining,
        protection.payload().len() as u64,
        snapshot.required.len() as u64,
    )?;
    // Every typed/body/capacity precondition above is settled before any DDL or
    // SQL mutation. No callbacks, I/O or content hashing occur under this API.
    ready.require(c, family_c, cancel)?;
    published.require(source_c, cancel)?;
    cancellation::check(cancel)?;
    c.execute_batch(schema::SQL)?;
    c.execute_batch(schema::SQL10_WITNESS)?;
    for row in &witness {
        cancellation::check(cancel)?;
        c.execute("INSERT INTO projection_import_representations(file_id,manifest_id,bytes) VALUES(?1,?2,?3)", params![row.file_id.as_str(),row.manifest_id.as_str(),row.bytes])?;
    }
    let copied = c.execute("INSERT INTO projection_import_checkout(path,operation,digest,payload,state) SELECT path,operation,digest,payload,state FROM projection_prejournal",[])?;
    if copied as u64 != record.archive.count {
        return Err(invalid(
            "managed admission checkout archive copy count differs",
        ));
    }
    c.execute("INSERT INTO projection_managed_admission(singleton,digest,source_published_digest,source_published,receipt_id,ready_generation,ready_capture_sequence,archive_count,archive_digest,payload) VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![record.digest()?,record.source_published_digest,source_body,record.receipt_id,record.boundaries.ready_generation,record.boundaries.ready_capture_sequence,record.archive.count,record.archive.digest,payload])?;
    let deleted = c.execute(
        "DELETE FROM projection_prejournal WHERE operation=?1",
        [snapshot.expected.operation()],
    )?;
    if deleted != copied {
        return Err(invalid(
            "managed admission operational journal removal count differs",
        ));
    }
    c.execute_batch("PRAGMA user_version=10")?;
    let after = read_admitted_import(c, marker, cancel)?;
    if after.private_schema() != 10
        || after.witness_column_bytes() != representations::column_bytes(witness.len() as u64)?
        || after.record() != &record
        || after.source_published() != published.published()
        || after.archive().rows() != snapshot.journal.rows()
        || !same_representations(after.required(), &snapshot.required)
    {
        return Err(invalid("managed admission post-write provenance differs"));
    }
    published.require(source_c, cancel)?;
    cancellation::check(cancel)?;
    Ok(AdmissionWriteEffect::Admitted)
}
