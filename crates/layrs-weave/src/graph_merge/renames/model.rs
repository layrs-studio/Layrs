use super::*;

/// A normative multi-placement proposal derived from exact original Trees.
/// PlannedConflict accepts its canonical shape; readers must additionally
/// validate the original Tree closure before planning or resolving it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PlannedRenameConflict(Value);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameTermResolution {
    pub conflict_id: ConflictId,
    pub term_id: ObjectId,
    /// Exact replacement/deletion at EVERY affected path, applied as one unit.
    /// This is not an accepted Step, checkout edit or independent path choices.
    pub values: BTreeMap<RepoPath, Option<TreeEntry>>,
}

impl PlannedRenameConflict {
    pub(super) fn from_shape(value: Value) -> Self {
        Self(value)
    }
    pub(in crate::graph_merge) fn validate_trees(
        &self,
        objects: &MergeObjects,
        limits: GraphMergeLimits,
    ) -> MergeResult<()> {
        let step = |term: &Value| -> MergeResult<StepId> {
            let id =
                StepId::parse(term["provenance"]["stepId"].as_str().unwrap()).map_err(invalid)?;
            if objects.steps.get(&id).is_none_or(|s| s.id() != &id) {
                return Err(GraphMergeError::Missing(id.to_string()));
            }
            Ok(id)
        };
        let base = step(&self.0["bases"][0])?;
        let sides = self.0["sides"]
            .as_array()
            .unwrap()
            .iter()
            .map(step)
            .collect::<MergeResult<Vec<_>>>()?;
        let dag = StepDag::from_steps(objects.steps.values()).map_err(invalid)?;
        if dag.merge_bases(&sides).map_err(invalid)? != [base.clone()] {
            return Err(invalid("rename base does not match original side ancestry"));
        }
        let expected = analyze_validated(objects, &base, &sides, limits)?;
        if !expected.conflicts.iter().any(|c| c == self) {
            return Err(invalid("rename proposal differs from actual source Trees"));
        }
        Ok(())
    }
    pub fn id(&self) -> ConflictId {
        ConflictId::parse(self.0["conflictId"].as_str().unwrap()).expect("derived Conflict ID")
    }
    pub fn document(&self) -> &Value {
        &self.0
    }
    pub fn canonical_bytes(&self) -> MergeResult<Vec<u8>> {
        canonical_json(&self.0).map_err(invalid)
    }

    /// No unchecked JSON constructor: reread the exact original Trees and
    /// reproduce the complete policy result, including ambiguity components.
    pub fn from_canonical_bytes(
        bytes: &[u8],
        objects: &MergeObjects,
        limits: GraphMergeLimits,
    ) -> MergeResult<Self> {
        if bytes.len() > limits.max_input_bytes {
            return Err(GraphMergeError::Limit("rename document"));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(invalid)?;
        if canonical_json(&value).map_err(invalid)? != bytes {
            return Err(invalid("noncanonical rename document"));
        }
        let bases = value["bases"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or_else(|| invalid("rename base"))?;
        let sides = value["sides"]
            .as_array()
            .filter(|v| v.len() == 2)
            .ok_or_else(|| invalid("rename sides"))?;
        let step = |term: &Value| {
            StepId::parse(
                term["provenance"]["stepId"]
                    .as_str()
                    .ok_or_else(|| invalid("rename provenance"))?,
            )
            .map_err(invalid)
        };
        let analysis = analyze_renames(
            objects,
            &step(&bases[0])?,
            &sides.iter().map(step).collect::<MergeResult<Vec<_>>>()?,
            limits,
        )?;
        analysis
            .conflicts
            .into_iter()
            .find(|candidate| candidate.0 == value)
            .ok_or_else(|| invalid("rename document does not match exact original Trees/policy"))
    }

    /// Select a complete base/side snapshot. Never allow independent choices to
    /// accidentally materialize two destinations for one coupled rename.
    pub fn select_term(&self, term_id: &ObjectId) -> MergeResult<RenameTermResolution> {
        let term = self.0["bases"]
            .as_array()
            .unwrap()
            .iter()
            .chain(self.0["sides"].as_array().unwrap())
            .find(|term| term["termId"].as_str() == Some(term_id.as_str()))
            .ok_or_else(|| invalid("term does not belong to rename conflict"))?;
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
        let mut values = BTreeMap::new();
        for placed in term["values"].as_array().unwrap() {
            let path = placements[placed["placementId"].as_str().unwrap()];
            let name =
                RepoPathComponent::parse(path.rsplit('/').next().unwrap()).map_err(invalid)?;
            let entry = structural::value_entry(&placed["value"], &name)?;
            values.insert(RepoPath::parse(path).map_err(invalid)?, entry);
        }
        Ok(RenameTermResolution {
            conflict_id: self.id(),
            term_id: term_id.clone(),
            values,
        })
    }
}

pub(super) fn conflict(
    base: &StepId,
    sides: &[StepId],
    paths: &BTreeSet<RepoPath>,
    sources: &BTreeSet<RepoPath>,
    destinations: &BTreeSet<RepoPath>,
    inventories: &BTreeMap<StepId, Inventory>,
) -> MergeResult<PlannedRenameConflict> {
    let mut placements = vec![];
    for path in paths {
        let role = match (sources.contains(path), destinations.contains(path)) {
            (true, false) => "source",
            (false, true) => "destination",
            _ => "affected",
        };
        let mut value = json!({"path":path,"role":role});
        value["placementId"] = json!(digest("layrs-conflict-placement-v1", &value)?);
        placements.push(value);
    }
    placements.sort_by(|a, b| a["placementId"].as_str().cmp(&b["placementId"].as_str()));
    let make_term = |kind: &str, step: &StepId| -> MergeResult<Value> {
        let mut values = vec![];
        for placement in &placements {
            let path = RepoPath::parse(placement["path"].as_str().unwrap()).map_err(invalid)?;
            let entry = inventories[step].get(&path).cloned();
            // The common term encoder supplies the sole normative value shape.
            let single = term(kind, step, &entry, placement)?;
            values.push(single["values"][0].clone());
        }
        let mut value =
            json!({"termKind":kind,"provenance":{"kind":"step","stepId":step},"values":values});
        value["termId"] = json!(digest("layrs-conflict-term-v1", &value)?);
        Ok(value)
    };
    let bases = vec![make_term("base", base)?];
    let mut side_terms = sides
        .iter()
        .map(|id| make_term("side", id))
        .collect::<MergeResult<Vec<_>>>()?;
    side_terms.sort_by(|a, b| a["termId"].as_str().cmp(&b["termId"].as_str()));
    let origins: BTreeSet<_> = std::iter::once(base).chain(sides).collect();
    let mut value = json!({"schemaVersion":"layrs.scm.conflict.v1","kind":"rename","placements":placements,
        "bases":bases,"sides":side_terms,"lensPin":null,"lensImplementationDigest":null,"renderedObjectId":null,
        "resolutionState":"unresolved","placementResolutions":[],"blocks":[],"segments":[],"originStepIds":origins});
    value["conflictId"] = json!(digest("layrs-conflict-v1", &value)?);
    Ok(PlannedRenameConflict(value))
}
