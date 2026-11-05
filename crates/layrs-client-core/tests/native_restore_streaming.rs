use layrs_client_core::native_scm::{NativeClientError, NativeWorkspace};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("layrs-restore-stream-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn digest(path: &Path) -> blake3::Hash {
    let mut input = fs::File::open(path).unwrap();
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = input.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    hash.finalize()
}

#[test]
#[ignore = "physical100MiB restore/switch/undo qualification"]
fn large_projection_restore_guards_history_and_switch_preserve_exact_bytes() {
    let fixture = Fixture::new();
    let path = fixture.0.join("asset.bin");
    let mut file = fs::File::create(&path).unwrap();
    let block = (0..1024 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<_>>();
    for _ in 0..100 {
        file.write_all(&block).unwrap();
    }
    drop(file);
    drop(block);
    let original = digest(&path);
    let workspace = NativeWorkspace::initialize(&fixture.0, "Large restore").unwrap();
    let initial = workspace
        .create_marker("large", "")
        .unwrap()
        .target_step_id()
        .clone();
    let main = workspace.state().unwrap().layers[0].id().clone();
    assert!(!workspace.is_dirty().unwrap());
    assert!(
        workspace.read_step_files(&initial).is_err(),
        "bounded byte API remains explicit"
    );
    fs::write(&path, b"small current").unwrap();
    workspace.capture().unwrap();
    let preview = workspace.preview_restore_step(&initial, None).unwrap();
    assert_eq!(preview.changes.len(), 1);
    assert_eq!(preview.changes[0].after_bytes, Some(100 * 1024 * 1024));
    assert!(preview.changes[0].text_omitted);
    assert!(preview.collisions.is_empty());
    let before = workspace.state().unwrap().operation.id().clone();
    fs::write(&path, b"changed after preview").unwrap();
    assert!(matches!(
        workspace.restore_step_guarded(&initial, None, &preview.expected),
        Err(NativeClientError::StaleCapture)
    ));
    assert_eq!(workspace.state().unwrap().operation.id(), &before);
    assert_eq!(fs::read(&path).unwrap(), b"changed after preview");
    workspace.capture().unwrap();
    let preview = workspace.preview_restore_step(&initial, None).unwrap();
    workspace
        .restore_step_guarded(&initial, None, &preview.expected)
        .unwrap();
    assert_eq!(digest(&path), original);
    workspace.undo().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"changed after preview");
    workspace.redo(None).unwrap();
    assert_eq!(digest(&path), original);
    workspace.create_layer("small").unwrap();
    workspace.switch_layer("small").unwrap();
    fs::write(&path, b"branch small bytes").unwrap();
    workspace.capture().unwrap();
    workspace.switch_layer(main.as_str()).unwrap();
    assert_eq!(digest(&path), original);
    let reopened = NativeWorkspace::open(&fixture.0).unwrap();
    assert!(!reopened.is_dirty().unwrap());
    assert!(!reopened.needs_repair().unwrap());
    let store = layrs_store_local::scm_v1::fsck_existing(reopened.store_root(), Default::default())
        .unwrap();
    assert!(store.is_clean());
}
