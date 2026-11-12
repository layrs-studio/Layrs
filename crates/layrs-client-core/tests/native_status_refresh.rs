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

/// The index with its entries resolved by the base projection (an index is
/// written against it, council s3 D1).
fn resolved_index(workspace: &NativeWorkspace) -> ChangeIndex {
    let mut index = ChangeIndex::load(workspace.store_root(), workspace.worktree_id()).unwrap();
    let state = workspace.state().unwrap();
    let step = state
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == state.worktree.id())
        .and_then(|w| w.base_step_id())
        .cloned()
        .unwrap();
    let projection = workspace.read_step_projection(&step).unwrap();
    assert!(
        index.resolve(&projection),
        "index bound to the base projection"
    );
    assert!(!index.has_unresolved());
    index
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
    let index = resolved_index(&workspace);
    let before = index.entry("b.txt").unwrap().clone();
    // Wait past the margin, so a refreshed proof makes the entry trusted.
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    let report = workspace.status_report(true).unwrap();
    assert_eq!(report.dirty, Some(false));
    let after = resolved_index(&workspace);
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
    let last = resolved_index(&workspace);
    assert_eq!(last.entry("d/a.txt").cloned(), old_a);
    let _ = fs::remove_dir_all(&root);
}

/// A file no other handle may read: opened without sharing on Windows,
/// made unreadable on Unix.
struct Held {
    #[cfg(windows)]
    _file: fs::File,
    #[cfg(unix)]
    path: std::path::PathBuf,
}

fn hold(path: &std::path::Path) -> Held {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap();
        assert!(fs::read(path).is_err(), "the held file is unreadable");
        Held { _file: file }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
        Held {
            path: path.to_owned(),
        }
    }
}

#[cfg(unix)]
impl Drop for Held {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o644));
    }
}

/// `status` answers whether the folder changed: a file whose length differs
/// from the base's, or that the base lacks, changed without being read (as
/// git compares sizes before hashing), so even an unreadable one is
/// reported; a same-length edit is read and found.
#[test]
fn status_decides_a_length_change_or_a_new_file_without_reading_it() {
    let (root, workspace) = workspace();
    // Same length: read, found, then found equal again.
    fs::write(root.join("b.txt"), b"BETA\n").unwrap();
    assert_eq!(workspace.status_report(true).unwrap().dirty, Some(true));
    fs::write(root.join("b.txt"), b"beta\n").unwrap();
    assert_eq!(workspace.status_report(true).unwrap().dirty, Some(false));
    // Another length, unreadable.
    fs::write(root.join("d/a.txt"), b"alpha, longer\n").unwrap();
    let held = hold(&root.join("d/a.txt"));
    assert_eq!(workspace.status_report(true).unwrap().dirty, Some(true));
    drop(held);
    fs::write(root.join("d/a.txt"), b"alpha\n").unwrap();
    assert_eq!(workspace.status_report(true).unwrap().dirty, Some(false));
    // A new file, unreadable.
    fs::write(root.join("new.txt"), b"new\n").unwrap();
    let held = hold(&root.join("new.txt"));
    assert_eq!(workspace.status_report(true).unwrap().dirty, Some(true));
    drop(held);
    let _ = fs::remove_dir_all(&root);
}
