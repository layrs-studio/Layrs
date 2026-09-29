use layrs_store_local::scm_v1::checkout::*;
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::PathBuf,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "layrs-stream-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn limits() -> CheckoutLimits {
    CheckoutLimits {
        max_entries: 100,
        max_depth: 10,
        max_file_bytes: 4 * 1024 * 1024,
        max_total_bytes: 8 * 1024 * 1024,
    }
}
#[test]
fn streaming_scan_rejects_continuous_changes_and_checks_limits_before_content() {
    let root = Fixture::new();
    let path = root.0.join("changing.bin");
    fs::write(&path, vec![3; 2 * 1024 * 1024]).unwrap();
    let reader = CheckoutReader::open(&root.0, None).unwrap();
    let mut calls = 0;
    let error = reader
        .capture_streaming(
            limits(),
            &BTreeSet::new(),
            &mut |_| {
                calls += 1;
                let mut file = fs::OpenOptions::new().append(true).open(&path)?;
                file.write_all(b"changed")?;
                Ok(())
            },
            &mut || Ok(()),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(calls >= 3);
    let mut bounds = limits();
    bounds.max_file_bytes = 1024;
    assert!(
        reader
            .capture_streaming(
                bounds,
                &BTreeSet::new(),
                &mut |_| panic!("limits before content"),
                &mut || Ok(())
            )
            .is_err()
    );
}
#[test]
fn streaming_projection_matches_legacy_and_keeps_tracked_ignored_paths() {
    let root = Fixture::new();
    fs::write(root.0.join(".layrsignore"), b"ignored\n").unwrap();
    fs::write(root.0.join("ignored"), b"tracked").unwrap();
    fs::write(root.0.join("binary"), [0, 255, 1]).unwrap();
    fs::write(root.0.join("empty"), []).unwrap();
    let reader = CheckoutReader::open(&root.0, None).unwrap();
    let tracked = BTreeSet::from(["ignored".into()]);
    let legacy = reader.capture_tracked(limits(), &tracked).unwrap();
    let (projection, manifests) = reader
        .capture_streaming(limits(), &tracked, &mut |_| Ok(()), &mut || Ok(()))
        .unwrap();
    assert_eq!(
        projection,
        legacy
            .iter()
            .map(CheckoutProjectionEntry::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        projection_tree_id(&projection).unwrap(),
        entries_tree_id(&legacy).unwrap()
    );
    assert!(manifests.iter().any(|m| m.byte_length() == 0));
}
