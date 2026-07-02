//! Shared synchronization models for Layrs clients and server-side surfaces.
//!
//! This crate intentionally avoids transport and runtime dependencies. It is a
//! stable contract layer for API crates, store implementations, and future Axum
//! handlers.

mod chunking;
mod digest;
mod legacy;
mod manifest;
mod objects;
mod requests;
mod validation;

pub use chunking::*;
pub use digest::*;
pub use legacy::*;
pub use manifest::*;
pub use objects::*;
pub use requests::*;
pub use validation::{SyncResult, SyncValidationError};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_key_accepts_stable_ascii_token() {
        let key = IdempotencyKey::new("client-a:0000000001").expect("valid key");
        assert_eq!(key.as_str(), "client-a:0000000001");
    }

    #[test]
    fn idempotency_key_rejects_short_or_unsafe_values() {
        assert!(IdempotencyKey::new("short").is_err());
        assert!(IdempotencyKey::new("client-a key with spaces").is_err());
    }

    #[test]
    fn publish_request_validates_manifest_and_cursor() {
        let request = PublishRequest {
            idempotency_key: IdempotencyKey::new("client-a:0000000002").unwrap(),
            manifest: SyncManifest {
                manifest_id: "manifest-1".into(),
                workspace_id: "workspace-1".into(),
                space_id: Some("space-1".into()),
                source_client_id: "client-a".into(),
                base_cursor: None,
                capability_epoch: 1,
                generated_at: "2026-06-29T18:00:00Z".into(),
                chunks: vec![ChunkRef {
                    chunk_id: ChunkId::new("chunk-1").unwrap(),
                    digest: ContentDigest::new("sha256:abc").unwrap(),
                    byte_len: 128,
                    media_type: None,
                    compression: None,
                    encryption: None,
                }],
                operations: vec![SyncOperationRef {
                    operation_id: "op-1".into(),
                    entity_kind: SyncEntityKind::Layer,
                    entity_id: "layer-1".into(),
                    operation_kind: SyncOperationKind::Update,
                    client_sequence: 1,
                    base_version: None,
                    resulting_version: "v1".into(),
                    chunks: vec![ChunkId::new("chunk-1").unwrap()],
                }],
            },
            expected_server_cursor: Some("cursor-1".into()),
            dry_run: false,
        };

        assert_eq!(request.validate(), Ok(()));
    }

    #[test]
    fn object_digest_is_blake3_prefixed() {
        let digest = ObjectDigest::blake3_for(b"hello layrs");

        assert!(digest.as_str().starts_with(OBJECT_DIGEST_PREFIX));
        assert_eq!(digest.algorithm(), OBJECT_DIGEST_ALGORITHM);
        assert_eq!(digest.hex().len(), BLAKE3_HEX_LEN);
        assert_eq!(ObjectDigest::new(digest.as_str()), Ok(digest));
        assert!(ObjectDigest::new("sha256:abc").is_err());
        assert!(
            ObjectDigest::new(
                "blake3:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            )
            .is_err()
        );
    }

    #[test]
    fn deterministic_chunker_uses_single_chunk_for_small_files() {
        let chunker = DeterministicChunker::default();
        let bytes = vec![7_u8; SMALL_FILE_CHUNK_THRESHOLD];

        assert_eq!(
            chunker.chunk(&bytes),
            vec![ChunkSpan {
                offset: 0,
                byte_len: SMALL_FILE_CHUNK_THRESHOLD as u64
            }]
        );
    }

    #[test]
    fn deterministic_chunker_splits_large_files_contiguously() {
        let chunker = DeterministicChunker::default();
        let bytes = vec![9_u8; CDC_TARGET_CHUNK_BYTES + CDC_MIN_CHUNK_BYTES + 1];
        let chunks = chunker.chunk(&bytes);

        assert!(chunks.len() >= 2);
        assert_eq!(chunks.first().unwrap().offset, 0);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.byte_len).sum::<u64>(),
            bytes.len() as u64
        );
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].offset + pair[0].byte_len, pair[1].offset);
        }
    }

    #[test]
    fn file_tree_and_step_ids_are_stable_merkle_refs() {
        let chunker = DeterministicChunker::default();
        let (file, chunks) = FileObject::from_bytes(b"hello", &chunker).unwrap();
        let tree = TreeObject::from_entries(vec![TreeEntry::file("src/hello.txt", &file)]).unwrap();
        let step = LocalStepRef::new(
            "layer-main",
            tree.tree_id.clone(),
            None,
            7,
            "2026-06-30T12:00:00Z",
            Some("initial local step".into()),
        )
        .unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(
            file.file_object_id
                .as_str()
                .starts_with(OBJECT_DIGEST_PREFIX)
        );
        assert!(tree.tree_id.as_str().starts_with(OBJECT_DIGEST_PREFIX));
        assert!(step.step_id.as_str().starts_with(OBJECT_DIGEST_PREFIX));
        assert_eq!(file.validate(), Ok(()));
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(step.validate(), Ok(()));
    }

    #[test]
    fn publish_v2_serializes_desktop_server_contract_in_camel_case() {
        let bytes = b"hello";
        let chunk = ChunkObject::from_bytes(bytes);
        let (file, _) = FileObject::from_bytes(bytes, &DeterministicChunker::default()).unwrap();
        let tree = TreeObject::from_entries(vec![TreeEntry::file("src/hello.txt", &file)]).unwrap();
        let request = PublishRequestV2 {
            protocol: SYNC_PROTOCOL_V2.into(),
            layer_id: "layer-main".into(),
            policy_epoch: 9,
            idempotency_key: IdempotencyKey::new("desktop-a:0000001").unwrap(),
            source_client_id: "desktop-a".into(),
            base_tree_id: Some(tree.tree_id.clone()),
            root_tree_id: tree.tree_id.clone(),
            changed_paths: vec!["src/hello.txt".into()],
            store_objects: StoreObjectsV2 {
                chunks: vec![ChunkStoreObject::from_chunk_object(
                    chunk,
                    Some(ContentBytes::base64(bytes)),
                )],
                file_objects: vec![file],
                tree_objects: vec![tree],
                tombstones: vec![],
                deleted_paths: vec!["old/hello.txt".into()],
            },
        };

        request.validate().unwrap();
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["protocol"], SYNC_PROTOCOL_V2);
        assert_eq!(json["layerId"], "layer-main");
        assert_eq!(json["policyEpoch"], 9);
        assert_eq!(json["sourceClientId"], "desktop-a");
        assert!(json["rootTreeId"].as_str().unwrap().starts_with("blake3:"));
        assert!(json.get("root_tree_id").is_none());
        assert_eq!(
            json["storeObjects"]["chunks"][0]["content"]["encoding"],
            "base64"
        );
        assert_eq!(json["storeObjects"]["deletedPaths"][0], "old/hello.txt");

        let round_trip: PublishRequestV2 = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip.validate(), Ok(()));
    }

    #[test]
    fn publish_v2_rejects_non_blake3_or_hex_content_shapes() {
        let valid_digest = ObjectDigest::blake3_for(b"hello");
        let request = serde_json::json!({
            "protocol": SYNC_PROTOCOL_V2,
            "layerId": "layer-main",
            "policyEpoch": 1,
            "idempotencyKey": "desktop-a:0000002",
            "sourceClientId": "desktop-a",
            "baseTreeId": null,
            "rootTreeId": "sha256:abc",
            "changedPaths": ["src/hello.txt"],
            "storeObjects": {
                "chunks": [],
                "fileObjects": [],
                "treeObjects": [],
                "tombstones": [],
                "deletedPaths": []
            }
        });

        assert!(serde_json::from_value::<PublishRequestV2>(request).is_err());

        let request_with_hex_content = serde_json::json!({
            "protocol": SYNC_PROTOCOL_V2,
            "layerId": "layer-main",
            "policyEpoch": 1,
            "idempotencyKey": "desktop-a:0000003",
            "sourceClientId": "desktop-a",
            "baseTreeId": null,
            "rootTreeId": valid_digest.as_str(),
            "changedPaths": ["src/hello.txt"],
            "storeObjects": {
                "chunks": [{
                    "chunkId": valid_digest.as_str(),
                    "digest": valid_digest.as_str(),
                    "byteLen": 5,
                    "content": {"encoding": "hex", "bytes": "68656c6c6f"},
                    "compression": null,
                    "encryption": null
                }],
                "fileObjects": [],
                "treeObjects": [],
                "tombstones": [],
                "deletedPaths": []
            }
        });

        assert!(serde_json::from_value::<PublishRequestV2>(request_with_hex_content).is_err());
    }

    #[test]
    fn content_v2_response_uses_decodable_base64_chunks() {
        let bytes = b"hello";
        let digest = ObjectDigest::blake3_for(bytes);
        let response = ContentResponseV2 {
            protocol: SYNC_PROTOCOL_V2.into(),
            store_objects: StoreObjectsV2 {
                chunks: vec![ChunkStoreObject {
                    chunk_id: digest.clone(),
                    digest,
                    byte_len: bytes.len() as u64,
                    content: Some(ContentBytes::base64(bytes)),
                    compression: None,
                    encryption: None,
                }],
                file_objects: vec![],
                tree_objects: vec![],
                tombstones: vec![],
                deleted_paths: vec![],
            },
            missing_objects: vec![],
        };

        assert_eq!(response.validate(), Ok(()));
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(
            json["storeObjects"]["chunks"][0]["content"]["bytes"],
            "aGVsbG8="
        );
        let decoded = response.store_objects.chunks[0]
            .content
            .as_ref()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded, bytes);
    }
}
