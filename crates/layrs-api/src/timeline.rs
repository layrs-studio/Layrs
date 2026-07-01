use crate::ids::{LayerId, SpaceId, TimelineEventId, WorkspaceId};
use crate::validation::{ApiResult, Validate, optional_positive};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TimelineEventKind {
    WorkspaceCreated,
    SpaceCreated,
    LayerCreated,
    LayerStateChanged,
    ChunkCommitted,
    WeaveCreated,
    ProofSubmitted,
    PolicyChanged,
    SyncPublished,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: Option<SpaceId>,
    pub layer_id: Option<LayerId>,
    pub since_cursor: Option<String>,
    pub limit: Option<u32>,
    pub include_weaves: bool,
    pub include_proofs: bool,
}

impl Validate for TimelineRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if let Some(space_id) = &self.space_id {
            space_id.validate_field("space_id")?;
        }
        if let Some(layer_id) = &self.layer_id {
            layer_id.validate_field("layer_id")?;
        }
        optional_positive("limit", self.limit)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineEventResponse {
    pub workspace_id: WorkspaceId,
    pub event_id: TimelineEventId,
    pub event_kind: TimelineEventKind,
    pub object_kind: String,
    pub object_id: String,
    pub actor_id: Option<String>,
    pub occurred_at: String,
    pub summary: String,
}
