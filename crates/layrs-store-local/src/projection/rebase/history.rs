use super::*;
use layrs_core::scm::TreeEntryTarget;
use rusqlite::{OptionalExtension, params};

pub(super) fn completed(
    c: &Connection,
    store: &ProjectionStore,
    preview: &RebasePreview,
) -> Result<Option<CapturedPatch>> {
    let Some((journal, captured)) = record(c, store, &preview.operation_id)? else {
        return Ok(None);
    };
    if journal.preview != *preview {
        return Err(invalid("completed rebase differs from preview"));
    }
    Ok(Some(captured))
}

pub(super) fn completed_id(
    c: &Connection,
    store: &ProjectionStore,
    operation_id: &str,
) -> Result<Option<CapturedPatch>> {
    if operation_id.is_empty()
        || operation_id.len() > 128
        || !operation_id
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-')
    {
        return Err(invalid("invalid rebase operation identity"));
    }
    Ok(record(c, store, operation_id)?.map(|(_, captured)| captured))
}

pub(in crate::projection) fn verify_history(
    c: &Connection,
    store: &ProjectionStore,
) -> Result<u64> {
    let mut statement =
        c.prepare("SELECT operation_id FROM projection_rebase_history ORDER BY generation")?;
    let ids = statement
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if ids.len() > 100_000 {
        return Err(invalid("rebase history exceeds validation budget"));
    }
    for id in &ids {
        record(c, store, id)?.ok_or_else(|| invalid("rebase history disappeared"))?;
    }
    Ok(ids.len() as u64)
}

fn record(
    c: &Connection,
    store: &ProjectionStore,
    operation_id: &str,
) -> Result<Option<(Journal, CapturedPatch)>> {
    let row: Option<(u64,u64,u64)> = c.query_row(
        "SELECT generation,capture_sequence,length(payload) FROM projection_rebase_history WHERE operation_id=?1",
        [operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((generation, sequence, len)) = row else {
        return Ok(None);
    };
    if len > MAX_JOURNAL as u64 {
        return Err(invalid("rebase history exceeds64MiB"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_rebase_history WHERE operation_id=?1",
        [operation_id],
        |r| r.get(0),
    )?;
    let journal = engine::decode(&bytes)?;
    let p = &journal.preview;
    if !journal.ready
        || p.operation_id != operation_id
        || p.digest != p.computed_digest()?
        || !journal
            .directory
            .starts_with(&format!("rebase-{}-", &p.digest[7..23]))
        || journal.directory.len() > 96
        || journal
            .directory_identity
            .as_ref()
            .is_none_or(String::is_empty)
        || generation
            != p.projection_generation
                .checked_add(1)
                .ok_or_else(|| invalid("rebase generation overflow"))?
    {
        return Err(invalid("rebase history identity mismatch"));
    }
    let old: (String, Vec<u8>) = c.query_row(
        "SELECT digest,payload FROM projection_snapshots WHERE generation=?1",
        [p.projection_generation],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if old.0 != p.projection_digest {
        return Err(invalid("rebase old projection differs"));
    }
    let old_snapshot: ProjectionSnapshot = serde_json::from_slice(&old.1).map_err(invalid)?;
    if old_snapshot.canonical_bytes()? != old.1 {
        return Err(invalid("rebase old projection noncanonical"));
    }
    validate_target(&old_snapshot, &p.target)?;
    validate_resolutions(&p.target, &p.resolutions)?;
    if p.before.keys().ne(p.resolutions.keys()) {
        return Err(invalid("rebase history paths differ"));
    }
    let next: (String, Option<String>, Vec<u8>) = c.query_row(
        "SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if next.1.as_deref() != Some(&old.0)
        || p.target.canonical_bytes()? != next.2
        || database::digest(generation, Some(&old.0), &next.2) != next.0
    {
        return Err(invalid("rebase resulting projection differs"));
    }
    let old_capture = capture::storage::load(c, store, p.capture_sequence)?;
    if old_capture.projection_generation != p.projection_generation
        || old_capture.projection_digest != p.projection_digest
        || old_capture.digest != p.capture_digest
    {
        return Err(invalid("rebase source capture differs"));
    }
    let mut old_changes = BTreeMap::new();
    for change in &old_capture.changes {
        old_changes.insert(&change.path, &change.replacement);
    }
    let old_entries: BTreeMap<_, _> = old_snapshot.entries.iter().map(|e| (&e.path, e)).collect();
    for (path, before) in &p.before {
        if let Some(replacement) = old_changes.get(path) {
            if *replacement != before {
                return Err(invalid("rebase source value differs from capture"));
            }
        } else if let Some(entry) = old_entries.get(path) {
            match &entry.target {
                Some(TreeEntryTarget::File { file_id })
                    if before.as_ref().is_some_and(|f| {
                        &f.file_id == file_id && f.executable == entry.executable
                    }) => {}
                None if before.is_none() => {}
                _ => return Err(invalid("rebase source value differs from base")),
            }
        } else if before.is_some() {
            return Err(invalid("rebase added path was occupied"));
        }
    }
    let expected_old: Vec<_> = p
        .before
        .iter()
        .filter(|(path, before)| *before != &p.resolutions[*path])
        .filter_map(|(path, file)| file.as_ref().map(|file| (path, file)))
        .collect();
    let expected_new: Vec<_> = p
        .resolutions
        .iter()
        .filter(|(path, file)| *file != &p.before[*path])
        .filter_map(|(path, file)| file.as_ref().map(|file| (path, file)))
        .collect();
    if journal.old.len() != expected_old.len() || journal.new.len() != expected_new.len() {
        return Err(invalid("rebase history file witnesses differ"));
    }
    for (index, (record, (path, file))) in journal.old.iter().zip(expected_old).enumerate() {
        if record.path != *path
            || record.file != *file
            || record.identity.is_empty()
            || record.name != format!("old-{index:08}")
        {
            return Err(invalid("rebase old witness differs"));
        }
        capture::storage::file_manifest(c, file)?;
    }
    for (index, (record, (path, file))) in journal.new.iter().zip(expected_new).enumerate() {
        if record.path != *path
            || record.file != *file
            || record.identity.is_empty()
            || record.name != format!("new-{index:08}")
        {
            return Err(invalid("rebase new witness differs"));
        }
    }
    let captured = capture::storage::load(c, store, sequence)?;
    if captured.projection_generation != generation
        || captured.projection_digest != next.0
        || captured.sequence
            != p.capture_sequence
                .checked_add(1)
                .ok_or_else(|| invalid("rebase capture sequence overflow"))?
        || captured.previous_patch_digest.as_deref() != Some(&p.capture_digest)
    {
        return Err(invalid("rebase capture projection differs"));
    }
    let mut expected_changes = Vec::new();
    for entry in &p.target.entries {
        if !entry.writable || !regular(entry) {
            continue;
        }
        let file = &p.resolutions[&entry.path];
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some(CapturedTarget {
                file_id: file_id.clone(),
                executable: entry.executable,
            }),
            None => None,
            _ => return Err(invalid("rebase history target changed")),
        };
        if file.as_ref().map(|f| (&f.file_id, f.executable))
            != expected.as_ref().map(|f| (&f.file_id, f.executable))
        {
            expected_changes.push(CapturedChange {
                path: entry.path.clone(),
                expected,
                replacement: file.clone(),
            });
        }
    }
    if captured.changes != expected_changes {
        return Err(invalid("rebase capture differs from resolutions"));
    }
    let history_capture: u64 = c.query_row(
        "SELECT capture_sequence FROM projection_rebase_history WHERE operation_id=?1",
        params![operation_id],
        |r| r.get(0),
    )?;
    if history_capture != captured.sequence {
        return Err(invalid("rebase capture sequence differs"));
    }
    Ok(Some((journal, captured)))
}
