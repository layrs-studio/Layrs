//! Pure declared DATA cases, not network-completeness/SQL/native evidence.
use super::*;
use crate::projection::provider::{
    records::import::{ImportClosure, ImportRowset, ImportVersion, ProviderImportKind},
    validation::tests::fixture::Fixture,
};
use layrs_core::{
    projection::ProjectionEntry,
    scm::{FileManifest, WorktreeId},
};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const PARENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Source {
    identity: ProjectionIdentity,
    destination: ProjectionIdentity,
    snapshot: ProjectionSnapshot,
    selected: ProjectionSelection,
    manifest: FileManifest,
}
impl Source {
    fn new() -> Self {
        let fixture = Fixture::new("G1", false);
        let mut snapshot = fixture.source.initial().clone();
        snapshot.entries[0].writable = true;
        let selected = selected(&["README.md"]);
        let identity = fixture.allocation.format.identity.clone();
        let mut destination = identity.clone();
        destination.worktree_id = WorktreeId::parse("worktree:observation-destination").unwrap();
        Self {
            identity,
            destination,
            snapshot,
            selected,
            manifest: fixture.source.descriptors()[0].manifest.clone(),
        }
    }
    fn prefix(&self, selection_generation: u64) -> (ProviderImportPrefixRecord, String) {
        self.selected.validate_snapshot(&self.snapshot).unwrap();
        let payload = codec::encode(&self.snapshot, codec::SNAPSHOT_MAX).unwrap();
        let snapshot_digest = database::digest(7, Some(PARENT), &payload);
        let selection_snapshot = if selection_generation == 7 {
            snapshot_digest.clone()
        } else {
            database::digest(0, None, &payload)
        };
        let selection_digest = database::digest(
            selection_generation,
            Some(&selection_snapshot),
            &selection::encode(&self.selected).unwrap(),
        );
        let row = |count| ImportRowset {
            count,
            digest: TOKEN.into(),
        };
        let mut inherited = blake3::Hasher::new();
        inherited.update(b"layrs-projection-provider-import-segments-v1\0");
        inherited.update(&0u64.to_be_bytes());
        (
            ProviderImportPrefixRecord {
                schema_version: "layrs.projection-provider-import-prefix.v1".into(),
                kind: ProviderImportKind::ManagedOrdinary,
                source: ImportSource::Ordinary {
                    admission: None,
                    checkout_identity: None,
                    identity: self.identity.clone(),
                    format_digest: TOKEN.into(),
                    private_schema: 6,
                    version: ImportVersion {
                        generation: 7,
                        snapshot_digest,
                        selection_digest,
                        capture_count: 0,
                        capture_head: None,
                        effective_image_digest: "a".repeat(64),
                    },
                },
                destination_identity: self.destination.clone(),
                inherited_segments: ImportRowset {
                    count: 0,
                    digest: format!("blake3:{}", inherited.finalize()),
                },
                ancestry_root_receipt: None,
                closure: ImportClosure {
                    denial_observations: None,
                    restore_history: None,
                    snapshots: row(8),
                    selections: row(1),
                    captures: row(0),
                    publications: row(0),
                    catalogs: row(0),
                    descriptors: row(0),
                    activation_maps: row(0),
                    transition_receipts: row(0),
                    companions: row(0),
                    companion_files: row(0),
                    companion_deletions: row(0),
                    content_mappings: row(0),
                },
            },
            selection_snapshot,
        )
    }
    fn readable(&self, path: &str) -> ProjectionEntry {
        let mut entry = self
            .snapshot
            .entries
            .iter()
            .find(|e| matches!(e.target, Some(TreeEntryTarget::File { .. })))
            .unwrap()
            .clone();
        entry.path = p(path);
        entry.state = EntryState::Unavailable;
        entry
    }
    fn descriptor(&self, path: &str) -> ProviderSourceDescriptor {
        ProviderSourceDescriptor {
            path: p(path),
            manifest: self.manifest.clone(),
        }
    }
    fn fresh(&self) -> ProjectionSnapshot {
        let mut fresh = self.snapshot.clone();
        fresh.observed_authorization_epoch += 1;
        fresh
    }
    fn normalize(
        &self,
        fresh: ProjectionSnapshot,
        selection: ProjectionSelection,
        descriptors: Vec<ProviderSourceDescriptor>,
        coverage: ObservationCoverageData,
        authority: AuthorityObservationData,
    ) -> Result<TransitionObservationData> {
        self.normalize_with(fresh, selection, descriptors, coverage, authority, 7, None)
    }
    fn normalize_with(
        &self,
        fresh: ProjectionSnapshot,
        selection: ProjectionSelection,
        descriptors: Vec<ProviderSourceDescriptor>,
        coverage: ObservationCoverageData,
        authority: AuthorityObservationData,
        selection_generation: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<TransitionObservationData> {
        let (prefix, selection_snapshot) = self.prefix(selection_generation);
        TransitionObservationData::normalize(
            SourceObservationData {
                prefix: &prefix,
                identity: &self.identity,
                snapshot: &self.snapshot,
                snapshot_parent_digest: Some(PARENT),
                selection: &self.selected,
                selection_generation,
                selection_snapshot_digest: &selection_snapshot,
            },
            self.destination.clone(),
            8,
            fresh,
            selection,
            descriptors,
            coverage,
            authority,
            cancel,
        )
    }
}
fn p(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}
fn selected(paths: &[&str]) -> ProjectionSelection {
    ProjectionSelection::selected(
        paths.iter().map(|v| p(v)).collect(),
        vec![],
        PathSemantics::PortableWindows,
    )
    .unwrap()
}
fn reobserved(snapshot: &ProjectionSnapshot) -> AuthorityObservationData {
    AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: snapshot.observed_policy_set_id.clone(),
        authorization_epoch: snapshot.observed_authorization_epoch,
    }
}
fn preserved(snapshot: &ProjectionSnapshot) -> AuthorityObservationData {
    AuthorityObservationData::PreservedEpoch {
        policy_set_id: snapshot.observed_policy_set_id.clone(),
        authorization_epoch: snapshot.observed_authorization_epoch,
    }
}
fn error(result: Result<TransitionObservationData>) -> String {
    match result {
        Ok(_) => panic!("ambiguous/incomplete observation admitted"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn reobserved_expansion_accepts_new_generation_and_revoked_original_access() {
    let source = Source::new();
    let mut fresh = source.fresh();
    fresh.entries[0].state = EntryState::Reserved;
    fresh.entries[0].writable = false;
    fresh.entries.push(source.readable("new.txt"));
    let mode = reobserved(&fresh);
    let data = source
        .normalize(
            fresh,
            selected(&["README.md", "new.txt"]),
            vec![source.descriptor("new.txt")],
            ObservationCoverageData::default(),
            mode,
        )
        .unwrap();
    assert_eq!(
        (data.binding().source_generation, data.binding().generation),
        (7, 8)
    );
    assert_eq!(data.binding().source_identity, source.identity);
    assert_eq!(data.binding().destination_identity, source.destination);
    assert_eq!(data.snapshot().entries[0].state, EntryState::Reserved);
    assert!(!data.snapshot().entries[0].writable);
    assert_eq!(data.descriptors().len(), 1);
    assert_eq!(data.descriptors()[0].path.as_str(), "new.txt");
    assert!(data.selection().is_expansion_of(&source.selected));
    assert!(matches!(
        data.authority_data(),
        AuthorityObservationData::ReobservedExpandedScope { .. }
    ));
}

#[test]
fn normalization_binds_the_actual_versioned_import_prefix() {
    let source = Source::new();
    for restore_history in [false, true] {
        let (mut prefix, selection_snapshot) = source.prefix(7);
        if restore_history {
            prefix.schema_version = "layrs.projection-provider-import-prefix.v2".into();
            prefix.closure.restore_history = Some(ImportRowset {
                count: 0,
                digest: TOKEN.into(),
            });
            let ImportSource::Ordinary {
                checkout_identity, ..
            } = &mut prefix.source
            else {
                unreachable!()
            };
            *checkout_identity = Some("fs:v1:0000000000000001:0000000000000002".into());
        }
        prefix.validate_fields().unwrap();
        let canonical_prefix_id = prefix.digest().unwrap();
        let legacy_domain_id = codec::hash(
            "layrs-projection-provider-import-prefix-v1",
            &prefix.encode().unwrap(),
        );
        assert_eq!(canonical_prefix_id == legacy_domain_id, !restore_history);
        let fresh = source.fresh();
        let authority = reobserved(&fresh);
        let data = TransitionObservationData::normalize(
            SourceObservationData {
                prefix: &prefix,
                identity: &source.identity,
                snapshot: &source.snapshot,
                snapshot_parent_digest: Some(PARENT),
                selection: &source.selected,
                selection_generation: 7,
                selection_snapshot_digest: &selection_snapshot,
            },
            source.destination.clone(),
            8,
            fresh,
            source.selected.clone(),
            vec![source.descriptor("README.md")],
            ObservationCoverageData::default(),
            authority,
            None,
        )
        .unwrap();
        assert_eq!(data.binding().source_prefix_id, canonical_prefix_id);
        if restore_history {
            assert_ne!(data.binding().source_prefix_id, legacy_domain_id);
        }
    }
}

#[test]
fn denied_former_absence_is_not_fabricated_as_an_absence_or_write_grant() {
    let mut source = Source::new();
    source.snapshot.entries.push(ProjectionEntry {
        path: p("created.txt"),
        target: None,
        executable: false,
        writable: true,
        state: EntryState::Absent,
    });
    source.snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    source.selected = selected(&["README.md", "created.txt"]);
    let mut fresh = source.fresh();
    fresh
        .entries
        .retain(|entry| entry.path.as_str() != "created.txt");
    let mode = reobserved(&fresh);
    let data = source
        .normalize(
            fresh,
            source.selected.clone(),
            vec![source.descriptor("README.md")],
            ObservationCoverageData {
                denied_paths: vec![DeniedPathData {
                    path: p("created.txt"),
                }],
            },
            mode,
        )
        .unwrap();
    assert_eq!(data.denied_paths().len(), 1);
    assert_eq!(data.binding().denied_path_count, 1);
    assert!(data
        .snapshot()
        .entries
        .iter()
        .all(|e| e.path.as_str() != "created.txt"));
    assert!(data.selection().includes(&p("created.txt")));
}

#[test]
fn shrink_and_missing_old_or_added_coverage_are_refused() {
    let source = Source::new();
    let mut fresh = source.fresh();
    fresh.entries = vec![source.readable("new.txt")];
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        selected(&["new.txt"]),
        vec![source.descriptor("new.txt")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("selection shrink"));
    let mut fresh = source.fresh();
    fresh.entries = vec![source.readable("new.txt")];
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        selected(&["README.md", "new.txt"]),
        vec![source.descriptor("new.txt")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("omitted old selected namespace"));
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        selected(&["README.md", "new.txt"]),
        vec![source.descriptor("README.md")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("omitted a selected scope anchor"));
}

#[test]
fn preserved_epoch_does_not_bypass_old_access_consistency() {
    let source = Source::new();
    let mut fresh = source.snapshot.clone();
    fresh.entries.push(source.readable("new.txt"));
    let mode = preserved(&fresh);
    let data = source
        .normalize(
            fresh,
            selected(&["README.md", "new.txt"]),
            vec![source.descriptor("new.txt"), source.descriptor("README.md")],
            ObservationCoverageData::default(),
            mode,
        )
        .unwrap();
    assert!(matches!(
        data.authority_data(),
        AuthorityObservationData::PreservedEpoch { .. }
    ));
    let mut fresh = source.snapshot.clone();
    fresh.entries[0].writable = false;
    let mode = preserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![source.descriptor("README.md")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("changed original access DATA"));
    let fresh = source.fresh();
    let mode = preserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![source.descriptor("README.md")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("preserved epoch DATA changed"));
}

#[test]
fn explicit_reserved_tree_frontier_covers_known_old_descendants_without_revealing_them() {
    let mut source = Source::new();
    source.snapshot.entries[0].path = p("src/a.txt");
    let mut tree = source.snapshot.entries[0].clone();
    tree.path = p("src");
    tree.target = Some(TreeEntryTarget::Tree {
        tree_id: source.snapshot.base_root_tree_id.clone(),
    });
    tree.writable = false;
    source.snapshot.entries.insert(0, tree.clone());
    source.selected = selected(&["src/a.txt"]);
    let mut fresh = source.fresh();
    tree.state = EntryState::Reserved;
    fresh.entries = vec![tree, source.readable("new.txt")];
    let mode = reobserved(&fresh);
    let data = source
        .normalize(
            fresh,
            selected(&["src/a.txt", "new.txt"]),
            vec![source.descriptor("new.txt")],
            ObservationCoverageData::default(),
            mode,
        )
        .unwrap();
    assert!(data
        .snapshot()
        .entries
        .iter()
        .all(|entry| entry.path.as_str() != "src/a.txt"));
    assert!(data.selection().includes(&p("src/a.txt")));
}

#[test]
fn preserved_epoch_cannot_lift_an_unselected_ancestor_frontier() {
    let mut source = Source::new();
    let child = source.readable("src/a.txt");
    let mut frontier = child.clone();
    frontier.path = p("src");
    frontier.target = Some(TreeEntryTarget::Tree {
        tree_id: source.snapshot.base_root_tree_id.clone(),
    });
    frontier.state = EntryState::Reserved;
    frontier.writable = false;
    source.snapshot.entries = vec![frontier.clone()];
    source.selected = selected(&["src/a.txt"]);
    let mut fresh = source.snapshot.clone();
    frontier.state = EntryState::Unavailable;
    fresh.entries = vec![frontier, child.clone()];
    let mode = preserved(&fresh);
    assert!(error(source.normalize(
        fresh.clone(),
        source.selected.clone(),
        vec![source.descriptor("src/a.txt")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("changed original access DATA"));
    let mut omitted = source.fresh();
    omitted.entries = vec![child];
    let mode = reobserved(&omitted);
    assert!(error(source.normalize(
        omitted,
        source.selected.clone(),
        vec![source.descriptor("src/a.txt")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("omitted old selected namespace"));
    fresh.observed_authorization_epoch += 1;
    let mode = reobserved(&fresh);
    source
        .normalize(
            fresh,
            source.selected.clone(),
            vec![source.descriptor("src/a.txt")],
            ObservationCoverageData::default(),
            mode,
        )
        .unwrap();
}

#[test]
fn descriptor_coverage_and_cross_view_aliases_remain_exact() {
    let source = Source::new();
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("readable file descriptor is missing"));
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    let wrong = ProviderSourceDescriptor {
        path: p("README.md"),
        manifest: FileManifest::from_chunks(
            layrs_core::scm::ChunkingDescriptor::single(),
            &[layrs_core::scm::Chunk::new(b"different descriptor bytes".to_vec()).unwrap()],
        )
        .unwrap(),
    };
    assert_ne!(wrong.manifest.file_id(), source.manifest.file_id());
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![wrong],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("descriptor differs from exact observed target"));
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![
            source.descriptor("README.md"),
            source.descriptor("README.md")
        ],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("descriptors repeat"));
    let mut fresh = source.fresh();
    fresh.entries[0].path = p("readme.md");
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        ProjectionSelection::All,
        vec![source.descriptor("readme.md")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("alias"));
    let mut fresh = source.fresh();
    fresh.entries[0].state = EntryState::Reserved;
    fresh.entries[0].writable = false;
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![source.descriptor("README.md")],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("extra or denied descriptor"));
}

#[test]
fn changed_target_step_identity_and_denied_visible_overlap_are_refused() {
    let source = Source::new();
    let mut fresh = source.fresh();
    fresh.entries[0].target = Some(TreeEntryTarget::file(layrs_core::scm::FileId::from_bytes(
        b"other",
    )));
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("changed an immutable source target"));
    let mut fresh = source.fresh();
    fresh.base_step_id = layrs_core::scm::StepId::parse(TOKEN.replace('1', "2")).unwrap();
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("Step/root or placement"));
    let mut fresh = source.fresh();
    fresh.path_semantics = PathSemantics::CaseSensitive;
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("Step/root or placement"));
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![source.descriptor("README.md")],
        ObservationCoverageData {
            denied_paths: vec![DeniedPathData {
                path: p("README.md")
            }]
        },
        mode
    ))
    .contains("overlap"));
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        ProjectionSelection::All,
        vec![source.descriptor("README.md")],
        ObservationCoverageData {
            denied_paths: vec![DeniedPathData {
                path: p("README.md/child")
            }]
        },
        mode
    ))
    .contains("denied path scope is ambiguous"));
    let mut source = Source::new();
    source.destination.principal_id = "principal:different".into();
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("identity, version, Step/root or placement"));
}

#[test]
fn historical_selection_anchor_is_supported_and_large_rules_stop_before_normalization() {
    let source = Source::new();
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    let data = source
        .normalize_with(
            fresh,
            source.selected.clone(),
            vec![source.descriptor("README.md")],
            ObservationCoverageData::default(),
            mode,
            0,
            None,
        )
        .unwrap();
    assert_eq!(data.binding().source_generation, 7);
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    let oversized = ProjectionSelection::Selected {
        paths: vec![p("README.md"); 100_001],
        subtrees: vec![],
    };
    assert!(error(source.normalize(
        fresh,
        oversized,
        vec![],
        ObservationCoverageData::default(),
        mode
    ))
    .contains("path count exceeds bound"));
    let cancel = Cancellation::default();
    cancel.cancel();
    let fresh = source.fresh();
    let mode = reobserved(&fresh);
    assert!(matches!(
        source.normalize_with(
            fresh,
            source.selected.clone(),
            vec![],
            ObservationCoverageData::default(),
            mode,
            7,
            Some(&cancel)
        ),
        Err(super::super::recovery::error::ProviderBootstrapCause::Vfs(
            layrs_vfs::Error::Cancelled
        ))
    ));
}

#[test]
fn normalization_rejects_combined_visible_and_denied_count_before_duplicate_checks() {
    let source = Source::new();
    let fresh = source.fresh();
    assert_eq!(fresh.entries.len(), 1);
    let mode = reobserved(&fresh);
    let coverage = ObservationCoverageData {
        denied_paths: (0..100_000)
            .map(|_| DeniedPathData { path: p("denied") })
            .collect(),
    };
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![source.descriptor("README.md")],
        coverage,
        mode,
    ))
    .contains("path count exceeds bound"));
}

#[test]
fn normalization_rejects_combined_visible_and_denied_bytes_before_scope_checks() {
    let source = Source::new();
    let fresh = source.fresh();
    assert_eq!(fresh.entries.len(), 1);
    let mode = reobserved(&fresh);
    const DENIED_COUNT: usize = 99_999;
    const PATH_BYTES: usize = 8 * 1024 * 1024;
    let width = PATH_BYTES / DENIED_COUNT;
    let remainder = PATH_BYTES % DENIED_COUNT;
    let denied_paths: Vec<_> = (0..DENIED_COUNT)
        .map(|index| {
            let length = width + usize::from(index < remainder);
            DeniedPathData {
                path: p(&format!("{index:06}-{}", "x".repeat(length - 7))),
            }
        })
        .collect();
    assert_eq!(
        denied_paths
            .iter()
            .map(|row| row.path.as_str().len())
            .sum::<usize>(),
        PATH_BYTES
    );
    assert!(error(source.normalize(
        fresh,
        source.selected.clone(),
        vec![source.descriptor("README.md")],
        ObservationCoverageData { denied_paths },
        mode,
    ))
    .contains("path bytes exceed bound"));
}
