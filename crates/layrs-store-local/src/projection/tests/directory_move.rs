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
struct Stop(NamespacePoint);
impl NamespaceFaults for Stop {
    fn check(&self, point: NamespacePoint) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other("directory move interrupted"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn move_replays_exact_capture_and_checks_history() {
    let (fixture, store, preview) = observed_move();
    assert_eq!(preview.source_files.len(), 1);
    assert_eq!(
        preview.source_files["nested/b.txt"].file_id,
        FileId::from_bytes(b"beta")
    );
    let capture = store.apply_directory_move(&preview).unwrap();
    assert!(!fixture.0.join("nested").exists());
    assert_eq!(fs::read(fixture.0.join("moved/b.txt")).unwrap(), b"beta");
    assert_eq!(capture.changes.len(), 2);
    assert_eq!(capture.changes[0].path.as_str(), "moved/b.txt");
    assert_eq!(capture.changes[1].path.as_str(), "nested/b.txt");
    assert!(capture.changes[1].replacement.is_none());
    let reopened = ProjectionStore::open(&fixture.0).unwrap();
    assert_eq!(reopened.apply_directory_move(&preview).unwrap(), capture);
    assert_eq!(reopened.fsck().unwrap().namespace_operations, 1);
}

#[test]
fn move_recovers_every_durable_boundary() {
    for point in [
        NamespacePoint::JournalDurable,
        NamespacePoint::PathApplied,
        NamespacePoint::BeforeCommit,
        NamespacePoint::AfterCommit,
    ] {
        let (fixture, store, preview) = observed_move();
        assert!(
            store
                .apply_directory_move_with_faults(&preview, &Stop(point))
                .is_err()
        );
        let capture = if point == NamespacePoint::AfterCommit {
            store.apply_directory_move(&preview).unwrap()
        } else {
            assert!(ProjectionStore::open(&fixture.0).is_err());
            ProjectionStore::resume_namespace(&fixture.0).unwrap()
        };
        assert!(!fixture.0.join("nested").exists());
        assert_eq!(fs::read(fixture.0.join("moved/b.txt")).unwrap(), b"beta");
        let reopened = ProjectionStore::open(&fixture.0).unwrap();
        assert_eq!(reopened.apply_directory_move(&preview).unwrap(), capture);
        assert_eq!(reopened.fsck().unwrap().namespace_operations, 1);
    }
}

#[test]
fn locally_created_source_leaf_has_no_remote_deletion() {
    let (fixture, store) = ready(false);
    let state = store.state().unwrap();
    let observed = store
        .observe_absent_paths(
            state.generation,
            &state.digest,
            &[
                RepoPath::parse("nested/local.txt").unwrap(),
                RepoPath::parse("moved/b.txt").unwrap(),
                RepoPath::parse("moved/local.txt").unwrap(),
            ],
        )
        .unwrap();
    fs::write(fixture.0.join("nested/local.txt"), b"local").unwrap();
    let preview = store
        .preview_directory_move(
            observed.generation,
            &RepoPath::parse("nested").unwrap(),
            &RepoPath::parse("moved").unwrap(),
        )
        .unwrap();
    assert_eq!(preview.source_files.len(), 2);
    let capture = store.apply_directory_move(&preview).unwrap();
    assert_eq!(
        fs::read(fixture.0.join("moved/local.txt")).unwrap(),
        b"local"
    );
    assert!(
        capture
            .changes
            .iter()
            .all(|c| c.path.as_str() != "nested/local.txt")
    );
    assert!(
        capture
            .changes
            .iter()
            .any(|c| c.path.as_str() == "nested/b.txt" && c.replacement.is_none())
    );
    assert_eq!(store.fsck().unwrap().namespace_operations, 1);
}

#[test]
fn move_refuses_collisions_aliases_and_reserved_source_subtree() {
    let (fixture, store, preview) = observed_move();
    fs::create_dir(fixture.0.join("moved")).unwrap();
    fs::write(fixture.0.join("moved/foreign.txt"), b"foreign").unwrap();
    assert!(store.apply_directory_move(&preview).is_err());
    assert_eq!(fs::read(fixture.0.join("nested/b.txt")).unwrap(), b"beta");
    assert_eq!(
        fs::read(fixture.0.join("moved/foreign.txt")).unwrap(),
        b"foreign"
    );
    assert!(
        store
            .preview_directory_move(
                preview.projection_generation,
                &RepoPath::parse("nested").unwrap(),
                &RepoPath::parse("NESTED").unwrap()
            )
            .is_err()
    );
    let fixture = Fixture::new();
    let (mut before, mut after) = hydration();
    let reserved = ProjectionEntry {
        path: RepoPath::parse("nested/hidden.txt").unwrap(),
        target: Some(TreeEntryTarget::file(FileId::from_bytes(b"hidden"))),
        executable: false,
        writable: false,
        state: EntryState::Reserved,
    };
    before.entries.push(reserved.clone());
    after.entries.push(reserved);
    before.entries.sort_by(|a, b| a.path.cmp(&b.path));
    after.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let store = ProjectionStore::create(&fixture.0, identity(), before).unwrap();
    let state = store.install_projection(0, &after, &mut provide).unwrap();
    let observed = store
        .observe_absent_paths(
            state.generation,
            &state.digest,
            &[RepoPath::parse("moved/b.txt").unwrap()],
        )
        .unwrap();
    assert!(
        store
            .preview_directory_move(
                observed.generation,
                &RepoPath::parse("nested").unwrap(),
                &RepoPath::parse("moved").unwrap()
            )
            .is_err()
    );
    assert_eq!(fs::read(fixture.0.join("nested/b.txt")).unwrap(), b"beta");
}

#[test]
fn recovery_retains_journal_on_source_replacement_and_changed_destination() {
    let (fixture, store, preview) = observed_move();
    assert!(
        store
            .apply_directory_move_with_faults(&preview, &Stop(NamespacePoint::JournalDurable))
            .is_err()
    );
    fs::rename(fixture.0.join("nested"), fixture.0.join("original")).unwrap();
    fs::create_dir(fixture.0.join("nested")).unwrap();
    fs::write(fixture.0.join("nested/b.txt"), b"beta").unwrap();
    assert!(ProjectionStore::resume_namespace(&fixture.0).is_err());
    assert_eq!(fs::read(fixture.0.join("original/b.txt")).unwrap(), b"beta");
    assert_eq!(fs::read(fixture.0.join("nested/b.txt")).unwrap(), b"beta");
    assert!(!fixture.0.join("moved").exists());
    let (fixture, store, preview) = observed_move();
    assert!(
        store
            .apply_directory_move_with_faults(&preview, &Stop(NamespacePoint::JournalDurable))
            .is_err()
    );
    fs::write(fixture.0.join("nested/b.txt"), b"changed").unwrap();
    assert!(ProjectionStore::resume_namespace(&fixture.0).is_err());
    assert!(!fixture.0.join("moved").exists());
    let (fixture, store, preview) = observed_move();
    assert!(
        store
            .apply_directory_move_with_faults(&preview, &Stop(NamespacePoint::PathApplied))
            .is_err()
    );
    fs::write(fixture.0.join("moved/b.txt"), b"changed").unwrap();
    assert!(ProjectionStore::resume_namespace(&fixture.0).is_err());
    assert_eq!(fs::read(fixture.0.join("moved/b.txt")).unwrap(), b"changed");
    assert!(ProjectionStore::open(&fixture.0).is_err());
    let (fixture, store, preview) = observed_move();
    assert!(
        store
            .apply_directory_move_with_faults(&preview, &Stop(NamespacePoint::PathApplied))
            .is_err()
    );
    fs::rename(fixture.0.join("moved"), fixture.0.join("original-moved")).unwrap();
    fs::create_dir(fixture.0.join("moved")).unwrap();
    fs::write(fixture.0.join("moved/b.txt"), b"beta").unwrap();
    assert!(ProjectionStore::resume_namespace(&fixture.0).is_err());
    assert_eq!(
        fs::read(fixture.0.join("original-moved/b.txt")).unwrap(),
        b"beta"
    );
    assert_eq!(fs::read(fixture.0.join("moved/b.txt")).unwrap(), b"beta");
}
