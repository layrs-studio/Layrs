//! Pure, source-defined inventories. No SQL/native fixture is opened, and no
//! DATA result here establishes network authority or durable recovery safety.
use super::super::{
    records::{
        ProviderSourceDescriptor,
        import::{
            ImportClosure, ImportRowset, ImportSource, ImportVersion, ProviderImportKind,
            ProviderImportPrefixRecord,
        },
    },
    transition_observation::{AuthorityObservationData, DeniedPathData, ObservationCoverageData},
    validation::tests::fixture::Fixture,
};
use super::*;
use crate::projection::{ProjectionIdentity, database, selection};
use layrs_core::{
    projection::{ProjectionEntry, ProjectionSelection, ProjectionSnapshot},
    scm::{Chunk, ChunkingDescriptor, WorktreeId},
};

const PARENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
fn p(path: &str) -> RepoPath {
    RepoPath::parse(path).unwrap()
}
fn manifest(bytes: &[u8], fixed: bool) -> FileManifest {
    FileManifest::from_chunks(
        if fixed {
            ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap()
        } else {
            ChunkingDescriptor::single()
        },
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap()
}

struct Data {
    identity: ProjectionIdentity,
    snapshot: ProjectionSnapshot,
    selected: ProjectionSelection,
    prefix: ProviderImportPrefixRecord,
    readme: FileManifest,
    a: FileManifest,
    b: FileManifest,
    captures: Vec<CapturedPatch>,
    mappings: BTreeMap<FileId, (FileManifestId, u64)>,
}
impl Data {
    fn new(captured_deletion: bool) -> Self {
        let fixture = Fixture::new("G1", false);
        let identity = fixture.allocation.format.identity.clone();
        let mut destination = identity.clone();
        destination.worktree_id = WorktreeId::parse("worktree:reconcile-destination").unwrap();
        let mut snapshot = fixture.source.initial().clone();
        snapshot.entries[0].state = EntryState::Unavailable;
        snapshot.entries[0].writable = true;
        let readme = fixture.source.descriptors()[0].manifest.clone();
        let a = manifest(b"captured creation A", false);
        let b = manifest(b"live edit B", true);
        snapshot.entries.push(ProjectionEntry {
            path: p("new.txt"),
            target: captured_deletion.then(|| TreeEntryTarget::file(a.file_id().clone())),
            executable: false,
            writable: true,
            state: if captured_deletion {
                EntryState::Unavailable
            } else {
                EntryState::Absent
            },
        });
        snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
        snapshot.validate().unwrap();
        let selected = ProjectionSelection::All;
        let digest = database::digest(7, Some(PARENT), &snapshot.canonical_bytes().unwrap());
        let selection_digest =
            database::digest(7, Some(&digest), &selection::encode(&selected).unwrap());
        let mut patch = CapturedPatch {
            schema_version: "layrs.projection-capture.v1".into(),
            sequence: 1,
            previous_patch_digest: None,
            projection_generation: 7,
            projection_digest: digest.clone(),
            identity: identity.clone(),
            changes: vec![CapturedChange {
                path: p("new.txt"),
                expected: captured_deletion.then(|| CapturedTarget {
                    file_id: a.file_id().clone(),
                    executable: false,
                }),
                replacement: (!captured_deletion).then(|| {
                    crate::projection::capture::CapturedFile {
                        file_id: a.file_id().clone(),
                        bytes: a.byte_length(),
                        executable: false,
                    }
                }),
            }],
            digest: String::new(),
        };
        patch.digest = patch.computed_digest().unwrap();
        let mut mappings = BTreeMap::new();
        for file in [&readme, &a] {
            mappings.insert(
                file.file_id().clone(),
                (file.id().clone(), file.byte_length()),
            );
        }
        let row = |count| ImportRowset {
            count,
            digest: TOKEN.into(),
        };
        let mut segments = blake3::Hasher::new();
        segments.update(b"layrs-projection-provider-import-segments-v1\0");
        segments.update(&0u64.to_be_bytes());
        let prefix = ProviderImportPrefixRecord {
            schema_version: "layrs.projection-provider-import-prefix.v1".into(),
            kind: ProviderImportKind::ExpandedProvider,
            source: ImportSource::Ordinary {
                admission: None,
                checkout_identity: None,
                identity: identity.clone(),
                format_digest: TOKEN.into(),
                private_schema: 6,
                version: ImportVersion {
                    generation: 7,
                    snapshot_digest: digest,
                    selection_digest,
                    capture_count: 1,
                    capture_head: Some(patch.digest.clone()),
                    effective_image_digest: "a".repeat(64),
                },
            },
            destination_identity: destination,
            ancestry_root_receipt: None,
            inherited_segments: ImportRowset {
                count: 0,
                digest: format!("blake3:{}", segments.finalize()),
            },
            closure: ImportClosure {
                denial_observations: None,
                restore_history: None,
                snapshots: row(8),
                selections: row(1),
                captures: row(1),
                publications: row(0),
                catalogs: row(0),
                descriptors: row(0),
                activation_maps: row(0),
                transition_receipts: row(0),
                companions: row(0),
                companion_files: row(0),
                companion_deletions: row(0),
                content_mappings: super::super::import_validation::content_mapping_rowset_data(
                    &mappings, None,
                )
                .unwrap(),
            },
        };
        Self {
            identity,
            snapshot,
            selected,
            prefix,
            readme,
            a,
            b,
            captures: vec![patch],
            mappings,
        }
    }
    fn input(&self) -> SourceObservationData<'_> {
        let version = match &self.prefix.source {
            ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => {
                version
            }
        };
        SourceObservationData {
            prefix: &self.prefix,
            identity: &self.identity,
            snapshot: &self.snapshot,
            snapshot_parent_digest: Some(PARENT),
            selection: &self.selected,
            selection_generation: 7,
            selection_snapshot_digest: &version.snapshot_digest,
        }
    }
    fn observation(&self, revoked: bool) -> TransitionObservationData {
        self.observation_with_extra_absent(revoked, None)
    }
    fn observation_with_extra_absent(
        &self,
        revoked: bool,
        added: Option<&str>,
    ) -> TransitionObservationData {
        let mut fresh = self.snapshot.clone();
        fresh.observed_authorization_epoch += 1;
        let mut coverage = ObservationCoverageData::default();
        if revoked {
            fresh.entries.retain(|entry| entry.path != p("new.txt"));
            coverage
                .denied_paths
                .push(DeniedPathData { path: p("new.txt") });
        }
        if let Some(path) = added {
            fresh.entries.push(ProjectionEntry {
                path: p(path),
                target: None,
                executable: false,
                writable: true,
                state: EntryState::Absent,
            });
        }
        let descriptors = fresh
            .entries
            .iter()
            .filter_map(|entry| {
                let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                    return None;
                };
                Some(ProviderSourceDescriptor {
                    path: entry.path.clone(),
                    manifest: if file_id == self.readme.file_id() {
                        self.readme.clone()
                    } else {
                        self.a.clone()
                    },
                })
            })
            .collect();
        let authority = AuthorityObservationData::ReobservedExpandedScope {
            policy_set_id: fresh.observed_policy_set_id.clone(),
            authorization_epoch: fresh.observed_authorization_epoch,
        };
        TransitionObservationData::normalize(
            self.input(),
            self.prefix.destination_identity.clone(),
            8,
            fresh,
            self.selected.clone(),
            descriptors,
            coverage,
            authority,
            None,
        )
        .unwrap()
    }
}

#[test]
fn captured_creation_live_edit_revoked_uses_effective_preimage_and_all_representations() {
    let data = Data::new(false);
    let observation = data.observation(true);
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    let path = p("new.txt");
    let readme = p("README.md");
    let image = image::from_test_inventory(
        &source,
        &[
            ImageFileData {
                path: &readme,
                manifest: &data.readme,
                executable: Some(false),
            },
            ImageFileData {
                path: &path,
                manifest: &data.b,
                executable: Some(true),
            },
        ],
        &[],
    )
    .unwrap();
    let output = reconcile(&source, &image, &observation, None).unwrap();
    assert!(output.overlay.is_empty());
    assert_eq!(output.recovery_files.len(), 1);
    let file = &output.recovery_files[0];
    assert_eq!(file.row.file_id, *data.b.file_id());
    assert_eq!(file.row.base_file_id.as_ref(), Some(data.a.file_id()));
    assert_eq!(file.row.base_executable, Some(false));
    assert_eq!(file.row.reason, CompanionReason::RevokedRights);
    assert_eq!(
        file.base_representation.as_ref().unwrap().manifest_id,
        *data.a.id()
    );
    assert!(output.representations.contains(&representation(&data.a)));
    assert!(output.representations.contains(&representation(&data.b)));
}

#[test]
fn accepted_creation_and_uncaptured_edit_are_rebased_on_fresh_absence() {
    let data = Data::new(false);
    let observation = data.observation(false);
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    let path = p("new.txt");
    let readme = p("README.md");
    for wanted in [&data.a, &data.b] {
        let image = image::from_test_inventory(
            &source,
            &[
                ImageFileData {
                    path: &readme,
                    manifest: &data.readme,
                    executable: Some(false),
                },
                ImageFileData {
                    path: &path,
                    manifest: wanted,
                    executable: Some(true),
                },
            ],
            &[],
        )
        .unwrap();
        let output = reconcile(&source, &image, &observation, None).unwrap();
        assert!(output.recovery_files.is_empty());
        assert!(output.recovery_deletions.is_empty());
        assert_eq!(output.overlay.len(), 1);
        assert_eq!(output.overlay[0].path, path);
        assert!(output.overlay[0].expected.is_none());
        let replacement = output.overlay[0].replacement.as_ref().unwrap();
        assert_eq!(replacement.file_id, *wanted.file_id());
        assert_eq!(replacement.bytes, wanted.byte_length());
        assert!(replacement.executable);
    }
}

#[test]
fn deleting_captured_creation_preserves_satisfied_intent_without_none_none_patch() {
    let data = Data::new(false);
    let observation = data.observation(false);
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    let path = p("README.md");
    let image = image::from_test_inventory(
        &source,
        &[ImageFileData {
            path: &path,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[],
    )
    .unwrap();
    let output = reconcile(&source, &image, &observation, None).unwrap();
    assert!(output.overlay.is_empty());
    assert!(output.recovery_deletions.is_empty());
    assert_eq!(output.satisfied_deletions.len(), 1);
    let deletion = &output.satisfied_deletions[0];
    assert_eq!(deletion.path, p("new.txt"));
    assert_eq!(
        deletion.source_effective_base.as_ref().unwrap().file_id,
        *data.a.file_id()
    );
    assert!(deletion.accepted_deletion.is_none());
}

#[test]
fn deleting_captured_creation_revoked_preserves_effective_file_deletion() {
    let data = Data::new(false);
    let observation = data.observation(true);
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    let path = p("README.md");
    let image = image::from_test_inventory(
        &source,
        &[ImageFileData {
            path: &path,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[],
    )
    .unwrap();
    let output = reconcile(&source, &image, &observation, None).unwrap();
    assert!(output.overlay.is_empty());
    assert!(output.satisfied_deletions.is_empty());
    assert_eq!(output.recovery_deletions.len(), 1);
    let RecoveryDeletionIntentData::FromEffectiveFile(row) = &output.recovery_deletions[0] else {
        panic!("captured creation lost its effective File base");
    };
    assert_eq!(row.path, p("new.txt"));
    assert_eq!(row.base_file_id, *data.a.file_id());
    assert_eq!(row.base_manifest_id.as_ref(), Some(data.a.id()));
    assert_eq!(row.reason, CompanionReason::RevokedRights);
}

#[test]
fn already_captured_deletion_revoked_keeps_exact_accepted_capture_provenance() {
    let data = Data::new(true);
    let observation = data.observation(true);
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    let path = p("README.md");
    let image = image::from_test_inventory(
        &source,
        &[ImageFileData {
            path: &path,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[p("new.txt")],
    )
    .unwrap();
    let output = reconcile(&source, &image, &observation, None).unwrap();
    assert!(output.overlay.is_empty());
    assert_eq!(output.recovery_deletions.len(), 1);
    let RecoveryDeletionIntentData::FromAcceptedCapture { provenance, reason } =
        &output.recovery_deletions[0]
    else {
        panic!("effective Absent was falsified as a File preimage");
    };
    assert_eq!(*reason, CompanionReason::RevokedRights);
    assert_eq!(provenance.change, data.captures[0].changes[0]);
    assert_eq!(provenance.sequence, 1);
    assert_eq!(provenance.capture_digest, data.captures[0].digest);
    assert_eq!(provenance.preimage.manifest_id, *data.a.id());
}

#[test]
fn complete_inventory_missing_fixed_catalog_unknown_mode_and_alias_are_refused() {
    let data = Data::new(false);
    let observation = data.observation(false);
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    assert!(
        image::from_test_inventory(&source, &[], &[])
            .err()
            .unwrap()
            .to_string()
            .contains("omitted fixed catalog")
    );
    let path = p("README.md");
    assert!(
        image::from_test_inventory(
            &source,
            &[ImageFileData {
                path: &path,
                manifest: &data.readme,
                executable: None
            }],
            &[]
        )
        .err()
        .unwrap()
        .to_string()
        .contains("mode is unknown")
    );
    let alias = p("readme.md");
    assert!(
        image::from_test_inventory(
            &source,
            &[
                ImageFileData {
                    path: &path,
                    manifest: &data.readme,
                    executable: Some(false)
                },
                ImageFileData {
                    path: &alias,
                    manifest: &data.b,
                    executable: Some(false)
                },
            ],
            &[]
        )
        .err()
        .unwrap()
        .to_string()
        .contains("alias")
    );
}

#[test]
fn accepted_deletion_wrong_path_expected_capture_or_prefix_sequence_is_refused() {
    let mut data = Data::new(true);
    let observation = data.observation(true);
    {
        let source = source::from_test_data(
            data.input(),
            &data.captures,
            &data.mappings,
            &[],
            &observation,
        )
        .unwrap();
        assert!(source.accepted_deletion(&p("other.txt")).is_err());
    }
    data.captures[0].changes[0]
        .expected
        .as_mut()
        .unwrap()
        .file_id = data.b.file_id().clone();
    {
        let source = source::from_test_data(
            data.input(),
            &data.captures,
            &data.mappings,
            &[],
            &observation,
        )
        .unwrap();
        assert!(
            source
                .accepted_deletion(&p("new.txt"))
                .err()
                .unwrap()
                .to_string()
                .contains("escaped source prefix")
        );
    }
    data.captures[0].changes[0]
        .expected
        .as_mut()
        .unwrap()
        .file_id = data.a.file_id().clone();
    data.captures[0].sequence = 2;
    {
        let source = source::from_test_data(
            data.input(),
            &data.captures,
            &data.mappings,
            &[],
            &observation,
        )
        .unwrap();
        assert!(
            source
                .accepted_deletion(&p("new.txt"))
                .err()
                .unwrap()
                .to_string()
                .contains("escaped source prefix")
        );
    }
    data.captures[0].sequence = 1;
    data.captures[0].digest = TOKEN.into();
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    assert!(
        source
            .accepted_deletion(&p("new.txt"))
            .err()
            .unwrap()
            .to_string()
            .contains("escaped source prefix")
    );
}

#[test]
fn same_file_multiple_manifest_representations_are_not_collapsed_and_new_grant_does_not_promote_unknown()
 {
    let mut data = Data::new(false);
    let other = manifest(b"live edit B", false);
    assert_eq!(other.file_id(), data.b.file_id());
    assert_ne!(other.id(), data.b.id());
    data.mappings.insert(
        other.file_id().clone(),
        (other.id().clone(), other.byte_length()),
    );
    // Refresh the prefix commitment after adding a genuine historical mapping.
    data.prefix.closure.content_mappings =
        super::super::import_validation::content_mapping_rowset_data(&data.mappings, None).unwrap();
    let observation = data.observation_with_extra_absent(true, Some("unknown.txt"));
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &[],
        &observation,
    )
    .unwrap();
    let path = p("new.txt");
    let readme = p("README.md");
    let unknown = p("unknown.txt");
    let image = image::from_test_inventory(
        &source,
        &[
            ImageFileData {
                path: &readme,
                manifest: &data.readme,
                executable: Some(false),
            },
            ImageFileData {
                path: &path,
                manifest: &data.b,
                executable: Some(false),
            },
            ImageFileData {
                path: &unknown,
                manifest: &data.b,
                executable: Some(false),
            },
        ],
        &[],
    )
    .unwrap();
    let output = reconcile(&source, &image, &observation, None).unwrap();
    assert!(output.overlay.is_empty());
    assert_eq!(output.recovery_files.len(), 2);
    assert_eq!(output.recovery_files[0].row.manifest_id, *other.id());
    assert_eq!(
        output.recovery_files[1].row.reason,
        CompanionReason::UnknownPath
    );
    assert!(output.representations.contains(&representation(&data.b)));
    assert!(output.representations.contains(&representation(&other)));
}

#[path = "tests/companions.rs"]
mod companion_codec_recipes;
