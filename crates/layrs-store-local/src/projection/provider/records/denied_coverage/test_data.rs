//! Pure normalized DATA shared by codec and guarded writer recipes.
use super::*;
use crate::projection::{
    database,
    provider::{
        records::import::{ImportClosure, ImportRowset, ProviderImportKind},
        transition_observation::{DeniedPathData, ObservationCoverageData, SourceObservationData},
        validation::tests::fixture::Fixture,
    },
    selection,
};
use layrs_core::{
    projection::{EntryState, ProjectionEntry, ProjectionSelection},
    scm::WorktreeId,
};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}

pub(in crate::projection::provider) fn observation(
) -> (ProviderImportPrefixRecord, TransitionObservationData) {
    let fixture = Fixture::new("G1", false);
    let identity = fixture.allocation.format.identity.clone();
    let mut destination = identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:coverage-normalized").unwrap();
    let mut original = fixture.source.initial().clone();
    original.entries.push(ProjectionEntry {
        path: path("created.txt"),
        target: None,
        executable: false,
        writable: true,
        state: EntryState::Absent,
    });
    original.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let selected = ProjectionSelection::All;
    let snapshot_digest = database::digest(
        0,
        None,
        &codec::encode(&original, codec::SNAPSHOT_MAX).unwrap(),
    );
    let selection_digest = database::digest(
        0,
        Some(&snapshot_digest),
        &selection::encode(&selected).unwrap(),
    );
    let row = |count| ImportRowset {
        count,
        digest: TOKEN.into(),
    };
    let prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            admission: None,
            checkout_identity: None,
            identity: identity.clone(),
            format_digest: TOKEN.into(),
            private_schema: 6,
            version: ImportVersion {
                generation: 0,
                snapshot_digest: snapshot_digest.clone(),
                selection_digest,
                capture_count: 0,
                capture_head: None,
                effective_image_digest: "a".repeat(64),
            },
        },
        destination_identity: destination.clone(),
        inherited_segments: row(0),
        ancestry_root_receipt: None,
        closure: ImportClosure {
            denial_observations: None,
            snapshots: row(1),
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
            restore_history: None,
        },
    };
    let mut fresh = original.clone();
    fresh
        .entries
        .retain(|entry| entry.path.as_str() != "created.txt");
    fresh.observed_authorization_epoch += 1;
    let authority = AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: fresh.observed_policy_set_id.clone(),
        authorization_epoch: fresh.observed_authorization_epoch,
    };
    let normalized = TransitionObservationData::normalize(
        SourceObservationData {
            prefix: &prefix,
            identity: &identity,
            snapshot: &original,
            snapshot_parent_digest: None,
            selection: &selected,
            selection_generation: 0,
            selection_snapshot_digest: &snapshot_digest,
        },
        destination,
        1,
        fresh,
        selected.clone(),
        fixture
            .source
            .descriptors()
            .iter()
            .map(
                |descriptor| crate::projection::provider::records::ProviderSourceDescriptor {
                    path: descriptor.path.clone(),
                    manifest: descriptor.manifest.clone(),
                },
            )
            .collect(),
        ObservationCoverageData {
            denied_paths: vec![DeniedPathData {
                path: path("created.txt"),
            }],
        },
        authority,
        None,
    )
    .unwrap();
    (prefix, normalized)
}
