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
        self, tests::fresh_ordinary_support::with_observer, WriterCloseObserver, WriterOpenPoint,
    },
    family_upgrade::upgrade_source_family,
    import_validation::{self, ImportDatabaseKind},
    process_profile::tests::child,
    records::{
        denied_coverage::DerivedDeniedCoverage, import::ImportSourceReferences,
        transition::ProviderTransitionIntentRecord, ProviderBootstrapSource,
        ProviderSourceDescriptor, Record,
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
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

#[path = "c14_denial_start_tests/admission.rs"]
mod admission;
#[path = "c14_denial_start_tests/capture.rs"]
mod capture;
#[path = "c14_denial_start_tests/content.rs"]
mod content;
#[path = "c14_denial_start_tests/content_plain.rs"]
mod content_plain;
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

fn source_with_immutable(case: PreimageCase) -> ProviderBootstrapSource {
    let old = Fixture::writable_file_source();
    let bytes = if !matches!(case, PreimageCase::CurrentAlias) {
        b"C14 distinct immutable bytes".as_slice()
    } else {
        b"provider base content".as_slice()
    };
    let manifest = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1048576).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap();
    let mut snapshot = old.initial().clone();
    snapshot.entries.push(ProjectionEntry {
        path: RepoPath::parse("unchanged.txt").unwrap(),
        target: Some(TreeEntryTarget::File {
            file_id: manifest.file_id().clone(),
        }),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    });
    snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut descriptors = old
        .descriptors()
        .iter()
        .map(|row| ProviderSourceDescriptor {
            path: row.path.clone(),
            // Public preparation installs fixed1MiB representations. Match it
            // before bootstrap for a genuinely resident descriptor-only base.
            manifest: if !matches!(case, PreimageCase::CurrentAlias)
                && row.path.as_str() == "README.md"
            {
                let manifest = FileManifest::from_chunks(
                    ChunkingDescriptor::deterministic_fixed(1048576).unwrap(),
                    &[Chunk::new(b"provider base content".to_vec()).unwrap()],
                )
                .unwrap();
                assert_eq!(manifest.file_id(), row.manifest.file_id());
                manifest
            } else {
                row.manifest.clone()
            },
        })
        .collect::<Vec<_>>();
    descriptors.push(ProviderSourceDescriptor {
        path: RepoPath::parse("unchanged.txt").unwrap(),
        manifest,
    });
    descriptors.sort_by(|a, b| a.path.cmp(&b.path));
    ProviderBootstrapSource::validate(snapshot, old.selection().clone(), descriptors).unwrap()
}

fn require_retained_preimage(
    image: &super::super::managed_image::complete::VerifiedCompleteManagedImage<'_, '_>,
    case: PreimageCase,
    preimage: &FileManifest,
    immutable: &FileManifest,
) {
    let pinned = image.retained_offline_manifests();
    match case {
        PreimageCase::CurrentAlias => {
            panic!("ambiguous original descriptors unexpectedly admitted");
        }
        PreimageCase::DescriptorOnly => {
            assert!(
                image
                    .retained()
                    .files()
                    .iter()
                    .all(|file| file.manifest.manifest().file_id() != preimage.file_id()),
                "descriptor-only preimage has a current byte producer"
            );
            assert_eq!(pinned.len(), 2);
            assert!(
                pinned.contains(preimage),
                "exact historical representation was not pinned"
            );
            assert!(pinned.contains(immutable));
        }
        PreimageCase::Nonresident => panic!("nonresident preimage unexpectedly prepared"),
    }
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

/// Explicit server-side fixture input, established before bootstrap/observation.
/// An absent writable leaf grants this test's later creation; native existence
/// is never used to infer that grant.
fn source_for_configuration(case: PreimageCase, capture: bool) -> ProviderBootstrapSource {
    let source = source_with_immutable(case);
    if !capture {
        return source;
    }
    let mut snapshot = source.initial().clone();
    snapshot.entries.push(ProjectionEntry {
        path: RepoPath::parse("recovery.txt").unwrap(),
        target: None,
        executable: false,
        writable: true,
        state: EntryState::Absent,
    });
    snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    source.selection().validate_snapshot(&snapshot).unwrap();
    ProviderBootstrapSource::validate(
        snapshot,
        source.selection().clone(),
        source
            .descriptors()
            .iter()
            .map(|row| ProviderSourceDescriptor {
                path: row.path.clone(),
                manifest: row.manifest.clone(),
            })
            .collect(),
    )
    .unwrap()
}

#[derive(Clone, Copy)]
enum CopyCase {
    Publish,
    WithheldFamilyAck,
    WithheldDestinationAck,
}

type CatalogSnapshot = BTreeMap<String, Vec<Vec<rusqlite::types::Value>>>;

// Read-only logical evidence covers all application tables: restoring this
// baseline proves the exact B row was the only catalog DATA removed.
fn catalog_snapshot(f: &Fixture, cancel: &Cancellation) -> CatalogSnapshot {
    let family =
        crate::projection::shared::provider_access::FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, false).unwrap();
    super::super::managed_destination::valid(
        database_io::read_cancellable_strict(&family.catalog, cancel, table_snapshot).unwrap(),
    )
    .unwrap()
}

fn table_snapshot(c: &rusqlite::Connection) -> CauseResult<CatalogSnapshot> {
    let mut names = c.prepare(
                "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name COLLATE BINARY",
            )?;
    let names = names
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut tables = BTreeMap::new();
    for name in names {
        let quoted = name.replace('"', "\"\"");
        let columns = c
            .prepare(&format!("SELECT * FROM \"{quoted}\""))?
            .column_count();
        let order = (1..=columns)
            .map(|column| format!("{column} COLLATE BINARY"))
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = c.prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY {order}"))?;
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| row.get::<_, rusqlite::types::Value>(column))
                    .collect::<std::result::Result<Vec<_>, _>>()
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        tables.insert(name, rows);
    }
    Ok(tables)
}

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

fn require_validation_message(error: Cause, expected: &str) {
    require_validation_message_ref(&error, expected);
}

fn require_validation_message_ref(error: &Cause, expected: &str) {
    match error {
        Cause::Validation(error) => match error.as_ref() {
            crate::projection::ProjectionStoreError::Invalid(message) => {
                assert_eq!(message, expected);
            }
            error => panic!("legacy driver returned another validation error: {error}"),
        },
        error => panic!("legacy driver returned another error category: {error}"),
    }
}

fn require_denial_guard(error: Cause) {
    require_validation_message(error, "managed allocation requires a denial-aware driver");
}

#[allow(clippy::too_many_arguments)]
fn require_denial_copy_state(
    held: &crate::projection::provider::bootstrap::outcome::PublicationPins,
    owner: &super::super::managed_journal::denial_start::PendingDenialStart<'_, '_, '_, '_>,
    copying: &ProviderTransitionIntentRecord,
    destination: &super::super::managed_allocation::ManagedDestinationAllocation<'_>,
    coverage: &DerivedDeniedCoverage,
    copied: bool,
    cancel: &Cancellation,
) {
    let _locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .unwrap();
    owner.with_copying_destination_source(held, copying, destination, false, cancel, |source, proof, closure, current| {
        let a = &held.artifacts.allocation;
        let p = held.artifacts.prepared.as_ref().unwrap();
        let prefix = &copying.plan.source_prefix;
        let intent = super::super::managed_destination::destination_intent(copying)?;
        current.require_destination(&intent, Some(cancel))?;
        let source_body = crate::projection::provider::denied_coverage_storage::read_required(
            source, copying.plan.denial_coverage.as_ref().unwrap(), Some(cancel))?;
        assert_eq!(&source_body, coverage.record());
        let registration = super::super::managed_destination::valid(database_io::read_cancellable_strict(
            &held.family.catalog, cancel, |c| held.family.validate_import_initializing(
                c, a, p, &proof.active.activation, &proof.capture, copying, Some(cancel)),
        )?)?;
        assert_eq!(registration, crate::projection::shared::provider_access::ImportRegistration::Initializing);
        if copied {
            super::super::managed_destination::valid(database_io::read_cancellable_strict(
                destination.destination_database(), cancel, |c| {
                    let marker = &destination.proof().format;
                    closure.require_copied_prefix_scope(source, c, marker, prefix, Some(cancel))?;
                    crate::projection::provider::destination_validation::read_copied(
                        c, marker, &proof.active.source, a, p, &proof.active.activation,
                        prefix, &intent, closure, Some(cancel),
                    )?;
                    let actual = crate::projection::provider::denied_coverage_storage::read_required(
                        c, copying.plan.denial_coverage.as_ref().unwrap(), Some(cancel))?;
                    assert_eq!(actual, source_body);
                    actual.require_intent(&intent, Some(cancel))?;
                    Ok(())
                },
            )?)?;
        } else {
            assert_eq!(fs::metadata(destination.destination_database().path()).unwrap().len(), 0,
                "family ACK refusal initialized destination SQL");
        }
        Ok(())
    }).unwrap();
}

fn observe(
    active: &PrivateActiveCaptureValidation,
    identity: &crate::projection::ProjectionIdentity,
    planned: &ProviderTransitionIntentRecord,
    epoch_increment: u64,
    cancel: &Cancellation,
) -> TransitionObservationData {
    observe_with_reserved_image(active, identity, planned, epoch_increment, false, cancel)
}

fn observe_with_reserved_image(
    active: &PrivateActiveCaptureValidation,
    identity: &crate::projection::ProjectionIdentity,
    planned: &ProviderTransitionIntentRecord,
    epoch_increment: u64,
    reserve_readme: bool,
    cancel: &Cancellation,
) -> TransitionObservationData {
    let source = &active.active.source;
    let mut fresh = source.initial().clone();
    fresh.observed_authorization_epoch += epoch_increment;
    if reserve_readme {
        let row = fresh
            .entries
            .iter_mut()
            .find(|row| row.path.as_str() == "README.md")
            .unwrap();
        row.state = EntryState::Reserved;
        row.writable = false;
    }
    let authority = AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: fresh.observed_policy_set_id.clone(),
        authorization_epoch: fresh.observed_authorization_epoch,
    };
    TransitionObservationData::normalize(
        SourceObservationData {
            prefix: &planned.plan.source_prefix,
            identity,
            snapshot: &active.active.activated.snapshot,
            snapshot_parent_digest: Some(&active.active.activation.source_digest),
            selection: source.selection(),
            selection_generation: source.commitment().source_generation,
            selection_snapshot_digest: &source.commitment().snapshot_digest,
        },
        planned.plan.source_prefix.destination_identity.clone(),
        active.active.activated.generation + 1,
        fresh,
        source.selection().clone(),
        source
            .descriptors()
            .iter()
            .filter(|row| !reserve_readme || row.path.as_str() != "README.md")
            .map(|row| ProviderSourceDescriptor {
                path: row.path.clone(),
                manifest: row.manifest.clone(),
            })
            .collect(),
        ObservationCoverageData {
            denied_paths: vec![DeniedPathData {
                path: RepoPath::parse("secret.txt").unwrap(),
            }],
        },
        authority,
        Some(cancel),
    )
    .unwrap()
}

fn run_start(retry: bool, pending_capture: bool, recovery: Option<RecoveryCase>) {
    run_start_configuration(
        retry,
        pending_capture,
        recovery,
        PreimageCase::DescriptorOnly,
        None,
    );
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
    assert!(image
        .retained()
        .files()
        .iter()
        .any(|file| file.path.as_str() == "unchanged.txt"
            && matches!(
                &file.manifest,
                super::super::managed_image::ManagedManifest::Source(_)
            )));
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
    assert!(image
        .retained()
        .files()
        .iter()
        .all(|file| file.manifest.manifest().file_id() != preimage.file_id()));
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
                assert!(state
                    .capture
                    .transactions
                    .iter()
                    .any(|record| record.transaction_id == tx.transaction_id
                        && matches!(&record.state, ProviderCaptureState::Staging { .. })));
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
        use crate::projection::SharedQuota;
        let funding =
            crate::projection::shared::source_denial::SourceDeniedCoverageReservation::new(
                &allocation,
                &planned,
                coverage.record(),
                Some(&cancel),
            )
            .unwrap();
        let usage = f.family.usage().unwrap();
        // Only source denial d and this plan's future C quota q are tested;
        // a later source-companion charge is outside this boundary.
        let d = funding.reservation_bytes();
        let q = planned.plan.quota_bytes;
        assert!(d > 0 && q > 0);
        let used_and_reserved = usage
            .content_bytes
            .checked_add(usage.reserved_bytes)
            .unwrap();
        let threshold = used_and_reserved
            .checked_add(d)
            .unwrap()
            .checked_add(q)
            .unwrap();
        let insufficient = threshold - 1;
        assert!(
            used_and_reserved + d <= insufficient,
            "B alone must fit this negative quota"
        );
        let target = std::path::Path::new(&planned.plan.destination_parent)
            .join(&planned.plan.destination_leaf_nonce);
        let control = f.metadata.join("provider-control");
        let control_inventory = || {
            fs::read_dir(&control)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    assert!(entry.file_type().unwrap().is_file());
                    (
                        entry.file_name(),
                        database_io::tests::support::witness(&entry.path()),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };
        let original_control = control_inventory();
        let private_before = database_io::tests::support::witness(&private_path);
        let family_before = database_io::tests::support::witness(&family_path);
        let other_parent = f.outer.join("r1-other-parent");
        fs::create_dir(&other_parent).unwrap();
        let error = super::super::managed_journal::denial_start::PendingDenialStart::prepare(
            held,
            session.session.guard_export_parent(&other_parent).unwrap(),
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
        .expect("different real parent guard admitted");
        require_validation_message(error, "denial start retained destination parent differs");
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_eq!(control_inventory(), original_control);
        assert!(!target.exists());

        // Replace only the dedicated empty destination parent. The new guard
        // has the exact spelling but another inode, so only identity can reject.
        let retained_parent = f.outer.join("r1-original-parent");
        assert_eq!(plan_parent, f.outer.join("r1-destination-parent"));
        fs::rename(&plan_parent, &retained_parent).unwrap();
        fs::create_dir(&plan_parent).unwrap();
        let replacement = session.session.guard_export_parent(&plan_parent).unwrap();
        assert_ne!(
            replacement.filesystem_identity(),
            planned.plan.destination_parent_identity
        );
        let error = super::super::managed_journal::denial_start::PendingDenialStart::prepare(
            held,
            replacement,
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
        .expect("same-path replacement parent admitted");
        require_validation_message(error, "denial start retained destination parent differs");
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_eq!(control_inventory(), original_control);
        assert!(!target.exists());
        fs::remove_dir(&plan_parent).unwrap(); // empty fixture directory only
        fs::rename(&retained_parent, &plan_parent).unwrap();

        // A real guard from another Provider Session cannot stand in for the
        // source owner merely because its parent path and inode are correct.
        let (_foreign_fixture, foreign_holder, _, _, _) =
            capture_family::active_holder_with_source(None);
        let foreign_session = foreign_holder
            .into_disconnected_session()
            .unwrap_or_else(|error| panic!("foreign session: {}", error.cause()));
        let foreign_parent = foreign_session
            .session
            .guard_export_parent(&plan_parent)
            .unwrap();
        assert_eq!(
            foreign_parent.filesystem_identity(),
            planned.plan.destination_parent_identity
        );
        let error = super::super::managed_journal::denial_start::PendingDenialStart::prepare(
            held,
            foreign_parent,
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
        .expect("foreign source Session parent admitted");
        assert!(
            matches!(error, Cause::Vfs(layrs_vfs::Error::IdentityMismatch)),
            "{error}"
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_eq!(control_inventory(), original_control);
        assert!(!target.exists());

        f.family
            .set_quota(SharedQuota {
                bytes: insufficient,
            })
            .unwrap();
        let before_low = catalog_snapshot(&f, &cancel);
        let family_before_low = database_io::tests::support::witness(&family_path);
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
        .expect("unfundable d+q admitted because d alone fits");
        require_validation_message(
            error,
            "partial_store_quota: denial source and future destination unfunded",
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before_low
        );
        assert_eq!(catalog_snapshot(&f, &cancel), before_low);
        assert_eq!(control_inventory(), original_control);
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
        assert!(!target.exists());
        assert_eq!(
            f.family.usage().unwrap().reserved_bytes,
            usage.reserved_bytes
        );

        f.family
            .set_quota(SharedQuota { bytes: threshold })
            .unwrap();
        let family_at_threshold = database_io::tests::support::witness(&family_path);
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
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_at_threshold
        );
        assert_eq!(control_inventory(), original_control);
        assert!(!target.exists());

        // A quota change after prepare must be rechecked before receipt/B.
        f.family
            .set_quota(SharedQuota {
                bytes: insufficient,
            })
            .unwrap();
        let family_before_begin = database_io::tests::support::witness(&family_path);
        let catalog_before_begin = catalog_snapshot(&f, &cancel);
        let mut family_close = Close::default();
        let mut source_close = Close::default();
        let error = pending
            .begin(&mut family_close, &mut source_close, &cancel)
            .unwrap_err();
        require_validation_message(
            error,
            "partial_store_quota: denial source and future destination unfunded",
        );
        assert_eq!(
            (
                family_close.opened,
                family_close.closed,
                source_close.opened,
                source_close.closed
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before_begin
        );
        assert_eq!(catalog_snapshot(&f, &cancel), catalog_before_begin);
        assert_eq!(control_inventory(), original_control);
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
        assert!(!target.exists());

        f.family
            .set_quota(SharedQuota { bytes: threshold })
            .unwrap();
        // Stop at the real family COMMIT to exercise capacity with B already
        // reserved but Planned absent. Re-adding d would exceed this exact limit.
        let refused = Rc::new(Cell::new(false));
        let mut family_close = Close::default();
        family_close.funded = Some(range_watch.funded.clone());
        family_close.refused = Some(refused.clone());
        let mut source_close = Close::default();
        let error = pending
            .begin(&mut family_close, &mut source_close, &cancel)
            .unwrap_err();
        require_validation_message(error, "C14 start SQLite close ACK withheld");
        assert!(refused.get());
        assert_eq!((family_close.opened, family_close.closed), (1, 1));
        assert_eq!((source_close.opened, source_close.closed), (0, 0));
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert!(!target.exists());
        let funded_usage = f.family.usage().unwrap();
        assert_eq!(funded_usage.content_bytes, usage.content_bytes);
        assert_eq!(funded_usage.reserved_bytes, usage.reserved_bytes + d);
        assert_eq!(
            funded_usage.content_bytes + funded_usage.reserved_bytes + q,
            threshold
        );
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_some()
        );
        let family_funded = database_io::tests::support::witness(&family_path);
        let mut family_close = Close::default();
        let mut source_close = Close::default();
        let source_refused = Rc::new(Cell::new(false));
        source_close.refused = Some(source_refused.clone());
        require_validation_message(
            pending
                .begin(&mut family_close, &mut source_close, &cancel)
                .unwrap_err(),
            "C14 start SQLite close ACK withheld",
        );
        assert!(source_refused.get());
        assert_eq!((family_close.opened, family_close.closed), (0, 0));
        assert_eq!((source_close.opened, source_close.closed), (1, 1));
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_funded
        );
        let actual = super::super::managed_destination::valid(
            database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
                validation::private_active_transition_with_source(
                    c,
                    &allocation,
                    &prepared,
                    &binding,
                    Some(session.source.root.registration().root_id()),
                    Some(&cancel),
                )
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(actual.transitions, std::slice::from_ref(&planned));
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_some()
        );
        let private_committed = database_io::tests::support::witness(&private_path);
        // Planned is already durable. Lowering prospective C capacity must not
        // prevent this exact no-writer replay or cleanup of the compact receipt.
        f.family
            .set_quota(SharedQuota {
                bytes: insufficient,
            })
            .unwrap();
        let family_lowered = database_io::tests::support::witness(&family_path);
        let usage_lowered = f.family.usage().unwrap();
        let mut family_close = Close::default();
        let mut source_close = Close::default();
        assert_eq!(
            pending
                .begin(&mut family_close, &mut source_close, &cancel)
                .unwrap(),
            SourceIntentEffect::AlreadyPresent
        );
        assert_eq!(
            (
                family_close.opened,
                family_close.closed,
                source_close.opened,
                source_close.closed
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_committed
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_lowered
        );
        assert_eq!(f.family.usage().unwrap(), usage_lowered);
        assert_eq!(control_inventory(), original_control);
        assert!(
            denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                .unwrap()
                .is_none()
        );
        assert!(!target.exists());
        f.family
            .set_quota(SharedQuota { bytes: threshold })
            .unwrap();
        assert_eq!(f.family.usage().unwrap(), funded_usage);
        let private_before_foreign = database_io::tests::support::witness(&private_path);
        let family_before_foreign = database_io::tests::support::witness(&family_path);
        let failure = super::super::managed_allocation::allocate_denial(
            &foreign_session.session,
            held,
            &mut pending,
            &planned,
            &cancel,
        )
        .err()
        .expect("foreign allocation Session created C");
        assert!(
            matches!(
                &failure.cause,
                Cause::Vfs(layrs_vfs::Error::IdentityMismatch)
            ),
            "{}",
            failure.cause
        );
        assert!(failure.effects.is_empty());
        assert!(!target.exists());
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before_foreign
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before_foreign
        );
        drop(failure);
        drop(foreign_session);
        drop(pending);
        require_retained_preimage(&image, preimage_case, preimage, immutable);
        drop(image);
        drop(checkpoint);
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
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
        use super::super::managed_allocation::{ManagedFormatState, ManagedFormatStep};
        use crate::projection::provider::records::transition::TransitionState;
        let family_before = database_io::tests::support::witness(&family_path);
        let catalog_before = catalog_snapshot(&f, &cancel);
        let read_source = || {
            super::super::managed_destination::valid(
                database_io::read_cancellable_strict(
                    &held.artifacts.database,
                    &cancel,
                    table_snapshot,
                )
                .unwrap(),
            )
            .unwrap()
        };
        let source_before = read_source();
        let private_before = database_io::tests::support::witness(&private_path);
        if matches!(
            case,
            AllocationCase::AlteredPlanned
                | AllocationCase::LegacyRoutes
                | AllocationCase::ImageChanged
        ) {
            let mut candidate =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            if matches!(case, AllocationCase::AlteredPlanned) {
                candidate.plan.destination_leaf_nonce = "ffffffffffffffffffffffffffffffff".into();
                assert_ne!(candidate.plan, planned.plan);
                candidate.encode().unwrap();
            }
            if matches!(case, AllocationCase::ImageChanged) {
                fs::write(
                    f.checkout.join("c14-after-planned.txt"),
                    b"external namespace change after Planned",
                )
                .unwrap();
            }
            let target = f.outer.join(&candidate.plan.destination_leaf_nonce);
            let error = if matches!(case, AllocationCase::LegacyRoutes) {
                super::super::managed_allocation::allocate(
                    &session.session,
                    held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    &attempt,
                    &candidate,
                    &cancel,
                )
            } else {
                super::super::managed_allocation::allocate_denial(
                    &session.session,
                    held,
                    &mut pending,
                    &candidate,
                    &cancel,
                )
            }
            .err()
            .expect("negative allocation guard created C");
            assert!(error.effects.is_empty());
            assert_eq!(error.attempted_root, target);
            match case {
                AllocationCase::ImageChanged => {
                    assert!(
                        matches!(&error.cause, Cause::Vfs(layrs_vfs::Error::Busy)),
                        "{}",
                        error.cause
                    )
                }
                AllocationCase::LegacyRoutes => require_validation_message_ref(
                    &error.cause,
                    "managed allocation requires a denial-aware driver",
                ),
                _ => require_validation_message_ref(
                    &error.cause,
                    "denial allocation owner or immutable plan differs",
                ),
            }
            assert!(!target.exists());
            assert!(!f.outer.join(&planned.plan.destination_leaf_nonce).exists());
            assert_eq!(read_source(), source_before);
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
            if matches!(case, AllocationCase::ImageChanged) {
                drop(pending);
                drop(image);
                drop(checkpoint);
                assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
                drop(range_watch);
                drop(attempt);
                session.disconnect().unwrap();
                return;
            }
        }
        let mut destination = super::super::managed_allocation::allocate_denial(
            &session.session,
            held,
            &mut pending,
            &planned,
            &cancel,
        )
        .unwrap_or_else(|error| panic!("denial allocation failed: {error:?}"));
        destination.verify(&planned.plan).unwrap();
        assert_eq!(
            destination.root(),
            f.outer.join(&planned.plan.destination_leaf_nonce)
        );
        let proof_digest = destination.proof().digest(&planned.plan).unwrap();
        let temporary = destination.root().join(".layrs").join(format!(
            "FORMAT.pending-{}",
            planned.plan.destination_leaf_nonce,
        ));
        let final_format = destination.root().join(".layrs/FORMAT");
        assert!(!final_format.exists());
        assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
        assert_eq!(
            fs::metadata(destination.destination_database().path())
                .unwrap()
                .len(),
            0
        );
        let destination_before =
            database_io::tests::support::witness(destination.destination_database().path());
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );

        let mut bind_close = Close::default();
        let (copying, effect) = if matches!(case, AllocationCase::WithheldCopyingAck) {
            let refused = Rc::new(Cell::new(false));
            let probes = Rc::new(Cell::new(0));
            bind_close.refused = Some(refused.clone());
            let watch_refused = refused.clone();
            let watch_probes = probes.clone();
            let error = with_observer(
                move |_, _| {
                    if watch_refused.get() {
                        watch_probes.set(watch_probes.get() + 1);
                        return Err(invalid("forbidden probe after SourceCopying ACK"));
                    }
                    Ok(())
                },
                || pending.bind_denial_allocation(&destination, &mut bind_close, &cancel),
            )
            .unwrap_err();
            require_validation_message(error, "C14 start SQLite close ACK withheld");
            assert!(refused.get());
            assert_eq!(probes.get(), 0);
            assert_eq!((bind_close.opened, bind_close.closed), (1, 1));
            assert!(
                !final_format.exists(),
                "failed Copying ACK published FORMAT"
            );
            assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
            assert_eq!(
                database_io::tests::support::witness(&family_path),
                family_before
            );
            assert_eq!(
                database_io::tests::support::witness(destination.destination_database().path()),
                destination_before
            );
            let before_retry = database_io::tests::support::witness(&private_path);
            let mut retry_close = Close::default();
            let result = pending
                .bind_denial_allocation(&destination, &mut retry_close, &cancel)
                .unwrap();
            assert_eq!((retry_close.opened, retry_close.closed), (0, 0));
            assert_eq!(
                database_io::tests::support::witness(&private_path),
                before_retry
            );
            assert_eq!(result.1, SourceIntentEffect::AlreadyPresent);
            result
        } else {
            let result = pending
                .bind_denial_allocation(&destination, &mut bind_close, &cancel)
                .unwrap();
            assert_eq!((bind_close.opened, bind_close.closed), (1, 1));
            assert_eq!(result.1, SourceIntentEffect::Updated);
            result
        };
        assert_eq!(
            effect,
            if matches!(case, AllocationCase::WithheldCopyingAck) {
                SourceIntentEffect::AlreadyPresent
            } else {
                SourceIntentEffect::Updated
            }
        );
        assert_eq!(copying.state, TransitionState::Copying);
        assert_eq!(copying.plan, planned.plan);
        assert_eq!(copying.transaction_id, planned.transaction_id);
        assert_eq!(copying.destination.as_ref(), Some(destination.proof()));
        copying.require_transition(&planned).unwrap();
        destination.verify(&copying.plan).unwrap();
        assert!(!final_format.exists());
        assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
        let source_after = read_source();
        let mut unchanged_before = source_before;
        let mut unchanged_after = source_after.clone();
        assert_eq!(
            unchanged_before
                .remove("projection_provider_transitions")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            unchanged_after
                .remove("projection_provider_transitions")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            unchanged_after, unchanged_before,
            "Copying changed DATA outside its exact intent row"
        );
        let private_before = database_io::tests::support::witness(&private_path);
        let mut replay_close = Close::default();
        let (replay_copying, replay_effect) = pending
            .bind_denial_allocation(&destination, &mut replay_close, &cancel)
            .unwrap();
        assert_eq!(replay_copying, copying);
        assert_eq!(replay_effect, SourceIntentEffect::AlreadyPresent);
        assert_eq!((replay_close.opened, replay_close.closed), (0, 0));
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );

        if matches!(
            case,
            AllocationCase::LegacyRoutes | AllocationCase::AlteredCopying
        ) {
            let before = database_io::tests::support::witness(&private_path);
            let temporary_before = database_io::tests::support::witness(&temporary);
            let error = if matches!(case, AllocationCase::LegacyRoutes) {
                destination
                    .publish_format(
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        &attempt,
                        &copying,
                        &cancel,
                    )
                    .unwrap_err()
            } else {
                let mut altered =
                    ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
                let digest = &mut altered.destination.as_mut().unwrap().family_marker_digest;
                *digest = format!(
                    "blake3:{}",
                    if digest.ends_with('0') { "1" } else { "0" }.repeat(64)
                );
                altered.encode().unwrap();
                let error = pending
                    .observe_copying_allocation(held, &altered, &destination, &cancel)
                    .unwrap_err();
                require_validation_message(
                    error,
                    "denial FORMAT requires exact retained Copying proof",
                );
                destination
                    .publish_format_denial(&mut pending, &altered, &cancel)
                    .unwrap_err()
            };
            assert!(error.effects.is_empty());
            if matches!(case, AllocationCase::LegacyRoutes) {
                require_denial_guard(error.cause);
            } else {
                require_validation_message(
                    error.cause,
                    "managed FORMAT publication requires exact source Copying",
                );
            }
            assert!(!final_format.exists());
            assert_eq!(
                database_io::tests::support::witness(&temporary),
                temporary_before
            );
            assert_eq!(
                database_io::tests::support::witness(destination.destination_database().path()),
                destination_before
            );
            assert_eq!(database_io::tests::support::witness(&private_path), before);
            assert_eq!(
                database_io::tests::support::witness(&family_path),
                family_before
            );
            destination.verify(&copying.plan).unwrap();
        }
        let publication = destination
            .publish_format_denial(&mut pending, &copying, &cancel)
            .unwrap();
        assert_eq!(publication.state, ManagedFormatState::Published);
        assert!(publication
            .effects
            .iter()
            .any(|effect| effect.step == ManagedFormatStep::Rename && effect.confirmed));
        destination.require_published(&copying.plan).unwrap();
        assert!(!temporary.exists());
        assert_eq!(
            fs::read(&final_format).unwrap(),
            destination.proof().format.bytes().unwrap()
        );
        assert_eq!(
            destination.proof().digest(&copying.plan).unwrap(),
            proof_digest
        );
        let format_before = database_io::tests::support::witness(&final_format);
        let replay = destination
            .publish_format_denial(&mut pending, &copying, &cancel)
            .unwrap();
        assert_eq!(replay.state, ManagedFormatState::AlreadyPublished);
        assert!(!replay.effects.iter().any(|effect| matches!(
            effect.step,
            ManagedFormatStep::Append | ManagedFormatStep::Rename
        )));
        destination.require_published(&copying.plan).unwrap();
        destination.verify(&copying.plan).unwrap();
        assert_eq!(
            database_io::tests::support::witness(&final_format),
            format_before
        );
        assert_eq!(
            read_source(),
            source_after,
            "FORMAT publication changed source body or intent"
        );
        assert_eq!(
            database_io::tests::support::witness(&private_path),
            private_before
        );
        assert_eq!(
            database_io::tests::support::witness(&family_path),
            family_before
        );
        assert_eq!(catalog_snapshot(&f, &cancel), catalog_before);
        let destination_owner = copying
            .plan
            .source_prefix
            .destination_identity
            .worktree_id
            .as_str();
        let registered = super::super::managed_destination::valid(
            database_io::read_cancellable_strict(&held.family.catalog, &cancel, |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM worktrees WHERE id=?1",
                    [destination_owner],
                    |row| row.get::<_, u64>(0),
                )?)
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            registered, 0,
            "allocation/FORMAT registered destination family state"
        );
        assert_eq!(
            fs::metadata(destination.destination_database().path())
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            database_io::tests::support::witness(destination.destination_database().path()),
            destination_before
        );
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
                assert_eq!(state.transitions.as_slice(), std::slice::from_ref(&copying));
                let actual = crate::projection::provider::denied_coverage_storage::read_required(
                    c,
                    copying.plan.denial_coverage.as_ref().unwrap(),
                    Some(&cancel),
                )?;
                assert_eq!(&actual, coverage.record());
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();
        if let AllocationCase::Stage(stage_case) = case {
            let mut close = Close::default();
            let copied = super::super::managed_destination::register_and_copy_denial(
                held,
                &mut pending,
                &copying,
                &destination,
                &mut close,
                &cancel,
            )
            .unwrap();
            assert_eq!(
                copied.family,
                crate::projection::shared::provider_access::ImportRegistrationEffect::Inserted
            );
            assert_eq!(
                copied.destination,
                super::super::managed_destination::DestinationSqlState::FreshCopied
            );
            assert_eq!((close.opened, close.closed), (2, 2));
            require_denial_copy_state(
                held,
                &pending,
                &copying,
                &destination,
                &coverage,
                true,
                &cancel,
            );
            staging::run(
                &f,
                held,
                &session.source.root,
                &mut pending,
                &copying,
                &destination,
                &coverage,
                stage_case,
                &cancel,
            );
            assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
            drop(destination);
            drop(pending);
            require_retained_preimage(&image, preimage_case, preimage, immutable);
            drop(image);
            drop(checkpoint);
            drop(range_watch);
            drop(attempt);
            session.disconnect().unwrap();
            return;
        }
        if let AllocationCase::Copy(copy_case) = case {
            use super::super::managed_destination::DestinationSqlState;
            use crate::projection::shared::provider_access::ImportRegistrationEffect;
            let family_before_copy = database_io::tests::support::witness(&family_path);
            let catalog_before_copy = catalog_snapshot(&f, &cancel);
            let source_before_copy = read_source();
            let private_before_copy = database_io::tests::support::witness(&private_path);
            let destination_before_copy =
                database_io::tests::support::witness(destination.destination_database().path());
            // This source is genuinely Copying and FORMAT is published, so an
            // absent predecessor cannot satisfy the legacy-route assertion.
            let mut legacy_close = Close::default();
            require_denial_guard(
                super::super::managed_destination::register_and_copy(
                    held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    &attempt,
                    &copying,
                    &destination,
                    &mut legacy_close,
                    &cancel,
                )
                .unwrap_err(),
            );
            assert_eq!((legacy_close.opened, legacy_close.closed), (0, 0));
            assert_eq!(
                database_io::tests::support::witness(&private_path),
                private_before_copy
            );
            assert_eq!(
                database_io::tests::support::witness(&family_path),
                family_before_copy
            );
            assert_eq!(
                database_io::tests::support::witness(destination.destination_database().path()),
                destination_before_copy
            );

            let mut copy_close = Close::default();
            let result = if matches!(copy_case, CopyCase::Publish) {
                let effect = super::super::managed_destination::register_and_copy_denial(
                    held,
                    &mut pending,
                    &copying,
                    &destination,
                    &mut copy_close,
                    &cancel,
                )
                .unwrap();
                assert_eq!((copy_close.opened, copy_close.closed), (2, 2));
                assert_eq!(effect.family, ImportRegistrationEffect::Inserted);
                assert_eq!(effect.destination, DestinationSqlState::FreshCopied);
                effect
            } else {
                let refused = Rc::new(Cell::new(false));
                let probes = Rc::new(Cell::new(0));
                copy_close.refused = Some(refused.clone());
                let ordinal = if matches!(copy_case, CopyCase::WithheldFamilyAck) {
                    1
                } else {
                    2
                };
                copy_close.refuse_at = Some(ordinal);
                let watch_refused = refused.clone();
                let watch_probes = probes.clone();
                let error = with_observer(
                    move |_, _| {
                        if watch_refused.get() {
                            watch_probes.set(watch_probes.get() + 1);
                            return Err(invalid("forbidden probe after denial C copy ACK"));
                        }
                        Ok(())
                    },
                    || {
                        super::super::managed_destination::register_and_copy_denial(
                            held,
                            &mut pending,
                            &copying,
                            &destination,
                            &mut copy_close,
                            &cancel,
                        )
                    },
                )
                .unwrap_err();
                require_validation_message(error, "C14 start SQLite close ACK withheld");
                assert!(refused.get());
                assert_eq!(probes.get(), 0);
                assert_eq!((copy_close.opened, copy_close.closed), (ordinal, ordinal));
                let copied = matches!(copy_case, CopyCase::WithheldDestinationAck);
                require_denial_copy_state(
                    held,
                    &pending,
                    &copying,
                    &destination,
                    &coverage,
                    copied,
                    &cancel,
                );
                if !copied {
                    assert_eq!(
                        database_io::tests::support::witness(
                            destination.destination_database().path()
                        ),
                        destination_before_copy,
                        "family ACK failure wrote C SQLite"
                    );
                }
                let family_before_retry = database_io::tests::support::witness(&family_path);
                let destination_before_retry =
                    database_io::tests::support::witness(destination.destination_database().path());
                let mut retry_close = Close::default();
                let effect = super::super::managed_destination::register_and_copy_denial(
                    held,
                    &mut pending,
                    &copying,
                    &destination,
                    &mut retry_close,
                    &cancel,
                )
                .unwrap();
                assert_eq!(effect.family, ImportRegistrationEffect::AlreadyPresent);
                assert_eq!(
                    database_io::tests::support::witness(&family_path),
                    family_before_retry,
                    "copy retry reopened family writer"
                );
                if copied {
                    assert_eq!((retry_close.opened, retry_close.closed), (0, 0));
                    assert_eq!(effect.destination, DestinationSqlState::ReplayedCopied);
                    assert_eq!(
                        database_io::tests::support::witness(
                            destination.destination_database().path()
                        ),
                        destination_before_retry
                    );
                } else {
                    assert_eq!((retry_close.opened, retry_close.closed), (1, 1));
                    assert_eq!(effect.destination, DestinationSqlState::FreshCopied);
                }
                effect
            };
            assert!(matches!(
                result.destination,
                DestinationSqlState::FreshCopied | DestinationSqlState::ReplayedCopied
            ));
            require_denial_copy_state(
                held,
                &pending,
                &copying,
                &destination,
                &coverage,
                true,
                &cancel,
            );
            let family_before_replay = database_io::tests::support::witness(&family_path);
            let destination_before_replay =
                database_io::tests::support::witness(destination.destination_database().path());
            let mut replay_close = Close::default();
            let replay = super::super::managed_destination::register_and_copy_denial(
                held,
                &mut pending,
                &copying,
                &destination,
                &mut replay_close,
                &cancel,
            )
            .unwrap();
            assert_eq!(replay.family, ImportRegistrationEffect::AlreadyPresent);
            assert_eq!(replay.destination, DestinationSqlState::ReplayedCopied);
            assert_eq!((replay_close.opened, replay_close.closed), (0, 0));
            require_denial_copy_state(
                held,
                &pending,
                &copying,
                &destination,
                &coverage,
                true,
                &cancel,
            );
            assert_eq!(
                database_io::tests::support::witness(&family_path),
                family_before_replay
            );
            assert_eq!(
                database_io::tests::support::witness(destination.destination_database().path()),
                destination_before_replay
            );
            assert_eq!(
                read_source(),
                source_before_copy,
                "C copy changed source intent or current denial body"
            );
            assert_eq!(
                database_io::tests::support::witness(&private_path),
                private_before_copy
            );
            let catalog_after_copy = catalog_snapshot(&f, &cancel);
            assert_eq!(
                catalog_after_copy.get("reservations"),
                catalog_before_copy.get("reservations"),
                "registration or C copy changed B funding"
            );
            destination.require_published(&copying.plan).unwrap();
            assert_eq!(
                database_io::tests::support::witness(&final_format),
                format_before
            );
            assert!(
                denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
        drop(destination);
        drop(pending);
        require_retained_preimage(&image, preimage_case, preimage, immutable);
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
