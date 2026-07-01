use crate::ids::{LayerId, SpaceId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, max_items};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LayerState {
    Draft,
    Review,
    Accepted,
    Archived,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateLayerRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub name: String,
    pub parent_layer_id: Option<LayerId>,
    pub base_layer_ids: Vec<LayerId>,
    pub description: Option<String>,
}

impl Validate for CreateLayerRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        self.space_id.validate_field("space_id")?;
        bounded_len("name", &self.name, 2, 128)?;
        if let Some(parent_layer_id) = &self.parent_layer_id {
            parent_layer_id.validate_field("parent_layer_id")?;
        }
        max_items("base_layer_ids", &self.base_layer_ids, 64)?;
        for layer_id in &self.base_layer_ids {
            layer_id.validate_field("base_layer_id")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateLayerStateRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub next_state: LayerState,
    pub reason: Option<String>,
}

impl Validate for UpdateLayerStateRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        self.space_id.validate_field("space_id")?;
        self.layer_id.validate_field("layer_id")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerResponse {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub name: String,
    pub state: LayerState,
    pub parent_layer_id: Option<LayerId>,
    pub base_layer_ids: Vec<LayerId>,
    pub created_at: String,
    pub updated_at: String,
}
