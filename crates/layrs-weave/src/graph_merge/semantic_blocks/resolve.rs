use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictBlockResolution {
    SelectTerm(ObjectId),
    /// Exact UTF-8 replacement. Provenance is explicit; no side label is stored.
    Manual {
        bytes: Vec<u8>,
        source_term_ids: Vec<ObjectId>,
    },
}

pub(in crate::graph_merge) struct BlockResult {
    pub conflict: Option<PlannedConflict>,
    pub file: Option<(FileId, bool)>,
    pub files: BTreeMap<FileId, Vec<u8>>,
}

pub(in crate::graph_merge) fn resolve_blocks(
    conflict: &PlannedConflict,
    choices: &BTreeMap<ObjectId, ConflictBlockResolution>,
    files: &BTreeMap<FileId, Vec<u8>>,
    limit: usize,
) -> MergeResult<BlockResult> {
    conflict.validate_payload(files)?;
    let original = originals(&conflict.0)?;
    let mode = original.values().next().unwrap().1;
    let mut value = conflict.0.clone();
    let mut proposed = BTreeMap::new();
    let mut applied = 0;
    let mut bytes_used = 0usize;
    for block in value["blocks"].as_array_mut().unwrap() {
        let id = ObjectId::parse(text(&block["blockId"])?).map_err(invalid)?;
        let Some(choice) = choices.get(&id) else {
            continue;
        };
        if !block["resolution"].is_null() {
            return Err(invalid("block already resolved"));
        }
        applied += 1;
        block["resolution"] = match choice {
            ConflictBlockResolution::SelectTerm(id) => {
                let selected = array(block, "baseValues")?
                    .iter()
                    .chain(array(block, "sideValues")?)
                    .find(|v| v["termId"].as_str() == Some(id.as_str()))
                    .ok_or_else(|| invalid("selected term not in block"))?;
                json!({"method":"select_term","sourceTermIds":[id],"result":selected["value"]})
            }
            ConflictBlockResolution::Manual {
                bytes,
                source_term_ids,
            } => {
                if bytes.len() > limit
                    || std::str::from_utf8(bytes).is_err()
                    || bytes.contains(&0)
                    || source_term_ids.is_empty()
                    || source_term_ids.len() > 3
                    || source_term_ids.windows(2).any(|w| w[0] >= w[1])
                    || source_term_ids
                        .iter()
                        .any(|id| !original.contains_key(id.as_str()))
                {
                    return Err(invalid("manual block bytes/provenance"));
                }
                bytes_used = bytes_used
                    .checked_add(bytes.len())
                    .ok_or(GraphMergeError::Limit("block replacement"))?;
                if bytes_used > limit {
                    return Err(GraphMergeError::Limit("block replacement"));
                }
                let id = FileId::from_bytes(bytes);
                proposed.insert(id.clone(), bytes.clone());
                json!({"method":"manual","sourceTermIds":source_term_ids,
                    "result":{"kind":"file","objectId":id,"executable":mode}})
            }
        };
    }
    if choices.is_empty() || applied != choices.len() {
        return Err(invalid("unknown or empty block choices"));
    }
    if array(&value, "blocks")?
        .iter()
        .any(|b| b["resolution"].is_null())
    {
        value["resolutionState"] = json!("partial");
        reidentify(&mut value)?;
        let updated =
            PlannedConflict::from_canonical_bytes(&canonical_json(&value).map_err(invalid)?)?;
        return Ok(BlockResult {
            conflict: Some(updated),
            file: None,
            files: proposed,
        });
    }
    let blocks: BTreeMap<_, _> = array(&value, "blocks")?
        .iter()
        .map(|b| (b["blockId"].as_str().unwrap(), b))
        .collect();
    let mut result = vec![];
    for segment in array(&value, "segments")? {
        let id = if segment["kind"] == "content" {
            &segment["objectId"]
        } else {
            &blocks[text(&segment["blockId"])?]["resolution"]["result"]["objectId"]
        };
        let id = FileId::parse(text(id)?).map_err(invalid)?;
        let bytes = proposed
            .get(&id)
            .or_else(|| files.get(&id))
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        let next = result
            .len()
            .checked_add(bytes.len())
            .ok_or(GraphMergeError::Limit("resolved file"))?;
        if next.saturating_add(bytes_used) > limit {
            return Err(GraphMergeError::Limit("resolved file"));
        }
        result.extend_from_slice(bytes);
    }
    let id = FileId::from_bytes(&result);
    proposed.insert(id.clone(), result);
    Ok(BlockResult {
        conflict: None,
        file: Some((id, mode)),
        files: proposed,
    })
}
