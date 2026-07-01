use crate::ids::{LayerId, PolicyId, SpaceId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, slug};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SpaceVisibility {
    Private,
    Team,
    Workspace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateSpaceRequest {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub key: String,
    pub visibility: SpaceVisibility,
    pub default_layer_id: Option<LayerId>,
    pub policy_ids: Vec<PolicyId>,
}

impl Validate for CreateSpaceRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        bounded_len("name", &self.name, 2, 128)?;
        slug("key", &self.key)?;
        if let Some(layer_id) = &self.default_layer_id {
            layer_id.validate_field("default_layer_id")?;
        }
        for policy_id in &self.policy_ids {
            policy_id.validate_field("policy_id")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpaceResponse {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub name: String,
    pub key: String,
    pub visibility: SpaceVisibility,
    pub default_layer_id: Option<LayerId>,
    pub policy_ids: Vec<PolicyId>,
    pub created_at: String,
    pub updated_at: String,
}
