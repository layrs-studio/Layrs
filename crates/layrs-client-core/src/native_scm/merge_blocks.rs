use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum BlockResolutionChoice {
    SelectTerm {
        term: ObjectId,
    },
    Manual {
        text: String,
        source_term_ids: Vec<ObjectId>,
    },
}

impl NativeWorkspace {
    pub fn preview_block_resolution(
        &self,
        layer: &str,
        conflict: ConflictId,
        block: ObjectId,
        choice: BlockResolutionChoice,
    ) -> Result<NativeResolutionPreview> {
        let choice = match choice {
            BlockResolutionChoice::SelectTerm { term } => {
                graph_merge::ConflictBlockResolution::SelectTerm(term)
            }
            BlockResolutionChoice::Manual {
                text,
                source_term_ids,
            } => {
                if text.len() > 1024 * 1024 {
                    return Err(NativeClientError::InvalidState(
                        "manual block exceeds 1 MiB",
                    ));
                }
                graph_merge::ConflictBlockResolution::Manual {
                    bytes: text.into_bytes(),
                    source_term_ids,
                }
            }
        };
        self.preview_conflict_blocks(
            &self.layer_named(layer)?,
            &BTreeMap::from([(conflict, BTreeMap::from([(block, choice)]))]),
            GraphMergeLimits::default(),
        )
    }

    /// A display reads exact admitted fragment bytes, never a truncated slice.
    pub fn conflict_block_variant(
        &self,
        layer: &LayerId,
        conflict: &ConflictId,
        block: &ObjectId,
        term: &ObjectId,
    ) -> Result<serde_json::Value> {
        if !self
            .layer_conflicts(layer)?
            .iter()
            .any(|d| d["conflictId"].as_str() == Some(conflict.as_str()))
        {
            return Err(NativeClientError::Missing("active Layer conflict"));
        }
        let state = self.state()?;
        let heads = state
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == layer)
            .ok_or(NativeClientError::Missing("Layer heads"))?
            .step_ids();
        let store = open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let objects = load_objects(
            &reader,
            state.space.id(),
            heads.to_vec(),
            GraphMergeLimits::default(),
        )?;
        let conflict = objects
            .conflicts
            .get(conflict)
            .ok_or(NativeClientError::Missing("Conflict"))?;
        conflict
            .validate_payload(&objects.files)
            .map_err(merge_error)?;
        let block_value = conflict.document()["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["blockId"].as_str() == Some(block.as_str()))
            .ok_or(NativeClientError::Missing("Conflict block"))?;
        let value = block_value["baseValues"]
            .as_array()
            .unwrap()
            .iter()
            .chain(block_value["sideValues"].as_array().unwrap())
            .find(|v| v["termId"].as_str() == Some(term.as_str()))
            .ok_or(NativeClientError::Missing("Conflict block term"))?;
        let id = FileId::parse(value["value"]["objectId"].as_str().unwrap())?;
        let bytes = objects
            .files
            .get(&id)
            .ok_or(NativeClientError::Missing("Conflict fragment"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(NativeClientError::InvalidState(
                "block display exceeds 1 MiB",
            ));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| NativeClientError::InvalidState("block is not UTF-8"))?;
        Ok(serde_json::json!({"text":text,"byteLength":bytes.len(),"blockId":block,"termId":term}))
    }
}
