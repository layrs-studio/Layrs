use super::*;
use crate::projection::provider::{database_io::with_reference, owned_test as child};
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use rusqlite::params;

const CATALOG: &str = "CREATE TABLE manifests(id TEXT PRIMARY KEY,file_id TEXT NOT NULL,bytes INTEGER NOT NULL,payload BLOB NOT NULL) STRICT;
CREATE TABLE chunks(id TEXT PRIMARY KEY,bytes INTEGER NOT NULL) STRICT;
CREATE TABLE manifest_chunks(manifest_id TEXT NOT NULL,ordinal INTEGER NOT NULL,chunk_id TEXT NOT NULL,PRIMARY KEY(manifest_id,ordinal)) STRICT;";

fn manifest(chunk_bytes: usize) -> FileManifest {
    let chunks: Vec<_> = b"abcdef"
        .chunks(chunk_bytes)
        .map(|bytes| Chunk::new(bytes.to_vec()).unwrap())
        .collect();
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(chunk_bytes as u64).unwrap(),
        &chunks,
    )
    .unwrap()
}

fn insert(c: &Connection, manifest: &FileManifest) {
    c.execute(
        "INSERT INTO manifests VALUES(?1,?2,?3,?4)",
        params![
            manifest.id().as_str(),
            manifest.file_id().as_str(),
            manifest.byte_length(),
            canonical_json(manifest).unwrap()
        ],
    )
    .unwrap();
    for (ordinal, part) in manifest.chunks().iter().enumerate() {
        c.execute(
            "INSERT INTO chunks VALUES(?1,?2) ON CONFLICT(id) DO NOTHING",
            params![part.chunk_id().as_str(), part.byte_length()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO manifest_chunks VALUES(?1,?2,?3)",
            params![
                manifest.id().as_str(),
                ordinal as u64,
                part.chunk_id().as_str()
            ],
        )
        .unwrap();
    }
}

#[test]
#[ignore = "owned audited SQLite child; trusted provider manifest choice and stable fallback"]
fn trusted_representation_is_preferred_and_corruption_never_falls_back() {
    child::run(
        concat!(
            module_path!(),
            "::trusted_representation_is_preferred_and_corruption_never_falls_back"
        ),
        120,
        || {
            child::declare();
            with_reference(CATALOG, |c| {
                c.execute_batch("BEGIN DEFERRED")?;
                let (one, two) = (manifest(6), manifest(3));
                assert_eq!(one.file_id(), two.file_id());
                insert(c, &two);
                insert(c, &one);
                let cancel = Cancellation::default();
                let read = |trusted| read_manifest(c, one.file_id(), trusted, 6, &cancel);
                assert_eq!(read(Some(one.id())).unwrap(), Some(one.clone()));
                assert_eq!(read(Some(two.id())).unwrap(), Some(two.clone()));
                let stable = if one.id() < two.id() { &one } else { &two };
                assert_eq!(read(None).unwrap().as_ref(), Some(stable));
                let missing = FileManifestId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
                assert_eq!(read(Some(&missing)).unwrap().as_ref(), Some(stable));
                c.execute(
                    "UPDATE manifests SET file_id=?1 WHERE id=?2",
                    params![format!("blake3:{}", "1".repeat(64)), one.id().as_str()],
                )?;
                assert!(
                    read(Some(one.id())).is_err(),
                    "existing trusted id with foreign FileId cannot fall back"
                );
                assert_eq!(read(Some(two.id())).unwrap(), Some(two.clone()));
                c.execute(
                    "UPDATE manifests SET file_id=?1,payload=zeroblob(262145) WHERE id=?2",
                    params![one.file_id().as_str(), one.id().as_str()],
                )?;
                assert!(
                    read(Some(one.id())).is_err(),
                    "existing oversized trusted row cannot fall back"
                );
                Ok(())
            })
            .unwrap();
        },
    );
}
