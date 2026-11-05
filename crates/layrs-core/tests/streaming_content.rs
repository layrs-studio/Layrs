use layrs_core::scm::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Meter;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(bytes: usize) {
    let now = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(now, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(p, layout, new_size) };
        if !next.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            add(new_size);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn descriptor() -> ChunkingDescriptor {
    ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap()
}

#[test]
fn streaming_preserves_empty_binary_repeated_chunk_wire_and_rejects_partial_or_bad_content() {
    for chunks in [
        vec![],
        vec![Chunk::new(vec![0, 255, 42]).unwrap()],
        vec![
            Chunk::new(b"abc".to_vec()).unwrap(),
            Chunk::new(b"different".to_vec()).unwrap(),
            Chunk::new(b"abc".to_vec()).unwrap(),
        ],
    ] {
        let bytes = chunks
            .iter()
            .flat_map(|c| c.bytes().iter().copied())
            .collect::<Vec<_>>();
        let mut offset = 0;
        let entries = chunks
            .iter()
            .map(|c| {
                let e = FileChunk::new(c.id().clone(), offset, c.byte_length()).unwrap();
                offset += c.byte_length();
                e
            })
            .collect();
        let reference = FileManifest::new(
            FileId::from_bytes(&bytes),
            bytes.len() as u64,
            descriptor(),
            entries,
        )
        .unwrap();
        let manifest = FileManifest::from_chunks(descriptor(), &chunks).unwrap();
        assert_eq!(
            canonical_json(&manifest).unwrap(),
            canonical_json(&reference).unwrap()
        );
        let mut verifier = manifest.verifier();
        for chunk in &chunks {
            verifier.push(chunk.bytes()).unwrap();
        }
        verifier.finish().unwrap();
        assert_eq!(
            manifest
                .reconstruct(|id| chunks
                    .iter()
                    .find(|c| c.id() == id)
                    .map(|c| c.bytes().to_vec()))
                .unwrap(),
            bytes
        );
        if !chunks.is_empty() {
            assert!(manifest.verifier().finish().is_err());
            assert!(manifest.verify_chunks(|_| None).is_err());
            let mut bad = manifest.verifier();
            assert!(bad.push(&[]).is_err());
            assert!(bad.push(chunks[0].bytes()).is_err());
            assert!(bad.finish().is_err());
            let mut bad = manifest.verifier();
            let mut altered = chunks[0].bytes().to_vec();
            altered[0] ^= 1;
            assert!(bad.push(&altered).is_err());
            let mut late = manifest.verifier();
            for chunk in &chunks {
                late.push(chunk.bytes()).unwrap();
            }
            assert!(late.push(b"extra").is_err());
            assert!(late.finish().is_err());
            let wrong = FileManifest::new(
                FileId::from_bytes(b"wrong file identity"),
                manifest.byte_length(),
                descriptor(),
                manifest.chunks().to_vec(),
            )
            .unwrap();
            let mut verify = wrong.verifier();
            for c in &chunks {
                verify.push(c.bytes()).unwrap();
            }
            assert!(verify.finish().is_err());
        }
    }
    let chunk = Chunk::new(b"chunk".to_vec()).unwrap();
    for (bytes, count) in [(4, 1), (100, 0)] {
        let mut b = FileManifestBuilder::new(descriptor(), bytes, count).unwrap();
        assert!(b.push_chunk(&chunk).is_err());
        assert!(b.finish().is_err());
    }
}

#[test]
fn hundred_mib_and_one_gib_identity_verification_keep_live_heap_below_sixteen_mib() {
    // This measures live Rust allocations, not OS RSS or an end-to-end checkout.
    // Every pass processes distinct 1MiB chunks, regenerated for verification.
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let mut bytes = (0..1024 * 1024)
        .map(|n| (n % 251) as u8)
        .collect::<Vec<_>>();
    for count in [100, 1024] {
        let mut expected = blake3::Hasher::new();
        expected.update(b"layrs-file-v1\0");
        let mut builder =
            FileManifestBuilder::new(descriptor(), count * 1024 * 1024, count as usize).unwrap();
        for index in 0..count {
            bytes[..8].copy_from_slice(&index.to_le_bytes());
            let chunk = Chunk::new(bytes.clone()).unwrap();
            builder.push_chunk(&chunk).unwrap();
            expected.update(chunk.bytes());
        }
        let manifest = builder.finish().unwrap();
        assert_eq!(
            manifest.file_id().as_str(),
            format!("blake3:{}", expected.finalize().to_hex())
        );
        assert_eq!(manifest.byte_length(), count * 1024 * 1024);
        let mut verifier = manifest.verifier();
        for index in 0..count {
            bytes[..8].copy_from_slice(&index.to_le_bytes());
            verifier.push(&bytes).unwrap();
        }
        verifier.finish().unwrap();
        let encoded = canonical_json(&manifest).unwrap();
        let reopened: FileManifest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(reopened, manifest);
    }
    let additional = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    eprintln!("100MiB + 1GiB streaming identity: additional Rust heap peak {additional} bytes");
    assert!(
        additional < 16 * 1024 * 1024,
        "streaming core retained full-file content"
    );
}
