use super::*;

struct Stop(RefreshPoint);
impl RefreshFaults for Stop {
    fn check(&self, point: RefreshPoint) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other("refresh interrupted"))
        } else {
            Ok(())
        }
    }
}
fn ready() -> (Fixture, ProjectionStore, ProjectionSnapshot) {
    let f = Fixture::new();
    let (before, after) = hydration();
    let store = ProjectionStore::create(&f.0, identity(), before).unwrap();
    store.install_projection(0, &after, &mut provide).unwrap();
    (f, store, after)
}
fn updated(before: &ProjectionSnapshot) -> ProjectionSnapshot {
    let mut after = before.clone();
    let id = format!("blake3:{}", "2".repeat(64));
    after.base_step_id = StepId::parse(&id).unwrap();
    after.base_root_tree_id = TreeId::parse(&id).unwrap();
    after
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "a.txt")
        .unwrap()
        .target = Some(TreeEntryTarget::file(FileId::from_bytes(b"received")));
    after
}
fn received(path: &str, out: &mut dyn io::Write) -> io::Result<()> {
    assert_eq!(path, "a.txt");
    out.write_all(b"received")
}
#[test]
fn refresh_recovers_every_durable_boundary_and_retains_captures() {
    for point in [
        RefreshPoint::IntentDurable,
        RefreshPoint::Staged,
        RefreshPoint::JournalDurable,
        RefreshPoint::PathQuarantined,
        RefreshPoint::PathInstalled,
        RefreshPoint::BeforeCommit,
        RefreshPoint::AfterCommit,
    ] {
        let (f, store, before) = ready();
        let capture = store.capture_patch(1).unwrap();
        let after = updated(&before);
        assert!(
            store
                .refresh_projection_with_faults(1, &after, &mut received, &Stop(point))
                .is_err()
        );
        let resumed = ProjectionStore::resume_refresh(&f.0).unwrap();
        if matches!(point, RefreshPoint::IntentDurable | RefreshPoint::Staged) {
            assert_eq!(resumed.generation, 1);
            assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
            store.refresh_projection(1, &after, &mut received).unwrap();
        } else {
            assert_eq!(resumed.generation, 2);
        }
        assert_eq!(store.state().unwrap().snapshot, after);
        assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"received");
        assert_eq!(fs::read(f.0.join("nested/b.txt")).unwrap(), b"beta");
        assert_eq!(
            store.captured_patch(capture.sequence).unwrap().digest,
            capture.digest
        );
        assert_eq!(ProjectionStore::resume_refresh(&f.0).unwrap().generation, 2);
    }
}
#[test]
fn refresh_revocation_quarantines_then_regrant_hydrates() {
    let (f, store, before) = ready();
    let mut revoked = before.clone();
    revoked.observed_authorization_epoch += 1;
    let e = revoked
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "a.txt")
        .unwrap();
    e.state = EntryState::Reserved;
    e.writable = false;
    store
        .refresh_projection(1, &revoked, &mut |_, _| panic!("revocation must not fetch"))
        .unwrap();
    assert!(!f.0.join("a.txt").exists());
    assert!(
        store
            .state()
            .unwrap()
            .snapshot
            .reserved_paths()
            .unwrap()
            .check_path("A.TXT")
            .is_err()
    );
    let directory = fs::read_dir(f.0.join(".layrs/projection-quarantine"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(directory.join("old-00000000")).unwrap(), b"alpha");
    let mut granted = before;
    granted.observed_authorization_epoch += 2;
    store.refresh_projection(2, &granted, &mut provide).unwrap();
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(directory.join("old-00000000")).unwrap(), b"alpha");
}
#[test]
fn refresh_refuses_dirty_missing_and_unowned_destination_before_fetch() {
    for missing in [false, true] {
        let (f, store, before) = ready();
        if missing {
            fs::remove_file(f.0.join("nested/b.txt")).unwrap();
        } else {
            fs::write(f.0.join("nested/b.txt"), b"local edits").unwrap();
        }
        assert!(
            store
                .refresh_projection(1, &updated(&before), &mut |_, _| panic!("dirty preflight"))
                .is_err()
        );
        assert_eq!(store.state().unwrap().generation, 1);
        assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
    }
    let (f, store, before) = ready();
    let mut after = updated(&before);
    after.entries.push(ProjectionEntry {
        path: RepoPath::parse("z/new.txt").unwrap(),
        target: Some(TreeEntryTarget::file(FileId::from_bytes(b"new"))),
        executable: false,
        writable: true,
        state: EntryState::Materialized,
    });
    fs::create_dir(f.0.join("z")).unwrap();
    fs::write(f.0.join("z/new.txt"), b"external").unwrap();
    assert!(
        store
            .refresh_projection(1, &after, &mut |_, _| panic!("collision preflight"))
            .is_err()
    );
    assert_eq!(fs::read(f.0.join("z/new.txt")).unwrap(), b"external");
}
#[test]
fn refresh_recovery_preserves_post_quarantine_collision_and_replaced_inode() {
    for replaced in [false, true] {
        let (f, store, before) = ready();
        let point = if replaced {
            RefreshPoint::PathInstalled
        } else {
            RefreshPoint::PathQuarantined
        };
        assert!(
            store
                .refresh_projection_with_faults(1, &updated(&before), &mut received, &Stop(point))
                .is_err()
        );
        if replaced {
            fs::rename(f.0.join("a.txt"), f.0.join("saved.txt")).unwrap();
        }
        fs::write(
            f.0.join("a.txt"),
            if replaced {
                b"received".as_slice()
            } else {
                b"external".as_slice()
            },
        )
        .unwrap();
        assert!(ProjectionStore::resume_refresh(&f.0).is_err());
        assert!(store.state().is_err());
        assert_eq!(
            fs::read(f.0.join("a.txt")).unwrap(),
            if replaced {
                b"received".as_slice()
            } else {
                b"external".as_slice()
            }
        );
    }
}
#[test]
fn refresh_corrupt_spool_never_changes_checkout_and_retries_after_resume() {
    let (f, store, before) = ready();
    let after = updated(&before);
    assert!(
        store
            .refresh_projection(1, &after, &mut |_, out| out.write_all(b"corrupt"))
            .is_err()
    );
    assert!(store.state().is_err());
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
    ProjectionStore::resume_refresh(&f.0).unwrap();
    store.refresh_projection(1, &after, &mut received).unwrap();
}
#[test]
fn refresh_leaf_to_nested_directory_preserves_unknown_files() {
    let (f, store, before) = ready();
    let mut after = updated(&before);
    let entry = after
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "a.txt")
        .unwrap();
    entry.path = RepoPath::parse("a.txt/deep/file.txt").unwrap();
    after.entries.sort_by(|a, b| a.path.cmp(&b.path));
    fs::write(f.0.join("nested/untracked.txt"), b"untracked").unwrap();
    store
        .refresh_projection(1, &after, &mut |path, out| {
            assert_eq!(path, "a.txt/deep/file.txt");
            out.write_all(b"received")
        })
        .unwrap();
    assert_eq!(
        fs::read(f.0.join("a.txt/deep/file.txt")).unwrap(),
        b"received"
    );
    assert_eq!(
        fs::read(f.0.join("nested/untracked.txt")).unwrap(),
        b"untracked"
    );
}
#[test]
fn refresh_rejects_stale_generation_and_authority_regression() {
    let (_f, store, before) = ready();
    let mut after = updated(&before);
    assert!(matches!(
        store.refresh_projection(0, &after, &mut received),
        Err(ProjectionStoreError::Stale { .. })
    ));
    after.observed_authorization_epoch -= 1;
    assert!(store.refresh_projection(1, &after, &mut received).is_err());
    assert_eq!(store.state().unwrap().snapshot, before);
}

#[test]
fn refresh_absent_grants_are_bound_to_the_exact_remote_base() {
    let (_f, store, before) = ready();
    let initial = store.state().unwrap();
    let observed = store
        .observe_absent_paths(
            initial.generation,
            &initial.digest,
            &[RepoPath::parse("new.txt").unwrap()],
        )
        .unwrap();
    assert_eq!(observed.generation, 2);
    let unchanged = store
        .refresh_projection(2, &before, &mut |_, _| panic!("unchanged files"))
        .unwrap();
    assert_eq!(unchanged.generation, 2);
    assert!(
        unchanged
            .snapshot
            .entries
            .iter()
            .any(|e| e.path.as_str() == "new.txt")
    );
    let changed = store
        .refresh_projection(2, &updated(&before), &mut received)
        .unwrap();
    assert!(
        !changed
            .snapshot
            .entries
            .iter()
            .any(|e| e.path.as_str() == "new.txt")
    );
    assert!(
        store
            .preview_namespace(
                changed.generation,
                &NamespaceMutation::Create {
                    destination: RepoPath::parse("new.txt").unwrap(),
                }
            )
            .is_err()
    );
}

#[test]
fn refresh_history_is_verified_against_immutable_snapshots() {
    let (_f, store, before) = ready();
    store
        .refresh_projection(1, &updated(&before), &mut received)
        .unwrap();
    assert_eq!(store.fsck().unwrap().refresh_operations, 1);
    let access = format::access(&store, SessionMode::Exclusive).unwrap();
    let bytes: Vec<u8> = access
        .connection
        .query_row(
            "SELECT payload FROM projection_refresh_history WHERE generation=2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["expectedDigest"] = serde_json::json!("0".repeat(64));
    access
        .connection
        .execute(
            "UPDATE projection_refresh_history SET payload=?1 WHERE generation=2",
            [layrs_core::scm::canonical_json(&value).unwrap()],
        )
        .unwrap();
    drop(access);
    assert!(store.fsck().is_err());
}

#[test]
fn refresh_refuses_directory_to_leaf_collision_without_mutating_children() {
    let (f, store, before) = ready();
    let mut after = updated(&before);
    let entry = after
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "nested/b.txt")
        .unwrap();
    entry.path = RepoPath::parse("nested").unwrap();
    after.entries.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(
        store
            .refresh_projection(1, &after, &mut |_, _| panic!(
                "directory collision before fetch"
            ))
            .is_err()
    );
    assert_eq!(store.state().unwrap().generation, 1);
    assert_eq!(fs::read(f.0.join("nested/b.txt")).unwrap(), b"beta");
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
}

#[test]
#[cfg(unix)]
fn refresh_never_follows_directory_symlink() {
    let (f, store, before) = ready();
    let other = Fixture::new();
    fs::write(other.0.join("sentinel.txt"), b"outside").unwrap();
    std::os::unix::fs::symlink(&other.0, f.0.join("link")).unwrap();
    let mut after = updated(&before);
    after.entries.push(ProjectionEntry {
        path: RepoPath::parse("link/new.txt").unwrap(),
        target: Some(TreeEntryTarget::file(FileId::from_bytes(b"new"))),
        executable: false,
        writable: true,
        state: EntryState::Materialized,
    });
    after.entries.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(
        store
            .refresh_projection(1, &after, &mut |_, _| panic!("symlink preflight"))
            .is_err()
    );
    assert!(!other.0.join("new.txt").exists());
    assert_eq!(fs::read(other.0.join("sentinel.txt")).unwrap(), b"outside");
}

#[cfg(windows)]
#[test]
fn refresh_keeps_reserved_executable_metadata_without_materializing_it() {
    let (_f, store, before) = ready();
    let mut after = updated(&before);
    after
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "private.txt")
        .unwrap()
        .executable = true;
    store.refresh_projection(1, &after, &mut received).unwrap();
    assert_eq!(store.state().unwrap().snapshot, after);
}
