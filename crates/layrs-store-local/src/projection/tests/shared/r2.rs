use super::*;

pub(super) fn assert_unchanged_database(actual: &[u8], expected: &[u8]) {
    assert!(
        actual == expected,
        "database bytes changed: expected {} bytes / {}, actual {} bytes / {}",
        expected.len(),
        blake3::hash(expected),
        actual.len(),
        blake3::hash(actual)
    );
}

#[test]
fn namespace_limit_fails_before_reserving_and_changed_capture_remains_possible() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let state = store.state().unwrap();
    let destination = RepoPath::parse("new.txt").unwrap();
    let state = store
        .observe_absent_paths(
            state.generation,
            &state.digest,
            std::slice::from_ref(&destination),
        )
        .unwrap();
    fs::write(
        store.checkout_root().join("a.txt"),
        vec![3u8; 15 * 1024 * 1024],
    )
    .unwrap();
    let preview = store
        .preview_namespace(state.generation, &NamespaceMutation::Create { destination })
        .unwrap();
    let error = store
        .apply_namespace(&preview, Some(&vec![4u8; 2 * 1024 * 1024]))
        .unwrap_err();
    assert!(error.to_string().contains("capture budget"), "{error}");
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    assert!(!store.checkout_root().join("new.txt").exists());
    assert_eq!(
        fs::metadata(store.checkout_root().join("a.txt"))
            .unwrap()
            .len(),
        15 * 1024 * 1024
    );
    fs::write(
        store.checkout_root().join("a.txt"),
        b"changed after refusal",
    )
    .unwrap();
    store.capture_patch(state.generation).unwrap();
    store.fsck().unwrap();
}

struct EditDuringRestore(PathBuf);
impl RestoreFaults for EditDuringRestore {
    fn check(&self, point: RestorePoint) -> io::Result<()> {
        if point == RestorePoint::Staged {
            fs::write(&self.0, b"new edit during staging")?;
        }
        Ok(())
    }
}
#[test]
fn stale_restore_can_cancel_exact_staging_then_capture_changed_checkout() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let path = store.checkout_root().join("a.txt");
    fs::write(&path, b"restore preimage").unwrap();
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Version {
                version: store.current_version().unwrap(),
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    assert!(
        store
            .apply_restore_with_faults(&preview, &EditDuringRestore(path.clone()))
            .unwrap_err()
            .to_string()
            .contains("checkout changed")
    );
    assert!(family.usage().unwrap().reserved_bytes > 0);
    store.cancel_shared_reservation().unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    assert_eq!(fs::read(&path).unwrap(), b"new edit during staging");
    assert_eq!(
        fs::read_dir(store.checkout_root().join(".layrs/projection-quarantine"))
            .unwrap()
            .count(),
        0
    );
    store.capture_patch(1).unwrap();
    store.fsck().unwrap();
}

#[test]
fn recovery_opener_keeps_pending_journal_and_cancel_cannot_drop_accepted_install() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    assert!(
        store
            .install_projection_with_faults(
                0,
                &hydration().1,
                &mut provide,
                &StopInstall(InstallPoint::JournalDurable)
            )
            .is_err()
    );
    assert!(ProjectionStore::open(store.checkout_root()).is_err());
    let db = family
        .member_metadata(&store.identity().worktree_id)
        .join("projection.sqlite3");
    let before = fs::read(&db).unwrap();
    let recovered = ProjectionStore::open_for_recovery(store.checkout_root()).unwrap();
    assert_eq!(recovered.identity(), store.identity());
    assert_unchanged_database(&fs::read(&db).unwrap(), &before);
    assert!(recovered.state().is_err());
    assert!(recovered.cancel_shared_reservation().is_err());
    ProjectionStore::resume_installation(store.checkout_root()).unwrap();
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"alpha"
    );
}

#[test]
fn capture_availability_matches_changed_only_write_eligibility() {
    let f = Fixture::new();
    let family = family(&f);
    let root = f.0.join("a");
    fs::create_dir(&root).unwrap();
    let (mut before, mut after) = hydration();
    for snapshot in [&mut before, &mut after] {
        snapshot
            .entries
            .iter_mut()
            .find(|e| e.path.as_str() == "nested/b.txt")
            .unwrap()
            .writable = false;
    }
    let store = family
        .create_worktree(
            &root,
            WorktreeId::parse("worktree:a").unwrap(),
            before,
            ProjectionSelection::All,
        )
        .unwrap();
    store.install_projection(0, &after, &mut provide).unwrap();
    fs::write(root.join("a.txt"), b"edited writable").unwrap();
    let mut req = request(&store, "a.txt");
    req.operation = OfflineOperation::Capture;
    assert!(store.availability(&req).unwrap().available);
    store.capture_patch(1).unwrap();
    for replacement in [Some(b"edited read only".as_slice()), None] {
        if let Some(bytes) = replacement {
            fs::write(root.join("nested/b.txt"), bytes).unwrap();
        } else {
            fs::remove_file(root.join("nested/b.txt")).unwrap();
        }
        assert!(!store.availability(&req).unwrap().available);
        assert!(store.capture_patch(1).is_err());
        assert_eq!(
            fs::read(root.join("nested/b.txt")).ok(),
            replacement.map(|b| b.to_vec())
        );
    }
}

#[test]
fn small_preparation_charges_actual_bytes_and_quota_failure_resumes() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    family.set_quota(SharedQuota { bytes: 128 * 1024 }).unwrap();
    store
        .prepare_offline(&request(&store, "a.txt"), &mut |_, _, out| {
            out.write_all(b"alpha")
        })
        .unwrap();
    let mut observed = store.state().unwrap().snapshot;
    let large = vec![5u8; 192 * 1024];
    observed
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "nested/b.txt")
        .unwrap()
        .target = Some(TreeEntryTarget::file(FileId::from_bytes(&large)));
    let version = store
        .record_version_observation(&observed, &ProjectionSelection::All)
        .unwrap();
    let req = AvailabilityRequest {
        version,
        other: None,
        paths: vec![RepoPath::parse("nested/b.txt").unwrap()],
        operation: OfflineOperation::Read,
    };
    let prep = store.begin_preparation(&req).unwrap();
    let before = family.usage().unwrap();
    assert!(
        store
            .resume_preparation(&prep.id, &mut |_, _, out| out.write_all(&large))
            .unwrap_err()
            .to_string()
            .contains("partial_store_quota")
    );
    let after = family.usage().unwrap();
    assert_eq!(after.content_bytes, before.content_bytes);
    assert!(after.content_bytes + after.reserved_bytes <= after.limit_bytes);
    assert_eq!(
        store
            .preparations()
            .unwrap()
            .iter()
            .find(|p| p.id == prep.id)
            .unwrap()
            .status,
        "acquiring"
    );
    family
        .set_quota(SharedQuota { bytes: 1024 * 1024 })
        .unwrap();
    store
        .resume_preparation(&prep.id, &mut |_, _, out| out.write_all(&large))
        .unwrap();
    assert!(store.availability(&req).unwrap().prepared);
}

#[test]
fn failed_multi_file_preparation_keeps_successful_progress_pinned() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let mut req = request(&store, "a.txt");
    req.paths.push(RepoPath::parse("nested/b.txt").unwrap());
    let p = store.begin_preparation(&req).unwrap();
    assert!(
        store
            .resume_preparation(&p.id, &mut |_, path, out| if path.as_str() == "a.txt" {
                out.write_all(b"alpha")
            } else {
                Err(io::Error::other("transport failed"))
            })
            .is_err()
    );
    family.evict_unretained().unwrap();
    store
        .resume_preparation(&p.id, &mut |_, path, out| {
            assert_eq!(path.as_str(), "nested/b.txt");
            out.write_all(b"beta")
        })
        .unwrap();
    assert!(store.availability(&req).unwrap().prepared);
}

fn change_b(store: &ProjectionStore) {
    let mut next = store.state().unwrap().snapshot;
    let id = format!("blake3:{}", "8".repeat(64));
    next.base_step_id = StepId::parse(&id).unwrap();
    next.base_root_tree_id = TreeId::parse(&id).unwrap();
    next.entries
        .iter_mut()
        .find(|e| e.path.as_str() == "nested/b.txt")
        .unwrap()
        .target = Some(TreeEntryTarget::file(FileId::from_bytes(b"new beta")));
    store
        .refresh_projection(1, &next, &mut |path, out| {
            assert_eq!(path, "nested/b.txt");
            out.write_all(b"new beta")
        })
        .unwrap();
}
#[test]
fn retired_generation_releases_old_b_preserving_exact_active_subset_and_pin() {
    for pinned in [false, true] {
        let f = Fixture::new();
        let family = family(&f);
        let store = member(&f, &family, "a", true);
        let old = request(&store, "nested/b.txt");
        let pin = if pinned {
            Some(
                store
                    .prepare_offline(&old, &mut |_, _, _| panic!("already retained"))
                    .unwrap(),
            )
        } else {
            None
        };
        change_b(&store);
        assert!(store.release_generation_retention(1).unwrap() > 0);
        family.evict_unretained().unwrap();
        for (path, expected) in [
            ("a.txt", b"alpha".as_slice()),
            ("nested/b.txt", b"new beta".as_slice()),
        ] {
            let mut out = Vec::new();
            store
                .read_version_file(
                    &store.current_version().unwrap(),
                    &RepoPath::parse(path).unwrap(),
                    &mut out,
                )
                .unwrap();
            assert_eq!(out, expected);
        }
        assert_eq!(store.availability(&old).unwrap().available, pinned);
        if let Some(pin) = pin {
            store.release_preparation(&pin.id).unwrap();
            family.evict_unretained().unwrap();
            assert!(!store.availability(&old).unwrap().available);
        }
        store.fsck().unwrap();
    }
}

struct StopCreation(CreationPoint);
impl CreationFaults for StopCreation {
    fn check(&self, point: CreationPoint) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other("creation interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn missing_marker_bootstrap_abandon_preserves_data_and_unblocks_other_worktree_gc() {
    for deleted in [false, true] {
        let f = Fixture::new();
        let family = family(&f);
        let other = member(&f, &family, "other", false);
        let p = other
            .prepare_offline(&request(&other, "a.txt"), &mut |_, _, out| {
                out.write_all(b"alpha")
            })
            .unwrap();
        other.release_preparation(&p.id).unwrap();
        let root = f.0.join("bootstrap");
        fs::create_dir(&root).unwrap();
        let id = WorktreeId::parse("worktree:bootstrap").unwrap();
        assert!(
            family
                .create_worktree_with_faults(
                    &root,
                    id.clone(),
                    hydration().0,
                    ProjectionSelection::All,
                    &StopCreation(CreationPoint::DatabaseDurable)
                )
                .is_err()
        );
        assert!(!root.join(".layrs/FORMAT").exists());
        let db = family.member_metadata(&id).join("projection.sqlite3");
        let original = fs::read(&db).unwrap();
        assert!(family.evict_unretained().is_err());
        if deleted {
            fs::remove_dir_all(&root).unwrap();
        } else {
            fs::write(root.join("user.txt"), b"keep physical data").unwrap();
        }
        family.abandon_initializing(&id).unwrap();
        assert_unchanged_database(&fs::read(&db).unwrap(), &original);
        if !deleted {
            assert_eq!(
                fs::read(root.join("user.txt")).unwrap(),
                b"keep physical data"
            );
        }
        assert!(family.evict_unretained().unwrap() > 0);
    }
}
#[test]
fn ready_deleted_checkout_preserves_accepted_capture_without_blocking_family_gc() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    fs::write(
        store.checkout_root().join("a.txt"),
        b"accepted capture survives",
    )
    .unwrap();
    let capture = store.capture_patch(1).unwrap();
    let other = member(&f, &family, "other", false);
    let mut observed = other.state().unwrap().snapshot;
    observed.entries[0].target = Some(TreeEntryTarget::file(FileId::from_bytes(b"temporary")));
    let version = other
        .record_version_observation(&observed, &ProjectionSelection::All)
        .unwrap();
    let mut req = request(&other, "a.txt");
    req.version = version;
    let p = other
        .prepare_offline(&req, &mut |_, _, out| out.write_all(b"temporary"))
        .unwrap();
    other.release_preparation(&p.id).unwrap();
    fs::remove_dir_all(store.checkout_root()).unwrap();
    assert!(family.evict_unretained().unwrap() > 0);
    assert!(
        family
            .abandon_initializing(&store.identity().worktree_id)
            .is_err()
    );
    let lease = family.lease(SessionMode::Shared).unwrap();
    let m = lease
        .manifest_for(&capture.changes[0].replacement.as_ref().unwrap().file_id)
        .unwrap()
        .unwrap();
    let mut out = Vec::new();
    lease.stream(&m, &mut out).unwrap();
    assert_eq!(out, b"accepted capture survives");
}
#[test]
fn unreadable_initializing_private_identity_remains_registered() {
    let f = Fixture::new();
    let family = family(&f);
    let root = f.0.join("bootstrap");
    fs::create_dir(&root).unwrap();
    let id = WorktreeId::parse("worktree:bootstrap").unwrap();
    assert!(
        family
            .create_worktree_with_faults(
                &root,
                id.clone(),
                hydration().0,
                ProjectionSelection::All,
                &StopCreation(CreationPoint::DatabaseDurable)
            )
            .is_err()
    );
    let db = family.member_metadata(&id).join("projection.sqlite3");
    let backup = fs::read(&db).unwrap();
    fs::write(&db, b"damaged identity").unwrap();
    assert!(family.abandon_initializing(&id).is_err());
    assert!(family.evict_unretained().is_err());
    fs::write(&db, backup).unwrap();
    family.abandon_initializing(&id).unwrap();
}

// Executed only by the bounded subprocess tests below. Deliberately killed with
// an uncommitted cache-spilled SQLite transaction; no graceful rollback occurs.
#[test]
fn sqlite_spill_child() {
    let Some(path) = std::env::var_os("LAYRS_PARTIAL_SPILL_PATH") else {
        return;
    };
    let mode = std::env::var("LAYRS_PARTIAL_SPILL_MODE").unwrap();
    let ready = std::env::var_os("LAYRS_PARTIAL_SPILL_READY").unwrap();
    let c = rusqlite::Connection::open(PathBuf::from(path)).unwrap();
    c.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=4; PRAGMA cache_spill=ON; BEGIN IMMEDIATE;").unwrap();
    if mode == "catalog" {
        c.execute("UPDATE quota SET bytes=1", []).unwrap();
        for i in 0..512 {
            c.execute(
                "INSERT INTO reservations VALUES(?1,'worktree:a',?2,1)",
                rusqlite::params![format!("spill-{i}"), "x".repeat(8192)],
            )
            .unwrap();
        }
    } else {
        c.execute("DELETE FROM projection_claims", []).unwrap();
        if mode == "provider-owner" {
            c.execute_batch("DELETE FROM projection_provider_companion_files; DELETE FROM projection_provider_companion_deletions; DELETE FROM projection_provider_companions;").unwrap();
        }
        for i in 0..128 {
            c.execute(
                "INSERT INTO projection_observations VALUES(?1,?2)",
                rusqlite::params![format!("spill-{i}"), vec![9u8; 65536]],
            )
            .unwrap();
        }
    }
    fs::write(ready, b"spilled").unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
pub(super) fn kill_spilled_writer(path: &Path, mode: &str, ready: &Path) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "projection::tests::shared::r2::sqlite_spill_child",
            "--nocapture",
        ])
        .env("LAYRS_PARTIAL_SPILL_PATH", path)
        .env("LAYRS_PARTIAL_SPILL_MODE", mode)
        .env("LAYRS_PARTIAL_SPILL_READY", ready)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !ready.exists() && std::time::Instant::now() < until {
        if child.try_wait().unwrap().is_some() {
            panic!("spill child exited before readiness");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let signaled = ready.exists();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(signaled, "spill child timed out");
    let journal = PathBuf::from(format!("{}-journal", path.display()));
    assert!(fs::metadata(journal).unwrap().len() > 512);
    let readonly =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let error = readonly
        .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
        .unwrap_err();
    assert!(database::readonly_rollback(
        &ProjectionStoreError::Database(error)
    ));
}
#[test]
fn killed_catalog_spill_recovers_once_under_bound_exclusive_catalog_lock() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let before = family.usage().unwrap();
    let path = family.root().join("metadata/catalog.sqlite3");
    kill_spilled_writer(&path, "catalog", &f.0.join("ready"));
    let spilled = fs::read(&path).unwrap();
    assert!(ProjectionStore::open_for_recovery(store.checkout_root()).is_err());
    let discovery = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
    assert_eq!(
        discovery.recovery,
        vec![ProjectionRecoveryReason::FamilyRollback]
    );
    assert!(!discovery.conversion_eligible);
    assert_unchanged_database(&fs::read(&path).unwrap(), &spilled);
    let recovered =
        ProjectionStore::recover_databases(store.checkout_root(), &discovery.binding).unwrap();
    assert!(recovered.recovery.is_empty());
    let recovered = PartialStoreFamily::open(family.root()).unwrap();
    assert_eq!(recovered.usage().unwrap(), before);
    store.fsck().unwrap();
}
#[test]
fn killed_owner_spill_recovers_under_lifecycle_and_owner_locks_without_losing_claims() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    fs::write(store.checkout_root().join("a.txt"), b"accepted").unwrap();
    store.capture_patch(1).unwrap();
    let path = family
        .member_metadata(&store.identity().worktree_id)
        .join("projection.sqlite3");
    kill_spilled_writer(&path, "owner", &f.0.join("ready"));
    let spilled = fs::read(&path).unwrap();
    let discovery = ProjectionStore::inspect_recovery(store.checkout_root()).unwrap();
    assert_eq!(
        discovery.recovery,
        vec![ProjectionRecoveryReason::WorktreeRollback]
    );
    assert_unchanged_database(&fs::read(&path).unwrap(), &spilled);
    family.reconcile_retention().unwrap();
    family.evict_unretained().unwrap();
    store.captured_patch(1).unwrap();
    store.fsck().unwrap();
}
