//! Real admitted restore staging plus owned installation fault states. Faults
//! return errors; no process crash, hot journal or source deletion is claimed.
use super::*;
use crate::projection::{
    format,
    provider::{admitted_ordinary, process_profile::tests::child},
    retention,
    shared::InstallationPoint,
    CapturedFile, ProjectionRecoveryReason, ProjectionStore, RestoreFaults, RestorePoint,
    RestoreSource, SessionMode,
};
use layrs_core::scm::{FileId, RepoPath};
use std::{fs, io, path::Path};

struct StopStaged;
impl RestoreFaults for StopStaged {
    fn check(&self, point: RestorePoint) -> io::Result<()> {
        if point == RestorePoint::Staged {
            Err(io::Error::other("owned prejournal stop"))
        } else {
            Ok(())
        }
    }
}

fn witnesses(
    root: &Path,
    family: &Path,
) -> (
    std::collections::BTreeMap<std::path::PathBuf, super::managed_ready_reopen_tests::Node>,
    std::collections::BTreeMap<std::path::PathBuf, super::managed_ready_reopen_tests::Node>,
) {
    (
        super::managed_ready_reopen_tests::tree(root),
        super::managed_ready_reopen_tests::tree(family),
    )
}

fn partial_installation(
    store: &ProjectionStore,
    file: &CapturedFile,
    mut bytes: &[u8],
    stop: InstallationPoint,
) {
    format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            let reservation = retention::active_budget(&access.connection)?;
            let error = access
                .family
                .as_ref()
                .unwrap()
                .install_file_with_faults(&mut bytes, file, &reservation, &mut |point| {
                    if point == stop {
                        Err(io::Error::other("owned installation stop"))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert!(
                error.to_string().contains("owned installation stop"),
                "{error}"
            );
            Ok(())
        })
        .unwrap();
}

/// Fixture corruption and explicit repair are separate from the public refusal.
/// Every comparison occurs after the private owner scope has positively closed.
fn private_refusals(store: &ProjectionStore, family: &Path) {
    let original: Vec<u8> = format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            let value = access.connection.query_row(
                "SELECT payload FROM projection_prejournal",
                [],
                |r| r.get(0),
            )?;
            access
                .connection
                .execute("UPDATE projection_prejournal SET payload=x'7b'", [])?;
            Ok(value)
        })
        .unwrap();
    let before = witnesses(&store.root, family);
    assert!(store.cancel_shared_reservation().is_err());
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "malformed staging refusal has no effects"
    );
    format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            access
                .connection
                .execute("UPDATE projection_prejournal SET payload=?1", [original])?;
            Ok(())
        })
        .unwrap();

    let (relative, reservation): (String, String) = format::access(store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            Ok((
                access
                    .connection
                    .query_row("SELECT path FROM projection_prejournal", [], |r| r.get(0))?,
                retention::active_budget(&access.connection)?,
            ))
        })
        .unwrap();
    let unexpected = store
        .root
        .join(relative)
        .join("unexpected-owned-fixture-entry");
    fs::write(&unexpected, b"preserve on refusal").unwrap();
    let before = witnesses(&store.root, family);
    assert!(store.cancel_shared_reservation().is_err());
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "unexpected staging entry has no effects"
    );
    fs::remove_file(&unexpected).unwrap(); // Only this exact newly-created fixture file.

    format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            access.connection.execute(
                "UPDATE projection_budgets SET reservation='missing-fixture-reservation'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let before = witnesses(&store.root, family);
    assert!(store.cancel_shared_reservation().is_err());
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "wrong reservation has no effects"
    );
    format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            access.connection.execute(
                "UPDATE projection_budgets SET reservation=?1",
                [reservation],
            )?;
            Ok(())
        })
        .unwrap();
}

/// A bad later installation must prevent cleanup of the first installation.
fn later_installation_refusal(store: &ProjectionStore, family: &Path) {
    let bytes = b"second owned installation for global preflight";
    let extra = CapturedFile {
        file_id: FileId::from_bytes(bytes),
        bytes: bytes.len() as u64,
        executable: false,
    };
    partial_installation(store, &extra, bytes, InstallationPoint::IntentDurable);
    let directory: String = format::access(store, SessionMode::Shared).unwrap().run(|access| {
        let reservation = retention::active_budget(&access.connection)?;
        access.family.as_ref().unwrap().with_content_catalog(false, |c| {
            let rows = c.prepare("SELECT payload FROM installations WHERE reservation=?1 ORDER BY file_id")?
                .query_map([reservation], |r| r.get::<_, Vec<u8>>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            assert_eq!(rows.len(), 2);
            let value: serde_json::Value = serde_json::from_slice(rows.last().unwrap()).unwrap();
            Ok(value["directory"].as_str().unwrap().to_owned())
        })
    }).unwrap();
    let unexpected = family
        .join("staging")
        .join(directory)
        .join("unexpected-owned-fixture-entry");
    fs::write(
        &unexpected,
        b"later invalid intent must preserve earlier staging",
    )
    .unwrap();
    let before = witnesses(&store.root, family);
    assert!(store.cancel_shared_reservation().is_err());
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "complete preflight precedes every cleanup"
    );
    fs::remove_file(&unexpected).unwrap();
}

fn hardlink_refusal(store: &ProjectionStore, family: &Path) {
    let directory: String = format::access(store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            let reservation = retention::active_budget(&access.connection)?;
            access
                .family
                .as_ref()
                .unwrap()
                .with_content_catalog(false, |c| {
                    let payload: Vec<u8> = c.query_row(
                        "SELECT payload FROM installations WHERE reservation=?1",
                        [reservation],
                        |r| r.get(0),
                    )?;
                    let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                    Ok(value["directory"].as_str().unwrap().to_owned())
                })
        })
        .unwrap();
    let alias = store.root.join("owned-cancellation-hardlink-alias");
    fs::hard_link(
        family.join("staging").join(directory).join("chunk-0"),
        &alias,
    )
    .unwrap();
    let before = witnesses(&store.root, family);
    let error = store.cancel_shared_reservation().unwrap_err();
    assert!(error.to_string().contains("single-link"), "{error}");
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "hardlinked staging refusal has no effects"
    );
    fs::remove_file(alias).unwrap(); // Retire only the alias made by this fixture.
}

fn ordinary_alias_refusal(store: &ProjectionStore, family: &Path) {
    let (reservation, original, other): (String, Vec<u8>, String) =
        format::access(store, SessionMode::Exclusive)
            .unwrap()
            .run(|access| {
                let reservation = retention::active_budget(&access.connection)?;
                let lease = access.family.as_ref().unwrap();
                let other = lease.reserve(&store.identity().worktree_id, "restore:1-2", 262144)?;
                let bytes = b"foreign";
                let file = CapturedFile {
                    file_id: FileId::from_bytes(bytes),
                    bytes: bytes.len() as u64,
                    executable: false,
                };
                let error = lease
                    .install_file_with_faults(&mut bytes.as_slice(), &file, &other, &mut |point| {
                        if point == InstallationPoint::ChunkStaged {
                            Err(io::Error::other("owned foreign installation stop"))
                        } else {
                            Ok(())
                        }
                    })
                    .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("owned foreign installation stop"),
                    "{error}"
                );
                let original = lease.with_content_catalog(true, |c| {
                    let original: Vec<u8> = c.query_row(
                        "SELECT payload FROM installations WHERE reservation=?1",
                        [&reservation],
                        |r| r.get(0),
                    )?;
                    let foreign: Vec<u8> = c.query_row(
                        "SELECT payload FROM installations WHERE reservation=?1",
                        [&other],
                        |r| r.get(0),
                    )?;
                    let mut forged: serde_json::Value = serde_json::from_slice(&original).unwrap();
                    let foreign: serde_json::Value = serde_json::from_slice(&foreign).unwrap();
                    forged["directory"] = foreign["directory"].clone();
                    forged["directoryIdentity"] = foreign["directoryIdentity"].clone();
                    let payload = layrs_core::scm::canonical_json(&forged).unwrap();
                    c.execute(
                        "UPDATE installations SET payload=?1 WHERE reservation=?2",
                        rusqlite::params![payload, reservation],
                    )?;
                    Ok(original)
                })?;
                Ok((reservation, original, other))
            })
            .unwrap();
    let before = witnesses(&store.root, family);
    let error = store.cancel_shared_reservation().unwrap_err();
    assert!(
        error.to_string().contains("ordinary staging alias"),
        "{error}"
    );
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "another reservation's staging stays untouched"
    );
    format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            let bytes = fs::read(store.root.join("README.md"))?;
            let file = CapturedFile {
                file_id: FileId::from_bytes(&bytes),
                bytes: bytes.len() as u64,
                executable: false,
            };
            let error = access
                .family
                .as_ref()
                .unwrap()
                .install_file(&mut bytes.as_slice(), &file, &reservation)
                .unwrap_err();
            assert!(
                error.to_string().contains("ordinary staging alias"),
                "{error}"
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        witnesses(&store.root, family),
        before,
        "ordinary retry also refuses the foreign staging alias"
    );
    // Separate fixture repair, then retire only the explicitly created extra reservation.
    format::access(store, SessionMode::Exclusive)
        .unwrap()
        .run(|access| {
            let lease = access.family.as_ref().unwrap();
            lease.with_content_catalog(true, |c| {
                c.execute(
                    "UPDATE installations SET payload=?1 WHERE reservation=?2",
                    rusqlite::params![original, reservation],
                )?;
                Ok(())
            })?;
            lease.cancel_restore_installations(
                &store.marker,
                &other,
                "restore:1-2",
                262144,
                false,
            )?;
            lease.release_reservation(&other, &store.identity().worktree_id)?;
            Ok(())
        })
        .unwrap();
}

#[test]
#[ignore = "ROOT source release and audited fresh C: fixture required; child1200 / outer1260"]
fn managed_admitted_restore_cancels_prejournal_and_partial_installations_then_retries() {
    child::run(
        concat!(
            module_path!(),
            "::managed_admitted_restore_cancels_prejournal_and_partial_installations_then_retries"
        ),
        1200,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let marker = format::read_marker(&root).unwrap();
            let family = marker.shared.as_ref().unwrap().root.clone();
            let state = store.state().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            let baseline = format::access(&store, SessionMode::Shared)
                .unwrap()
                .run(|access| {
                    admitted_ordinary::inspect(&access.connection, &marker, None)
                        .map_err(Into::into)
                })
                .unwrap();
            use retention::RestoreCancellationPoint as Cancel;
            for (index, (installation, cancel_stop)) in [
                (None, Some(Cancel::InstallationsCleared)),
                (
                    Some(InstallationPoint::IntentDurable),
                    Some(Cancel::StagingCleared),
                ),
                (
                    Some(InstallationPoint::ChunkStaged),
                    Some(Cancel::BudgetCommitted),
                ),
                (
                    Some(InstallationPoint::ManifestDurable),
                    Some(Cancel::ReservationReleased),
                ),
                (Some(InstallationPoint::ChunkPublished), None),
                (Some(InstallationPoint::BeforeCatalogCommit), None),
                (Some(InstallationPoint::CatalogCommitted), None),
                (None, Some(Cancel::StagingMarkedCleaning)),
                (None, Some(Cancel::StagingFileRemoved)),
                (None, Some(Cancel::StagingDirectoryRemoved)),
            ]
            .into_iter()
            .enumerate()
            {
                eprintln!("RESTORE_CANCELLATION_PHASE={index}:{installation:?}:{cancel_stop:?}");
                let target = format!("restorable target {index}").into_bytes();
                let dirty =
                    format!("dirty checkout preserved by cancellation {index}").into_bytes();
                fs::write(root.join(path.as_str()), &target).unwrap();
                let captured = store.capture_patch(state.generation).unwrap();
                fs::write(root.join(path.as_str()), &dirty).unwrap();
                let preview = store
                    .preview_restore(
                        state.generation,
                        &RestoreSource::Capture {
                            sequence: captured.sequence,
                            paths: vec![path.clone()],
                        },
                    )
                    .unwrap();
                let error = store
                    .apply_restore_with_faults(&preview, &StopStaged)
                    .unwrap_err();
                assert!(
                    error.to_string().contains("owned prejournal stop"),
                    "{error}"
                );
                if let Some(point) = installation {
                    // Same production installation helper/reservation used while preserving
                    // old checkout bytes; no private capture/journal has accepted them yet.
                    partial_installation(
                        &store,
                        preview.before[&path].as_ref().unwrap(),
                        &dirty,
                        point,
                    );
                }
                if index == 0 {
                    private_refusals(&store, &family);
                }
                if index == 1 {
                    later_installation_refusal(&store, &family);
                }
                if index == 2 {
                    ordinary_alias_refusal(&store, &family);
                }
                if index == 3 {
                    hardlink_refusal(&store, &family);
                }
                let reservation = format::access(&store, SessionMode::Shared)
                    .unwrap()
                    .run(|access| retention::active_budget(&access.connection))
                    .unwrap();
                let inspection = ProjectionStore::inspect_recovery(&root).unwrap();
                assert_eq!(
                    inspection.recovery,
                    vec![ProjectionRecoveryReason::Reservation]
                );
                assert!(
                    ProjectionStore::resume_restore(&root).is_err(),
                    "prejournal cannot resume as accepted restore"
                );
                if let Some(stop) = cancel_stop {
                    let access =
                        format::cancel_recovery_access(&store, Some(&inspection.binding)).unwrap();
                    let error = retention::cancel_admitted_with_faults(&store, access, &|point| {
                        if point == stop {
                            Err(io::Error::other("owned cancellation stop").into())
                        } else {
                            Ok(())
                        }
                    })
                    .unwrap_err();
                    assert!(
                        error.to_string().contains("owned cancellation stop"),
                        "{error}"
                    );
                    assert_eq!(
                        ProjectionStore::inspect_recovery(&root).unwrap().recovery,
                        vec![ProjectionRecoveryReason::Reservation]
                    );
                }
                if index % 2 == 0 {
                    ProjectionStore::open_bound_recovery(&root, &inspection.binding)
                        .unwrap()
                        .cancel_shared_reservation()
                        .unwrap();
                } else {
                    store.cancel_shared_reservation().unwrap();
                }
                store.cancel_shared_reservation().unwrap(); // Empty terminal retry performs no cleanup.
                assert_eq!(fs::read(root.join(path.as_str())).unwrap(), dirty);
                assert!(ProjectionStore::inspect_recovery(&root)
                    .unwrap()
                    .recovery
                    .is_empty());
                format::access(&store, SessionMode::Shared).unwrap().run(|access| {
                let count: u64 = access.connection.query_row("SELECT (SELECT count(*) FROM projection_budgets)+(SELECT count(*) FROM projection_prejournal)", [], |r| r.get(0))?;
                assert_eq!(count, 0);
                access.family.as_ref().unwrap().with_content_catalog(false, |c| {
                    let count: u64 = c.query_row("SELECT (SELECT count(*) FROM reservations WHERE id=?1)+(SELECT count(*) FROM installations WHERE reservation=?1)", [&reservation], |r| r.get(0))?;
                    assert_eq!(count, 0);
                    Ok(())
                })
            }).unwrap();
                let receipt = store.apply_restore(&preview).unwrap();
                assert_eq!(fs::read(root.join(path.as_str())).unwrap(), target);
                let inverse = store
                    .preview_restore(
                        state.generation,
                        &RestoreSource::Inverse {
                            operation_id: receipt.operation_id,
                        },
                    )
                    .unwrap();
                store.apply_restore(&inverse).unwrap();
                assert_eq!(fs::read(root.join(path.as_str())).unwrap(), dirty);
                let mut retained = Vec::new();
                store
                    .write_captured_file(captured.sequence, &path, &mut retained)
                    .unwrap();
                assert_eq!(retained, target);
            }
            format::access(&store, SessionMode::Shared)
                .unwrap()
                .run(|access| {
                    baseline.require_same_import(&admitted_ordinary::inspect(
                        &access.connection,
                        &marker,
                        None,
                    )?)?;
                    access
                        .with_content(|c, scope| retention::fsck_with_scope(c, &store, Some(scope)))
                })
                .unwrap();
            assert_eq!(store.state().unwrap(), state);
        },
    );
}
