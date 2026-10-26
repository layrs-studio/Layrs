use super::*;
use crate::projection::provider::{database_io::with_reference, owned_test as child};
use layrs_core::scm::{Chunk, ChunkingDescriptor};

const CATALOG: &str = "CREATE TABLE manifests(id TEXT PRIMARY KEY,file_id TEXT NOT NULL,bytes INTEGER NOT NULL,payload BLOB NOT NULL) STRICT;
CREATE TABLE chunks(id TEXT PRIMARY KEY,bytes INTEGER NOT NULL) STRICT;
CREATE TABLE manifest_chunks(manifest_id TEXT NOT NULL,ordinal INTEGER NOT NULL,chunk_id TEXT NOT NULL,PRIMARY KEY(manifest_id,ordinal)) STRICT;";

fn pair() -> (FileManifest, FileManifest) {
    let all = Chunk::new(b"abcdef".to_vec()).unwrap();
    let one =
        FileManifest::from_chunks(ChunkingDescriptor::deterministic_fixed(6).unwrap(), &[all])
            .unwrap();
    let two = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(3).unwrap(),
        &[
            Chunk::new(b"abc".to_vec()).unwrap(),
            Chunk::new(b"def".to_vec()).unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(one.file_id(), two.file_id());
    assert_ne!(one.id(), two.id());
    (one, two)
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
#[ignore = "owned audited SQLite child; exact and stable shared manifest reads"]
fn exact_manifest_reads_preserve_representations_and_reject_corruption() {
    child::run(
        concat!(
            module_path!(),
            "::exact_manifest_reads_preserve_representations_and_reject_corruption"
        ),
        120,
        || {
            child::declare();
            with_reference(CATALOG, |c| {
                c.execute_batch("BEGIN DEFERRED")?;
                let (one, two) = pair();
                insert(c, &two);
                insert(c, &one);
                assert_eq!(
                    read_manifest_optional(c, one.id()).unwrap(),
                    Some(one.clone())
                );
                assert_eq!(
                    read_manifest_optional(c, two.id()).unwrap(),
                    Some(two.clone())
                );
                let stable = if one.id() < two.id() { &one } else { &two };
                assert_eq!(
                    read_manifest_for(c, one.file_id()).unwrap().as_ref(),
                    Some(stable)
                );
                let missing = FileManifestId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
                assert_eq!(read_manifest_optional(c, &missing).unwrap(), None);
                c.execute(
                    "UPDATE manifests SET bytes=bytes+1 WHERE id=?1",
                    [one.id().as_str()],
                )?;
                assert!(read_manifest_optional(c, one.id()).is_err());
                assert_eq!(
                    read_manifest_optional(c, two.id()).unwrap(),
                    Some(two.clone())
                );
                c.execute(
                    "UPDATE manifests SET bytes=bytes-1,payload=zeroblob(262145) WHERE id=?1",
                    [one.id().as_str()],
                )?;
                assert!(
                    read_manifest_optional(c, one.id()).is_err(),
                    "present oversized payload must not become None"
                );
                Ok(())
            })
            .unwrap();
        },
    );
}
