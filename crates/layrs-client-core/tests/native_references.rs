use layrs_client_core::native_scm::*;
use layrs_store_local::scm_v1;
use std::{fs, path::PathBuf};

struct Fixture(PathBuf, NativeWorkspace);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("layrs-references-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"original").unwrap();
        let workspace = NativeWorkspace::initialize(&root, "References").unwrap();
        Self(root, workspace)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
#[test]
fn ambiguous_marker_names_require_ids_and_replacement_preserves_original() {
    let f = Fixture::new();
    let original = f.1.create_marker("milestone", "first").unwrap();
    let original_bytes = serde_jcs::to_vec(&original).unwrap();
    fs::write(f.0.join("file"), b"corrected").unwrap();
    let corrected = f.1.create_marker("milestone", "correction").unwrap();
    assert_ne!(original.id(), corrected.id());
    assert_ne!(original.target_step_id(), corrected.target_step_id());
    assert!(f.1.resolve_step_name("@marker:milestone").is_err());
    f.1.hide_marker(original.id()).unwrap();
    assert_eq!(
        f.1.resolve_step_name("@marker:milestone").unwrap().step_id,
        *corrected.target_step_id()
    );
    fs::write(f.0.join("file"), b"still dirty").unwrap();
    let restored = f.1.show_marker(original.id()).unwrap();
    assert_eq!(serde_jcs::to_vec(&restored).unwrap(), original_bytes);
    assert_eq!(fs::read(f.0.join("file")).unwrap(), b"still dirty");
    assert!(f.1.is_dirty().unwrap());
    assert!(f.1.resolve_step_name("@marker:milestone").is_err());
}
#[test]
fn corrected_release_is_a_new_version_and_invalid_requests_do_not_capture() {
    let f = Fixture::new();
    let original = f.1.create_release("v1", "initial version").unwrap();
    let original_bytes = serde_jcs::to_vec(&original).unwrap();
    fs::write(f.0.join("file"), b"corrected").unwrap();
    let before = f.1.state().unwrap().operation.id().clone();
    assert!(f.1.create_release("v1", "replace silently").is_err());
    assert!(f.1.create_release("", "invalid").is_err());
    assert!(f.1.create_marker("", "invalid").is_err());
    assert_eq!(f.1.state().unwrap().operation.id(), &before);
    assert!(f.1.is_dirty().unwrap());
    assert_eq!(fs::read(f.0.join("file")).unwrap(), b"corrected");
    let corrected = f.1.create_release("v1.1", "corrects v1").unwrap();
    assert_ne!(original.id(), corrected.id());
    assert_ne!(original.target_step_id(), corrected.target_step_id());
    let reopened = NativeWorkspace::open(&f.0).unwrap();
    let old = reopened
        .releases()
        .unwrap()
        .into_iter()
        .find(|r| r.id() == original.id())
        .unwrap();
    assert_eq!(serde_jcs::to_vec(&old).unwrap(), original_bytes);
    assert_eq!(
        reopened.resolve_step_name("@release:v1").unwrap().step_id,
        *original.target_step_id()
    );
    assert_eq!(
        reopened.resolve_step_name("@release:v1.1").unwrap().step_id,
        *corrected.target_step_id()
    );
    assert!(
        scm_v1::fsck_existing(reopened.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}
