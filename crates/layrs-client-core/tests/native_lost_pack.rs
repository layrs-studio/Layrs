//! A registered pack that cannot be read no longer refuses the whole Store
//! (council s17, B1a): readers answer what a readable copy proves, writers
//! act only on that, and a full capture brings back from the working tree
//! the content only the lost pack held.
use layrs_client_core::native_scm::{CapturePlan, NativeWorkspace};
use layrs_core::scm::CaptureReason;
use layrs_store_local::scm_v1::checkout::CheckoutContent;
use std::fs;
use std::path::PathBuf;

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn base_step(workspace: &NativeWorkspace) -> layrs_core::scm::StepId {
    let state = workspace.state().unwrap();
    state
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == state.worktree.id())
        .and_then(|w| w.base_step_id())
        .cloned()
        .unwrap()
}

fn file_bytes(workspace: &NativeWorkspace, path: &str) -> Vec<u8> {
    let files = workspace.read_step_files(&base_step(workspace)).unwrap();
    let entry = files.iter().find(|entry| entry.path == path).unwrap();
    match &entry.content {
        CheckoutContent::File { bytes, .. } => bytes.clone(),
        other => panic!("{path} is not a file: {other:?}"),
    }
}

#[test]
fn a_lost_pack_of_old_content_is_brought_back_by_a_full_capture() {
    let root = std::env::temp_dir().join(format!("layrs-lost-pack-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("a.txt"), b"alpha\n").unwrap();
    fs::write(root.join("d/b.txt"), b"beta\n").unwrap();
    let fixture = Fixture(root.clone());
    let workspace = NativeWorkspace::initialize(&root, "Lost pack").unwrap();
    let store = workspace.store_root().to_owned();
    if !layrs_store_local::scm_v1::open(&store).unwrap().compact() {
        // Formats 2 and 5 keep no registered pack catalog.
        return;
    }
    // Each capture now writes a pack file.
    layrs_store_local::scm_v1::set_inline_pack_limit(&store, Some(0));
    let packs = || -> Vec<PathBuf> {
        fs::read_dir(store.join("objects").join("packs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "lp2"))
            .collect()
    };
    fs::write(root.join("c.txt"), b"gamma\n").unwrap();
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Change(_)
    ));
    let second = packs();
    assert_eq!(second.len(), 1, "{second:?}");
    fs::write(root.join("d/e.txt"), b"epsilon\n").unwrap();
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Change(_)
    ));
    // The second capture's pack (c.txt, its Step and View) is lost; the
    // current Step, View and Trees are in the third, the Space and Layers in
    // the first.
    fs::remove_file(&second[0]).unwrap();
    // As the next command, a new process, would find it.
    layrs_store_local::scm_v1::forget_loaded_packs();

    // A read-only observation still answers.
    assert_eq!(workspace.status_report(true).unwrap().dirty, Some(false));
    // A plain step that trusts its index still finds nothing to capture
    // (its best-effort recovery cannot carry files it did not read).
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    // The capture before a folder is deleted reads every file beside an
    // unreadable pack, and stores again what the Store can no longer prove
    // (council s18 r2): the lost content is readable again.
    assert!(matches!(
        workspace
            .capture_before_deletion(CaptureReason::ManualSnapshot)
            .unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    // A full capture then finds everything proven.
    workspace
        .capture_verifying_all(CaptureReason::ManualSnapshot)
        .unwrap();
    for (path, bytes) in [
        ("a.txt", &b"alpha\n"[..]),
        ("c.txt", b"gamma\n"),
        ("d/b.txt", b"beta\n"),
        ("d/e.txt", b"epsilon\n"),
    ] {
        assert_eq!(file_bytes(&workspace, path), bytes, "{path}");
    }
    // The lost pack also held a Step of the history: until repair rebuilds
    // it (from the Store's own rows, or a server), a new Step cannot prove
    // its ancestry, and says which Step is missing.
    fs::write(root.join("f.txt"), b"phi\n").unwrap();
    let refused = workspace.capture().unwrap_err().to_string();
    assert!(refused.contains("Step parent"), "{refused}");
    // The refusal names the remedy: a lost Step needs repair, a full
    // capture cannot bring it back (council s18 r3).
    assert!(refused.contains("needs repair"), "{refused}");
    drop(fixture);
}
