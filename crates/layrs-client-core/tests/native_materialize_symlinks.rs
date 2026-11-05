#![cfg(target_os = "linux")]
use layrs_client_core::native_scm::*;
use layrs_core::scm::StepId;
use layrs_store_local::scm_v1::{
    self,
    materialize::{self, MaterializationFaults, MaterializationPoint},
};
use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    root: PathBuf,
    outside: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("layrs-symlink-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&base).unwrap();
        let root = base.join("checkout");
        let outside = base.join("external");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("safe"), [0, 255, 7]).unwrap();
        fs::write(root.join("p"), b"regular\0bytes").unwrap();
        Self { root, outside }
    }
    fn shape(&self, shape: usize) {
        let p = self.root.join("p");
        let metadata = fs::symlink_metadata(&p).unwrap();
        if metadata.is_dir() {
            fs::remove_file(p.join("child")).unwrap();
            fs::remove_dir(&p).unwrap();
        } else {
            fs::remove_file(&p).unwrap();
        }
        match shape {
            0 => fs::write(&p, b"regular\0bytes").unwrap(),
            1 => {
                fs::create_dir(&p).unwrap();
                fs::write(p.join("child"), [255, 0, 3]).unwrap();
            }
            2 => symlink(&self.outside, &p).unwrap(),
            3 => symlink("missing/relative-target", &p).unwrap(),
            _ => unreachable!(),
        }
    }
    fn prepare(&self) -> (NativeWorkspace, Vec<StepId>) {
        let workspace = NativeWorkspace::initialize(&self.root, "symlink changes").unwrap();
        let mut steps = vec![];
        for shape in 0..4 {
            if shape != 0 {
                self.shape(shape);
                workspace.capture().unwrap();
            }
            steps.push(
                workspace
                    .create_marker(&format!("shape{shape}"), "")
                    .unwrap()
                    .target_step_id()
                    .clone(),
            );
        }
        (workspace, steps)
    }
    fn assert_shape(&self, shape: usize) {
        let p = self.root.join("p");
        let metadata = fs::symlink_metadata(&p).unwrap();
        match shape {
            0 => {
                assert!(metadata.is_file());
                assert_eq!(fs::read(&p).unwrap(), b"regular\0bytes");
            }
            1 => {
                assert!(metadata.is_dir());
                assert_eq!(fs::read(p.join("child")).unwrap(), [255, 0, 3]);
            }
            2 => {
                assert!(metadata.file_type().is_symlink());
                assert_eq!(fs::read_link(&p).unwrap(), self.outside);
            }
            3 => {
                assert!(metadata.file_type().is_symlink());
                assert_eq!(
                    fs::read_link(&p).unwrap(),
                    PathBuf::from("missing/relative-target")
                );
            }
            _ => unreachable!(),
        }
        assert_eq!(fs::read(self.outside.join("safe")).unwrap(), [0, 255, 7]);
        assert_eq!(fs::read_dir(&self.outside).unwrap().count(), 1);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let base = self.root.parent().unwrap();
            assert_eq!(base.parent(), Some(std::env::temp_dir().as_path()));
            let _ = fs::remove_dir_all(base);
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
        if self.point == point && self.remaining.fetch_sub(1, Ordering::SeqCst) == 1 {
            Err(std::io::Error::other("symlink interrupted"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn file_directory_external_and_dangling_links_roundtrip_without_following_targets() {
    let f = Fixture::new();
    let (w, steps) = f.prepare();
    for from in 0..4 {
        for to in 0..4 {
            w.restore_step(&steps[from], None).unwrap();
            w.restore_step(&steps[to], None).unwrap();
            f.assert_shape(to);
            assert!(!w.is_dirty().unwrap());
        }
    }
    w.restore_step(&steps[1], None).unwrap();
    w.restore_step(&steps[2], None).unwrap();
    w.undo().unwrap();
    f.assert_shape(1);
    w.redo(None).unwrap();
    f.assert_shape(2);
    assert!(
        scm_v1::fsck_existing(w.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn link_changes_recover_at_each_apply_boundary_without_touching_external_targets() {
    for (from, to) in [(0, 2), (2, 0), (1, 2), (2, 1), (2, 3), (3, 2)] {
        for point in [
            MaterializationPoint::AfterJournal,
            MaterializationPoint::AfterBackup,
            MaterializationPoint::AfterDirectoryRemoval,
            MaterializationPoint::AfterPath,
            MaterializationPoint::BeforeMetadata,
            MaterializationPoint::AfterMetadata,
        ] {
            let f = Fixture::new();
            let (w, steps) = f.prepare();
            w.restore_step(&steps[from], None).unwrap();
            assert!(
                w.restore_step_with_faults(&steps[to], None, &Stop::at(point))
                    .is_err(),
                "{from}->{to} {point:?}"
            );
            let reopened = NativeWorkspace::open(&f.root).unwrap();
            assert_eq!(reopened.recover().unwrap(), 1);
            f.assert_shape(if point == MaterializationPoint::AfterMetadata {
                to
            } else {
                from
            });
            assert_eq!(reopened.recover().unwrap(), 0);
            assert!(!reopened.is_dirty().unwrap());
        }
    }
}

#[test]
fn interrupted_link_recovery_itself_can_resume() {
    for (from, to) in [(1, 2), (2, 1), (2, 3)] {
        for point in [
            MaterializationPoint::AfterBackup,
            MaterializationPoint::AfterDirectoryRemoval,
            MaterializationPoint::AfterPath,
        ] {
            let f = Fixture::new();
            let (w, steps) = f.prepare();
            w.restore_step(&steps[from], None).unwrap();
            assert!(w
                .restore_step_with_faults(
                    &steps[to],
                    None,
                    &Stop::at(MaterializationPoint::BeforeMetadata)
                )
                .is_err());
            assert!(materialize::recover_checkout_with_faults(
                w.store_root(),
                w.worktree_id(),
                &Stop::at(point)
            )
            .is_err());
            assert_eq!(w.recover().unwrap(), 1);
            f.assert_shape(from);
            assert!(!w.is_dirty().unwrap());
        }
    }
}

#[test]
fn changed_link_after_interruption_is_preserved_and_blocks_recovery() {
    let f = Fixture::new();
    let (w, steps) = f.prepare();
    w.restore_step(&steps[0], None).unwrap();
    assert!(w
        .restore_step_with_faults(
            &steps[2],
            None,
            &Stop::at(MaterializationPoint::BeforeMetadata)
        )
        .is_err());
    fs::remove_file(f.root.join("p")).unwrap();
    symlink("user-edited-target", f.root.join("p")).unwrap();
    assert!(w.recover().is_err());
    assert_eq!(
        fs::read_link(f.root.join("p")).unwrap(),
        PathBuf::from("user-edited-target")
    );
    assert!(w.needs_repair().unwrap());
    assert_eq!(fs::read(f.outside.join("safe")).unwrap(), [0, 255, 7]);
}
