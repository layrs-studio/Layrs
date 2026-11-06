//! `status` refreshes the proof time of change index entries that were read
//! again only because they were "racy" (listed too close to their proof),
//! like git's index refresh, and never records anything else.
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_store_local::scm_v1::change_index::ChangeIndex;
use std::fs;

fn workspace() -> (std::path::PathBuf, NativeWorkspace) {
    let root = std::env::temp_dir().join(format!("layrs-status-refresh-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("d/a.txt"), b"alpha\n").unwrap();
    fs::write(root.join("b.txt"), b"beta\n").unwrap();
    let workspace = NativeWorkspace::initialize(&root, "Status refresh").unwrap();
    (root, workspace)
}

#[test]
fn status_refreshes_only_racy_entries_of_unchanged_files() {
    let (root, workspace) = workspace();
    // Written right before the capture: within the timestamp margin, so the
    // capture's entry is racy and a status reads the file again.
    fs::write(root.join("b.txt"), b"beta 2\n").unwrap();
    assert!(matches!(
        workspace.capture().unwrap(),
        layrs_client_core::native_scm::CapturePlan::Change(_)
    ));
    let index = ChangeIndex::load(workspace.store_root(), workspace.worktree_id()).unwrap();
    let before = index.entry("b.txt").unwrap().clone();
    // Wait past the margin, so a refreshed proof makes the entry trusted.
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    let report = workspace.status_report(true).unwrap();
    assert_eq!(report.dirty, Some(false));
    let after = ChangeIndex::load(workspace.store_root(), workspace.worktree_id()).unwrap();
    let refreshed = after.entry("b.txt").unwrap();
    assert_eq!(refreshed.file_id, before.file_id);
    assert_eq!(refreshed.stat, before.stat);
    assert!(
        refreshed.verified_at > before.verified_at,
        "racy entry was not refreshed"
    );
    assert!(after.trusted("b.txt", &refreshed.stat).is_some());
    // An edited file is never recorded by status.
    fs::write(root.join("d/a.txt"), b"alpha edited\n").unwrap();
    let old_a = after.entry("d/a.txt").cloned();
    assert_eq!(workspace.status_report(true).unwrap().dirty, Some(true));
    let last = ChangeIndex::load(workspace.store_root(), workspace.worktree_id()).unwrap();
    assert_eq!(last.entry("d/a.txt").cloned(), old_a);
    let _ = fs::remove_dir_all(&root);
}
