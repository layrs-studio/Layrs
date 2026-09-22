use super::capture::ready;
use super::*;
fn destination(store: &ProjectionStore, path: &str) -> VersionedProjection {
    let state = store.state().unwrap();
    store
        .observe_absent_paths(
            state.generation,
            &state.digest,
            &[RepoPath::parse(path).unwrap()],
        )
        .unwrap()
}
#[test]
fn exact_absence_create_and_move_keep_remote_base_and_durable_captures() {
    let (f, store) = ready(false);
    let before = store.state().unwrap();
    assert!(
        store
            .preview_namespace(
                1,
                &NamespaceMutation::Create {
                    destination: RepoPath::parse("unknown").unwrap()
                }
            )
            .is_err()
    );
    let state = destination(&store, "new.txt");
    assert_eq!(state.generation, 2);
    assert!(!f.0.join("new.txt").exists());
    let create = NamespaceMutation::Create {
        destination: RepoPath::parse("new.txt").unwrap(),
    };
    let preview = store.preview_namespace(2, &create).unwrap();
    let capture = store.apply_namespace(&preview, Some(b"created")).unwrap();
    assert_eq!(fs::read(f.0.join("new.txt")).unwrap(), b"created");
    assert_eq!(capture.changes.len(), 1);
    assert!(capture.changes[0].expected.is_none());
    assert_eq!(
        store.apply_namespace(&preview, Some(b"created")).unwrap(),
        capture
    );
    assert!(store.apply_namespace(&preview, Some(b"different")).is_err());
    assert_eq!(
        store.state().unwrap().snapshot.base_step_id,
        before.snapshot.base_step_id
    );
    // A second destination gets a fresh observation, no implicit permission.
    let state = destination(&store, "elsewhere/moved.txt");
    let movement = NamespaceMutation::Move {
        source: RepoPath::parse("a.txt").unwrap(),
        destination: RepoPath::parse("elsewhere/moved.txt").unwrap(),
    };
    let preview = store
        .preview_namespace(state.generation, &movement)
        .unwrap();
    let moved = store.apply_namespace(&preview, None).unwrap();
    assert!(!f.0.join("a.txt").exists());
    assert_eq!(fs::read(f.0.join("elsewhere/moved.txt")).unwrap(), b"alpha");
    assert_eq!(moved.changes.len(), 3);
    assert_eq!(store.apply_namespace(&preview, None).unwrap(), moved);
    assert!(store.captured_patch(capture.sequence).is_ok());
    assert_eq!(store.fsck().unwrap().namespace_operations, 2);
}
struct StopNamespace(NamespacePoint);
impl NamespaceFaults for StopNamespace {
    fn check(&self, p: NamespacePoint) -> io::Result<()> {
        if p == self.0 {
            Err(io::Error::other("namespace interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn namespace_crash_recovery_is_atomic_for_creation_and_moves() {
    for moving in [false, true] {
        for point in [
            NamespacePoint::JournalDurable,
            NamespacePoint::PathApplied,
            NamespacePoint::BeforeCommit,
            NamespacePoint::AfterCommit,
        ] {
            let (f, store) = ready(false);
            let state = destination(&store, "result.txt");
            let mutation = if moving {
                NamespaceMutation::Move {
                    source: RepoPath::parse("a.txt").unwrap(),
                    destination: RepoPath::parse("result.txt").unwrap(),
                }
            } else {
                NamespaceMutation::Create {
                    destination: RepoPath::parse("result.txt").unwrap(),
                }
            };
            let preview = store
                .preview_namespace(state.generation, &mutation)
                .unwrap();
            let bytes = if moving {
                None
            } else {
                Some(b"new".as_slice())
            };
            assert!(
                store
                    .apply_namespace_with_faults(&preview, bytes, &StopNamespace(point))
                    .is_err()
            );
            let captured = if point == NamespacePoint::AfterCommit {
                store.apply_namespace(&preview, bytes).unwrap()
            } else {
                assert!(ProjectionStore::open(&f.0).is_err());
                ProjectionStore::resume_namespace(&f.0).unwrap()
            };
            let reopened = ProjectionStore::open(&f.0).unwrap();
            assert_eq!(
                reopened.captured_patch(captured.sequence).unwrap(),
                captured
            );
            assert_eq!(
                fs::read(f.0.join("result.txt")).unwrap(),
                if moving {
                    b"alpha".as_slice()
                } else {
                    b"new".as_slice()
                }
            );
            assert_eq!(f.0.join("a.txt").exists(), !moving);
            assert_eq!(reopened.apply_namespace(&preview, bytes).unwrap(), captured);
        }
    }
}
#[test]
fn namespace_refuses_stale_source_collision_alias_protected_paths_and_unknown_moves() {
    let (f, store) = ready(false);
    let initial = store.state().unwrap();
    for path in [
        "PRIVATE.TXT",
        "private.txt/child",
        "a.txt",
        "a.txt/child",
        "NUL",
        "new/../x",
    ] {
        if let Ok(path) = RepoPath::parse(path) {
            assert!(
                store
                    .observe_absent_paths(initial.generation, &initial.digest, &[path])
                    .is_err()
            );
        }
    }
    let state = destination(&store, "safe.txt");
    let mutation = NamespaceMutation::Move {
        source: RepoPath::parse("a.txt").unwrap(),
        destination: RepoPath::parse("safe.txt").unwrap(),
    };
    let preview = store
        .preview_namespace(state.generation, &mutation)
        .unwrap();
    fs::write(f.0.join("a.txt"), b"changed after preview").unwrap();
    assert!(store.apply_namespace(&preview, None).is_err());
    assert!(!f.0.join("safe.txt").exists());
    let preview = store
        .preview_namespace(state.generation, &mutation)
        .unwrap();
    fs::write(f.0.join("safe.txt"), b"external").unwrap();
    assert!(store.apply_namespace(&preview, None).is_err());
    assert_eq!(fs::read(f.0.join("safe.txt")).unwrap(), b"external");
    assert!(f.0.join("a.txt").exists());
    let (g, readonly) = ready(true);
    let s = destination(&readonly, "new.txt");
    assert!(
        readonly
            .preview_namespace(
                s.generation,
                &NamespaceMutation::Move {
                    source: RepoPath::parse("a.txt").unwrap(),
                    destination: RepoPath::parse("new.txt").unwrap()
                }
            )
            .is_err()
    );
    assert_eq!(fs::read(g.0.join("a.txt")).unwrap(), b"alpha");
}
#[test]
fn recovery_keeps_journal_if_destination_is_replaced_even_with_identical_bytes() {
    let (f, store) = ready(false);
    let state = destination(&store, "new.txt");
    let preview = store
        .preview_namespace(
            state.generation,
            &NamespaceMutation::Create {
                destination: RepoPath::parse("new.txt").unwrap(),
            },
        )
        .unwrap();
    assert!(
        store
            .apply_namespace_with_faults(
                &preview,
                Some(b"same"),
                &StopNamespace(NamespacePoint::PathApplied)
            )
            .is_err()
    );
    fs::rename(f.0.join("new.txt"), f.0.join("original.txt")).unwrap();
    fs::write(f.0.join("new.txt"), b"same").unwrap();
    assert!(ProjectionStore::resume_namespace(&f.0).is_err());
    assert!(ProjectionStore::open(&f.0).is_err());
    assert_eq!(fs::read(f.0.join("original.txt")).unwrap(), b"same");
    assert_eq!(fs::read(f.0.join("new.txt")).unwrap(), b"same");
}

#[test]
fn namespace_recovery_refuses_a_conflicting_refresh_without_touching_files() {
    let (f, store) = ready(false);
    let state = destination(&store, "new.txt");
    let preview = store
        .preview_namespace(
            state.generation,
            &NamespaceMutation::Move {
                source: RepoPath::parse("a.txt").unwrap(),
                destination: RepoPath::parse("new.txt").unwrap(),
            },
        )
        .unwrap();
    assert!(
        store
            .apply_namespace_with_faults(
                &preview,
                None,
                &StopNamespace(NamespacePoint::JournalDurable)
            )
            .is_err()
    );
    let db = rusqlite::Connection::open(f.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute(
        "INSERT INTO projection_refresh_pending VALUES(1,?1)",
        [b"{}".as_slice()],
    )
    .unwrap();
    drop(db);
    assert!(ProjectionStore::resume_namespace(&f.0).is_err());
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
    assert!(!f.0.join("new.txt").exists());
}

#[test]
fn a_created_file_can_be_renamed_before_publication_without_inventing_a_deletion() {
    let (f, store) = ready(false);
    let state = destination(&store, "new.txt");
    let create = store
        .preview_namespace(
            state.generation,
            &NamespaceMutation::Create {
                destination: RepoPath::parse("new.txt").unwrap(),
            },
        )
        .unwrap();
    let first = store
        .apply_namespace(&create, Some(b"local creation"))
        .unwrap();
    let state = destination(&store, "renamed.txt");
    let movement = store
        .preview_namespace(
            state.generation,
            &NamespaceMutation::Move {
                source: RepoPath::parse("new.txt").unwrap(),
                destination: RepoPath::parse("renamed.txt").unwrap(),
            },
        )
        .unwrap();
    let moved = store.apply_namespace(&movement, None).unwrap();
    assert_eq!(moved.changes.len(), 1);
    assert_eq!(moved.changes[0].path.as_str(), "renamed.txt");
    assert!(moved.changes[0].expected.is_none());
    assert_eq!(
        fs::read(f.0.join("renamed.txt")).unwrap(),
        b"local creation"
    );
    assert!(!f.0.join("new.txt").exists());
    assert_eq!(store.apply_namespace(&movement, None).unwrap(), moved);
    assert_eq!(store.captured_patch(first.sequence).unwrap(), first);
    assert_eq!(store.fsck().unwrap().namespace_operations, 2);
}
