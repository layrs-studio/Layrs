use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_core::scm::{
    ObjectId, RepositoryView, SpaceId, Tree, WorktreeId, WorktreeStatus, WorktreeViewState,
};
use layrs_store_server::v3::*;
use layrs_sync::v3::{Compression, ObjectDescriptor, ObjectKind};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP: AtomicU64 = AtomicU64::new(1);
#[path = "support/cas_upload_possession.rs"]
mod cas_upload_possession;

#[tokio::test]
async fn existing_filesystem_object_requires_full_verified_upload_body() {
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();
    let bytes = b"possession-object";
    let descriptor = descriptor(ObjectKind::Chunk, bytes, bytes, Compression::Identity);
    cas_upload_possession::check(&cas, &descriptor, bytes).await;
}
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "layrs-server-cas-{}-{}",
            std::process::id(),
            TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn domain(kind: ObjectKind) -> &'static [u8] {
    match kind {
        ObjectKind::Step => b"layrs-step-v1",
        ObjectKind::RepositoryView => b"layrs-repository-view-v1",
        ObjectKind::Chunk => b"layrs-chunk-v1",
        ObjectKind::FileManifest => b"layrs-file-manifest-v1",
        ObjectKind::Tree => b"layrs-tree-v1",
        ObjectKind::Conflict => b"layrs-conflict-v1",
        ObjectKind::Symlink => b"layrs-symlink-v1",
        ObjectKind::Tombstone => b"layrs-tombstone-v1",
        ObjectKind::Evidence => b"layrs-evidence-v1",
        ObjectKind::Product => b"layrs-product-v1",
        ObjectKind::PolicySet => b"layrs.sync.v3/policy-set",
        ObjectKind::AuthorizationSnapshot => b"layrs.sync.v3/authorization-snapshot",
    }
}

fn descriptor(
    kind: ObjectKind,
    canonical: &[u8],
    encoded: &[u8],
    compression: Compression,
) -> ObjectDescriptor {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain(kind));
    hasher.update(&[0]);
    hasher.update(canonical);
    ObjectDescriptor {
        kind,
        object_id: ObjectId::parse(format!("blake3:{}", hasher.finalize().to_hex())).unwrap(),
        canonical_byte_len: canonical.len() as u64,
        encoded_byte_len: encoded.len() as u64,
        compression,
    }
}

fn body(bytes: &[u8]) -> CasByteStream {
    Box::pin(stream::iter(
        bytes
            .chunks(3)
            .map(|v| Ok(Bytes::copy_from_slice(v)))
            .collect::<Vec<_>>(),
    ))
}

async fn collect(mut input: CasByteStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(chunk) = input.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    bytes
}

#[tokio::test]
async fn filesystem_cas_is_immutable_concurrent_and_supports_ranges() {
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();
    let bytes = b"hello streaming CAS";
    let descriptor = descriptor(ObjectKind::Chunk, bytes, bytes, Compression::Identity);
    let (left, right) = tokio::join!(
        cas.put_if_absent(&descriptor, body(bytes)),
        cas.put_if_absent(&descriptor, body(bytes))
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| **v == PutOutcome::Installed)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| **v == PutOutcome::AlreadyPresent)
            .count(),
        1
    );
    assert_eq!(
        collect(
            cas.verified_range(&descriptor, ByteRange::new(6, 15).unwrap())
                .await
                .unwrap()
        )
        .await,
        b"streaming"
    );
    assert_eq!(
        collect(cas.verified_stream(&descriptor).await.unwrap()).await,
        bytes
    );
}

#[tokio::test]
async fn zstd_is_bounded_and_malformed_input_is_rejected() {
    let root = TestDir::new();
    let cas = FsServerCas::open(
        &root.0,
        CasLimits {
            max_encoded_bytes: 1024 * 1024,
            max_canonical_bytes: 64,
        },
    )
    .await
    .unwrap();
    let bomb = vec![b'x'; 4096];
    let encoded = zstd::stream::encode_all(&bomb[..], 1).unwrap();
    let mut limited = descriptor(ObjectKind::Chunk, &bomb, &encoded, Compression::Zstd);
    limited.canonical_byte_len = 64;
    assert!(matches!(
        cas.put_if_absent(&limited, body(&encoded)).await,
        Err(V3StoreError::LimitExceeded {
            resource: "canonical object",
            ..
        })
    ));

    let malformed = b"not-zstd";
    let expected = b"x";
    let malformed_descriptor =
        descriptor(ObjectKind::Chunk, expected, malformed, Compression::Zstd);
    assert!(matches!(
        cas.put_if_absent(&malformed_descriptor, body(malformed))
            .await,
        Err(V3StoreError::CorruptObject(_))
    ));
}

#[tokio::test]
async fn scrub_and_reads_detect_tampering() {
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();
    let bytes = b"immutable";
    let descriptor = descriptor(ObjectKind::Chunk, bytes, bytes, Compression::Identity);
    cas.put_if_absent(&descriptor, body(bytes)).await.unwrap();
    let hex = &descriptor.object_id.as_str()[7..];
    let path = root.0.join("objects/chunk").join(&hex[..2]).join(hex);
    std::fs::write(path, b"tampered").unwrap();
    assert!(matches!(
        cas.scrub(&descriptor).await.unwrap(),
        ScrubOutcome::Corrupt(_)
    ));
    assert!(cas.verified_stream(&descriptor).await.is_err());
}

#[tokio::test]
async fn typed_scm_records_use_their_real_encoder_and_require_exact_jcs() {
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();
    let tree = Tree::new(vec![]).unwrap();
    let canonical = serde_jcs::to_vec(&tree).unwrap();
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Tree,
        object_id: ObjectId::parse(tree.id().as_str()).unwrap(),
        canonical_byte_len: canonical.len() as u64,
        encoded_byte_len: canonical.len() as u64,
        compression: Compression::Identity,
    };
    assert_eq!(
        cas.put_if_absent(&descriptor, body(&canonical))
            .await
            .unwrap(),
        PutOutcome::Installed
    );

    let pretty = serde_json::to_vec_pretty(&tree).unwrap();
    let mut noncanonical = descriptor.clone();
    noncanonical.canonical_byte_len = pretty.len() as u64;
    noncanonical.encoded_byte_len = pretty.len() as u64;
    let other_root = TestDir::new();
    let other = FsServerCas::open(&other_root.0, CasLimits::default())
        .await
        .unwrap();
    assert!(matches!(
        other.put_if_absent(&noncanonical, body(&pretty)).await,
        Err(V3StoreError::CorruptObject(_))
    ));
}

#[tokio::test]
async fn server_cas_rejects_repository_view_with_local_worktree_state() {
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();
    let view = RepositoryView::new(
        SpaceId::parse("space:demo").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![WorktreeViewState::new(
            WorktreeId::parse("worktree:local-only").unwrap(),
            None,
            None,
            None,
            WorktreeStatus::Ready,
        )],
    )
    .unwrap();
    let canonical = serde_jcs::to_vec(&view).unwrap();
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::RepositoryView,
        object_id: ObjectId::parse(view.id().as_str()).unwrap(),
        canonical_byte_len: canonical.len() as u64,
        encoded_byte_len: canonical.len() as u64,
        compression: Compression::Identity,
    };

    assert!(matches!(
        cas.put_if_absent(&descriptor, body(&canonical)).await,
        Err(V3StoreError::CorruptObject(_))
    ));
}

#[tokio::test]
async fn conflict_cas_accepts_the_normative_typed_golden_vector() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/contracts/scm-v1/golden-vectors.json"
    ))
    .unwrap();
    let vector = vectors["hashVectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "conflict-minimal-unresolved")
        .unwrap();
    let canonical = serde_jcs::to_vec(&vector["input"]).unwrap();
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Conflict,
        object_id: serde_json::from_value(vector["expectedObjectId"].clone()).unwrap(),
        canonical_byte_len: canonical.len() as u64,
        encoded_byte_len: canonical.len() as u64,
        compression: Compression::Identity,
    };
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();

    assert_eq!(
        cas.put_if_absent(&descriptor, body(&canonical))
            .await
            .unwrap(),
        PutOutcome::Installed
    );
}

#[tokio::test]
async fn conflict_cas_rejects_arbitrary_canonical_json_with_a_valid_hash() {
    let payload = serde_jcs::to_vec(&json!({
        "schemaVersion": "layrs.scm.conflict.v1"
    }))
    .unwrap();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-conflict-v1\0");
    hasher.update(&payload);
    let object_id = ObjectId::parse(format!("blake3:{}", hasher.finalize().to_hex())).unwrap();
    let canonical = serde_jcs::to_vec(&json!({
        "schemaVersion": "layrs.scm.conflict.v1",
        "conflictId": object_id,
    }))
    .unwrap();
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Conflict,
        object_id,
        canonical_byte_len: canonical.len() as u64,
        encoded_byte_len: canonical.len() as u64,
        compression: Compression::Identity,
    };
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();

    assert!(matches!(
        cas.put_if_absent(&descriptor, body(&canonical)).await,
        Err(V3StoreError::CorruptObject(_))
    ));
}

struct TestProductVerifier;
impl TypedObjectVerifier for TestProductVerifier {
    fn verify(
        &self,
        descriptor: &ObjectDescriptor,
        canonical: &mut dyn std::io::Read,
    ) -> V3StoreResult<()> {
        let mut bytes = Vec::new();
        canonical.read_to_end(&mut bytes)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"test-product-v1\0");
        hasher.update(&bytes);
        if format!("blake3:{}", hasher.finalize().to_hex()) != descriptor.object_id.as_str() {
            return Err(V3StoreError::IdentityMismatch);
        }
        Ok(())
    }
}

#[tokio::test]
async fn typed_extension_cannot_bypass_builtin_product_verification() {
    let bytes = br#"{"kind":"test"}"#;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"test-product-v1\0");
    hasher.update(bytes);
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Product,
        object_id: ObjectId::parse(format!("blake3:{}", hasher.finalize().to_hex())).unwrap(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    };
    let closed_root = TestDir::new();
    let closed = FsServerCas::open(&closed_root.0, CasLimits::default())
        .await
        .unwrap();
    assert!(
        closed
            .put_if_absent(&descriptor, body(bytes))
            .await
            .is_err()
    );

    let open_root = TestDir::new();
    let extended = FsServerCas::open_with_typed_verifier(
        &open_root.0,
        CasLimits::default(),
        std::sync::Arc::new(TestProductVerifier),
    )
    .await
    .unwrap();
    assert!(
        extended
            .put_if_absent(&descriptor, body(bytes))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn evidence_accepts_normative_empty_and_binary_vectors_and_enforces_64_mib() {
    let root = TestDir::new();
    let cas = FsServerCas::open(
        &root.0,
        CasLimits {
            max_encoded_bytes: 128 * 1024 * 1024,
            max_canonical_bytes: 128 * 1024 * 1024,
        },
    )
    .await
    .unwrap();
    for (bytes, expected) in [
        (
            &b""[..],
            "blake3:5c2342d0bb714f79a97ad9b13151ed909d20a1741df627afa2916eb4b4ed8ae6",
        ),
        (
            &[0x00, 0xff, 0x80, 0x00, b'l', b'a', b'y', b'r', b's', b'\n'][..],
            "blake3:0e99b39b4f4222e04068565695c9ec164ca03c7b541faf20ccc0b61e9c8524d9",
        ),
    ] {
        let descriptor = ObjectDescriptor {
            kind: ObjectKind::Evidence,
            object_id: ObjectId::parse(expected).unwrap(),
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        };
        assert_eq!(
            cas.put_if_absent(&descriptor, body(bytes)).await.unwrap(),
            PutOutcome::Installed
        );
    }

    let too_large = ObjectDescriptor {
        kind: ObjectKind::Evidence,
        object_id: ObjectId::parse(format!("blake3:{}", "a".repeat(64))).unwrap(),
        canonical_byte_len: 64 * 1024 * 1024 + 1,
        encoded_byte_len: 1,
        compression: Compression::Zstd,
    };
    assert!(matches!(
        cas.put_if_absent(&too_large, body(&[0])).await,
        Err(V3StoreError::LimitExceeded {
            resource: "evidence object",
            ..
        })
    ));
}

#[tokio::test]
async fn product_uses_normative_identity_and_rejects_wrong_claim_kind_and_non_jcs() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/contracts/sync-v3/identity-vectors.json"
    ))
    .unwrap();
    let vector = vectors["hashVectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["name"] == "product-layer-variant")
        .unwrap();
    let input = vector["input"].clone();
    let canonical = serde_jcs::to_vec(&input).unwrap();
    let expected = vector["expectedObjectId"].as_str().unwrap();
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Product,
        object_id: ObjectId::parse(expected).unwrap(),
        canonical_byte_len: canonical.len() as u64,
        encoded_byte_len: canonical.len() as u64,
        compression: Compression::Identity,
    };
    let root = TestDir::new();
    let cas = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();
    assert_eq!(
        cas.put_if_absent(&descriptor, body(&canonical))
            .await
            .unwrap(),
        PutOutcome::Installed
    );

    let mut wrong_claim = input.clone();
    wrong_claim["productObjectId"] =
        serde_json::Value::String(format!("blake3:{}", "f".repeat(64)));
    let wrong_claim = serde_jcs::to_vec(&wrong_claim).unwrap();
    let mut wrong_claim_descriptor = descriptor.clone();
    wrong_claim_descriptor.canonical_byte_len = wrong_claim.len() as u64;
    wrong_claim_descriptor.encoded_byte_len = wrong_claim.len() as u64;
    let wrong_root = TestDir::new();
    let wrong_cas = FsServerCas::open(&wrong_root.0, CasLimits::default())
        .await
        .unwrap();
    assert!(
        wrong_cas
            .put_if_absent(&wrong_claim_descriptor, body(&wrong_claim))
            .await
            .is_err()
    );

    let wrong_kind = ObjectDescriptor {
        kind: ObjectKind::Evidence,
        ..descriptor.clone()
    };
    let kind_root = TestDir::new();
    let kind_cas = FsServerCas::open(&kind_root.0, CasLimits::default())
        .await
        .unwrap();
    assert!(matches!(
        kind_cas.put_if_absent(&wrong_kind, body(&canonical)).await,
        Err(V3StoreError::IdentityMismatch)
    ));

    let non_jcs = serde_json::to_vec_pretty(&input).unwrap();
    let noncanonical_descriptor = ObjectDescriptor {
        canonical_byte_len: non_jcs.len() as u64,
        encoded_byte_len: non_jcs.len() as u64,
        ..descriptor
    };
    let pretty_root = TestDir::new();
    let pretty_cas = FsServerCas::open(&pretty_root.0, CasLimits::default())
        .await
        .unwrap();
    assert!(matches!(
        pretty_cas
            .put_if_absent(&noncanonical_descriptor, body(&non_jcs))
            .await,
        Err(V3StoreError::CorruptObject(_))
    ));
}
