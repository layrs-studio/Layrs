//! Owned artifact test for the final export publication cancellation boundary.
use super::*;
use crate::scm_v1::vfs_recovery::VfsExportWriter;
use layrs_core::scm::{FileId, RepoPath};
use std::{fs, io};

#[test]
#[ignore = "owned fresh audited host; cancellation before export manifest publication"]
fn cancelled_final_verification_retains_tree_without_completion_manifest() {
    child::run(
        concat!(
            module_path!(),
            "::cancelled_final_verification_retains_tree_without_completion_manifest"
        ),
        90,
        || {
            child::declare();
            let root = crate::projection::provider::database_io::tests::support::owned_root(
                "export-writer-check",
            );
            let mut writer =
                VfsExportWriter::create(&root, "00000000-0000-0000-0000-000000000063").unwrap();
            let container = writer.container_root().to_owned();
            let tree = writer.tree_root();
            let bytes = vec![0x63; 64 * 1024 + 13];
            writer
                .write_file(
                    &RepoPath::parse("preserved.bin").unwrap(),
                    &FileId::from_bytes(&bytes),
                    bytes.len() as u64,
                    &mut |out| out.write_all(&bytes),
                )
                .unwrap();
            let mut publication_boundary_seen = false;
            let error = writer
                .finish_with_check(br#"{"completeSelected":true}"#, &mut || {
                    if container.join("manifest.pending").is_file() {
                        publication_boundary_seen = true;
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "cancel before publication",
                        ));
                    }
                    Ok(())
                })
                .unwrap_err();
            assert!(publication_boundary_seen);
            assert_eq!(error.kind(), io::ErrorKind::Interrupted);
            assert!(!container.join("manifest.json").exists());
            assert!(container.join("manifest.pending").is_file());
            assert_eq!(fs::read(tree.join("preserved.bin")).unwrap(), bytes);
        },
    );
}
