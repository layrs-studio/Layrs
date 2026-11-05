use super::*;
use crate::projection::shared::{self as content_family, InstallationPoint};

#[test]
fn installation_intent_replays_each_boundary_without_duplicate_staging_charge() {
    for stop in [
        InstallationPoint::IntentDurable,
        InstallationPoint::ChunkStaged,
        InstallationPoint::ManifestDurable,
        InstallationPoint::ChunkPublished,
        InstallationPoint::BeforeCatalogCommit,
        InstallationPoint::CatalogCommitted,
    ] {
        let f = Fixture::new();
        let family = family(&f);
        let store = member(&f, &family, "a", false);
        let bytes = vec![73u8; 1024 * 1024 + 17];
        let expected = CapturedFile {
            file_id: FileId::from_bytes(&bytes),
            bytes: bytes.len() as u64,
            executable: false,
        };
        let access = format::access(&store, SessionMode::Exclusive).unwrap();
        retention::reserve(
            &access.connection,
            &store,
            "capture:test",
            "test",
            std::slice::from_ref(&expected),
            0,
        )
        .unwrap();
        let reservation = retention::active_budget(&access.connection).unwrap();
        let lease = access.family.as_ref().unwrap();
        let before = lease.usage().unwrap();
        assert!(
            lease
                .install_file_with_faults(
                    &mut bytes.as_slice(),
                    &expected,
                    &reservation,
                    &mut |point| {
                        if point == stop {
                            Err(io::Error::other("interruption"))
                        } else {
                            Ok(())
                        }
                    }
                )
                .is_err()
        );
        let interrupted = lease.usage().unwrap();
        assert_eq!(
            interrupted.content_bytes + interrupted.reserved_bytes,
            before.content_bytes + before.reserved_bytes
        );
        assert!(
            lease
                .release_reservation(&reservation, &store.identity().worktree_id)
                .is_err()
        );
        drop(access);
        let access = format::access(&store, SessionMode::Exclusive).unwrap();
        access
            .family
            .as_ref()
            .unwrap()
            .install_file(&mut bytes.as_slice(), &expected, &reservation)
            .unwrap();
        drop(access);
        assert_eq!(
            fs::read_dir(family.root().join("staging")).unwrap().count(),
            0
        );
        store.cancel_shared_reservation().unwrap();
        assert_eq!(family.usage().unwrap().reserved_bytes, 0);
        assert!(family.evict_unretained().unwrap() > 0);
    }
}

#[test]
fn acquiring_cancel_finishes_verified_owned_staging_without_network_or_checkout_changes() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let req = request(&store, "a.txt");
    let intent = store.begin_preparation(&req).unwrap();
    let access = format::access(&store, SessionMode::Exclusive).unwrap();
    let reservation: String = access
        .connection
        .query_row(
            "SELECT reservation FROM projection_preparations WHERE id=?1",
            [&intent.id],
            |r| r.get(0),
        )
        .unwrap();
    let file = CapturedFile {
        file_id: FileId::from_bytes(b"alpha"),
        bytes: 5,
        executable: false,
    };
    assert!(
        access
            .family
            .as_ref()
            .unwrap()
            .install_file_with_faults(
                &mut b"alpha".as_slice(),
                &file,
                &reservation,
                &mut |point| {
                    if point == InstallationPoint::ChunkPublished {
                        Err(io::Error::other("before catalog acceptance"))
                    } else {
                        Ok(())
                    }
                }
            )
            .is_err()
    );
    drop(access);
    store.release_preparation(&intent.id).unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    assert!(!store.checkout_root().join("a.txt").exists());
    assert!(family.evict_unretained().unwrap() > 0);
}

#[test]
fn orphan_reservation_reconciles_and_cached_begin_pins_against_eviction() {
    let f = Fixture::new();
    let family = family(&f);
    let a = member(&f, &family, "a", false);
    let b = member(&f, &family, "b", false);
    {
        let lease = content_family::for_store(&a).unwrap().unwrap();
        lease
            .reserve(&a.identity().worktree_id, "before private insert", 123456)
            .unwrap();
    }
    assert_eq!(family.usage().unwrap().reserved_bytes, 123456);
    family.reconcile_retention().unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    let pin = a
        .prepare_offline(&request(&a, "a.txt"), &mut |_, _, out| {
            out.write_all(b"alpha")
        })
        .unwrap();
    let intent = b.begin_preparation(&request(&b, "a.txt")).unwrap();
    a.release_preparation(&pin.id).unwrap();
    assert_eq!(family.evict_unretained().unwrap(), 0);
    b.resume_preparation(&intent.id, &mut |_, _, _| {
        panic!("begin retained exact resident bytes")
    })
    .unwrap();
    assert!(b.availability(&request(&b, "a.txt")).unwrap().prepared);
}

#[test]
fn failed_capture_budget_can_be_cancelled_after_more_edits_without_losing_accepted_work() {
    struct Stop;
    impl CaptureFaults for Stop {
        fn check(&self, point: CapturePoint) -> io::Result<()> {
            if point == CapturePoint::BeforeCommit {
                Err(io::Error::other("interrupted capture"))
            } else {
                Ok(())
            }
        }
    }
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    fs::write(store.checkout_root().join("a.txt"), b"accepted").unwrap();
    let accepted = store.capture_patch(1).unwrap();
    fs::write(store.checkout_root().join("a.txt"), b"interrupted").unwrap();
    assert!(store.capture_patch_with_faults(1, &Stop).is_err());
    fs::write(
        store.checkout_root().join("a.txt"),
        b"new edits after interruption",
    )
    .unwrap();
    assert!(store.capture_patch(1).is_err());
    store.cancel_shared_reservation().unwrap();
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"new edits after interruption"
    );
    let next = store.capture_patch(1).unwrap();
    assert_eq!(next.sequence, accepted.sequence + 1);
    assert_eq!(store.captured_patch(accepted.sequence).unwrap(), accepted);
    let check = store.fsck().unwrap();
    assert_eq!(check.captured_files, 2);
    assert_eq!(
        check.captured_bytes,
        (b"accepted".len() + b"new edits after interruption".len()) as u64
    );
}

#[test]
fn availability_checks_only_target_residency_and_restore_whole_capture_capacity() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let version = store.current_version().unwrap();
    fs::write(store.checkout_root().join("a.txt"), b"restore preimage").unwrap();
    fs::write(
        store.checkout_root().join("nested/b.txt"),
        vec![7u8; 1024 * 1024],
    )
    .unwrap();
    let used = family.usage().unwrap();
    family
        .set_quota(SharedQuota {
            bytes: used.content_bytes + used.reserved_bytes + 65536 + 4096,
        })
        .unwrap();
    let req = AvailabilityRequest {
        version: version.clone(),
        other: None,
        paths: vec![RepoPath::parse("a.txt").unwrap()],
        operation: OfflineOperation::Restore,
    };
    let coverage = store.availability(&req).unwrap();
    assert!(!coverage.available);
    assert!(
        coverage
            .blockers
            .contains(&layrs_core::projection::AvailabilityBlocker::Quota)
    );
    let lease = content_family::for_store(&store).unwrap().unwrap();
    let other = lease
        .manifest_for(&FileId::from_bytes(b"beta"))
        .unwrap()
        .unwrap();
    fs::remove_file(
        family.root().join("objects/chunks").join(
            other.chunks()[0]
                .chunk_id()
                .as_str()
                .strip_prefix("blake3:")
                .unwrap(),
        ),
    )
    .unwrap();
    drop(lease);
    assert!(
        store
            .availability(&request(&store, "a.txt"))
            .unwrap()
            .available
    );
    assert!(
        store
            .availability(&request(&store, "nested/b.txt"))
            .is_err()
    );
    assert!(store.fsck().is_err());
}

#[test]
fn abandoning_unready_conversion_keeps_physical_data_and_unblocks_other_family_gc() {
    let f = Fixture::new();
    let family = family(&f);
    let other = member(&f, &family, "other", false);
    let pin = other
        .prepare_offline(&request(&other, "a.txt"), &mut |_, _, out| {
            out.write_all(b"alpha")
        })
        .unwrap();
    other.release_preparation(&pin.id).unwrap();
    let root = f.0.join("source");
    fs::create_dir(&root).unwrap();
    let (before, after) = hydration();
    let source = ProjectionStore::create(&root, identity(), before).unwrap();
    source.install_projection(0, &after, &mut provide).unwrap();
    let destination = f.0.join("destination");
    fs::create_dir(&destination).unwrap();
    let before_db = fs::read(root.join(".layrs/metadata/projection.sqlite3")).unwrap();
    assert!(
        source
            .convert_into_family_with_faults(
                &family,
                &destination,
                WorktreeId::parse("worktree:abandoned").unwrap(),
                &ConversionStop(ConversionPoint::CheckoutCopied)
            )
            .is_err()
    );
    assert!(family.evict_unretained().is_err());
    fs::write(
        root.join("a.txt"),
        b"source changed after failed conversion",
    )
    .unwrap();
    assert!(source.resume_conversion_into(&destination).is_err());
    family.abandon_conversion(&destination).unwrap();
    assert_eq!(
        fs::read(root.join(".layrs/metadata/projection.sqlite3")).unwrap(),
        before_db
    );
    assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        fs::read(root.join("a.txt")).unwrap(),
        b"source changed after failed conversion"
    );
    assert!(family.evict_unretained().unwrap() > 0);
    assert!(source.resume_conversion_into(&destination).is_err());
}

#[test]
fn conversion_reader_refuses_old_schema_without_sql_write_and_bootstrap_can_resume() {
    let f = Fixture::new();
    let root = f.0.join("legacy");
    fs::create_dir(&root).unwrap();
    let store = ProjectionStore::create(&root, identity(), snapshot()).unwrap();
    let c = database::connect(&root.join(".layrs/metadata/projection.sqlite3"), true).unwrap();
    c.execute_batch("PRAGMA user_version=5;").unwrap();
    drop(c);
    let before = fs::read(root.join(".layrs/metadata/projection.sqlite3")).unwrap();
    assert!(ProjectionStore::open_conversion_source(&root).is_err());
    assert!(ProjectionStore::open_for_recovery(&root).is_err());
    assert_eq!(
        fs::read(root.join(".layrs/metadata/projection.sqlite3")).unwrap(),
        before
    );
    drop(store);
    let family = family(&f);
    let store = member(&f, &family, "bootstrap", false);
    {
        let lease = family.lease(SessionMode::Exclusive).unwrap();
        lease
            .catalog(true)
            .unwrap()
            .connection
            .execute(
                "DELETE FROM worktrees WHERE id=?1",
                [store.identity().worktree_id.as_str()],
            )
            .unwrap();
    }
    assert!(ProjectionStore::open(store.checkout_root()).is_err());
    let recovered = family
        .resume_worktree(
            store.checkout_root(),
            &hydration().0,
            &ProjectionSelection::All,
        )
        .unwrap();
    assert_eq!(
        recovered.generation_version(0).unwrap(),
        recovered.current_version().unwrap()
    );
    assert!(recovered.generation_version(1).is_err());
}

#[test]
fn old_generation_release_preserves_active_bytes_and_retained_capture_dependencies() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let mut next = store.state().unwrap().snapshot;
    next.observed_authorization_epoch += 1;
    store.compare_and_swap(1, &next).unwrap();
    assert!(store.release_generation_retention(2).is_err());
    assert!(store.release_generation_retention(1).unwrap() > 0);
    fs::write(store.checkout_root().join("a.txt"), b"retained capture").unwrap();
    store.capture_patch(2).unwrap();
    next.observed_authorization_epoch += 1;
    store.compare_and_swap(2, &next).unwrap();
    assert!(store.release_generation_retention(2).unwrap() > 0);
    family.evict_unretained().unwrap();
    let mut base = Vec::new();
    store
        .read_version_file(
            &store.generation_version(2).unwrap(),
            &RepoPath::parse("a.txt").unwrap(),
            &mut base,
        )
        .unwrap();
    assert_eq!(base, b"alpha");
    store.captured_patch(1).unwrap();
    assert_eq!(store.release_generation_retention(0).unwrap(), 0);
}

#[test]
fn interrupted_installation_protects_reused_cas_chunks_from_eviction() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let prefix = vec![8u8; 1024 * 1024];
    let first = CapturedFile {
        file_id: FileId::from_bytes(&prefix),
        bytes: prefix.len() as u64,
        executable: false,
    };
    {
        let access = format::access(&store, SessionMode::Exclusive).unwrap();
        retention::reserve(
            &access.connection,
            &store,
            "capture:first",
            "first",
            std::slice::from_ref(&first),
            0,
        )
        .unwrap();
        let reservation = retention::active_budget(&access.connection).unwrap();
        access
            .family
            .as_ref()
            .unwrap()
            .install_file(&mut prefix.as_slice(), &first, &reservation)
            .unwrap();
    }
    store.cancel_shared_reservation().unwrap();
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(b"tail");
    let next = CapturedFile {
        file_id: FileId::from_bytes(&bytes),
        bytes: bytes.len() as u64,
        executable: false,
    };
    let reservation;
    {
        let access = format::access(&store, SessionMode::Exclusive).unwrap();
        retention::reserve(
            &access.connection,
            &store,
            "capture:next",
            "next",
            std::slice::from_ref(&next),
            0,
        )
        .unwrap();
        reservation = retention::active_budget(&access.connection).unwrap();
        assert!(
            access
                .family
                .as_ref()
                .unwrap()
                .install_file_with_faults(
                    &mut bytes.as_slice(),
                    &next,
                    &reservation,
                    &mut |point| if point == InstallationPoint::ChunkPublished {
                        Err(io::Error::other("interrupted reused chunk"))
                    } else {
                        Ok(())
                    }
                )
                .is_err()
        );
    }
    // The old manifest is evictable; its reused chunk belongs to the active intent.
    family.evict_unretained().unwrap();
    {
        let access = format::access(&store, SessionMode::Exclusive).unwrap();
        let manifest = access
            .family
            .as_ref()
            .unwrap()
            .install_file(&mut bytes.as_slice(), &next, &reservation)
            .unwrap();
        access
            .family
            .as_ref()
            .unwrap()
            .verify_manifest(&manifest)
            .unwrap();
    }
    store.cancel_shared_reservation().unwrap();
}

#[test]
fn unknown_stage_entry_refuses_cancellation_and_keeps_its_reservation() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let file = CapturedFile {
        file_id: FileId::from_bytes(b"alpha"),
        bytes: 5,
        executable: false,
    };
    let path;
    {
        let access = format::access(&store, SessionMode::Exclusive).unwrap();
        retention::reserve(
            &access.connection,
            &store,
            "capture:ambiguous",
            "same",
            std::slice::from_ref(&file),
            0,
        )
        .unwrap();
        let reservation = retention::active_budget(&access.connection).unwrap();
        let lease = access.family.as_ref().unwrap();
        assert!(
            lease
                .install_file_with_faults(
                    &mut b"alpha".as_slice(),
                    &file,
                    &reservation,
                    &mut |point| if point == InstallationPoint::IntentDurable {
                        Err(io::Error::other("intent interrupted"))
                    } else {
                        Ok(())
                    }
                )
                .is_err()
        );
        let payload: Vec<u8> = lease
            .catalog(false)
            .unwrap()
            .connection
            .query_row(
                "SELECT payload FROM installations WHERE reservation=?1",
                [reservation],
                |r| r.get(0),
            )
            .unwrap();
        let intent: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        path = family
            .root()
            .join("staging")
            .join(intent["directory"].as_str().unwrap())
            .join("unrecognized");
        fs::write(&path, b"retain this unknown data").unwrap();
    }
    let before = family.usage().unwrap().reserved_bytes;
    assert!(store.cancel_shared_reservation().is_err());
    assert_eq!(family.usage().unwrap().reserved_bytes, before);
    assert_eq!(fs::read(&path).unwrap(), b"retain this unknown data");
    fs::remove_file(path).unwrap();
    store.cancel_shared_reservation().unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
}
