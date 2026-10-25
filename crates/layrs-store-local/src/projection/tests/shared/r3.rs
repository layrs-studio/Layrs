use super::*;

fn reviewed_digest(preview: &mut RestorePreview) {
    preview.digest.clear();
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-projection-restore-preview-v1\0");
    h.update(&layrs_core::scm::canonical_json(preview).unwrap());
    preview.digest = format!("blake3:{}", h.finalize());
}

#[test]
fn remote_restore_review_is_bound_to_destination_and_native_checkout_even_on_replay() {
    let f = Fixture::new();
    let family = family(&f);
    let a = member(&f, &family, "a", true);
    let b = member(&f, &family, "b", true);
    let state = a.state().unwrap();
    assert_eq!(state, b.state().unwrap());
    let remote = a
        .record_version_observation_at_generation(1, &state.snapshot, &ProjectionSelection::All)
        .unwrap();
    assert!(matches!(&remote, ProjectionVersion::Remote { .. }));
    assert_eq!(
        remote,
        b.record_version_observation_at_generation(1, &state.snapshot, &ProjectionSelection::All)
            .unwrap()
    );
    for store in [&a, &b] {
        fs::write(store.checkout_root().join("a.txt"), b"same current bytes").unwrap();
    }
    let preview = a
        .preview_restore(
            1,
            &RestoreSource::Version {
                version: remote,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    let usage = family.usage().unwrap();
    assert!(
        b.apply_restore(&preview)
            .unwrap_err()
            .to_string()
            .contains("destination")
    );
    let mut native_mismatch = preview.clone();
    native_mismatch.destination.as_mut().unwrap().identity = b.identity().clone();
    reviewed_digest(&mut native_mismatch);
    assert!(
        b.apply_restore(&native_mismatch)
            .unwrap_err()
            .to_string()
            .contains("destination")
    );
    let mut unbound = preview.clone();
    unbound.destination = None;
    reviewed_digest(&mut unbound);
    assert!(
        a.apply_restore(&unbound)
            .unwrap_err()
            .to_string()
            .contains("unbound")
    );
    assert_eq!(family.usage().unwrap(), usage);
    let receipt = a.apply_restore(&preview).unwrap();
    assert_eq!(a.apply_restore(&preview).unwrap(), receipt);
    assert!(
        b.apply_restore(&preview)
            .unwrap_err()
            .to_string()
            .contains("destination")
    );
    assert_eq!(
        fs::read(b.checkout_root().join("a.txt")).unwrap(),
        b"same current bytes"
    );
    assert!(b.captured_patch(1).is_err());
}

struct StopRestore;
impl RestoreFaults for StopRestore {
    fn check(&self, p: RestorePoint) -> io::Result<()> {
        if p == RestorePoint::JournalDurable {
            Err(io::Error::other("durable legacy fixture"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn imported_restore_history_keeps_source_binding_and_inverse_uses_new_destination() {
    let f = Fixture::new();
    let family = family(&f);
    let source_root = f.0.join("source");
    fs::create_dir(&source_root).unwrap();
    let (before, after) = hydration();
    let source = ProjectionStore::create(&source_root, identity(), before).unwrap();
    source.install_projection(0, &after, &mut provide).unwrap();
    fs::write(source_root.join("a.txt"), b"captured target").unwrap();
    let capture = source.capture_patch(1).unwrap();
    fs::write(source_root.join("a.txt"), b"retained preimage").unwrap();
    let preview = source
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: capture.sequence,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    let receipt = source.apply_restore(&preview).unwrap();
    let source_db = source_root.join(".layrs/metadata/projection.sqlite3");
    let source_bytes = fs::read(&source_db).unwrap();
    let destination = f.0.join("destination");
    fs::create_dir(&destination).unwrap();
    let (converted, _) = source
        .convert_into_family(
            &family,
            &destination,
            WorktreeId::parse("worktree:converted").unwrap(),
        )
        .unwrap();
    assert_eq!(converted.fsck().unwrap().restore_operations, 1);
    assert!(
        converted
            .apply_restore(&preview)
            .unwrap_err()
            .to_string()
            .contains("destination")
    );
    let inverse = converted
        .preview_restore(
            1,
            &RestoreSource::Inverse {
                operation_id: receipt.operation_id,
            },
        )
        .unwrap();
    assert_eq!(
        &inverse.destination.as_ref().unwrap().identity,
        converted.identity()
    );
    converted.apply_restore(&inverse).unwrap();
    assert_eq!(
        fs::read(destination.join("a.txt")).unwrap(),
        b"retained preimage"
    );
    r2::assert_unchanged_database(&fs::read(source_db).unwrap(), &source_bytes);
}

#[test]
fn legacy_unbound_durable_journal_recovers_but_its_approval_cannot_be_reapplied() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", true);
    fs::write(store.checkout_root().join("a.txt"), b"legacy preimage").unwrap();
    let mut preview = store
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
            .apply_restore_with_faults(&preview, &StopRestore)
            .is_err()
    );
    // Model an already committed pre-upgrade journal, preserving its canonical
    // payload/digest and native stage witnesses. This is not a public import.
    preview.destination = None;
    reviewed_digest(&mut preview);
    {
        let access = format::access(&store, SessionMode::Exclusive).unwrap();
        let bytes: Vec<u8> = access
            .connection
            .query_row("SELECT payload FROM projection_restore_pending", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut journal: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        journal["preview"] = serde_json::to_value(&preview).unwrap();
        access
            .connection
            .execute(
                "UPDATE projection_restore_pending SET payload=?1",
                [layrs_core::scm::canonical_json(&journal).unwrap()],
            )
            .unwrap();
    }
    let receipt = ProjectionStore::resume_restore(store.checkout_root()).unwrap();
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"alpha"
    );
    assert!(
        store
            .apply_restore(&preview)
            .unwrap_err()
            .to_string()
            .contains("unbound")
    );
    let inverse = store
        .preview_restore(
            1,
            &RestoreSource::Inverse {
                operation_id: receipt.operation_id,
            },
        )
        .unwrap();
    assert!(inverse.destination.is_some());
    store.apply_restore(&inverse).unwrap();
    assert_eq!(
        fs::read(store.checkout_root().join("a.txt")).unwrap(),
        b"legacy preimage"
    );
    store.fsck().unwrap();
}

#[test]
fn more_than_1024_preparation_cycles_keep_history_bounded_and_exact_active_lookup_available() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let req = request(&store, "a.txt");
    let active = store.begin_preparation(&req).unwrap();
    let first = store.begin_preparation(&req).unwrap();
    store.release_preparation(&first.id).unwrap();
    for _ in 0..1025 {
        let p = store.begin_preparation(&req).unwrap();
        store.release_preparation(&p.id).unwrap();
    }
    assert!(store.preparation(&first.id).unwrap().is_none());
    assert_eq!(store.preparation(&active.id).unwrap(), Some(active.clone()));
    let rows = store.preparations().unwrap();
    assert_eq!(rows.iter().filter(|p| p.status == "released").count(), 64);
    assert_eq!(rows.iter().filter(|p| p.status != "released").count(), 1);
    {
        let access = format::access(&store, SessionMode::Shared).unwrap();
        let count: u64 = access
            .connection
            .query_row("SELECT COUNT(*) FROM projection_preparations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 65);
    }
    store
        .resume_preparation(&active.id, &mut |_, _, out| out.write_all(b"alpha"))
        .unwrap();
    store.release_preparation(&active.id).unwrap();
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    family.evict_unretained().unwrap();
    store.fsck().unwrap();
}

#[test]
fn prior_oversized_active_history_has_exact_lookup_and_new_intents_fail_before_reservation() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let req = request(&store, "a.txt");
    // Compatibility fixture: a draft writer could exceed the new active bound.
    {
        let mut access = format::access(&store, SessionMode::Exclusive).unwrap();
        let tx = access.connection.transaction().unwrap();
        for n in 0..1025 {
            let p = OfflinePreparation {
                id: format!("legacy-{n:04}"),
                request: req.clone(),
                status: "acquiring".into(),
            };
            tx.execute(
                "INSERT INTO projection_preparations VALUES(?1,?2,'acquiring',NULL,?3)",
                rusqlite::params![
                    p.id,
                    layrs_core::scm::canonical_json(&p).unwrap(),
                    format!("missing-legacy-reservation-{n}")
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    assert_eq!(store.preparations().unwrap().len(), 1024);
    assert_eq!(
        store.preparation("legacy-0000").unwrap().unwrap().id,
        "legacy-0000"
    );
    let usage = family.usage().unwrap();
    assert!(
        store
            .begin_preparation(&req)
            .unwrap_err()
            .to_string()
            .contains("active preparation limit")
    );
    assert_eq!(family.usage().unwrap(), usage);
    store.release_preparation("legacy-0000").unwrap();
    store.release_preparation("legacy-0001").unwrap();
    store.begin_preparation(&req).unwrap();
}

struct StopCreation;
impl CreationFaults for StopCreation {
    fn check(&self, p: CreationPoint) -> io::Result<()> {
        if p == CreationPoint::MarkerDurable {
            Err(io::Error::other("marker durable"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn initializing_hot_owner_is_discovered_without_writes_and_resumes_from_private_bootstrap() {
    let f = Fixture::new();
    let family = family(&f);
    let root = f.0.join("a");
    fs::create_dir(&root).unwrap();
    let id = WorktreeId::parse("worktree:a").unwrap();
    assert!(
        family
            .create_worktree_with_faults(
                &root,
                id.clone(),
                hydration().0,
                ProjectionSelection::All,
                &StopCreation
            )
            .is_err()
    );
    let db = family.member_metadata(&id).join("projection.sqlite3");
    r2::kill_spilled_writer(&db, "owner", &f.0.join("ready"));
    let spilled = fs::read(&db).unwrap();
    let inspection = ProjectionStore::inspect_recovery(&root).unwrap();
    assert_eq!(
        inspection.recovery,
        vec![
            ProjectionRecoveryReason::Initializing,
            ProjectionRecoveryReason::WorktreeRollback
        ]
    );
    assert!(ProjectionStore::resume_initialization(&root, &inspection.binding).is_err());
    r2::assert_unchanged_database(&fs::read(&db).unwrap(), &spilled);
    let recovered = ProjectionStore::recover_databases(&root, &inspection.binding).unwrap();
    assert_eq!(
        recovered.recovery,
        vec![ProjectionRecoveryReason::Initializing]
    );
    let before_resume = fs::read(&db).unwrap();
    assert_eq!(ProjectionStore::inspect_recovery(&root).unwrap(), recovered);
    r2::assert_unchanged_database(&fs::read(&db).unwrap(), &before_resume);
    assert!(
        ProjectionStore::resume_initialization(&root, &inspection.binding)
            .unwrap()
            .recovery
            .is_empty()
    );
    assert!(
        ProjectionStore::resume_initialization(&root, &inspection.binding)
            .unwrap()
            .recovery
            .is_empty()
    );
    let store = ProjectionStore::open(&root).unwrap();
    assert_eq!(store.state().unwrap().generation, 0);
    assert_eq!(store.state().unwrap().snapshot, hydration().0);
}

#[test]
fn unready_conversion_hot_owner_is_readonly_discoverable_and_abandon_keeps_source_and_files() {
    let f = Fixture::new();
    let family = family(&f);
    let source_root = f.0.join("source");
    let destination = f.0.join("destination");
    fs::create_dir(&source_root).unwrap();
    fs::create_dir(&destination).unwrap();
    let (before, after) = hydration();
    let source = ProjectionStore::create(&source_root, identity(), before).unwrap();
    source.install_projection(0, &after, &mut provide).unwrap();
    let source_db = source_root.join(".layrs/metadata/projection.sqlite3");
    let source_bytes = fs::read(&source_db).unwrap();
    let id = WorktreeId::parse("worktree:a").unwrap();
    assert!(
        source
            .convert_into_family_with_faults(
                &family,
                &destination,
                id.clone(),
                &ConversionStop(ConversionPoint::CheckoutCopied)
            )
            .is_err()
    );
    let db = family.member_metadata(&id).join("projection.sqlite3");
    r2::kill_spilled_writer(&db, "owner", &f.0.join("ready"));
    let spilled = fs::read(&db).unwrap();
    let inspection = ProjectionStore::inspect_recovery(&destination).unwrap();
    assert!(
        inspection
            .recovery
            .contains(&ProjectionRecoveryReason::WorktreeRollback)
    );
    r2::assert_unchanged_database(&fs::read(&db).unwrap(), &spilled);
    let recovered = ProjectionStore::recover_databases(&destination, &inspection.binding).unwrap();
    assert_eq!(
        recovered.recovery,
        vec![
            ProjectionRecoveryReason::Initializing,
            ProjectionRecoveryReason::Conversion
        ]
    );
    assert!(ProjectionStore::resume_initialization(&destination, &inspection.binding).is_err());
    family.abandon_conversion(&destination).unwrap();
    r2::assert_unchanged_database(&fs::read(&source_db).unwrap(), &source_bytes);
    assert_eq!(fs::read(source_root.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"alpha");
}

#[test]
fn discovery_rejects_wrong_private_binding_before_classifying_family_rollback() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "a", false);
    let binding = ProjectionStore::inspect_recovery(store.checkout_root())
        .unwrap()
        .binding;
    let catalog = family.root().join("metadata/catalog.sqlite3");
    r2::kill_spilled_writer(&catalog, "catalog", &f.0.join("ready"));
    let spilled = fs::read(&catalog).unwrap();
    let db = family
        .member_metadata(&store.identity().worktree_id)
        .join("projection.sqlite3");
    let retained = db.with_extension("retained");
    fs::rename(&db, &retained).unwrap();
    fs::copy(&retained, &db).unwrap();
    assert!(ProjectionStore::inspect_recovery(store.checkout_root()).is_err());
    assert!(ProjectionStore::recover_databases(store.checkout_root(), &binding).is_err());
    r2::assert_unchanged_database(&fs::read(&catalog).unwrap(), &spilled);
}

#[test]
fn conversion_eligibility_reports_pending_filesystem_and_publication_without_mutation() {
    let (f, store) = super::super::capture::ready(false);
    let clean = ProjectionStore::inspect_recovery(&f.0).unwrap();
    assert!(clean.conversion_eligible);
    fs::write(f.0.join("a.txt"), b"accepted").unwrap();
    let capture = store.capture_patch(1).unwrap();
    store
        .begin_publication(capture.sequence, 1, br#"{"request":"pending"}"#)
        .unwrap();
    let db = f.0.join(".layrs/metadata/projection.sqlite3");
    let before = fs::read(&db).unwrap();
    let inspection = ProjectionStore::inspect_recovery(&f.0).unwrap();
    assert_eq!(
        inspection.recovery,
        vec![ProjectionRecoveryReason::Publication]
    );
    assert_eq!(
        inspection.conversion_blocker.as_deref(),
        Some("pending_publication")
    );
    assert!(!inspection.conversion_eligible);
    r2::assert_unchanged_database(&fs::read(&db).unwrap(), &before);

    let (g, second) = super::super::capture::ready(false);
    fs::write(g.0.join("a.txt"), b"capture").unwrap();
    let capture = second.capture_patch(1).unwrap();
    fs::write(g.0.join("a.txt"), b"preimage").unwrap();
    let preview = second
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: capture.sequence,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    assert!(
        second
            .apply_restore_with_faults(&preview, &StopRestore)
            .is_err()
    );
    let db = g.0.join(".layrs/metadata/projection.sqlite3");
    let before = fs::read(&db).unwrap();
    let inspection = ProjectionStore::inspect_recovery(&g.0).unwrap();
    assert_eq!(
        inspection.recovery,
        vec![ProjectionRecoveryReason::FilesystemOperation]
    );
    assert_eq!(
        inspection.conversion_blocker.as_deref(),
        Some("pending_filesystem_operation")
    );
    assert!(!inspection.conversion_eligible);
    r2::assert_unchanged_database(&fs::read(&db).unwrap(), &before);
}

#[test]
fn unsupported_legacy_schema_discovery_and_explicit_recovery_do_not_migrate() {
    let f = Fixture::new();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    let binding = ProjectionStore::inspect_recovery(store.checkout_root())
        .unwrap()
        .binding;
    let db = f.0.join(".layrs/metadata/projection.sqlite3");
    let c = database::connect(&db, true).unwrap();
    c.execute_batch("PRAGMA user_version=5;").unwrap();
    drop(c);
    let before = fs::read(&db).unwrap();
    assert!(ProjectionStore::inspect_recovery(store.checkout_root()).is_err());
    assert!(ProjectionStore::recover_databases(store.checkout_root(), &binding).is_err());
    r2::assert_unchanged_database(&fs::read(&db).unwrap(), &before);
}

#[cfg(windows)]
#[test]
fn discovery_accepts_case_alias_only_for_the_same_pinned_checkout() {
    let f = Fixture::new();
    let family = family(&f);
    let store = member(&f, &family, "case-root", false);
    let alias = store.checkout_root().to_string_lossy().to_uppercase();
    assert_eq!(
        ProjectionStore::inspect_recovery(&alias).unwrap(),
        ProjectionStore::inspect_recovery(store.checkout_root()).unwrap()
    );
}
