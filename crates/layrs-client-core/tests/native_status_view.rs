//! The status view (council s20): `status` decides from it alone, in clear
//! cases, and its answer never differs from the full comparison.
use layrs_client_core::native_scm::{CapturePlan, NativeWorkspace};
use layrs_store_local::scm_v1::checkout::CheckoutReader;
use layrs_store_local::scm_v1::status_view::{StatusView, Verdict};
use std::fs;
use std::path::{Path, PathBuf};

fn workspace(label: &str) -> (PathBuf, NativeWorkspace) {
    let root = std::env::temp_dir().join(format!(
        "layrs-status-view-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(root.join("d/e")).unwrap();
    fs::write(root.join("d/a.txt"), b"alpha\n").unwrap();
    fs::write(root.join("d/e/deep.txt"), b"deep\n").unwrap();
    fs::write(root.join("b.txt"), b"beta\n").unwrap();
    fs::write(root.join("a-b.txt"), b"sorted before d/\n").unwrap();
    // Older than the trust margin: proven at init, not racy.
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    let workspace = NativeWorkspace::initialize(&root, "Status view").unwrap();
    (root, workspace)
}

fn base_step(workspace: &NativeWorkspace) -> String {
    let state = workspace.state().unwrap();
    state
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == state.worktree.id())
        .and_then(|w| w.base_step_id())
        .unwrap()
        .as_str()
        .to_owned()
}

/// The view's verdict on the folder, and `status`'s answer (which must agree
/// whenever the view decides).
fn verdict(root: &Path, workspace: &NativeWorkspace) -> (Verdict, bool) {
    let view = StatusView::load(workspace.store_root(), workspace.worktree_id().as_str())
        .expect("a status view");
    assert_eq!(view.step(), Some(base_step(workspace).as_str()));
    let checkout = CheckoutReader::open(root, None).unwrap();
    let verdict = view.verdict(&checkout).unwrap();
    let dirty = workspace.status_report(true).unwrap().dirty.unwrap();
    // The full comparison, never the view.
    assert_eq!(workspace.status_report(false).unwrap().dirty, Some(dirty));
    match verdict {
        Verdict::Clean => assert!(!dirty),
        Verdict::Changed => assert!(dirty),
        Verdict::Unknown => {}
    }
    (verdict, dirty)
}

#[test]
fn the_view_decides_clear_cases_and_defers_the_others() {
    let (root, workspace) = workspace("cases");
    assert_eq!(verdict(&root, &workspace), (Verdict::Clean, false));
    // A new folder is walked (council s21 r3): empty, it changes nothing.
    fs::create_dir_all(root.join("empty/deeper")).unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Clean, false));
    fs::remove_dir_all(root.join("empty")).unwrap();
    // Another length: changed without reading.
    fs::write(root.join("d/a.txt"), b"alpha, longer\n").unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Changed, true));
    // The same length as the base, other bytes: the content decides.
    fs::write(root.join("d/a.txt"), b"ALPHA\n").unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Unknown, true));
    // The base bytes again, with another listing: the content decides.
    fs::write(root.join("d/a.txt"), b"alpha\n").unwrap();
    assert!(!verdict(&root, &workspace).1);
    // A tracked file deleted, deep down.
    fs::remove_file(root.join("d/e/deep.txt")).unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Changed, true));
    fs::write(root.join("d/e/deep.txt"), b"deep\n").unwrap();
    assert!(!verdict(&root, &workspace).1);
    // A new file, then ignored.
    fs::write(root.join("d/new.log"), b"log\n").unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Changed, true));
    fs::write(root.join(".layrsignore"), b"*.log\n").unwrap();
    // The ignore file itself is new: a change.
    assert!(verdict(&root, &workspace).1);
    fs::remove_file(root.join(".layrsignore")).unwrap();
    fs::remove_file(root.join("d/new.log")).unwrap();
    // A new folder with a file, deep down: a change.
    fs::create_dir_all(root.join("fresh/deeper")).unwrap();
    fs::write(root.join("fresh/deeper/x.txt"), b"x\n").unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Changed, true));
    fs::remove_dir_all(root.join("fresh")).unwrap();
    // A tracked folder replaced by a file of that name.
    fs::remove_dir_all(root.join("d/e")).unwrap();
    fs::write(root.join("d/e"), b"now a file\n").unwrap();
    assert!(verdict(&root, &workspace).1);
    let _ = fs::remove_dir_all(&root);
}

/// `.git` (a submodule's or a git worktree's file) and `.layrs` are never
/// part of a checkout, at any depth, for the view as for the scan (council
/// s21 r1).
#[test]
fn a_nested_git_entry_is_never_a_change() {
    let (root, workspace) = workspace("nested-git");
    fs::write(
        root.join("d/.git"),
        b"gitdir: ../.git/modules/d
",
    )
    .unwrap();
    fs::create_dir_all(root.join("d/e/.layrs")).unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Clean, false));
    let _ = fs::remove_dir_all(&root);
}

/// A name the scan refuses (not canonical NFC) is never outweighed by a
/// change found elsewhere (council s21 r3): the view leaves it to the full
/// path, which reports it, in either order of discovery.
#[test]
fn a_refused_name_outweighs_a_change() {
    let (root, workspace) = workspace("refused-name");
    fs::write(root.join("b.txt"), b"beta, longer\n").unwrap();
    for name in ["0e\u{301}.log", "ze\u{301}.log"] {
        fs::write(root.join("d").join(name), b"decomposed\n").unwrap();
        let view = StatusView::load(workspace.store_root(), workspace.worktree_id().as_str())
            .expect("a status view");
        let checkout = CheckoutReader::open(&root, None).unwrap();
        assert_eq!(view.verdict(&checkout).unwrap(), Verdict::Unknown, "{name}");
        assert!(workspace.status_report(true).is_err(), "{name}");
        assert!(workspace.status_report(false).is_err(), "{name}");
        fs::remove_file(root.join("d").join(name)).unwrap();
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_ignored_new_file_leaves_the_folder_clean() {
    let (root, workspace) = workspace("ignored");
    fs::write(root.join(".layrsignore"), b"*.log\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Change(_)
    ));
    assert_eq!(verdict(&root, &workspace), (Verdict::Clean, false));
    fs::write(root.join("d/new.log"), b"log\n").unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Clean, false));
    // A new folder holding only ignored files changes nothing.
    fs::create_dir_all(root.join("cache/inner")).unwrap();
    fs::write(root.join("cache/inner/run.log"), b"log\n").unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Clean, false));
    let _ = fs::remove_dir_all(&root);
}

/// Files written just before a capture are proven too soon to be trusted:
/// the view keeps their FileIds and hashes them, so the first `status` after
/// a step needs no full comparison.
#[test]
fn files_captured_right_after_their_write_are_hashed() {
    let (root, workspace) = workspace("racy");
    fs::write(root.join("b.txt"), b"beta 2\n").unwrap();
    fs::write(root.join("d/new.txt"), b"new\n").unwrap();
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Change(_)
    ));
    assert_eq!(verdict(&root, &workspace), (Verdict::Clean, false));
    // Same length, other bytes: the hash finds it.
    fs::write(root.join("b.txt"), b"BETA 2\n").unwrap();
    assert_eq!(verdict(&root, &workspace), (Verdict::Changed, true));
    let _ = fs::remove_dir_all(&root);
}
