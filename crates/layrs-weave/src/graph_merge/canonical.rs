use super::*;

pub(super) fn term(
    kind: &str,
    step: &StepId,
    entry: &Option<TreeEntry>,
    placement: &Value,
) -> MergeResult<Value> {
    let value = match entry {
        None => json!({"kind": "absent"}),
        Some(entry) => match entry.target() {
            TreeEntryTarget::File { file_id } => {
                json!({"kind":"file", "objectId":file_id, "executable":entry.executable()})
            }
            TreeEntryTarget::Tree { tree_id } => {
                json!({"kind":"tree", "objectId":tree_id, "executable":null})
            }
            TreeEntryTarget::Symlink { symlink_id } => {
                json!({"kind":"symlink", "objectId":symlink_id, "executable":null})
            }
            _ => return Err(GraphMergeError::Unsupported("conflict term kind".into())),
        },
    };
    let mut value = json!({"termKind":kind, "provenance":{"kind":"step", "stepId":step},
        "values":[{"placementId":placement["placementId"], "value":value}]});
    value["termId"] = json!(digest("layrs-conflict-term-v1", &value)?);
    Ok(value)
}
pub(super) fn conflict(
    path: &str,
    bases: &[TermEntry],
    sides: &[TermEntry],
    pin: Option<&layrs_lens_sdk::LensPin>,
) -> MergeResult<PlannedConflict> {
    let placement = placement(path)?;
    let terms = |kind, entries: &[TermEntry]| -> MergeResult<Vec<Value>> {
        let mut values = entries
            .iter()
            .map(|(step, entry)| term(kind, step, entry, &placement))
            .collect::<MergeResult<Vec<_>>>()?;
        values.sort_by(|a, b| a["termId"].as_str().cmp(&b["termId"].as_str()));
        values.dedup_by(|a, b| a["termId"] == b["termId"]);
        if values.len() > 64 || (kind == "side" && values.len() < 2) {
            return Err(GraphMergeError::Limit("canonical conflict terms"));
        }
        Ok(values)
    };
    let b = terms("base", bases)?;
    let s = terms("side", sides)?;
    let kinds: BTreeSet<_> = s
        .iter()
        .map(|t| t["values"][0]["value"]["kind"].as_str().unwrap())
        .collect();
    let kind = if kinds.contains("absent") {
        "delete_modify"
    } else if kinds.contains("tree") && kinds.contains("file") {
        "directory_file"
    } else if kinds.len() > 1 {
        "type"
    } else if pin.is_some() {
        "content"
    } else if kinds.contains("symlink") {
        "reference"
    } else {
        "binary"
    };
    let origins: BTreeSet<_> = bases.iter().chain(sides).map(|(step, _)| step).collect();
    let mut value = json!({"schemaVersion":"layrs.scm.conflict.v1", "kind":kind,
        "placements":[placement], "bases":b, "sides":s, "lensPin":pin,
        "lensImplementationDigest":pin.map(|p| &p.digest), "renderedObjectId":null,
        "resolutionState":"unresolved", "placementResolutions":[], "blocks":[], "segments":[],
        "originStepIds":origins });
    value["conflictId"] = json!(digest("layrs-conflict-v1", &value)?);
    Ok(PlannedConflict(value))
}
