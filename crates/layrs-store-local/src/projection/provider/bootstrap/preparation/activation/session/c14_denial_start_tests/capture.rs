//! Public local Capture on a genuine C14 admission after B is unavailable.
//! The explicit writable absence is part of the bootstrap server fixture.
use super::*;
use crate::projection::{
    capture::{CaptureFaults, CapturePoint, CapturedPatch},
    format,
    provider::{admitted_ordinary, admitted_ordinary::AdmittedOrdinaryData},
    shared::provider_access::FamilyAccess,
    ProjectionStore, RestoreSource,
};
use super::restore::forged_ready_version;
use crate::SessionMode;
use layrs_core::projection::ProjectionVersion;

pub(super) struct Immutable {
    pub(super) data: AdmittedOrdinaryData,
    private: CatalogSnapshot,
    family: CatalogSnapshot,
    format: (String, u64, String),
}

impl Immutable {
    pub(super) fn read(store: &ProjectionStore, cancel: &Cancellation) -> Box<Self> {
        let (data, private) = format::access(store, SessionMode::Shared)
            .unwrap()
            .run(|access| {
                let data = admitted_ordinary::inspect(&access.connection, &store.marker, None)?;
                let tables = table_snapshot(&access.connection)?;
                let private = tables
                    .into_iter()
                    .filter(|(name, _)| {
                        name.starts_with("projection_provider_")
                            || matches!(
                                name.as_str(),
                                "projection_managed_admission"
                                    | "projection_import_checkout"
                                    | "projection_import_representations"
                                    | "projection_import_segments"
                                    | "projection_snapshots"
                                    | "projection_selections"
                                    | "projection_observations"
                                    | "projection_head"
                            )
                    })
                    .collect();
                Ok((data, private))
            })
            .unwrap();
        let family = FamilyAccess::open(&store.marker.shared.as_ref().unwrap().root).unwrap();
        let retained = database_io::read_cancellable_strict(&family.catalog, cancel, |c| {
            let mut result = BTreeMap::new();
            for (name, sql, key) in [
                (
                    "root",
                    "SELECT * FROM roots WHERE id=?1 ORDER BY id",
                    data.root_id(),
                ),
                (
                    "root_manifests",
                    "SELECT * FROM root_manifests WHERE root_id=?1 ORDER BY manifest_id",
                    data.root_id(),
                ),
                (
                    "reservation",
                    "SELECT * FROM reservations WHERE id=?1 ORDER BY id",
                    data.binding().reservation.as_str(),
                ),
                (
                    "exact_installations",
                    "SELECT * FROM exact_installations WHERE reservation=?1 ORDER BY manifest_id",
                    data.binding().reservation.as_str(),
                ),
            ] {
                let mut statement = c.prepare(sql)?;
                let columns = statement.column_count();
                let rows = statement
                    .query_map([key], |row| {
                        (0..columns)
                            .map(|column| row.get::<_, rusqlite::types::Value>(column))
                            .collect()
                    })?
                    .collect::<rusqlite::Result<Vec<Vec<rusqlite::types::Value>>>>()?;
                result.insert(name.to_owned(), rows);
            }
            Ok(result)
        })
        .and_then(super::super::super::managed_destination::valid)
        .unwrap();
        assert_eq!(retained["root"].len(), 1);
        assert_eq!(retained["root_manifests"].len(), data.required().len());
        Box::new(Self {
            data,
            private,
            family: retained,
            format: database_io::tests::support::witness(&store.root.join(".layrs/FORMAT")),
        })
    }

    pub(super) fn require(&self, store: &ProjectionStore, cancel: &Cancellation) {
        let fresh = Self::read(store, cancel);
        self.data.require_same_import(&fresh.data).unwrap();
        assert_eq!(
            fresh.private, self.private,
            "Capture changed immutable C14 provenance"
        );
        assert_eq!(
            fresh.family, self.family,
            "Capture changed import union or funding"
        );
        assert_eq!(fresh.format, self.format);
    }
}

pub(super) fn require_bytes(store: &ProjectionStore, patch: &CapturedPatch, path: &RepoPath, bytes: &[u8]) {
    assert_eq!(store.captured_patch(patch.sequence).unwrap(), *patch);
    let mut streamed = Vec::new();
    store
        .write_captured_file(patch.sequence, path, &mut streamed)
        .unwrap();
    assert_eq!(streamed, bytes);
    let manifest = store.captured_file_manifest(patch.sequence, path).unwrap();
    let verified = store.verify_captured_file(patch.sequence, path).unwrap();
    assert_eq!(verified.manifest(), &manifest);
    let mut chunks = Vec::new();
    for ordinal in 0..manifest.chunks().len() {
        store
            .read_verified_captured_chunk(&verified, ordinal, &mut chunks)
            .unwrap();
    }
    assert_eq!(chunks, bytes);
}

fn settle_capture_budget(store: &ProjectionStore) {
    // A later Capture entry legitimately settles its predecessor's committed
    // budget before scanning. Complete that existing entry first so refusal
    // witnesses below measure only the operation under test.
    format::access_for_local_capture(store)
        .unwrap()
        .run(|access| access.verify())
        .unwrap();
}

struct FailCapture(CapturePoint);
impl CaptureFaults for FailCapture {
    fn check(&self, point: CapturePoint) -> std::io::Result<()> {
        if point == self.0 {
            Err(std::io::Error::other("injected C14 Capture boundary"))
        } else {
            Ok(())
        }
    }
}

const AFTER_COMMIT_BYTES: &[u8] = b"C14 Capture committed before caller acknowledgement";

fn require_capture_recovery(
    completed: &admission::Completed,
    store: &ProjectionStore,
    original: &Immutable,
    previous: &CapturedPatch,
    path: &RepoPath,
    cancel: &Cancellation,
) -> CapturedPatch {
    let generation = previous.projection_generation;
    let target = completed.root.join(path.as_str());
    fs::write(&target, b"C14 Capture fails before private commit").unwrap();
    let failure = store
        .capture_patch_with_faults(generation, &FailCapture(CapturePoint::BeforeCommit))
        .unwrap_err();
    assert!(failure
        .to_string()
        .contains("injected C14 Capture boundary"));
    assert_eq!(
        store.latest_captured_patch().unwrap().as_ref(),
        Some(previous)
    );
    original.require(store, cancel);

    // The old image is no longer available. Cancellation must release only
    // this operation's reservation, without adopting the new checkout bytes.
    let changed = b"C14 changed image after failed Capture";
    fs::write(&target, changed).unwrap();
    let failure = store.capture_patch(generation).unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("shared quota reservation pending"),
        "{failure}"
    );
    store.cancel_shared_reservation().unwrap();
    assert_eq!(fs::read(&target).unwrap(), changed);
    original.require(store, cancel);
    let resumed = store.capture_patch(generation).unwrap();
    assert_eq!(resumed.sequence, previous.sequence + 1);
    require_bytes(store, &resumed, path, changed);
    assert!(!ProjectionStore::inspect_recovery(&completed.root)
        .unwrap()
        .recovery
        .contains(&crate::projection::ProjectionRecoveryReason::Reservation));

    // A caller error after COMMIT preserves the accepted capture. The bound
    // recovery API settles its committed reservation and remains replayable.
    fs::write(&target, AFTER_COMMIT_BYTES).unwrap();
    let failure = store
        .capture_patch_with_faults(generation, &FailCapture(CapturePoint::AfterCommit))
        .unwrap_err();
    assert!(failure
        .to_string()
        .contains("injected C14 Capture boundary"));
    let committed = store.latest_captured_patch().unwrap().unwrap();
    assert_eq!(committed.sequence, resumed.sequence + 1);
    require_bytes(store, &committed, path, AFTER_COMMIT_BYTES);
    let inspection = ProjectionStore::inspect_recovery(&completed.root).unwrap();
    assert!(inspection
        .recovery
        .contains(&crate::projection::ProjectionRecoveryReason::Reservation));
    let recovery =
        ProjectionStore::open_bound_recovery(&completed.root, &inspection.binding).unwrap();
    recovery.cancel_shared_reservation().unwrap();
    let witness = database_io::tests::support::witness;
    let private = witness(&completed.destination_database);
    let family = witness(&completed.family_database);
    recovery.cancel_shared_reservation().unwrap();
    assert_eq!(witness(&completed.destination_database), private);
    assert_eq!(witness(&completed.family_database), family);
    assert_eq!(
        store.latest_captured_patch().unwrap().as_ref(),
        Some(&committed)
    );
    assert_eq!(fs::read(&target).unwrap(), AFTER_COMMIT_BYTES);
    original.require(store, cancel);
    committed
}

fn require_closed_operations(
    completed: &admission::Completed,
    store: &ProjectionStore,
    state: &crate::projection::VersionedProjection,
    selected: &layrs_core::projection::ProjectionSelection,
    patch: &CapturedPatch,
    path: &RepoPath,
) {
    let witness = database_io::tests::support::witness;
    let private = witness(&completed.destination_database);
    let family = witness(&completed.family_database);
    let error = format::access(store, SessionMode::Exclusive)
        .err()
        .expect("generic C14 Exclusive must remain closed");
    assert!(
        error
            .to_string()
            .contains("admitted denial coverage remains read-only"),
        "{error}"
    );
    assert!(store
        .record_version_observation(&state.snapshot, selected)
        .is_err());
    assert!(store
        .refresh_projection(state.generation, &state.snapshot, &mut |_, _| panic!(
            "closed refresh must not request bytes"
        ))
        .is_err());
    assert!(store
        .expand_selection(
            state.generation,
            selected,
            &state.snapshot,
            &mut |_, _| panic!("closed selection must not request bytes")
        )
        .is_err());
    let request = AvailabilityRequest {
        version: ProjectionVersion::Capture {
            worktree_id: store.identity().worktree_id.clone(),
            sequence: patch.sequence,
            digest: patch.digest.clone(),
        },
        other: None,
        paths: vec![path.clone()],
        operation: OfflineOperation::Read,
    };
    assert!(store.begin_preparation(&request).is_err());
    // Local Restore is open only for the post-Ready suffix: a remote version
    // and the imported/Ready captures stay closed, before any effect.
    let version = store
        .preview_restore(
            state.generation,
            &RestoreSource::Version {
                version: forged_ready_version(store),
                paths: vec![path.clone()],
            },
        )
        .unwrap_err();
    assert!(
        version
            .to_string()
            .contains("C14 local Restore accepts only the fixed Ready version, post-Ready Capture or local inverse sources"),
        "{version}"
    );
    let imported = store
        .preview_restore(
            state.generation,
            &RestoreSource::Capture {
                sequence: 1,
                paths: vec![path.clone()],
            },
        )
        .unwrap_err();
    assert!(
        imported
            .to_string()
            .contains("C14 local Restore source precedes fixed Ready capture frontier"),
        "{imported}"
    );
    assert_eq!(witness(&completed.destination_database), private);
    assert_eq!(witness(&completed.family_database), family);
    assert_eq!(store.state().unwrap(), *state);
}

pub(super) fn run(
    completed: &admission::Completed,
    coverage: &DerivedDeniedCoverage,
    cancel: &Cancellation,
) {
    let mut timing = child::phase("c14-local-capture-open");
    let store = ProjectionStore::open(&completed.root).unwrap();
    let state = store.state().unwrap();
    let selected = store.selection().unwrap();
    let path = RepoPath::parse("recovery.txt").unwrap();
    let entry = state
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap();
    assert_eq!(entry.state, EntryState::Absent);
    assert!(entry.writable && entry.target.is_none());
    assert!(selected.includes(&path));
    let readonly = state
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "unchanged.txt")
        .unwrap();
    assert!(!readonly.writable && readonly.state == EntryState::Materialized);
    let readonly_bytes = fs::read(completed.root.join("unchanged.txt")).unwrap();
    let original = Immutable::read(&store, cancel);
    assert_eq!(original.data.admission_digest(), completed.digest);
    assert_eq!(original.data.receipt(), &completed.receipt);
    assert_eq!(original.data.ready_generation(), state.generation);
    assert_eq!(
        original.data.ready().plan.denial_coverage.as_ref(),
        Some(&coverage.record().reference().unwrap())
    );
    assert_eq!(original.data.required().len(), completed.required.len());
    assert!(!completed.root.join("secret.txt").exists());
    let has_companion = completed.receipt.companion_id.is_some();
    if has_companion {
        let readme = state
            .snapshot
            .entries
            .iter()
            .find(|entry| entry.path.as_str() == "README.md")
            .unwrap();
        assert_eq!(readme.state, EntryState::Reserved);
        assert!(!readme.writable);
        assert!(!completed.root.join("README.md").exists());
    }
    timing.next("c14-local-capture-first");
    let first_bytes = b"C14 authorized absent creation A";
    fs::write(completed.root.join(path.as_str()), first_bytes).unwrap();
    let first = store.capture_patch(state.generation).unwrap();
    assert_eq!(first.identity, *store.identity());
    assert_eq!(first.projection_generation, state.generation);
    assert_eq!(first.projection_digest, state.digest);
    assert_eq!(first.sequence, original.data.ready_capture_sequence() + 1);
    assert!(first.changes.iter().any(|change| change.path == path
        && change.expected.is_none()
        && change.replacement.is_some()));
    assert!(first
        .changes
        .iter()
        .all(|change| change.path.as_str() != "secret.txt"
            && (!has_companion || change.path.as_str() != "README.md")));
    assert_eq!(
        store.capture_patch(state.generation).unwrap(),
        first,
        "same image must deduplicate"
    );
    require_bytes(&store, &first, &path, first_bytes);
    original.require(&store, cancel);

    timing.next("c14-local-capture-readonly-refusal");
    settle_capture_budget(&store);
    let witness = database_io::tests::support::witness;
    let private_before = witness(&completed.destination_database);
    let family_before = witness(&completed.family_database);
    fs::write(
        completed.root.join("unchanged.txt"),
        b"C14 forbidden readonly edit",
    )
    .unwrap();
    let error = store.capture_patch(state.generation).unwrap_err();
    assert!(
        error.to_string().contains("read-only projection file"),
        "{error}"
    );
    assert_eq!(
        fs::read(completed.root.join("unchanged.txt")).unwrap(),
        b"C14 forbidden readonly edit"
    );
    assert_eq!(witness(&completed.destination_database), private_before);
    assert_eq!(witness(&completed.family_database), family_before);
    // Restore only this expected-negative input so the next positive case has
    // the original readonly image. The failed operation itself preserves bytes.
    fs::write(completed.root.join("unchanged.txt"), &readonly_bytes).unwrap();

    timing.next("c14-local-capture-second");
    let second_bytes = b"C14 authorized same-generation edit B";
    fs::write(completed.root.join(path.as_str()), second_bytes).unwrap();
    let second = store.capture_patch(state.generation).unwrap();
    assert_eq!(second.sequence, first.sequence + 1);
    assert_eq!(second.previous_patch_digest.as_ref(), Some(&first.digest));
    assert_eq!(second.identity, first.identity);
    assert_eq!(second.projection_generation, state.generation);
    assert_eq!(second.projection_digest, state.digest);
    assert_eq!(store.capture_patch(state.generation).unwrap(), second);
    require_bytes(&store, &first, &path, first_bytes);
    require_bytes(&store, &second, &path, second_bytes);
    original.require(&store, cancel);
    settle_capture_budget(&store);
    // The physical terminal resolver checks the complete family first. The
    // accepted Capture roots therefore reject this terminal-only entry before
    // its later private-table inventory check; both are intentionally strict.
    let terminal_error =
        crate::projection::provider::managed_ready_reopen::DenialTerminalResolution::open(
            &completed.root,
            cancel,
        )
        .err()
        .expect("terminal-only resolution must refuse a mutable Capture suffix");
    assert!(
        terminal_error
            .to_string()
            .contains("exact protection owner has additional roots"),
        "{terminal_error}"
    );
    require_closed_operations(completed, &store, &state, &selected, &second, &path);
    drop(store);

    timing.next("c14-local-capture-sourceless-reopen");
    let store = ProjectionStore::open(&completed.root).unwrap();
    assert_eq!(store.state().unwrap(), state);
    assert_eq!(store.selection().unwrap(), selected);
    assert_eq!(store.latest_captured_patch().unwrap().unwrap(), second);
    require_bytes(&store, &first, &path, first_bytes);
    require_bytes(&store, &second, &path, second_bytes);
    original.require(&store, cancel);
    timing.next("c14-local-capture-failure-cancellation");
    let recovered = require_capture_recovery(completed, &store, &original, &second, &path, cancel);
    if has_companion {
        timing.next("c14-local-capture-reserved-local-bytes-refusal");
        settle_capture_budget(&store);
        let private_before = witness(&completed.destination_database);
        let family_before = witness(&completed.family_database);
        fs::write(
            completed.root.join("README.md"),
            b"C14 denied bytes still present locally",
        )
        .unwrap();
        let error = store.capture_patch(state.generation).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("reserved projection path exists on disk"),
            "{error}"
        );
        assert_eq!(
            fs::read(completed.root.join("README.md")).unwrap(),
            b"C14 denied bytes still present locally"
        );
        assert_eq!(witness(&completed.destination_database), private_before);
        assert_eq!(witness(&completed.family_database), family_before);
        assert_eq!(store.latest_captured_patch().unwrap().unwrap(), recovered);
        original.require(&store, cancel);
    }
    assert_eq!(
        fs::read(completed.root.join("unchanged.txt")).unwrap(),
        readonly_bytes
    );
    assert_eq!(
        fs::read(completed.root.join(path.as_str())).unwrap(),
        AFTER_COMMIT_BYTES
    );
    if has_companion {
        // Only the expected-negative input is removed; Restore then runs on
        // the same authorized image, with README still Reserved and absent.
        fs::remove_file(completed.root.join("README.md")).unwrap();
    }
    // Local Restore reuses this one expensive C14 admission and its suffix.
    super::restore::run(
        completed,
        &store,
        &original,
        &path,
        &first,
        &recovered,
        has_companion,
        cancel,
        &mut timing,
    );
}

#[test]
#[ignore = "UNQUALIFIED native C14 local Capture+Restore plain; ROOT review/release serial child5400 outer5460 required"]
fn c14_plain_local_capture_retains_versions_after_source_database_quarantine() {
    child::run(
        concat!(
            module_path!(),
            "::c14_plain_local_capture_retains_versions_after_source_database_quarantine"
        ),
        // r204 with local Restore phases: 2364s.
        5400,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::ContentPlainCapture),
            );
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED native C14 local Capture+Restore companion; ROOT review/release serial child5400 outer5460 required"]
fn c14_companion_local_capture_preserves_denied_work_after_source_database_quarantine() {
    child::run(
        concat!(
            module_path!(),
            "::c14_companion_local_capture_preserves_denied_work_after_source_database_quarantine"
        ),
        // r204 with local Restore phases: 3537s of the former 3600s budget.
        5400,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::ContentCapture),
            );
        },
    );
}
