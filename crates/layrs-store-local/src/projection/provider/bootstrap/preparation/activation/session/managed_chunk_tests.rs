//! Owned native qualification of one retained current-image chunk range.
//! Root reviews/releases this source and runs a pinned binary on a fresh C:
//! parent, with the 240-second owned child inside its 300-second outer bound.
use super::*;
use crate::projection::ProjectionStoreError;
use crate::projection::provider::{
    bootstrap::tests::capture_family::active_holder_with_writable_file_private,
    process_profile::tests::child,
};
use layrs_core::scm::{Chunk, ChunkId, FileChunk, FileManifest, RepoPath};
use std::fs;

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned native managed-chunk recipe"]
fn retained_native_chunk_reads_exact_tail_and_refuses_changed_image() {
    child::run(
        concat!(
            module_path!(),
            "::retained_native_chunk_reads_exact_tail_and_refuses_changed_image"
        ),
        240,
        || {
            child::declare();
            let (fixture, holder, _, _, _) = active_holder_with_writable_file_private();
            let mut session = holder
                .into_disconnected_session()
                .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
            session.connect().unwrap();
            let native_path = fixture.checkout.join("README.md");
            assert!(native_path.exists(), "exact writable placeholder installed");
            if fs::remove_file(&native_path).is_err() {
                let mut permissions = fs::metadata(&native_path).unwrap().permissions();
                permissions.set_readonly(false);
                fs::set_permissions(&native_path, permissions).unwrap();
                fs::remove_file(&native_path).unwrap();
            }
            const FIRST: usize = 1024 * 1024;
            const TAIL: usize = 512 * 1024 + 17;
            let replacement: Vec<u8> = (0..FIRST + TAIL)
                .map(|index| {
                    if index < FIRST {
                        (index % 113) as u8
                    } else {
                        128 + ((index - FIRST) % 127) as u8
                    }
                })
                .collect();
            fs::write(&native_path, &replacement).unwrap();

            // All owner/catalog validation finishes before native hashing or
            // per-piece reads. No content callback runs under catalog gates.
            let active = {
                let held = &session.source.held;
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                capture_state::read(
                    held,
                    Some(&session.source.root),
                    Some(&Cancellation::default()),
                )
                .unwrap_or_else(|error| panic!("active source read: {}", error.cause))
            };
            let path = RepoPath::parse("README.md").unwrap();
            let mut checkpoint = session.session.export_checkpoint().unwrap();
            // The VFS consumer sees exactly the requested tail and a synthetic
            // range EOF; it cannot consume any first-chunk or out-of-range byte.
            let ranged = checkpoint
                .with_range_reader(&path, FIRST as u64, TAIL as u64, |reader| {
                    let mut bytes = Vec::new();
                    assert_eq!(reader.read_to_end(&mut bytes)?, TAIL);
                    assert_eq!(reader.read(&mut [0u8; 1])?, 0);
                    Ok(bytes)
                })
                .unwrap();
            assert_eq!(ranged, replacement[FIRST..]);
            // Bounds rejection precedes consumer invocation even for an empty
            // range whose offset lies beyond the retained native file.
            let out_of_bounds = checkpoint.with_range_reader(
                &path,
                (replacement.len() + 1) as u64,
                0,
                |_| -> layrs_vfs::Result<()> { panic!("invalid range reached consumer") },
            );
            assert!(matches!(
                out_of_bounds,
                Err(layrs_vfs::Error::ExportRead { cause, .. })
                    if matches!(*cause, layrs_vfs::Error::InvalidRange)
            ));
            let cancel = Cancellation::default();
            let mut image = super::super::managed_image::plan(
                &mut checkpoint,
                session.source.as_ref(),
                &active.active.source,
                &active.active.activated.snapshot,
                &cancel,
            )
            .unwrap();
            image
                .require_owner(
                    &session.source.root,
                    &active.active.source,
                    &active.active.activated.snapshot,
                )
                .unwrap();
            assert_eq!(image.files().len(), 1);
            let file = &image.files()[0];
            assert_eq!(file.path, path);
            let manifest = file.manifest.manifest().clone();
            assert_eq!(manifest.file_id(), &FileId::from_bytes(&replacement));
            assert_eq!(manifest.chunks().len(), 2);
            assert_eq!(manifest.chunks()[0].offset(), 0);
            assert_eq!(manifest.chunks()[0].byte_length(), FIRST as u64);
            assert_eq!(manifest.chunks()[1].offset(), FIRST as u64);
            assert_eq!(manifest.chunks()[1].byte_length(), TAIL as u64);
            assert_eq!(
                manifest.chunks()[1].chunk_id(),
                Chunk::new(replacement[FIRST..].to_vec()).unwrap().id()
            );
            let mut copied = Vec::new();
            image
                .copy_exact_chunk(&path, &manifest, 1, &mut copied, &cancel)
                .unwrap();
            assert_eq!(copied.len(), TAIL);
            assert_eq!(copied, replacement[FIRST..]);

            let mut refused = Vec::new();
            let absent = image
                .copy_exact_chunk(&path, &manifest, 2, &mut refused, &cancel)
                .unwrap_err();
            assert_invalid(absent, "managed copy chunk ordinal absent");
            assert!(refused.is_empty(), "invalid ordinal precedes output");
            let mut forged_chunks = manifest.chunks().to_vec();
            forged_chunks[1] = FileChunk::new(
                ChunkId::parse(format!("blake3:{}", "0".repeat(64))).unwrap(),
                FIRST as u64,
                TAIL as u64,
            )
            .unwrap();
            let forged = FileManifest::new(
                manifest.file_id().clone(),
                manifest.byte_length(),
                manifest.chunking().clone(),
                forged_chunks,
            )
            .unwrap();
            assert_eq!(forged.file_id(), manifest.file_id());
            assert_ne!(forged.id(), manifest.id());
            let mismatch = image
                .copy_exact_chunk(&path, &forged, 1, &mut refused, &cancel)
                .unwrap_err();
            assert_invalid(mismatch, "managed image file representation differs");
            assert!(refused.is_empty(), "exact representation precedes output");

            let unknown = fixture.checkout.join("unknown.txt");
            fs::write(&unknown, b"outside the retained image").unwrap();
            assert!(
                image
                    .copy_exact_chunk(&path, &manifest, 1, &mut refused, &cancel)
                    .is_err()
            );
            assert!(refused.is_empty(), "namespace drift precedes output");
            assert_eq!(fs::read(&native_path).unwrap(), replacement);
            assert_eq!(fs::read(&unknown).unwrap(), b"outside the retained image");
            drop(image);
            drop(checkpoint);
            session.disconnect().unwrap();
        },
    );
}

fn assert_invalid(cause: Cause, expected: &str) {
    assert!(matches!(cause,
        Cause::Validation(error)
            if matches!(error.as_ref(), ProjectionStoreError::Invalid(message) if message == expected)
    ));
}
