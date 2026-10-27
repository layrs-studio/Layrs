//! Read-only SQL DATA planning for an actual admitted ordinary source. The
//! caller retains the snapshot and independently witnesses the effective image.
use super::super::{
    managed_admission::read_admitted_import,
    records::{
        codec,
        import::{
            imported_segments_rowset, AdmittedSourceBinding, ImportVersion, ProviderImportKind,
        },
        Record,
    },
};
use super::*;
use crate::projection::{offline, ProjectionIdentity};

const MAX_ROWS: u64 = 1_000_000;
// Internal, syntactically valid commitments only. No provisional prefix or
// closure leaves this module; every digest is replaced and verified below.
const PLACEHOLDER: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";

pub(in crate::projection) fn plan_admitted_import(
    c: &Connection,
    marker: &format::Marker,
    destination: ProjectionIdentity,
    image_digest: &str,
    cancel: Option<&Cancellation>,
) -> CauseResult<(ProviderImportPrefixRecord, VerifiedImportClosure)> {
    // This fences actual schema9 and FORMAT, and follows the database's own
    // incoming admission before using any caller-selected outgoing DATA.
    let admitted = read_admitted_import(c, marker, cancel)?;
    format::validate_identity(&destination)?;
    codec::raw_digest(image_digest)?;
    if destination == marker.identity {
        return Err(invalid("admitted import destination equals its source"));
    }
    let shared = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("admitted import source shared FORMAT absent"))?;
    let binding = AdmittedSourceBinding {
        admission_digest: admitted.record().digest()?,
        incoming_prefix_id: admitted.prefix().digest()?,
        incoming_receipt_id: admitted.receipt().digest()?,
        database_identity: shared.database_identity.clone(),
    };
    let segments = admitted.export_segments()?;
    let inherited_segments = imported_segments_rowset(&segments)?;
    let (version, selection_count) = current_version(c, image_digest, cancel)?;
    let snapshot_count = version
        .generation
        .checked_add(1)
        .ok_or_else(|| invalid("admitted import snapshot count overflow"))?;
    let mut prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v2".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: marker.identity.clone(),
            format_digest: codec::hash(
                "layrs-projection-provider-import-format-v1",
                &marker.bytes()?,
            ),
            private_schema: 9,
            checkout_identity: Some(marker.checkout_identity().to_owned()),
            admission: Some(binding.clone()),
            version: version.clone(),
        },
        destination_identity: destination,
        ancestry_root_receipt: Some(binding.incoming_receipt_id.clone()),
        closure: ImportClosure {
            snapshots: provisional_rowset(snapshot_count),
            selections: provisional_rowset(selection_count),
            captures: provisional_rowset(version.capture_count),
            publications: provisional_rowset(0),
            catalogs: provisional_rowset(0),
            descriptors: provisional_rowset(0),
            activation_maps: provisional_rowset(0),
            transition_receipts: provisional_rowset(inherited_segments.count),
            companions: provisional_rowset(0),
            companion_files: provisional_rowset(0),
            companion_deletions: provisional_rowset(0),
            content_mappings: provisional_rowset(0),
            restore_history: Some(provisional_rowset(0)),
        },
        inherited_segments,
    };
    prefix.validate_fields()?;
    prefix.validate_segments(&segments)?;
    let references = || ImportSourceReferences::AdmittedOrdinary {
        marker,
        admission: &binding,
    };
    let metadata = read_source_metadata(
        c,
        marker,
        ImportDatabaseKind::AdmittedOrdinary,
        &prefix,
        references(),
        image_digest,
        cancel,
    )?;
    prefix.closure = scoped::compute_scoped_closure(
        c,
        ImportDatabaseKind::AdmittedOrdinary,
        &prefix,
        &metadata,
        cancel,
    )?;
    // Reinterpret the final prefix, including all historical closures. The
    // opaque proof can only be produced by this complete source validator.
    let closure = verify_source(
        c,
        marker,
        ImportDatabaseKind::AdmittedOrdinary,
        &prefix,
        references(),
        None,
        image_digest,
        cancel,
    )?;
    Ok((prefix, closure))
}

fn provisional_rowset(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: PLACEHOLDER.into(),
    }
}

/// Bound the current ordinary suffix before loading its variable-width values.
/// Historical admission validation has already fenced the incoming prefix;
/// the full source interpreter below authenticates every current history row.
fn current_version(
    c: &Connection,
    image_digest: &str,
    cancel: Option<&Cancellation>,
) -> CauseResult<(ImportVersion, u64)> {
    super::super::cancellation::check(cancel)?;
    let (head_count, generation): (u64, Option<u64>) = c.query_row(
        "SELECT count(*),max(generation) FROM projection_head",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let generation = generation
        .filter(|generation| head_count == 1 && *generation < MAX_ROWS)
        .ok_or_else(|| invalid("admitted import current head missing or exceeds bound"))?;
    let (snapshot_count, digest_bytes, parent_bytes, payload_bytes): (u64, u64, u64, u64) =
        c.query_row(
            "SELECT count(*),coalesce(max(length(CAST(digest AS BLOB))),0),coalesce(max(length(CAST(parent_digest AS BLOB))),0),coalesce(max(length(payload)),0) FROM projection_snapshots WHERE generation=?1",
            [generation],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    if snapshot_count != 1
        || digest_bytes != 64
        || parent_bytes > 64
        || payload_bytes > 64 * 1024 * 1024
    {
        return Err(invalid(
            "admitted import current snapshot missing or oversized",
        ));
    }
    let (selection_count, selection_oversized): (u64, bool) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(digest AS BLOB))!=64 OR length(payload)>16777216),0) FROM projection_selections WHERE generation<=?1",
        [generation],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if selection_count == 0 || selection_count > generation + 1 || selection_oversized {
        return Err(invalid(
            "admitted import selection frontier missing or oversized",
        ));
    }
    let (capture_count, capture_sequence, capture_oversized): (u64, Option<u64>, bool) =
        c.query_row(
            "SELECT count(*),max(sequence),coalesce(max(length(CAST(digest AS BLOB))!=71 OR length(payload)>8388608),0) FROM projection_captures",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
    if capture_count > MAX_ROWS
        || capture_sequence != (capture_count > 0).then_some(capture_count)
        || capture_oversized
    {
        return Err(invalid(
            "admitted import capture frontier has a gap or exceeds bound",
        ));
    }
    let (publication_count, publication_oversized): (u64, bool) = c.query_row(
        "SELECT count(*),coalesce(max(sequence>?1 OR projection_generation>?2 OR length(CAST(request_digest AS BLOB))!=71 OR length(CAST(receipt_digest AS BLOB))!=71 OR length(receipt_bytes)>25165824 OR length(next_snapshot)>67108864 OR length(CAST(projection_digest AS BLOB))!=64 OR length(after_state)>68157440),0) FROM projection_publications",
        rusqlite::params![capture_count, generation],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if publication_count > capture_count || publication_oversized {
        return Err(invalid(
            "admitted import publication frontier exceeds bound",
        ));
    }
    super::super::cancellation::check(cancel)?;
    let snapshot = offline::snapshot_at(c, generation, None)?;
    let selection_digest: String = c.query_row(
        "SELECT digest FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1",
        [generation],
        |r| r.get(0),
    )?;
    let capture_head = if capture_count == 0 {
        None
    } else {
        Some(c.query_row(
            "SELECT digest FROM projection_captures WHERE sequence=?1",
            [capture_count],
            |r| r.get(0),
        )?)
    };
    Ok((
        ImportVersion {
            generation,
            snapshot_digest: snapshot.digest,
            selection_digest,
            capture_count,
            capture_head,
            effective_image_digest: image_digest.to_owned(),
        },
        selection_count,
    ))
}
