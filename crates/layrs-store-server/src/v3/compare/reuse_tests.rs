use super::fixture::{Fixture, entry, space};
use super::tests::reader;
use super::*;
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use layrs_sync::v3::Compression;

fn nested_fixture() -> (Fixture, StepId, FileId) {
    let mut fixture = Fixture::default();
    let file = fixture.file(b"verified bytes");
    let child = fixture.tree(vec![entry(
        "file",
        TreeEntryTarget::file(file.clone()),
        false,
    )]);
    let root = fixture.tree(vec![entry("child", TreeEntryTarget::tree(child), false)]);
    let step = fixture.step(root);
    (fixture, step, file)
}

#[tokio::test]
async fn equal_second_snapshot_has_zero_duplicate_tree_manifest_or_chunk_reads() {
    let (fixture, step, _) = nested_fixture();
    let source = fixture.source();
    let space_id = space();
    let mut actual = reader(&source, &space_id);
    let before = actual.step(&step).await.unwrap();
    let cold = source.counts.lock().unwrap().clone();
    let cold_budget = (actual.bytes, actual.objects);
    let after = actual.step(&step).await.unwrap();
    let warm = source.counts.lock().unwrap().clone();
    for kind in [
        ObjectKind::Tree,
        ObjectKind::FileManifest,
        ObjectKind::Chunk,
    ] {
        assert_eq!(warm.reads(kind), cold.reads(kind));
    }
    assert_eq!(warm.reads(ObjectKind::Step), 2);
    assert_eq!(warm.authorizations.len(), 2 * cold.authorizations.len());
    assert_eq!(warm.descriptors.len(), 2 * cold.descriptors.len());
    assert_eq!(
        (actual.bytes, actual.objects),
        (2 * cold_budget.0, 2 * cold_budget.1)
    );
    assert!(diff_entries(&entries(&before), &entries(&after)).is_empty());
    println!(
        "equal snapshot cold={:?}, second-side extra Step=1 Tree=0 FileManifest=0 Chunk=0",
        cold.reads
    );
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Missing,
    Kind,
    Oversized,
    Encoding,
}

fn mutate(fixture: &mut Fixture, kind: ObjectKind, mutation: Mutation) {
    let id = fixture
        .objects
        .iter()
        .find(|(_, (descriptor, _))| descriptor.kind == kind)
        .unwrap()
        .0
        .clone();
    if matches!(mutation, Mutation::Missing) {
        fixture.objects.remove(&id);
        return;
    }
    let descriptor = &mut fixture.objects.get_mut(&id).unwrap().0;
    match mutation {
        Mutation::Kind => descriptor.kind = ObjectKind::Product,
        Mutation::Oversized => descriptor.encoded_byte_len = MAX_FILE + 1,
        // Source::read returns canonical bytes. This models a newly encoded CAS
        // representation without trusting its changed descriptor for reuse.
        Mutation::Encoding => descriptor.compression = Compression::Zstd,
        Mutation::Missing => unreachable!(),
    }
}

#[tokio::test]
async fn changed_remote_descriptors_are_rechecked_not_borrowed() {
    let (fixture, step, _) = nested_fixture();
    let space_id = space();
    for kind in [
        ObjectKind::Tree,
        ObjectKind::FileManifest,
        ObjectKind::Chunk,
    ] {
        for mutation in [
            Mutation::Missing,
            Mutation::Kind,
            Mutation::Oversized,
            Mutation::Encoding,
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
            actual.step(&step).await.unwrap();
            eager.step(&step).await.unwrap();
            let cold_reads = source.counts.lock().unwrap().reads(kind);
            mutate(&mut source.data.lock().unwrap(), kind, mutation);
            mutate(&mut eager_source.data.lock().unwrap(), kind, mutation);
            let result = actual
                .step(&step)
                .await
                .map(|value| value.len())
                .map_err(|error| error.to_string());
            let expected = eager
                .step(&step)
                .await
                .map(|value| value.len())
                .map_err(|error| error.to_string());
            assert_eq!(result, expected, "{kind:?} {mutation:?}");
            assert_eq!((actual.bytes, actual.objects), (eager.bytes, eager.objects));
            if matches!(mutation, Mutation::Encoding) {
                assert!(result.is_ok());
                assert_eq!(source.counts.lock().unwrap().reads(kind), cold_reads + 1);
            } else {
                assert!(result.is_err());
            }
        }
    }
}

#[tokio::test]
async fn a_newly_selected_manifest_gets_full_reconstruction() {
    let (fixture, _, file) = nested_fixture();
    let space_id = space();
    let source = fixture.source();
    let mut actual = reader(&source, &space_id);
    let before = actual.file(&file).await.unwrap();
    let chunks = [
        Chunk::new(b"verified ").unwrap(),
        Chunk::new(b"bytes").unwrap(),
    ];
    let manifest =
        FileManifest::from_chunks(ChunkingDescriptor::deterministic_fixed(9).unwrap(), &chunks)
            .unwrap();
    {
        let mut data = source.data.lock().unwrap();
        for chunk in &chunks {
            data.object(
                chunk.id().as_str(),
                ObjectKind::Chunk,
                chunk.bytes().to_vec(),
            );
        }
        data.object(
            manifest.id().as_str(),
            ObjectKind::FileManifest,
            serde_json::to_vec(&manifest).unwrap(),
        );
        data.manifests
            .insert(file.clone(), manifest.id().to_string());
    }
    let after = actual.file(&file).await.unwrap();
    assert_eq!(before, after);
    let counts = source.counts.lock().unwrap();
    assert_eq!(counts.reads(ObjectKind::FileManifest), 2);
    assert_eq!(counts.reads(ObjectKind::Chunk), 3);
}

#[tokio::test]
async fn changed_chunk_representation_must_match_the_verified_file_slice() {
    let (fixture, _, file) = nested_fixture();
    let source = fixture.source();
    let space_id = space();
    let mut actual = reader(&source, &space_id);
    actual.file(&file).await.unwrap();
    {
        let mut data = source.data.lock().unwrap();
        let (descriptor, bytes) = data
            .objects
            .values_mut()
            .find(|(descriptor, _)| descriptor.kind == ObjectKind::Chunk)
            .unwrap();
        descriptor.compression = Compression::Zstd;
        bytes[0] ^= 1;
    }
    assert_eq!(
        actual.file(&file).await.unwrap_err().code(),
        "identity_mismatch"
    );
}
