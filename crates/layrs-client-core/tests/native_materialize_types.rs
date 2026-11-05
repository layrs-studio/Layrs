use layrs_client_core::native_scm::*;
use layrs_core::scm::StepId;
use layrs_store_local::scm_v1::{
    self,
    materialize::{MaterializationFaults, MaterializationPoint},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("layrs-type-change-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        fs::write(p.join("p"), [0, 255, 1]).unwrap();
        fs::write(p.join("keep"), b"unchanged").unwrap();
        Self(p)
    }
    fn prepare(&self) -> (NativeWorkspace, StepId, StepId) {
        let w = NativeWorkspace::initialize(&self.0, "type changes").unwrap();
        let file = w
            .create_marker("file", "")
            .unwrap()
            .target_step_id()
            .clone();
        fs::remove_file(self.0.join("p")).unwrap();
        fs::create_dir(self.0.join("p")).unwrap();
        fs::create_dir(self.0.join("p/deep")).unwrap();
        fs::write(self.0.join("p/a"), b"first child\0").unwrap();
        fs::write(self.0.join("p/deep/b"), [0, 255, 7]).unwrap();
        w.capture().unwrap();
        let directory = w
            .create_marker("directory", "")
            .unwrap()
            .target_step_id()
            .clone();
        (w, file, directory)
    }
    fn assert_shape(&self, directory: bool) {
        assert_eq!(fs::read(self.0.join("keep")).unwrap(), b"unchanged");
        if directory {
            assert!(self.0.join("p").is_dir());
            assert_eq!(fs::read(self.0.join("p/a")).unwrap(), b"first child\0");
            assert_eq!(fs::read(self.0.join("p/deep/b")).unwrap(), [0, 255, 7]);
        } else {
            assert_eq!(fs::read(self.0.join("p")).unwrap(), [0, 255, 1]);
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
struct Stop {
    point: MaterializationPoint,
    remaining: AtomicUsize,
}
impl Stop {
    fn at(point: MaterializationPoint) -> Self {
        Self {
            point,
            remaining: AtomicUsize::new(1),
        }
    }
}
impl MaterializationFaults for Stop {
    fn check(&self, point: MaterializationPoint) -> std::io::Result<()> {
        if point == self.point && self.remaining.fetch_sub(1, Ordering::SeqCst) == 1 {
            Err(std::io::Error::other("type-change crash"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn nested_directory_to_file_and_back_preserve_exact_bytes_and_history() {
    let f = Fixture::new();
    let (w, file, directory) = f.prepare();
    w.restore_step(&file, None).unwrap();
    f.assert_shape(false);
    w.restore_step(&directory, None).unwrap();
    f.assert_shape(true);
    w.undo().unwrap();
    f.assert_shape(false);
    w.redo(None).unwrap();
    f.assert_shape(true);
    assert!(!w.is_dirty().unwrap());
    assert!(
        scm_v1::fsck_existing(w.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn type_changes_recover_at_every_phase_before_and_after_commit() {
    for target_directory in [false, true] {
        for point in [
            MaterializationPoint::AfterJournal,
            MaterializationPoint::AfterBackup,
            MaterializationPoint::AfterDirectoryRemoval,
            MaterializationPoint::AfterPath,
            MaterializationPoint::BeforeMetadata,
            MaterializationPoint::AfterMetadata,
        ] {
            let f = Fixture::new();
            let (w, file, directory) = f.prepare();
            if target_directory {
                w.restore_step(&file, None).unwrap();
            }
            let target = if target_directory { &directory } else { &file };
            let old = w.state().unwrap().operation.id().clone();
            assert!(
                w.restore_step_with_faults(target, None, &Stop::at(point))
                    .is_err(),
                "{target_directory:?} {point:?}"
            );
            let reopened = NativeWorkspace::open(&f.0).unwrap();
            assert_eq!(
                reopened.recover().unwrap(),
                1,
                "{target_directory:?} {point:?}"
            );
            f.assert_shape(if point == MaterializationPoint::AfterMetadata {
                target_directory
            } else {
                !target_directory
            });
            if point != MaterializationPoint::AfterMetadata {
                assert_eq!(reopened.state().unwrap().operation.id(), &old);
            }
            assert_eq!(reopened.recover().unwrap(), 0);
            assert!(!reopened.is_dirty().unwrap());
        }
    }
}

#[test]
fn edits_after_type_change_interruption_are_preserved_instead_of_removed() {
    let f = Fixture::new();
    let (w, file, _) = f.prepare();
    assert!(w
        .restore_step_with_faults(&file, None, &Stop::at(MaterializationPoint::BeforeMetadata))
        .is_err());
    fs::write(f.0.join("p"), b"editor changed newly installed file").unwrap();
    assert!(w.recover().is_err());
    assert_eq!(
        fs::read(f.0.join("p")).unwrap(),
        b"editor changed newly installed file"
    );
    assert!(w.needs_repair().unwrap());
}

#[test]
fn unexpected_directory_children_block_recovery_without_recursive_deletion() {
    let f = Fixture::new();
    let (w, file, directory) = f.prepare();
    w.restore_step(&file, None).unwrap();
    assert!(w
        .restore_step_with_faults(
            &directory,
            None,
            &Stop::at(MaterializationPoint::BeforeMetadata)
        )
        .is_err());
    fs::write(f.0.join("p/user-added"), b"valuable external file").unwrap();
    assert!(w.recover().is_err());
    assert_eq!(
        fs::read(f.0.join("p/user-added")).unwrap(),
        b"valuable external file"
    );
    assert!(w.needs_repair().unwrap());
    fs::rename(f.0.join("p/user-added"), f.0.join("saved-external")).unwrap();
    assert_eq!(w.recover().unwrap(), 1);
    f.assert_shape(false);
    assert_eq!(
        fs::read(f.0.join("saved-external")).unwrap(),
        b"valuable external file"
    );
}

#[test]
fn recovery_itself_can_be_interrupted_and_restarted_in_both_directions() {
    for target_directory in [false, true] {
        for point in [
            MaterializationPoint::AfterBackup,
            MaterializationPoint::AfterDirectoryRemoval,
            MaterializationPoint::AfterPath,
        ] {
            let f = Fixture::new();
            let (w, file, directory) = f.prepare();
            if target_directory {
                w.restore_step(&file, None).unwrap();
            }
            let target = if target_directory { &directory } else { &file };
            assert!(w
                .restore_step_with_faults(
                    target,
                    None,
                    &Stop::at(MaterializationPoint::BeforeMetadata)
                )
                .is_err());
            assert!(scm_v1::materialize::recover_checkout_with_faults(
                w.store_root(),
                w.worktree_id(),
                &Stop::at(point)
            )
            .is_err());
            assert_eq!(NativeWorkspace::open(&f.0).unwrap().recover().unwrap(), 1);
            f.assert_shape(!target_directory);
            assert!(!w.is_dirty().unwrap());
        }
    }
}
