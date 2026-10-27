//! Point reads for declared SQL frontiers. No admission/provenance dispatcher,
//! source image, content mapping, family opener or filesystem reader is used.
use super::*;
use crate::projection::{capture, offline, selection, VersionedProjection};

const MAX_ROWS: u64 = 1_000_000;

pub(super) struct Current {
    generation: u64,
    captures: u64,
}
impl Current {
    pub(super) fn read(
        c: &Connection,
        admission: &ManagedAdmissionRecord,
        cancel: Option<&Cancellation>,
    ) -> CauseResult<Self> {
        cancellation::check(cancel)?;
        let (count, generation): (u64, Option<u64>) = c.query_row(
            "SELECT count(*),max(generation) FROM projection_head",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let generation = generation
            .filter(|generation| count == 1 && *generation < MAX_ROWS)
            .ok_or_else(|| invalid("ordinary outgoing current head missing or exceeds bound"))?;
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM projection_captures LIMIT 1000001)",
            [],
            |row| row.get(0),
        )?;
        if count > MAX_ROWS {
            return Err(invalid("ordinary outgoing capture count exceeds bound"));
        }
        let (first, last, malformed): (Option<u64>, Option<u64>, bool) = c.query_row(
            "SELECT min(sequence),max(sequence),coalesce(max(length(CAST(digest AS BLOB))!=71 OR length(payload)=0 OR length(payload)>8388608 OR projection_generation>?1),0) FROM projection_captures",
            [generation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if count == 0
            || first != Some(1)
            || last != Some(count)
            || malformed
            || generation < admission.boundaries.ready_generation
            || count < admission.boundaries.ready_capture_sequence
        {
            return Err(invalid("ordinary outgoing actual capture frontier differs"));
        }
        cancellation::check(cancel)?;
        Ok(Self {
            generation,
            captures: count,
        })
    }
}

pub(super) fn require_version(
    c: &Connection,
    marker: &format::Marker,
    admission: &ManagedAdmissionRecord,
    record: &ProviderTransitionIntentRecord,
    current: &Current,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    cancellation::check(cancel)?;
    let version = match &record.plan.source_prefix.source {
        ImportSource::Ordinary { version, .. } => version,
        _ => {
            return Err(invalid(
                "ordinary outgoing frontier requires ordinary source",
            ))
        }
    };
    if version.generation < admission.boundaries.ready_generation
        || version.generation > current.generation
        || version.capture_count < admission.boundaries.ready_capture_sequence
        || version.capture_count > current.captures
        || (is_live(record.state)
            && (version.generation != current.generation
                || version.capture_count != current.captures))
    {
        return Err(invalid(
            "ordinary outgoing declared frontier escaped admission/current bounds",
        ));
    }
    let snapshot = snapshot(
        c,
        version.generation,
        Some(&version.snapshot_digest),
        cancel,
    )?;
    let selection_generation = probe_selection(c, version.generation)?;
    let digest: String = c.query_row(
        "SELECT digest FROM projection_selections WHERE generation=?1",
        [selection_generation],
        |row| row.get(0),
    )?;
    if digest != version.selection_digest {
        return Err(invalid("ordinary outgoing selection boundary differs"));
    }
    selection::at(c, version.generation)?
        .validate_snapshot(&snapshot.snapshot)
        .map_err(|error| invalid(error.to_string()))?;
    let (capture_generation, payload_bytes, digest_bytes): (u64, u64, u64) = c.query_row(
        "SELECT projection_generation,length(payload),length(CAST(digest AS BLOB)) FROM projection_captures WHERE sequence=?1",
        [version.capture_count],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if capture_generation < admission.boundaries.ready_generation
        || capture_generation > version.generation
        || payload_bytes == 0
        || payload_bytes > 8 * 1024 * 1024
        || digest_bytes != 71
    {
        return Err(invalid(
            "ordinary outgoing capture boundary missing or oversized",
        ));
    }
    // Bound every variable-width field reached by the nonrecursive existing
    // capture interpreter, including its referenced selection and snapshot.
    probe_snapshot(c, capture_generation)?;
    probe_selection(c, capture_generation)?;
    let capture = capture::storage::load_metadata_for_origin(
        c,
        version.capture_count,
        &marker.identity,
        version.generation,
    )?;
    if version.capture_head.as_deref() != Some(capture.digest.as_str()) {
        return Err(invalid("ordinary outgoing capture head differs"));
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_captures WHERE sequence<=?1 AND projection_generation>?2)",
        rusqlite::params![version.capture_count, version.generation],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(invalid("ordinary outgoing capture prefix escaped snapshot frontier"));
    }
    cancellation::check(cancel)
}

fn probe_snapshot(c: &Connection, generation: u64) -> CauseResult<()> {
    let (count, digest, parent, payload): (u64, u64, u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(digest AS BLOB))),0),coalesce(max(length(CAST(parent_digest AS BLOB))),0),coalesce(max(length(payload)),0) FROM projection_snapshots WHERE generation=?1",
        [generation],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if generation >= MAX_ROWS
        || count != 1
        || digest != 64
        || parent != if generation == 0 { 0 } else { 64 }
        || payload == 0
        || payload > 64 * 1024 * 1024
    {
        return Err(invalid(
            "ordinary outgoing referenced snapshot missing or oversized",
        ));
    }
    Ok(())
}

fn snapshot(
    c: &Connection,
    generation: u64,
    expected: Option<&str>,
    cancel: Option<&Cancellation>,
) -> CauseResult<VersionedProjection> {
    cancellation::check(cancel)?;
    probe_snapshot(c, generation)?;
    let parent: Option<String> = c.query_row(
        "SELECT parent_digest FROM projection_snapshots WHERE generation=?1",
        [generation],
        |row| row.get(0),
    )?;
    if generation == 0 {
        if parent.is_some() {
            return Err(invalid("ordinary outgoing generation zero has a parent"));
        }
    } else {
        probe_snapshot(c, generation - 1)?;
        let previous: String = c.query_row(
            "SELECT digest FROM projection_snapshots WHERE generation=?1",
            [generation - 1],
            |row| row.get(0),
        )?;
        if parent.as_deref() != Some(previous.as_str()) {
            return Err(invalid("ordinary outgoing snapshot parent linkage differs"));
        }
    }
    let snapshot = offline::snapshot_at(c, generation, expected)?;
    cancellation::check(cancel)?;
    Ok(snapshot)
}

fn probe_selection(c: &Connection, generation: u64) -> CauseResult<u64> {
    let (selected, digest, payload): (u64, u64, u64) = c.query_row(
        "SELECT generation,length(CAST(digest AS BLOB)),length(payload) FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1",
        [generation],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if selected >= MAX_ROWS || digest != 64 || payload == 0 || payload > 16 * 1024 * 1024 {
        return Err(invalid(
            "ordinary outgoing referenced selection missing or oversized",
        ));
    }
    probe_snapshot(c, selected)?;
    Ok(selected)
}
