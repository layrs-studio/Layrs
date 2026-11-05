use super::{
    AuthorizationRoot, ClientId, Compression, Cursor, Nullable, ObjectDescriptor, ObjectReference,
    PlanId, PrincipalId, RequestId, SyncV3Protocol,
};
use layrs_core::scm::{FileId, FileManifestId, ObjectId, RepoPath, SpaceId, Timestamp};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectPlanRequestType {
    #[serde(rename = "objectPlanRequest")]
    Request,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectPlanResponseType {
    #[serde(rename = "objectPlanResponse")]
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectPlanRequest {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: ObjectPlanRequestType,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
    pub objects: Vec<ObjectDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadMethod {
    #[serde(rename = "PUT")]
    Put,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadCapability {
    pub principal_id: PrincipalId,
    pub authorization_epoch: u64,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
    pub plan_id: PlanId,
    pub descriptor: ObjectDescriptor,
    pub descriptor_digest: ObjectId,
    pub method: UploadMethod,
    pub upload_url: String,
    pub token: String,
    pub expires_at: Timestamp,
    pub max_canonical_byte_len: u64,
    pub max_encoded_byte_len: u64,
    pub compression: Compression,
}

/// PUT capabilities always describe a complete encoded object. There is no
/// upload offset/range state: retry is full-object idempotent re-emission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectPlanResponse {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: ObjectPlanResponseType,
    pub principal_id: PrincipalId,
    pub authorization_epoch: u64,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
    pub plan_id: PlanId,
    pub present_objects: Vec<ObjectDescriptor>,
    pub missing_objects: Vec<ObjectDescriptor>,
    pub upload_capabilities: Vec<UploadCapability>,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthorizationSnapshotPurpose {
    Audit,
    Administration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum FetchClaim {
    #[serde(rename = "object")]
    Object {
        object: ObjectReference,
        root: AuthorizationRoot,
        #[serde(rename = "repoPath")]
        repo_path: Nullable<RepoPath>,
    },
    #[serde(rename = "file")]
    File {
        #[serde(rename = "fileId")]
        file_id: FileId,
        #[serde(rename = "preferredManifestIds")]
        preferred_manifest_ids: Vec<FileManifestId>,
        root: AuthorizationRoot,
        #[serde(rename = "repoPath")]
        repo_path: RepoPath,
    },
    #[serde(rename = "authorizationSnapshot")]
    AuthorizationSnapshot {
        #[serde(rename = "authorizationSnapshotId")]
        authorization_snapshot_id: ObjectId,
        purpose: AuthorizationSnapshotPurpose,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileManifestSelection {
    pub file_id: FileId,
    pub file_manifest_id: FileManifestId,
    pub manifest_descriptor: ObjectDescriptor,
    pub chunk_descriptors: Vec<ObjectDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadMethod {
    #[serde(rename = "GET")]
    Get,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadCapability {
    pub principal_id: PrincipalId,
    pub authorization_epoch: u64,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
    pub plan_id: PlanId,
    pub claim: FetchClaim,
    pub descriptor: ObjectDescriptor,
    pub claim_digest: ObjectId,
    pub method: DownloadMethod,
    pub download_url: String,
    pub token: String,
    pub expires_at: Timestamp,
    pub resume_token: String,
    /// `true` authorizes HTTP Range resume, still subject to full decoded-object verification.
    pub accept_ranges: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeniedClaimCode {
    NotReachable,
    PathDenied,
    AuditDenied,
    NotAvailable,
    InvalidRoot,
    InvalidPath,
    NoVerifiedManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeniedClaim {
    pub claim: FetchClaim,
    pub code: DeniedClaimCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FetchPlanRequestType {
    #[serde(rename = "fetchPlanRequest")]
    Request,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FetchPlanResponseType {
    #[serde(rename = "fetchPlanResponse")]
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FetchPlanRequest {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: FetchPlanRequestType,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
    pub claims: Vec<FetchClaim>,
    pub page_cursor: Nullable<Cursor>,
    pub max_objects: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FetchPlanResponse {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: FetchPlanResponseType,
    pub principal_id: PrincipalId,
    pub authorization_epoch: u64,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
    pub plan_id: PlanId,
    pub file_manifest_selections: Vec<FileManifestSelection>,
    pub download_capabilities: Vec<DownloadCapability>,
    pub denied_claims: Vec<DeniedClaim>,
    pub next_page_cursor: Nullable<Cursor>,
    pub has_more: bool,
    pub expires_at: Timestamp,
}
