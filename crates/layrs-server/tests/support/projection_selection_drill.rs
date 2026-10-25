//! Selected partial copies over the existing isolated HTTPS/PostgreSQL fixture.
use super::*;
use layrs_core::projection::{EntryState, PathSemantics, ProjectionSelection};
use layrs_store_local::projection::ProjectionStore;
use std::{fs, path::PathBuf};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("layrs-selected-tls-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn selection(paths: &[&str]) -> ProjectionSelection {
    ProjectionSelection::selected(
        paths.iter().map(|p| RepoPath::parse(*p).unwrap()).collect(),
        vec![],
        PathSemantics::PortableWindows,
    )
    .unwrap()
}
pub(super) fn clone_and_expand(read: &Http<'_>, remote: &NativeRemote, step: &StepId) {
    let selected = selection(&["public/allowed.txt"]);
    let root = Directory::new();
    let store = remote
        .clone_projection_selected_into(&root.0, &read.space, step, &selected)
        .unwrap();
    assert_eq!(
        fs::read(root.0.join("public/allowed.txt")).unwrap(),
        b"same bytes with three distinct access paths"
    );
    assert!(!root.0.join("private.txt").exists());
    assert!(!root.0.join("public/blocked.txt").exists());
    assert_eq!(store.selection().unwrap(), selected);
    assert_eq!(
        store
            .state()
            .unwrap()
            .snapshot
            .entries
            .iter()
            .filter(|e| e.state == EntryState::Materialized)
            .count(),
        1
    );

    let empty_root = Directory::new();
    let empty = selection(&[]);
    let store = remote
        .clone_projection_selected_into(&empty_root.0, &read.space, step, &empty)
        .unwrap();
    let before = store.state().unwrap();
    assert_eq!(before.generation, 0);
    assert!(!empty_root.0.join("public").exists());
    fs::create_dir(empty_root.0.join("public")).unwrap();
    fs::write(empty_root.0.join("public/allowed.txt"), b"do not overwrite").unwrap();
    assert!(
        remote
            .expand_projection_selection(&store, before.generation, &selected)
            .is_err()
    );
    assert_eq!(store.state().unwrap(), before);
    assert_eq!(store.selection().unwrap(), empty);
    assert_eq!(
        fs::read(empty_root.0.join("public/allowed.txt")).unwrap(),
        b"do not overwrite"
    );
    fs::remove_file(empty_root.0.join("public/allowed.txt")).unwrap();
    assert!(
        remote
            .expand_projection_selection(&store, before.generation + 1, &selected)
            .is_err()
    );
    // The same FileId at a denied exact path must never inherit the allowed grant.
    let expanded = selection(&["private.txt", "public/allowed.txt"]);
    let after = remote
        .expand_projection_selection(&store, before.generation, &expanded)
        .unwrap();
    assert_eq!(after.snapshot.base_step_id, *step);
    assert_eq!(
        after.snapshot.base_root_tree_id,
        before.snapshot.base_root_tree_id
    );
    assert_eq!(
        fs::read(empty_root.0.join("public/allowed.txt")).unwrap(),
        b"same bytes with three distinct access paths"
    );
    assert!(!empty_root.0.join("private.txt").exists());
    assert_eq!(
        after
            .snapshot
            .entries
            .iter()
            .find(|e| e.path.as_str() == "private.txt")
            .unwrap()
            .state,
        EntryState::Reserved
    );
    assert!(
        store
            .capture_patch(after.generation)
            .unwrap()
            .changes
            .is_empty()
    );
    ProjectionStore::open(&empty_root.0)
        .unwrap()
        .fsck()
        .unwrap();
}

pub(super) fn publication(
    remote: &NativeRemote,
    read: &Http<'_>,
    layer: &layrs_core::scm::LayerId,
) {
    let base = remote.bootstrap(&read.space).unwrap();
    let step = &base
        .layers
        .iter()
        .find(|l| l.record.id() == layer)
        .unwrap()
        .head_step_ids[0];
    let root = Directory::new();
    let selected = selection(&["public/allowed.txt"]);
    let store = remote
        .clone_projection_selected_into(&root.0, &read.space, step, &selected)
        .unwrap();
    let before = store.state().unwrap();
    let outside: Vec<_> = before
        .snapshot
        .entries
        .iter()
        .filter(|e| e.path.as_str() != "public/allowed.txt")
        .cloned()
        .collect();
    assert!(
        outside
            .iter()
            .any(|e| e.writable && e.state == EntryState::Unavailable)
    );
    fs::write(root.0.join("private.txt"), b"excluded local file stays").unwrap();
    fs::write(
        root.0.join("public/allowed.txt"),
        b"selected TLS publication",
    )
    .unwrap();
    let capture = store.capture_patch(before.generation).unwrap();
    assert_eq!(capture.changes.len(), 1);
    let published = remote
        .publish_projection(&store, capture.sequence, layer)
        .unwrap();
    assert_eq!(published.receipt.receipt.status, ReceiptStatus::Applied);
    assert!(!published.pending_local_acceptance);
    for expected in &outside {
        assert_eq!(
            published
                .projection
                .snapshot
                .entries
                .iter()
                .find(|e| e.path == expected.path),
            Some(expected)
        );
    }
    let received = remote.receive_projection(&store, layer).unwrap();
    assert_eq!(
        fs::read(root.0.join("private.txt")).unwrap(),
        b"excluded local file stays"
    );
    for expected in &outside {
        assert_eq!(
            received
                .snapshot
                .entries
                .iter()
                .find(|e| e.path == expected.path),
            Some(expected)
        );
    }
    assert_eq!(store.captured_patch(capture.sequence).unwrap(), capture);
    assert_eq!(store.selection().unwrap(), selected);
    let actual = read.file_id(
        &published.projection.snapshot.base_step_id,
        "public/allowed.txt",
    );
    assert_eq!(actual, FileId::from_bytes(b"selected TLS publication"));
    store.fsck().unwrap();
    eprintln!(
        "HTTPS selected projection: selected payload, exact denied alias, expansion CAS/collision, capture/publication and selective receive PASS"
    );
}
