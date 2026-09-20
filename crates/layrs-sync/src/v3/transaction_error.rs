use super::{ClientId, IdempotencyKey, SyncV3Protocol};
use layrs_core::scm::{ObjectId, SpaceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionErrorType {
    #[serde(rename = "syncTransactionError")]
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionErrorCode {
    IdempotencyDigestMismatch,
    RequestDigestMismatch,
}

/// Closed pre-semantic error response. These failures never create a receipt,
/// AuthorizationSnapshot, idempotency success row, or Space event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncTransactionError {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: TransactionErrorType,
    pub code: TransactionErrorCode,
    pub http_status: u16,
    pub receipt_created: bool,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: ObjectId,
    pub expected_request_digest: ObjectId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_receipt_unchanged: Option<bool>,
    pub message: String,
}

impl SyncTransactionError {
    pub fn idempotency_digest_mismatch(
        space_id: SpaceId,
        client_id: ClientId,
        idempotency_key: IdempotencyKey,
        request_digest: ObjectId,
        stored_request_digest: ObjectId,
    ) -> Self {
        Self {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionErrorType::Error,
            code: TransactionErrorCode::IdempotencyDigestMismatch,
            http_status: 409,
            receipt_created: false,
            space_id,
            client_id,
            idempotency_key,
            request_digest,
            expected_request_digest: stored_request_digest,
            existing_receipt_unchanged: Some(true),
            message: "idempotency key is already bound to another request digest".into(),
        }
    }

    pub fn request_digest_mismatch(
        space_id: SpaceId,
        client_id: ClientId,
        idempotency_key: IdempotencyKey,
        request_digest: ObjectId,
        computed_request_digest: ObjectId,
    ) -> Self {
        Self {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionErrorType::Error,
            code: TransactionErrorCode::RequestDigestMismatch,
            http_status: 409,
            receipt_created: false,
            space_id,
            client_id,
            idempotency_key,
            request_digest,
            expected_request_digest: computed_request_digest,
            existing_receipt_unchanged: None,
            message: "requestDigest does not match the canonical transaction".into(),
        }
    }
}
