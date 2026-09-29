use super::*;

#[test]
fn history_is_paginated_and_fsck_checks_old_snapshots_and_captured_bytes() {
    let fixture = Fixture::new();
    let (initial, target) = hydration();
    let store = ProjectionStore::create(&fixture.0, identity(), initial).unwrap();
    let installed = store.install_projection(0, &target, &mut provide).unwrap();
    for value in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
        fs::write(fixture.0.join("a.txt"), value).unwrap();
        store.capture_patch(installed.generation).unwrap();
    }
    let report = store.fsck().unwrap();
    assert_eq!(report.snapshots, 2);
    assert_eq!(report.captures, 3);
    assert_eq!(report.captured_files, 3);
    assert_eq!(report.captured_bytes, 11);
    let page = store.capture_history(None, 2).unwrap();
    assert_eq!(
        page.captures.iter().map(|p| p.sequence).collect::<Vec<_>>(),
        [3, 2]
    );
    assert_eq!(page.next_before, Some(2));
    let last = store.capture_history(page.next_before, 2).unwrap();
    assert_eq!(last.captures[0].sequence, 1);
    assert!(last.next_before.is_none());
    assert!(store.capture_history(None, 0).is_err());
    let access = format::access(&store, SessionMode::Exclusive).unwrap();
    access
        .connection
        .execute(
            "UPDATE projection_snapshots SET payload=x'7b7d' WHERE generation=0",
            [],
        )
        .unwrap();
    drop(access);
    // The active snapshot itself is intact; only a full history check detects it.
    assert!(store.state().is_ok());
    assert!(store.fsck().is_err());
}

#[test]
fn fsck_detects_unreferenced_corrupt_capture_blobs() {
    let fixture = Fixture::new();
    let store = ProjectionStore::create(&fixture.0, identity(), snapshot()).unwrap();
    let access = format::access(&store, SessionMode::Exclusive).unwrap();
    access
        .connection
        .execute(
            "INSERT INTO projection_capture_files VALUES(?1,?2)",
            rusqlite::params![
                FileId::from_bytes(b"expected").as_str(),
                b"different".as_slice()
            ],
        )
        .unwrap();
    drop(access);
    assert!(store.fsck().is_err());
}
