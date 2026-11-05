//! Detached admitted-source planning DATA; no SQL, bytes or native authority.
use super::*;
use crate::projection::provider::{
    import_validation::content_mapping_rowset_data,
    records::import::{AdmittedSourceBinding, ImportClosure, ImportRowset, ImportVersion},
    validation::tests::fixture::Fixture,
};
use layrs_core::scm::{Chunk, ChunkingDescriptor, WorktreeId};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const SELECTION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIR: &str = "fs:v1:1111111111111111:2222222222222222";
const NONCE: &str = "00000000000000000000000000000001";
const TX: &str = "00000000000000000000000000000002";

struct Data {
    prefix: ProviderImportPrefixRecord,
    current: ProjectionSnapshot,
    selection: ProjectionSelection,
    base: FileManifest,
    historical: Vec<RequiredManifest>,
}

fn row(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: TOKEN.into(),
    }
}

fn manifest(bytes: &[u8]) -> FileManifest {
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap()
}

fn history_commitment(history: &[RequiredManifest]) -> ImportRowset {
    let rows = history
        .iter()
        .map(|row| (row.file_id.clone(), (row.manifest_id.clone(), row.bytes)))
        .collect();
    content_mapping_rowset_data(&rows, None).unwrap()
}

fn fixture() -> Data {
    let pure = Fixture::new("G1", false);
    let mut current = pure.source.initial().clone();
    current.entries[0].state = EntryState::Materialized;
    current.entries[0].writable = true;
    current.validate().unwrap();
    let base = pure.source.descriptors()[0].manifest.clone();
    let historical = vec![RequiredManifest::from_manifest(&base)];
    let mut destination = pure.allocation.format.identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:ordinary-plan-destination").unwrap();
    let prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v2".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: pure.allocation.format.identity,
            format_digest: TOKEN.into(),
            private_schema: 9,
            checkout_identity: Some(DIR.into()),
            admission: Some(AdmittedSourceBinding {
                admission_digest: TOKEN.into(),
                incoming_prefix_id: TOKEN.into(),
                incoming_receipt_id: TOKEN.into(),
                database_identity: "1:0:11".into(),
            }),
            version: ImportVersion {
                generation: 3,
                snapshot_digest: SELECTION.into(),
                selection_digest: SELECTION.into(),
                capture_count: 2,
                capture_head: Some(TOKEN.into()),
                effective_image_digest: SELECTION.into(),
            },
        },
        destination_identity: destination,
        inherited_segments: row(1),
        ancestry_root_receipt: Some(TOKEN.into()),
        closure: ImportClosure {
            denial_observations: None,
            snapshots: row(4),
            selections: row(1),
            captures: row(2),
            publications: row(0),
            catalogs: row(0),
            descriptors: row(0),
            activation_maps: row(0),
            transition_receipts: row(1),
            companions: row(0),
            companion_files: row(0),
            companion_deletions: row(0),
            content_mappings: history_commitment(&historical),
            restore_history: Some(row(0)),
        },
    };
    prefix.validate_fields().unwrap();
    Data {
        prefix,
        current,
        selection: ProjectionSelection::All,
        base,
        historical,
    }
}

fn image<'a>(data: &'a Data, manifest: &'a FileManifest) -> ImageFile<'a> {
    ImageFile {
        path: &data.current.entries[0].path,
        manifest,
        executable: false,
        expected: Some((data.base.file_id().clone(), false)),
    }
}

fn plan(
    data: &Data,
    bases: &[&FileManifest],
    files: &[ImageFile<'_>],
    deletions: &[ImageDeletion<'_>],
    supplied: &[FileManifest],
) -> Result<ManagedPlanData> {
    derive_ordinary_data(
        &data.prefix,
        &data.current,
        &data.selection,
        SELECTION,
        bases,
        files,
        deletions,
        data.prefix.destination_identity.clone(),
        "C:\\managed-parent",
        DIR,
        NONCE,
        TX,
        &data.historical,
        supplied,
        100,
    )
}

#[test]
fn ordinary_plan_protects_base_and_current_with_empty_historical_mapping() {
    let mut data = fixture();
    data.historical.clear();
    data.prefix.closure.content_mappings = history_commitment(&[]);
    let current = manifest(b"current image without a historical base mapping");
    let supplied = [data.base.clone(), current.clone()];
    let result = plan(
        &data,
        &[&data.base],
        &[image(&data, &current)],
        &[],
        &supplied,
    )
    .unwrap();
    assert_eq!(
        result.plan.source_prefix.closure.content_mappings,
        history_commitment(&[])
    );
    assert_eq!(result.plan.protected_manifests.count, 2);
    assert_eq!(
        result.capture.changes[0].expected.as_ref().unwrap().file_id,
        *data.base.file_id()
    );
    assert_eq!(
        result.capture.changes[0]
            .replacement
            .as_ref()
            .unwrap()
            .file_id,
        *current.file_id()
    );
    require_manifest_union(&result.plan, &supplied).unwrap();
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &current)],
            &[],
            &[current.clone()]
        )
        .is_err()
    );

    let unchanged = plan(
        &data,
        &[&data.base],
        &[image(&data, &data.base)],
        &[],
        &[data.base.clone()],
    )
    .unwrap();
    assert!(unchanged.capture.changes.is_empty());
    assert_eq!(unchanged.plan.protected_manifests.count, 1);
    assert_eq!(
        unchanged.plan.source_prefix.closure.content_mappings,
        history_commitment(&[])
    );
}

#[test]
fn ordinary_plan_preserves_current_snapshot_expected_base_and_historical_preimage() {
    let mut data = fixture();
    let preimage = manifest(b"restore-only historical preimage");
    data.historical
        .push(RequiredManifest::from_manifest(&preimage));
    data.prefix.closure.content_mappings = history_commitment(&data.historical);
    let edited = manifest(b"ordinary current working bytes");
    let supplied = [data.base.clone(), edited.clone(), preimage];
    let result = plan(
        &data,
        &[&data.base],
        &[image(&data, &edited)],
        &[],
        &supplied,
    )
    .unwrap();
    assert_eq!(result.observation, data.current);
    assert_eq!(result.placement, data.current);
    assert_eq!(result.plan.observation.generation, 4);
    assert_eq!(result.plan.placement_generation, 5);
    assert_eq!(result.capture.sequence, 3);
    assert_eq!(result.capture.previous_patch_digest.as_deref(), Some(TOKEN));
    assert_eq!(
        result.capture.changes[0].expected.as_ref().unwrap().file_id,
        *data.base.file_id()
    );
    assert_eq!(
        result.capture.changes[0]
            .replacement
            .as_ref()
            .unwrap()
            .file_id,
        *edited.file_id()
    );
    assert_eq!(result.plan.protected_manifests.count, 3);
    assert!(result.required_manifests.contains(&data.historical[1]));
    require_manifest_union(&result.plan, &supplied).unwrap();
}

#[test]
fn ordinary_base_manifest_covers_shared_file_paths_once() {
    let mut data = fixture();
    let mut shared = data.current.entries[0].clone();
    shared.path = RepoPath::parse("z-shared.txt").unwrap();
    data.current.entries.push(shared);
    let files: Vec<_> = data
        .current
        .entries
        .iter()
        .map(|entry| ImageFile {
            path: &entry.path,
            manifest: &data.base,
            executable: entry.executable,
            expected: Some((data.base.file_id().clone(), entry.executable)),
        })
        .collect();
    let result = plan(&data, &[&data.base], &files, &[], &[data.base.clone()]).unwrap();
    assert!(result.capture.changes.is_empty());
    assert_eq!(result.required_manifests.len(), 1);
    assert!(
        plan(
            &data,
            &[&data.base, &data.base],
            &files,
            &[],
            &[data.base.clone()]
        )
        .is_err()
    );
}

#[test]
fn ordinary_plan_refuses_missing_extra_wrong_and_conflicting_length_bases() {
    let data = fixture();
    let files = [image(&data, &data.base)];
    assert!(plan(&data, &[], &files, &[], &[data.base.clone()]).is_err());
    let other = manifest(b"wrong snapshot base");
    assert!(
        plan(
            &data,
            &[&other],
            &files,
            &[],
            &[data.base.clone(), other.clone()]
        )
        .is_err()
    );
    assert!(
        plan(
            &data,
            &[&data.base, &other],
            &files,
            &[],
            &[data.base.clone(), other.clone()]
        )
        .is_err()
    );
    let longer = manifest(&vec![b'x'; data.base.byte_length() as usize + 1]);
    let wrong_length = FileManifest::new(
        data.base.file_id().clone(),
        longer.byte_length(),
        longer.chunking().clone(),
        longer.chunks().to_vec(),
    )
    .unwrap();
    assert_ne!(wrong_length.byte_length(), data.base.byte_length());
    assert!(
        plan(
            &data,
            &[&wrong_length],
            &files,
            &[],
            &[data.base.clone(), wrong_length.clone()]
        )
        .is_err()
    );
}

#[test]
fn ordinary_plan_requires_exact_admitted_source_selection_and_history_commitment() {
    let mut data = fixture();
    let original = data.prefix.encode().unwrap();
    if let ImportSource::Ordinary {
        private_schema,
        admission,
        ..
    } = &mut data.prefix.source
    {
        *private_schema = 8;
        *admission = None;
    }
    data.prefix.validate_fields().unwrap();
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &data.base)],
            &[],
            &[data.base.clone()]
        )
        .is_err()
    );
    data.prefix = ProviderImportPrefixRecord::decode(&original).unwrap();
    if let ImportSource::Ordinary { version, .. } = &mut data.prefix.source {
        version.selection_digest = "b".repeat(64);
    }
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &data.base)],
            &[],
            &[data.base.clone()]
        )
        .is_err()
    );
    data.prefix = ProviderImportPrefixRecord::decode(&original).unwrap();
    data.selection =
        ProjectionSelection::selected(vec![], vec![], data.current.path_semantics).unwrap();
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &data.base)],
            &[],
            &[data.base.clone()]
        )
        .is_err()
    );
    data.selection = ProjectionSelection::All;
    data.historical.clear();
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &data.base)],
            &[],
            &[data.base.clone()]
        )
        .is_err()
    );
    let substitute = manifest(b"substituted history");
    data.historical
        .push(RequiredManifest::from_manifest(&substitute));
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &data.base)],
            &[],
            &[data.base.clone(), substitute]
        )
        .is_err()
    );
}

#[test]
fn ordinary_plan_keeps_writable_exact_image_and_protected_union_fences() {
    let mut data = fixture();
    let edited = manifest(b"edited");
    let supplied = [data.base.clone(), edited.clone()];
    let mut wrong = image(&data, &edited);
    wrong.expected = Some((edited.file_id().clone(), false));
    assert!(plan(&data, &[&data.base], &[wrong], &[], &supplied).is_err());
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &edited)],
            &[],
            &[edited.clone()]
        )
        .is_err()
    );
    data.current.entries[0].writable = false;
    assert!(
        plan(
            &data,
            &[&data.base],
            &[image(&data, &edited)],
            &[],
            &supplied
        )
        .is_err()
    );
    data.current.entries[0].writable = true;
    let deletion = ImageDeletion {
        path: &data.current.entries[0].path,
        base: data.base.file_id(),
        executable: false,
    };
    let result = plan(&data, &[&data.base], &[], &[deletion], &[data.base.clone()]).unwrap();
    assert_eq!(
        result.capture.changes[0].expected.as_ref().unwrap().file_id,
        *data.base.file_id()
    );
    assert!(result.capture.changes[0].replacement.is_none());
}

#[test]
fn provider_data_wrapper_keeps_fixed_observation_activation_and_selection_checks() {
    let pure = Fixture::new("G1", false);
    let mut data = fixture();
    if let ImportSource::Ordinary { version, .. } = &mut data.prefix.source {
        version.selection_digest = pure.source.commitment().selection_digest.clone();
    }
    let activated = activation_snapshot(&pure.source).unwrap();
    let descriptor = &pure.source.descriptors()[0];
    let files = [ImageFile {
        path: &descriptor.path,
        manifest: &descriptor.manifest,
        executable: descriptor.executable,
        expected: Some((descriptor.file_id.clone(), descriptor.executable)),
    }];
    let supplied = [descriptor.manifest.clone()];
    let call = |prefix: &ProviderImportPrefixRecord,
                fresh: &ProjectionSnapshot,
                placement: &ProjectionSnapshot| {
        derive_data(
            prefix,
            &pure.allocation.format.identity,
            &pure.source,
            placement,
            fresh,
            &files,
            &[],
            data.prefix.destination_identity.clone(),
            "C:\\managed-parent",
            DIR,
            NONCE,
            TX,
            &data.historical,
            &supplied,
            100,
        )
    };
    call(&data.prefix, pure.source.initial(), &activated).unwrap();
    let mut wrong_fresh = pure.source.initial().clone();
    wrong_fresh.entries[0].writable = !wrong_fresh.entries[0].writable;
    assert!(call(&data.prefix, &wrong_fresh, &activated).is_err());
    let mut wrong_activated = activated.clone();
    wrong_activated.entries[0].writable = !wrong_activated.entries[0].writable;
    assert!(call(&data.prefix, pure.source.initial(), &wrong_activated).is_err());
    let mut wrong_selection =
        ProviderImportPrefixRecord::decode(&data.prefix.encode().unwrap()).unwrap();
    if let ImportSource::Ordinary { version, .. } = &mut wrong_selection.source {
        version.selection_digest = "c".repeat(64);
    }
    assert!(call(&wrong_selection, pure.source.initial(), &activated).is_err());
}
