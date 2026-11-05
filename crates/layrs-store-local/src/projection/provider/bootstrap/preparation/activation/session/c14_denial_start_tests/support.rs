//! Source, snapshot, refusal and observation helpers of the C14 denial start tests.
use super::*;

pub(super) fn source_with_immutable(case: PreimageCase) -> ProviderBootstrapSource {
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

pub(super) fn require_retained_preimage(
    image: &super::super::super::managed_image::complete::VerifiedCompleteManagedImage<'_, '_>,
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

/// Explicit server-side fixture input, established before bootstrap/observation.
/// An absent writable leaf grants this test's later creation; native existence
/// is never used to infer that grant.
pub(super) fn source_for_configuration(
    case: PreimageCase,
    capture: bool,
) -> ProviderBootstrapSource {
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

// Read-only logical evidence covers all application tables: restoring this
// baseline proves the exact B row was the only catalog DATA removed.
pub(super) fn catalog_snapshot(f: &Fixture, cancel: &Cancellation) -> CatalogSnapshot {
    let family =
        crate::projection::shared::provider_access::FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, false).unwrap();
    super::super::super::managed_destination::valid(
        database_io::read_cancellable_strict(&family.catalog, cancel, table_snapshot).unwrap(),
    )
    .unwrap()
}

pub(super) fn table_snapshot(c: &rusqlite::Connection) -> CauseResult<CatalogSnapshot> {
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

pub(super) fn require_validation_message(error: Cause, expected: &str) {
    require_validation_message_ref(&error, expected);
}

pub(super) fn require_validation_message_ref(error: &Cause, expected: &str) {
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

pub(super) fn require_denial_guard(error: Cause) {
    require_validation_message(error, "managed allocation requires a denial-aware driver");
}

#[allow(clippy::too_many_arguments)]
pub(super) fn require_denial_copy_state(
    held: &crate::projection::provider::bootstrap::outcome::PublicationPins,
    owner: &super::super::super::managed_journal::denial_start::PendingDenialStart<'_, '_, '_, '_>,
    copying: &ProviderTransitionIntentRecord,
    destination: &super::super::super::managed_allocation::ManagedDestinationAllocation<'_>,
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
        let intent = super::super::super::managed_destination::destination_intent(copying)?;
        current.require_destination(&intent, Some(cancel))?;
        let source_body = crate::projection::provider::denied_coverage_storage::read_required(
            source, copying.plan.denial_coverage.as_ref().unwrap(), Some(cancel))?;
        assert_eq!(&source_body, coverage.record());
        let registration = super::super::super::managed_destination::valid(database_io::read_cancellable_strict(
            &held.family.catalog, cancel, |c| held.family.validate_import_initializing(
                c, a, p, &proof.active.activation, &proof.capture, copying, Some(cancel)),
        )?)?;
        assert_eq!(registration, crate::projection::shared::provider_access::ImportRegistration::Initializing);
        if copied {
            super::super::super::managed_destination::valid(database_io::read_cancellable_strict(
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

pub(super) fn observe(
    active: &PrivateActiveCaptureValidation,
    identity: &crate::projection::ProjectionIdentity,
    planned: &ProviderTransitionIntentRecord,
    epoch_increment: u64,
    cancel: &Cancellation,
) -> TransitionObservationData {
    observe_with_reserved_image(active, identity, planned, epoch_increment, false, cancel)
}

pub(super) fn observe_with_reserved_image(
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

pub(super) fn run_start(retry: bool, pending_capture: bool, recovery: Option<RecoveryCase>) {
    run_start_configuration(
        retry,
        pending_capture,
        recovery,
        PreimageCase::DescriptorOnly,
        None,
    );
}
