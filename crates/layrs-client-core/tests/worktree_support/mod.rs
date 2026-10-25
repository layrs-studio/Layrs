#![allow(dead_code)]
use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store_local::scm_v1;
use std::{fs, path::PathBuf};

pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("layrs-worktree-safety-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    pub fn directory(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir(&root).unwrap();
        root
    }
    pub fn primary(&self) -> NativeWorkspace {
        let root = self.directory("primary");
        fs::write(root.join("same.txt"), b"base\0bytes").unwrap();
        NativeWorkspace::initialize(root, "Worktree safety").unwrap()
    }
    pub fn secondary(&self, primary: &NativeWorkspace) -> NativeWorkspace {
        let state = own(primary);
        primary
            .create_worktree(
                self.directory("secondary"),
                state.active_layer_id().unwrap(),
                state.base_step_id().unwrap(),
            )
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
pub fn own(workspace: &NativeWorkspace) -> WorktreeViewState {
    workspace
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == workspace.worktree_id())
        .unwrap()
        .clone()
}
pub fn assert_clean(workspace: &NativeWorkspace, protected: &[StepId]) {
    let report =
        scm_v1::fsck_existing(workspace.store_root(), scm_v1::FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    assert!(!report.is_truncated());
    for step in protected {
        assert!(!report
            .gc_reachability
            .unreachable_objects
            .contains(&step.to_string()));
    }
}
