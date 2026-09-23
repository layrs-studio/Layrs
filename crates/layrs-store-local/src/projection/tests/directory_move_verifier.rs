use super::capture::ready;
use super::*;

fn observed_move() -> (Fixture, ProjectionStore, DirectoryMovePreview) {
    let (fixture, store) = ready(false);
    let state = store.state().unwrap();
    let observed = store
        .observe_absent_paths(
            state.generation,
            &state.digest,
            &[RepoPath::parse("moved/b.txt").unwrap()],
        )
        .unwrap();
    let preview = store
        .preview_directory_move(
            observed.generation,
            &RepoPath::parse("nested").unwrap(),
            &RepoPath::parse("moved").unwrap(),
        )
        .unwrap();
    (fixture, store, preview)
}

fn assert_pending_without_capture(fixture: &Fixture) {
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    let counts: (u64, u64, u64) = db.query_row(
        "SELECT (SELECT COUNT(*) FROM projection_namespace_pending), (SELECT COUNT(*) FROM projection_namespace_operations), (SELECT COUNT(*) FROM projection_captures)",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(counts, (1, 0, 0));
}

#[test]
fn late_destination_replacement_retains_journal_and_both_directories() {
    struct Replace(PathBuf);
    impl NamespaceFaults for Replace {
        fn check(&self, point: NamespacePoint) -> io::Result<()> {
            if point == NamespacePoint::BeforeCommit {
                fs::rename(self.0.join("moved"), self.0.join("retained-original"))?;
                fs::create_dir(self.0.join("moved"))?;
                fs::write(self.0.join("moved/b.txt"), b"beta")?;
            }
            Ok(())
        }
    }
    let (fixture, store, preview) = observed_move();
    let outcome = store.apply_directory_move_with_faults(&preview, &Replace(fixture.0.clone()));
    assert_eq!(
        fs::read(fixture.0.join("retained-original/b.txt")).unwrap(),
        b"beta"
    );
    assert_eq!(fs::read(fixture.0.join("moved/b.txt")).unwrap(), b"beta");
    assert!(
        outcome.is_err(),
        "replacement destination was committed: {outcome:?}"
    );
    assert_pending_without_capture(&fixture);
    assert!(ProjectionStore::resume_namespace(&fixture.0).is_err());
}

#[test]
fn late_source_reappearance_retains_journal_and_external_bytes() {
    struct Reappear(PathBuf);
    impl NamespaceFaults for Reappear {
        fn check(&self, point: NamespacePoint) -> io::Result<()> {
            if point == NamespacePoint::BeforeCommit {
                fs::create_dir(self.0.join("nested"))?;
                fs::write(self.0.join("nested/b.txt"), b"external source")?;
            }
            Ok(())
        }
    }
    let (fixture, store, preview) = observed_move();
    let outcome = store.apply_directory_move_with_faults(&preview, &Reappear(fixture.0.clone()));
    assert_eq!(fs::read(fixture.0.join("moved/b.txt")).unwrap(), b"beta");
    assert_eq!(
        fs::read(fixture.0.join("nested/b.txt")).unwrap(),
        b"external source"
    );
    assert!(
        outcome.is_err(),
        "reappeared source was committed: {outcome:?}"
    );
    assert_pending_without_capture(&fixture);
    assert!(ProjectionStore::resume_namespace(&fixture.0).is_err());
}

#[test]
fn late_destination_content_edit_retains_journal_and_external_bytes() {
    struct Edit(PathBuf);
    impl NamespaceFaults for Edit {
        fn check(&self, point: NamespacePoint) -> io::Result<()> {
            if point == NamespacePoint::BeforeCommit {
                fs::write(self.0.join("moved/b.txt"), b"external edit")?;
            }
            Ok(())
        }
    }
    let (fixture, store, preview) = observed_move();
    let outcome = store.apply_directory_move_with_faults(&preview, &Edit(fixture.0.clone()));
    assert_eq!(
        fs::read(fixture.0.join("moved/b.txt")).unwrap(),
        b"external edit"
    );
    assert!(
        outcome.is_err(),
        "changed destination was committed: {outcome:?}"
    );
    assert_pending_without_capture(&fixture);
    assert!(ProjectionStore::resume_namespace(&fixture.0).is_err());
}

#[test]
fn history_cannot_rebind_move_to_a_later_capture_with_unrelated_edits() {
    let (fixture, store, preview) = observed_move();
    let original = store.apply_directory_move(&preview).unwrap();
    fs::write(fixture.0.join("a.txt"), b"later unrelated edit").unwrap();
    let later = store.capture_patch(preview.projection_generation).unwrap();
    assert_ne!(later.sequence, original.sequence);
    assert_ne!(later.changes, original.changes);
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    assert_eq!(db.execute(
        "UPDATE projection_namespace_operations SET capture_sequence=?1 WHERE preview_digest=?2",
        rusqlite::params![later.sequence, preview.digest],
    ).unwrap(), 1);
    drop(db);
    let replay = store.apply_directory_move(&preview);
    let fsck = store.fsck();
    assert!(
        replay.is_err(),
        "replay accepted different capture: {replay:?}; fsck: {fsck:?}"
    );
    assert!(fsck.is_err());
}

#[test]
fn every_present_source_and_mapped_destination_needs_exact_write_observation() {
    for denied in ["source", "destination", "unobserved_destination"] {
        let fixture = Fixture::new();
        let (mut before, mut after) = hydration();
        if denied == "source" {
            for snapshot in [&mut before, &mut after] {
                snapshot
                    .entries
                    .iter_mut()
                    .find(|e| e.path.as_str() == "nested/b.txt")
                    .unwrap()
                    .writable = false;
            }
        }
        if denied != "unobserved_destination" {
            let absent = ProjectionEntry {
                path: RepoPath::parse("moved/b.txt").unwrap(),
                target: None,
                executable: false,
                writable: denied != "destination",
                state: EntryState::Absent,
            };
            before.entries.push(absent.clone());
            after.entries.push(absent);
            before.entries.sort_by(|a, b| a.path.cmp(&b.path));
            after.entries.sort_by(|a, b| a.path.cmp(&b.path));
        }
        let store = ProjectionStore::create(&fixture.0, identity(), before).unwrap();
        let state = store.install_projection(0, &after, &mut provide).unwrap();
        assert!(
            store
                .preview_directory_move(
                    state.generation,
                    &RepoPath::parse("nested").unwrap(),
                    &RepoPath::parse("moved").unwrap(),
                )
                .is_err(),
            "missing authority accepted: {denied}"
        );
        assert_eq!(fs::read(fixture.0.join("nested/b.txt")).unwrap(), b"beta");
        assert!(!fixture.0.join("moved").exists());
        assert!(store.latest_captured_patch().unwrap().is_none());
    }
}

#[test]
fn reserved_destination_descendants_and_windows_aliases_are_fenced() {
    let fixture = Fixture::new();
    let (mut before, mut after) = hydration();
    let reserved = ProjectionEntry {
        path: RepoPath::parse("moved/hidden.txt").unwrap(),
        target: Some(TreeEntryTarget::file(FileId::from_bytes(b"hidden"))),
        executable: false,
        writable: false,
        state: EntryState::Reserved,
    };
    let absent = ProjectionEntry {
        path: RepoPath::parse("moved/b.txt").unwrap(),
        target: None,
        executable: false,
        writable: true,
        state: EntryState::Absent,
    };
    for snapshot in [&mut before, &mut after] {
        snapshot.entries.extend([reserved.clone(), absent.clone()]);
        snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    }
    let store = ProjectionStore::create(&fixture.0, identity(), before).unwrap();
    let state = store.install_projection(0, &after, &mut provide).unwrap();
    for destination in ["moved", "MOVED", "moved.", "MOVED~1", "moved:stream"] {
        assert!(
            store
                .preview_directory_move(
                    state.generation,
                    &RepoPath::parse("nested").unwrap(),
                    &RepoPath::parse(destination).unwrap(),
                )
                .is_err(),
            "reserved/aliased destination accepted: {destination}"
        );
    }
    assert_eq!(fs::read(fixture.0.join("nested/b.txt")).unwrap(), b"beta");
    assert!(!fixture.0.join("moved").exists());
}

#[cfg(windows)]
#[test]
fn over_budget_source_path_is_rejected_before_durable_move() {
    let fixture = Fixture::new();
    let source = vec!["d".repeat(210); 20].join("/");
    let leaf = format!("{source}/b.txt");
    assert!(leaf.len() > 4096);
    let mut before = snapshot();
    before.entries.extend([
        ProjectionEntry {
            path: RepoPath::parse(&leaf).unwrap(),
            target: Some(TreeEntryTarget::file(FileId::from_bytes(b"beta"))),
            executable: false,
            writable: true,
            state: EntryState::Unavailable,
        },
        ProjectionEntry {
            path: RepoPath::parse("moved/b.txt").unwrap(),
            target: None,
            executable: false,
            writable: true,
            state: EntryState::Absent,
        },
    ]);
    before.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut after = before.clone();
    after
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == leaf)
        .unwrap()
        .state = EntryState::Materialized;
    let store = ProjectionStore::create(&fixture.0, identity(), before).unwrap();
    let state = store
        .install_projection(0, &after, &mut |_, out| out.write_all(b"beta"))
        .unwrap();
    let preview = store.preview_directory_move(
        state.generation,
        &RepoPath::parse(&source).unwrap(),
        &RepoPath::parse("moved").unwrap(),
    );
    if let Ok(preview) = &preview {
        let apply = store.apply_directory_move(preview);
        assert!(
            fixture.0.join(&source).exists(),
            "preview accepted an unfinalizable source path and moved it: {apply:?}"
        );
    }
    assert!(
        preview.is_err(),
        "source deletion exceeds capture path budget"
    );
    assert_eq!(fs::read(fixture.0.join(&leaf)).unwrap(), b"beta");
    assert!(!fixture.0.join("moved").exists());
}
