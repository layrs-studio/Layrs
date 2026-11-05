//! Native ACK refusal after a real admission COMMIT, followed by destination-
//! only resolution after source disconnect and release of every setup holder.
//! Ordinary reopen/finalization is separate from the retained-holder ACK test.
//! No process crash, source deletion or cleanup is tested.
use super::*;
use crate::{
    lock::StoreLock,
    projection::{
        SessionMode, format,
        provider::{
            admitted_ordinary::{self, AdmittedOrdinaryData},
            database_io::{self, DatabaseFile, ReadState, WriterCloseObserver, WriterOpenPoint},
            managed_ready_reopen::{AdmissionStep, ManagedReadyAdmission},
            process_profile::tests::child,
            records::transition::{TransitionRole, TransitionState},
        },
        shared::{admitted_catalog::AdmittedFamilyContext, provider_access::FamilyAccess},
    },
};
use rusqlite::ffi;
use std::{fs, path::Path};

fn witnesses(
    destination: &Path,
    family: &Path,
) -> (
    std::collections::BTreeMap<std::path::PathBuf, super::managed_ready_reopen_tests::Node>,
    std::collections::BTreeMap<std::path::PathBuf, super::managed_ready_reopen_tests::Node>,
) {
    let marker = format::read_marker(destination).unwrap();
    let owner = marker
        .shared
        .as_ref()
        .unwrap()
        .metadata_path
        .join("layrs.lock");
    (
        super::managed_ready_reopen_tests::tree_with_owner_lock(destination, &owner),
        super::managed_ready_reopen_tests::tree_with_owner_lock(family, &owner),
    )
}

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
    refuse_acknowledgement: bool,
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, _: *mut ffi::sqlite3) -> CauseResult<()> {
        if point == WriterOpenPoint::BeforeRaw {
            self.opens += 1;
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "admission recipe requires successful physical close"
        );
        self.closes += 1;
        self.refuse_acknowledgement
            .then(|| invalid("owned admission acknowledgement withheld after positive close"))
    }
}

fn valid<T>(state: ReadState<T>) -> T {
    match state {
        ReadState::Valid(value) => value,
        ReadState::Rejected { cause, .. }
        | ReadState::ReadButUnavailable(cause)
        | ReadState::Unavailable(cause) => panic!("owned admission RO read unavailable: {cause}"),
        ReadState::UnboundRollback { trigger, cause } => {
            panic!("owned admission unexpected unbound rollback: {trigger}; {cause}")
        }
        ReadState::Hot(_) => panic!("owned admission unexpectedly has a hot journal"),
        ReadState::Uninitialized => panic!("owned admission database unexpectedly empty"),
    }
}

/// Caller retains the genuine destination owner/lifecycle: either the opaque
/// ManagedReadyAdmission failure, or explicit fresh shared scopes below. The
/// helper acquires no owner recursively and opens no ordinary store or source.
fn inspect_under_owner(root: &Path, cancel: &Cancellation) -> AdmittedOrdinaryData {
    let marker = format::read_marker(root).unwrap();
    let shared = marker.shared.as_ref().unwrap();
    let family = FamilyAccess::open(&shared.root).unwrap();
    let _lifecycle = family.lifecycle_lease().unwrap();
    let private = DatabaseFile::open(
        shared.metadata_path.join("projection.sqlite3"),
        &shared.database_identity,
    )
    .unwrap();
    let read = || {
        valid(
            database_io::read_cancellable(&private, cancel, |c| {
                admitted_ordinary::inspect(c, &marker, Some(cancel))
            })
            .unwrap(),
        )
    };
    let data = read(); // private snapshot and positive close precede catalog gate
    let context = AdmittedFamilyContext::new(&data).unwrap();
    {
        let _catalog = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .unwrap();
        let manifests = valid(
            database_io::read_cancellable(&family.catalog, cancel, |c| {
                let proof = context.validate_catalog(c, family.marker_bytes(), Some(cancel))?;
                Ok(proof
                    .manifests()
                    .iter()
                    .map(|m| (m.file_id().to_string(), m.id().to_string(), m.byte_length()))
                    .collect::<Vec<_>>())
            })
            .unwrap(),
        );
        assert_eq!(manifests.len(), 2);
        assert_eq!(manifests[0].0, manifests[1].0);
        assert_ne!(manifests[0].1, manifests[1].1);
        let mut expected: Vec<_> = data
            .required()
            .iter()
            .map(|m| (m.file_id.to_string(), m.manifest_id.to_string(), m.bytes))
            .collect();
        expected.sort();
        let mut manifests = manifests;
        manifests.sort();
        assert_eq!(manifests, expected);
    }
    data.require_same_import(&read()).unwrap();
    private.verify().unwrap();
    family.verify().unwrap();
    data
}

fn version_under_owner(root: &Path, cancel: &Cancellation) -> u32 {
    let marker = format::read_marker(root).unwrap();
    let shared = marker.shared.as_ref().unwrap();
    let private = DatabaseFile::open(
        shared.metadata_path.join("projection.sqlite3"),
        &shared.database_identity,
    )
    .unwrap();
    let version = valid(
        database_io::read_cancellable(&private, cancel, |c| {
            Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?)
        })
        .unwrap(),
    );
    private.verify().unwrap();
    version
}

#[test]
#[ignore = "UNQUALIFIED physical managed admission ACK refusal/source-disconnected resolve; ROOT exact source review and owned child240 / outer300 required"]
fn managed_admission_commit_ack_refusal_resolves_after_source_disconnect_without_writer() {
    child::run(
        concat!(
            module_path!(),
            "::managed_admission_commit_ack_refusal_resolves_after_source_disconnect_without_writer"
        ),
        240,
        || {
            child::declare();
            let mut retained = None;
            let mut close = Close::default();
            super::managed_staging_tests::with_content_planned_bytes_then(
                b"provider base content",
                |_, session, attempt, planned| {
                    super::managed_ready_reopen_tests::publish_destination(
                        session, attempt, planned,
                    )
                },
                |_, session, destination| {
                    let cancel = Cancellation::default();
                    let marker = format::read_marker(&destination).unwrap();
                    let family_root = marker.shared.as_ref().unwrap().root.clone();
                    let held = ManagedReadyAdmission::open(&destination, &cancel).unwrap();
                    assert_eq!(version_under_owner(&destination, &cancel), 8);
                    let before_cancel = witnesses(&destination, &family_root);
                    let cancelled = Cancellation::default();
                    cancelled.cancel();
                    let withheld = session
                        .admit_ready_destination(held, &mut close, &cancelled)
                        .expect_err("pre-admission cancellation must refuse");
                    assert_eq!(withheld.step, AdmissionStep::Validating);
                    assert_eq!((close.opens, close.closes), (0, 0));
                    assert_eq!(version_under_owner(&destination, &cancel), 8);
                    assert_eq!(witnesses(&destination, &family_root), before_cancel);
                    close.refuse_acknowledgement = true;
                    let failure = session
                        .admit_ready_destination(withheld.held, &mut close, &cancel)
                        .expect_err("after-close observer must withhold admission ACK");
                    assert_eq!(failure.step, AdmissionStep::CommitReturned);
                    assert!(
                        failure
                            .cause
                            .to_string()
                            .contains("acknowledgement withheld")
                    );
                    assert_eq!((close.opens, close.closes), (1, 1));
                    assert_eq!(version_under_owner(&destination, &cancel), 9);
                    let data = inspect_under_owner(&destination, &cancel);
                    assert_eq!(data.required().len(), 2);
                    assert_eq!(data.required()[0].file_id, data.required()[1].file_id);
                    assert_ne!(
                        data.required()[0].manifest_id,
                        data.required()[1].manifest_id
                    );
                    assert_eq!(data.ready().role, TransitionRole::Destination);
                    assert_eq!(data.ready().state, TransitionState::Ready);
                    assert_eq!(data.source_published().role, TransitionRole::Source);
                    assert_eq!(data.source_published().state, TransitionState::Published);
                    let after_commit = witnesses(&destination, &family_root);
                    retained = Some((failure.held, destination, family_root, data, after_commit));
                },
            );
            // This line is after helper return, explicit Session::disconnect and
            // drop of session/source/allocation/checkpoint/old transfer holders.
            // Only independent destination pins from the failure remain live.
            let (held, destination, family_root, committed, after_commit) =
                retained.expect("failure must retain independent destination authority");
            let cancel = Cancellation::default();
            let digest = held.resolve(&cancel).unwrap_or_else(|failure| {
                panic!(
                    "source-free resolution {:?}: {}",
                    failure.step, failure.cause
                )
            });
            assert_eq!(digest, committed.admission_digest());
            assert_eq!((close.opens, close.closes), (1, 1));
            let marker = format::read_marker(&destination).unwrap();
            let shared = marker.shared.as_ref().unwrap();
            let family = FamilyAccess::open(&shared.root).unwrap();
            let _lifecycle = family.lifecycle_lease().unwrap();
            let _owner = StoreLock::acquire_existing(
                &shared.metadata_path.join("layrs.lock"),
                SessionMode::Shared,
            )
            .unwrap();
            committed
                .require_same_import(&inspect_under_owner(&destination, &cancel))
                .unwrap();
            assert_eq!(witnesses(&destination, &family_root), after_commit);
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED ordinary Admitted9 reopen/finalization; ROOT source release, owned child1800 / outer1860 required"]
fn managed_admitted_ordinary_reopen_finalizes_success_errors_and_unfinished_transactions() {
    // Diagnostic bound, not a measured duration for this recipe: r47 native
    // recipes sharing this admitted setup took 611.529-785.795s. Allow over
    // twice that maximum plus the repeated ordinary reopen/cleanup checks,
    // retaining the owned child's finite kill-and-reap boundary.
    child::run(
        concat!(
            module_path!(),
            "::managed_admitted_ordinary_reopen_finalizes_success_errors_and_unfinished_transactions"
        ),
        1800,
        || {
            let started = std::time::Instant::now();
            let checkpoint = |step: &str| {
                eprintln!(
                    "ADMITTED_ORDINARY_REOPEN_CHECKPOINT elapsed={:.3}s step={step}",
                    started.elapsed().as_secs_f64()
                );
            };
            checkpoint("declare:begin");
            child::declare();
            checkpoint("declare:done");
            let mut destination = None;
            checkpoint("setup:begin");
            super::managed_staging_tests::with_content_planned_bytes_then(
                b"provider base content",
                |_, session, attempt, planned| {
                    checkpoint("setup:source-plan-ready");
                    checkpoint("setup:publish-destination:begin");
                    let root = super::managed_ready_reopen_tests::publish_destination(
                        session, attempt, planned,
                    );
                    checkpoint("setup:publish-destination:done");
                    root
                },
                |_, session, root| {
                    checkpoint("setup:capture-lane-released");
                    let cancel = Cancellation::default();
                    checkpoint("admission:open:begin");
                    let held = ManagedReadyAdmission::open(&root, &cancel).unwrap();
                    checkpoint("admission:open:done");
                    let mut close = Close::default();
                    checkpoint("admission:commit:begin");
                    let digest = session
                        .admit_ready_destination(held, &mut close, &cancel)
                        .unwrap_or_else(|failure| {
                            panic!("admission {:?}: {}", failure.step, failure.cause)
                        });
                    assert_eq!((close.opens, close.closes), (1, 1));
                    checkpoint("admission:commit-and-close:done");
                    destination = Some((root, digest));
                },
            );
            checkpoint("setup:holders-released");
            let (root, digest) = destination.unwrap();
            checkpoint("baseline:markers-and-trees:begin");
            let marker = format::read_marker(&root).unwrap();
            let family = marker.shared.as_ref().unwrap().root.clone();
            let before = witnesses(&root, &family);
            checkpoint("baseline:markers-and-trees:done");
            checkpoint("ordinary:open:begin");
            let store = crate::projection::ProjectionStore::open(&root).unwrap();
            checkpoint("ordinary:open:done");
            checkpoint("ordinary:initial-state:begin");
            let state = store.state().unwrap();
            checkpoint("ordinary:initial-state:done");
            checkpoint("stale-capture:begin");
            assert!(matches!(store.capture_patch(state.generation + 1),
            Err(crate::projection::ProjectionStoreError::Stale { expected, actual })
                if expected == state.generation + 1 && actual == state.generation));
            checkpoint("stale-capture:returned");
            assert_eq!(
                store.state().unwrap(),
                state,
                "early error still positively closes"
            );
            checkpoint("stale-capture:fresh-state-verified");
            checkpoint("unfinished-transaction:access:begin");
            let unfinished =
                format::access(&store, SessionMode::Exclusive)
                    .unwrap()
                    .run(|access| {
                        checkpoint("unfinished-transaction:body");
                        access.connection.execute_batch("BEGIN IMMEDIATE")?;
                        Ok(())
                    });
            checkpoint("unfinished-transaction:returned");
            assert!(
                unfinished.is_err(),
                "unfinished transaction cannot be acknowledged"
            );
            assert_eq!(
                store.state().unwrap(),
                state,
                "unfinished transaction rolled back and closed"
            );
            checkpoint("unfinished-transaction:fresh-state-verified");
            checkpoint("operation-error:access:begin");
            let failed: crate::projection::Result<()> =
                format::access(&store, SessionMode::Exclusive)
                    .unwrap()
                    .run(|access| {
                        checkpoint("operation-error:body");
                        access.connection.execute_batch("BEGIN IMMEDIATE")?;
                        Err(crate::projection::invalid(
                            "owned ordinary operation failure",
                        ))
                    });
            checkpoint("operation-error:returned");
            assert!(
                failed
                    .unwrap_err()
                    .to_string()
                    .contains("owned ordinary operation failure")
            );
            // An enclosing operation can fail after a separately owned close.
            // Its cleanup must not start fresh admission validation SQL. Make the
            // uncommitted admission invalid so ordinary finish() would add a
            // validation error; the narrow error-only path must preserve exactly
            // the operation error while rolling the mutation back and closing.
            checkpoint("error-only-cleanup:access:begin");
            let access = format::access(&store, SessionMode::Exclusive).unwrap();
            checkpoint("error-only-cleanup:access:done");
            access.connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            access
                .connection
                .execute(
                    "UPDATE projection_provider_transitions SET digest=?1",
                    [format!("blake3:{}", "0".repeat(64))],
                )
                .unwrap();
            checkpoint("error-only-cleanup:corrupted-uncommitted-admission");
            checkpoint("error-only-cleanup:close:begin");
            let failed: crate::projection::Result<()> = access.close_failed_operation(
                crate::projection::invalid("enclosing publication operation failed"),
            );
            checkpoint("error-only-cleanup:close:returned");
            assert!(matches!(failed,
            Err(crate::projection::ProjectionStoreError::Invalid(message))
                if message == "enclosing publication operation failed"));
            assert_eq!(
                store.state().unwrap(),
                state,
                "error-only cleanup rolls back and positively closes"
            );
            checkpoint("error-only-cleanup:fresh-state-verified");
            // A deliberately invalid ROLLBACK tests authority invalidation, not an
            // OS rollback failure. Later validation must reject the poisoned token.
            checkpoint("poisoned-scope:access:begin");
            let mut access = format::access(&store, SessionMode::Exclusive).unwrap();
            checkpoint("poisoned-scope:access:done");
            let failed = access.with_content(|c, scope| -> crate::projection::Result<()> {
                checkpoint("poisoned-scope:body");
                assert!(c.is_autocommit());
                let rollback = scope.rollback_transaction(c).unwrap_err();
                assert!(
                    scope
                        .verify(c, &store)
                        .unwrap_err()
                        .to_string()
                        .contains("failed rollback")
                );
                checkpoint("poisoned-scope:invalid-rollback-rejected");
                Err(rollback)
            });
            checkpoint("poisoned-scope:close:begin");
            let failed: crate::projection::Result<()> =
                access.close_failed_operation(failed.unwrap_err());
            checkpoint("poisoned-scope:close:returned");
            assert!(matches!(
                failed,
                Err(crate::projection::ProjectionStoreError::Database(_))
            ));
            assert_eq!(
                store.state().unwrap(),
                state,
                "fresh preflight remains independent of poisoned clones"
            );
            checkpoint("poisoned-scope:fresh-state-verified");
            drop(store);
            checkpoint("ordinary:reopen:begin");
            let reopened = crate::projection::ProjectionStore::open(&root).unwrap();
            checkpoint("ordinary:reopen:done");
            assert_eq!(reopened.state().unwrap(), state);
            checkpoint("ordinary:reopened-state-verified");
            drop(reopened);
            checkpoint("admission-digest:reopen-and-access:begin");
            let access = format::access(
                &crate::projection::ProjectionStore::open(&root).unwrap(),
                SessionMode::Shared,
            )
            .unwrap();
            checkpoint("admission-digest:access:done");
            access
                .run(|access| {
                    checkpoint("admission-digest:inspect:begin");
                    let data = admitted_ordinary::inspect(&access.connection, &marker, None)?;
                    assert_eq!(data.admission_digest(), digest);
                    checkpoint("admission-digest:inspect:done");
                    Ok(())
                })
                .unwrap();
            checkpoint("admission-digest:close:done");
            checkpoint("final-trees:begin");
            assert_eq!(witnesses(&root, &family), before);
            checkpoint("final-trees:verified");
            checkpoint("ordinary-observation:begin");
            let observed_store = crate::projection::ProjectionStore::open(&root).unwrap();
            let selected = observed_store.selection().unwrap();
            let observation = observed_store
                .record_version_observation(&state.snapshot, &selected)
                .unwrap();
            assert!(matches!(
                observation,
                layrs_core::projection::ProjectionVersion::Remote { .. }
            ));
            assert_eq!(observed_store.state().unwrap(), state);
            drop(observed_store);
            let after_observation = crate::projection::ProjectionStore::open(&root).unwrap();
            assert_eq!(after_observation.state().unwrap(), state);
            assert_eq!(after_observation.selection().unwrap(), selected);
            // Replay must read the committed observation without locking the store.
            assert_eq!(
                after_observation
                    .record_version_observation(&state.snapshot, &selected)
                    .unwrap(),
                observation
            );
            drop(after_observation);
            checkpoint("ordinary-observation:committed-reopened-and-replayed");
            checkpoint("recipe:done");
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED ordinary Admitted9 mutable cycle; ROOT source release, owned child900 / outer960 required"]
fn managed_admitted_ordinary_edit_capture_restore_without_source() {
    // r202: legacy Capture cancellation phases pushed the cycle past 480s
    // (second restore began at ~459s); the budget, not the protocol, failed.
    child::run(
        concat!(
            module_path!(),
            "::managed_admitted_ordinary_edit_capture_restore_without_source"
        ),
        900,
        || {
            use crate::projection::{ProjectionStore, RestoreSource};
            use layrs_core::{
                projection::{AvailabilityRequest, OfflineOperation, ProjectionVersion},
                scm::RepoPath,
            };
            child::declare();
            let mut destination = None;
            super::managed_staging_tests::with_content_planned_bytes_then(
                b"provider base content",
                |_, session, attempt, planned| {
                    super::managed_ready_reopen_tests::publish_destination(
                        session, attempt, planned,
                    )
                },
                |fixture, session, root| {
                    let cancel = Cancellation::default();
                    let held = ManagedReadyAdmission::open(&root, &cancel).unwrap();
                    let mut close = Close::default();
                    let digest = session
                        .admit_ready_destination(held, &mut close, &cancel)
                        .unwrap_or_else(|failure| {
                            panic!("admission {:?}: {}", failure.step, failure.cause)
                        });
                    assert_eq!((close.opens, close.closes), (1, 1));
                    destination = Some((root, digest, fixture.checkout.clone()));
                },
            );
            // Every source/session/checkpoint holder has gone. Move only this fresh
            // owned fixture checkout aside: the original source path is unavailable.
            // Preserve its bytes for investigation; no recursive deletion or reuse.
            let (root, digest, source) = destination.unwrap();
            let parent =
                std::path::absolute(std::env::var_os("LAYRS_VFS_TEST_PARENT").unwrap()).unwrap();
            let source = std::path::absolute(source).unwrap();
            let unavailable = source.with_extension("source-unavailable");
            assert!(source.starts_with(&parent) && source != parent);
            assert!(unavailable.starts_with(&parent) && unavailable != parent);
            assert!(!root.starts_with(&source) && !unavailable.exists());
            fs::rename(&source, &unavailable).unwrap();
            assert!(!source.exists() && unavailable.is_dir());

            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let original_version = store.current_version().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            let (baseline, exact) = format::access(&store, SessionMode::Shared)
                .unwrap()
                .run(|access| {
                    assert!(
                        crate::projection::capture::scan::scan(
                            &store,
                            &access.connection,
                            &state.snapshot,
                            false
                        )
                        .is_err(),
                        "an unscoped caller cannot admit private9"
                    );
                    let data = admitted_ordinary::inspect(&access.connection, &store.marker, None)?;
                    let file = data
                        .checkout()
                        .files()
                        .iter()
                        .find(|file| file.path == path)
                        .unwrap();
                    let manifest: String = access.connection.query_row(
                        "SELECT manifest_id FROM projection_content WHERE file_id=?1",
                        [file.file_id.as_str()],
                        |r| r.get(0),
                    )?;
                    let manifest = layrs_core::scm::FileManifestId::parse(manifest)
                        .map_err(crate::projection::invalid)?;
                    Ok((data, manifest))
                })
                .unwrap();
            assert_eq!(baseline.admission_digest(), digest);
            assert_eq!(
                fs::read(root.join(path.as_str())).unwrap(),
                b"provider base content"
            );
            let request = AvailabilityRequest {
                version: original_version.clone(),
                other: None,
                paths: vec![path.clone()],
                operation: OfflineOperation::Read,
            };
            let coverage = store.availability(&request).unwrap();
            assert!(coverage.available);
            assert_eq!(coverage.paths[0].manifest_id.as_ref(), Some(&exact));
            let preparation = store.begin_preparation(&request).unwrap();
            let prepared = store
                .resume_preparation(&preparation.id, &mut |_, _, _| {
                    panic!("retained exact content must not reacquire from a source")
                })
                .unwrap();
            assert_eq!(prepared.status, "ready");
            let coverage = store.availability(&request).unwrap();
            assert!(coverage.available && coverage.prepared);
            assert_eq!(coverage.paths[0].manifest_id.as_ref(), Some(&exact));

            // Legacy Capture follows the actual mutable head, rather than the
            // immutable import Ready generation used by the C14-only route.
            let mut advanced = state.snapshot.clone();
            advanced.observed_authorization_epoch = advanced
                .observed_authorization_epoch
                .checked_add(1)
                .unwrap();
            let state = store.compare_and_swap(state.generation, &advanced).unwrap();
            assert!(state.generation > baseline.ready_generation());
            assert_ne!(state.digest, baseline.ready().plan.placement_digest);

            struct BeforeCaptureCommit;
            impl crate::projection::capture::CaptureFaults for BeforeCaptureCommit {
                fn check(
                    &self,
                    point: crate::projection::capture::CapturePoint,
                ) -> std::io::Result<()> {
                    if point == crate::projection::capture::CapturePoint::BeforeCommit {
                        Err(std::io::Error::other(
                            "injected legacy Capture before COMMIT",
                        ))
                    } else {
                        Ok(())
                    }
                }
            }
            let previous = store.latest_captured_patch().unwrap();
            fs::write(
                root.join(path.as_str()),
                b"legacy Capture interrupted input",
            )
            .unwrap();
            let failure = store
                .capture_patch_with_faults(state.generation, &BeforeCaptureCommit)
                .unwrap_err();
            assert!(
                failure
                    .to_string()
                    .contains("injected legacy Capture before COMMIT"),
                "{failure}"
            );
            assert_eq!(store.latest_captured_patch().unwrap(), previous);
            let changed_input = b"legacy Capture input changed after interruption";
            fs::write(root.join(path.as_str()), changed_input).unwrap();
            let failure = store.capture_patch(state.generation).unwrap_err();
            assert!(
                failure
                    .to_string()
                    .contains("shared quota reservation pending"),
                "{failure}"
            );
            store.cancel_shared_reservation().unwrap();
            assert_eq!(fs::read(root.join(path.as_str())).unwrap(), changed_input);
            assert_eq!(store.latest_captured_patch().unwrap(), previous);
            let private_database = store
                .marker
                .shared
                .as_ref()
                .unwrap()
                .metadata_path
                .join("projection.sqlite3");
            let family_database = store
                .marker
                .shared
                .as_ref()
                .unwrap()
                .root
                .join("metadata/catalog.sqlite3");
            let witness = database_io::tests::support::witness;
            let private_after_cancel = witness(&private_database);
            let family_after_cancel = witness(&family_database);
            store.cancel_shared_reservation().unwrap();
            assert_eq!(witness(&private_database), private_after_cancel);
            assert_eq!(witness(&family_database), family_after_cancel);

            let first_bytes = b"ordinary destination edit A: retained after source removal";
            fs::write(root.join(path.as_str()), first_bytes).unwrap();
            let first = store.capture_patch(state.generation).unwrap();
            assert_eq!(first.identity, *store.identity());
            assert_eq!(
                store.capture_patch(state.generation).unwrap(),
                first,
                "same capture deduplicates"
            );
            let mut bytes = Vec::new();
            store
                .write_captured_file(first.sequence, &path, &mut bytes)
                .unwrap();
            assert_eq!(bytes, first_bytes);
            let verified = store.verify_captured_file(first.sequence, &path).unwrap();
            let mut chunks = Vec::new();
            for ordinal in 0..verified.manifest().chunks().len() {
                store
                    .read_verified_captured_chunk(&verified, ordinal, &mut chunks)
                    .unwrap();
            }
            assert_eq!(chunks, first_bytes);
            let first_version = ProjectionVersion::Capture {
                worktree_id: store.identity().worktree_id.clone(),
                sequence: first.sequence,
                digest: first.digest.clone(),
            };
            let second_bytes = b"ordinary destination edit B: preserve before restore";
            fs::write(root.join(path.as_str()), second_bytes).unwrap();
            let second = store.capture_patch(state.generation).unwrap();
            assert_eq!(second.sequence, first.sequence + 1);
            let comparison = AvailabilityRequest {
                version: first_version.clone(),
                other: Some(ProjectionVersion::Capture {
                    worktree_id: store.identity().worktree_id.clone(),
                    sequence: second.sequence,
                    digest: second.digest.clone(),
                }),
                paths: vec![path.clone()],
                operation: OfflineOperation::CompareContent,
            };
            assert_eq!(store.compare_versions(&comparison).unwrap().len(), 1);
            drop(store);

            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(store.captured_patch(first.sequence).unwrap(), first);
            assert_eq!(store.latest_captured_patch().unwrap().unwrap(), second);
            let preview = store
                .preview_restore(
                    state.generation,
                    &RestoreSource::Capture {
                        sequence: first.sequence,
                        paths: vec![path.clone()],
                    },
                )
                .unwrap();
            let restored = store.apply_restore(&preview).unwrap();
            assert_eq!(fs::read(root.join(path.as_str())).unwrap(), first_bytes);
            let inverse = store
                .preview_restore(
                    state.generation,
                    &RestoreSource::Inverse {
                        operation_id: restored.operation_id,
                    },
                )
                .unwrap();
            store.apply_restore(&inverse).unwrap();
            assert_eq!(fs::read(root.join(path.as_str())).unwrap(), second_bytes);
            let preview = store
                .preview_restore(
                    state.generation,
                    &RestoreSource::Version {
                        version: original_version,
                        paths: vec![path.clone()],
                    },
                )
                .unwrap();
            store.apply_restore(&preview).unwrap();
            assert_eq!(
                fs::read(root.join(path.as_str())).unwrap(),
                b"provider base content"
            );
            let mut immutable = Vec::new();
            store
                .read_version_file(&first_version, &path, &mut immutable)
                .unwrap();
            assert_eq!(immutable, first_bytes);
            format::access(&store, SessionMode::Shared)
                .unwrap()
                .run(|access| {
                    let after =
                        admitted_ordinary::inspect(&access.connection, &store.marker, None)?;
                    baseline.require_same_import(&after)?;
                    access.with_content(|c, scope| {
                        crate::projection::retention::fsck_with_scope(c, &store, Some(scope))
                    })?;
                    Ok(())
                })
                .unwrap();
            drop(store);
            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(store.state().unwrap(), state);
            assert_eq!(store.captured_patch(first.sequence).unwrap(), first);
            assert!(
                !source.exists(),
                "ordinary cycle did not recreate the source"
            );
        },
    );
}
