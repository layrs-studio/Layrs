use super::*;
use layrs_lens_sdk::{LensConflictSegmentKind, LensReconcileResult};

/// Upgrade only after a lossless, unambiguous reconstruction. The result from a
/// Lens can contain auto-merged text which does not reconstruct every original;
/// that case deliberately retains the complete structural conflict.
pub(in crate::graph_merge) fn upgrade(
    structural: PlannedConflict,
    result: &LensReconcileResult,
    ordered_terms: [&str; 3],
    files: &BTreeMap<FileId, Vec<u8>>,
    limit: usize,
) -> MergeResult<(PlannedConflict, BTreeMap<FileId, Vec<u8>>)> {
    let original = match originals(&structural.0) {
        Ok(original) => original,
        Err(_) => return Ok((structural, BTreeMap::new())),
    };
    if result.blocks.is_empty() {
        return Ok((structural, BTreeMap::new()));
    }
    let mode = original.values().next().unwrap().1;
    let mut output = structural.0.clone();
    let mut fragments = BTreeMap::new();
    let mut bytes_used = 0usize;
    let mut insert = |bytes: &[u8]| -> MergeResult<FileId> {
        let id = FileId::from_bytes(bytes);
        if !fragments.contains_key(&id) {
            bytes_used = bytes_used
                .checked_add(bytes.len())
                .ok_or(GraphMergeError::Limit("block output"))?;
            if bytes_used > limit {
                return Err(GraphMergeError::Limit("block output"));
            }
            fragments.insert(id.clone(), bytes.to_vec());
        }
        Ok(id)
    };
    let legacy: BTreeMap<_, _> = result
        .blocks
        .iter()
        .map(|b| (b.block_id.as_str(), b))
        .collect();
    if legacy.len() != result.blocks.len() {
        return Err(invalid("duplicate Lens block"));
    }
    let mut positions = [0usize; 3];
    let mut blocks = vec![];
    let mut segments = vec![];
    let mut used = BTreeSet::new();
    for segment in &result.segments {
        let (parts, block) = match segment.kind {
            LensConflictSegmentKind::Text => {
                let content = segment
                    .text
                    .as_deref()
                    .ok_or_else(|| invalid("Lens text segment"))?;
                ([content.as_bytes(); 3], None)
            }
            LensConflictSegmentKind::Block => {
                let key = segment
                    .block_id
                    .as_deref()
                    .ok_or_else(|| invalid("Lens block segment"))?;
                let block = legacy
                    .get(key)
                    .ok_or_else(|| invalid("missing Lens block"))?;
                if !used.insert(key) {
                    return Err(invalid("duplicate Lens segment"));
                }
                (
                    [
                        block.base.as_bytes(),
                        block.ours.as_bytes(),
                        block.theirs.as_bytes(),
                    ],
                    Some(*block),
                )
            }
        };
        let mut spans = vec![];
        let mut values = BTreeMap::new();
        for i in 0..3 {
            let source = files
                .get(&original[ordered_terms[i]].0)
                .ok_or_else(|| invalid("missing block original"))?;
            let end = positions[i]
                .checked_add(parts[i].len())
                .ok_or(GraphMergeError::Limit("block offset"))?;
            if source.get(positions[i]..end) != Some(parts[i]) {
                return Ok((structural, BTreeMap::new()));
            }
            spans.push(json!({"termId":ordered_terms[i],"startByte":positions[i],"endByte":end}));
            positions[i] = end;
            let id = insert(parts[i])?;
            values.insert(
                ordered_terms[i],
                json!({"termId":ordered_terms[i],"value":{
                "kind":"file","objectId":id,"executable":mode}}),
            );
        }
        if block.is_some() {
            spans.sort_by(|a, b| a["termId"].as_str().cmp(&b["termId"].as_str()));
            let mut canonical = json!({
                "baseValues":[values[ordered_terms[0]]],
                "sideValues":output["sides"].as_array().unwrap().iter()
                    .map(|t| values[t["termId"].as_str().unwrap()].clone()).collect::<Vec<_>>(),
                "supportedMethods":["manual","select_term"],
                "occurrenceAnchor":{"kind":"text","placementId":output["placements"][0]["placementId"],"spans":spans},
                "resolution":null
            });
            let id = block_id(&output, &canonical)?;
            canonical["blockId"] = json!(id);
            blocks.push(canonical);
            segments.push(json!({"kind":"block","blockId":id}));
        } else {
            segments.push(json!({"kind":"content","objectId":FileId::from_bytes(parts[0])}));
        }
    }
    if used.len() != legacy.len() {
        return Err(invalid("unreferenced Lens block"));
    }
    for i in 0..3 {
        if positions[i] != files[&original[ordered_terms[i]].0].len() {
            return Ok((structural, BTreeMap::new()));
        }
    }
    blocks.sort_by(|a, b| a["blockId"].as_str().cmp(&b["blockId"].as_str()));
    output["blocks"] = json!(blocks);
    output["segments"] = json!(segments);
    reidentify(&mut output)?;
    let conflict =
        PlannedConflict::from_canonical_bytes(&canonical_json(&output).map_err(invalid)?)?;
    Ok((conflict, fragments))
}
