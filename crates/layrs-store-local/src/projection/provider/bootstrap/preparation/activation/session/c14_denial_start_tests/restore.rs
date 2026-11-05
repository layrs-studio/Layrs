//! Public local Restore on a genuine C14 admission after B is unavailable.
//! Runs after the local Capture phases on the same admission: only the
//! post-Ready suffix may be sourced, every overwritten byte stays recoverable
//! and the immutable import/Ready provenance never changes.
use super::capture::{require_bytes, Immutable};
use super::*;
use crate::projection::{
    capture::CapturedPatch, ProjectionRecoveryReason, ProjectionStore, RestoreFaults,
    RestorePoint, RestorePreview, RestoreReceipt, RestoreSource,
};
use layrs_core::projection::ProjectionVersion;

struct StopAt(RestorePoint);
impl RestoreFaults for StopAt {
    fn check(&self, point: RestorePoint) -> std::io::Result<()> {
        if point == self.0 {
            Err(std::io::Error::other("injected C14 Restore boundary"))
        } else {
            Ok(())
        }
    }
}

const DIRTY_BYTES: &[u8] = b"C14 dirty bytes never captured before Restore";

/// Same worktree and Ready generation with a wrong digest: never the image.
pub(super) fn forged_ready_version(store: &ProjectionStore) -> ProjectionVersion {
    match store.current_version().unwrap() {
        ProjectionVersion::Generation {
            worktree_id,
            generation,
            ..
        } => ProjectionVersion::Generation {
            worktree_id,
            generation,
            digest: "0".repeat(64),
        },
        other => panic!("C14 current version is not a generation: {other:?}"),
    }
}

fn reasons(completed: &admission::Completed) -> Vec<ProjectionRecoveryReason> {
    ProjectionStore::inspect_recovery(&completed.root)
        .unwrap()
        .recovery
}

fn require_refused(
    completed: &admission::Completed,
    store: &ProjectionStore,
    generation: u64,
    source: RestoreSource,
    expected: &str,
) {
    let witness = database_io::tests::support::witness;
    let private = witness(&completed.destination_database);
    let family = witness(&completed.family_database);
    let error = store.preview_restore(generation, &source).unwrap_err();
    assert!(error.to_string().contains(expected), "{error}");
    assert_eq!(witness(&completed.destination_database), private);
    assert_eq!(witness(&completed.family_database), family);
}

fn require_receipt(
    store: &ProjectionStore,
    preview: &RestorePreview,
    receipt: &RestoreReceipt,
    previous: &CapturedPatch,
    path: &RepoPath,
    bytes: &[u8],
) {
    // A restore never deduplicates its capture: it always advances the
    // frontier, chained to the previous capture, with the restored image.
    assert_eq!(receipt.operation_id, preview.operation_id);
    assert_eq!(receipt.capture.sequence, previous.sequence + 1);
    assert_eq!(
        receipt.capture.previous_patch_digest.as_ref(),
        Some(&previous.digest)
    );
    assert_eq!(receipt.capture.projection_generation, previous.projection_generation);
    assert_eq!(receipt.capture.projection_digest, previous.projection_digest);
    assert_eq!(
        store.latest_captured_patch().unwrap().as_ref(),
        Some(&receipt.capture)
    );
    require_bytes(store, &receipt.capture, path, bytes);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    completed: &admission::Completed,
    store: &ProjectionStore,
    original: &Immutable,
    path: &RepoPath,
    first: &CapturedPatch,
    latest: &CapturedPatch,
    has_companion: bool,
    cancel: &Cancellation,
    timing: &mut child::PhaseTimer,
) {
    let state = store.state().unwrap();
    let generation = state.generation;
    let target = completed.root.join(path.as_str());
    let first_bytes = b"C14 authorized absent creation A";

    timing.next("c14-local-restore-source-gate");
    require_refused(
        completed,
        store,
        generation,
        RestoreSource::Version {
            version: forged_ready_version(store),
            paths: vec![path.clone()],
        },
        "C14 local Restore accepts only the fixed Ready version, post-Ready Capture or local inverse sources",
    );
    // The immutable Ready capture addressed as a Version stays closed too.
    require_refused(
        completed,
        store,
        generation,
        RestoreSource::Version {
            version: ProjectionVersion::Capture {
                worktree_id: store.identity().worktree_id.clone(),
                sequence: original.data.ready_capture_sequence(),
                digest: "0".repeat(64),
            },
            paths: vec![path.clone()],
        },
        "C14 local Restore accepts only the fixed Ready version, post-Ready Capture or local inverse sources",
    );
    require_refused(
        completed,
        store,
        generation,
        RestoreSource::Capture {
            sequence: original.data.ready_capture_sequence(),
            paths: vec![path.clone()],
        },
        "C14 local Restore source precedes fixed Ready capture frontier",
    );
    require_refused(
        completed,
        store,
        generation,
        RestoreSource::Inverse {
            operation_id: "1-1".into(),
        },
        "restore operation unknown",
    );
    if has_companion {
        // Reserved denied paths are never local-suffix restore targets.
        let error = store
            .preview_restore(
                generation,
                &RestoreSource::Capture {
                    sequence: first.sequence,
                    paths: vec![RepoPath::parse("README.md").unwrap()],
                },
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("selected path is absent from capture"),
            "{error}"
        );
    }
    original.require(store, cancel);

    // Dirty bytes that no Capture ever saw are the restore preimage.
    timing.next("c14-local-restore-apply");
    fs::write(&target, DIRTY_BYTES).unwrap();
    let witness = database_io::tests::support::witness;
    let private = witness(&completed.destination_database);
    let family = witness(&completed.family_database);
    let to_first = store
        .preview_restore(
            generation,
            &RestoreSource::Capture {
                sequence: first.sequence,
                paths: vec![path.clone()],
            },
        )
        .unwrap();
    assert_eq!(witness(&completed.destination_database), private, "preview is read-only");
    assert_eq!(witness(&completed.family_database), family, "preview is read-only");
    assert_eq!(to_first.projection_generation, generation);
    assert_eq!(fs::read(&target).unwrap(), DIRTY_BYTES);
    let restored = store.apply_restore(&to_first).unwrap();
    assert_eq!(fs::read(&target).unwrap(), first_bytes);
    require_receipt(store, &to_first, &restored, latest, path, first_bytes);
    assert_eq!(
        store.capture_patch(generation).unwrap(),
        restored.capture,
        "the restored image deduplicates to the restore's own capture"
    );
    assert_eq!(store.apply_restore(&to_first).unwrap(), restored, "exact replay");
    assert!(reasons(completed).is_empty());
    assert_eq!(store.state().unwrap(), state);
    original.require(store, cancel);

    timing.next("c14-local-restore-inverse-retains-dirty-preimage");
    let back = store
        .preview_restore(
            generation,
            &RestoreSource::Inverse {
                operation_id: restored.operation_id.clone(),
            },
        )
        .unwrap();
    let inverse = store.apply_restore(&back).unwrap();
    assert_eq!(fs::read(&target).unwrap(), DIRTY_BYTES);
    require_receipt(store, &back, &inverse, &restored.capture, path, DIRTY_BYTES);
    original.require(store, cancel);

    // Before its journal, a restore owns only its budget, prejournal and
    // private staging: resume refuses, cancellation releases exactly those,
    // and the unchanged reviewed preview then applies.
    timing.next("c14-local-restore-prejournal-cancel");
    let staged = store
        .preview_restore(
            generation,
            &RestoreSource::Capture {
                sequence: first.sequence,
                paths: vec![path.clone()],
            },
        )
        .unwrap();
    let failure = store
        .apply_restore_with_faults(&staged, &StopAt(RestorePoint::Staged))
        .unwrap_err();
    assert!(
        failure.to_string().contains("injected C14 Restore boundary"),
        "{failure}"
    );
    assert_eq!(fs::read(&target).unwrap(), DIRTY_BYTES);
    assert_eq!(reasons(completed), vec![ProjectionRecoveryReason::Reservation]);
    assert!(ProjectionStore::resume_restore(&completed.root).is_err());
    let failure = store.capture_patch(generation).unwrap_err();
    assert!(
        failure.to_string().contains("shared quota reservation pending"),
        "{failure}"
    );
    store.cancel_shared_reservation().unwrap();
    assert_eq!(fs::read(&target).unwrap(), DIRTY_BYTES);
    assert!(reasons(completed).is_empty());
    let private = witness(&completed.destination_database);
    let family = witness(&completed.family_database);
    store.cancel_shared_reservation().unwrap();
    assert_eq!(witness(&completed.destination_database), private);
    assert_eq!(witness(&completed.family_database), family);
    let retried = store.apply_restore(&staged).unwrap();
    assert_eq!(fs::read(&target).unwrap(), first_bytes);
    require_receipt(store, &staged, &retried, &inverse.capture, path, first_bytes);
    original.require(store, cancel);

    // After its journal is durable, a restore can only finish: cancellation
    // refuses without effect and resume completes the reviewed operation.
    timing.next("c14-local-restore-journal-resume");
    let journaled = store
        .preview_restore(
            generation,
            &RestoreSource::Inverse {
                operation_id: retried.operation_id.clone(),
            },
        )
        .unwrap();
    let failure = store
        .apply_restore_with_faults(&journaled, &StopAt(RestorePoint::PathQuarantined))
        .unwrap_err();
    assert!(
        failure.to_string().contains("injected C14 Restore boundary"),
        "{failure}"
    );
    assert_eq!(
        reasons(completed),
        vec![ProjectionRecoveryReason::FilesystemOperation]
    );
    let private = witness(&completed.destination_database);
    let family = witness(&completed.family_database);
    let refused = store.cancel_shared_reservation().unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("C14 Restore cancellation cannot cancel an accepted journal"),
        "{refused}"
    );
    assert_eq!(witness(&completed.destination_database), private);
    assert_eq!(witness(&completed.family_database), family);
    let resumed = ProjectionStore::resume_restore(&completed.root).unwrap();
    assert_eq!(fs::read(&target).unwrap(), DIRTY_BYTES);
    require_receipt(store, &journaled, &resumed, &retried.capture, path, DIRTY_BYTES);
    // The recovery route leaves only its committed budget; the next local
    // access settles it, and replay returns the recorded receipt.
    assert_eq!(store.apply_restore(&journaled).unwrap(), resumed);
    assert!(reasons(completed).is_empty());
    original.require(store, cancel);

    // An error after COMMIT preserves the accepted restore; replay settles.
    timing.next("c14-local-restore-after-commit");
    let committed = store
        .preview_restore(
            generation,
            &RestoreSource::Capture {
                sequence: first.sequence,
                paths: vec![path.clone()],
            },
        )
        .unwrap();
    let failure = store
        .apply_restore_with_faults(&committed, &StopAt(RestorePoint::AfterCommit))
        .unwrap_err();
    assert!(
        failure.to_string().contains("injected C14 Restore boundary"),
        "{failure}"
    );
    assert_eq!(fs::read(&target).unwrap(), first_bytes);
    assert_eq!(reasons(completed), vec![ProjectionRecoveryReason::Reservation]);
    let replayed = store.apply_restore(&committed).unwrap();
    require_receipt(store, &committed, &replayed, &resumed.capture, path, first_bytes);
    assert!(reasons(completed).is_empty());
    original.require(store, cancel);

    // Discarding local work offline: the fixed Ready image is the one
    // restorable Version. recovery.txt is an authorized absence there.
    timing.next("c14-local-restore-ready-version");
    let discard = store
        .preview_restore(
            generation,
            &RestoreSource::Version {
                version: store.current_version().unwrap(),
                paths: vec![path.clone()],
            },
        )
        .unwrap();
    assert_eq!(discard.after[path], None);
    let discarded = store.apply_restore(&discard).unwrap();
    assert!(!target.exists());
    assert_eq!(discarded.capture.sequence, replayed.capture.sequence + 1);
    assert_eq!(
        discarded.capture.previous_patch_digest.as_ref(),
        Some(&replayed.capture.digest)
    );
    assert!(discarded.capture.changes.iter().all(|change| &change.path != path));
    assert!(reasons(completed).is_empty());
    original.require(store, cancel);
    let undo = store
        .preview_restore(
            generation,
            &RestoreSource::Inverse {
                operation_id: discarded.operation_id.clone(),
            },
        )
        .unwrap();
    let undone = store.apply_restore(&undo).unwrap();
    require_receipt(store, &undo, &undone, &discarded.capture, path, first_bytes);
    original.require(store, cancel);

    // Product offline route (PreviewRestore/ApplyRestore) accepts Versions
    // only: a post-Ready local Capture addressed as a Version restores it.
    timing.next("c14-local-restore-capture-version");
    fs::write(&target, b"C14 dirty bytes before a Capture-version restore").unwrap();
    let local_version = ProjectionVersion::Capture {
        worktree_id: store.identity().worktree_id.clone(),
        sequence: inverse.capture.sequence,
        digest: inverse.capture.digest.clone(),
    };
    let by_version = store
        .preview_restore(
            generation,
            &RestoreSource::Version {
                version: local_version.clone(),
                paths: vec![path.clone()],
            },
        )
        .unwrap();
    let restored_version = store.apply_restore(&by_version).unwrap();
    require_receipt(store, &by_version, &restored_version, &undone.capture, path, DIRTY_BYTES);
    original.require(store, cancel);

    // Every read the product Status/History/offline screens issue works on C14.
    timing.next("c14-local-restore-product-surface");
    let (current, _) = store.state_with_selection().unwrap();
    assert_eq!(current, state);
    assert!(store.pending_publication().unwrap().is_none());
    assert!(!store
        .is_imported_capture(restored_version.capture.sequence)
        .unwrap());
    let page = store.capture_history(None, 3).unwrap();
    assert_eq!(page.captures.first(), Some(&restored_version.capture));
    assert_eq!(page.captures.len(), 3);
    store.preparations().unwrap();
    let differences = store
        .compare_versions(&layrs_core::projection::AvailabilityRequest {
            version: store.current_version().unwrap(),
            other: Some(local_version),
            paths: vec![path.clone()],
            operation: layrs_core::projection::OfflineOperation::CompareContent,
        })
        .unwrap();
    assert_eq!(differences.len(), 1, "Ready absence differs from the local capture");
    assert!(reasons(completed).is_empty());
    original.require(store, cancel);

    timing.next("c14-local-restore-sourceless-reopen");
    let reopened = ProjectionStore::open(&completed.root).unwrap();
    assert_eq!(reopened.state().unwrap(), state);
    assert_eq!(
        reopened.latest_captured_patch().unwrap().as_ref(),
        Some(&restored_version.capture)
    );
    // Every overwritten image remains readable from its own capture.
    require_bytes(&reopened, &inverse.capture, path, DIRTY_BYTES);
    require_bytes(&reopened, &resumed.capture, path, DIRTY_BYTES);
    require_bytes(&reopened, first, path, first_bytes);
    let version = ProjectionVersion::Capture {
        worktree_id: reopened.identity().worktree_id.clone(),
        sequence: inverse.capture.sequence,
        digest: inverse.capture.digest.clone(),
    };
    let mut bytes = Vec::new();
    reopened.read_version_file(&version, path, &mut bytes).unwrap();
    assert_eq!(bytes, DIRTY_BYTES);
    let terminal_error =
        crate::projection::provider::managed_ready_reopen::DenialTerminalResolution::open(
            &completed.root,
            cancel,
        )
        .err()
        .expect("terminal-only resolution must refuse a local Restore suffix");
    assert!(!terminal_error.to_string().is_empty());
    let after = b"C14 Capture after local Restore";
    fs::write(&target, after).unwrap();
    let next = reopened.capture_patch(generation).unwrap();
    assert_eq!(next.sequence, restored_version.capture.sequence + 1);
    require_bytes(&reopened, &next, path, after);
    original.require(&reopened, cancel);
    assert!(!completed.root.join("secret.txt").exists());
    if has_companion {
        assert!(!completed.root.join("README.md").exists());
    }
}
