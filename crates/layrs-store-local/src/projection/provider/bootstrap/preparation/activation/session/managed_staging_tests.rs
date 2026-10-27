//! Source and destination Staging with a retained native image checkpoint.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::{capture_family, fixture::Fixture, transition_capture::planned_for_image},
    capture_lock::CaptureAttemptLock,
    database_io::{self, WriterCloseObserver},
    import_validation::{self, ImportDatabaseKind},
    process_profile::tests::child,
    records::{
        import::ImportSourceReferences,
        transition::{ProviderTransitionIntentRecord, TransitionState},
        Record,
    },
    transition_sql::SourceIntentEffect,
    validation,
};
use layrs_core::scm::{canonical_json, FileManifest};
use rusqlite::{ffi, Connection, OpenFlags};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Default)]
struct Close {
    calls: usize,
}
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(succeeded);
        self.calls += 1;
        None
    }
}

#[derive(Default)]
struct RefuseSourceAcknowledgement {
    closes: usize,
}
impl WriterCloseObserver for RefuseSourceAcknowledgement {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(succeeded, "source Staging COMMIT must close physically");
        self.closes += 1;
        (self.closes == 1).then(|| invalid("source Staging close acknowledgement withheld"))
    }
}

pub(super) fn manifests_for(
    source: &crate::projection::provider::records::ProviderBootstrapSource,
    image: &super::super::managed_image::VerifiedManagedImage<'_, '_>,
) -> Vec<FileManifest> {
    let mut unique = BTreeMap::<(String, String), FileManifest>::new();
    for descriptor in source.descriptors() {
        let manifest = descriptor.manifest.clone();
        unique.insert(
            (
                manifest.file_id().as_str().into(),
                manifest.id().as_str().into(),
            ),
            manifest,
        );
    }
    for file in image.files() {
        let manifest = file.manifest.manifest().clone();
        unique.insert(
            (
                manifest.file_id().as_str().into(),
                manifest.id().as_str().into(),
            ),
            manifest,
        );
    }
    unique.into_values().collect()
}

fn remove_placeholder(path: &Path) {
    if fs::remove_file(path).is_err() {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
        fs::remove_file(path).unwrap();
    }
}

/// Unlike older allocation recipes, this fixture derives the actual protected
/// manifest rowset and capture plan from the retained image and SQL closure.
fn with_real_planned(
    test: impl FnOnce(
        &Fixture,
        &ProviderActiveSession,
        &CaptureAttemptLock,
        &ProviderTransitionIntentRecord,
    ),
) {
    with_planned_configuration(
        None,
        false,
        b"ordinary managed destination replacement",
        false,
        test,
    );
}

/// The managed-content branch upgrades before retaining any session lifecycle
/// or checkpoint. `false` deliberately makes a valid plan with no metadata
/// envelope, so the later reservation reader must reject its measured floor.
pub(super) fn with_content_planned(
    sufficient_metadata: bool,
    empty_union: bool,
    test: impl FnOnce(
        &Fixture,
        &ProviderActiveSession,
        &CaptureAttemptLock,
        &ProviderTransitionIntentRecord,
    ),
) {
    with_planned_configuration(
        Some(sufficient_metadata),
        empty_union,
        b"ordinary managed destination replacement",
        false,
        test,
    );
}

/// Keeps the same native owners and genuine plan derivation, with caller bytes
/// written before the first checkpoint and protected manifest union are formed.
pub(super) fn with_content_planned_bytes(
    replacement: &[u8],
    test: impl FnOnce(
        &Fixture,
        &ProviderActiveSession,
        &CaptureAttemptLock,
        &ProviderTransitionIntentRecord,
    ),
) {
    with_planned_configuration(Some(true), false, replacement, false, test);
}

/// Native checkout coverage adds a nested ordinary file before the first
/// retained image and durable plan are derived. Existing recipes are unchanged.
pub(super) fn with_content_planned_checkout(
    test: impl FnOnce(
        &Fixture,
        &ProviderActiveSession,
        &CaptureAttemptLock,
        &ProviderTransitionIntentRecord,
    ),
) {
    with_planned_configuration(Some(true), false, b"provider base content", true, test);
}

fn with_planned_configuration(
    content_metadata: Option<bool>,
    empty_union: bool,
    replacement: &[u8],
    nested_checkout: bool,
    test: impl FnOnce(
        &Fixture,
        &ProviderActiveSession,
        &CaptureAttemptLock,
        &ProviderTransitionIntentRecord,
    ),
) {
    let (fixture, holder, allocation, prepared, binding) = if nested_checkout {
        // The nested creation is authorized by an explicit writable Absent
        // source entry before bootstrap. Do not invent a grant after planning.
        let old = Fixture::writable_file_source();
        let mut snapshot = old.initial().clone();
        snapshot
            .entries
            .push(layrs_core::projection::ProjectionEntry {
                path: layrs_core::scm::RepoPath::parse("dir/extra.txt").unwrap(),
                target: None,
                executable: false,
                writable: true,
                state: layrs_core::projection::EntryState::Absent,
            });
        snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
        let descriptors = old
            .descriptors()
            .iter()
            .map(
                |d| crate::projection::provider::records::ProviderSourceDescriptor {
                    path: d.path.clone(),
                    manifest: d.manifest.clone(),
                },
            )
            .collect();
        let source = crate::projection::provider::records::ProviderBootstrapSource::validate(
            snapshot,
            old.selection().clone(),
            descriptors,
        )
        .unwrap();
        capture_family::active_holder_with_source(Some(source))
    } else {
        capture_family::active_holder_with_writable_file_private()
    };
    let mut session = if content_metadata.is_some() {
        drop(holder);
        let effect = crate::projection::provider::family_upgrade::upgrade_source_family(
            fixture.family.root(),
            &fixture.worktree,
            &allocation.transaction_id,
            &mut Close::default(),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(
            effect,
            crate::projection::shared::schema_v2::MigrationEffect::Migrated
        );
        PartialStoreFamily::open_provider_active(
            fixture.family.root(),
            &fixture.worktree,
            &binding.transaction_id,
            &binding,
        )
        .unwrap()
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("upgraded session withheld: {}", error.cause()))
    } else {
        holder
            .into_disconnected_session()
            .unwrap_or_else(|error| panic!("session withheld: {}", error.cause()))
    };
    session.connect().unwrap();
    let path = fixture.checkout.join("README.md");
    remove_placeholder(&path);
    fs::write(&path, replacement).unwrap();
    if nested_checkout {
        fs::create_dir_all(fixture.checkout.join("dir")).unwrap();
        fs::write(
            fixture.checkout.join("dir/extra.txt"),
            b"nested checkout content",
        )
        .unwrap();
    }
    let held = &session.source.held;
    let cancel = Cancellation::default();
    let active = {
        let _locks = held
            .family
            .locks(&allocation.format.identity.worktree_id, false)
            .unwrap();
        capture_state::read(held, Some(&session.source.root), Some(&cancel))
            .unwrap_or_else(|error| panic!("active read: {}", error.cause))
    };
    let mut checkpoint = session.session.export_checkpoint().unwrap();
    let mut image = super::super::managed_image::plan(
        &mut checkpoint,
        session.source.as_ref(),
        &active.active.source,
        &active.active.activated.snapshot,
        &cancel,
    )
    .unwrap();
    let mut planned = planned_for_image(&fixture, &allocation, &prepared, &binding, image.digest());
    let parent = session.session.guard_export_parent(&fixture.outer).unwrap();
    let parent_path = parent.root().to_str().unwrap().to_owned();
    let parent_identity = parent.filesystem_identity();
    drop(parent);
    let supplied = manifests_for(&active.active.source, &image);
    let legacy_metadata_bytes = if content_metadata.is_none() {
        fs::metadata(fixture.metadata.join("projection.sqlite3"))
            .unwrap()
            .len()
    } else {
        0
    };
    let derived = {
        let _locks = held
            .family
            .locks(&allocation.format.identity.worktree_id, false)
            .unwrap();
        super::super::managed_destination::valid(
            database_io::read_cancellable(&held.artifacts.database, &cancel, |c| {
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
                let derive = |metadata| {
                    super::super::managed_plan::derive(
                        &planned.plan.source_prefix,
                        &closure,
                        &image,
                        &proof,
                        proof.active.source.initial(),
                        planned.plan.source_prefix.destination_identity.clone(),
                        &parent_path,
                        &parent_identity,
                        &planned.plan.destination_leaf_nonce,
                        &planned.transaction_id,
                        &supplied,
                        metadata,
                    )
                };
                let metadata = match content_metadata {
                    None => legacy_metadata_bytes,
                    Some(false) => 0,
                    Some(true) => {
                        let baseline = derive(0)?;
                        let mut envelope = closure.metadata_bytes(
                            c,
                            &planned.plan.source_prefix,
                            Some(&cancel),
                        )?;
                        for bytes in [
                            baseline.observation.canonical_bytes().unwrap().len(),
                            baseline.placement.canonical_bytes().unwrap().len(),
                            canonical_json(&baseline.capture).unwrap().len(),
                            planned.plan.source_prefix.encode()?.len(),
                            ProviderTransitionIntentRecord::MAX_BYTES,
                            ProviderTransitionIntentRecord::MAX_BYTES,
                            65536,
                        ] {
                            envelope = envelope
                                .checked_add(bytes as u64)
                                .ok_or_else(|| invalid("owned metadata envelope overflow"))?;
                        }
                        envelope
                    }
                };
                derive(metadata)
            })
            .unwrap(),
        )
        .unwrap()
    };
    planned.plan = derived.plan;
    if empty_union {
        // Deliberately incomplete DATA from the beginning, before allocation
        // binds this exact plan. Private Staging consistency alone must not
        // grant reservation authority for an omitted source/image union.
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
        hash.update(&0u64.to_be_bytes());
        planned.plan.protected_manifests =
            crate::projection::provider::records::transition::TransitionRowset {
                count: 0,
                digest: format!("blake3:{}", hash.finalize()),
            };
    }
    planned.encode().unwrap();
    let attempt = CaptureAttemptLock::acquire(&held.artifacts, &held.family).unwrap();
    let mut close = Close::default();
    assert_eq!(
        super::super::managed_journal::begin(
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
        .unwrap(),
        SourceIntentEffect::Inserted,
    );
    assert_eq!(close.calls, 1);
    drop(image);
    drop(checkpoint);
    test(&fixture, &session, &attempt, &planned);
    drop(attempt);
    session.disconnect().unwrap();
}

fn case(gap: bool) {
    with_real_planned(|fixture, session, attempt, planned| {
        let (mut destination, copying) =
            super::managed_allocation_tests::bound_holder(session, attempt, planned);
        let cancel = Cancellation::default();
        destination
            .publish_format(
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &cancel,
            )
            .unwrap();
        let held = &session.source.held;
        let mut close = Close::default();
        super::super::managed_destination::register_and_copy(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            attempt,
            &copying,
            &destination,
            &mut close,
            &cancel,
        )
        .unwrap();
        assert_eq!(close.calls, 2);
        // Reopen the retained checkpoint after the Planned fixture's original
        // checkpoint closed; no content or namespace changed between them.
        let active = {
            let _locks = held
                .family
                .locks(
                    &held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .unwrap();
            capture_state::read(held, Some(&session.source.root), Some(&cancel))
                .unwrap_or_else(|error| panic!("active read: {}", error.cause))
        };
        let mut checkpoint = session.session.export_checkpoint().unwrap();
        let mut image = super::super::managed_image::plan(
            &mut checkpoint,
            session.source.as_ref(),
            &active.active.source,
            &active.active.activated.snapshot,
            &cancel,
        )
        .unwrap();
        let manifests = manifests_for(&active.active.source, &image);
        super::super::managed_plan::require_manifest_union(&copying.plan, &manifests).unwrap();
        let mut staging =
            ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
        staging.state = TransitionState::Staging;
        staging.reservation = Some(staging.reservation_operation().unwrap());
        staging.require_transition(&copying).unwrap();
        if gap {
            let mut refused = RefuseSourceAcknowledgement::default();
            let error = super::super::managed_staging::stage(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &staging,
                &destination,
                &mut image,
                &manifests,
                &mut refused,
                &cancel,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("acknowledgement withheld"),
                "{error}"
            );
            assert_eq!(
                refused.closes, 1,
                "no destination writer after source close refusal"
            );
            let copied = Connection::open_with_flags(
                destination.destination_database().path(),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let phase: String = copied
                .query_row(
                    "SELECT state FROM projection_provider_transitions",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                phase, "copying",
                "source Staging may precede destination CAS"
            );
            copied.close().unwrap();
            let source_database = Connection::open_with_flags(
                fixture.metadata.join("projection.sqlite3"),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let source_phase: String = source_database
                .query_row(
                    "SELECT state FROM projection_provider_transitions WHERE id=?1",
                    [staging.transaction_id.as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                source_phase, "staging",
                "source COMMIT survived ack refusal"
            );
            source_database.close().unwrap();
        }
        let first = super::super::managed_staging::stage(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            attempt,
            &copying,
            &staging,
            &destination,
            &mut image,
            &manifests,
            &mut close,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            first.source,
            if gap {
                SourceIntentEffect::AlreadyPresent
            } else {
                SourceIntentEffect::Updated
            }
        );
        assert_eq!(
            first.destination,
            crate::projection::provider::destination_sql::CopiedStagingEffect::Updated
        );
        let after_first = close.calls;
        let replay = super::super::managed_staging::stage(
            held,
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            attempt,
            &copying,
            &staging,
            &destination,
            &mut image,
            &manifests,
            &mut close,
            &cancel,
        )
        .unwrap();
        assert_eq!(replay.source, SourceIntentEffect::AlreadyPresent);
        assert_eq!(
            replay.destination,
            crate::projection::provider::destination_sql::CopiedStagingEffect::AlreadyPresent
        );
        assert_eq!(close.calls, after_first, "Staging replay opens no writer");
        let catalog = Connection::open_with_flags(
            fixture.family.root().join("metadata/catalog.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let owner = copying
            .plan
            .source_prefix
            .destination_identity
            .worktree_id
            .as_str();
        let family_state: String = catalog
            .query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |row| {
                row.get(0)
            })
            .unwrap();
        let reservations: u64 = catalog
            .query_row(
                "SELECT count(*) FROM reservations WHERE owner=?1",
                [owner],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(family_state, "initializing");
        assert_eq!(reservations, 0);
        catalog.close().unwrap();
        let copied = Connection::open_with_flags(
            destination.destination_database().path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let phase: String = copied
            .query_row(
                "SELECT state FROM projection_provider_transitions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(phase, "staging");
        copied.close().unwrap();
        drop(image);
        drop(checkpoint);
    });
}

#[test]
#[ignore = "owned audited Windows provider process; exact managed Staging"]
fn source_and_destination_stage_without_family_reservation() {
    child::run(
        concat!(
            module_path!(),
            "::source_and_destination_stage_without_family_reservation"
        ),
        240,
        || {
            child::declare();
            case(false);
        },
    );
}

#[test]
#[ignore = "owned audited Windows provider process; source Staging destination Copying crash gap"]
fn source_staging_destination_copying_gap_resumes() {
    child::run(
        concat!(
            module_path!(),
            "::source_staging_destination_copying_gap_resumes"
        ),
        240,
        || {
            child::declare();
            case(true);
        },
    );
}
