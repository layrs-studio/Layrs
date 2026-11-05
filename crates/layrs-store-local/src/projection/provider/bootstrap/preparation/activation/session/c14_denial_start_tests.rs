//! Real retained Provider owner and two database commits. Observation is local
//! fixture DATA, not a remote lease. Withheld ACK is not an OS close failure.
//! Required files need a verified current producer or resident CAS bytes.
//! Current ImmutableSource files must themselves be resident before start;
//! preparation does not acquire their missing bytes.
use super::super::managed_journal::denial_recovery::{self, DenialRecoveryEffect};
use super::*;
use crate::projection::provider::{
    bootstrap::denial_receipt,
    bootstrap::tests::{capture_family, fixture::Fixture, transition_capture},
    capture_lock::CaptureAttemptLock,
    database_io::{
        self, WriterCloseObserver, WriterOpenPoint, tests::fresh_ordinary_support::with_observer,
    },
    family_upgrade::upgrade_source_family,
    import_validation::{self, ImportDatabaseKind},
    process_profile::tests::child,
    records::{
        ProviderBootstrapSource, ProviderSourceDescriptor, Record,
        denied_coverage::DerivedDeniedCoverage, import::ImportSourceReferences,
        transition::ProviderTransitionIntentRecord,
    },
    transition_observation::{
        AuthorityObservationData, DeniedPathData, ObservationCoverageData, SourceObservationData,
        TransitionObservationData,
    },
    transition_sql::SourceIntentEffect,
    validation::PrivateActiveCaptureValidation,
};
use layrs_core::{
    projection::{AvailabilityRequest, EntryState, OfflineOperation, ProjectionEntry},
    scm::{Chunk, ChunkingDescriptor, FileId, FileManifest, RepoPath, TreeEntryTarget, WorktreeId},
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[path = "c14_denial_start_tests/admission.rs"]
mod admission;
#[path = "c14_denial_start_tests/allocation_arm.rs"]
mod allocation_arm;
#[path = "c14_denial_start_tests/capture.rs"]
mod capture;
#[path = "c14_denial_start_tests/content.rs"]
mod content;
#[path = "c14_denial_start_tests/content_plain.rs"]
mod content_plain;
#[path = "c14_denial_start_tests/prospective_capacity_arm.rs"]
mod prospective_capacity_arm;
#[path = "c14_denial_start_tests/recovery_refusals.rs"]
mod recovery_refusals;
#[path = "c14_denial_start_tests/restore.rs"]
mod restore;
#[path = "c14_denial_start_tests/staging.rs"]
mod staging;

struct RangeWatch {
    source: Arc<LocalSource>,
    funded: Arc<AtomicBool>,
    reads: AtomicUsize,
    late: AtomicUsize,
}
impl SnapshotSource for RangeWatch {
    fn catalog(&self) -> &Catalog {
        self.source.catalog()
    }
    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        output: &mut [u8],
        cancel: &Cancellation,
    ) -> layrs_vfs::Result<()> {
        if self.funded.load(Ordering::SeqCst) {
            self.late.fetch_add(1, Ordering::SeqCst);
            return Err(layrs_vfs::Error::NeedsRepair(
                "C14 source range-reader after B",
            ));
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.source.read_exact_at(id, offset, output, cancel)
    }
}

#[derive(Clone, Copy)]
enum PreimageCase {
    CurrentAlias,
    Nonresident,
    DescriptorOnly,
}

#[derive(Clone, Copy)]
enum RecoveryCase {
    Orphan,
    ActualPlanned,
    ReleaseAck,
    Reject(recovery_refusals::Case),
}

impl RecoveryCase {
    fn has_actual_intent(self) -> bool {
        matches!(
            self,
            Self::ActualPlanned | Self::Reject(recovery_refusals::Case::ConflictingActualIntent)
        )
    }
}

#[derive(Clone, Copy)]
enum AllocationCase {
    Publish,
    WithheldCopyingAck,
    Copy(CopyCase),
    Unsettled,
    AlteredPlanned,
    LegacyRoutes,
    AlteredCopying,
    ImageChanged,
    ProspectiveCapacity,
    Stage(staging::Case),
    Content,
    ContentPlain,
    ContentPlainRetry,
    ContentPlainAdmission,
    ContentAdmission,
    ContentPlainCapture,
    ContentCapture,
}

impl AllocationCase {
    fn captures_locally(self) -> bool {
        matches!(self, Self::ContentPlainCapture | Self::ContentCapture)
    }
}

#[derive(Clone, Copy)]
enum CopyCase {
    Publish,
    WithheldFamilyAck,
    WithheldDestinationAck,
}

type CatalogSnapshot = BTreeMap<String, Vec<Vec<rusqlite::types::Value>>>;

#[derive(Default)]
struct Close {
    opened: usize,
    closed: usize,
    refused: Option<Rc<Cell<bool>>>,
    funded: Option<Arc<AtomicBool>>,
    refuse_at: Option<usize>,
}
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut rusqlite::ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "recipe requires successful SQLite connection close"
        );
        self.closed += 1;
        if let Some(funded) = &self.funded {
            funded.store(true, Ordering::SeqCst);
        }
        if self.refuse_at.is_some_and(|ordinal| ordinal != self.closed) {
            return None;
        }
        self.refused.as_ref().map(|flag| {
            flag.set(true);
            invalid("C14 start SQLite close ACK withheld")
        })
    }
    fn open_point(
        &mut self,
        point: WriterOpenPoint,
        _: *mut rusqlite::ffi::sqlite3,
    ) -> CauseResult<()> {
        if point == WriterOpenPoint::BeforeRaw {
            self.opened += 1;
        }
        Ok(())
    }
}

fn run_start_configuration(
    retry: bool,
    pending_capture: bool,
    recovery: Option<RecoveryCase>,
    preimage_case: PreimageCase,
    allocation_case: Option<AllocationCase>,
) {
    let cancel = Cancellation::default();
    let missing_preimage = matches!(preimage_case, PreimageCase::Nonresident);
    let local_capture = allocation_case.is_some_and(AllocationCase::captures_locally);
    let seed_source = source_for_configuration(preimage_case, local_capture);
    let source = source_for_configuration(preimage_case, local_capture);
    let (f, holder, allocation, prepared, binding) =
        capture_family::active_holder_with_source(Some(source));
    drop(holder);
    // Public acquisition supplies fixed1MiB representations before the lease.
    // Successful modes declare that exact preimage before bootstrap.
    let seed_root = f.outer.join("c14-resident-source");
    fs::create_dir(&seed_root).unwrap();
    let seed = f
        .family
        .create_worktree(
            &seed_root,
            WorktreeId::parse("worktree:c14-resident-source").unwrap(),
            seed_source.initial().clone(),
            seed_source.selection().clone(),
        )
        .unwrap();
    let request = AvailabilityRequest {
        version: seed.current_version().unwrap(),
        other: None,
        paths: ["README.md", "unchanged.txt"]
            .into_iter()
            .filter(|path| !missing_preimage || *path != "README.md")
            .map(|path| RepoPath::parse(path).unwrap())
            .collect(),
        operation: OfflineOperation::Read,
    };
    let preparation = seed.begin_preparation(&request).unwrap();
    seed.resume_preparation(&preparation.id, &mut |_, path, output| {
        output.write_all(match path.as_str() {
            "README.md" => b"provider base content",
            "unchanged.txt" if !matches!(preimage_case, PreimageCase::CurrentAlias) => {
                b"C14 distinct immutable bytes"
            }
            "unchanged.txt" => b"provider base content",
            _ => panic!("unexpected C14 seed path"),
        })
    })
    .unwrap();
    drop(seed);
    upgrade_source_family(
        f.family.root(),
        &f.worktree,
        &allocation.transaction_id,
        &mut Close::default(),
        &cancel,
    )
    .unwrap();
    let holder = crate::projection::shared::PartialStoreFamily::open_provider_active(
        f.family.root(),
        &f.worktree,
        &binding.transaction_id,
        &binding,
    )
    .unwrap();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|e| panic!("session: {}", e.cause()));
    session.connect().unwrap();
    let path = f.checkout.join("README.md");
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"C14 current native image").unwrap();
    let held = &session.source.held;
    let attempt = CaptureAttemptLock::acquire(&held.artifacts, &held.family).unwrap();
    let active = {
        let _locks = held
            .family
            .locks(&allocation.format.identity.worktree_id, false)
            .unwrap();
        capture_state::read(held, Some(&session.source.root), None)
            .unwrap_or_else(|e| panic!("active: {}", e.cause))
    };
    let range_watch = RangeWatch {
        source: session.source.clone(),
        funded: Arc::new(AtomicBool::new(false)),
        reads: AtomicUsize::new(0),
        late: AtomicUsize::new(0),
    };
    let mut checkpoint = session.session.export_checkpoint().unwrap();
    let mut image = super::super::managed_image::complete::plan(
        &mut checkpoint,
        &range_watch,
        &active.active.source,
        &active.active.activated.snapshot,
        &cancel,
    )
    .unwrap();
    assert!(
        range_watch.reads.load(Ordering::SeqCst) > 0,
        "current ImmutableSource was never read"
    );
    assert!(
        image
            .retained()
            .files()
            .iter()
            .any(|file| file.path.as_str() == "unchanged.txt"
                && matches!(
                    &file.manifest,
                    super::super::managed_image::ManagedManifest::Source(_)
                ))
    );
    // Authenticate the allocation parent before deriving any immutable plan.
    // Allocation modes stop at FORMAT with empty destination SQL.
    let mut planned =
        transition_capture::planned_for_image(&f, &allocation, &prepared, &binding, image.digest());
    let plan_parent = if matches!(allocation_case, Some(AllocationCase::ProspectiveCapacity)) {
        let path = f.outer.join("r1-destination-parent");
        fs::create_dir(&path).unwrap();
        path
    } else {
        f.outer.clone()
    };
    let parent = session.session.guard_export_parent(&plan_parent).unwrap();
    planned.plan.destination_parent = parent.root().to_str().unwrap().into();
    planned.plan.destination_parent_identity = parent.filesystem_identity();
    drop(parent);
    let observed = observe_with_reserved_image(
        &active,
        &allocation.format.identity,
        &planned,
        1,
        matches!(
            allocation_case,
            Some(
                AllocationCase::Content
                    | AllocationCase::ContentAdmission
                    | AllocationCase::ContentCapture
            )
        ),
        &cancel,
    );
    let coverage = DerivedDeniedCoverage::from_observation(
        &planned.plan.source_prefix,
        &planned.transaction_id,
        &observed,
        Some(&cancel),
    )
    .unwrap();
    let body = coverage.record();
    planned.schema_version = "layrs.projection-provider-transition-intent.v2".into();
    planned.plan.denial_coverage = Some(body.reference().unwrap());
    planned.plan.observation.step_id = body.step_id.clone();
    planned.plan.observation.root_tree_id = body.root_tree_id.clone();
    planned.plan.observation.policy_set_id = observed.snapshot().observed_policy_set_id.clone();
    planned.plan.observation.authorization_epoch = observed.snapshot().observed_authorization_epoch;
    planned.plan.observation.generation = body.observation_generation;
    planned.plan.observation.snapshot_digest = body.observation_snapshot_digest.clone();
    planned.plan.observation.selection_digest = body.observation_selection_digest.clone();
    planned.encode().unwrap();
    let mut all = BTreeMap::new();
    for manifest in active
        .active
        .source
        .descriptors()
        .iter()
        .map(|d| &d.manifest)
        .chain(
            image
                .retained()
                .files()
                .iter()
                .map(|file| file.manifest.manifest()),
        )
    {
        all.insert(
            (manifest.file_id().clone(), manifest.id().clone()),
            manifest.clone(),
        );
    }
    let supplied = all.into_values().collect::<Vec<_>>();
    assert_eq!(
        supplied.len(),
        3,
        "historical preimage, immutable representation and current native image"
    );
    let private_path = f.metadata.join("projection.sqlite3");
    let family_path = f.family.root().join("metadata/catalog.sqlite3");
    let candidate_witnesses = if matches!(preimage_case, PreimageCase::CurrentAlias) {
        Some((
            database_io::tests::support::witness(&private_path),
            database_io::tests::support::witness(&family_path),
        ))
    } else {
        None
    };
    let candidate = {
        let _locks = held
            .family
            .locks(&allocation.format.identity.worktree_id, false)
            .unwrap();
        database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
                let proof = validation::private_active_capture_with_source(
                    c,
                    &allocation,
                    &prepared,
                    &binding,
                    Some(session.source.root.registration().root_id()),
                    Some(&cancel),
                )?;
                let closure = import_validation::verify_source(
                    c,
                    &allocation.format,
                    ImportDatabaseKind::Provider,
                    &planned.plan.source_prefix,
                    ImportSourceReferences::Provider {
                        allocation: &allocation,
                        prepared: &prepared,
                        activation: &proof.active.activation,
                    },
                    Some(&proof),
                    image.digest(),
                    Some(&cancel),
                )?;
                let admitted = super::super::managed_journal::denial_plan::derive_candidate(
                    c,
                    &allocation,
                    &closure,
                    &proof,
                    &image,
                    &observed,
                    &planned,
                    &coverage,
                    &supplied,
                    &cancel,
                )?;
                let (baseline_quota, minimum_metadata) = crate::projection::provider::transition_reconcile::source_sql::with_capture_source(
                    c, &allocation.format, &planned.plan.source_prefix, &closure, &proof, &observed,
                    Some(&cancel), |source| {
                        let owner = image.reconcile_capture_retained(source, &observed, &proof, &supplied, 0, &cancel)?;
                        let baseline = super::super::managed_plan::derive_reconciled_denial_data(
                            &planned.plan.source_prefix, &owner, &observed,
                            planned.plan.source_prefix.destination_identity.clone(),
                            &planned.plan.destination_parent, &planned.plan.destination_parent_identity,
                            &planned.plan.destination_leaf_nonce, &planned.transaction_id, &coverage,
                        )?;
                        let baseline_quota = baseline.quota_bytes;
                        let mut prospective = ProviderTransitionIntentRecord::decode(&planned.encode()?)?;
                        prospective.plan = baseline.plan;
                        let cost = crate::projection::provider::managed_budget::DeniedCoverageCost::new(
                            &prospective, coverage.record(), Some(&cancel))?;
                        let minimum = crate::projection::provider::managed_budget::prospective_denial_metadata_floor(
                            closure.metadata_bytes(c, &planned.plan.source_prefix, Some(&cancel))?,
                            &baseline.observation, &baseline.placement, &baseline.capture, &prospective,
                            observed.selection(), owner.manifests(), &cost, Some(&cancel),
                        )?;
                        Ok((baseline_quota, minimum))
                    },
                )?;
                Ok((admitted, baseline_quota, minimum_metadata))
            })
            .and_then(super::super::managed_destination::valid)
    };
    if let Some((private_before, family_before)) = candidate_witnesses {
        require_validation_message(
            candidate
                .err()
                .expect("ambiguous original descriptors admitted"),
            "original source has ambiguous descriptor representation",
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert!(!f.outer.join(&planned.plan.destination_leaf_nonce).exists());
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
        assert!(!range_watch.funded.load(Ordering::SeqCst));
        drop(image);
        drop(checkpoint);
        drop(range_watch);
        drop(attempt);
        session.disconnect().unwrap();
        return;
    }
    let (admitted, baseline_quota, minimum_metadata) = candidate.unwrap();
    // Copy DATA through its production wire codec; the closed result grants
    // no SQL/source authority after its strict read scope ends.
    planned.plan = serde_json::from_slice(&serde_json::to_vec(admitted.plan()).unwrap()).unwrap();
    let required = admitted.manifests().to_vec();
    assert_eq!(required, supplied);
    assert!(baseline_quota > 0 && minimum_metadata > 0);
    assert_eq!(planned.plan.quota_bytes, baseline_quota + minimum_metadata);
    assert_eq!(planned.plan.quota_bytes - baseline_quota, minimum_metadata);
    assert_eq!(planned.plan.protected_manifests.count, 3);
    planned.encode().unwrap();
    if matches!(
        allocation_case,
        Some(
            AllocationCase::ContentPlain
                | AllocationCase::ContentPlainRetry
                | AllocationCase::ContentPlainAdmission
                | AllocationCase::ContentPlainCapture
        )
    ) {
        assert!(planned.plan.companion_id.is_none());
        assert_eq!(planned.plan.companion_files.count, 0);
        assert_eq!(planned.plan.companion_deletions.count, 0);
        assert!(
            admitted.companion_cost().is_none(),
            "plain denial must not preflight a source companion s"
        );
        let mut timing = child::phase("c14-plain-publication");
        let completed = content_plain::run(
            &f,
            &session,
            &attempt,
            &planned,
            &coverage,
            &observed,
            &mut image,
            &required,
            minimum_metadata,
            &range_watch,
            matches!(allocation_case, Some(AllocationCase::ContentPlainRetry)),
            &cancel,
        );
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
        drop(image);
        drop(checkpoint);
        drop(range_watch);
        drop(attempt);
        timing.next("c14-plain-admission-call");
        let admitted = matches!(
            allocation_case,
            Some(AllocationCase::ContentPlainAdmission | AllocationCase::ContentPlainCapture)
        )
        .then(|| {
            admission::commit(
                &f,
                &session,
                &completed.root,
                &completed.receipt,
                &required,
                &cancel,
            )
        });
        timing.next("c14-plain-source-release");
        session.disconnect().unwrap();
        drop(session);
        drop(allocation);
        drop(prepared);
        drop(binding);
        if let Some(admitted) = admitted {
            drop(completed);
            drop(f);
            timing.next("c14-plain-terminal-call");
            if local_capture {
                admission::require_reopened_without_source_then(
                    admitted,
                    &coverage,
                    &cancel,
                    |completed| capture::run(completed, &coverage, &cancel),
                );
            } else {
                admission::require_reopened_without_source(admitted, &coverage, &cancel);
            }
        } else {
            content_plain::require_reopened(&completed, &coverage, &cancel);
        }
        return;
    }
    if matches!(
        allocation_case,
        Some(
            AllocationCase::Content
                | AllocationCase::ContentAdmission
                | AllocationCase::ContentCapture
        )
    ) {
        let completed = content::run(
            &f,
            &session,
            &attempt,
            &planned,
            &coverage,
            &observed,
            &mut image,
            &required,
            minimum_metadata,
            admitted
                .companion_cost()
                .expect("Content plan must have prospective s"),
            &range_watch,
            &cancel,
        );
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
        drop(image);
        drop(checkpoint);
        drop(range_watch);
        drop(attempt);
        let admitted = matches!(
            allocation_case,
            Some(AllocationCase::ContentAdmission | AllocationCase::ContentCapture)
        )
        .then(|| {
            admission::commit(
                &f,
                &session,
                &completed.root,
                &completed.receipt,
                &required,
                &cancel,
            )
        });
        session.disconnect().unwrap();
        drop(session);
        drop(allocation);
        drop(prepared);
        drop(binding);
        if let Some(admitted) = admitted {
            drop(completed);
            drop(f);
            if local_capture {
                admission::require_reopened_without_source_then(
                    admitted,
                    &coverage,
                    &cancel,
                    |completed| capture::run(completed, &coverage, &cancel),
                );
            } else {
                admission::require_reopened_without_source(admitted, &coverage, &cancel);
            }
        }
        return;
    }
    let mut zero_quota =
        ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
    zero_quota.plan.quota_bytes = 0;
    let mut below_floor =
        ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
    below_floor.plan.quota_bytes -= 1;
    assert_eq!(
        below_floor.plan.quota_bytes - baseline_quota,
        minimum_metadata - 1
    );
    assert!(
        below_floor.plan.quota_bytes >= minimum_metadata,
        "the negative must pass an incorrect total-quota versus metadata-floor check"
    );
    let mut changed_rowset =
        ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
    let original = changed_rowset.plan.protected_manifests.digest.clone();
    changed_rowset.plan.protected_manifests.digest = format!(
        "blake3:{}",
        if original.ends_with('0') { "1" } else { "0" }.repeat(64)
    );
    changed_rowset.encode().unwrap();
    let changed_observation = observe(&active, &allocation.format.identity, &planned, 2, &cancel);
    let mut omitted = required.clone();
    let preimage = &active
        .active
        .source
        .descriptors()
        .iter()
        .find(|row| row.path.as_str() == "README.md")
        .unwrap()
        .manifest;
    let immutable = &active
        .active
        .source
        .descriptors()
        .iter()
        .find(|row| row.path.as_str() == "unchanged.txt")
        .unwrap()
        .manifest;
    assert_eq!(
        preimage.chunking(),
        &ChunkingDescriptor::deterministic_fixed(1048576).unwrap()
    );
    assert_ne!(preimage.file_id(), immutable.file_id());
    assert!(
        image
            .retained()
            .files()
            .iter()
            .all(|file| file.manifest.manifest().file_id() != preimage.file_id())
    );
    omitted.retain(|manifest| manifest != preimage);
    assert_eq!(omitted.len(), required.len() - 1);
    for (label, candidate, observation, manifests, expected) in [
        (
            "zero quota",
            &zero_quota,
            &observed,
            required.as_slice(),
            "denial C metadata quota is insufficient",
        ),
        (
            "metadata envelope one byte below floor",
            &below_floor,
            &observed,
            required.as_slice(),
            "denial C metadata quota is insufficient",
        ),
        (
            "changed derived protected-manifest digest",
            &changed_rowset,
            &observed,
            required.as_slice(),
            "denial plan differs from complete source and observation",
        ),
        (
            "omitted preimage",
            &planned,
            &observed,
            omitted.as_slice(),
            "owned companion manifest union cardinality differs",
        ),
        (
            "changed observation",
            &planned,
            &changed_observation,
            required.as_slice(),
            "denied coverage differs from complete normalized observation",
        ),
    ] {
        let private_before = database_io::tests::support::witness(&private_path);
        let family_before = database_io::tests::support::witness(&family_path);
        let error = super::super::managed_journal::denial_start::PendingDenialStart::prepare(
            held,
            session
                .session
                .guard_export_parent(std::path::Path::new(&candidate.plan.destination_parent))
                .unwrap(),
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            candidate,
            &coverage,
            observation,
            &mut image,
            manifests,
            &cancel,
        )
        .err()
        .unwrap_or_else(|| panic!("{label} admitted denial funding"));
        require_validation_message(error, expected);
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before,
            "{label}"
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before,
            "{label}"
        );
    }
    if missing_preimage {
        let private_before = database_io::tests::support::witness(&private_path);
        let family_before = database_io::tests::support::witness(&family_path);
        let error = super::super::managed_journal::denial_start::PendingDenialStart::prepare(
            held,
            session
                .session
                .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
                .unwrap(),
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &planned,
            &coverage,
            &observed,
            &mut image,
            &required,
            &cancel,
        )
        .err()
        .expect("missing preimage bytes admitted before B");
        match error {
            Cause::Io(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
            error => panic!("expected missing historical CAS object, got {error}"),
        }
        assert!(!image.retained_offline_manifests().contains(preimage));
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
        assert!(!range_watch.funded.load(Ordering::SeqCst));
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
        drop(image);
        drop(checkpoint);
        drop(range_watch);
        drop(attempt);
        session.disconnect().unwrap();
        return;
    }
    if pending_capture {
        use crate::projection::provider::records::capture::{
            ProviderCaptureStage, ProviderCaptureState, ProviderCaptureTransactionRecord,
        };
        // Release the export checkpoint before taking the real capture lane's
        // checkpoint. The production begin/transition writers create this gap.
        drop(image);
        drop(checkpoint);
        let mut capture_checkpoint = session.session.checkpoint().unwrap();
        let tx = ProviderCaptureTransactionRecord::begin(
            &allocation,
            &session.source.record,
            &session.source.activation,
            "c14-durable-staging".into(),
            0,
            None,
        )
        .unwrap();
        let mut close = Close::default();
        super::super::capture_sql::begin(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &tx,
            &mut close,
        )
        .unwrap();
        let proposed = super::super::capture_image::plan(
            &mut capture_checkpoint,
            &active.active.source,
            &active.active.activated.snapshot,
            &cancel,
        )
        .unwrap();
        assert!(!proposed.changes.is_empty());
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-provider-capture-reservation-v1\0");
        hash.update(tx.owner_identity_digest.as_bytes());
        hash.update(b"\0");
        hash.update(tx.transaction_id.as_bytes());
        let mut staged = tx.clone();
        staged.state = ProviderCaptureState::Staging {
            stage: ProviderCaptureStage {
                image_digest: proposed.image_digest,
                image_path_count: proposed.image_path_count,
                changes: proposed.changes,
                reservation: format!("capture-{}", hash.finalize().to_hex()),
            },
        };
        super::super::capture_sql::transition(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &tx,
            &staged,
            &capture_checkpoint,
            &mut close,
        )
        .unwrap();
        assert_eq!((close.opened, close.closed), (2, 2));
        drop(capture_checkpoint);
        let mut checkpoint = session.session.export_checkpoint().unwrap();
        let mut image = super::super::managed_image::complete::plan(
            &mut checkpoint,
            &range_watch,
            &active.active.source,
            &active.active.activated.snapshot,
            &cancel,
        )
        .unwrap();
        super::super::managed_destination::valid(
            database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
                let state = validation::private_active_transition_with_source(
                    c,
                    &allocation,
                    &prepared,
                    &binding,
                    Some(session.source.root.registration().root_id()),
                    Some(&cancel),
                )?;
                assert!(
                    state
                        .capture
                        .transactions
                        .iter()
                        .any(|record| record.transaction_id == tx.transaction_id
                            && matches!(&record.state, ProviderCaptureState::Staging { .. }))
                );
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();
        let private_before = database_io::tests::support::witness(&private_path);
        let family_before = database_io::tests::support::witness(&family_path);
        let result = super::super::managed_journal::denial_start::PendingDenialStart::prepare(
            held,
            session
                .session
                .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
                .unwrap(),
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &planned,
            &coverage,
            &observed,
            &mut image,
            &required,
            &cancel,
        );
        let error = match result {
            Ok(_) => panic!("pending capture admitted denial start"),
            Err(error) => error,
        };
        require_validation_message(
            error,
            "source transition pending head or capture state differs",
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        drop(image);
        drop(checkpoint);
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
        drop(range_watch);
        drop(attempt);
        session.disconnect().unwrap();
        return;
    }
    if matches!(allocation_case, Some(AllocationCase::ProspectiveCapacity)) {
        prospective_capacity_arm::run(
            preimage_case,
            &cancel,
            &f,
            &allocation,
            &prepared,
            &binding,
            &session,
            &attempt,
            &range_watch,
            &mut image,
            &planned,
            &plan_parent,
            &observed,
            &coverage,
            &private_path,
            &family_path,
            &required,
            preimage,
            immutable,
        );
        drop(image);
        drop(checkpoint);
        drop(range_watch);
        drop(attempt);
        session.disconnect().unwrap();
        return;
    }
    let private_before_prepare = database_io::tests::support::witness(&private_path);
    let family_before_prepare = database_io::tests::support::witness(&family_path);
    let mut pending = super::super::managed_journal::denial_start::PendingDenialStart::prepare(
        held,
        session
            .session
            .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
            .unwrap(),
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &planned,
        &coverage,
        &observed,
        &mut image,
        &required,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before_prepare,
        "exact baseline+metadata-floor admission changed private SQL"
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before_prepare,
        "exact baseline+metadata-floor admission funded B during prepare"
    );
    if let Some(case) = recovery {
        let baseline = catalog_snapshot(&f, &cancel);
        let funding =
            crate::projection::shared::source_denial::SourceDeniedCoverageReservation::new(
                &allocation,
                &planned,
                coverage.record(),
                Some(&cancel),
            )
            .unwrap();
        let private_before = database_io::tests::support::witness(&private_path);
        let refused = Rc::new(Cell::new(false));
        let probes = Rc::new(Cell::new(0));
        let mut family_close = Close::default();
        family_close.funded = Some(range_watch.funded.clone());
        let mut source_close = Close::default();
        if case.has_actual_intent() {
            source_close.refused = Some(refused.clone());
        } else {
            family_close.refused = Some(refused.clone());
        }
        let watch_refused = refused.clone();
        let watch_probes = probes.clone();
        let error = with_observer(
            move |_, _| {
                if watch_refused.get() {
                    watch_probes.set(watch_probes.get() + 1);
                    return Err(invalid("forbidden probe after owner-loss ACK"));
                }
                Ok(())
            },
            || pending.begin(&mut family_close, &mut source_close, &cancel),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("C14 start SQLite close ACK withheld"),
            "{error}"
        );
        assert!(refused.get());
        assert_eq!(probes.get(), 0);
        assert_eq!((family_close.opened, family_close.closed), (1, 1));
        assert_eq!(
            (source_close.opened, source_close.closed),
            if case.has_actual_intent() {
                (1, 1)
            } else {
                (0, 0)
            }
        );
        // A new complete SQL read distinguishes the two real committed gaps.
        super::super::managed_destination::valid(
            database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
                let state = validation::private_active_transition_with_source(
                    c,
                    &allocation,
                    &prepared,
                    &binding,
                    Some(session.source.root.registration().root_id()),
                    Some(&cancel),
                )?;
                if case.has_actual_intent() {
                    assert_eq!(state.transitions.as_slice(), std::slice::from_ref(&planned));
                    assert_eq!(
                        state.capture.denial_reservations(),
                        std::slice::from_ref(&funding)
                    );
                } else {
                    assert!(state.transitions.is_empty());
                    assert!(state.capture.denial_reservations().is_empty());
                }
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();
        let funded = catalog_snapshot(&f, &cancel);
        let mut without_funding = funded.clone();
        let reservations = without_funding.get_mut("reservations").unwrap();
        use rusqlite::types::Value;
        let exact_row = vec![
            Value::Text(funding.operation().into()),
            Value::Text(allocation.format.identity.worktree_id.as_str().into()),
            Value::Text(funding.operation().into()),
            Value::Integer(funding.reservation_bytes().try_into().unwrap()),
        ];
        assert_eq!(
            reservations.iter().filter(|row| **row == exact_row).count(),
            1
        );
        let position = reservations
            .iter()
            .position(|row| *row == exact_row)
            .unwrap();
        reservations.remove(position);
        assert_eq!(
            without_funding, baseline,
            "start changed catalog DATA beyond exact B"
        );
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_some()
        );
        if !case.has_actual_intent() {
            assert_eq!(
                database_io::tests::support::witness(&private_path),
                private_before
            );
        }
        drop(pending);
        require_retained_preimage(&image, preimage_case, preimage, immutable);
        drop(image);
        drop(checkpoint);
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
        drop(range_watch);
        drop(attempt);
        session.disconnect().unwrap();
        drop(session);
        if let RecoveryCase::Reject(rejection) = case {
            recovery_refusals::assert_fresh_refusal(
                &f,
                rejection,
                &allocation,
                &prepared,
                &binding,
                &planned,
                &coverage,
                &funding,
                &cancel,
            );
            return;
        }
        if !case.has_actual_intent() {
            fs::write(&path, b"C14 edited after retained owner loss").unwrap();
            assert!(
                crate::projection::shared::PartialStoreFamily::open_provider_active(
                    f.family.root(),
                    &f.worktree,
                    &binding.transaction_id,
                    &binding,
                )
                .is_err(),
                "ordinary opener adopted unproven B"
            );
        }
        let private_before = database_io::tests::support::witness(&private_path);
        let family_before = database_io::tests::support::witness(&family_path);
        let mut recovery_close = Close::default();
        if matches!(case, RecoveryCase::ReleaseAck) {
            let refused = Rc::new(Cell::new(false));
            let probes = Rc::new(Cell::new(0));
            recovery_close.refused = Some(refused.clone());
            let watch_refused = refused.clone();
            let watch_probes = probes.clone();
            let error = with_observer(
                move |_, _| {
                    if watch_refused.get() {
                        watch_probes.set(watch_probes.get() + 1);
                        return Err(invalid("forbidden probe after release ACK"));
                    }
                    Ok(())
                },
                || {
                    denial_recovery::recover(
                        f.family.root(),
                        &f.worktree,
                        &binding.transaction_id,
                        &binding,
                        &mut recovery_close,
                        &cancel,
                    )
                },
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("C14 start SQLite close ACK withheld"),
                "{error}"
            );
            assert!(refused.get());
            assert_eq!(probes.get(), 0);
            assert_eq!((recovery_close.opened, recovery_close.closed), (1, 1));
            assert!(
                denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                    .unwrap()
                    .is_some()
            );
            assert_eq!(catalog_snapshot(&f, &cancel), baseline);
            let mut retry_close = Close::default();
            assert_eq!(
                denial_recovery::recover(
                    f.family.root(),
                    &f.worktree,
                    &binding.transaction_id,
                    &binding,
                    &mut retry_close,
                    &cancel
                )
                .unwrap(),
                DenialRecoveryEffect::NoReservation
            );
            assert_eq!((retry_close.opened, retry_close.closed), (0, 0));
        } else {
            let expected = if case.has_actual_intent() {
                DenialRecoveryEffect::RetainedActualFunding
            } else {
                DenialRecoveryEffect::Released
            };
            assert_eq!(
                denial_recovery::recover(
                    f.family.root(),
                    &f.worktree,
                    &binding.transaction_id,
                    &binding,
                    &mut recovery_close,
                    &cancel
                )
                .unwrap(),
                expected
            );
            assert_eq!(
                (recovery_close.opened, recovery_close.closed),
                if case.has_actual_intent() {
                    (0, 0)
                } else {
                    (1, 1)
                }
            );
        }
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            catalog_snapshot(&f, &cancel),
            if case.has_actual_intent() {
                funded
            } else {
                baseline
            }
        );
        if case.has_actual_intent() {
            assert_eq!(
                database_io::tests::support::witness(&family_path),
                family_before
            );
        }
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
        let private_before = database_io::tests::support::witness(&private_path);
        let family_before = database_io::tests::support::witness(&family_path);
        let mut close = Close::default();
        assert_eq!(
            denial_recovery::recover(
                f.family.root(),
                &f.worktree,
                &binding.transaction_id,
                &binding,
                &mut close,
                &cancel
            )
            .unwrap(),
            DenialRecoveryEffect::NoReceipt
        );
        assert_eq!((close.opened, close.closed), (0, 0));
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        let reopened = crate::projection::shared::PartialStoreFamily::open_provider_active(
            f.family.root(),
            &f.worktree,
            &binding.transaction_id,
            &binding,
        )
        .unwrap();
        drop(reopened);
        assert_eq!(
            fs::read(&path).unwrap(),
            if case.has_actual_intent() {
                b"C14 current native image".as_slice()
            } else {
                b"C14 edited after retained owner loss".as_slice()
            }
        );
        return;
    }
    if matches!(allocation_case, Some(AllocationCase::Unsettled)) {
        let refused = Rc::new(Cell::new(false));
        let probes = Rc::new(Cell::new(0));
        let mut family_close = Close::default();
        family_close.funded = Some(range_watch.funded.clone());
        let mut source_close = Close::default();
        source_close.refused = Some(refused.clone());
        let watch_refused = refused.clone();
        let watch_probes = probes.clone();
        let error = with_observer(
            move |_, _| {
                if watch_refused.get() {
                    watch_probes.set(watch_probes.get() + 1);
                    return Err(invalid("forbidden probe after unsettled SourcePlanned ACK"));
                }
                Ok(())
            },
            || pending.begin(&mut family_close, &mut source_close, &cancel),
        )
        .unwrap_err();
        require_validation_message(error, "C14 start SQLite close ACK withheld");
        assert!(refused.get());
        assert_eq!(probes.get(), 0);
        assert_eq!((family_close.closed, source_close.closed), (1, 1));
        super::super::managed_destination::valid(
            database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
                let actual = validation::private_active_transition_with_source(
                    c,
                    &allocation,
                    &prepared,
                    &binding,
                    Some(session.source.root.registration().root_id()),
                    Some(&cancel),
                )?;
                assert_eq!(
                    actual.transitions.as_slice(),
                    std::slice::from_ref(&planned)
                );
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();
        let private_before = database_io::tests::support::witness(&private_path);
        let family_before = database_io::tests::support::witness(&family_path);
        let catalog_before = catalog_snapshot(&f, &cancel);
        let target = f.outer.join(&planned.plan.destination_leaf_nonce);
        let error = super::super::managed_allocation::allocate_denial(
            &session.session,
            held,
            &mut pending,
            &planned,
            &cancel,
        )
        .err()
        .expect("unsettled start allocated C");
        assert!(error.effects.is_empty());
        assert_eq!(error.attempted_root, target);
        require_validation_message_ref(
            &error.cause,
            "denial allocation requires acknowledged settled Planned",
        );
        assert!(!target.exists());
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_eq!(catalog_snapshot(&f, &cancel), catalog_before);
        drop(error);
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_some()
        );
        let mut replay_family = Close::default();
        let mut replay_source = Close::default();
        assert_eq!(
            pending
                .begin(&mut replay_family, &mut replay_source, &cancel)
                .unwrap(),
            SourceIntentEffect::AlreadyPresent
        );
        assert_eq!((replay_family.opened, replay_source.opened), (0, 0));
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
    } else if retry {
        for family_ack in [true, false] {
            let private_before = database_io::tests::support::witness(&private_path);
            let family_before = database_io::tests::support::witness(&family_path);
            let refused = Rc::new(Cell::new(false));
            let probes = Rc::new(Cell::new(0));
            let mut family_close = Close::default();
            family_close.funded = Some(range_watch.funded.clone());
            let mut source_close = Close::default();
            if family_ack {
                family_close.refused = Some(refused.clone());
            } else {
                source_close.refused = Some(refused.clone());
            }
            let watch_refused = refused.clone();
            let watch_probes = probes.clone();
            let error = with_observer(
                move |_, _| {
                    if watch_refused.get() {
                        watch_probes.set(watch_probes.get() + 1);
                        return Err(invalid("forbidden probe after C14 withheld ACK"));
                    }
                    Ok(())
                },
                || pending.begin(&mut family_close, &mut source_close, &cancel),
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("C14 start SQLite close ACK withheld"),
                "{error}"
            );
            assert!(refused.get());
            assert_eq!(probes.get(), 0);
            if family_ack {
                assert_eq!((family_close.opened, family_close.closed), (1, 1));
                assert_eq!((source_close.opened, source_close.closed), (0, 0));
                assert_eq!(
                    database_io::tests::support::witness(&private_path),
                    private_before
                );
            } else {
                assert_eq!((family_close.opened, family_close.closed), (0, 0));
                assert_eq!((source_close.opened, source_close.closed), (1, 1));
                assert_eq!(
                    database_io::tests::support::witness(&family_path),
                    family_before
                );
            }
        }
    } else {
        let mut family_close = Close::default();
        family_close.funded = Some(range_watch.funded.clone());
        let mut source_close = Close::default();
        assert_eq!(
            pending
                .begin(&mut family_close, &mut source_close, &cancel)
                .unwrap(),
            SourceIntentEffect::Inserted
        );
        assert_eq!((family_close.opened, family_close.closed), (1, 1));
        assert_eq!((source_close.opened, source_close.closed), (1, 1));
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none(),
            "successful begin retained its compact receipt"
        );
    }
    let private_before = database_io::tests::support::witness(&private_path);
    let family_before = database_io::tests::support::witness(&family_path);
    let mut family_close = Close::default();
    family_close.funded = Some(range_watch.funded.clone());
    let mut source_close = Close::default();
    assert_eq!(
        pending
            .begin(&mut family_close, &mut source_close, &cancel)
            .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert_eq!((family_close.opened, family_close.closed), (0, 0));
    assert_eq!((source_close.opened, source_close.closed), (0, 0));
    assert!(
        denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
            .unwrap()
            .is_none(),
        "successful Planned replay left a compact receipt"
    );
    // Once settled, another replay must not republish even temporarily across
    // the observable source/family read boundaries.
    let observed_allocation = crate::projection::provider::records::AllocationRecord::decode(
        &allocation.encode().unwrap(),
    )
    .unwrap();
    let observed_prepared =
        crate::projection::provider::records::PreparedRecord::decode(&prepared.encode().unwrap())
            .unwrap();
    let receipt_checks = Rc::new(Cell::new(0));
    let watch_checks = receipt_checks.clone();
    let mut settled_family_close = Close::default();
    let mut settled_source_close = Close::default();
    assert_eq!(
        with_observer(
            move |_, _| {
                watch_checks.set(watch_checks.get() + 1);
                if denial_receipt::reopen(&observed_allocation, &observed_prepared, None)?.is_some()
                {
                    return Err(invalid("settled Planned replay recreated compact receipt"));
                }
                Ok(())
            },
            || pending.begin(
                &mut settled_family_close,
                &mut settled_source_close,
                &cancel
            )
        )
        .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert!(receipt_checks.get() > 0);
    assert_eq!(
        (settled_family_close.opened, settled_source_close.opened),
        (0, 0)
    );
    assert!(
        denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );
    assert_eq!(fs::read(&path).unwrap(), b"C14 current native image");
    if let Some(case) = allocation_case {
        if allocation_arm::run(
            &cancel,
            &f,
            &allocation,
            &prepared,
            &binding,
            &session,
            &attempt,
            &range_watch,
            &planned,
            &coverage,
            &private_path,
            &family_path,
            pending,
            case,
        ) {
            require_retained_preimage(&image, preimage_case, preimage, immutable);
        }
        drop(image);
        drop(checkpoint);
        drop(range_watch);
        drop(attempt);
        session.disconnect().unwrap();
        return;
    }
    drop(pending);
    // Exact funded Planned has now survived its complete durable readback and
    // writer-free replay. Missing-row rejection cannot satisfy these checks.
    let private_before = database_io::tests::support::witness(&private_path);
    let family_before = database_io::tests::support::witness(&family_path);
    require_denial_guard(
        super::super::managed_journal::require_legacy_allocation(&planned).unwrap_err(),
    );
    require_denial_guard(
        super::super::managed_journal::observe_planned(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &planned,
            &cancel,
        )
        .unwrap_err(),
    );
    let mut legacy_close = Close::default();
    require_denial_guard(
        super::super::managed_journal::begin_complete(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &mut image,
            &planned,
            &mut legacy_close,
            &cancel,
        )
        .unwrap_err(),
    );
    assert_eq!((legacy_close.opened, legacy_close.closed), (0, 0));
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );
    require_retained_preimage(&image, preimage_case, preimage, immutable);
    assert!(range_watch.funded.load(Ordering::SeqCst));
    assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
    drop(image);
    drop(checkpoint);
    drop(range_watch);
    drop(attempt);
    session.disconnect().unwrap();
}

#[test]
#[ignore = "owned native C14 start; requires retained ROOT qualification"]
fn c14_denial_start_funds_then_commits_document_and_replays_without_writers() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_start_funds_then_commits_document_and_replays_without_writers"
        ),
        1200,
        || {
            child::declare();
            run_start(false, false, None);
        },
    );
}

#[test]
#[ignore = "owned native C14 two ACK boundaries; not an OS failure or crash"]
fn c14_denial_start_retries_both_withheld_acks_without_late_probes() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_start_retries_both_withheld_acks_without_late_probes"
        ),
        1200,
        || {
            child::declare();
            run_start(true, false, None);
        },
    );
}

#[test]
#[ignore = "owned native C14 pending capture preflight; requires retained ROOT qualification"]
fn c14_denial_start_rejects_durable_staging_before_family_changes() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_start_rejects_durable_staging_before_family_changes"
        ),
        1200,
        || {
            child::declare();
            run_start(false, true, None);
        },
    );
}

#[test]
#[ignore = "owned native C14 missing historical CAS bytes; refusal must precede B"]
fn c14_denial_start_rejects_nonresident_preimage_before_family_changes() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_start_rejects_nonresident_preimage_before_family_changes"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(false, false, None, PreimageCase::Nonresident, None);
        },
    );
}

#[test]
#[ignore = "owned native C14 unsupported original representation alias; must refuse before B"]
fn c14_denial_start_rejects_ambiguous_original_representations_before_funding() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_start_rejects_ambiguous_original_representations_before_funding"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(false, false, None, PreimageCase::CurrentAlias, None);
        },
    );
}

#[test]
#[ignore = "owned native C14 d+q capacity boundary; no future companion or disk-fault claim"]
fn c14_denial_start_rechecks_source_and_future_destination_capacity_before_funding() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_start_rechecks_source_and_future_destination_capacity_before_funding"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::ProspectiveCapacity),
            );
        },
    );
}

macro_rules! allocation_guard_recipe {
    ($name:ident, $case:ident) => {
        #[test]
        #[ignore = "owned native C14 deterministic allocation guard; no Ready claim"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                1200,
                || {
                    child::declare();
                    run_start_configuration(
                        false,
                        false,
                        None,
                        PreimageCase::DescriptorOnly,
                        Some(AllocationCase::$case),
                    );
                },
            );
        }
    };
}

allocation_guard_recipe!(
    c14_denial_unsettled_owner_refuses_allocation_until_begin_replay,
    Unsettled
);
allocation_guard_recipe!(
    c14_denial_allocation_refuses_altered_planned_template,
    AlteredPlanned
);
allocation_guard_recipe!(
    c14_denial_legacy_allocation_and_format_refuse_without_effects,
    LegacyRoutes
);
allocation_guard_recipe!(
    c14_denial_format_refuses_altered_expected_copying_proof,
    AlteredCopying
);
allocation_guard_recipe!(
    c14_denial_allocation_refuses_native_image_changed_after_planned,
    ImageChanged
);

#[test]
#[ignore = "owned native C14 allocation and FORMAT only; no copy or Ready claim"]
fn c14_denial_allocation_binds_copying_and_publishes_exact_format() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_allocation_binds_copying_and_publishes_exact_format"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::Publish),
            );
        },
    );
}

#[test]
#[ignore = "owned native C14 Copying ACK retry and FORMAT only; no copy or Ready claim"]
fn c14_denial_allocation_retries_copying_ack_before_format_publication() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_allocation_retries_copying_ack_before_format_publication"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::WithheldCopyingAck),
            );
        },
    );
}

#[test]
#[ignore = "owned native C14 current-body CopiedPrefix only; no Stage or Ready claim"]
fn c14_denial_copy_registers_initializing_and_replays_exact_current_body() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_copy_registers_initializing_and_replays_exact_current_body"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::Copy(CopyCase::Publish)),
            );
        },
    );
}

#[test]
#[ignore = "owned native C14 family ACK retry before C SQLite; no Stage or Ready claim"]
fn c14_denial_copy_retries_family_ack_before_destination_schema() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_copy_retries_family_ack_before_destination_schema"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::Copy(CopyCase::WithheldFamilyAck)),
            );
        },
    );
}

#[test]
#[ignore = "owned native C14 copied destination ACK retry; no Stage or Ready claim"]
fn c14_denial_copy_retries_destination_ack_without_writers() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_copy_retries_destination_ack_without_writers"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::Copy(CopyCase::WithheldDestinationAck)),
            );
        },
    );
}

#[test]
#[ignore = "owned native C14 owner-loss recovery; no OS close failure or power-loss claim"]
fn c14_denial_orphan_recovery_after_owner_loss_and_checkout_edit() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_orphan_recovery_after_owner_loss_and_checkout_edit"
        ),
        1200,
        || {
            child::declare();
            run_start(false, false, Some(RecoveryCase::Orphan));
        },
    );
}

#[test]
#[ignore = "owned native C14 resident descriptor-only preimage recovery; no power-loss claim"]
fn c14_denial_descriptor_only_preimage_recovers_orphan_after_owner_loss() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_descriptor_only_preimage_recovers_orphan_after_owner_loss"
        ),
        1200,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                Some(RecoveryCase::Orphan),
                PreimageCase::DescriptorOnly,
                None,
            );
        },
    );
}

#[test]
#[ignore = "owned native C14 actual-funding recovery; no OS close failure or power-loss claim"]
fn c14_denial_recovery_retains_actual_planned_funding_after_owner_loss() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_recovery_retains_actual_planned_funding_after_owner_loss"
        ),
        1200,
        || {
            child::declare();
            run_start(false, false, Some(RecoveryCase::ActualPlanned));
        },
    );
}

#[test]
#[ignore = "owned native C14 release ACK retry; no OS close failure or power-loss claim"]
fn c14_denial_recovery_retries_withheld_release_ack_without_late_probes() {
    child::run(
        concat!(
            module_path!(),
            "::c14_denial_recovery_retries_withheld_release_ack_without_late_probes"
        ),
        1200,
        || {
            child::declare();
            run_start(false, false, Some(RecoveryCase::ReleaseAck));
        },
    );
}

#[path = "c14_denial_start_tests/support.rs"]
mod support;
use support::*;
