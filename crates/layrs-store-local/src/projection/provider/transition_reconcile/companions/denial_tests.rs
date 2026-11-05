//! Pure normalized observation/reconcile fixture. No SQL/native/remote proof.
use super::*;
use crate::projection::{
    database,
    provider::{
        records::{
            denied_coverage::DerivedDeniedCoverage,
            import::{ImportClosure, ImportRowset, ImportVersion, ProviderImportKind},
        },
        transition_observation::{
            AuthorityObservationData, DeniedPathData, ObservationCoverageData,
        },
        validation::tests::fixture::Fixture,
    },
    selection,
};
use layrs_core::{projection::ProjectionSelection, scm::WorktreeId};
const ID: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
pub(in crate::projection::provider) const TX: &str = "00000000000000000000000000000002";

pub(in crate::projection::provider) fn with_fixture<T>(
    expanded: bool,
    action: impl FnOnce(
        &super::super::super::records::import::ProviderImportPrefixRecord,
        &TransitionObservationData,
        &OwnedCompanionData,
        &DerivedDeniedCoverage,
    ) -> T,
) -> T {
    let fixture = Fixture::new("G1", false);
    let mut snapshot = fixture.source.initial().clone();
    snapshot.entries[0].state = EntryState::Unavailable;
    let readme = snapshot.entries[0].path.clone();
    let manifest = fixture.source.descriptors()[0].manifest.clone();
    let selected = if expanded {
        ProjectionSelection::selected(vec![readme.clone()], vec![], snapshot.path_semantics)
            .unwrap()
    } else {
        ProjectionSelection::All
    };
    let source_identity = fixture.allocation.format.identity.clone();
    let mut destination = source_identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:denial-plan-destination").unwrap();
    let source_digest = database::digest(0, None, &snapshot.canonical_bytes().unwrap());
    let selected_digest = database::digest(
        0,
        Some(&source_digest),
        &selection::encode(&selected).unwrap(),
    );
    let maps = BTreeMap::from([(
        manifest.file_id().clone(),
        (manifest.id().clone(), manifest.byte_length()),
    )]);
    let row = |count| ImportRowset {
        count,
        digest: ID.into(),
    };
    let prefix = super::super::super::records::import::ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: source_identity.clone(),
            format_digest: ID.into(),
            private_schema: 6,
            checkout_identity: None,
            admission: None,
            version: ImportVersion {
                generation: 0,
                snapshot_digest: source_digest.clone(),
                selection_digest: selected_digest,
                capture_count: 0,
                capture_head: None,
                effective_image_digest: "a".repeat(64),
            },
        },
        destination_identity: destination.clone(),
        inherited_segments: row(0),
        ancestry_root_receipt: None,
        closure: ImportClosure {
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
            content_mappings: super::super::super::import_validation::content_mapping_rowset_data(
                &maps, None,
            )
            .unwrap(),
            restore_history: None,
            denial_observations: None,
        },
    };
    let input = || SourceObservationData {
        prefix: &prefix,
        identity: &source_identity,
        snapshot: &snapshot,
        snapshot_parent_digest: None,
        selection: &selected,
        selection_generation: 0,
        selection_snapshot_digest: &source_digest,
    };
    let mut fresh = snapshot.clone();
    fresh.observed_authorization_epoch += 1;
    let authority = AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: fresh.observed_policy_set_id.clone(),
        authorization_epoch: fresh.observed_authorization_epoch,
    };
    let observed = TransitionObservationData::normalize(
        input(),
        destination,
        1,
        fresh,
        ProjectionSelection::All,
        vec![super::super::super::records::ProviderSourceDescriptor {
            path: readme.clone(),
            manifest: manifest.clone(),
        }],
        ObservationCoverageData {
            denied_paths: vec![DeniedPathData {
                path: RepoPath::parse("blocked.txt").unwrap(),
            }],
        },
        authority,
        None,
    )
    .unwrap();
    let source = super::super::source::from_test_data(input(), &[], &maps, &[], &observed).unwrap();
    let files = [ImageFileData {
        path: &readme,
        manifest: &manifest,
        executable: Some(snapshot.entries[0].executable),
    }];
    let image = super::super::image::from_test_inventory(&source, &files, &[]).unwrap();
    let owner = OwnedCompanionData::derive(
        &source,
        &image,
        &observed,
        &[manifest.clone()],
        0,
        None,
        None,
    )
    .unwrap();
    let coverage = DerivedDeniedCoverage::from_observation(&prefix, TX, &observed, None).unwrap();
    action(&prefix, &observed, &owner, &coverage)
}

#[test]
fn denial_owner_binds_expanded_observation_without_weakening_legacy_selection() {
    for expanded in [false, true] {
        with_fixture(expanded, |prefix, observed, owner, _| {
            owner
                .require_denial_plan_observation(prefix, observed)
                .unwrap();
            assert_eq!(
                owner.require_plan_observation(prefix, observed).is_err(),
                expanded
            );
        });
    }
}
