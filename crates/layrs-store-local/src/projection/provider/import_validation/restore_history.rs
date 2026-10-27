//! Bounded restore DATA at one immutable capture frontier. Checkout evidence
//! comes from the capture's exact committed segment, never an identity search.
use super::super::records::Record;
use super::*;
use crate::projection::{capture, offline, restore};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ROWS: u64 = 1_000_000;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PAYLOAD: u64 = 8 * 1024 * 1024;
const MAX_ID: u64 = 96;

pub(super) fn require_supported_scope(
    c: &Connection,
    prefix: &ProviderImportPrefixRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    super::super::cancellation::check(cancel)?;
    prefix.validate_fields()?;
    if !prefix.supports_restore_history() {
        let version = match &prefix.source {
            ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => {
                version
            }
        };
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_restore_history WHERE capture_sequence<=?1)",
            [version.capture_count],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(invalid(
                "legacy import prefix cannot authorize restore history",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{
        capture::{CapturedFile, CapturedPatch},
        provider::{
            database_io::BareConnection,
            process_profile::tests::child,
            records::import::{ImportClosure, ImportVersion, ProviderImportKind},
            validation::tests::fixture::Fixture,
        },
        restore::{RestoreDestination, RestorePreview, RestoreSource},
    };
    use layrs_core::scm::{FileId, WorktreeId};
    use rusqlite::params;

    const TOKEN: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
    const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn rowset(count: u64) -> ImportRowset {
        ImportRowset {
            count,
            digest: TOKEN.into(),
        }
    }

    fn payload(preview: &RestorePreview) -> Vec<u8> {
        let mut preview = preview.clone();
        preview.digest.clear();
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-projection-restore-preview-v1\0");
        hash.update(&canonical_json(&preview).unwrap());
        preview.digest = format!("blake3:{}", hash.finalize());
        let witnesses: BTreeMap<_, _> = preview
            .before
            .keys()
            .map(|path| (path.clone(), "1:0:1"))
            .collect();
        canonical_json(&serde_json::json!({
            "schemaVersion": "layrs.projection-restore.v1",
            "preview": preview,
            "directory": "restore-1-1",
            "directoryIdentity": "fs:v1:0000000000000001:0000000000000002",
            "oldIdentity": witnesses,
            "newIdentity": witnesses,
        }))
        .unwrap()
    }

    fn fixture(c: &Connection) -> (ProviderImportPrefixRecord, ProvenanceIndex, RestorePreview) {
        let data = Fixture::new("G1", false);
        // SQL DATA fixture only: this exercises detached metadata and mapping
        // commitments, not FORMAT authentication, content bytes or admission.
        let mut snapshot = data.source.initial().clone();
        snapshot.entries[0].state = layrs_core::projection::EntryState::Materialized;
        snapshot.entries[0].writable = true;
        snapshot.validate().unwrap();
        let bytes = snapshot.canonical_bytes().unwrap();
        let digest = database::digest(0, None, &bytes);
        c.execute_batch(
            "CREATE TABLE projection_snapshots(generation INTEGER PRIMARY KEY,digest TEXT NOT NULL,parent_digest TEXT,payload BLOB NOT NULL) STRICT;
             CREATE TABLE projection_selections(generation INTEGER PRIMARY KEY,digest TEXT NOT NULL,payload BLOB NOT NULL) STRICT;
             CREATE TABLE projection_captures(sequence INTEGER PRIMARY KEY,digest TEXT NOT NULL,projection_generation INTEGER NOT NULL,payload BLOB NOT NULL) STRICT;
             CREATE TABLE projection_restore_history(operation_id TEXT PRIMARY KEY,capture_sequence INTEGER NOT NULL,payload BLOB NOT NULL) STRICT;
             CREATE TABLE projection_content(file_id TEXT PRIMARY KEY,manifest_id TEXT NOT NULL,bytes INTEGER NOT NULL) STRICT;
             CREATE TABLE projection_publications(sequence INTEGER PRIMARY KEY,request_digest TEXT,receipt_digest TEXT,receipt_bytes BLOB,next_snapshot BLOB,projection_generation INTEGER,projection_digest TEXT,after_state BLOB) STRICT;",
        ).unwrap();
        c.execute(
            "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
            params![digest, bytes],
        )
        .unwrap();
        selection::insert(c, 0, data.source.selection()).unwrap();
        let identity = data.allocation.format.identity.clone();
        let mut patch = CapturedPatch {
            schema_version: "layrs.projection-capture.v1".into(),
            sequence: 1,
            previous_patch_digest: None,
            projection_generation: 0,
            projection_digest: digest.clone(),
            identity: identity.clone(),
            changes: Vec::new(),
            digest: String::new(),
        };
        patch.digest = patch.computed_digest().unwrap();
        c.execute(
            "INSERT INTO projection_captures VALUES(1,?1,0,?2)",
            params![patch.digest, canonical_json(&patch).unwrap()],
        )
        .unwrap();
        let mut destination = identity.clone();
        destination.worktree_id = WorktreeId::parse("worktree:restore-destination").unwrap();
        let checkout = data.allocation.format.checkout_identity().to_owned();
        let selection_digest: String = c
            .query_row("SELECT digest FROM projection_selections", [], |row| {
                row.get(0)
            })
            .unwrap();
        let prefix = ProviderImportPrefixRecord {
            schema_version: "layrs.projection-provider-import-prefix.v2".into(),
            kind: ProviderImportKind::ManagedOrdinary,
            source: ImportSource::Ordinary {
                admission: None,
                identity: identity.clone(),
                format_digest: TOKEN.into(),
                private_schema: 7,
                checkout_identity: Some(checkout.clone()),
                version: ImportVersion {
                    generation: 0,
                    snapshot_digest: digest.clone(),
                    selection_digest,
                    capture_count: 1,
                    capture_head: Some(patch.digest),
                    effective_image_digest: RAW.into(),
                },
            },
            destination_identity: destination,
            inherited_segments: rowset(0),
            ancestry_root_receipt: None,
            closure: ImportClosure {
                snapshots: rowset(1),
                selections: rowset(1),
                captures: rowset(1),
                publications: rowset(0),
                catalogs: rowset(0),
                descriptors: rowset(0),
                activation_maps: rowset(0),
                transition_receipts: rowset(0),
                companions: rowset(0),
                companion_files: rowset(0),
                companion_deletions: rowset(0),
                content_mappings: rowset(0),
                restore_history: Some(rowset(1)),
            },
        };
        let lineage = ProvenanceIndex {
            segments: Vec::new(),
            receipts: Vec::new(),
            local_identity: identity.clone(),
            capture_count: 1,
            local_min_generation: 0,
        };
        let entry = &snapshot.entries[0];
        let layrs_core::scm::TreeEntryTarget::File { file_id } = entry.target.as_ref().unwrap()
        else {
            panic!("fixture file required")
        };
        let before = CapturedFile {
            file_id: FileId::from_bytes(b"preimage-only"),
            bytes: b"preimage-only".len() as u64,
            executable: false,
        };
        let after = CapturedFile {
            file_id: file_id.clone(),
            bytes: 7,
            executable: entry.executable,
        };
        for file in [&before, &after] {
            c.execute(
                "INSERT INTO projection_content VALUES(?1,?2,?3)",
                params![file.file_id.as_str(), TOKEN, file.bytes],
            )
            .unwrap();
        }
        let preview = RestorePreview {
            operation_id: "1-1".into(),
            destination: Some(RestoreDestination {
                identity,
                checkout_identity: checkout,
            }),
            source: RestoreSource::Capture {
                sequence: 1,
                paths: vec![entry.path.clone()],
            },
            projection_generation: 0,
            projection_digest: digest,
            checkout_digest: TOKEN.into(),
            after_checkout_digest: TOKEN.into(),
            before: BTreeMap::from([(entry.path.clone(), Some(before))]),
            after: BTreeMap::from([(entry.path.clone(), Some(after))]),
            digest: String::new(),
        };
        c.execute(
            "INSERT INTO projection_restore_history VALUES('1-1',1,?1)",
            [payload(&preview)],
        )
        .unwrap();
        (prefix, lineage, preview)
    }

    #[test]
    #[ignore = "owned audited Windows SQLite process; restore metadata closure"]
    fn canonical_history_includes_restore_only_preimage_and_shared_capture_operations() {
        child::run(
            concat!(
                module_path!(),
                "::canonical_history_includes_restore_only_preimage_and_shared_capture_operations"
            ),
            120,
            || {
                child::declare();
                let bare = BareConnection::reference().unwrap();
                let c = bare.connection();
                let (prefix, lineage, mut preview) = fixture(c);
                preview.operation_id = "2-1".into();
                c.execute(
                    "INSERT INTO projection_restore_history VALUES('2-1',1,?1)",
                    [payload(&preview)],
                )
                .unwrap();
                let mut ids = BTreeSet::new();
                let mut lengths = BTreeMap::new();
                extend_content(c, &prefix, &lineage, &mut ids, &mut lengths, None).unwrap();
                let preimage = preview.before.values().next().unwrap().as_ref().unwrap();
                assert!(ids.contains(preimage.file_id.as_str()));
                assert_eq!(
                    lengths.get(preimage.file_id.as_str()),
                    Some(&preimage.bytes)
                );
                assert_eq!(ids.len(), 2);
                let empty = BTreeSet::new();
                let prefix_id = prefix.digest().unwrap();
                let scope = scoped::ScopeView {
                    kind: ImportDatabaseKind::SharedOrdinary,
                    prefix_id: &prefix_id,
                    catalog_ids: &empty,
                    activation_ids: &empty,
                    receipt_ids: &empty,
                    companion_ids: &empty,
                    content_file_ids: &ids,
                    capture_lengths: &lengths,
                    receipts: &[],
                    lineage: &lineage,
                };
                let closure = scoped::compute_from_scope(
                    c,
                    ImportDatabaseKind::SharedOrdinary,
                    &prefix,
                    &scope,
                    None,
                )
                .unwrap();
                assert_eq!(closure.restore_history.as_ref().unwrap().count, 2);
                assert_eq!(closure.content_mappings.count, 2);
                c.execute(
                    "DELETE FROM projection_content WHERE file_id=?1",
                    [preimage.file_id.as_str()],
                )
                .unwrap();
                assert!(scoped::compute_from_scope(
                    c,
                    ImportDatabaseKind::SharedOrdinary,
                    &prefix,
                    &scope,
                    None
                )
                .is_err());
                bare.finish(Ok(())).unwrap();
            },
        );
    }

    #[test]
    #[ignore = "owned audited Windows SQLite process; legacy restore frontier"]
    fn legacy_prefix_refuses_scoped_restore_but_allows_local_tail() {
        child::run(
            concat!(
                module_path!(),
                "::legacy_prefix_refuses_scoped_restore_but_allows_local_tail"
            ),
            120,
            || {
                child::declare();
                let bare = BareConnection::reference().unwrap();
                let c = bare.connection();
                let (mut prefix, _, _) = fixture(c);
                prefix.schema_version = "layrs.projection-provider-import-prefix.v1".into();
                prefix.closure.restore_history = None;
                if let ImportSource::Ordinary {
                    checkout_identity, ..
                } = &mut prefix.source
                {
                    *checkout_identity = None;
                }
                assert!(require_supported_scope(c, &prefix, None).is_err());
                c.execute(
                    "UPDATE projection_restore_history SET capture_sequence=2",
                    [],
                )
                .unwrap();
                require_supported_scope(c, &prefix, None).unwrap();
                bare.finish(Ok(())).unwrap();
            },
        );
    }

    #[test]
    #[ignore = "owned audited Windows SQLite process; restore metadata refusals"]
    fn history_refuses_checkout_origin_canonical_id_capture_and_oversized_rows() {
        child::run(
            concat!(
                module_path!(),
                "::history_refuses_checkout_origin_canonical_id_capture_and_oversized_rows"
            ),
            120,
            || {
                child::declare();
                let bare = BareConnection::reference().unwrap();
                let c = bare.connection();
                let (prefix, lineage, preview) = fixture(c);
                let read = || {
                    extend_content(
                        c,
                        &prefix,
                        &lineage,
                        &mut BTreeSet::new(),
                        &mut BTreeMap::new(),
                        None,
                    )
                };
                let mut wrong = preview.clone();
                wrong.destination.as_mut().unwrap().checkout_identity =
                    "fs:v1:ffffffffffffffff:ffffffffffffffff".into();
                c.execute(
                    "UPDATE projection_restore_history SET payload=?1",
                    [payload(&wrong)],
                )
                .unwrap();
                assert!(read().is_err());
                wrong = preview.clone();
                wrong.destination.as_mut().unwrap().identity.worktree_id =
                    WorktreeId::parse("worktree:wrong-origin").unwrap();
                c.execute(
                    "UPDATE projection_restore_history SET payload=?1",
                    [payload(&wrong)],
                )
                .unwrap();
                assert!(read().is_err());
                let mut noncanonical = payload(&preview);
                noncanonical.push(b' ');
                c.execute(
                    "UPDATE projection_restore_history SET payload=?1",
                    [noncanonical],
                )
                .unwrap();
                assert!(read().is_err());
                c.execute(
                    "UPDATE projection_restore_history SET operation_id='2-1',payload=?1",
                    [payload(&preview)],
                )
                .unwrap();
                assert!(read().is_err());
                c.execute(
                    "UPDATE projection_restore_history SET operation_id='1-1'",
                    [],
                )
                .unwrap();
                c.execute("DELETE FROM projection_captures", []).unwrap();
                assert!(read().is_err());
                c.execute(
                    "UPDATE projection_restore_history SET payload=zeroblob(8388609)",
                    [],
                )
                .unwrap();
                let failure = read().unwrap_err().to_string();
                assert!(
                    failure.contains("row or aggregate exceeds bound"),
                    "{failure}"
                );
                c.execute(
                    "UPDATE projection_restore_history SET operation_id=?1,payload=x'00'",
                    ["1".repeat(97)],
                )
                .unwrap();
                assert!(read()
                    .unwrap_err()
                    .to_string()
                    .contains("row or aggregate exceeds bound"));
                bare.finish(Ok(())).unwrap();
            },
        );
    }
}

/// Merge only this prefix's history. An ancestor must call this with its own
/// frontier before computing its closure, rather than reuse final-source IDs.
pub(super) fn extend_content(
    c: &Connection,
    prefix: &ProviderImportPrefixRecord,
    lineage: &ProvenanceIndex,
    content_ids: &mut BTreeSet<String>,
    lengths: &mut BTreeMap<String, u64>,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    require_supported_scope(c, prefix, cancel)?;
    if !prefix.supports_restore_history() {
        return Ok(());
    }
    let version = match &prefix.source {
        ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => version,
    };
    let (count, bytes, min_id, max_id, min_payload, max_payload, min_sequence):
        (u64, u64, u64, u64, u64, u64, Option<i64>) = c.query_row(
        "SELECT count(*),coalesce(sum(8+length(CAST(operation_id AS BLOB))+length(payload)),0),coalesce(min(length(CAST(operation_id AS BLOB))),0),coalesce(max(length(CAST(operation_id AS BLOB))),0),coalesce(min(length(payload)),0),coalesce(max(length(payload)),0),min(capture_sequence) FROM projection_restore_history WHERE capture_sequence<=?1",
        [version.capture_count],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
    )?;
    if count > MAX_ROWS
        || bytes > MAX_BYTES
        || max_id > MAX_ID
        || max_payload > MAX_PAYLOAD
        || (count > 0 && (min_id == 0 || min_payload == 0 || min_sequence.is_none_or(|n| n <= 0)))
    {
        return Err(invalid(
            "import restore history row or aggregate exceeds bound",
        ));
    }
    let mut statement = c.prepare(
        "SELECT operation_id,capture_sequence,payload FROM projection_restore_history WHERE capture_sequence<=?1 ORDER BY operation_id COLLATE BINARY",
    )?;
    let mut rows = statement.query([version.capture_count])?;
    let mut seen = 0u64;
    let mut seen_bytes = 0u64;
    while let Some(row) = rows.next()? {
        super::super::cancellation::check(cancel)?;
        let id: String = row.get(0)?;
        let sequence: u64 = row.get(1)?;
        let payload: Vec<u8> = row.get(2)?;
        seen += 1;
        seen_bytes = seen_bytes
            .checked_add(8 + id.len() as u64 + payload.len() as u64)
            .filter(|n| *n <= bytes)
            .ok_or_else(|| invalid("import restore history changed after preflight"))?;
        if seen > count
            || id.is_empty()
            || id.len() as u64 > MAX_ID
            || payload.is_empty()
            || payload.len() as u64 > MAX_PAYLOAD
            || sequence == 0
            || sequence > version.capture_count
        {
            return Err(invalid(
                "import restore history row changed after preflight",
            ));
        }
        let (origin, first_generation, max_generation) =
            lineage.capture_source(sequence, version.generation)?;
        let patch =
            capture::storage::load_metadata_for_origin(c, sequence, origin, max_generation)?;
        if patch.projection_generation < first_generation
            || patch.projection_generation > version.generation
        {
            return Err(invalid(
                "import restore capture escaped historical generation interval",
            ));
        }
        let at = lineage
            .segments()
            .partition_point(|segment| segment.capture_count_through < sequence);
        let origin_prefix = match lineage
            .segments()
            .get(at)
            .filter(|segment| sequence > segment.capture_count_before)
        {
            Some(_) => {
                &lineage
                    .receipts()
                    .get(at)
                    .ok_or_else(|| invalid("import restore origin receipt missing"))?
                    .source_prefix
            }
            None => prefix,
        };
        let checkout = origin_prefix
            .authenticated_checkout_identity()?
            .ok_or_else(|| invalid("import restore origin lacks checkout commitment"))?;
        let base = offline::snapshot_at(
            c,
            patch.projection_generation,
            Some(&patch.projection_digest),
        )?
        .snapshot;
        let selected = selection::at(c, patch.projection_generation)?;
        selected
            .validate_snapshot(&base)
            .map_err(|e| invalid(e.to_string()))?;
        let preview = restore::imported_history_preview(&payload, &id, &patch, &base, checkout)?;
        for path in preview.before.keys().chain(preview.after.keys()) {
            if !selected.includes(path) {
                return Err(invalid("import restore path escaped historical selection"));
            }
        }
        for file in preview
            .before
            .values()
            .chain(preview.after.values())
            .flatten()
        {
            if file.bytes > capture::MAX_CAPTURE_FILE_BYTES {
                return Err(invalid("import restore file length exceeds bound"));
            }
            let id = file.file_id.as_str().to_owned();
            if lengths
                .insert(id.clone(), file.bytes)
                .is_some_and(|prior| prior != file.bytes)
            {
                return Err(invalid("import restore FileId has conflicting lengths"));
            }
            content_ids.insert(id);
            if content_ids.len() > MAX_ROWS as usize || lengths.len() > MAX_ROWS as usize {
                return Err(invalid("import restore content scope exceeds row bound"));
            }
        }
    }
    if seen != count || seen_bytes != bytes {
        return Err(invalid("import restore history changed after preflight"));
    }
    Ok(())
}
