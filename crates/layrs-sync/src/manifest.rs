use crate::objects::{LayerStateRef, LocalStepRef, ObjectRef};
use crate::validation::{SyncResult, validate_non_empty};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncManifestV2 {
    pub manifest_id: String,
    pub workspace_id: String,
    pub space_id: String,
    pub source_client_id: String,
    pub base_cursor: Option<String>,
    pub generated_at: String,
    pub layer_states: Vec<LayerStateRef>,
    pub local_steps: Vec<LocalStepRef>,
    pub required_objects: Vec<ObjectRef>,
}

impl SyncManifestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("manifest_id", &self.manifest_id)?;
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("space_id", &self.space_id)?;
        validate_non_empty("source_client_id", &self.source_client_id)?;
        validate_non_empty("generated_at", &self.generated_at)?;
        if let Some(cursor) = &self.base_cursor {
            validate_non_empty("base_cursor", cursor)?;
        }
        for layer_state in &self.layer_states {
            layer_state.validate()?;
        }
        for step in &self.local_steps {
            step.validate()?;
        }
        for object in &self.required_objects {
            object.validate()?;
        }
        Ok(())
    }
}
