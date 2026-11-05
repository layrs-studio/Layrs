use super::*;

impl PlannedConflict {
    pub fn is_multi_placement(&self) -> bool {
        self.0["placements"].as_array().unwrap().len() > 1
    }

    /// Tree occurrences occupy the outermost declared paths. Descendant
    /// placements are masked by an ancestor Conflict entry and remain explicit
    /// values in its canonical document, not independent resolvable entries.
    pub fn anchor_paths(&self) -> MergeResult<BTreeSet<RepoPath>> {
        let paths = self.0["placements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| RepoPath::parse(p["path"].as_str().unwrap()).map_err(invalid))
            .collect::<MergeResult<BTreeSet<_>>>()?;
        Ok(paths
            .iter()
            .filter(|path| {
                !path.as_str().match_indices('/').any(|(i, _)| {
                    paths.contains(
                        &RepoPath::parse(&path.as_str()[..i]).expect("canonical ancestor"),
                    )
                })
            })
            .cloned()
            .collect())
    }

    /// Required after loading the complete origin Step/Tree closure. Replaying
    /// the exact analysis binds every rename placement to its real source tree.
    pub fn validate_rename_context(
        &self,
        objects: &MergeObjects,
        limits: GraphMergeLimits,
    ) -> MergeResult<()> {
        if self.is_multi_placement() {
            renames::parse_shape(&self.0)?.validate_trees(objects, limits)?;
        }
        Ok(())
    }

    pub(super) fn placed_entries(
        &self,
        role: &str,
    ) -> MergeResult<Vec<(StepId, RepoPath, Option<TreeEntry>)>> {
        let placements: BTreeMap<_, _> = self.0["placements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["placementId"].as_str().unwrap(),
                    p["path"].as_str().unwrap(),
                )
            })
            .collect();
        let mut output = vec![];
        for term in self.0[role].as_array().unwrap() {
            let step =
                StepId::parse(term["provenance"]["stepId"].as_str().unwrap()).map_err(invalid)?;
            for placed in term["values"].as_array().unwrap() {
                let path = placements[placed["placementId"].as_str().unwrap()];
                let name =
                    RepoPathComponent::parse(path.rsplit('/').next().unwrap()).map_err(invalid)?;
                output.push((
                    step.clone(),
                    RepoPath::parse(path).map_err(invalid)?,
                    structural::value_entry(&placed["value"], &name)?,
                ));
            }
        }
        Ok(output)
    }

    pub(super) fn selected_at(
        &self,
        term_id: &ObjectId,
        path: &str,
    ) -> MergeResult<Option<TreeEntry>> {
        let placement = self.0["placements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"].as_str() == Some(path))
            .ok_or_else(|| invalid("conflict occurrence is not a placement"))?;
        let term = self.0["bases"]
            .as_array()
            .unwrap()
            .iter()
            .chain(self.0["sides"].as_array().unwrap())
            .find(|t| t["termId"].as_str() == Some(term_id.as_str()))
            .ok_or_else(|| invalid("resolution term does not belong to conflict"))?;
        let placed = term["values"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["placementId"] == placement["placementId"])
            .ok_or_else(|| invalid("resolution missing placement"))?;
        let name = RepoPathComponent::parse(path.rsplit('/').next().unwrap()).map_err(invalid)?;
        structural::value_entry(&placed["value"], &name)
    }
}

pub(super) fn validate_occurrences(
    objects: &MergeObjects,
    root: &TreeId,
    limits: GraphMergeLimits,
) -> MergeResult<()> {
    validate_root(objects, root, limits, &mut 0)
}

/// Validate exact Conflict anchor coverage independently of materialization.
/// The shared path budget covers all roots, including repeated subtrees.
pub fn validate_conflict_layout(
    objects: &MergeObjects,
    roots: &[TreeId],
    limits: GraphMergeLimits,
) -> MergeResult<()> {
    let mut paths = 0;
    for root in roots {
        validate_root(objects, root, limits, &mut paths)?;
    }
    Ok(())
}

fn validate_root(
    objects: &MergeObjects,
    root: &TreeId,
    limits: GraphMergeLimits,
    paths: &mut usize,
) -> MergeResult<()> {
    let mut pending = vec![(root.clone(), String::new(), 0usize)];
    let mut found: BTreeMap<ConflictId, BTreeSet<RepoPath>> = BTreeMap::new();
    while let Some((id, prefix, depth)) = pending.pop() {
        if depth > limits.max_depth {
            return Err(GraphMergeError::Limit("conflict occurrence depth"));
        }
        let tree = objects
            .trees
            .get(&id)
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        for entry in tree.entries() {
            *paths += 1;
            if *paths > limits.max_paths {
                return Err(GraphMergeError::Limit("conflict occurrences"));
            }
            let path = joined(&prefix, entry.name().as_str());
            if let Some(id) = entry.target().tree_id() {
                pending.push((id.clone(), path.clone(), depth + 1));
            }
            if let Some(id) = entry.target().conflict_id() {
                found
                    .entry(id.clone())
                    .or_default()
                    .insert(RepoPath::parse(path).map_err(invalid)?);
            }
        }
    }
    for (id, paths) in found {
        let conflict = objects
            .conflicts
            .get(&id)
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        if paths != conflict.anchor_paths()? {
            return Err(invalid(
                "conflict Tree occurrences do not cover all anchor placements",
            ));
        }
    }
    Ok(())
}
