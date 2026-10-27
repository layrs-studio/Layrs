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
