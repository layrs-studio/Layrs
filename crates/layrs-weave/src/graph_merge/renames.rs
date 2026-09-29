//! Exact-value movement analysis. No similarity threshold or candidate ordering
//! can select a destination. Captured Trees are never rewritten by inference.
use super::*;

mod model;
mod shape;
pub use model::{PlannedRenameConflict, RenameTermResolution};
pub(super) use shape::parse as parse_shape;

pub const POLICY_ID: &str = "layrs.exact-value-rename.v1";
type Inventory = BTreeMap<RepoPath, TreeEntry>;
type Identity = (u8, String, bool);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactRename {
    pub step_id: StepId,
    pub source: RepoPath,
    pub destination: RepoPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmbiguousRename {
    pub step_id: StepId,
    /// All original occurrences, not an arbitrarily selected source.
    pub base_candidates: Vec<RepoPath>,
    pub removed_sources: Vec<RepoPath>,
    pub added_destinations: Vec<RepoPath>,
    pub surviving_occurrences: Vec<RepoPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameAnalysis {
    pub policy: &'static str,
    pub exact: Vec<ExactRename>,
    pub ambiguous: Vec<AmbiguousRename>,
    /// Coupled paths whose side snapshots differ. These must never be resolved
    /// independently by a path-only planner.
    pub conflicts: Vec<PlannedRenameConflict>,
}

/// One real base and two exact side Steps. The whole input closure is validated
/// and charged before deriving movements. Other base strategies stay explicit.
pub fn analyze_renames(
    objects: &MergeObjects,
    base: &StepId,
    sides: &[StepId],
    limits: GraphMergeLimits,
) -> MergeResult<RenameAnalysis> {
    if objects.steps.len() > limits.max_steps {
        return Err(GraphMergeError::Limit("rename Steps"));
    }
    if sides.len() != 2 || sides[0] == sides[1] {
        return Err(invalid("rename analysis requires two distinct sides"));
    }
    let mut input_bytes = 0usize;
    for (id, step) in &objects.steps {
        if step.id() != id {
            return Err(invalid("rename Step identity"));
        }
        input_bytes = input_bytes
            .checked_add(step.identity_bytes().len())
            .ok_or(GraphMergeError::Limit("rename input"))?;
    }
    if input_bytes > limits.max_input_bytes {
        return Err(GraphMergeError::Limit("rename input"));
    }
    StepDag::from_steps(objects.steps.values()).map_err(invalid)?;
    let mut validator = InputValidator {
        objects,
        limits,
        paths: 0,
        bytes: input_bytes,
        seen_files: BTreeSet::new(),
        seen_metadata: BTreeSet::new(),
        validated_conflicts: BTreeSet::new(),
    };
    for id in std::iter::once(base).chain(sides) {
        let step = objects
            .steps
            .get(id)
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        validator.tree(step.root_tree_id(), "", 0)?;
    }
    analyze_validated(objects, base, sides, limits)
}

impl Planner<'_> {
    pub(super) fn install_renames(
        &mut self,
        bases: &[StepId],
        sides: &[StepId],
    ) -> MergeResult<()> {
        if bases.len() != 1 || sides.len() != 2 {
            return Ok(());
        }
        let analysis = analyze_validated(self.objects, &bases[0], sides, self.plan.limits)?;
        for proposal in analysis.conflicts {
            let conflict = PlannedConflict::from_canonical_bytes(&proposal.canonical_bytes()?)?;
            let id = conflict.id();
            for path in conflict.anchor_paths()? {
                if self
                    .rename_overrides
                    .insert(path.to_string(), id.clone())
                    .is_some()
                {
                    return Err(invalid("overlapping rename conflict anchors"));
                }
            }
            self.charge(conflict.canonical_bytes()?.len())?;
            self.plan.conflicts.insert(id, conflict);
        }
        Ok(())
    }
}

fn identity(entry: &TreeEntry) -> Option<Identity> {
    match entry.target() {
        TreeEntryTarget::File { file_id } => Some((0, file_id.to_string(), entry.executable())),
        TreeEntryTarget::Symlink { symlink_id } => Some((1, symlink_id.to_string(), false)),
        _ => None,
    }
}

fn occurrences(inventory: &Inventory) -> BTreeMap<Identity, Vec<RepoPath>> {
    let mut index = BTreeMap::<_, Vec<_>>::new();
    for (path, entry) in inventory {
        if let Some(id) = identity(entry) {
            index.entry(id).or_default().push(path.clone());
        }
    }
    index
}

fn analyze_validated(
    objects: &MergeObjects,
    base: &StepId,
    sides: &[StepId],
    limits: GraphMergeLimits,
) -> MergeResult<RenameAnalysis> {
    let mut side_ids = sides.to_vec();
    side_ids.sort();
    let mut inventories = BTreeMap::new();
    for id in std::iter::once(base).chain(&side_ids) {
        let mut inventory = BTreeMap::new();
        inventory_tree(
            objects,
            objects.steps[id].root_tree_id(),
            "",
            0,
            &mut inventory,
            limits,
        )?;
        inventories.insert(id.clone(), inventory);
    }
    let original = &inventories[base];
    let before = occurrences(original);
    let mut result = RenameAnalysis {
        policy: POLICY_ID,
        exact: vec![],
        ambiguous: vec![],
        conflicts: vec![],
    };
    let mut links = BTreeMap::<RepoPath, BTreeSet<RepoPath>>::new();
    let mut sources = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for step in &side_ids {
        let side = &inventories[step];
        let after = occurrences(side);
        for (value, base_paths) in &before {
            let removed: Vec<_> = base_paths
                .iter()
                .filter(|p| !side.contains_key(*p))
                .cloned()
                .collect();
            if removed.is_empty() {
                continue;
            }
            let Some(side_paths) = after.get(value) else {
                continue;
            };
            let added: Vec<_> = side_paths
                .iter()
                .filter(|p| !original.contains_key(*p))
                .cloned()
                .collect();
            if added.is_empty() {
                continue;
            }
            if base_paths.len() == 1 && side_paths.len() == 1 {
                result.exact.push(ExactRename {
                    step_id: step.clone(),
                    source: removed[0].clone(),
                    destination: added[0].clone(),
                });
            } else {
                result.ambiguous.push(AmbiguousRename {
                    step_id: step.clone(),
                    base_candidates: base_paths.clone(),
                    removed_sources: removed.clone(),
                    added_destinations: added.clone(),
                    surviving_occurrences: side_paths
                        .iter()
                        .filter(|p| original.contains_key(*p))
                        .cloned()
                        .collect(),
                });
            }
            sources.extend(base_paths.iter().cloned());
            destinations.extend(added.iter().cloned());
            // A star connects the component in linear space, including repeated
            // originals. Shared destinations unite competing value identities.
            let anchor = &base_paths[0];
            for path in base_paths.iter().chain(&added) {
                links
                    .entry(anchor.clone())
                    .or_default()
                    .insert(path.clone());
                links
                    .entry(path.clone())
                    .or_default()
                    .insert(anchor.clone());
            }
        }
    }
    // Parent existence/type is part of the exact tree state too. Include only
    // changed structural ancestors, not every changed Tree hash (which would
    // couple unrelated edits in an otherwise existing directory).
    let leaves: Vec<_> = links.keys().cloned().collect();
    for leaf in leaves {
        let mut parent = leaf.as_str();
        while let Some((ancestor, _)) = parent.rsplit_once('/') {
            parent = ancestor;
            let path = RepoPath::parse(ancestor).map_err(invalid)?;
            if std::iter::once(base).chain(&side_ids).all(|id| {
                inventories[id]
                    .get(&path)
                    .is_some_and(|e| e.target().tree_id().is_some())
            }) {
                continue;
            }
            links.entry(leaf.clone()).or_default().insert(path.clone());
            links.entry(path).or_default().insert(leaf.clone());
        }
    }
    let mut visited = BTreeSet::new();
    let mut output_bytes = 0usize;
    for start in links.keys() {
        if visited.contains(start) {
            continue;
        }
        let mut pending = vec![start.clone()];
        let mut component = BTreeSet::new();
        while let Some(path) = pending.pop() {
            if !visited.insert(path.clone()) {
                continue;
            }
            pending.extend(
                links[&path]
                    .iter()
                    .filter(|p| !visited.contains(*p))
                    .cloned(),
            );
            component.insert(path);
            if component.len() > 4096 {
                return Err(GraphMergeError::Limit("rename placements"));
            }
        }
        let changed: Vec<_> = side_ids
            .iter()
            .filter(|id| {
                component
                    .iter()
                    .any(|p| inventories[*id].get(p) != original.get(p))
            })
            .collect();
        if changed.len() < 2
            || component
                .iter()
                .all(|p| inventories[changed[0]].get(p) == inventories[changed[1]].get(p))
        {
            continue;
        }
        let conflict = model::conflict(
            base,
            &side_ids,
            &component,
            &sources,
            &destinations,
            &inventories,
        )?;
        output_bytes = output_bytes
            .checked_add(conflict.canonical_bytes()?.len())
            .ok_or(GraphMergeError::Limit("rename output"))?;
        if output_bytes > limits.max_output_bytes {
            return Err(GraphMergeError::Limit("rename output"));
        }
        result.conflicts.push(conflict);
    }
    result.exact.sort_by(|a, b| {
        (&a.step_id, &a.source, &a.destination).cmp(&(&b.step_id, &b.source, &b.destination))
    });
    result.conflicts.sort_by_key(PlannedRenameConflict::id);
    if canonical_json(&serde_json::to_value(&result).map_err(invalid)?)
        .map_err(invalid)?
        .len()
        > limits.max_output_bytes
    {
        return Err(GraphMergeError::Limit("rename analysis output"));
    }
    Ok(result)
}

fn inventory_tree(
    objects: &MergeObjects,
    id: &TreeId,
    prefix: &str,
    depth: usize,
    output: &mut Inventory,
    limits: GraphMergeLimits,
) -> MergeResult<()> {
    if depth > limits.max_depth {
        return Err(GraphMergeError::Limit("rename depth"));
    }
    let tree = objects
        .trees
        .get(id)
        .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
    if tree.id() != id {
        return Err(invalid("rename source Tree identity mismatch"));
    }
    for entry in tree.entries() {
        if matches!(entry.target(), TreeEntryTarget::Tombstone { .. }) {
            continue;
        }
        if output.len() >= limits.max_paths {
            return Err(GraphMergeError::Limit("rename paths"));
        }
        let path = RepoPath::parse(joined(prefix, entry.name().as_str())).map_err(invalid)?;
        output.insert(path.clone(), entry.clone());
        if let Some(id) = entry.target().tree_id() {
            inventory_tree(objects, id, path.as_str(), depth + 1, output, limits)?;
        }
    }
    Ok(())
}
