//! A capture records its next change index while it is accepted (council
//! s22): when the acceptance then fails, the index names content the Store
//! never received, bound to a Tree whose projection may be cached. The next
//! capture must not trust it (council s23 review r3).
use layrs_client_core::native_scm::{CapturePlan, NativeWorkspace};
use layrs_store_local::scm_v1::{self, fsck::FsckOptions};
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

/// Writes `text` at `path`, dated an hour ago: old enough for a change
/// index entry proven now to be trusted later.
fn write_old(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
}

#[test]
fn a_capture_after_a_failed_acceptance_reads_what_it_did_not_store() {
    // The only test of this binary: a refused acceptance gives up at once.
    // SAFETY: set before any thread of this test reads the environment.
    unsafe { std::env::set_var("LAYRS_TEST_ACQUISITION_MS", "200") };
    let root = std::env::temp_dir().join(format!("layrs-failed-accept-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("d")).unwrap();
    for i in 0..4 {
        write_old(&root.join(format!("d/f{i}.txt")), &format!("line {i}\n"));
    }
    let workspace = NativeWorkspace::initialize(&root, "Failed acceptance").unwrap();
    // A capture that proves every file into the index.
    let _ = workspace.capture().unwrap();
    write_old(&root.join("d/f1.txt"), "edited, never stored\n");
    {
        // A shared session of this process: the capture's exclusive open
        // fails, after its index and projection were recorded.
        let held = workspace.open_store().unwrap();
        assert!(workspace.capture().is_err());
        drop(held);
    }
    match workspace.capture().unwrap() {
        CapturePlan::Change(_) => {}
        CapturePlan::Unchanged { .. } => panic!("the edit was not captured"),
    }
    assert!(!workspace.status_report(false).unwrap().dirty.unwrap());
    let report = scm_v1::fsck_existing(workspace.store_root(), FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    drop(workspace);
    let _ = fs::remove_dir_all(&root);
}
