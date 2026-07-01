use crate::ids::{ArtifactId, LayerId, ProofId, SpaceId, WorkspaceId};
use crate::validation::{ApiResult, Validate, max_items, required};
use layrs_sync::ChunkRef as SyncChunkRef;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ProofKind {
    Test,
    Review,
    Build,
    PolicyEvaluation,
    ManualAttestation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProofSubject {
    pub object_kind: String,
    pub object_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitProofRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: Option<LayerId>,
    pub kind: ProofKind,
    pub subject: ProofSubject,
    pub summary: String,
    pub artifact_id: Option<ArtifactId>,
    pub chunks: Vec<SyncChunkRef>,
}

impl Validate for SubmitProofRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        self.space_id.validate_field("space_id")?;
        if let Some(layer_id) = &self.layer_id {
            layer_id.validate_field("layer_id")?;
        }
        required("subject.object_kind", &self.subject.object_kind)?;
        required("subject.object_id", &self.subject.object_id)?;
        required("summary", &self.summary)?;
        if let Some(artifact_id) = &self.artifact_id {
            artifact_id.validate_field("artifact_id")?;
        }
        max_items("chunks", &self.chunks, 64)?;
        for chunk in &self.chunks {
            if chunk.validate().is_err() {
                return Err(crate::validation::ValidationError::new(
                    "chunk",
                    "is invalid",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProofResponse {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub proof_id: ProofId,
    pub kind: ProofKind,
    pub subject: ProofSubject,
    pub summary: String,
    pub artifact_id: Option<ArtifactId>,
    pub created_at: String,
}
