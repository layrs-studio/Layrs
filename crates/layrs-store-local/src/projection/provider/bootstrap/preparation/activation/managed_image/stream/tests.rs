use super::*;
use layrs_core::scm::{Chunk, ChunkId, FileChunk};

fn manifest(bytes: &[u8]) -> FileManifest {
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap()
}

fn copy_bytes(
    manifest: &FileManifest,
    bytes: &[u8],
    output: &mut dyn Write,
    cancel: &Cancellation,
) -> Result<()> {
    let mut offset = 0usize;
    copy_verified(manifest, output, cancel, |part| {
        let end = offset + part.len();
        let data = bytes
            .get(offset..end)
            .ok_or_else(|| invalid("injected source bytes missing"))?;
        part.copy_from_slice(data);
        offset = end;
        Ok(())
    })
}

#[test]
fn copied_bytes_require_exact_chunk_and_file_identity() {
    let bytes = vec![b'q'; 130 * 1024];
    let authentic = manifest(&bytes);
    let wrong_chunk = FileChunk::new(
        ChunkId::parse(format!("blake3:{}", "0".repeat(64))).unwrap(),
        0,
        bytes.len() as u64,
    )
    .unwrap();
    let forged = FileManifest::new(
        authentic.file_id().clone(),
        authentic.byte_length(),
        authentic.chunking().clone(),
        vec![wrong_chunk],
    )
    .unwrap();
    let mut output = Vec::new();
    copy_bytes(&authentic, &bytes, &mut output, &Cancellation::default()).unwrap();
    assert_eq!(output, bytes);
    assert!(copy_bytes(&forged, &bytes, &mut Vec::new(), &Cancellation::default()).is_err());
    let mut altered = bytes;
    altered[17] ^= 1;
    assert!(
        copy_bytes(
            &authentic,
            &altered,
            &mut Vec::new(),
            &Cancellation::default()
        )
        .is_err()
    );
}

#[test]
fn missing_bytes_cancellation_and_short_sink_refuse_success() {
    let bytes = vec![b'x'; 70 * 1024];
    let expected = manifest(&bytes);
    assert!(
        copy_bytes(
            &expected,
            &bytes[..64 * 1024],
            &mut Vec::new(),
            &Cancellation::default(),
        )
        .is_err()
    );
    let missing = copy_verified(&expected, &mut Vec::new(), &Cancellation::default(), |_| {
        Err(layrs_vfs::Error::MissingBytes {
            file_id: expected.file_id().clone(),
            offset: 0,
            length: 64 * 1024,
        }
        .into())
    });
    assert!(matches!(
        missing,
        Err(
            crate::projection::provider::recovery::error::ProviderBootstrapCause::Vfs(
                layrs_vfs::Error::MissingBytes { .. }
            )
        )
    ));

    let cancelled = Cancellation::default();
    cancelled.cancel();
    let mut reads = 0;
    assert!(
        copy_verified(&expected, &mut Vec::new(), &cancelled, |_| {
            reads += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(reads, 0);
    let during = Cancellation::default();
    let mut copied = Vec::new();
    assert!(
        copy_verified(&expected, &mut copied, &during, |part| {
            part.fill(b'x');
            during.cancel();
            Ok(())
        })
        .is_err()
    );
    assert!(copied.is_empty());

    struct ShortSink(usize);
    impl Write for ShortSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            let count = self.0.min(bytes.len());
            self.0 -= count;
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(
        copy_bytes(
            &expected,
            &bytes,
            &mut ShortSink(1024),
            &Cancellation::default(),
        )
        .is_err()
    );
}

#[test]
fn selected_chunk_reads_only_its_absolute_range_with_bounded_scratch() {
    let first = Chunk::new(vec![b'a'; 1024 * 1024]).unwrap();
    let second = Chunk::new(vec![b'b'; 500 * 1024 + 17]).unwrap();
    let manifest = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
        &[first.clone(), second.clone()],
    )
    .unwrap();
    let chunk = &manifest.chunks()[1];
    let mut expected_offset = chunk.offset();
    let mut output = Vec::new();
    copy_verified_chunk(
        &manifest,
        1,
        &mut output,
        &Cancellation::default(),
        |offset, part| {
            assert_eq!(offset, expected_offset);
            assert!(part.len() <= 64 * 1024);
            let local = usize::try_from(offset - chunk.offset()).unwrap();
            part.copy_from_slice(&second.bytes()[local..local + part.len()]);
            expected_offset += part.len() as u64;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(output, second.bytes());
    assert_eq!(expected_offset, manifest.byte_length());

    // A chunk proof deliberately does not claim to verify the full FileId.
    // The engine verifies it once after assembling every chunk.
    let different_file_id = FileManifest::new(
        FileId::from_bytes(b"different whole file"),
        manifest.byte_length(),
        manifest.chunking().clone(),
        manifest.chunks().to_vec(),
    )
    .unwrap();
    copy_verified_chunk(
        &different_file_id,
        1,
        &mut std::io::sink(),
        &Cancellation::default(),
        |offset, part| {
            let local = usize::try_from(offset - chunk.offset()).unwrap();
            part.copy_from_slice(&second.bytes()[local..local + part.len()]);
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn selected_chunk_rejects_absent_ordinal_corruption_cancel_and_sink_failure() {
    let bytes = vec![b'x'; 70 * 1024];
    let expected = manifest(&bytes);
    let mut output = Vec::new();
    assert!(
        copy_verified_chunk(
            &expected,
            1,
            &mut output,
            &Cancellation::default(),
            |_, _| panic!("absent ordinal must not read"),
        )
        .is_err()
    );
    assert!(output.is_empty());
    assert!(
        copy_verified_chunk(
            &FileManifest::empty(ChunkingDescriptor::single()).unwrap(),
            0,
            &mut output,
            &Cancellation::default(),
            |_, _| panic!("empty file has no chunk"),
        )
        .is_err()
    );
    assert!(
        copy_verified_chunk(
            &expected,
            0,
            &mut output,
            &Cancellation::default(),
            |_, part| {
                part.fill(b'z');
                Ok(())
            },
        )
        .is_err()
    );

    let cancelled = Cancellation::default();
    cancelled.cancel();
    let mut output = Vec::new();
    assert!(
        copy_verified_chunk(&expected, 0, &mut output, &cancelled, |_, _| {
            panic!("cancelled before read")
        })
        .is_err()
    );
    assert!(output.is_empty());
    let during = Cancellation::default();
    assert!(
        copy_verified_chunk(&expected, 0, &mut output, &during, |_, part| {
            part.fill(b'x');
            during.cancel();
            Ok(())
        })
        .is_err()
    );
    assert!(output.is_empty());

    struct FailingSink;
    impl Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WriteZero.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let error = copy_verified_chunk(
        &expected,
        0,
        &mut FailingSink,
        &Cancellation::default(),
        |_, part| {
            part.fill(b'x');
            Ok(())
        },
    );
    assert!(
        matches!(error, Err(crate::projection::provider::recovery::error::ProviderBootstrapCause::Io(e)) if e.kind() == io::ErrorKind::WriteZero)
    );
    let missing = copy_verified_chunk(
        &expected,
        0,
        &mut Vec::new(),
        &Cancellation::default(),
        |offset, part| {
            Err(layrs_vfs::Error::MissingBytes {
                file_id: expected.file_id().clone(),
                offset,
                length: part.len(),
            }
            .into())
        },
    );
    assert!(matches!(
        missing,
        Err(
            crate::projection::provider::recovery::error::ProviderBootstrapCause::Vfs(
                layrs_vfs::Error::MissingBytes {
                    offset: 0,
                    length: 65536,
                    ..
                }
            )
        )
    ));
}
