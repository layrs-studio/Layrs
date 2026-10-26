use super::*;
use crate::projection::provider::{
    records::import::{ImportClosure, ImportRowset, ImportSource, ImportVersion},
    validation::tests::fixture::Fixture,
};
use layrs_core::scm::{Chunk, ChunkingDescriptor, StoreId, WorktreeId};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const IMAGE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIR: &str = "fs:v1:1111111111111111:2222222222222222";
const NONCE: &str = "00000000000000000000000000000001";
const TX: &str = "00000000000000000000000000000002";

fn row(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: TOKEN.into(),
    }
}
fn source(writable: bool) -> (ProviderBootstrapSource, ProjectionIdentity) {
    let fixture = Fixture::new("G1", false);
    let mut initial = fixture.source.initial().clone();
    initial.entries[0].writable = writable;
    let descriptors = fixture
        .source
        .descriptors()
        .iter()
        .map(
            |descriptor| crate::projection::provider::records::ProviderSourceDescriptor {
                path: descriptor.path.clone(),
                manifest: descriptor.manifest.clone(),
            },
        )
        .collect();
    let source =
        ProviderBootstrapSource::validate(initial, fixture.source.selection().clone(), descriptors)
            .unwrap();
    (source, fixture.allocation.format.identity)
}
fn destination(source: &ProjectionIdentity) -> ProjectionIdentity {
    let mut result = source.clone();
    result.worktree_id = WorktreeId::parse("worktree:managed-destination").unwrap();
    result
}
fn prefix(
    source: &ProviderBootstrapSource,
    identity: &ProjectionIdentity,
    destination: &ProjectionIdentity,
    count: u64,
    head: Option<String>,
) -> ProviderImportPrefixRecord {
    let version = ImportVersion {
        generation: 1,
        snapshot_digest: database::digest(
            1,
            Some(&source.commitment().snapshot_digest),
            &activation_snapshot(source)
                .unwrap()
                .canonical_bytes()
                .unwrap(),
        ),
        selection_digest: source.commitment().selection_digest.clone(),
        capture_count: count,
        capture_head: head,
        effective_image_digest: IMAGE.into(),
    };
    ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: identity.clone(),
            format_digest: TOKEN.into(),
            private_schema: 8,
            version,
        },
        destination_identity: destination.clone(),
        inherited_segments: row(0),
        ancestry_root_receipt: None,
        closure: ImportClosure {
            snapshots: row(2),
            selections: row(1),
            captures: row(count),
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
    }
}
fn manifest(bytes: &[u8]) -> FileManifest {
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap()
}
fn plan<'a>(
    prefix: &ProviderImportPrefixRecord,
    source: &ProviderBootstrapSource,
    identity: &ProjectionIdentity,
    files: &[ImageFile<'a>],
    deletions: &[ImageDeletion<'a>],
    historical: &[RequiredManifest],
    supplied: &[FileManifest],
    metadata: u64,
) -> Result<ManagedPlanData> {
    derive_data(
        prefix,
        identity,
        source,
        &activation_snapshot(source).unwrap(),
        source.initial(),
        files,
        deletions,
        prefix.destination_identity.clone(),
        "C:\\managed-parent",
        DIR,
        NONCE,
        TX,
        historical,
        supplied,
        metadata,
    )
}

#[test]
fn empty_diff_still_appends_destination_capture_and_preserves_old_head() {
    let (source, identity) = source(false);
    let destination = destination(&identity);
    let d = &source.descriptors()[0];
    let input = [ImageFile {
        path: &d.path,
        manifest: &d.manifest,
        executable: d.executable,
        expected: Some((d.file_id.clone(), d.executable)),
    }];
    let manifests = [d.manifest.clone()];
    let empty = prefix(&source, &identity, &destination, 0, None);
    let first = plan(
        &empty,
        &source,
        &identity,
        &input,
        &[],
        &[],
        &manifests,
        100,
    )
    .unwrap();
    assert_eq!(first.capture.sequence, 1);
    assert!(first.capture.changes.is_empty());
    assert_eq!(first.capture.identity, destination);
    assert_eq!(first.plan.observation.generation, 2);
    assert_eq!(first.plan.placement_generation, 3);
    assert_eq!(first.capture.projection_digest, first.placement_digest);
    assert_eq!(first.plan.working_overlay.count, 0);
    let mut old = prefix(&source, &identity, &destination, 1, Some(TOKEN.into()));
    let retained = manifest(b"historical captured replacement");
    let history = [RequiredManifest::from_manifest(&retained)];
    old.closure.content_mappings = row(1);
    let protected = [d.manifest.clone(), retained];
    let next = plan(
        &old,
        &source,
        &identity,
        &input,
        &[],
        &history,
        &protected,
        100,
    )
    .unwrap();
    assert_eq!(next.capture.sequence, 2);
    assert_eq!(next.capture.previous_patch_digest.as_deref(), Some(TOKEN));
    assert_ne!(first.capture.digest, next.capture.digest);
    assert!(next.required_manifests.contains(&history[0]));
    assert_eq!(next.plan.protected_manifests.count, 2);
}

#[test]
fn fresh_identity_and_rights_must_match_entire_fixed_observation() {
    let (source, identity) = source(false);
    let destination = destination(&identity);
    let prefix = prefix(&source, &identity, &destination, 0, None);
    let d = &source.descriptors()[0];
    let input = [ImageFile {
        path: &d.path,
        manifest: &d.manifest,
        executable: d.executable,
        expected: Some((d.file_id.clone(), d.executable)),
    }];
    let manifests = [d.manifest.clone()];
    let mut fresh = source.initial().clone();
    fresh.entries[0].writable = true;
    assert!(derive_data(
        &prefix,
        &identity,
        &source,
        &activation_snapshot(&source).unwrap(),
        &fresh,
        &input,
        &[],
        destination.clone(),
        "C:\\managed-parent",
        DIR,
        NONCE,
        TX,
        &[],
        &manifests,
        0
    )
    .is_err());
    let mut stale_identity = destination.clone();
    stale_identity.worktree_id = identity.worktree_id.clone();
    assert!(derive_data(
        &prefix,
        &identity,
        &source,
        &activation_snapshot(&source).unwrap(),
        source.initial(),
        &input,
        &[],
        stale_identity,
        "C:\\managed-parent",
        DIR,
        NONCE,
        TX,
        &[],
        &manifests,
        0
    )
    .is_err());
    let mut other_family = destination;
    other_family.store_id = StoreId::parse("store:other").unwrap();
    assert!(derive_data(
        &prefix,
        &identity,
        &source,
        &activation_snapshot(&source).unwrap(),
        source.initial(),
        &input,
        &[],
        other_family,
        "C:\\managed-parent",
        DIR,
        NONCE,
        TX,
        &[],
        &manifests,
        0
    )
    .is_err());
}

#[test]
fn writable_diff_binds_exact_manifest_and_readonly_change_refuses() {
    let (writable_source, identity) = source(true);
    let destination_identity = destination(&identity);
    let planned_prefix = prefix(&writable_source, &identity, &destination_identity, 0, None);
    let d = &writable_source.descriptors()[0];
    let edited = manifest(b"managed edited bytes");
    let input = [ImageFile {
        path: &d.path,
        manifest: &edited,
        executable: d.executable,
        expected: Some((d.file_id.clone(), d.executable)),
    }];
    let supplied = [d.manifest.clone(), edited.clone()];
    let result = plan(
        &planned_prefix,
        &writable_source,
        &identity,
        &input,
        &[],
        &[],
        &supplied,
        0,
    )
    .unwrap();
    assert_eq!(result.capture.changes.len(), 1);
    assert_eq!(result.plan.working_overlay.count, 1);
    assert_eq!(result.required_manifests.len(), 2);
    assert!(result.quota_bytes >= d.bytes + edited.byte_length());
    let (readonly, identity) = source(false);
    let d = &readonly.descriptors()[0];
    let input = [ImageFile {
        path: &d.path,
        manifest: &edited,
        executable: d.executable,
        expected: Some((d.file_id.clone(), d.executable)),
    }];
    let prefix = prefix(&readonly, &identity, &destination(&identity), 0, None);
    assert!(plan(
        &prefix,
        &readonly,
        &identity,
        &input,
        &[],
        &[],
        &supplied,
        0
    )
    .is_err());
}

#[test]
fn exact_manifest_coverage_and_size_are_required() {
    let (source, identity) = source(false);
    let prefix = prefix(&source, &identity, &destination(&identity), 0, None);
    let d = &source.descriptors()[0];
    let input = [ImageFile {
        path: &d.path,
        manifest: &d.manifest,
        executable: d.executable,
        expected: Some((d.file_id.clone(), d.executable)),
    }];
    assert!(plan(&prefix, &source, &identity, &input, &[], &[], &[], 0).is_err());
    let wrong = manifest(b"unrelated bytes");
    assert!(plan(&prefix, &source, &identity, &input, &[], &[], &[wrong], 0).is_err());
    let contradictory = RequiredManifest {
        file_id: d.file_id.clone(),
        manifest_id: d.manifest_id.clone(),
        bytes: d.bytes + 1,
    };
    assert!(plan(
        &prefix,
        &source,
        &identity,
        &input,
        &[],
        &[contradictory],
        &[d.manifest.clone()],
        0
    )
    .is_err());
    assert!(plan(
        &prefix,
        &source,
        &identity,
        &input,
        &[],
        &[],
        &[d.manifest.clone()],
        u64::MAX
    )
    .is_err());
}

#[test]
fn one_file_id_can_have_two_exact_manifest_representations() {
    let bytes = b"the same logical file";
    let a = manifest(bytes);
    let b = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(2 * 1024 * 1024).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap();
    assert_eq!(a.file_id(), b.file_id());
    assert_ne!(a.id(), b.id());
    let mut rows = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut owners = BTreeMap::new();
    insert_required(
        &mut rows,
        &mut lengths,
        &mut owners,
        RequiredManifest::from_manifest(&a),
    )
    .unwrap();
    insert_required(
        &mut rows,
        &mut lengths,
        &mut owners,
        RequiredManifest::from_manifest(&b),
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    let mut conflicting = RequiredManifest::from_manifest(&b);
    conflicting.bytes += 1;
    assert!(insert_required(&mut rows, &mut lengths, &mut owners, conflicting).is_err());
}
