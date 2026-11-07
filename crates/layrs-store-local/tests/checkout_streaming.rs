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
        // Parallel tests can read the same 100ns clock tick: add a counter.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "layrs-stream-scan-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
/// Several root subdirectories are walked as parallel subtrees: the result
/// equals the sequential walk's, every subtree's chunks reach the observer,
/// and the entry limit holds across all of them.
#[test]
fn parallel_subtrees_capture_like_a_sequential_walk_and_share_the_limits() {
    let root = Fixture::new();
    fs::write(root.0.join("top.txt"), b"top").unwrap();
    for directory in 0..6 {
        let path = root.0.join(format!("d{directory}/nested"));
        fs::create_dir_all(&path).unwrap();
        for file in 0..5 {
            fs::write(
                root.0.join(format!("d{directory}/f{file}.txt")),
                format!("{directory}:{file}"),
            )
            .unwrap();
        }
        fs::write(path.join("deep.txt"), format!("deep {directory}")).unwrap();
    }
    let reader = CheckoutReader::open(&root.0, None).unwrap();
    let legacy = reader.capture_tracked(limits(), &BTreeSet::new()).unwrap();
    let mut chunks = 0;
    let (projection, manifests) = reader
        .capture_streaming(
            limits(),
            &BTreeSet::new(),
            &mut |_| {
                chunks += 1;
                Ok(())
            },
            &mut || Ok(()),
        )
        .unwrap();
    assert_eq!(
        projection,
        legacy
            .iter()
            .map(CheckoutProjectionEntry::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(projection.len(), 1 + 6 * 6);
    assert!(projection.windows(2).all(|pair| pair[0].path < pair[1].path));
    assert_eq!(manifests.len(), 37);
    assert!(chunks >= 37);
    // 1 file + 6 directories + 6 * (5 files + 1 directory + 1 file) entries.
    let mut bounds = limits();
    bounds.max_entries = 1 + 6 + 6 * 7 - 1;
    let error = reader
        .capture_streaming(bounds, &BTreeSet::new(), &mut |_| Ok(()), &mut || Ok(()))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    bounds.max_entries += 1;
    assert!(reader
        .capture_streaming(bounds, &BTreeSet::new(), &mut |_| Ok(()), &mut || Ok(()))
        .is_ok());
    // A file the trust hook vouches for is not opened; the others are.
    let vouched = |path: &str, listed: &fs::Metadata| {
        path.ends_with("f0.txt").then(|| TrustedFile {
            file_id: layrs_core::scm::FileId::from_bytes(b"vouched"),
            length: listed.len(),
            executable: false,
        })
    };
    let observed = reader
        .observe_streaming_trusting(limits(), &BTreeSet::new(), None, &vouched)
        .unwrap();
    assert_eq!(observed.trusted, 6);
    assert_eq!(observed.hashed.len(), 37 - 6);
    assert_eq!(observed.entries.len(), 37);
    // A subtree that cannot be represented fails the whole walk.
    fs::write(root.0.join("d3/cafe\u{301}.txt"), b"x").unwrap();
    assert!(reader
        .capture_streaming(limits(), &BTreeSet::new(), &mut |_| Ok(()), &mut || Ok(()))
        .is_err());
}
