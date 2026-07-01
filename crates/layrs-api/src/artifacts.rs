use crate::ids::{AccountId, ArtifactId, LayerId, SpaceId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, optional_required, required};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ArtifactState {
    Active,
    Redacted,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateArtifactRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub path: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub media_type: Option<String>,
    pub created_by_account_id: AccountId,
}

impl Validate for CreateArtifactRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        self.space_id.validate_field("space_id")?;
        self.layer_id.validate_field("layer_id")?;
        validate_artifact_path("path", &self.path)?;
        bounded_len("content_hash", &self.content_hash, 8, 256)?;
        if self.size_bytes == 0 {
            return Err(crate::validation::ValidationError::new(
                "size_bytes",
                "must be greater than zero",
            ));
        }
        optional_required("media_type", &self.media_type)?;
        self.created_by_account_id
            .validate_field("created_by_account_id")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactResponse {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub artifact_id: ArtifactId,
    pub path: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub media_type: Option<String>,
    pub state: ArtifactState,
    pub created_by_account_id: AccountId,
    pub created_at: String,
    pub updated_at: String,
}

pub fn validate_artifact_path(field: &'static str, value: &str) -> ApiResult<()> {
    required(field, value)?;
    if value.len() > 1024
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains("//")
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(crate::validation::ValidationError::new(
            field,
            "must be a normalized relative path",
        ));
    }
    Ok(())
}
