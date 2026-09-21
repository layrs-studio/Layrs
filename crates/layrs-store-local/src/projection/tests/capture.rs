use super::*;
fn ready(readonly: bool) -> (Fixture, ProjectionStore) {
    let fixture = Fixture::new();
    let (mut initial, mut target) = hydration();
    if readonly {
        initial.entries[0].writable = false;
        target.entries[0].writable = false;
    }
    let store = ProjectionStore::create(&fixture.0, identity(), initial).unwrap();
    store.install_projection(0, &target, &mut provide).unwrap();
    (fixture, store)
}
#[test]
fn captures_are_durable_base_relative_and_reopen_exact_bytes_without_live_files() {
    let (f, store) = ready(false);
    let baseline = store.state().unwrap();
    let clean = store.capture_patch(1).unwrap();
    assert!(clean.changes.is_empty());
    fs::write(f.0.join("a.txt"), b"first edit").unwrap();
    fs::remove_file(f.0.join("nested/b.txt")).unwrap();
    let first = store.capture_patch(1).unwrap();
    assert_eq!(first.sequence, 2);
    assert_eq!(first.changes.len(), 2);
    assert_eq!(
        first.changes[0].expected.as_ref().unwrap().file_id,
        FileId::from_bytes(b"alpha")
    );
    assert!(first.changes[1].replacement.is_none());
    assert_eq!(store.capture_patch(1).unwrap(), first);
    fs::write(f.0.join("a.txt"), b"second edit").unwrap();
    let second = store.capture_patch(1).unwrap();
    assert_eq!(second.sequence, 3);
    assert_eq!(second.previous_patch_digest, Some(first.digest.clone()));
    assert_eq!(second.changes[0].expected, first.changes[0].expected);
    let reopened = ProjectionStore::open(&f.0).unwrap();
    let mut bytes = Vec::new();
    reopened
        .write_captured_file(
            first.sequence,
            &RepoPath::parse("a.txt").unwrap(),
            &mut bytes,
        )
        .unwrap();
    assert_eq!(bytes, b"first edit");
    assert_eq!(reopened.captured_patch(first.sequence).unwrap(), first);
    assert_eq!(reopened.latest_captured_patch().unwrap(), Some(second));
    assert_eq!(reopened.state().unwrap(), baseline);
    assert!(crate::scm_v1::open(f.0.join(".layrs")).is_err());
}
#[test]
fn protected_unknown_alias_and_readonly_changes_never_produce_a_capture() {
    let (f, store) = ready(true);
    for path in ["private.txt", "PRIVATE.TXT", "private.txt.", "unknown.txt"] {
        // Windows normalizes a trailing period, still referring to the reserved leaf.
        fs::write(f.0.join(path), b"external bytes").unwrap();
        assert!(store.capture_patch(1).is_err(), "{path}");
        assert_eq!(fs::read(f.0.join(path)).unwrap(), b"external bytes");
        fs::remove_file(f.0.join(path)).unwrap();
    }
    fs::write(f.0.join("a.txt"), b"forbidden edit").unwrap();
    assert!(store.capture_patch(1).is_err());
    fs::remove_file(f.0.join("a.txt")).unwrap();
    assert!(store.capture_patch(1).is_err());
    assert!(store.latest_captured_patch().unwrap().is_none());
}
struct StopCapture(CapturePoint);
impl CaptureFaults for StopCapture {
    fn check(&self, p: CapturePoint) -> io::Result<()> {
        if p == self.0 {
            Err(io::Error::other("capture interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn capture_atomic_commit_and_retry_keep_history_and_byte_references_together() {
    let (f, store) = ready(false);
    fs::write(f.0.join("a.txt"), b"durable").unwrap();
    for point in [CapturePoint::FirstScan, CapturePoint::BeforeCommit] {
        assert!(
            store
                .capture_patch_with_faults(1, &StopCapture(point))
                .is_err()
        );
        assert!(store.latest_captured_patch().unwrap().is_none());
        let db =
            rusqlite::Connection::open(f.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM projection_capture_files", [], |r| r
                .get::<_, u64>(
                0
            ))
            .unwrap(),
            0
        );
    }
    assert!(
        store
            .capture_patch_with_faults(1, &StopCapture(CapturePoint::AfterCommit))
            .is_err()
    );
    let stored = store.latest_captured_patch().unwrap().unwrap();
    assert_eq!(stored.sequence, 1);
    assert_eq!(store.capture_patch(1).unwrap(), stored);
    assert!(store.capture_patch(0).is_err());
}
#[test]
fn concurrent_external_edit_and_corrupted_persistent_bytes_fail_closed() {
    struct Edit(PathBuf);
    impl CaptureFaults for Edit {
        fn check(&self, p: CapturePoint) -> io::Result<()> {
            if p == CapturePoint::FirstScan {
                fs::write(self.0.join("a.txt"), b"during capture")?;
            }
            Ok(())
        }
    }
    let (f, store) = ready(false);
    assert!(
        store
            .capture_patch_with_faults(1, &Edit(f.0.clone()))
            .is_err()
    );
    assert!(store.latest_captured_patch().unwrap().is_none());
    let capture = store.capture_patch(1).unwrap();
    let db = rusqlite::Connection::open(f.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute(
        "UPDATE projection_capture_files SET bytes=?1",
        [b"corrupt".as_slice()],
    )
    .unwrap();
    drop(db);
    assert!(store.captured_patch(capture.sequence).is_err());
    assert!(store.capture_patch(1).is_err());
    let mut out = Vec::new();
    assert!(
        store
            .write_captured_file(
                capture.sequence,
                &RepoPath::parse("a.txt").unwrap(),
                &mut out
            )
            .is_err()
    );
    assert!(out.is_empty());
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"during capture");
}
#[test]
fn changed_file_budget_rejects_without_partial_capture_or_file_mutation() {
    let (f, store) = ready(false);
    let file = fs::OpenOptions::new()
        .write(true)
        .open(f.0.join("a.txt"))
        .unwrap();
    file.set_len((MAX_PATCH_BYTES + 1) as u64).unwrap();
    drop(file);
    assert!(store.capture_patch(1).is_err());
    assert!(store.latest_captured_patch().unwrap().is_none());
    assert_eq!(
        fs::metadata(f.0.join("a.txt")).unwrap().len(),
        (MAX_PATCH_BYTES + 1) as u64
    );
}
#[cfg(unix)]
#[test]
fn capture_refuses_symlink_parent_and_leaf_without_reading_external_content() {
    let (f, store) = ready(false);
    let external = f.0.with_extension("external");
    fs::write(&external, b"outside").unwrap();
    fs::remove_file(f.0.join("a.txt")).unwrap();
    std::os::unix::fs::symlink(&external, f.0.join("a.txt")).unwrap();
    assert!(store.capture_patch(1).is_err());
    assert_eq!(fs::read(&external).unwrap(), b"outside");
    fs::remove_file(&external).unwrap();
}
