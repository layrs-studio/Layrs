//! A new restore advances history even when its capture changes are identical.
//! Real admitted files and production writers; no process-crash claim.
use super::*;
use crate::projection::{
    format, provider::process_profile::tests::child, ProjectionStore, RestoreSource, SessionMode,
};
use layrs_core::scm::RepoPath;
use std::fs;

fn historical_rows(store: &ProjectionStore, through: u64) -> Vec<(String, u64, Vec<u8>)> {
    format::access(store, SessionMode::Shared).unwrap().run(|access| {
        let mut statement = access.connection.prepare(
            "SELECT operation_id,capture_sequence,payload FROM projection_restore_history WHERE capture_sequence<=?1 ORDER BY operation_id COLLATE BINARY",
        )?;
        let rows = statement.query_map([through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }).unwrap()
}

#[test]
#[ignore = "ROOT source release and audited owned native child480/outer540 required"]
fn managed_restore_advances_frontier_without_disabling_capture_deduplication() {
    child::run(
        concat!(
            module_path!(),
            "::managed_restore_advances_frontier_without_disabling_capture_deduplication"
        ),
        480,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            fs::write(root.join("README.md"), b"restore target A").unwrap();
            let target = store.capture_patch(state.generation).unwrap();
            assert_eq!(store.capture_patch(state.generation).unwrap(), target);
            let source = RestoreSource::Capture {
                sequence: target.sequence,
                paths: vec![path],
            };

            fs::write(root.join("README.md"), b"dirty bytes B").unwrap();
            let first = store.preview_restore(state.generation, &source).unwrap();
            let first_receipt = store.apply_restore(&first).unwrap();
            assert_eq!(first_receipt.capture.sequence, target.sequence + 1);
            assert_eq!(first_receipt.capture.changes, target.changes);
            assert_eq!(
                first_receipt.capture.previous_patch_digest.as_deref(),
                Some(target.digest.as_str())
            );
            let committed_rows = historical_rows(&store, first_receipt.capture.sequence);
            assert_eq!(committed_rows.len(), 1);
            assert_eq!(committed_rows[0].0, first.operation_id);

            fs::write(root.join("README.md"), b"dirty bytes C").unwrap();
            let second = store.preview_restore(state.generation, &source).unwrap();
            let second_receipt = store.apply_restore(&second).unwrap();
            assert_ne!(first.operation_id, second.operation_id);
            assert_eq!(
                second_receipt.capture.sequence,
                first_receipt.capture.sequence + 1
            );
            assert_eq!(
                second_receipt.capture.changes,
                first_receipt.capture.changes
            );
            assert_eq!(
                second_receipt.capture.previous_patch_digest.as_deref(),
                Some(first_receipt.capture.digest.as_str())
            );
            assert_eq!(
                historical_rows(&store, first_receipt.capture.sequence),
                committed_rows
            );
            assert_eq!(
                historical_rows(&store, second_receipt.capture.sequence).len(),
                2
            );
            assert_eq!(
                store.capture_patch(state.generation).unwrap(),
                second_receipt.capture
            );
            assert_eq!(store.apply_restore(&first).unwrap(), first_receipt);
            assert_eq!(
                historical_rows(&store, second_receipt.capture.sequence).len(),
                2
            );
            drop(store);

            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(
                historical_rows(&store, first_receipt.capture.sequence),
                committed_rows
            );
            let inverse = store
                .preview_restore(
                    state.generation,
                    &RestoreSource::Inverse {
                        operation_id: second.operation_id,
                    },
                )
                .unwrap();
            let inverse_receipt = store.apply_restore(&inverse).unwrap();
            assert_eq!(
                inverse_receipt.capture.sequence,
                second_receipt.capture.sequence + 1
            );
            assert_eq!(fs::read(root.join("README.md")).unwrap(), b"dirty bytes C");
            assert_eq!(
                historical_rows(&store, first_receipt.capture.sequence),
                committed_rows
            );
            format::access(&store, SessionMode::Shared)
                .unwrap()
                .run(|access| {
                    access.with_content(|c, scope| {
                        crate::projection::retention::fsck_with_scope(c, &store, Some(scope))
                    })
                })
                .unwrap();
        },
    );
}
