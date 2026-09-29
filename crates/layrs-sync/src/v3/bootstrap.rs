use super::{Cursor, PolicySet, PrincipalId, ProductRoot, SyncV3Protocol};
use layrs_core::scm::{
    Layer, Marker, ObjectId, Release, RepositoryView, RepositoryViewId, SpaceId, StepId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapType {
    #[serde(rename = "spaceBootstrap")]
    Bootstrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentAccessSchema {
    #[serde(rename = "layrs.content-access.v1")]
    V1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentAccessMode {
    #[serde(rename = "restricted")]
    Restricted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentAccess {
    pub schema_version: ContentAccessSchema,
    pub mode: ContentAccessMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryViewProjection {
    pub repository_view_id: RepositoryViewId,
    pub record: RepositoryView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicySetProjection {
    pub policy_set_id: ObjectId,
    pub record: PolicySet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerProjection {
    pub product_object_id: ObjectId,
    pub record: Layer,
    pub head_step_ids: Vec<StepId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkerProjection {
    pub product_object_id: ObjectId,
    pub record: Marker,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseProjection {
    pub product_object_id: ObjectId,
    pub record: Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceBootstrap {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: BootstrapType,
    pub principal_id: PrincipalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_access: Option<ContentAccess>,
    pub authorization_epoch: u64,
    pub space_id: SpaceId,
    pub watermark_cursor: Cursor,
    pub watermark_sequence: u64,
    pub current_repository_view_generation: u64,
    pub current_repository_view: RepositoryViewProjection,
    pub current_policy_set_id: ObjectId,
    pub current_policy_set: PolicySetProjection,
    pub layers: Vec<LayerProjection>,
    pub markers: Vec<MarkerProjection>,
    pub releases: Vec<ReleaseProjection>,
    pub policy_roots: Vec<ProductRoot>,
    pub graph_root_step_ids: Vec<StepId>,
    pub product_roots: Vec<ProductRoot>,
}
