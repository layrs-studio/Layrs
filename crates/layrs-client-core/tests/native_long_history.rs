//! A history longer than the complete admission resolver loads (council s23
//! review r2). Debug builds lower its bounds (`LAYRS_TEST_HISTORY_LIMIT`):
//! past them only the bounded resolver can admit a write, and the debug
//! oracle has no verdict, so this test checks the outcomes directly.
use layrs_client_core::native_scm::{CapturePlan, NativeWorkspace};
use layrs_store_local::scm_v1::{self, fsck::FsckOptions};
use std::fs;

#[test]
fn writes_go_on_past_the_complete_resolvers_history_bound() {
    // The only test of this binary: the variable is read by every
    // resolver in the process.
    // SAFETY: set before any thread of this test reads the environment.
    unsafe { std::env::set_var("LAYRS_TEST_HISTORY_LIMIT", "24") };
    let root = std::env::temp_dir().join(format!("layrs-long-history-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("d")).unwrap();
    for i in 0..8 {
        fs::write(root.join(format!("d/f{i}.txt")), format!("line {i}\n")).unwrap();
    }
    let workspace = NativeWorkspace::initialize(&root, "Long history").unwrap();
    let mut steps = Vec::new();
    // Each capture adds a Step and an Operation: 40 of each, past 24.
    for k in 0..40 {
        fs::write(
            root.join(format!("d/f{}.txt", k % 8)),
            format!("line {k}, edited\n"),
        )
        .unwrap();
        match workspace.capture().unwrap() {
            CapturePlan::Change(change) => steps.push(change.step_id().clone()),
            CapturePlan::Unchanged { .. } => panic!("capture {k} saw no change"),
        }
    }
    assert!(!workspace.status_report(false).unwrap().dirty.unwrap());
    // A restore far back, then a capture on top of it.
    workspace.restore_step(&steps[3], None).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("d/f3.txt")).unwrap(),
        "line 3, edited\n"
    );
    fs::write(root.join("d/new.txt"), b"after the restore\n").unwrap();
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Change(_)
    ));
    let report = scm_v1::fsck_existing(workspace.store_root(), FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    drop(workspace);
    let _ = fs::remove_dir_all(&root);
}
