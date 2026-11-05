use super::*;
use crate::projection::provider::{database_io::with_reference, owned_test as child};
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use layrs_vfs::Cancellation;

#[test]
#[ignore = "owned audited SQLite child; schema2 reclaims a file key only after its last manifest"]
fn schema2_manifest_reclamation_preserves_siblings_and_claims() {
    child::run(
        concat!(
            module_path!(),
            "::schema2_manifest_reclamation_preserves_siblings_and_claims"
        ),
        120,
        || {
            child::declare();
            with_reference(SQL, |c| {
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                schema_v2::migrate(c, &Cancellation::default())?;
                let one = FileManifest::from_chunks(
                    ChunkingDescriptor::deterministic_fixed(6).unwrap(),
                    &[Chunk::new(b"abcdef".to_vec()).unwrap()],
                )
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
                c.execute("INSERT INTO files VALUES(?1,6)", [one.file_id().as_str()])?;
                for manifest in [&one, &two] {
                    c.execute(
                        "INSERT INTO manifests VALUES(?1,?2,6,?3)",
                        params![
                            manifest.id().as_str(),
                            manifest.file_id().as_str(),
                            canonical_json(manifest).unwrap()
                        ],
                    )?;
                }
                c.execute(
                    "INSERT INTO worktrees VALUES('owner','member','database',?1,'ready')",
                    [b"marker".as_slice()],
                )?;
                c.execute(
                    "INSERT INTO roots VALUES('root','owner','database','operation','digest',?1)",
                    [b"claim".as_slice()],
                )?;
                c.execute(
                    "INSERT INTO root_manifests VALUES('root',?1)",
                    [one.id().as_str()],
                )?;
                assert!(
                    delete_manifest_metadata(c, one.id().as_str(), CatalogSchema::V2).is_err(),
                    "FK protection must prevent deleting a claimed manifest"
                );
                let count: u64 = c.query_row("SELECT count(*) FROM manifests", [], |r| r.get(0))?;
                assert_eq!(count, 2);
                c.execute("DELETE FROM root_manifests WHERE root_id='root'", [])?;
                delete_manifest_metadata(c, one.id().as_str(), CatalogSchema::V2).unwrap();
                assert_eq!(
                    c.query_row(
                        "SELECT bytes FROM files WHERE file_id=?1",
                        [one.file_id().as_str()],
                        |r| r.get::<_, u64>(0)
                    )?,
                    6
                );
                delete_manifest_metadata(c, two.id().as_str(), CatalogSchema::V2).unwrap();
                assert_eq!(
                    c.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, u64>(0))?,
                    0
                );
                c.execute_batch("ROLLBACK")?;
                assert_eq!(require_catalog_schema(c).unwrap(), CatalogSchema::V1);
                Ok(())
            })
            .unwrap();
        },
    );
}
