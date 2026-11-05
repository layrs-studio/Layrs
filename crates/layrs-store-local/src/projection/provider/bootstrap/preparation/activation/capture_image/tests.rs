use super::*;
use crate::projection::provider::validation::tests::fixture::Fixture;
use layrs_core::{
    projection::ProjectionEntry,
    scm::{Chunk, FileManifest, RepoPath},
};

fn make_source(writable: bool, absent: bool) -> ProviderBootstrapSource {
    let f = Fixture::new("G1", false);
    let mut initial = f.source.initial().clone();
    initial.entries[0].writable = writable;
    if absent {
        initial.entries.push(ProjectionEntry {
            path: RepoPath::parse("Z-new.txt").unwrap(),
            target: None,
            executable: false,
            writable: true,
            state: EntryState::Absent,
        });
    }
    ProviderBootstrapSource::validate(
        initial,
        f.source.selection().clone(),
        vec![
            crate::projection::provider::records::ProviderSourceDescriptor {
                path: RepoPath::parse("README.md").unwrap(),
                manifest: f.source.descriptors()[0].manifest.clone(),
            },
        ],
    )
    .unwrap()
}

fn original(source: &ProviderBootstrapSource) -> ImageObservation {
    let descriptor = &source.descriptors()[0];
    ImageObservation {
        path: descriptor.path.clone(),
        evidence: CaptureEvidence::Immutable {
            file_id: descriptor.file_id.clone(),
            size: descriptor.bytes,
            original_path: descriptor.path.clone(),
        },
    }
}

fn hashed(source: &ProviderBootstrapSource) -> ImageObservation {
    let descriptor = &source.descriptors()[0];
    ImageObservation {
        path: descriptor.path.clone(),
        evidence: CaptureEvidence::NeedsHash {
            base: Some(descriptor.file_id.clone()),
            size: descriptor.bytes,
        },
    }
}

fn replacement() -> FileManifest {
    let chunk = Chunk::new(b"replacement bytes".to_vec()).unwrap();
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(CHUNK_BYTES as u64).unwrap(),
        &[chunk],
    )
    .unwrap()
}

#[test]
fn ordinary_replacement_at_known_path_is_hashed_against_the_bound_base() {
    let source = make_source(true, false);
    let activated = activation_snapshot(&source).unwrap();
    let manifest = replacement();
    let observation = ImageObservation {
        path: RepoPath::parse("README.md").unwrap(),
        evidence: CaptureEvidence::NeedsHash {
            base: None,
            size: manifest.byte_length(),
        },
    };
    let result = reconcile(
        &[observation],
        &source,
        &activated,
        &Cancellation::default(),
        |_, _, _| Ok(manifest.clone()),
    )
    .unwrap();
    assert_eq!(result.changes.len(), 1);
    assert_eq!(
        result.changes[0].expected.as_ref().unwrap().file_id,
        source.descriptors()[0].file_id
    );
    assert_eq!(
        result.changes[0].replacement.as_ref().unwrap().file_id,
        *manifest.file_id()
    );
}

#[test]
fn explicit_deletion_is_captured_without_reading_cold_content() {
    let source = make_source(true, false);
    let activated = activation_snapshot(&source).unwrap();
    let observation = ImageObservation {
        path: RepoPath::parse("README.md").unwrap(),
        evidence: CaptureEvidence::Deleted {
            base: source.descriptors()[0].file_id.clone(),
        },
    };
    let result = reconcile(
        &[observation],
        &source,
        &activated,
        &Cancellation::default(),
        |_, _, _| panic!("deletion must not hydrate"),
    )
    .unwrap();
    assert_eq!(result.changes.len(), 1);
    assert!(result.changes[0].replacement.is_none());
    assert_eq!(
        result.changes[0].expected.as_ref().unwrap().file_id,
        source.descriptors()[0].file_id
    );
    assert!(result.hashed.is_empty());
}

#[test]
fn stream_hash_crosses_chunks_and_cancels_after_one_read_block() {
    let bytes = vec![0x51; CHUNK_BYTES + 23];
    let manifest = hash_reader(
        &mut std::io::Cursor::new(&bytes),
        bytes.len() as u64,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(manifest.file_id(), &FileId::from_bytes(&bytes));
    assert_eq!(manifest.chunks().len(), 2);
    assert_eq!(manifest.chunks()[1].byte_length(), 23);
    struct CancellingReader<'a> {
        inner: std::io::Cursor<&'a [u8]>,
        cancel: Cancellation,
        read: usize,
    }
    impl Read for CancellingReader<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(output)?;
            self.read += n;
            if self.read >= READ_BYTES {
                self.cancel.cancel();
            }
            Ok(n)
        }
    }
    let cancel = Cancellation::default();
    let mut reader = CancellingReader {
        inner: std::io::Cursor::new(bytes.as_slice()),
        cancel: cancel.clone(),
        read: 0,
    };
    assert!(matches!(
        hash_reader(&mut reader, bytes.len() as u64, &cancel),
        Err(layrs_vfs::Error::Cancelled)
    ));
    assert_eq!(reader.read, READ_BYTES);
}

#[test]
fn clean_immutable_and_hashed_same_file_have_same_image_without_diff() {
    let source = make_source(false, false);
    let activated = activation_snapshot(&source).unwrap();
    let cancel = Cancellation::default();
    let immutable = reconcile(
        &[original(&source)],
        &source,
        &activated,
        &cancel,
        |_, _, _| panic!("immutable file must not be read"),
    )
    .unwrap();
    assert!(immutable.changes.is_empty());
    assert!(immutable.hashed.is_empty());
    assert_eq!(immutable.image_path_count, 1);
    // The source fixture uses another manifest representation. A real hash
    // pass builds our local fixed-chunk representation of the same empty file.
    assert_eq!(source.descriptors()[0].bytes, 0);
    let local = hash_reader(&mut std::io::Cursor::new(Vec::<u8>::new()), 0, &cancel).unwrap();
    assert_eq!(local.file_id(), &source.descriptors()[0].file_id);
    let hashed = reconcile(
        &[hashed(&source)],
        &source,
        &activated,
        &cancel,
        |_, _, _| Ok(local.clone()),
    )
    .unwrap();
    assert!(hashed.changes.is_empty());
    assert_eq!(immutable.image_digest, hashed.image_digest);
}

#[test]
fn changed_file_and_selected_absence_are_exact_base_relative_changes() {
    let source = make_source(true, true);
    let activated = activation_snapshot(&source).unwrap();
    let cancel = Cancellation::default();
    let old = hashed(&source);
    let new = ImageObservation {
        path: RepoPath::parse("Z-new.txt").unwrap(),
        evidence: CaptureEvidence::NeedsHash {
            base: None,
            size: 17,
        },
    };
    let manifest = replacement();
    let result = reconcile(&[old, new], &source, &activated, &cancel, |_, _, _| {
        Ok(manifest.clone())
    });
    // A synthetic hash callback cannot lie about the declared VFS size.
    assert!(result.is_err());
    let size = manifest.byte_length();
    let observations = [
        ImageObservation {
            path: RepoPath::parse("README.md").unwrap(),
            evidence: CaptureEvidence::NeedsHash {
                base: Some(source.descriptors()[0].file_id.clone()),
                size,
            },
        },
        ImageObservation {
            path: RepoPath::parse("Z-new.txt").unwrap(),
            evidence: CaptureEvidence::NeedsHash { base: None, size },
        },
    ];
    let result = reconcile(&observations, &source, &activated, &cancel, |_, _, _| {
        Ok(manifest.clone())
    })
    .unwrap();
    assert_eq!(result.image_path_count, 2);
    assert_eq!(result.changes.len(), 2);
    assert_eq!(result.hashed.len(), 2);
    assert_eq!(
        result.changes[0].expected.as_ref().unwrap().file_id,
        source.descriptors()[0].file_id
    );
    assert!(result.changes[1].expected.is_none());
    assert_eq!(
        result.changes[1].replacement.as_ref().unwrap().file_id,
        *manifest.file_id()
    );
}

#[test]
fn unknown_alias_missing_catalog_and_readonly_change_refuse_at_correct_boundary() {
    let source = make_source(true, false);
    let activated = activation_snapshot(&source).unwrap();
    let cancel = Cancellation::default();
    let unknown = ImageObservation {
        path: RepoPath::parse("unknown.txt").unwrap(),
        evidence: CaptureEvidence::NeedsHash {
            base: None,
            size: 1,
        },
    };
    let mut called = false;
    assert!(
        reconcile(
            &[original(&source), unknown],
            &source,
            &activated,
            &cancel,
            |_, _, _| {
                called = true;
                panic!("unknown path must refuse before hashing")
            }
        )
        .is_err()
    );
    assert!(!called);
    assert!(
        reconcile(&[], &source, &activated, &cancel, |_, _, _| {
            panic!("omitted catalog must refuse before hashing")
        })
        .is_err()
    );
    let alias = ImageObservation {
        path: RepoPath::parse("readme.md").unwrap(),
        evidence: CaptureEvidence::NeedsHash {
            base: None,
            size: 1,
        },
    };
    assert!(
        reconcile(
            &[original(&source), alias],
            &source,
            &activated,
            &cancel,
            |_, _, _| { panic!("alias must refuse before hashing") }
        )
        .is_err()
    );
    let readonly = make_source(false, false);
    let activated = activation_snapshot(&readonly).unwrap();
    let manifest = replacement();
    let observation = ImageObservation {
        path: RepoPath::parse("README.md").unwrap(),
        evidence: CaptureEvidence::NeedsHash {
            base: Some(readonly.descriptors()[0].file_id.clone()),
            size: manifest.byte_length(),
        },
    };
    assert!(
        reconcile(&[observation], &readonly, &activated, &cancel, |_, _, _| {
            Ok(manifest.clone())
        })
        .is_err()
    );
}

#[test]
fn stream_hash_enforces_declared_length_and_cancellation() {
    let cancel = Cancellation::default();
    let bytes = b"small image".to_vec();
    let manifest = hash_reader(
        &mut std::io::Cursor::new(&bytes),
        bytes.len() as u64,
        &cancel,
    )
    .unwrap();
    assert_eq!(manifest.file_id(), &FileId::from_bytes(&bytes));
    assert!(
        hash_reader(
            &mut std::io::Cursor::new(&bytes),
            bytes.len() as u64 + 1,
            &cancel
        )
        .is_err()
    );
    cancel.cancel();
    assert!(matches!(
        hash_reader(
            &mut std::io::Cursor::new(&bytes),
            bytes.len() as u64,
            &cancel
        ),
        Err(layrs_vfs::Error::Cancelled)
    ));
}
