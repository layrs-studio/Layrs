use super::*;
mod provider_schema;
mod r2;
mod r3;
mod r4;
mod recovery;
mod validation_seams;
use layrs_core::projection::{
    AvailabilityRequest, OfflineOperation, ProjectionSelection, ProjectionVersion,
};
use std::io::Write;

fn family(f: &Fixture) -> PartialStoreFamily {
    let root = f.0.join("family");
    fs::create_dir(&root).unwrap();
    let id = identity();
    PartialStoreFamily::create(
        root,
        PartialStoreIdentity {
            store_id: id.store_id,
            space_id: id.space_id,
            origin: id.origin,
            principal_id: id.principal_id,
        },
        SharedQuota {
            bytes: 16 * 1024 * 1024 * 1024,
        },
    )
    .unwrap()
}
fn member(f: &Fixture, family: &PartialStoreFamily, name: &str, hydrate: bool) -> ProjectionStore {
    let root = f.0.join(name);
    fs::create_dir(&root).unwrap();
    let (before, after) = hydration();
    let store = family
        .create_worktree(
            &root,
            WorktreeId::parse(format!("worktree:{name}")).unwrap(),
            before,
            ProjectionSelection::All,
        )
        .unwrap();
    if hydrate {
        store.install_projection(0, &after, &mut provide).unwrap();
    }
    store
}
fn request(store: &ProjectionStore, path: &str) -> AvailabilityRequest {
    AvailabilityRequest {
        version: store.current_version().unwrap(),
        other: None,
        paths: vec![RepoPath::parse(path).unwrap()],
        operation: OfflineOperation::Read,
    }
}
#[test]
fn editing_before_first_capture_keeps_the_exact_base_and_unchanged_capture_leaves() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let version = store.current_version().unwrap();
    fs::write(
        store.checkout_root().join("a.txt"),
        b"edited before first capture",
    )
    .unwrap();
    let patch = store.capture_patch(1).unwrap();
    let mut base = Vec::new();
    store
        .read_version_file(&version, &RepoPath::parse("a.txt").unwrap(), &mut base)
        .unwrap();
    assert_eq!(base, b"alpha");
    let captured = ProjectionVersion::Capture {
        worktree_id: store.identity().worktree_id.clone(),
        sequence: patch.sequence,
        digest: patch.digest,
    };
    let mut unchanged = Vec::new();
    store
        .read_version_file(
            &captured,
            &RepoPath::parse("nested/b.txt").unwrap(),
            &mut unchanged,
        )
        .unwrap();
    assert_eq!(unchanged, b"beta");
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Version {
                version,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    store.apply_restore(&preview).unwrap();
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"alpha"
    );
    store.fsck().unwrap();
}
#[test]
fn two_private_worktrees_share_bytes_but_keep_generations_captures_and_pending_publication() {
    let f = Fixture::new();
    let family = family(&f);
    let a = member(&f, &family, "a", true);
    let bytes = family.usage().unwrap().content_bytes;
    let b = member(&f, &family, "b", true);
    assert_eq!(family.usage().unwrap().content_bytes, bytes);
    fs::write(a.checkout_root().join("a.txt"), b"agent A").unwrap();
    fs::write(b.checkout_root().join("a.txt"), b"agent B").unwrap();
    let ca = a.capture_patch(1).unwrap();
    let cb = b.capture_patch(1).unwrap();
    assert_ne!(ca.digest, cb.digest);
    a.begin_publication(ca.sequence, 1, br#"{"request":"A"}"#)
        .unwrap();
    assert!(b.pending_publication().unwrap().is_none());
    let mut av = Vec::new();
    a.write_captured_file(ca.sequence, &RepoPath::parse("a.txt").unwrap(), &mut av)
        .unwrap();
    assert_eq!(av, b"agent A");
    let mut bv = Vec::new();
    b.write_captured_file(cb.sequence, &RepoPath::parse("a.txt").unwrap(), &mut bv)
        .unwrap();
    assert_eq!(bv, b"agent B");
    assert_eq!(a.state().unwrap().generation, b.state().unwrap().generation);
    a.fsck().unwrap();
    b.fsck().unwrap();
}
struct PreparationStop(PreparationPoint);
impl PreparationFaults for PreparationStop {
    fn check(&self, p: PreparationPoint) -> io::Result<()> {
        if p == self.0 {
            Err(io::Error::other("preparation interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn preparation_is_ready_only_after_exact_content_and_private_claim_acceptance() {
    for point in [
        PreparationPoint::IntentDurable,
        PreparationPoint::ContentVerified,
        PreparationPoint::ProtectionDurable,
        PreparationPoint::ReadyDurable,
    ] {
        let f = Fixture::new();
        let family = family(&f);
        let store = member(&f, &family, "a", false);
        let req = request(&store, "a.txt");
        assert!(!store.availability(&req).unwrap().available);
        let intent = store.begin_preparation(&req).unwrap();
        assert!(
            store
                .resume_preparation_with_faults(
                    &intent.id,
                    &mut |_, _, out| out.write_all(b"alpha"),
                    &PreparationStop(point)
                )
                .is_err()
        );
        let store = ProjectionStore::open(store.checkout_root()).unwrap();
        assert_eq!(
            store.preparations().unwrap()[0].status,
            if point == PreparationPoint::ReadyDurable {
                "ready"
            } else {
                "acquiring"
            }
        );
        store
            .resume_preparation(&intent.id, &mut |_, _, out| out.write_all(b"alpha"))
            .unwrap();
        assert!(store.availability(&req).unwrap().prepared);
        store.fsck().unwrap();
    }
}
#[test]
fn reader_lease_blocks_reclamation_through_final_output_and_released_pin_can_be_evicted() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let req = request(&store, "a.txt");
    let pin = store
        .prepare_offline(&req, &mut |_, _, out| out.write_all(b"alpha"))
        .unwrap();
    store.release_preparation(&pin.id).unwrap();
    struct Inspect<'a> {
        family: &'a PartialStoreFamily,
        bytes: Vec<u8>,
    }
    impl Write for Inspect<'_> {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            assert!(self.family.evict_unretained().is_err());
            self.bytes.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut output = Inspect {
        family: &family,
        bytes: vec![],
    };
    store
        .read_version_file(&req.version, &req.paths[0], &mut output)
        .unwrap();
    assert_eq!(output.bytes, b"alpha");
    assert!(family.evict_unretained().unwrap() > 0);
    assert!(!store.availability(&req).unwrap().available);
}
#[test]
fn whole_capture_quota_preflight_protects_unrelated_dirty_files_before_restore() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let base = store.current_version().unwrap();
    fs::write(store.checkout_root().join("a.txt"), b"before restore").unwrap();
    fs::write(
        store.checkout_root().join("nested/b.txt"),
        vec![b'x'; 1024 * 1024],
    )
    .unwrap();
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Version {
                version: base,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    let usage = family.usage().unwrap();
    family
        .set_quota(SharedQuota {
            bytes: usage.content_bytes + 128 * 1024,
        })
        .unwrap();
    assert!(
        store
            .apply_restore(&preview)
            .unwrap_err()
            .to_string()
            .contains("quota")
    );
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"before restore"
    );
    assert_eq!(
        fs::metadata(store.checkout_root().join("nested/b.txt"))
            .unwrap()
            .len(),
        1024 * 1024
    );
}
struct ConversionStop(ConversionPoint);
impl ConversionFaults for ConversionStop {
    fn check(&self, p: ConversionPoint) -> io::Result<()> {
        if p == self.0 {
            Err(io::Error::other("conversion interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn conversion_preserves_source_and_exact_import_boundary_with_fresh_first_capture() {
    for point in [
        ConversionPoint::DestinationCreated,
        ConversionPoint::DatabaseCopied,
        ConversionPoint::ContentInstalled,
        ConversionPoint::CheckoutCopied,
        ConversionPoint::BeforeReady,
        ConversionPoint::ReadyRegistered,
    ] {
        let f = Fixture::new();
        let family = family(&f);
        let source_root = f.0.join("source");
        fs::create_dir(&source_root).unwrap();
        let (before, after) = hydration();
        let source = ProjectionStore::create(&source_root, identity(), before).unwrap();
        source.install_projection(0, &after, &mut provide).unwrap();
        fs::write(source_root.join("a.txt"), b"unpublished original").unwrap();
        let original = source.capture_patch(1).unwrap();
        let source_db = fs::read(source_root.join(".layrs/metadata/projection.sqlite3")).unwrap();
        let source_marker = fs::read(source_root.join(".layrs/FORMAT")).unwrap();
        let destination = f.0.join("destination");
        fs::create_dir(&destination).unwrap();
        assert!(
            source
                .convert_into_family_with_faults(
                    &family,
                    &destination,
                    WorktreeId::parse("worktree:converted").unwrap(),
                    &ConversionStop(point)
                )
                .is_err()
        );
        assert_eq!(
            ProjectionStore::open(&destination).is_ok(),
            point == ConversionPoint::ReadyRegistered
        );
        assert_eq!(
            fs::read(source_root.join(".layrs/metadata/projection.sqlite3")).unwrap(),
            source_db
        );
        assert_eq!(
            fs::read(source_root.join(".layrs/FORMAT")).unwrap(),
            source_marker
        );
        let (converted, receipt) = source.resume_conversion_into(&destination).unwrap();
        assert!(receipt.source_retained);
        let usage = family.usage().unwrap();
        family
            .set_quota(SharedQuota {
                bytes: usage.content_bytes,
            })
            .unwrap();
        // An already-accepted conversion needs no new allocation, even if quota
        // was lowered after the caller lost the successful response.
        source.resume_conversion_into(&destination).unwrap();
        family
            .set_quota(SharedQuota {
                bytes: 16 * 1024 * 1024 * 1024,
            })
            .unwrap();
        assert_eq!(converted.captured_patch(1).unwrap(), original);
        assert!(converted.is_imported_capture(1).unwrap());
        assert!(
            converted
                .begin_publication(1, 1, br#"{"request":"bad-import"}"#)
                .is_err()
        );
        let fresh = converted.capture_patch(1).unwrap();
        assert_eq!(fresh.sequence, 2);
        assert_eq!(fresh.identity, *converted.identity());
        assert_eq!(fresh.changes, original.changes);
        assert_eq!(converted.capture_patch(1).unwrap(), fresh);
        converted.fsck().unwrap();
        assert_eq!(
            fs::read(source_root.join(".layrs/metadata/projection.sqlite3")).unwrap(),
            source_db
        );
    }
}
#[test]
fn imported_terminal_receipt_retains_original_publication_identity() {
    let f = Fixture::new();
    let family = family(&f);
    let root = f.0.join("source");
    fs::create_dir(&root).unwrap();
    let (before, after) = hydration();
    let source = ProjectionStore::create(&root, identity(), before).unwrap();
    source.install_projection(0, &after, &mut provide).unwrap();
    fs::write(root.join("a.txt"), b"rejected proposal").unwrap();
    let capture = source.capture_patch(1).unwrap();
    source
        .begin_publication(capture.sequence, 1, br#"{"request":"historical"}"#)
        .unwrap();
    source
        .record_publication_receipt(capture.sequence, br#"{"result":"rejected"}"#)
        .unwrap();
    source
        .finish_publication(capture.sequence, br#"{"result":"rejected"}"#, None)
        .unwrap();
    let root = f.0.join("converted");
    fs::create_dir(&root).unwrap();
    let (converted, _) = source
        .convert_into_family(
            &family,
            &root,
            WorktreeId::parse("worktree:converted").unwrap(),
        )
        .unwrap();
    assert!(converted.completed_publication(1).is_err());
    let (owner, receipt) = converted.imported_publication(1).unwrap().unwrap();
    assert_eq!(owner, *source.identity());
    assert_eq!(receipt.receipt_bytes, br#"{"result":"rejected"}"#);
    let access = format::access(&converted, SessionMode::Exclusive).unwrap();
    access
        .connection
        .execute("DELETE FROM projection_publications WHERE sequence=1", [])
        .unwrap();
    drop(access);
    assert!(
        ProjectionStore::open(converted.checkout_root())
            .err()
            .unwrap()
            .to_string()
            .contains("publication prefix")
    );
}
#[test]
fn legacy_marker_does_not_adopt_schema7_or_shared_locator() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    let bytes = fs::read(store.checkout_root().join(".layrs/FORMAT")).unwrap();
    let marker: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(marker["formatVersion"], 4);
    assert_eq!(marker["minReaderVersion"], 4);
    let legacy = f.0.join("legacy");
    fs::create_dir(&legacy).unwrap();
    let legacy = ProjectionStore::create(&legacy, identity(), snapshot()).unwrap();
    let c = database::connect(
        &legacy.root.join(".layrs/metadata/projection.sqlite3"),
        true,
    )
    .unwrap();
    c.execute_batch("PRAGMA user_version=7;").unwrap();
    drop(c);
    assert!(ProjectionStore::open(legacy.checkout_root()).is_err());
    assert!(!family.usage().unwrap().windows_power_loss_qualified);
}

#[test]
fn exact_metadata_resolver_preserves_versions_and_validates_family_binding() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    assert!(store.is_shared());
    assert_eq!(
        store.shared_family().unwrap().unwrap().root(),
        family.root()
    );
    let base = store.current_version().unwrap();
    let initial = store.version_metadata(&base).unwrap();
    let remote = store
        .record_version_observation_at_generation(1, &initial.0, &initial.1)
        .unwrap();
    fs::write(store.checkout_root().join("a.txt"), b"new capture").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let captured = ProjectionVersion::Capture {
        worktree_id: store.identity().worktree_id.clone(),
        sequence: capture.sequence,
        digest: capture.digest.clone(),
    };
    let captured_metadata = store.version_metadata(&captured).unwrap();
    assert_eq!(
        captured_metadata.0.entries[0].target,
        Some(TreeEntryTarget::file(FileId::from_bytes(b"new capture")))
    );
    assert_eq!(captured_metadata.0.base_step_id, initial.0.base_step_id);
    let mut next = initial.0.clone();
    next.observed_authorization_epoch += 1;
    store.compare_and_swap(1, &next).unwrap();
    assert_eq!(store.version_metadata(&base).unwrap(), initial);
    assert_eq!(store.version_metadata(&remote).unwrap(), initial);
    assert_eq!(
        store.version_metadata(&captured).unwrap(),
        captured_metadata
    );
    let mut wrong = captured;
    if let ProjectionVersion::Capture { digest, .. } = &mut wrong {
        *digest = "wrong".into();
    }
    assert!(store.version_metadata(&wrong).is_err());
    let legacy = f.0.join("legacy");
    fs::create_dir(&legacy).unwrap();
    let legacy = ProjectionStore::create(&legacy, identity(), snapshot()).unwrap();
    assert!(!legacy.is_shared());
    assert!(legacy.shared_family().unwrap().is_none());
}

#[test]
fn generation_bound_preparation_rejects_stale_acceptance_and_resumes_stored_request() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let req = request(&store, "a.txt");
    let intent = store.begin_preparation_at_generation(0, &req).unwrap();
    let mut next = store.state().unwrap().snapshot;
    next.observed_authorization_epoch += 1;
    store.compare_and_swap(0, &next).unwrap();
    let before = family.usage().unwrap();
    assert!(matches!(
        store.record_version_observation_at_generation(0, &next, &ProjectionSelection::All),
        Err(ProjectionStoreError::Stale {
            expected: 0,
            actual: 1
        })
    ));
    assert!(matches!(
        store.begin_preparation_at_generation(0, &req),
        Err(ProjectionStoreError::Stale {
            expected: 0,
            actual: 1
        })
    ));
    assert!(matches!(
        store.prepare_offline_at_generation(0, &req, &mut |_, _, _| panic!("stale acquisition")),
        Err(ProjectionStoreError::Stale {
            expected: 0,
            actual: 1
        })
    ));
    assert!(matches!(
        store.resume_preparation_at_generation(0, &intent.id, &mut |_, _, _| panic!(
            "stale acquisition"
        )),
        Err(ProjectionStoreError::Stale {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(family.usage().unwrap(), before);
    assert_eq!(store.preparations().unwrap(), vec![intent.clone()]);
    let checkout = store.checkout_root().to_owned();
    let prepared = store
        .resume_preparation_at_generation(1, &intent.id, &mut |version, path, out| {
            assert_eq!(version, &req.version);
            assert_eq!(path, &req.paths[0]);
            // A concurrent private generation writer cannot cross the acceptance lock.
            let checkout = checkout.clone();
            assert!(
                std::thread::spawn(move || ProjectionStore::open(checkout).is_err())
                    .join()
                    .unwrap()
            );
            out.write_all(b"alpha")
        })
        .unwrap();
    assert_eq!(prepared.request, req);
    assert_eq!(prepared.status, "ready");
    assert!(store.availability(&req).unwrap().prepared);
}

#[test]
fn missing_captured_replacement_is_corruption_never_acquisition_fallback() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    fs::write(
        store.checkout_root().join("a.txt"),
        b"local only replacement",
    )
    .unwrap();
    let capture = store.capture_patch(1).unwrap();
    let replacement = capture.changes[0].replacement.as_ref().unwrap();
    let lease = super::super::shared::for_store(&store).unwrap().unwrap();
    let manifest = lease.manifest_for(&replacement.file_id).unwrap().unwrap();
    let chunk = manifest.chunks()[0]
        .chunk_id()
        .as_str()
        .strip_prefix("blake3:")
        .unwrap();
    fs::remove_file(family.root().join("objects/chunks").join(chunk)).unwrap();
    drop(lease);
    let req = AvailabilityRequest {
        version: ProjectionVersion::Capture {
            worktree_id: store.identity().worktree_id.clone(),
            sequence: capture.sequence,
            digest: capture.digest,
        },
        other: None,
        paths: vec![RepoPath::parse("a.txt").unwrap()],
        operation: OfflineOperation::Read,
    };
    assert!(store.version_metadata(&req.version).is_err());
    assert!(
        store
            .prepare_offline_at_generation(1, &req, &mut |_, _, _| panic!(
                "local capture corruption must not refetch"
            ))
            .is_err()
    );
    assert!(store.preparations().unwrap().is_empty());
}

#[test]
fn interrupted_sweep_readmission_cannot_delete_newly_retained_content() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let req = request(&store, "a.txt");
    let pin = store
        .prepare_offline(&req, &mut |_, _, out| out.write_all(b"alpha"))
        .unwrap();
    store.release_preparation(&pin.id).unwrap();
    family.reconcile_retention().unwrap();
    {
        // Reproduce the durable state after sweep's index commit but before
        // unlink (process interruption or a Windows sharing violation).
        let lease = family.lease(SessionMode::Exclusive).unwrap();
        let mut catalog = lease.catalog(true).unwrap();
        let tx = catalog.connection.transaction().unwrap();
        tx.execute_batch(
            "INSERT INTO sweep SELECT 'manifests',id,length(payload) FROM manifests;
            INSERT INTO sweep SELECT 'chunks',id,bytes FROM chunks;
            DELETE FROM manifest_chunks; DELETE FROM manifests; DELETE FROM chunks;",
        )
        .unwrap();
        tx.commit().unwrap();
    }
    store
        .prepare_offline(&req, &mut |_, _, out| out.write_all(b"alpha"))
        .unwrap();
    assert_eq!(family.evict_unretained().unwrap(), 0);
    let mut actual = Vec::new();
    store
        .read_version_file(&req.version, &req.paths[0], &mut actual)
        .unwrap();
    assert_eq!(actual, b"alpha");
    store.fsck().unwrap();
}

#[test]
fn preparation_dependencies_follow_operation_and_explicit_acquisition_cancel_releases_charge() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let read = request(&store, "a.txt");
    for operation in [
        OfflineOperation::CompareMetadata,
        OfflineOperation::CompareContent,
    ] {
        let req = AvailabilityRequest {
            other: Some(read.version.clone()),
            operation,
            ..read.clone()
        };
        let prepared = store
            .prepare_offline(&req, &mut |_, _, _| panic!("comparison requires no bytes"))
            .unwrap();
        assert_eq!(prepared.status, "ready");
        assert!(store.availability(&req).unwrap().prepared);
    }
    assert_eq!(family.usage().unwrap().content_bytes, 0);
    assert!(!store.availability(&read).unwrap().available);
    for operation in [OfflineOperation::Capture, OfflineOperation::Restore] {
        let req = AvailabilityRequest {
            operation,
            ..read.clone()
        };
        assert!(
            store
                .begin_preparation(&req)
                .unwrap_err()
                .to_string()
                .contains("offline_preparation_unsupported")
        );
    }
    let intent = store.begin_preparation(&read).unwrap();
    let before = family.usage().unwrap().reserved_bytes;
    family.reconcile_retention().unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, before);
    store.release_preparation(&intent.id).unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    assert!(
        store
            .resume_preparation(&intent.id, &mut |_, _, _| panic!(
                "released preparation must not acquire"
            ))
            .is_err()
    );
}
