//! Shape-only ingress. Actual source Trees must subsequently reproduce the
//! exact policy proposal; metadata hashes alone are insufficient evidence.
use super::*;

pub(in crate::graph_merge) fn parse(value: &Value) -> MergeResult<PlannedRenameConflict> {
    let input = value["placements"]
        .as_array()
        .filter(|p| (2..=4096).contains(&p.len()))
        .ok_or_else(|| invalid("rename placements require 2..4096 paths"))?;
    let mut placements = vec![];
    let mut paths = BTreeSet::new();
    for placement in input {
        let path = RepoPath::parse(
            placement["path"]
                .as_str()
                .ok_or_else(|| invalid("rename path"))?,
        )
        .map_err(invalid)?;
        let role = placement["role"]
            .as_str()
            .filter(|r| ["source", "destination", "affected"].contains(r))
            .ok_or_else(|| invalid("rename placement role"))?;
        if !paths.insert(path.clone()) {
            return Err(invalid("duplicate rename path"));
        }
        let mut expected = json!({"path":path,"role":role});
        expected["placementId"] = json!(digest("layrs-conflict-placement-v1", &expected)?);
        if placement != &expected {
            return Err(invalid("rename placement identity/fields"));
        }
        placements.push(expected);
    }
    placements.sort_by(|a, b| a["placementId"].as_str().cmp(&b["placementId"].as_str()));
    if &placements != input {
        return Err(invalid("rename placement order"));
    }
    let mut origins = BTreeSet::new();
    let mut parse_terms = |key: &str, kind: &str, count: usize| -> MergeResult<Vec<Value>> {
        let terms = value[key]
            .as_array()
            .filter(|t| t.len() == count)
            .ok_or_else(|| invalid("rename term count"))?;
        let mut output = vec![];
        for original in terms {
            let step = StepId::parse(
                original["provenance"]["stepId"]
                    .as_str()
                    .ok_or_else(|| invalid("rename real provenance"))?,
            )
            .map_err(invalid)?;
            origins.insert(step.clone());
            let input = original["values"]
                .as_array()
                .filter(|v| v.len() == placements.len())
                .ok_or_else(|| invalid("rename term must supply every placement"))?;
            let mut values = vec![];
            for (placed, placement) in input.iter().zip(&placements) {
                if placed["placementId"] != placement["placementId"] {
                    return Err(invalid("rename placed value order"));
                }
                let path = placement["path"].as_str().unwrap();
                let name =
                    RepoPathComponent::parse(path.rsplit('/').next().unwrap()).map_err(invalid)?;
                let entry = structural::value_entry(&placed["value"], &name)?;
                let encoded = term(kind, &step, &entry, placement)?;
                let expected = &encoded["values"][0];
                if expected != placed {
                    return Err(invalid("rename placed value shape"));
                }
                values.push(expected.clone());
            }
            let mut expected =
                json!({"termKind":kind,"provenance":{"kind":"step","stepId":step},"values":values});
            expected["termId"] = json!(digest("layrs-conflict-term-v1", &expected)?);
            if original != &expected {
                return Err(invalid("rename term identity/fields"));
            }
            output.push(expected);
        }
        if output
            .windows(2)
            .any(|w| w[0]["termId"].as_str() >= w[1]["termId"].as_str())
        {
            return Err(invalid("rename term order"));
        }
        Ok(output)
    };
    let bases = parse_terms("bases", "base", 1)?;
    let sides = parse_terms("sides", "side", 2)?;
    let mut expected = json!({"schemaVersion":"layrs.scm.conflict.v1","kind":"rename","placements":placements,
        "bases":bases,"sides":sides,"lensPin":null,"lensImplementationDigest":null,"renderedObjectId":null,
        "resolutionState":"unresolved","placementResolutions":[],"blocks":[],"segments":[],"originStepIds":origins});
    expected["conflictId"] = json!(digest("layrs-conflict-v1", &expected)?);
    if value != &expected {
        return Err(invalid("rename document identity/fields/state"));
    }
    Ok(PlannedRenameConflict::from_shape(expected))
}
