use crate::ids::{LayerId, SpaceId, WeaveId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, max_items, required};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeaveAnchor {
    pub object_kind: String,
    pub object_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateWeaveRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: Option<LayerId>,
    pub title: String,
    pub body: String,
    pub anchors: Vec<WeaveAnchor>,
}

impl Validate for CreateWeaveRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        self.space_id.validate_field("space_id")?;
        if let Some(layer_id) = &self.layer_id {
            layer_id.validate_field("layer_id")?;
        }
        bounded_len("title", &self.title, 2, 160)?;
        required("body", &self.body)?;
        max_items("anchors", &self.anchors, 64)?;
        for anchor in &self.anchors {
            required("anchor.object_kind", &anchor.object_kind)?;
            required("anchor.object_id", &anchor.object_id)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeaveResponse {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub weave_id: WeaveId,
    pub layer_id: Option<LayerId>,
    pub title: String,
    pub body: String,
    pub anchors: Vec<WeaveAnchor>,
    pub created_at: String,
    pub updated_at: String,
}
