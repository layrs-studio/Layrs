//! Active generation-one resident ranges, with durable reopen and exact-map fencing.
use super::super::preparation::{
    ProviderActivationOutcome, ProviderActiveRangeFailure, ProviderPreparationOutcome,
};
use super::*;
use layrs_core::{
    projection::{AvailabilityRequest, OfflineOperation},
    scm::{
        Chunk, ChunkingDescriptor, FileChunk, FileManifest, RepoPath, WorktreeId, canonical_json,
    },
};
use layrs_vfs::Cancellation;
use std::{fs, io::Write};

macro_rules! owned {
    ($name:ident,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host active resident-range recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare();
                hooks::reset();
                $body
            });
        }
    };
}

fn activated(
    f: &Fixture,
    source: ProviderBootstrapSource,
) -> super::super::preparation::ProviderActive {
    let published = match f
        .family
        .begin_provider_bootstrap(&f.checkout, &f.stages, &f.worktree, fixture::TX, source)
        .unwrap()
    {
        ProviderControlPublicationOutcome::Published(value) => value,
        _ => panic!("range source did not publish"),
    };
    match published.prepare_provider([0x71; 16]) {
        ProviderPreparationOutcome::Prepared(value) => match value.activate() {
            ProviderActivationOutcome::Active(active) => active,
            ProviderActivationOutcome::Withheld(value) => {
                panic!("range activation withheld: {:?}", value.failure())
            }
        },
        ProviderPreparationOutcome::Withheld(value) => {
            panic!("range root withheld: {:?}", value.failure())
        }
    }
}

fn cold_source_for(bytes: &[u8]) -> ProviderBootstrapSource {
    use layrs_core::scm::TreeEntryTarget;
    let chunks = bytes
        .chunks(1024 * 1024)
        .map(|bytes| Chunk::new(bytes.to_vec()).unwrap())
        .collect::<Vec<_>>();
    let manifest = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
        &chunks,
    )
    .unwrap();
    let old = Fixture::source();
    let mut snapshot = old.initial().clone();
    let entry = snapshot.entries.first_mut().unwrap();
    let Some(TreeEntryTarget::File { file_id }) = &mut entry.target else {
        panic!("G1 source must select one file")
    };
    *file_id = manifest.file_id().clone();
    let path = entry.path.clone();
    ProviderBootstrapSource::validate(
        snapshot,
        old.selection().clone(),
        vec![ProviderSourceDescriptor { path, manifest }],
    )
    .unwrap()
}

owned!(cold_miss_cancel_and_empty_eof_leave_buffer_unchanged, {
    let f = Fixture::new();
    let source = super::preparation::cold_nonempty_source();
    let descriptor = &source.descriptors()[0];
    let id = descriptor.file_id.clone();
    let size = descriptor.bytes;
    let holder = activated(&f, source);
    let cancel = Cancellation::default();
    let mut output = [0x5a; 7];
    assert!(matches!(
        holder.read_resident_range(&id, 0, &mut output, &cancel),
        Err(ProviderActiveRangeFailure::NeedsHydration { .. })
    ));
    assert_eq!(output, [0x5a; 7]);
    holder
        .read_resident_range(&id, size, &mut [], &cancel)
        .unwrap();
    assert!(matches!(
        holder.read_resident_range(&id, size + 1, &mut [], &cancel),
        Err(ProviderActiveRangeFailure::InvalidRange)
    ));
    cancel.cancel();
    assert!(matches!(
        holder.read_resident_range(&id, 0, &mut output, &cancel),
        Err(ProviderActiveRangeFailure::Cancelled)
    ));
    assert_eq!(output, [0x5a; 7]);
    let binding = ProviderBootstrapBinding::decode(&holder.binding().encode().unwrap()).unwrap();
    drop(holder);
    let reopened = PartialStoreFamily::open_provider_active(
        f.family.root(),
        &f.worktree,
        fixture::TX,
        &binding,
    )
    .unwrap();
    let fresh = Cancellation::default();
    assert!(matches!(
        reopened.read_resident_range(&id, 0, &mut output, &fresh),
        Err(ProviderActiveRangeFailure::NeedsHydration { .. })
    ));
    assert_eq!(output, [0x5a; 7]);
    reopened
        .read_resident_range(&id, size, &mut [], &fresh)
        .unwrap();
});

owned!(ordinary_member_content_serves_and_truncation_refuses, {
    let f = Fixture::new();
    let source = super::preparation::cold_nonempty_source();
    let descriptor = &source.descriptors()[0];
    let id = descriptor.file_id.clone();
    let root = f.outer.join("ordinary");
    fs::create_dir(&root).unwrap();
    let ordinary = f
        .family
        .create_worktree(
            &root,
            WorktreeId::parse("worktree:resident-other").unwrap(),
            source.initial().clone(),
            source.selection().clone(),
        )
        .unwrap();
    let request = AvailabilityRequest {
        version: ordinary.current_version().unwrap(),
        other: None,
        paths: vec![RepoPath::parse("README.md").unwrap()],
        operation: OfflineOperation::Read,
    };
    let intent = ordinary.begin_preparation(&request).unwrap();
    let bytes = b"native metadata without Store content";
    ordinary
        .resume_preparation(&intent.id, &mut |_, _, out| out.write_all(bytes))
        .unwrap();
    let holder = activated(&f, source);
    let cancel = Cancellation::default();
    let mut output = [0u8; 12];
    holder
        .read_resident_range(&id, 7, &mut output, &cancel)
        .unwrap();
    assert_eq!(&output, &bytes[7..19]);
    let chunk = Chunk::new(bytes.to_vec()).unwrap();
    let path = f
        .family
        .root()
        .join("objects/chunks")
        .join(chunk.id().as_str().strip_prefix("blake3:").unwrap());
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_len(1).unwrap();
    let mut unchanged = [0x5a; 12];
    assert!(matches!(
        holder.read_resident_range(&id, 7, &mut unchanged, &cancel),
        Err(ProviderActiveRangeFailure::Unavailable(_))
    ));
    assert_eq!(unchanged, [0x5a; 12]);
});

owned!(cross_chunk_resident_range_matches_selected_bytes, {
    let f = Fixture::new();
    let mut bytes = vec![0x19u8; 1024 * 1024];
    bytes.extend_from_slice(b"boundary-twenty-byte");
    let source = cold_source_for(&bytes);
    let id = source.descriptors()[0].file_id.clone();
    let root = f.outer.join("ordinary");
    fs::create_dir(&root).unwrap();
    let ordinary = f
        .family
        .create_worktree(
            &root,
            WorktreeId::parse("worktree:resident-other").unwrap(),
            source.initial().clone(),
            source.selection().clone(),
        )
        .unwrap();
    let request = AvailabilityRequest {
        version: ordinary.current_version().unwrap(),
        other: None,
        paths: vec![RepoPath::parse("README.md").unwrap()],
        operation: OfflineOperation::Read,
    };
    let intent = ordinary.begin_preparation(&request).unwrap();
    ordinary
        .resume_preparation(&intent.id, &mut |_, _, out| out.write_all(&bytes))
        .unwrap();
    let holder = activated(&f, source);
    let mut output = [0u8; 18];
    holder
        .read_resident_range(&id, 1024 * 1024 - 7, &mut output, &Cancellation::default())
        .unwrap();
    assert_eq!(&output, &bytes[1024 * 1024 - 7..1024 * 1024 + 11]);
});

owned!(cancel_after_first_chunk_keeps_lease_and_output_unchanged, {
    use crate::projection::shared::{provider_access::FamilyAccess, provider_content::test_hooks};
    let f = Fixture::new();
    let mut bytes = vec![0x19u8; 1024 * 1024];
    bytes.extend_from_slice(b"boundary-twenty-byte");
    let source = cold_source_for(&bytes);
    let id = source.descriptors()[0].file_id.clone();
    let root = f.outer.join("ordinary");
    fs::create_dir(&root).unwrap();
    let ordinary = f
        .family
        .create_worktree(
            &root,
            WorktreeId::parse("worktree:resident-other").unwrap(),
            source.initial().clone(),
            source.selection().clone(),
        )
        .unwrap();
    let request = AvailabilityRequest {
        version: ordinary.current_version().unwrap(),
        other: None,
        paths: vec![RepoPath::parse("README.md").unwrap()],
        operation: OfflineOperation::Read,
    };
    let intent = ordinary.begin_preparation(&request).unwrap();
    ordinary
        .resume_preparation(&intent.id, &mut |_, _, out| out.write_all(&bytes))
        .unwrap();
    let holder = activated(&f, source);
    let cancellation = Cancellation::default();
    let check = cancellation.clone();
    let family_root = f.family.root().to_path_buf();
    let worktree = f.worktree.clone();
    test_hooks::install(move || {
        let family = FamilyAccess::open(&family_root).unwrap();
        // Same-process contention is rejected by the local lock table before
        // an OS lock attempt. A different-process case needs its own recipe.
        match family.locks(&worktree, true) {
            Err(Cause::Store(crate::StoreError::ReentrantLock(_))) => {}
            _ => panic!("exclusive local owner lock was not excluded during resident read"),
        }
        check.cancel();
    });
    let mut output = [0x5a; 18];
    assert!(matches!(
        holder.read_resident_range(&id, 1024 * 1024 - 7, &mut output, &cancellation),
        Err(ProviderActiveRangeFailure::Cancelled)
    ));
    assert_eq!(output, [0x5a; 18]);
    // The stopped attempt released all operation gates; the same immutable
    // family bytes are still available to a new, uncancelled read.
    holder
        .read_resident_range(&id, 1024 * 1024 - 7, &mut output, &Cancellation::default())
        .unwrap();
    assert_eq!(&output, &bytes[1024 * 1024 - 7..1024 * 1024 + 11]);
});

owned!(
    canonical_forged_local_manifest_cannot_claim_source_file_id,
    {
        let f = Fixture::new();
        let source = super::preparation::cold_nonempty_source();
        let original = &source.descriptors()[0];
        let id = original.file_id.clone();
        let size = original.bytes;
        let original_manifest_id = original.manifest_id.clone();
        let holder = activated(&f, source);
        let forged_bytes = vec![b'X'; size as usize];
        assert_ne!(
            FileManifest::from_chunks(
                ChunkingDescriptor::single(),
                &[Chunk::new(forged_bytes.clone()).unwrap()]
            )
            .unwrap()
            .file_id(),
            &id
        );
        let chunk = Chunk::new(forged_bytes.clone()).unwrap();
        let forged = FileManifest::new(
            id.clone(),
            size,
            ChunkingDescriptor::single(),
            vec![FileChunk::new(chunk.id().clone(), 0, size).unwrap()],
        )
        .unwrap();
        assert_ne!(forged.id(), &original_manifest_id);
        fs::write(
            f.family
                .root()
                .join("objects/chunks")
                .join(chunk.id().as_str().strip_prefix("blake3:").unwrap()),
            &forged_bytes,
        )
        .unwrap();
        let payload = canonical_json(&forged).unwrap();
        fs::write(
            f.family
                .root()
                .join("objects/manifests")
                .join(forged.id().as_str().strip_prefix("blake3:").unwrap()),
            &payload,
        )
        .unwrap();
        let c = rusqlite::Connection::open_with_flags(
            f.family.root().join("metadata/catalog.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        c.execute(
            "INSERT INTO chunks VALUES(?1,?2)",
            rusqlite::params![chunk.id().as_str(), size],
        )
        .unwrap();
        c.execute(
            "INSERT INTO manifests VALUES(?1,?2,?3,?4)",
            rusqlite::params![forged.id().as_str(), id.as_str(), size, payload],
        )
        .unwrap();
        c.execute(
            "INSERT INTO manifest_chunks VALUES(?1,0,?2)",
            rusqlite::params![forged.id().as_str(), chunk.id().as_str()],
        )
        .unwrap();
        c.close().unwrap();
        let mut output = [0x5a; 8];
        assert!(matches!(
            holder.read_resident_range(&id, 0, &mut output, &Cancellation::default()),
            Err(ProviderActiveRangeFailure::Unavailable(_))
        ));
        assert_eq!(output, [0x5a; 8]);
    }
);
