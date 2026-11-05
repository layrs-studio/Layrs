//! Integrated native recipes. Uses real Provider bootstrap,
//! accepted Capture, retained export, exact C engine and private/family Ready.
//! Local observation is fixture DATA, never a server-held authority lease.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::{capture_family, fixture::Fixture},
    capture_lock::CaptureAttemptLock,
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    import_validation::{self, ImportDatabaseKind},
    managed_ready_reopen::ManagedReadyInspection,
    process_profile::tests::child,
    records::{
        ProviderBootstrapSource, ProviderSourceDescriptor, Record,
        import::{
            ImportClosure, ImportRowset, ImportSource, ImportSourceReferences, ImportVersion,
            ProviderImportKind, ProviderImportPrefixRecord,
        },
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    source_companion::SourceCompanionPublication,
    transition_observation::{
        AuthorityObservationData, ObservationCoverageData, SourceObservationData,
        TransitionObservationData,
    },
    transition_reconcile::source_sql,
    validation,
};
use layrs_core::{
    projection::{AvailabilityRequest, EntryState, OfflineOperation, ProjectionEntry},
    scm::{Chunk, ChunkingDescriptor, FileManifest, RepoPath, TreeEntryTarget, WorktreeId},
};
use rusqlite::{Connection, ffi};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const README_BASE: &[u8] = b"provider base content";
const RECOVERY_BASE: &[u8] = b"C13 distinct recovery original bytes";
const UNCHANGED: &[u8] = b"C13 unchanged immutable source bytes";
const HISTORICAL: &[u8] = b"C13 superseded accepted capture bytes";

#[test]
#[ignore = "only the C13 owned native recipe launches this exact reader"]
fn c13_external_hydration_reader() {
    let root = PathBuf::from(std::env::var_os("LAYRS_C13_HYDRATION_ROOT").unwrap());
    assert!(root.is_absolute());
    for (name, bytes) in [("README.md", README_BASE), ("recovery.txt", RECOVERY_BASE)] {
        if name == "README.md" && std::env::var_os("LAYRS_C13_SKIP_README").is_some() {
            continue;
        }
        assert_eq!(fs::read(root.join(name)).unwrap(), bytes);
    }
}

fn hydrate_originals(root: &Path, seed_readme: bool) {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let name = concat!(module_path!(), "::c13_external_hydration_reader");
    let name = name.strip_prefix("layrs_store_local::").unwrap_or(name);
    let mut command = Command::new(std::env::current_exe().unwrap());
    if !seed_readme {
        command.env("LAYRS_C13_SKIP_README", "1");
    } else {
        command.env_remove("LAYRS_C13_SKIP_README");
    }
    let mut reader = child::OwnedChild::new(
        command
            .args([
                "--exact",
                name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LAYRS_C13_HYDRATION_ROOT", root)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .creation_flags(0x08000000)
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    loop {
        if let Some(status) = reader.try_wait().unwrap() {
            assert!(
                status.success(),
                "C13 external hydration reader failed: {status}"
            );
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "C13 hydration reader timeout"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, p: WriterOpenPoint, _: *mut ffi::sqlite3) -> CauseResult<()> {
        if p == WriterOpenPoint::BeforeRaw {
            self.opens += 1;
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, success: bool) -> Option<Cause> {
        assert!(
            success,
            "positive SQLite CONNECTION close observation; no OS handle-close claim"
        );
        self.closes += 1;
        None
    }
}

#[derive(Default)]
struct RetryClose {
    normal: Close,
    phase: usize,
    armed: bool,
    refused: bool,
    source_owner: String,
    destination_owner: String,
}
impl WriterCloseObserver for RetryClose {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        assert!(
            !self.refused,
            "driver opened another writer after refused close ACK"
        );
        self.normal.open_point(point, raw)
    }
    fn before_close(&mut self, raw: *mut ffi::sqlite3) -> Option<Cause> {
        // Borrow only this already-open writer before its actual close. The
        // rusqlite borrowed wrapper does not own or close the raw connection.
        let c = unsafe { Connection::from_handle(raw) }.unwrap();
        assert!(
            c.is_autocommit(),
            "fault must follow COMMIT/rollback completion"
        );
        let table = |name| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [name],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        };
        self.armed = match self.phase {
            0 if table("reservations") => c.query_row("SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND operation LIKE 'provider-companion:%')", [&self.source_owner], |r| r.get(0)).unwrap(),
            1 if table("reservations") => c.query_row("SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND operation LIKE 'provider-transition:%')", [&self.destination_owner], |r| r.get(0)).unwrap(),
            2 if table("exact_installations") => c.query_row("SELECT EXISTS(SELECT 1 FROM exact_installations)", [], |r| r.get(0)).unwrap(),
            3 if table("roots") => c.query_row("SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)", [&self.destination_owner], |r| r.get(0)).unwrap(),
            4 if table("roots") => c.query_row("SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1 AND operation LIKE 'provider-companion:%')", [&self.source_owner], |r| r.get(0)).unwrap(),
            5 if table("projection_prejournal") && table("projection_provider_transitions") => c.query_row("SELECT EXISTS(SELECT 1 FROM projection_prejournal) AND EXISTS(SELECT 1 FROM projection_provider_transitions WHERE role='destination' AND state='staging')", [], |r| r.get(0)).unwrap(),
            6 if table("projection_prejournal") => {
                use crate::projection::provider::checkout_journal::{CheckoutRow, CheckoutEntry, DirectoryState};
                let mut statement = c.prepare("SELECT payload FROM projection_prejournal").unwrap();
                statement.query_map([], |r| r.get::<_, Vec<u8>>(0)).unwrap().any(|payload| matches!(CheckoutRow::decode(&payload.unwrap()).unwrap().entry,
                    CheckoutEntry::Stage { state: DirectoryState::Owned { .. } }))
            },
            7 if table("projection_provider_transitions") => c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE role='destination' AND state='ready')", [], |r| r.get(0)).unwrap(),
            8 if table("worktrees") => c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND state='ready')", [&self.destination_owner], |r| r.get(0)).unwrap(),
            9 | 10 if table("projection_provider_transitions") => c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE role='source' AND state=?1)", [if self.phase == 9 { "ready" } else { "published" }], |r| r.get(0)).unwrap(),
            _ => false,
        };
        None
    }
    fn after_close(&mut self, success: bool) -> Option<Cause> {
        self.normal.after_close(success);
        if self.armed {
            self.armed = false;
            self.refused = true;
            Some(invalid("C13 selected durable phase close ACK withheld"))
        } else {
            None
        }
    }
}
fn replace(path: &Path, bytes: &[u8]) {
    if fs::remove_file(path).is_err() {
        let mut p = fs::metadata(path).unwrap().permissions();
        p.set_readonly(false);
        fs::set_permissions(path, p).unwrap();
        fs::remove_file(path).unwrap();
    }
    fs::write(path, bytes).unwrap();
}
fn manifest(bytes: &[u8], native: bool) -> FileManifest {
    FileManifest::from_chunks(
        if native {
            ChunkingDescriptor::deterministic_fixed(1048576).unwrap()
        } else {
            ChunkingDescriptor::single()
        },
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap()
}
fn with_source(
    test: impl FnOnce(&Fixture, &ProviderActiveSession, &[u8], &[u8]) -> PathBuf,
) -> PathBuf {
    with_source_configuration(true, test)
}

fn with_source_configuration(
    seed_readme: bool,
    test: impl FnOnce(&Fixture, &ProviderActiveSession, &[u8], &[u8]) -> PathBuf,
) -> PathBuf {
    let old = Fixture::writable_file_source();
    let accepted = b"C13 real accepted writable edit";
    let recovered = b"C13 distinct denied local recovery edit";
    let mut snapshot = old.initial().clone();
    let recovery_manifest = manifest(RECOVERY_BASE, true);
    snapshot.entries.push(ProjectionEntry {
        path: RepoPath::parse("recovery.txt").unwrap(),
        target: Some(TreeEntryTarget::File {
            file_id: recovery_manifest.file_id().clone(),
        }),
        executable: false,
        writable: true,
        state: EntryState::Unavailable,
    });
    snapshot.entries.push(ProjectionEntry {
        path: RepoPath::parse("unchanged.txt").unwrap(),
        target: Some(TreeEntryTarget::File {
            file_id: manifest(UNCHANGED, true).file_id().clone(),
        }),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    });
    snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut descriptors = old
        .descriptors()
        .iter()
        .map(|d| ProviderSourceDescriptor {
            path: d.path.clone(),
            manifest: manifest(README_BASE, true),
        })
        .collect::<Vec<_>>();
    descriptors.push(ProviderSourceDescriptor {
        path: RepoPath::parse("recovery.txt").unwrap(),
        manifest: recovery_manifest,
    });
    descriptors.push(ProviderSourceDescriptor {
        path: RepoPath::parse("unchanged.txt").unwrap(),
        manifest: manifest(UNCHANGED, true),
    });
    descriptors.sort_by(|a, b| a.path.cmp(&b.path));
    let source =
        ProviderBootstrapSource::validate(snapshot, old.selection().clone(), descriptors).unwrap();
    let seed_snapshot = source.initial().clone();
    let seed_selection = source.selection().clone();
    let (f, holder, allocation, _, binding) =
        capture_family::active_holder_with_source(Some(source));
    drop(holder);
    // Populate the immutable CAS through the public preparation path before
    // the Provider session acquires its lifecycle lease. No raw CAS/SQL writes.
    let seed_root = f.outer.join("c13-resident-source");
    fs::create_dir(&seed_root).unwrap();
    let seed = f
        .family
        .create_worktree(
            &seed_root,
            WorktreeId::parse("worktree:c13-resident-source").unwrap(),
            seed_snapshot,
            seed_selection,
        )
        .unwrap();
    let request = AvailabilityRequest {
        version: seed.current_version().unwrap(),
        other: None,
        paths: ["README.md", "recovery.txt", "unchanged.txt"]
            .into_iter()
            .filter(|path| seed_readme || *path != "README.md")
            .map(|p| RepoPath::parse(p).unwrap())
            .collect(),
        operation: OfflineOperation::Read,
    };
    let preparation = seed.begin_preparation(&request).unwrap();
    seed.resume_preparation(&preparation.id, &mut |_, path, output| {
        let bytes = match path.as_str() {
            "README.md" => README_BASE,
            "recovery.txt" => RECOVERY_BASE,
            "unchanged.txt" => UNCHANGED,
            _ => panic!("unexpected original path"),
        };
        output.write_all(bytes)
    })
    .unwrap();
    drop(seed);
    crate::projection::provider::family_upgrade::upgrade_source_family(
        f.family.root(),
        &f.worktree,
        &allocation.transaction_id,
        &mut Close::default(),
        &Cancellation::default(),
    )
    .unwrap();
    let mut session = PartialStoreFamily::open_provider_active(
        f.family.root(),
        &f.worktree,
        &binding.transaction_id,
        &binding,
    )
    .unwrap()
    .into_disconnected_session()
    .unwrap_or_else(|e| panic!("session: {}", e.cause()));
    session.connect().unwrap();
    // Keep unchanged.txt a partial in-sync placeholder. A fully hydrated clean
    // file correctly requires native hashing, even when it is read-only. Its
    // immutable bytes are already resident in CAS through seed preparation.
    hydrate_originals(&f.checkout, seed_readme);
    assert!(
        session
            .hydration_requests()
            .iter()
            .any(|r| r.source_bytes_read > 0 && r.transferred_bytes > 0)
    );
    assert!(session.callback_failures().is_empty());
    replace(&f.checkout.join("README.md"), HISTORICAL);
    let historical = session
        .capture("c13-historical-accepted", &Cancellation::default())
        .unwrap();
    assert_eq!(historical.patch.changes.len(), 1);
    replace(&f.checkout.join("README.md"), accepted);
    let ack = session
        .capture("c13-real-accepted", &Cancellation::default())
        .unwrap();
    assert_eq!(ack.patch.changes.len(), 1);
    assert_eq!(ack.patch.changes[0].path.as_str(), "README.md");
    // This path was explicitly registered as a writable PRESENT file before
    // bootstrap. No unknown-path or Capture authority guard is bypassed.
    replace(&f.checkout.join("recovery.txt"), recovered);
    let result = test(&f, &session, accepted, recovered);
    session.disconnect().unwrap();
    result
}

fn publish(
    f: &Fixture,
    session: &ProviderActiveSession,
    accepted: &[u8],
    recovered: &[u8],
) -> PathBuf {
    publish_mode(f, session, accepted, recovered, false, false, false)
}

fn publish_retries(
    f: &Fixture,
    session: &ProviderActiveSession,
    accepted: &[u8],
    recovered: &[u8],
) -> PathBuf {
    publish_mode(f, session, accepted, recovered, true, false, false)
}

fn publish_mode(
    f: &Fixture,
    session: &ProviderActiveSession,
    accepted: &[u8],
    recovered: &[u8],
    retry: bool,
    missing_preimage: bool,
    reserve_unchanged: bool,
) -> PathBuf {
    let cancel = Cancellation::default();
    let held = &session.source.held;
    let anchor = session.transition_source(&cancel).unwrap();
    let full = {
        let _locks = held
            .family
            .locks(
                &held.artifacts.allocation.format.identity.worktree_id,
                false,
            )
            .unwrap();
        capture_state::read_live(held, Some(&session.source.root), Some(&cancel))
            .unwrap_or_else(|e| panic!("source read: {}", e.cause))
    };
    assert_eq!(full.capture.patches.len(), 2);
    let mut checkpoint = session.session.export_checkpoint().unwrap();
    let mut image = super::super::managed_image::complete::plan(
        &mut checkpoint,
        session.source.as_ref(),
        &full.active.source,
        &full.active.activated.snapshot,
        &cancel,
    )
    .unwrap();
    assert!(
        matches!(
            &image
                .retained()
                .files()
                .iter()
                .find(|file| file.path.as_str() == "unchanged.txt")
                .unwrap()
                .manifest,
            super::super::managed_image::ManagedManifest::Source(_)
        ),
        "unchanged file must exercise the immutable Source reader"
    );
    let parent = session.session.guard_export_parent(&f.outer).unwrap();
    let parent_path = parent.root().to_str().unwrap().to_owned();
    let parent_identity = parent.filesystem_identity();
    drop(parent);
    let mut destination_identity = held.artifacts.allocation.format.identity.clone();
    destination_identity.worktree_id = WorktreeId::parse("worktree:c13-complete-ready").unwrap();
    let placeholder = |count| ImportRowset {
        count,
        digest: format!("blake3:{}", "0".repeat(64)),
    };
    let mut segment_hash = blake3::Hasher::new();
    segment_hash.update(b"layrs-projection-provider-import-segments-v1\0");
    segment_hash.update(&0u64.to_be_bytes());
    let source = &full.active.source;
    let a = &held.artifacts.allocation;
    let prepared = held.artifacts.prepared.as_ref().unwrap();
    let mut prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Provider {
            identity: a.format.identity.clone(),
            format_digest: crate::projection::provider::records::codec::hash(
                "layrs-projection-provider-import-format-v1",
                &a.format.bytes().unwrap(),
            ),
            private_schema: 8,
            allocation: a.data_reference().unwrap(),
            prepared: prepared.data_reference().unwrap(),
            binding: crate::projection::provider::records::ProviderBootstrapBinding::decode(
                &held.binding.encode().unwrap(),
            )
            .unwrap(),
            activation_map_id: full.active.activation.digest().unwrap(),
            version: ImportVersion {
                generation: full.active.activated.generation,
                snapshot_digest: full.active.activated.digest.clone(),
                selection_digest: full.active.activation.selection_digest.clone(),
                capture_count: full.capture.patches.len() as u64,
                capture_head: Some(full.capture.patches.last().unwrap().digest.clone()),
                effective_image_digest: image.digest().into(),
            },
        },
        destination_identity,
        inherited_segments: ImportRowset {
            count: 0,
            digest: format!("blake3:{}", segment_hash.finalize()),
        },
        ancestry_root_receipt: None,
        closure: ImportClosure {
            denial_observations: None,
            restore_history: None,
            snapshots: placeholder(full.active.activated.generation + 1),
            selections: placeholder(1),
            captures: placeholder(full.capture.patches.len() as u64),
            publications: placeholder(0),
            catalogs: placeholder(1),
            descriptors: placeholder(source.descriptors().len() as u64),
            activation_maps: placeholder(1),
            transition_receipts: placeholder(0),
            companions: placeholder(0),
            companion_files: placeholder(0),
            companion_deletions: placeholder(0),
            content_mappings: placeholder(full.capture.content.len() as u64),
        },
    };
    let (observation, rows, derived, metadata) = {
        let _locks = held
            .family
            .locks(&a.format.identity.worktree_id, false)
            .unwrap();
        super::super::managed_destination::valid(
            database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
                let proof = validation::private_active_publication_with_source(
                    c,
                    a,
                    prepared,
                    &held.binding,
                    Some(session.source.root.registration().root_id()),
                    Some(&cancel),
                )?;
                let refs = || ImportSourceReferences::Provider {
                    allocation: a,
                    prepared,
                    activation: &proof.active.activation,
                };
                let meta = import_validation::read_source_metadata(
                    c,
                    &a.format,
                    ImportDatabaseKind::Provider,
                    &prefix,
                    refs(),
                    image.digest(),
                    Some(&cancel),
                )?;
                prefix.closure = import_validation::compute_scoped_closure(
                    c,
                    ImportDatabaseKind::Provider,
                    &prefix,
                    &meta,
                    Some(&cancel),
                )?;
                let closure = import_validation::verify_source_publication(
                    c,
                    &a.format,
                    &prefix,
                    refs(),
                    &proof,
                    image.digest(),
                    Some(&cancel),
                )?;
                let input = SourceObservationData {
                    prefix: &prefix,
                    identity: &a.format.identity,
                    snapshot: &proof.active.activated.snapshot,
                    snapshot_parent_digest: Some(&proof.active.activation.source_digest),
                    selection: source.selection(),
                    selection_generation: source.commitment().source_generation,
                    selection_snapshot_digest: &source.commitment().snapshot_digest,
                };
                let mut fresh = source.initial().clone();
                fresh.observed_authorization_epoch += 1;
                let denied = fresh
                    .entries
                    .iter_mut()
                    .find(|e| e.path.as_str() == "recovery.txt")
                    .unwrap();
                denied.writable = false;
                denied.state = EntryState::Reserved;
                if reserve_unchanged {
                    let entry = fresh
                        .entries
                        .iter_mut()
                        .find(|e| e.path.as_str() == "unchanged.txt")
                        .unwrap();
                    entry.state = EntryState::Reserved;
                    entry.writable = false;
                }
                let policy = fresh.observed_policy_set_id.clone();
                let epoch = fresh.observed_authorization_epoch;
                let descriptors = source
                    .descriptors()
                    .iter()
                    .filter(|d| d.path.as_str() != "recovery.txt")
                    .filter(|d| !reserve_unchanged || d.path.as_str() != "unchanged.txt")
                    .map(|d| ProviderSourceDescriptor {
                        path: d.path.clone(),
                        manifest: d.manifest.clone(),
                    })
                    .collect();
                let observation = TransitionObservationData::normalize(
                    input,
                    prefix.destination_identity.clone(),
                    proof.active.activated.generation + 1,
                    fresh,
                    source.selection().clone(),
                    descriptors,
                    ObservationCoverageData::default(),
                    AuthorityObservationData::ReobservedExpandedScope {
                        policy_set_id: policy,
                        authorization_epoch: epoch,
                    },
                    Some(&cancel),
                )?;
                let mut all = BTreeMap::new();
                let historical_manifest = manifest(HISTORICAL, true);
                assert!(proof.capture.content.iter().any(|content| {
                    content.file_id == historical_manifest.file_id().as_str()
                        && content.manifest_id == historical_manifest.id().as_str()
                        && content.bytes == historical_manifest.byte_length()
                }));
                all.insert(
                    (
                        historical_manifest.file_id().clone(),
                        historical_manifest.id().clone(),
                    ),
                    historical_manifest,
                );
                for m in source.descriptors().iter().map(|d| &d.manifest).chain(
                    image
                        .retained()
                        .files()
                        .iter()
                        .map(|f| f.manifest.manifest()),
                ) {
                    all.insert((m.file_id().clone(), m.id().clone()), m.clone());
                }
                let manifests = all.into_values().collect::<Vec<_>>();
                // Same measured/conservative envelope convention as existing
                // managed recipes. Q_B itself is independently derived exactly.
                let metadata = closure.metadata_bytes(c, &prefix, Some(&cancel))?
                    + 2 * ProviderTransitionIntentRecord::MAX_BYTES as u64
                    + 8 * 65536;
                let rows = source_sql::with_source(
                    c,
                    &a.format,
                    &prefix,
                    &closure,
                    &proof,
                    &observation,
                    Some(&cancel),
                    |source| {
                        image.reconcile_publication_retained(
                            source,
                            &observation,
                            &proof,
                            &manifests,
                            metadata,
                            &cancel,
                        )
                    },
                )?;
                let derived = super::super::managed_plan::derive_reconciled_data(
                    &prefix,
                    &rows,
                    &observation,
                    prefix.destination_identity.clone(),
                    &parent_path,
                    &parent_identity,
                    "00000000000000000000000000000071",
                    "00000000000000000000000000000072",
                )?;
                Ok((observation, rows, derived, metadata))
            })
            .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(rows.overlay().len(), 1);
    assert_eq!(
        rows.authorized_files().len(),
        if reserve_unchanged { 1 } else { 2 }
    );
    assert_eq!(
        rows.header().unwrap().file_count,
        if reserve_unchanged { 2 } else { 1 }
    );
    assert_eq!(rows.manifests().len(), 6);
    let planned = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: derived.transaction_id.clone(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: full.active.activated.generation,
        plan: derived.plan,
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    planned.encode().unwrap();
    let attempt = CaptureAttemptLock::acquire(&held.artifacts, &held.family).unwrap();
    let mut close = Close::default();
    super::super::managed_journal::begin_complete(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &mut image,
        &planned,
        &mut close,
        &cancel,
    )
    .unwrap();
    let (mut destination, copying) =
        super::managed_allocation_tests::bound_holder(session, &attempt, &planned);
    destination
        .publish_format(
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &copying,
            &cancel,
        )
        .unwrap();
    super::super::managed_destination::register_and_copy(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &copying,
        &destination,
        &mut close,
        &cancel,
    )
    .unwrap();
    let mut staging = ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
    staging.state = TransitionState::Staging;
    staging.reservation = Some(staging.reservation_operation().unwrap());
    super::super::managed_staging::stage_complete(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &copying,
        &staging,
        &destination,
        &mut image,
        rows.manifests(),
        &mut close,
        &cancel,
    )
    .unwrap();
    let binding = {
        let family = &a.family;
        let proof = destination.proof();
        let shared = proof.format.shared.as_ref().unwrap();
        let operation = staging.reservation_operation().unwrap();
        crate::projection::shared::exact_content::ExactBinding {
            family_incarnation: family.incarnation.clone(),
            family_root_identity: family.root_identity.clone(),
            family_marker_digest: family.marker_digest.clone(),
            family_catalog_identity: family.catalog_file_identity.clone(),
            plan_digest: staging.plan.digest().unwrap(),
            prefix_id: staging.plan.source_prefix_id.clone(),
            destination_proof_digest: proof.digest(&staging.plan).unwrap(),
            transaction_id: staging.transaction_id.clone(),
            owner: proof.format.identity.worktree_id.clone(),
            database_identity: shared.database_identity.clone(),
            reservation: operation.clone(),
            operation,
            quota_bytes: staging.plan.quota_bytes,
        }
    };
    let publication =
        SourceCompanionPublication::derive(a, &staging, &binding, &rows, Some(&cancel)).unwrap();
    let mut pins = super::super::managed_content::CompanionPins::default();
    let requests = session.hydration_requests().len();
    let writer_baseline = (close.opens, close.closes);
    if missing_preimage {
        let databases = [
            held.artifacts.database.path(),
            destination.destination_database().path(),
            held.family.catalog.path(),
        ];
        let before = databases
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let error = super::super::managed_content::commit_companion(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &destination,
            &mut image,
            &staging,
            &publication,
            &anchor,
            &observation,
            metadata,
            &mut pins,
            &mut close,
            &cancel,
        )
        .err()
        .expect("descriptor-only cold preimage must refuse before reservation");
        let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        let mut missing = false;
        while let Some(value) = cause {
            if let Some(io) = value.downcast_ref::<std::io::Error>() {
                missing |= io.kind() == std::io::ErrorKind::NotFound;
            }
            cause = value.source();
        }
        assert!(
            missing,
            "cold refusal must retain the missing CAS I/O cause: {error}"
        );
        assert_eq!(
            (close.opens, close.closes),
            writer_baseline,
            "cold refusal opened a writer: {error}"
        );
        for (path, bytes) in databases.iter().zip(before) {
            assert_eq!(
                fs::read(path).unwrap(),
                bytes,
                "cold refusal mutated {}",
                path.display()
            );
        }
        assert_eq!(
            session.hydration_requests().len(),
            requests,
            "commit attempted implicit hydration"
        );
        return destination.root().to_owned();
    }
    if retry {
        let mut faults = RetryClose {
            source_owner: a.format.identity.worktree_id.as_str().into(),
            destination_owner: staging
                .plan
                .source_prefix
                .destination_identity
                .worktree_id
                .as_str()
                .into(),
            ..RetryClose::default()
        };
        for phase in 0..11 {
            faults.phase = phase;
            faults.refused = false;
            let previous_opens = faults.normal.opens;
            let error = super::super::managed_content::commit_companion(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                &attempt,
                &destination,
                &mut image,
                &staging,
                &publication,
                &anchor,
                &observation,
                metadata,
                &mut pins,
                &mut faults,
                &cancel,
            )
            .err()
            .expect("selected durable phase must withhold ACK");
            assert!(
                faults.refused,
                "phase {phase} failed before the selected ACK: {error}"
            );
            assert!(
                error
                    .to_string()
                    .contains("C13 selected durable phase close ACK withheld"),
                "original ACK error lost: {error}"
            );
            assert_eq!(faults.normal.opens, faults.normal.closes);
            if !matches!(phase, 3 | 7) {
                assert_eq!(
                    faults.normal.opens - previous_opens,
                    1,
                    "phase {phase} rewrote an already durable predecessor"
                );
            }
        }
    }
    let receipt = if retry {
        super::super::managed_content::commit_companion(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &destination,
            &mut image,
            &staging,
            &publication,
            &anchor,
            &observation,
            metadata,
            &mut pins,
            &mut close,
            &cancel,
        )
        .unwrap()
    } else {
        let mut lease = super::super::managed_content::ManagedImportLease::open_companion(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &destination,
            &mut image,
            &staging,
            &publication,
            &anchor,
            &observation,
            metadata,
            &cancel,
        )
        .unwrap();
        lease
            .reserve_source_companion(&publication, &mut close, &cancel)
            .unwrap();
        lease.reserve_plan(&mut close, &cancel).unwrap();
        for manifest in rows.manifests() {
            lease
                .install_exact(manifest.id(), &mut close, &cancel)
                .unwrap();
        }
        let protected = lease.protect_exact(&mut close, &cancel).unwrap();
        lease
            .assert_unprotected_source_refuses_receipt(&protected, &cancel)
            .unwrap();
        let source = lease
            .protect_source_companion(&publication, &mut close, &cancel)
            .unwrap();
        let checkout = lease
            .copy_checkout(&protected, &mut close, &cancel)
            .unwrap();
        let private = lease
            .finalize_private_ready(&protected, &checkout, &mut close, &cancel)
            .unwrap();
        let mut replay = Close::default();
        let private_replay = lease
            .finalize_private_ready(&protected, &checkout, &mut replay, &cancel)
            .unwrap();
        assert_eq!(private.receipt(), private_replay.receipt());
        let family = lease
            .publish_family_ready(&private, &mut close, &cancel)
            .unwrap();
        let family_replay = lease
            .publish_family_ready(&private, &mut replay, &cancel)
            .unwrap();
        assert_eq!(family.receipt(), family_replay.receipt());
        lease
            .assert_companion_ready_refusals(&family, &source, &cancel)
            .unwrap();
        lease
            .publish_source_companion(&family, &source, &mut close, &cancel)
            .unwrap();
        lease
            .publish_source_companion(&family, &source, &mut replay, &cancel)
            .unwrap();
        assert_eq!((replay.opens, replay.closes), (0, 0));
        crate::projection::provider::records::import::ProviderTransitionReceiptRecord::decode(
            &family.receipt().encode().unwrap(),
        )
        .unwrap()
    };
    if retry {
        assert_eq!(
            (close.opens, close.closes),
            writer_baseline,
            "Published ACK recovery must open no writer"
        );
    }
    assert!(receipt.companion_id.is_some());
    assert_eq!(receipt.protected_manifests.count, 6);
    assert_eq!(image.retained_offline_manifests().len(), 4);
    // Exercise the real offline producer explicitly even when the exact engine
    // adopts already-indexed CAS chunks without calling its producer callback.
    for bytes in [README_BASE, RECOVERY_BASE, UNCHANGED, HISTORICAL] {
        let expected = manifest(bytes, true);
        assert!(image.retained_offline_manifests().contains(&expected));
        let mut produced = Vec::new();
        image
            .copy_required_chunk(&expected, 0, &mut produced, &cancel)
            .unwrap();
        assert_eq!(produced, bytes);
    }
    for file in [&held.artifacts.database, destination.destination_database()] {
        super::super::managed_destination::valid(database_io::read_cancellable_strict(file, &cancel, |c| {
            publication.rows().require_new_rows(c, Some(&cancel))?;
            let actual: Vec<u8> = c.query_row("SELECT payload FROM projection_provider_transition_receipts WHERE transaction_id=?1", [staging.transaction_id.as_str()], |r| r.get(0))?;
            assert_eq!(actual, receipt.encode()?);
            Ok(())
        }).unwrap()).unwrap();
    }
    assert_eq!(session.hydration_requests().len(), requests);
    assert_eq!(close.opens, close.closes);
    assert_eq!(
        fs::read(destination.root().join("README.md")).unwrap(),
        accepted
    );
    assert!(!destination.root().join("recovery.txt").exists());
    if reserve_unchanged {
        assert!(!destination.root().join("unchanged.txt").exists());
    } else {
        assert_eq!(
            fs::read(destination.root().join("unchanged.txt")).unwrap(),
            UNCHANGED
        );
    }
    assert_eq!(
        fs::read(f.checkout.join("recovery.txt")).unwrap(),
        recovered
    );
    // Only the root path escapes. Complete image, both owners, source native
    // session, CAS/checkout proof handles and all SQL scopes drop normally.
    destination.root().to_owned()
}

#[test]
#[ignore = "UNQUALIFIED integrated C13 complete/Ready route; ROOT serial child3600/outer3660 release required; bounds unmeasured"]
fn c13_real_accepted_edit_denied_present_companion_reaches_c_ready_and_b_published() {
    child::run(
        concat!(
            module_path!(),
            "::c13_real_accepted_edit_denied_present_companion_reaches_c_ready_and_b_published"
        ),
        3600,
        || {
            child::declare();
            let destination = with_source(publish);
            let cancel = Cancellation::default();
            let reopened = ManagedReadyInspection::open(&destination, &cancel).unwrap();
            reopened.require(&cancel).unwrap();
            assert!(reopened.ready().plan.companion_id.is_some());
            assert_eq!(reopened.required().len(), 6);
            assert_eq!(reopened.expected().files().len(), 2);
            assert!(reopened.journal().is_complete());
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED C13 same-owner ACK recovery; ROOT serial child3600/outer3660 release required"]
fn c13_same_owner_commit_retries_install_checkout_source_ready_and_published_acks() {
    child::run(
        concat!(
            module_path!(),
            "::c13_same_owner_commit_retries_install_checkout_source_ready_and_published_acks"
        ),
        3600,
        || {
            child::declare();
            let destination = with_source(publish_retries);
            let cancel = Cancellation::default();
            let reopened = ManagedReadyInspection::open(&destination, &cancel).unwrap();
            reopened.require(&cancel).unwrap();
            assert!(reopened.ready().plan.companion_id.is_some());
            assert_eq!(reopened.required().len(), 6);
            assert_eq!(reopened.expected().files().len(), 2);
            assert!(reopened.journal().is_complete());
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED cold original preimage refusal; ROOT serial child3600/outer3660 release required"]
fn c13_missing_cold_preimage_refuses_before_reservations_without_implicit_hydration() {
    child::run(
        concat!(
            module_path!(),
            "::c13_missing_cold_preimage_refuses_before_reservations_without_implicit_hydration"
        ),
        3600,
        || {
            child::declare();
            with_source_configuration(false, |f, session, accepted, recovered| {
                publish_mode(f, session, accepted, recovered, false, true, false)
            });
        },
    );
}

#[test]
#[ignore = "C13 unchanged Reserved companion and autonomous reopen regression; ROOT serial child3600/outer3660"]
fn c13_unchanged_reserved_file_survives_publication_and_sourceless_reopen() {
    child::run(
        concat!(
            module_path!(),
            "::c13_unchanged_reserved_file_survives_publication_and_sourceless_reopen"
        ),
        3600,
        || {
            child::declare();
            let destination = with_source(|f, session, accepted, recovered| {
                publish_mode(f, session, accepted, recovered, false, false, true)
            });
            let cancel = Cancellation::default();
            let reopened = ManagedReadyInspection::open(&destination, &cancel).unwrap();
            reopened.require(&cancel).unwrap();
            assert_eq!(reopened.ready().plan.companion_files.count, 2);
            assert_eq!(reopened.required().len(), 6);
            assert_eq!(reopened.expected().files().len(), 1);
            assert!(reopened.journal().is_complete());
        },
    );
}
