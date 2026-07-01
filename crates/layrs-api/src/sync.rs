use crate::ids::WorkspaceId;
use crate::validation::{ApiResult, Validate, ValidationError};
use layrs_sync::{
    ContentRequestV2, ContentResponseV2, PublishReceipt, PublishRequest, PublishRequestV2,
    ReceiveRequest, ReceiveRequestV2, ReceiveResponse, ReceiveResponseV2, SyncDecision,
};

pub type PublishSyncRequest = PublishRequest;
pub type ReceiveSyncRequest = ReceiveRequest;
pub type PublishSyncV2Request = PublishRequestV2;
pub type ReceiveSyncV2Request = ReceiveRequestV2;
pub type SyncContentV2Request = ContentRequestV2;
pub type SyncContentV2Response = ContentResponseV2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspacePublishSyncRequest {
    pub workspace_id: WorkspaceId,
    pub publish: PublishRequest,
}

impl Validate for WorkspacePublishSyncRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if self.publish.validate().is_err() {
            return Err(ValidationError::new("publish", "is invalid"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncPublishResponse {
    pub decision: SyncDecision,
    pub receipt: Option<PublishReceipt>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncReceiveResponse {
    pub response: ReceiveResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspacePublishSyncV2Request {
    pub workspace_id: WorkspaceId,
    pub publish: PublishRequestV2,
}

impl Validate for WorkspacePublishSyncV2Request {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if self.publish.validate().is_err() {
            return Err(ValidationError::new("publish", "is invalid"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncReceiveV2Response {
    pub response: ReceiveResponseV2,
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_sync::{
        ChunkStoreObject, ContentBytes, FileObject, ObjectDigest, SYNC_PROTOCOL_V2, StoreObjectsV2,
    };

    #[test]
    fn publish_sync_v2_request_serializes_common_camel_case_contract() {
        let bytes = b"hello";
        let digest = ObjectDigest::blake3_for(bytes);
        let file = FileObject::from_chunks(
            digest.clone(),
            bytes.len() as u64,
            layrs_sync::ChunkingStrategy::Single,
            vec![layrs_sync::FileObjectChunk {
                chunk_id: digest.clone(),
                digest: digest.clone(),
                offset: 0,
                byte_len: bytes.len() as u64,
            }],
        )
        .unwrap();
        let request = PublishSyncV2Request {
            protocol: SYNC_PROTOCOL_V2.into(),
            layer_id: "layer-main".into(),
            policy_epoch: 3,
            idempotency_key: layrs_sync::IdempotencyKey::new("client-a:00000042").unwrap(),
            source_client_id: "desktop-a".into(),
            base_tree_id: None,
            root_tree_id: ObjectDigest::blake3_for(b"tree"),
            changed_paths: vec!["src/hello.txt".into()],
            store_objects: StoreObjectsV2 {
                chunks: vec![ChunkStoreObject {
                    chunk_id: digest.clone(),
                    digest,
                    byte_len: bytes.len() as u64,
                    content: Some(ContentBytes::base64(bytes)),
                    compression: None,
                    encryption: None,
                }],
                file_objects: vec![file],
                tree_objects: vec![],
                tombstones: vec![],
                deleted_paths: vec![],
            },
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["protocol"], SYNC_PROTOCOL_V2);
        assert_eq!(json["layerId"], "layer-main");
        assert_eq!(json["policyEpoch"], 3);
        assert!(json.get("layer_id").is_none());
        assert_eq!(
            json["storeObjects"]["chunks"][0]["content"]["encoding"],
            "base64"
        );
    }
}
