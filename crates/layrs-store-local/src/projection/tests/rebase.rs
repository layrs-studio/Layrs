use super::capture::ready;
use super::*;
use std::collections::BTreeMap;

fn plan() -> (Fixture, ProjectionStore, RebasePreview) {
    let (fixture, store) = ready(false);
    fs::write(fixture.0.join("a.txt"), b"local merged bytes").unwrap();
    let captured = store.capture_patch(1).unwrap();
    let state = store.state().unwrap();
    let mut target = state.snapshot.clone();
    target.base_step_id = StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap();
    target.base_root_tree_id = TreeId::parse(format!("blake3:{}", "3".repeat(64))).unwrap();
    target
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "a.txt")
        .unwrap()
        .target = Some(TreeEntryTarget::file(FileId::from_bytes(b"remote bytes")));
    let mut resolutions = BTreeMap::new();
    resolutions.insert(
        RepoPath::parse("a.txt").unwrap(),
        Some(CapturedFile {
            file_id: FileId::from_bytes(b"resolved content"),
            executable: false,
            bytes: 16,
        }),
    );
    resolutions.insert(
        RepoPath::parse("nested/b.txt").unwrap(),
        Some(CapturedFile {
            file_id: FileId::from_bytes(b"beta"),
            executable: false,
            bytes: 4,
        }),
    );
    let preview = store
        .preview_rebase(1, captured.sequence, &target, &resolutions)
        .unwrap()
        .with_operation_id("123-456")
        .unwrap();
    (fixture, store, preview)
}
fn provide(path: &str, out: &mut dyn io::Write) -> io::Result<()> {
    assert_eq!(path, "a.txt");
    out.write_all(b"resolved content")
}
#[test]
fn rebase_refuses_late_equal_replacement_and_recreated_deletion() {
    struct Edit {
        root: std::path::PathBuf,
        deletion: bool,
    }
    impl RebaseFaults for Edit {
        fn check(&self, point: RebasePoint) -> io::Result<()> {
            if point == RebasePoint::BeforeCommit {
                if !self.deletion {
                    fs::rename(
                        self.root.join("a.txt"),
                        self.root.join("retained-installed.txt"),
                    )?;
                }
                fs::write(
                    self.root.join("a.txt"),
                    if self.deletion {
                        b"external recreation".as_slice()
                    } else {
                        b"resolved content".as_slice()
                    },
                )?;
            }
            Ok(())
        }
    }
    for deletion in [false, true] {
        let (fixture, store, mut preview) = plan();
        if deletion {
            let mut resolutions = preview.resolutions.clone();
            resolutions.insert(RepoPath::parse("a.txt").unwrap(), None);
            preview = store
                .preview_rebase(1, preview.capture_sequence, &preview.target, &resolutions)
                .unwrap();
        }
        assert!(
            store
                .apply_rebase_with_faults(
                    &preview,
                    &mut provide,
                    &Edit {
                        root: fixture.0.clone(),
                        deletion
                    }
                )
                .is_err()
        );
        let expected = if deletion {
            b"external recreation".as_slice()
        } else {
            b"resolved content".as_slice()
        };
        assert_eq!(fs::read(fixture.0.join("a.txt")).unwrap(), expected);
        let c = rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3"))
            .unwrap();
        assert_eq!(
            c.query_row("SELECT generation FROM projection_head", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM projection_rebase_pending", [], |r| {
                r.get::<_, u64>(0)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM projection_rebase_history", [], |r| {
                r.get::<_, u64>(0)
            })
            .unwrap(),
            0
        );
        assert!(ProjectionStore::resume_rebase(&fixture.0).is_err());
    }
}
struct Stop(RebasePoint);
impl RebaseFaults for Stop {
    fn check(&self, point: RebasePoint) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other("rebase interrupted"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn rebase_keeps_resolved_local_bytes_against_exact_new_remote_base() {
    let (fixture, store, preview) = plan();
    let original = store.captured_patch(preview.capture_sequence).unwrap();
    let captured = store.apply_rebase(&preview, &mut provide).unwrap();
    assert_eq!(
        fs::read(fixture.0.join("a.txt")).unwrap(),
        b"resolved content"
    );
    assert_eq!(captured.projection_generation, 2);
    assert_eq!(captured.changes.len(), 1);
    assert_eq!(
        captured.changes[0].expected.as_ref().unwrap().file_id,
        FileId::from_bytes(b"remote bytes")
    );
    assert_eq!(
        captured.changes[0].replacement.as_ref().unwrap().file_id,
        FileId::from_bytes(b"resolved content")
    );
    assert_eq!(
        store.captured_patch(preview.capture_sequence).unwrap(),
        original
    );
    assert_eq!(store.state().unwrap().snapshot, preview.target);
    assert_eq!(
        store.completed_rebase("123-456").unwrap(),
        Some(captured.clone())
    );
    assert_eq!(
        store
            .apply_rebase(&preview, &mut |_, _| panic!("replay fetched bytes"))
            .unwrap(),
        captured
    );
    assert_eq!(store.fsck().unwrap().rebase_operations, 1);
}

#[test]
fn rebase_recovers_each_durable_boundary() {
    for point in [
        RebasePoint::JournalDurable,
        RebasePoint::PathQuarantined,
        RebasePoint::PathInstalled,
        RebasePoint::BeforeCommit,
        RebasePoint::AfterCommit,
    ] {
        let (fixture, store, preview) = plan();
        assert!(
            store
                .apply_rebase_with_faults(&preview, &mut provide, &Stop(point))
                .is_err()
        );
        let captured = if point == RebasePoint::AfterCommit {
            store
                .completed_rebase(&preview.operation_id)
                .unwrap()
                .unwrap()
        } else {
            assert!(ProjectionStore::open(&fixture.0).is_err());
            ProjectionStore::resume_rebase(&fixture.0).unwrap()
        };
        assert_eq!(
            fs::read(fixture.0.join("a.txt")).unwrap(),
            b"resolved content"
        );
        assert_eq!(
            store
                .apply_rebase(&preview, &mut |_, _| panic!("replay fetched bytes"))
                .unwrap(),
            captured
        );
        assert_eq!(store.fsck().unwrap().rebase_operations, 1);
    }
}

#[test]
fn rebase_rejects_stale_and_protects_external_bytes() {
    let (fixture, store, preview) = plan();
    fs::write(fixture.0.join("a.txt"), b"new edit").unwrap();
    assert!(store.apply_rebase(&preview, &mut provide).is_err());
    assert_eq!(fs::read(fixture.0.join("a.txt")).unwrap(), b"new edit");
    assert_eq!(store.state().unwrap().generation, 1);
}

#[test]
fn incomplete_staging_can_retry_same_operation_id() {
    for point in [RebasePoint::IntentDurable, RebasePoint::Staged] {
        let (fixture, store, preview) = plan();
        assert!(
            store
                .apply_rebase_with_faults(&preview, &mut provide, &Stop(point))
                .is_err()
        );
        assert!(ProjectionStore::resume_rebase(&fixture.0).is_err());
        assert_eq!(
            fs::read(fixture.0.join("a.txt")).unwrap(),
            b"local merged bytes"
        );
        let captured = store.apply_rebase(&preview, &mut provide).unwrap();
        assert_eq!(
            store.completed_rebase(&preview.operation_id).unwrap(),
            Some(captured)
        );
    }
}

#[test]
fn rebase_recovery_refuses_collision_and_corrupt_staging() {
    let (fixture, store, preview) = plan();
    assert!(
        store
            .apply_rebase_with_faults(&preview, &mut provide, &Stop(RebasePoint::PathQuarantined))
            .is_err()
    );
    fs::write(fixture.0.join("a.txt"), b"external bytes").unwrap();
    assert!(ProjectionStore::resume_rebase(&fixture.0).is_err());
    assert_eq!(
        fs::read(fixture.0.join("a.txt")).unwrap(),
        b"external bytes"
    );
    assert!(
        store
            .completed_rebase(&preview.operation_id)
            .unwrap()
            .is_none()
    );

    let (fixture, store, preview) = plan();
    assert!(
        store
            .apply_rebase_with_faults(&preview, &mut provide, &Stop(RebasePoint::JournalDurable))
            .is_err()
    );
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    let bytes: Vec<u8> = db
        .query_row("SELECT payload FROM projection_rebase_pending", [], |r| {
            r.get(0)
        })
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let directory = value["directory"].as_str().unwrap();
    fs::write(
        fixture
            .0
            .join(".layrs/projection-quarantine")
            .join(directory)
            .join("new-00000000"),
        b"tampered",
    )
    .unwrap();
    assert!(ProjectionStore::resume_rebase(&fixture.0).is_err());
    assert_eq!(
        fs::read(fixture.0.join("a.txt")).unwrap(),
        b"local merged bytes"
    );
}

#[test]
fn rebase_refuses_protected_collision_and_migrates_schema_four_history() {
    let (fixture, store, preview) = plan();
    fs::write(fixture.0.join("private.txt"), b"external protected bytes").unwrap();
    assert!(store.apply_rebase(&preview, &mut provide).is_err());
    assert_eq!(
        fs::read(fixture.0.join("private.txt")).unwrap(),
        b"external protected bytes"
    );
    assert_eq!(store.state().unwrap().generation, 1);
    fs::remove_file(fixture.0.join("private.txt")).unwrap();
    let captured = store.apply_rebase(&preview, &mut provide).unwrap();
    assert_eq!(store.fsck().unwrap().rebase_operations, 1);
    assert_eq!(
        store.completed_rebase(&preview.operation_id).unwrap(),
        Some(captured)
    );

    let (fixture, store) = ready(false);
    fs::write(fixture.0.join("a.txt"), b"schema four local").unwrap();
    let captured = store.capture_patch(1).unwrap();
    drop(store);
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute_batch("DROP TABLE projection_selections; DROP TABLE projection_rebase_pending; DROP TABLE projection_rebase_history; DROP TABLE projection_rebase_previews; PRAGMA user_version=4;").unwrap();
    drop(db);
    let reopened = ProjectionStore::open(&fixture.0).unwrap();
    assert_eq!(
        reopened.captured_patch(captured.sequence).unwrap(),
        captured
    );
    assert_eq!(reopened.fsck().unwrap().rebase_operations, 0);
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        6
    );
}

#[test]
fn disjoint_remote_and_local_edits_remain_base_relative() {
    let (fixture, store) = ready(false);
    fs::write(fixture.0.join("a.txt"), b"my local edit").unwrap();
    let old_capture = store.capture_patch(1).unwrap();
    let mut target = store.state().unwrap().snapshot;
    target.base_step_id = StepId::parse(format!("blake3:{}", "4".repeat(64))).unwrap();
    target.base_root_tree_id = TreeId::parse(format!("blake3:{}", "5".repeat(64))).unwrap();
    target
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "nested/b.txt")
        .unwrap()
        .target = Some(TreeEntryTarget::file(FileId::from_bytes(b"remote beta")));
    let resolutions = BTreeMap::from([
        (
            RepoPath::parse("a.txt").unwrap(),
            Some(CapturedFile {
                file_id: FileId::from_bytes(b"my local edit"),
                executable: false,
                bytes: 13,
            }),
        ),
        (
            RepoPath::parse("nested/b.txt").unwrap(),
            Some(CapturedFile {
                file_id: FileId::from_bytes(b"remote beta"),
                executable: false,
                bytes: 11,
            }),
        ),
    ]);
    let preview = store
        .preview_rebase(1, old_capture.sequence, &target, &resolutions)
        .unwrap();
    let captured = store
        .apply_rebase(&preview, &mut |path, out| {
            assert_eq!(path, "nested/b.txt");
            out.write_all(b"remote beta")
        })
        .unwrap();
    assert_eq!(fs::read(fixture.0.join("a.txt")).unwrap(), b"my local edit");
    assert_eq!(
        fs::read(fixture.0.join("nested/b.txt")).unwrap(),
        b"remote beta"
    );
    assert_eq!(captured.changes.len(), 1);
    assert_eq!(captured.changes[0].path.as_str(), "a.txt");
    assert_eq!(store.fsck().unwrap().rebase_operations, 1);
}

#[test]
fn durable_merge_draft_survives_reopen_but_grants_no_stale_apply_authority() {
    let (fixture, store, preview) = plan();
    let document = br#"{"choice":"keepLocal"}"#;
    store
        .save_rebase_draft(
            preview.projection_generation,
            &preview.projection_digest,
            preview.capture_sequence,
            document,
        )
        .unwrap();
    let reopened = ProjectionStore::open(&fixture.0).unwrap();
    assert_eq!(reopened.rebase_draft().unwrap(), Some(document.to_vec()));
    fs::write(fixture.0.join("a.txt"), b"edit after draft").unwrap();
    let later = reopened.capture_patch(1).unwrap();
    assert!(later.sequence > preview.capture_sequence);
    assert_eq!(reopened.rebase_draft().unwrap(), Some(document.to_vec()));
    assert!(reopened.apply_rebase(&preview, &mut provide).is_err());
    assert_eq!(
        fs::read(fixture.0.join("a.txt")).unwrap(),
        b"edit after draft"
    );
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute("UPDATE projection_rebase_previews SET payload=X'00'", [])
        .unwrap();
    assert!(reopened.rebase_draft().is_err());
    assert!(reopened.fsck().is_err());
}

#[test]
fn rebase_handles_explicit_regular_path_addition_and_deletion() {
    let (fixture, store) = ready(false);
    let source = store.capture_patch(1).unwrap();
    let mut target = store.state().unwrap().snapshot;
    target.base_step_id = StepId::parse(format!("blake3:{}", "6".repeat(64))).unwrap();
    target.base_root_tree_id = TreeId::parse(format!("blake3:{}", "7".repeat(64))).unwrap();
    let deleted = target
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "a.txt")
        .unwrap();
    deleted.state = EntryState::Absent;
    deleted.target = None;
    target.entries.push(ProjectionEntry {
        path: RepoPath::parse("new.txt").unwrap(),
        target: Some(TreeEntryTarget::file(FileId::from_bytes(b"new remote"))),
        executable: false,
        writable: true,
        state: EntryState::Materialized,
    });
    target.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let resolutions = BTreeMap::from([
        (RepoPath::parse("a.txt").unwrap(), None),
        (
            RepoPath::parse("nested/b.txt").unwrap(),
            Some(CapturedFile {
                file_id: FileId::from_bytes(b"beta"),
                executable: false,
                bytes: 4,
            }),
        ),
        (
            RepoPath::parse("new.txt").unwrap(),
            Some(CapturedFile {
                file_id: FileId::from_bytes(b"new remote"),
                executable: false,
                bytes: 10,
            }),
        ),
    ]);
    let preview = store
        .preview_rebase(1, source.sequence, &target, &resolutions)
        .unwrap();
    let captured = store
        .apply_rebase(&preview, &mut |path, out| {
            assert_eq!(path, "new.txt");
            out.write_all(b"new remote")
        })
        .unwrap();
    assert!(!fixture.0.join("a.txt").exists());
    assert_eq!(fs::read(fixture.0.join("new.txt")).unwrap(), b"new remote");
    assert!(captured.changes.is_empty());
    assert_eq!(store.fsck().unwrap().rebase_operations, 1);
}

#[test]
fn immutable_rebase_history_rejects_capture_rebinding() {
    let (fixture, store, preview) = plan();
    store.apply_rebase(&preview, &mut provide).unwrap();
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute(
        "UPDATE projection_rebase_history SET capture_sequence=?1 WHERE operation_id=?2",
        rusqlite::params![preview.capture_sequence, preview.operation_id],
    )
    .unwrap();
    assert!(store.completed_rebase(&preview.operation_id).is_err());
    assert!(store.fsck().is_err());
}
