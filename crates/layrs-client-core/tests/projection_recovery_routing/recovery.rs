use super::*;
use layrs_store_local::projection::{CreationFaults, CreationPoint};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn family(f: &Fixture) -> PartialStoreFamily {
    let id = identity();
    PartialStoreFamily::create(
        f.directory("family"),
        PartialStoreIdentity {
            store_id: id.store_id,
            space_id: id.space_id,
            origin: id.origin,
            principal_id: id.principal_id,
        },
        SharedQuota {
            bytes: 32 * 1024 * 1024,
        },
    )
    .unwrap()
}

pub(super) fn fingerprints(root: &Path) -> BTreeMap<PathBuf, (usize, FileId)> {
    files(root)
        .into_iter()
        .map(|(path, bytes)| (path, (bytes.len(), FileId::from_bytes(&bytes))))
        .collect()
}

// This helper never runs in an ordinary suite. Only its parent may start and
// kill it, using a fresh fixture and a bounded readiness deadline.
#[test]
#[ignore = "owned SQLite crash helper"]
fn sqlite_spill_child() {
    let path = std::env::var_os("LAYRS_ROUTING_SPILL_DATABASE").unwrap();
    let ready = std::env::var_os("LAYRS_ROUTING_SPILL_READY").unwrap();
    let mode = std::env::var("LAYRS_ROUTING_SPILL_MODE").unwrap();
    let c = rusqlite::Connection::open(PathBuf::from(path)).unwrap();
    c.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=4; PRAGMA cache_spill=ON; BEGIN IMMEDIATE;").unwrap();
    if mode == "catalog" {
        c.execute("UPDATE quota SET bytes=1", []).unwrap();
        for i in 0..512 {
            c.execute(
                "INSERT INTO reservations VALUES(?1,'worktree:routing',?2,1)",
                rusqlite::params![format!("routing-spill-{i}"), "x".repeat(8192)],
            )
            .unwrap();
        }
    } else {
        c.execute("DELETE FROM projection_claims", []).unwrap();
        for i in 0..128 {
            c.execute(
                "INSERT INTO projection_observations VALUES(?1,?2)",
                rusqlite::params![format!("routing-spill-{i}"), vec![9u8; 65536]],
            )
            .unwrap();
        }
    }
    fs::write(ready, b"spilled").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn kill_spilled_writer(path: &Path, mode: &str, ready: &Path) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "recovery::sqlite_spill_child",
            "--nocapture",
        ])
        .env("LAYRS_ROUTING_SPILL_DATABASE", path)
        .env("LAYRS_ROUTING_SPILL_READY", ready)
        .env("LAYRS_ROUTING_SPILL_MODE", mode)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = OwnedChild(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && Instant::now() < deadline {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "spill child exited before readiness"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ready.exists(), "spill child readiness timeout");
    drop(child);
    let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let error = c
        .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
        .unwrap_err();
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(ref e, _)
        if e.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK),
        "{error}"
    );
}

#[test]
fn hot_databases_route_without_writes_then_explicit_recovery_keeps_application_journal() {
    for mode in ["catalog", "owner"] {
        let f = Fixture::new();
        let family = family(&f);
        let root = f.directory("checkout");
        let store = family
            .create_worktree(
                &root,
                identity().worktree_id,
                snapshot(),
                ProjectionSelection::All,
            )
            .unwrap();
        let mut target = snapshot();
        target.entries[0].state = EntryState::Materialized;
        assert!(store
            .install_projection_with_faults(
                0,
                &target,
                &mut |_, out| out.write_all(b"alpha"),
                &StopAfterJournal
            )
            .is_err());
        let path = if mode == "catalog" {
            family.root().join("metadata/catalog.sqlite3")
        } else {
            let marker: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(".layrs/FORMAT")).unwrap()).unwrap();
            let metadata = PathBuf::from(marker["shared"]["metadataPath"].as_str().unwrap());
            assert!(metadata.is_absolute() && metadata.starts_with(family.root()));
            metadata.join("projection.sqlite3")
        };
        drop(store);
        kill_spilled_writer(&path, mode, &f.0.join("spill-ready"));
        let before = fingerprints(&f.0);
        assert_eq!(checkout_kind(&root).unwrap(), "projection");
        let inspected =
            execute_projection_offline(&root, ProjectionOfflineAction::InspectRecovery {}).unwrap();
        let expected = if mode == "catalog" {
            "familyRollback"
        } else {
            "worktreeRollback"
        };
        assert!(inspected["recovery"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == expected));
        assert_eq!(inspected["conversion"]["allowed"], false);
        assert_eq!(
            fingerprints(&f.0),
            before,
            "discovery changed durable bytes"
        );
        let recovered = execute_projection_offline(
            &root,
            ProjectionOfflineAction::RecoverDatabases {
                expected: binding(&inspected),
            },
        )
        .unwrap();
        assert_eq!(
            recovered["recovery"],
            serde_json::json!(["filesystemOperation"])
        );
        assert!(
            !root.join("a.txt").exists(),
            "SQL rollback replayed an application journal"
        );
        assert_eq!(checkout_kind(&root).unwrap(), "projection");
        execute_projection(
            &root,
            ProjectionAction::ResumeInstallation {
                expected: binding(&recovered),
            },
        )
        .unwrap();
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"alpha");
        let info =
            execute_projection_offline(&root, ProjectionOfflineAction::InspectRecovery {}).unwrap();
        assert_eq!(info["recovery"], serde_json::json!([]));
        ProjectionStore::open(&root).unwrap().fsck().unwrap();
    }
}

pub(super) struct StopAtMarker;
impl CreationFaults for StopAtMarker {
    fn check(&self, point: CreationPoint) -> io::Result<()> {
        if point == CreationPoint::MarkerDurable {
            Err(io::Error::other("injected marker interruption"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn initializing_marker_routes_without_writes_and_resumes_only_its_recorded_bootstrap() {
    let f = Fixture::new();
    let family = family(&f);
    let root = f.directory("checkout");
    assert!(family
        .create_worktree_with_faults(
            &root,
            identity().worktree_id,
            snapshot(),
            ProjectionSelection::All,
            &StopAtMarker
        )
        .is_err());
    let before = fingerprints(&f.0);
    assert_eq!(checkout_kind(&root).unwrap(), "projection");
    let inspected =
        execute_projection_offline(&root, ProjectionOfflineAction::InspectRecovery {}).unwrap();
    assert_eq!(inspected["recovery"], serde_json::json!(["initializing"]));
    assert_eq!(inspected["conversion"]["allowed"], false);
    assert_eq!(fingerprints(&f.0), before);
    let ready = execute_projection_offline(
        &root,
        ProjectionOfflineAction::ResumeInitialization {
            expected: binding(&inspected),
        },
    )
    .unwrap();
    assert_eq!(ready["identity"], inspected["identity"]);
    assert_eq!(ready["recovery"], serde_json::json!([]));
    let store = ProjectionStore::open(&root).unwrap();
    assert_eq!(store.state().unwrap().snapshot, snapshot());
    assert_eq!(store.state().unwrap().generation, 0);
    let before = fingerprints(&f.0);
    execute_projection_offline(
        &root,
        ProjectionOfflineAction::ResumeInitialization {
            expected: binding(&ready),
        },
    )
    .unwrap();
    assert_eq!(
        fingerprints(&f.0),
        before,
        "ready retry should be read-only"
    );
}
