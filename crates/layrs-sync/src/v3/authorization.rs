use super::{
    AuthorizationRoot, ClientId, IdempotencyKey, PolicyId, PolicyVersionId, PrincipalId,
    ProductRoot,
};
use layrs_core::scm::{LayerId, ObjectId, RepoPath, SpaceId, Timestamp};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyVersionEntry {
    pub policy_id: PolicyId,
    pub policy_version_id: PolicyVersionId,
    pub policy_product_object_id: ObjectId,
    pub dependency_object_ids: Vec<ObjectId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicySetSchemaVersion {
    #[serde(rename = "layrs.policy-set.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicySet {
    pub schema_version: PolicySetSchemaVersion,
    pub policy_set_id: ObjectId,
    pub space_id: SpaceId,
    pub policy_versions: Vec<PolicyVersionEntry>,
}

impl PolicySet {
    pub fn computed_id(&self) -> Result<ObjectId, serde_json::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Identity<'a> {
            schema_version: PolicySetSchemaVersion,
            space_id: &'a SpaceId,
            policy_versions: &'a [PolicyVersionEntry],
        }
        let jcs = serde_jcs::to_vec(&Identity {
            schema_version: self.schema_version,
            space_id: &self.space_id,
            policy_versions: &self.policy_versions,
        })?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs.sync.v3/policy-set\0");
        hasher.update(&jcs);
        Ok(ObjectId::parse(format!("blake3:{}", hasher.finalize()))
            .expect("BLAKE3 is a valid ObjectId"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum AuthorizationSubject {
    #[serde(rename = "principal")]
    Principal {
        #[serde(rename = "principalId")]
        principal_id: PrincipalId,
    },
    #[serde(rename = "subjectDigest")]
    SubjectDigest {
        #[serde(rename = "subjectDigest")]
        subject_digest: ObjectId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum AuthorizationScope {
    #[serde(rename = "space")]
    Space {
        #[serde(rename = "spaceId")]
        space_id: SpaceId,
    },
    #[serde(rename = "layer")]
    Layer {
        #[serde(rename = "layerId")]
        layer_id: LayerId,
    },
    #[serde(rename = "path")]
    Path {
        root: AuthorizationRoot,
        #[serde(rename = "repoPath")]
        repo_path: RepoPath,
    },
    #[serde(rename = "product")]
    Product { product: ProductRoot },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationDecision {
    pub action: String,
    pub scope: AuthorizationScope,
    pub allowed: bool,
    pub policy_version_ids: Vec<PolicyVersionId>,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationRequestBinding {
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: ObjectId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorizationSnapshotSchemaVersion {
    #[serde(rename = "layrs.authorization-snapshot.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationSnapshot {
    pub schema_version: AuthorizationSnapshotSchemaVersion,
    pub authorization_snapshot_id: ObjectId,
    pub space_id: SpaceId,
    pub subject: AuthorizationSubject,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub request_binding: AuthorizationRequestBinding,
    pub decisions: Vec<AuthorizationDecision>,
    pub evaluated_at: Timestamp,
}

impl AuthorizationSnapshot {
    pub fn computed_id(&self) -> Result<ObjectId, serde_json::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Identity<'a> {
            schema_version: AuthorizationSnapshotSchemaVersion,
            space_id: &'a SpaceId,
            subject: &'a AuthorizationSubject,
            policy_set_id: &'a ObjectId,
            authorization_epoch: u64,
            request_binding: &'a AuthorizationRequestBinding,
            decisions: &'a [AuthorizationDecision],
            evaluated_at: &'a Timestamp,
        }
        let jcs = serde_jcs::to_vec(&Identity {
            schema_version: self.schema_version,
            space_id: &self.space_id,
            subject: &self.subject,
            policy_set_id: &self.policy_set_id,
            authorization_epoch: self.authorization_epoch,
            request_binding: &self.request_binding,
            decisions: &self.decisions,
            evaluated_at: &self.evaluated_at,
        })?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs.sync.v3/authorization-snapshot\0");
        hasher.update(&jcs);
        Ok(ObjectId::parse(format!("blake3:{}", hasher.finalize()))
            .expect("BLAKE3 is a valid ObjectId"))
    }
}
