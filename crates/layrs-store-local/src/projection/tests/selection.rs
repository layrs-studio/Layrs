use super::*;
use layrs_core::projection::ProjectionSelection;

fn selected(paths: &[&str], subtrees: &[&str]) -> ProjectionSelection {
    ProjectionSelection::selected(
        paths.iter().map(|p| RepoPath::parse(*p).unwrap()).collect(),
        subtrees
            .iter()
            .map(|p| RepoPath::parse(*p).unwrap())
            .collect(),
        PathSemantics::PortableWindows,
    )
    .unwrap()
}
fn ready() -> (Fixture, ProjectionStore) {
    let f = Fixture::new();
    let (before, mut target) = hydration();
    target
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "nested/b.txt")
        .unwrap()
        .state = EntryState::Unavailable;
    let store =
        ProjectionStore::create_selected(&f.0, identity(), before, selected(&["a.txt"], &[]))
            .unwrap();
    store
        .install_projection(0, &target, &mut |path, out| {
            assert_eq!(path, "a.txt");
            provide(path, out)
        })
        .unwrap();
    (f, store)
}

#[test]
fn exclusion_is_distinct_from_unknown_absence_and_rights() {
    let (f, store) = ready();
    let state = store.state().unwrap();
    let outside = &state.snapshot.entries[1];
    assert_eq!(outside.state, EntryState::Unavailable);
    assert!(outside.target.is_some() && outside.writable);
    assert!(!store.selection().unwrap().includes(&outside.path));
    assert!(!f.0.join("nested/b.txt").exists());
    let unknown = RepoPath::parse("unknown.txt").unwrap();
    assert!(
        store
            .observe_absent_paths(state.generation, &state.digest, &[unknown])
            .is_err()
    );
    let mut fake = state.snapshot.clone();
    fake.entries[1].state = EntryState::Materialized;
    assert!(
        store
            .install_projection(state.generation, &fake, &mut |_, _| panic!("excluded read"))
            .is_err()
    );
    assert!(
        store
            .refresh_projection(state.generation, &fake, &mut |_, _| panic!("excluded read"))
            .is_err()
    );
    // Even All cannot hydrate a denied path.
    fake.entries[1].state = EntryState::Unavailable;
    fake.entries[2].state = EntryState::Materialized;
    assert!(
        store
            .expand_selection(
                state.generation,
                &ProjectionSelection::All,
                &fake,
                &mut |_, _| panic!("denied read")
            )
            .is_err()
    );
    assert_eq!(store.state().unwrap(), state);
}

#[test]
fn capture_and_publication_preserve_excluded_files_targets_and_selection() {
    let (f, store) = ready();
    fs::create_dir(f.0.join("nested")).unwrap();
    // A directory at an excluded file path proves capture never opens that path
    // as content. Its absence later must not become a deletion either.
    fs::create_dir(f.0.join("nested/b.txt")).unwrap();
    fs::write(f.0.join("nested/b.txt/unowned"), b"private local bytes").unwrap();
    fs::write(f.0.join("a.txt"), b"edit").unwrap();
    let before = store.state().unwrap();
    let capture = store.capture_patch(before.generation).unwrap();
    assert_eq!(capture.changes.len(), 1);
    assert_eq!(capture.changes[0].path.as_str(), "a.txt");
    let request = br#"{"request":"selection-test"}"#;
    let receipt = br#"{"result":"applied"}"#;
    let pending = store
        .begin_publication(capture.sequence, before.generation, request)
        .unwrap();
    assert!(
        store
            .expand_selection(
                before.generation,
                &ProjectionSelection::All,
                &before.snapshot,
                &mut provide
            )
            .is_err()
    );
    assert_eq!(store.pending_publication().unwrap(), Some(pending));
    let mut next = before.snapshot.clone();
    next.base_step_id = StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap();
    next.base_root_tree_id = TreeId::parse(format!("blake3:{}", "3".repeat(64))).unwrap();
    next.entries[0].target = Some(TreeEntryTarget::file(FileId::from_bytes(b"edit")));
    let after = store
        .finish_publication(capture.sequence, receipt, Some(&next))
        .unwrap();
    assert_eq!(after.snapshot.entries[1..], before.snapshot.entries[1..]);
    assert_eq!(
        fs::read(f.0.join("nested/b.txt/unowned")).unwrap(),
        b"private local bytes"
    );
    assert_eq!(store.captured_patch(capture.sequence).unwrap(), capture);
    assert_eq!(store.selection().unwrap(), selected(&["a.txt"], &[]));
    assert!(
        store
            .capture_patch(after.generation)
            .unwrap()
            .changes
            .is_empty()
    );
    store.fsck().unwrap();
}

#[test]
fn expansion_preserves_dirty_files_and_captures_and_rejects_collisions_stale_shrink() {
    let (f, store) = ready();
    fs::write(f.0.join("a.txt"), b"dirty accepted edit").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let before = store.state().unwrap();
    let mut target = before.snapshot.clone();
    target.entries[1].state = EntryState::Materialized;
    let expanded = selected(&["a.txt"], &["nested"]);
    fs::create_dir(f.0.join("nested")).unwrap();
    fs::write(f.0.join("nested/b.txt"), b"unowned collision").unwrap();
    assert!(
        store
            .expand_selection(1, &expanded, &target, &mut |_, _| panic!(
                "read before collision check"
            ))
            .is_err()
    );
    assert_eq!(store.selection().unwrap(), selected(&["a.txt"], &[]));
    assert_eq!(
        fs::read(f.0.join("nested/b.txt")).unwrap(),
        b"unowned collision"
    );
    fs::remove_file(f.0.join("nested/b.txt")).unwrap();
    assert!(
        store
            .expand_selection(0, &expanded, &target, &mut |_, _| panic!("stale read"))
            .is_err()
    );
    let after = store
        .expand_selection(1, &expanded, &target, &mut |path, out| {
            assert_eq!(path, "nested/b.txt");
            provide(path, out)
        })
        .unwrap();
    assert_eq!(after.generation, 2);
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"dirty accepted edit");
    assert_eq!(store.captured_patch(capture.sequence).unwrap(), capture);
    let mut bytes = Vec::new();
    store
        .write_captured_file(
            capture.sequence,
            &RepoPath::parse("a.txt").unwrap(),
            &mut bytes,
        )
        .unwrap();
    assert_eq!(bytes, b"dirty accepted edit");
    assert!(
        store
            .expand_selection(2, &selected(&["a.txt"], &[]), &target, &mut provide)
            .is_err()
    );
    assert_eq!(
        ProjectionStore::open(&f.0).unwrap().selection().unwrap(),
        expanded
    );
    let recaptured = store.capture_patch(2).unwrap();
    assert_eq!(recaptured.changes, capture.changes);
    store.fsck().unwrap();
}

#[test]
fn expansion_intent_and_files_recover_together_at_every_durable_boundary() {
    for point in [
        InstallPoint::JournalDurable,
        InstallPoint::PathInstalled,
        InstallPoint::BeforeCommit,
        InstallPoint::AfterCommit,
    ] {
        let (f, store) = ready();
        let (_, target) = hydration();
        let all = ProjectionSelection::All;
        assert!(
            store
                .expand_selection_with_faults(1, &all, &target, &mut provide, &StopInstall(point))
                .is_err()
        );
        if point != InstallPoint::AfterCommit {
            ProjectionStore::resume_installation(&f.0).unwrap();
        }
        let reopened = ProjectionStore::open(&f.0).unwrap();
        assert_eq!(reopened.state().unwrap().generation, 2);
        assert_eq!(reopened.selection().unwrap(), all);
        assert_eq!(fs::read(f.0.join("nested/b.txt")).unwrap(), b"beta");
        reopened.fsck().unwrap();
    }
}

#[test]
fn selective_refresh_never_rehydrates_or_erases_unselected_local_content() {
    let (f, store) = ready();
    fs::create_dir(f.0.join("nested")).unwrap();
    fs::write(f.0.join("nested/b.txt"), b"unowned").unwrap();
    let mut next = store.state().unwrap().snapshot;
    next.base_step_id = StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap();
    next.base_root_tree_id = TreeId::parse(format!("blake3:{}", "3".repeat(64))).unwrap();
    next.entries[1].target = Some(TreeEntryTarget::file(FileId::from_bytes(b"remote update")));
    let after = store
        .refresh_projection(1, &next, &mut |_, _| panic!("excluded transfer"))
        .unwrap();
    assert_eq!(after.snapshot, next);
    assert_eq!(fs::read(f.0.join("nested/b.txt")).unwrap(), b"unowned");
    assert!(
        store
            .capture_patch(after.generation)
            .unwrap()
            .changes
            .is_empty()
    );
    store.fsck().unwrap();
}

#[test]
fn selective_rebase_preserves_excluded_entries_and_never_requires_their_bytes() {
    let (f, store) = ready();
    fs::write(f.0.join("a.txt"), b"local merge").unwrap();
    let captured = store.capture_patch(1).unwrap();
    let mut target = store.state().unwrap().snapshot;
    target.base_step_id = StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap();
    target.base_root_tree_id = TreeId::parse(format!("blake3:{}", "3".repeat(64))).unwrap();
    target.entries[1].target = Some(TreeEntryTarget::file(FileId::from_bytes(
        b"remote excluded",
    )));
    let resolutions = std::collections::BTreeMap::from([(
        RepoPath::parse("a.txt").unwrap(),
        captured.changes[0].replacement.clone(),
    )]);
    let preview = store
        .preview_rebase(1, captured.sequence, &target, &resolutions)
        .unwrap();
    let rebased = store
        .apply_rebase(&preview, &mut |path, out| {
            assert_eq!(path, "a.txt");
            out.write_all(b"local merge")
        })
        .unwrap();
    assert_eq!(rebased.changes, captured.changes);
    assert_eq!(
        store.state().unwrap().snapshot.entries[1],
        target.entries[1]
    );
    assert!(!f.0.join("nested/b.txt").exists());
    assert_eq!(store.captured_patch(captured.sequence).unwrap(), captured);
    store.fsck().unwrap();
}

#[test]
fn schema_five_migration_preserves_pending_request_captures_and_snapshot_bytes() {
    let (f, store) = super::capture::ready(false);
    fs::write(f.0.join("a.txt"), b"pending durable").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let pending = store
        .begin_publication(capture.sequence, 1, br#"{"request":"keep-exact"}"#)
        .unwrap();
    let state = store.state().unwrap();
    drop(store);
    let path = f.0.join(".layrs/metadata/projection.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    let snapshot_bytes: Vec<u8> = db
        .query_row(
            "SELECT payload FROM projection_snapshots WHERE generation=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    db.execute_batch("DROP TABLE projection_selections; PRAGMA user_version=5;")
        .unwrap();
    drop(db);
    let reopened = ProjectionStore::open(&f.0).unwrap();
    assert_eq!(reopened.state().unwrap(), state);
    assert_eq!(reopened.selection().unwrap(), ProjectionSelection::All);
    assert_eq!(reopened.pending_publication().unwrap(), Some(pending));
    assert_eq!(reopened.captured_patch(capture.sequence).unwrap(), capture);
    let mut db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        6
    );
    assert!(database::migrate_legacy(&mut db).is_err());
    assert_eq!(
        db.query_row(
            "SELECT payload FROM projection_snapshots WHERE generation=1",
            [],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        snapshot_bytes
    );
    // Exact old schema fingerprint differs; every old open/mutation rejects6.
    let old = rusqlite::Connection::open_in_memory().unwrap();
    old.execute_batch(database::SQL).unwrap();
    assert_eq!(
        old.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
    reopened.fsck().unwrap();
}

#[test]
fn metadata_only_expansion_is_generation_bound_and_does_not_fabricate_absence() {
    let (_f, store) = ready();
    let before = store.state().unwrap();
    let selected = selected(&["a.txt", "future.txt"], &[]);
    let after = store
        .expand_selection(1, &selected, &before.snapshot, &mut |_, _| {
            panic!("no hydration expected")
        })
        .unwrap();
    assert_eq!(after.generation, 2);
    assert_eq!(after.snapshot, before.snapshot);
    assert!(
        !after
            .snapshot
            .entries
            .iter()
            .any(|e| e.path.as_str() == "future.txt")
    );
    assert_eq!(store.selection().unwrap(), selected);
    let other = ProjectionSelection::All;
    assert!(
        store
            .expand_selection(1, &other, &before.snapshot, &mut |_, _| panic!(
                "stale selection-only request"
            ))
            .is_err()
    );
}

#[test]
fn failed_selection_migration_rolls_back_schema_and_retries() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(database::SQL).unwrap();
    let pages: u64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    db.pragma_update(None, "max_page_count", pages).unwrap();
    assert!(database::migrate(&mut db).is_err());
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='projection_selections'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    db.pragma_update(None, "max_page_count", pages + 100)
        .unwrap();
    database::migrate(&mut db).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        6
    );
}

#[test]
fn selected_bootstrap_recovery_requires_exact_selection_and_never_adopts_all() {
    for point in [CreationPoint::MarkerDurable, CreationPoint::DatabaseDurable] {
        let f = Fixture::new();
        let (initial, _) = hydration();
        let selection = selected(&["a.txt"], &[]);
        assert!(
            ProjectionStore::create_selected_with_faults(
                &f.0,
                identity(),
                initial.clone(),
                selection.clone(),
                &Stop(point)
            )
            .is_err()
        );
        assert!(ProjectionStore::resume_create(&f.0, &identity(), &initial).is_err());
        let store =
            ProjectionStore::resume_create_selected(&f.0, &identity(), &initial, &selection)
                .unwrap();
        assert_eq!(store.selection().unwrap(), selection);
        assert_eq!(store.state().unwrap().generation, 0);
    }
}

#[test]
fn rebase_accepts_remote_additions_and_removals_outside_selection_without_local_changes() {
    let (f, store) = ready();
    fs::write(f.0.join("a.txt"), b"local merge").unwrap();
    fs::create_dir(f.0.join("nested")).unwrap();
    fs::write(f.0.join("nested/b.txt"), b"unowned excluded").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let mut target = store.state().unwrap().snapshot;
    target.base_step_id = StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap();
    target.base_root_tree_id = TreeId::parse(format!("blake3:{}", "3".repeat(64))).unwrap();
    let mut added = target.entries.remove(1);
    added.path = RepoPath::parse("outside/new.txt").unwrap();
    target.entries.push(added);
    target.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let resolutions = std::collections::BTreeMap::from([(
        RepoPath::parse("a.txt").unwrap(),
        capture.changes[0].replacement.clone(),
    )]);
    let preview = store
        .preview_rebase(1, capture.sequence, &target, &resolutions)
        .unwrap();
    store
        .apply_rebase(&preview, &mut |path, out| {
            assert_eq!(path, "a.txt");
            out.write_all(b"local merge")
        })
        .unwrap();
    assert_eq!(
        fs::read(f.0.join("nested/b.txt")).unwrap(),
        b"unowned excluded"
    );
    assert!(!f.0.join("outside/new.txt").exists());
    store.fsck().unwrap();
}

#[test]
fn directory_move_cannot_relocate_excluded_unknown_local_files() {
    let f = Fixture::new();
    let (mut before, _) = hydration();
    before.entries.retain(|e| e.path.as_str() != "nested/b.txt");
    before.entries[0].path = RepoPath::parse("src/a.txt").unwrap();
    before.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let selection = selected(&["src/a.txt", "dst/a.txt"], &[]);
    let store =
        ProjectionStore::create_selected(&f.0, identity(), before.clone(), selection).unwrap();
    before
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "src/a.txt")
        .unwrap()
        .state = EntryState::Materialized;
    store
        .install_projection(0, &before, &mut |_, out| out.write_all(b"alpha"))
        .unwrap();
    let state = store.state().unwrap();
    let state = store
        .observe_absent_paths(1, &state.digest, &[RepoPath::parse("dst/a.txt").unwrap()])
        .unwrap();
    fs::write(f.0.join("src/unselected.txt"), b"never relocate").unwrap();
    assert!(
        store
            .preview_directory_move(
                state.generation,
                &RepoPath::parse("src").unwrap(),
                &RepoPath::parse("dst").unwrap()
            )
            .is_err()
    );
    assert_eq!(
        fs::read(f.0.join("src/unselected.txt")).unwrap(),
        b"never relocate"
    );
    assert!(!f.0.join("dst").exists());
}

#[test]
fn expanding_selection_refuses_platform_alias_in_existing_parent() {
    let (f, store) = ready();
    fs::create_dir(f.0.join("NESTED")).unwrap();
    let before = store.state().unwrap();
    let (_, target) = hydration();
    assert!(
        store
            .expand_selection(
                before.generation,
                &ProjectionSelection::All,
                &target,
                &mut |_, _| panic!("alias must be rejected before bytes")
            )
            .is_err()
    );
    assert_eq!(store.state().unwrap(), before);
    assert_eq!(store.selection().unwrap(), selected(&["a.txt"], &[]));
    assert!(f.0.join("NESTED").is_dir());
    assert!(!f.0.join("NESTED/b.txt").exists());
    assert!(
        store
            .expand_selection(
                before.generation,
                &selected(&["A.txt"], &[]),
                &before.snapshot,
                &mut provide
            )
            .is_err()
    );
}
