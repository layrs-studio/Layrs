use super::capture::ready;
use super::*;
fn setup() -> (Fixture, ProjectionStore, CapturedPatch, RestorePreview) {
    let (f, store) = ready(false);
    fs::write(f.0.join("a.txt"), b"first capture").unwrap();
    let captured = store.capture_patch(1).unwrap();
    fs::write(f.0.join("a.txt"), b"current unsaved bytes").unwrap();
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: captured.sequence,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    (f, store, captured, preview)
}
struct Stop(RestorePoint);
impl RestoreFaults for Stop {
    fn check(&self, point: RestorePoint) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other("restore interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn selected_restore_preserves_base_and_bytes_and_supports_exact_undo_redo() {
    let (f, store, captured, preview) = setup();
    let base = store.state().unwrap();
    let receipt = store.apply_restore(&preview).unwrap();
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"first capture");
    assert_eq!(store.state().unwrap(), base);
    assert_eq!(store.apply_restore(&preview).unwrap(), receipt);
    assert_eq!(store.captured_patch(captured.sequence).unwrap(), captured);
    let undo = store
        .preview_restore(
            1,
            &RestoreSource::Inverse {
                operation_id: receipt.operation_id,
            },
        )
        .unwrap();
    let undone = store.apply_restore(&undo).unwrap();
    assert_eq!(
        fs::read(f.0.join("a.txt")).unwrap(),
        b"current unsaved bytes"
    );
    let redo = store
        .preview_restore(
            1,
            &RestoreSource::Inverse {
                operation_id: undone.operation_id,
            },
        )
        .unwrap();
    store.apply_restore(&redo).unwrap();
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"first capture");
    assert_eq!(store.fsck().unwrap().restore_operations, 3);
}
#[test]
fn restore_recovers_every_durable_boundary_without_losing_original_bytes() {
    for point in [
        RestorePoint::Staged,
        RestorePoint::JournalDurable,
        RestorePoint::PathQuarantined,
        RestorePoint::PathInstalled,
        RestorePoint::BeforeCommit,
        RestorePoint::AfterCommit,
    ] {
        let (f, store, _, preview) = setup();
        assert!(
            store
                .apply_restore_with_faults(&preview, &Stop(point))
                .is_err()
        );
        let receipt = if matches!(point, RestorePoint::Staged | RestorePoint::AfterCommit) {
            store.apply_restore(&preview).unwrap()
        } else {
            assert!(ProjectionStore::open(&f.0).is_err());
            ProjectionStore::resume_restore(&f.0).unwrap()
        };
        let store = ProjectionStore::open(&f.0).unwrap();
        assert_eq!(store.apply_restore(&preview).unwrap(), receipt);
        let inverse = store
            .preview_restore(
                1,
                &RestoreSource::Inverse {
                    operation_id: receipt.operation_id,
                },
            )
            .unwrap();
        store.apply_restore(&inverse).unwrap();
        assert_eq!(
            fs::read(f.0.join("a.txt")).unwrap(),
            b"current unsaved bytes"
        );
        assert!(store.fsck().is_ok());
    }
}

#[test]
fn installation_recovery_rejects_a_restore_journal_before_creating_a_file() {
    let f = Fixture::new();
    let (initial, target) = hydration();
    let store = ProjectionStore::create(&f.0, identity(), initial).unwrap();
    assert!(
        store
            .install_projection_with_faults(
                0,
                &target,
                &mut provide,
                &StopInstall(InstallPoint::JournalDurable)
            )
            .is_err()
    );
    let db = rusqlite::Connection::open(f.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute(
        "INSERT INTO projection_restore_pending VALUES(1,?1)",
        [b"{}".as_slice()],
    )
    .unwrap();
    drop(db);
    assert!(ProjectionStore::resume_installation(&f.0).is_err());
    assert!(!f.0.join("a.txt").exists());
}
#[test]
fn restore_uses_current_permissions_and_refuses_stale_checkout_or_generation() {
    let (f, store, captured, preview) = setup();
    fs::write(f.0.join("a.txt"), b"changed after preview").unwrap();
    assert!(store.apply_restore(&preview).is_err());
    assert_eq!(
        fs::read(f.0.join("a.txt")).unwrap(),
        b"changed after preview"
    );
    let mut revoked = store.state().unwrap().snapshot;
    revoked
        .entries
        .iter_mut()
        .find(|entry| entry.path.as_str() == "a.txt")
        .unwrap()
        .writable = false;
    revoked.observed_authorization_epoch += 1;
    fs::write(f.0.join("a.txt"), b"alpha").unwrap();
    store
        .refresh_projection(1, &revoked, &mut |_, _| {
            panic!("permission-only refresh must not fetch")
        })
        .unwrap();
    let source = RestoreSource::Capture {
        sequence: captured.sequence,
        paths: vec![RepoPath::parse("a.txt").unwrap()],
    };
    assert!(store.preview_restore(2, &source).is_err());
    assert!(store.apply_restore(&preview).is_err());
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
}
#[test]
fn deletion_restore_and_inverse_keep_explicit_absence_and_history() {
    let (f, store) = ready(false);
    fs::remove_file(f.0.join("a.txt")).unwrap();
    let deletion = store.capture_patch(1).unwrap();
    fs::write(f.0.join("a.txt"), b"keep these bytes").unwrap();
    let preview = store
        .preview_restore(
            1,
            &RestoreSource::Capture {
                sequence: deletion.sequence,
                paths: vec![RepoPath::parse("a.txt").unwrap()],
            },
        )
        .unwrap();
    let receipt = store.apply_restore(&preview).unwrap();
    assert!(!f.0.join("a.txt").exists());
    let undo = store
        .preview_restore(
            1,
            &RestoreSource::Inverse {
                operation_id: receipt.operation_id,
            },
        )
        .unwrap();
    store.apply_restore(&undo).unwrap();
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"keep these bytes");
    assert!(store.fsck().is_ok());
}
#[test]
fn restore_recovery_preserves_external_collision_and_replaced_stage_identity() {
    for collision in [true, false] {
        let (f, store, _, preview) = setup();
        let point = if collision {
            RestorePoint::PathQuarantined
        } else {
            RestorePoint::PathInstalled
        };
        assert!(
            store
                .apply_restore_with_faults(&preview, &Stop(point))
                .is_err()
        );
        if !collision {
            fs::rename(f.0.join("a.txt"), f.0.join("saved-original.txt")).unwrap();
        }
        fs::write(
            f.0.join("a.txt"),
            if collision {
                b"external".as_slice()
            } else {
                b"first capture"
            },
        )
        .unwrap();
        assert!(ProjectionStore::resume_restore(&f.0).is_err());
        assert_eq!(
            fs::read(f.0.join("a.txt")).unwrap(),
            if collision {
                b"external".as_slice()
            } else {
                b"first capture"
            }
        );
        assert!(ProjectionStore::open(&f.0).is_err());
    }
}
