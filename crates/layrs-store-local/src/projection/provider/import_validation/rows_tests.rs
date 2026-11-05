//! Compare detached commitments against actual SQLite value normalization.
//! Owned memory SQL only; no imported closure or physical authority is claimed.
use super::*;
use crate::projection::provider::{database_io::BareConnection, process_profile::tests::child};
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest};

#[test]
#[ignore = "owned audited SQLite memory process; mapping DATA versus SQL commitments"]
fn detached_mapping_commitment_matches_sql_and_rejects_substitution() {
    child::run(
        concat!(
            module_path!(),
            "::detached_mapping_commitment_matches_sql_and_rejects_substitution"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch("CREATE TABLE mappings(file_id TEXT PRIMARY KEY,manifest_id TEXT NOT NULL,bytes ANY NOT NULL) STRICT").unwrap();
            let mut mappings = BTreeMap::new();
            let spec = Spec {
                kind: Kind::ContentMappings,
                preflight_sql: "SELECT count(*),0 FROM mappings",
                rows_sql:
                    "SELECT file_id,manifest_id,bytes FROM mappings ORDER BY file_id COLLATE BINARY",
                params: &[],
                max_rows: MAX_CLOSURE_ROWS,
                max_bytes: MAX_CLOSURE_BYTES,
            };
            let sql = || {
                let (count, bytes): (u64, u64) = c.query_row(
                "SELECT count(*),coalesce(sum(length(CAST(file_id AS BLOB))+length(CAST(manifest_id AS BLOB))+CASE typeof(bytes) WHEN 'integer' THEN 8 WHEN 'real' THEN 8 ELSE length(CAST(bytes AS BLOB)) END),0) FROM mappings",
                [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
                digest_one(c, &spec, count, bytes, None)
            };
            assert_eq!(
                content_mapping_rowset_data(&mappings, None).unwrap(),
                sql().unwrap()
            );
            for (body, length) in [
                (b"first mapping".as_slice(), 0),
                (b"second mapping".as_slice(), i64::MAX as u64),
            ] {
                let manifest = FileManifest::from_chunks(
                    ChunkingDescriptor::single(),
                    &[Chunk::new(body.to_vec()).unwrap()],
                )
                .unwrap();
                mappings.insert(manifest.file_id().clone(), (manifest.id().clone(), length));
            }
            // Reverse insertion order cannot influence either commitment.
            for (file, (manifest, length)) in mappings.iter().rev() {
                c.execute(
                    "INSERT INTO mappings VALUES(?1,?2,?3)",
                    rusqlite::params![file.as_str(), manifest.as_str(), *length as i64],
                )
                .unwrap();
            }
            let bound = content_mapping_rowset_data(&mappings, None).unwrap();
            assert_eq!(bound, sql().unwrap());
            let mut subset = mappings.clone();
            subset.pop_first();
            assert_ne!(content_mapping_rowset_data(&subset, None).unwrap(), bound);
            let key = mappings.keys().next().unwrap().clone();
            let mut substituted = mappings.clone();
            substituted.get_mut(&key).unwrap().0 = mappings.values().last().unwrap().0.clone();
            assert_ne!(
                content_mapping_rowset_data(&substituted, None).unwrap(),
                bound
            );
            c.execute(
                "UPDATE mappings SET bytes=CAST(bytes AS TEXT) WHERE file_id=?1",
                [key.as_str()],
            )
            .unwrap();
            assert_ne!(
                sql().unwrap(),
                bound,
                "TEXT and INTEGER cannot share a commitment"
            );
            c.execute(
                "UPDATE mappings SET bytes=1.5 WHERE file_id=?1",
                [key.as_str()],
            )
            .unwrap();
            assert!(sql().err().unwrap().to_string().contains("REAL"));
            substituted.get_mut(&key).unwrap().1 = i64::MAX as u64 + 1;
            assert!(content_mapping_rowset_data(&substituted, None).is_err());
            let cancel = Cancellation::default();
            cancel.cancel();
            assert!(content_mapping_rowset_data(&mappings, Some(&cancel)).is_err());
            bare.finish(Ok(())).unwrap();
        },
    );
}

fn closure_specs<'a>(denials: &'a [&'a dyn ToSql]) -> Vec<Spec<'a>> {
    let mut result = Vec::new();
    for kind in [
        Kind::Snapshots,
        Kind::Selections,
        Kind::Captures,
        Kind::Publications,
        Kind::Catalogs,
        Kind::Descriptors,
        Kind::ActivationMaps,
        Kind::TransitionReceipts,
        Kind::Companions,
        Kind::CompanionFiles,
        Kind::CompanionDeletions,
        Kind::ContentMappings,
        Kind::RestoreHistory,
    ] {
        result.push(Spec {
            kind,
            preflight_sql: "SELECT 0,0",
            rows_sql: "SELECT 0 WHERE 0",
            params: &[],
            max_rows: MAX_CLOSURE_ROWS,
            max_bytes: MAX_CLOSURE_BYTES,
        });
    }
    result.push(Spec {
        kind: Kind::DenialObservations,
        preflight_sql: super::super::scoped::denial_history::PREFLIGHT_SQL,
        rows_sql: super::super::scoped::denial_history::ROWS_SQL,
        params: denials,
        max_rows: MAX_CLOSURE_ROWS,
        max_bytes: MAX_CLOSURE_BYTES,
    });
    result
}

#[test]
#[ignore = "owned audited SQLite memory process; versioned denial rowset DATA"]
fn denial_rowset_extends_legacy_shapes_and_hashes_full_ordered_document_bytes() {
    child::run(
        concat!(
            module_path!(),
            "::denial_rowset_extends_legacy_shapes_and_hashes_full_ordered_document_bytes"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch(crate::projection::retention::SQL).unwrap();
            let low = format!("blake3:{}", "1".repeat(64));
            let high = format!("blake3:{}", "2".repeat(64));
            let selected = serde_json::to_string(&[&low, &high]).unwrap();
            let bind: [&dyn ToSql; 1] = [&selected];
            let specs = closure_specs(&bind);
            let legacy = stream(
                c,
                &specs[..12],
                &preflight(c, &specs[..12], None).unwrap(),
                None,
            )
            .unwrap();
            let extended = stream(
                c,
                &specs[..13],
                &preflight(c, &specs[..13], None).unwrap(),
                None,
            )
            .unwrap();
            let empty = stream(c, &specs, &preflight(c, &specs, None).unwrap(), None).unwrap();
            assert!(legacy.restore_history.is_none() && legacy.denial_observations.is_none());
            assert!(extended.restore_history.is_some() && extended.denial_observations.is_none());
            assert_eq!(legacy.snapshots, extended.snapshots);
            assert_eq!(extended.snapshots, empty.snapshots);
            // Independent frozen v1 domain and empty framing, unaffected by v3.
            let mut old = blake3::Hasher::new();
            old.update(b"layrs-provider-import-snapshots-v1\0");
            old.update(&0u64.to_be_bytes());
            assert_eq!(
                legacy.snapshots.digest,
                format!("blake3:{}", old.finalize())
            );
            let mut hash = blake3::Hasher::new();
            hash.update(b"layrs-provider-import-denial-observations-v1\0");
            hash.update(&0u64.to_be_bytes());
            assert_eq!(
                empty.denial_observations.unwrap().digest,
                format!("blake3:{}", hash.finalize())
            );
            let a = b"{\"first\":1}";
            let b = b"{\"second\":2}";
            for (id, body) in [(&high, b.as_slice()), (&low, a.as_slice())] {
                c.execute(
                    "INSERT INTO projection_observations VALUES(?1,?2)",
                    rusqlite::params![id, body],
                )
                .unwrap();
            }
            let pre = preflight(c, &specs, None).unwrap();
            assert_eq!(pre.metadata_bytes(), 142 + a.len() as u64 + b.len() as u64);
            let actual = stream(c, &specs, &pre, None)
                .unwrap()
                .denial_observations
                .unwrap();
            assert_eq!(actual.count, 2); // Documents, not paths inside those documents.
            let mut expected = blake3::Hasher::new();
            expected.update(b"layrs-provider-import-denial-observations-v1\0");
            expected.update(&2u64.to_be_bytes());
            for (id, body) in [(&low, a.as_slice()), (&high, b.as_slice())] {
                expected.update(&2u32.to_be_bytes());
                expected.update(&[2]);
                expected.update(&71u64.to_be_bytes());
                expected.update(id.as_bytes());
                expected.update(&[3]);
                expected.update(&(body.len() as u64).to_be_bytes());
                expected.update(body);
            }
            assert_eq!(actual.digest, format!("blake3:{}", expected.finalize()));
            let mut aggregate = closure_specs(&bind);
            aggregate[0].preflight_sql = "SELECT 0,1073741824";
            assert!(preflight(c, &aggregate, None)
                .err()
                .unwrap()
                .to_string()
                .contains("closure metadata exceeds"));
            aggregate[0].preflight_sql = "SELECT 1000000,0";
            assert!(preflight(c, &aggregate, None)
                .err()
                .unwrap()
                .to_string()
                .contains("closure row count exceeds"));
            // Bypass only the disposable test DB's CHECK to model a corrupt
            // stored BLOB; preflight must refuse before streaming/owned copy.
            c.execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            c.execute(
                "UPDATE projection_observations SET payload=zeroblob(67108865) WHERE digest=?1",
                [&low],
            )
            .unwrap();
            c.execute_batch("PRAGMA ignore_check_constraints=OFF")
                .unwrap();
            let before = c.total_changes();
            assert!(preflight(c, &specs, None)
                .err()
                .unwrap()
                .to_string()
                .contains("stored body type/length"));
            assert_eq!(before, c.total_changes());
            bare.finish(Ok(())).unwrap();
        },
    );
}
