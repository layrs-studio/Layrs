use super::fixture::{Fixture, entry, space};
use super::tests::{differential, reader};
use layrs_core::scm::{ChunkId, ChunkingDescriptor, FileChunk};

#[tokio::test]
async fn differential_file_and_service_object_caps_are_not_relaxed() {
    let mut fixture = Fixture::default();
    let file = fixture.file(b"small");
    let root = fixture.tree(vec![entry(
        "file",
        TreeEntryTarget::file(file.clone()),
        false,
    )]);
    let step = fixture.step(root);
    let chunk_id = fixture
        .objects
        .values()
        .find(|(descriptor, _)| descriptor.kind == ObjectKind::Chunk)
        .unwrap()
        .0
        .object_id
        .clone();
    let manifest = FileManifest::new(
        file.clone(),
        MAX_FILE + 1,
        ChunkingDescriptor::single(),
        vec![FileChunk::new(ChunkId::parse(chunk_id.as_str()).unwrap(), 0, MAX_FILE + 1).unwrap()],
    )
    .unwrap();
    let selected = fixture.manifests[&file].clone();
    fixture.object(
        manifest.id().as_str(),
        ObjectKind::FileManifest,
        serde_json::to_vec(&manifest).unwrap(),
    );
    fixture
        .manifests
        .insert(file.clone(), manifest.id().to_string());
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("comparison file")
    );
    fixture.manifests.insert(file, selected);
    fixture
        .objects
        .get_mut(&chunk_id)
        .unwrap()
        .0
        .encoded_byte_len = MAX_FILE + 1;
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("service object")
    );
}

#[tokio::test]
async fn differential_logical_budgets_charge_reused_files_at_the_same_boundary() {
    let mut fixture = Fixture::default();
    let file = fixture.file(b"budget");
    let cost: u64 = fixture
        .objects
        .values()
        .map(|(d, _)| d.canonical_byte_len)
        .sum();
    let space_id = space();
    for (bytes, objects) in [
        (MAX_BYTES - cost, 0),
        (MAX_BYTES - cost + 1, 0),
        (u64::MAX, 0),
        (0, 99_998),
        (0, 99_999),
    ] {
        let source = fixture.source();
        let eager_source = fixture.source();
        let mut actual = reader(&source, &space_id);
        let mut eager = legacy::LegacyReader {
            source: &eager_source,
            space_id: &space_id,
            bytes: 0,
            objects: 0,
        };
        actual.file(&file).await.unwrap();
        eager.file(&file).await.unwrap();
        actual.bytes = bytes;
        actual.objects = objects;
        eager.bytes = bytes;
        eager.objects = objects;
        let result = actual
            .file(&file)
            .await
            .map(|value| value.len())
            .map_err(|error| error.to_string());
        let expected = eager
            .file(&file)
            .await
            .map(|value| value.len())
            .map_err(|error| error.to_string());
        assert_eq!(result, expected);
        assert_eq!((actual.bytes, actual.objects), (eager.bytes, eager.objects));
        assert_eq!(
            result.is_ok(),
            bytes == MAX_BYTES - cost || objects == 99_998
        );
    }
}
use super::*;

#[tokio::test]
async fn differential_entry_and_depth_limits_include_equal_trees() {
    let mut fixture = Fixture::default();
    let empty = fixture.file(b"");
    for count in [MAX_ENTRIES, MAX_ENTRIES + 1] {
        let root = fixture.tree(
            (0..count)
                .map(|i| {
                    entry(
                        &format!("f{i}"),
                        TreeEntryTarget::file(empty.clone()),
                        false,
                    )
                })
                .collect(),
        );
        let step = fixture.step(root);
        let result = differential(&fixture, &step, &step).await.0;
        if count == MAX_ENTRIES {
            assert_eq!(result.unwrap(), serde_json::json!([]));
        } else {
            assert!(result.unwrap_err().contains("comparison entries"));
        }
    }
    let mut tree = fixture.tree(vec![]);
    for _ in 0..128 {
        tree = fixture.tree(vec![entry("d", TreeEntryTarget::tree(tree), false)]);
    }
    let step = fixture.step(tree.clone());
    differential(&fixture, &step, &step).await.0.unwrap();
    let root = fixture.tree(vec![entry("d", TreeEntryTarget::tree(tree), false)]);
    let step = fixture.step(root);
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("comparison depth")
    );
}

#[tokio::test]
async fn differential_manifest_identity_and_initial_reconstruction_are_verified() {
    let mut fixture = Fixture::default();
    let file = fixture.file(b"wanted");
    let other = fixture.file(b"wrong!");
    let root = fixture.tree(vec![entry(
        "file",
        TreeEntryTarget::file(file.clone()),
        false,
    )]);
    let step = fixture.step(root);
    fixture
        .manifests
        .insert(file.clone(), fixture.manifests[&other].clone());
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("identity mismatch")
    );
    let selected = fixture.manifests[&other].clone();
    let original: FileManifest =
        parse(&fixture.objects[&ObjectId::parse(&selected).unwrap()].1).unwrap();
    // A structurally valid manifest with the wrong reconstructed FileId must not seed reuse.
    let forged = FileManifest::new(
        file.clone(),
        original.byte_length(),
        original.chunking().clone(),
        original.chunks().to_vec(),
    )
    .unwrap();
    fixture.object(
        forged.id().as_str(),
        ObjectKind::FileManifest,
        serde_json::to_vec(&forged).unwrap(),
    );
    fixture.manifests.insert(file, forged.id().to_string());
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("identity mismatch")
    );
}

#[tokio::test]
async fn differential_missing_admission_and_missing_chunk_fail_closed() {
    let mut fixture = Fixture::default();
    let file = fixture.file(b"data");
    let root = fixture.tree(vec![entry("file", TreeEntryTarget::file(file), false)]);
    let step = fixture.step(root);
    fixture.admitted.clear();
    assert!(differential(&fixture, &step, &step).await.0.is_err());
    fixture.admitted.insert(step.clone());
    fixture
        .objects
        .retain(|_, (descriptor, _)| descriptor.kind != ObjectKind::Chunk);
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("not found")
    );
}

#[tokio::test]
async fn live_denial_and_availability_are_checked_on_second_side() {
    let mut fixture = Fixture::default();
    let file = fixture.file(b"data");
    let child = fixture.tree(vec![entry("file", TreeEntryTarget::file(file), false)]);
    let root = fixture.tree(vec![entry("child", TreeEntryTarget::tree(child), false)]);
    let step = fixture.step(root);
    let space_id = space();
    for remove_chunk in [false, true] {
        let source = fixture.source();
        let mut actual = reader(&source, &space_id);
        actual.step(&step).await.unwrap();
        if remove_chunk {
            source
                .data
                .lock()
                .unwrap()
                .objects
                .retain(|_, (descriptor, _)| descriptor.kind != ObjectKind::Chunk);
        } else {
            source
                .data
                .lock()
                .unwrap()
                .denied
                .insert("child/file".into());
        }
        let error = actual
            .step(&step)
            .await
            .err()
            .expect("second-side live check");
        assert_eq!(
            error.code(),
            if remove_chunk {
                "not_found"
            } else {
                "invalid_capability"
            }
        );
    }
}
