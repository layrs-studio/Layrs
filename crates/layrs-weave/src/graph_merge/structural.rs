use super::*;

impl PlannedConflict {
    /// Read single-placement structural conflicts and the exact three-term
    /// text-block subset, plus coupled exact-identity rename placements.
    /// Identity and exact JCS are checked here; call validate_payload after
    /// loading all referenced Files to validate semantic reconstruction, and
    /// validate_rename_context for the complete original Step/Tree closure.
    pub fn from_canonical_bytes(bytes: &[u8]) -> MergeResult<Self> {
        let value: Value = serde_json::from_slice(bytes).map_err(invalid)?;
        if canonical_json(&value).map_err(invalid)? != bytes {
            return Err(invalid("non-canonical Conflict JSON"));
        }
        if value["kind"] == "rename" && value["placements"].as_array().is_some_and(|p| p.len() > 1)
        {
            return Ok(Self(renames::parse_shape(&value)?.document().clone()));
        }
        let p = value["placements"]
            .as_array()
            .filter(|p| p.len() == 1)
            .ok_or_else(|| {
                GraphMergeError::Unsupported("requires single-placement structural conflict".into())
            })?;
        let path = p[0]["path"]
            .as_str()
            .ok_or_else(|| invalid("conflict path"))?;
        let expected_placement = placement(path)?;
        if p[0] != expected_placement {
            return Err(invalid("conflict placement identity/shape"));
        }
        let name = RepoPathComponent::parse(path.rsplit('/').next().unwrap()).map_err(invalid)?;
        let parse_terms = |key: &str| -> MergeResult<Vec<TermEntry>> {
            let terms = value[key]
                .as_array()
                .ok_or_else(|| invalid("conflict term array"))?;
            if terms.len() > 64 || (key == "sides" && terms.len() < 2) {
                return Err(invalid("conflict term count"));
            }
            let mut parsed = vec![];
            let mut previous: Option<String> = None;
            for input in terms {
                let step = StepId::parse(
                    input["provenance"]["stepId"]
                        .as_str()
                        .ok_or_else(|| invalid("real Step provenance required"))?,
                )
                .map_err(invalid)?;
                let values = input["values"]
                    .as_array()
                    .filter(|v| v.len() == 1)
                    .ok_or_else(|| invalid("one placed value required"))?;
                let entry = value_entry(&values[0]["value"], &name)?;
                let expected = term(
                    if key == "sides" { "side" } else { "base" },
                    &step,
                    &entry,
                    &expected_placement,
                )?;
                if input != &expected {
                    return Err(invalid("conflict term identity/shape"));
                }
                let id = input["termId"].as_str().unwrap().to_string();
                if previous.as_ref().is_some_and(|p| p >= &id) {
                    return Err(invalid("term set must be sorted and unique"));
                }
                previous = Some(id);
                parsed.push((step, entry));
            }
            Ok(parsed)
        };
        let bases = parse_terms("bases")?;
        let sides = parse_terms("sides")?;
        let pin: Option<layrs_lens_sdk::LensPin> =
            serde_json::from_value(value["lensPin"].clone()).map_err(invalid)?;
        if let Some(pin) = &pin {
            pin.validate().map_err(invalid)?;
        }
        let mut expected = conflict(path, &bases, &sides, pin.as_ref())?.0;
        let kind = value["kind"]
            .as_str()
            .ok_or_else(|| invalid("conflict kind"))?;
        if ![
            "content",
            "delete_modify",
            "rename",
            "type",
            "mode",
            "directory_file",
            "binary",
            "reference",
        ]
        .contains(&kind)
        {
            return Err(GraphMergeError::Unsupported("custom conflict kind".into()));
        }
        expected["kind"] = json!(kind);
        if value["blocks"].as_array().is_some_and(|b| !b.is_empty()) {
            semantic_blocks::validate_shape(&value, &mut expected)?;
        }
        expected.as_object_mut().unwrap().remove("conflictId");
        expected["conflictId"] = json!(digest("layrs-conflict-v1", &expected)?);
        if value != expected {
            return Err(invalid(
                "conflict identity, fields or unsupported rich resolution state",
            ));
        }
        Ok(Self(value))
    }

    pub(super) fn entries(
        &self,
        role: &str,
        name: &RepoPathComponent,
    ) -> MergeResult<Vec<TermEntry>> {
        if self.is_multi_placement() {
            return Err(GraphMergeError::Unsupported(
                "operation requires all rename placements".into(),
            ));
        }
        self.0[role]
            .as_array()
            .expect("validated conflict terms")
            .iter()
            .map(|t| {
                Ok((
                    StepId::parse(t["provenance"]["stepId"].as_str().unwrap()).map_err(invalid)?,
                    value_entry(&t["values"][0]["value"], name)?,
                ))
            })
            .collect()
    }
}

pub(super) fn value_entry(
    value: &Value,
    name: &RepoPathComponent,
) -> MergeResult<Option<TreeEntry>> {
    let kind = value["kind"]
        .as_str()
        .ok_or_else(|| invalid("conflict value kind"))?;
    if kind == "absent" {
        if value != &json!({"kind":"absent"}) {
            return Err(invalid("absent value fields"));
        }
        return Ok(None);
    }
    let object = value["objectId"]
        .as_str()
        .ok_or_else(|| invalid("conflict value objectId"))?;
    let (target, executable) = match kind {
        "file" => (
            TreeEntryTarget::file(FileId::parse(object).map_err(invalid)?),
            value["executable"]
                .as_bool()
                .ok_or_else(|| invalid("file mode required"))?,
        ),
        "tree" => (
            TreeEntryTarget::tree(TreeId::parse(object).map_err(invalid)?),
            false,
        ),
        "symlink" => (
            TreeEntryTarget::symlink(SymlinkId::parse(object).map_err(invalid)?),
            false,
        ),
        _ => return Err(invalid("unsupported canonical conflict value")),
    };
    if kind != "file" && !value["executable"].is_null() {
        return Err(invalid("non-file mode must be null"));
    }
    Ok(Some(TreeEntry::new(name.clone(), target, executable)))
}

impl Planner<'_> {
    pub(super) fn expanded_conflict(
        &mut self,
        path: &str,
        name: &RepoPathComponent,
        bases: &[TermEntry],
        sides: &[TermEntry],
    ) -> MergeResult<Option<TreeEntry>> {
        let mut expanded_bases = vec![];
        let mut expanded_sides = vec![];
        for (is_base, terms) in [(true, bases), (false, sides)] {
            for (step, entry) in terms {
                if let Some(id) = entry.as_ref().and_then(|e| e.target().conflict_id()) {
                    let stored = &self.objects.conflicts[id];
                    if !stored.0["blocks"].as_array().unwrap().is_empty() {
                        return Err(GraphMergeError::Unsupported(
                            "resolve semantic blocks before reconciling this conflict again".into(),
                        ));
                    }
                    expanded_bases.extend(stored.entries("bases", name)?);
                    let retained = stored.entries("sides", name)?;
                    if is_base {
                        expanded_bases.extend(retained);
                    } else {
                        expanded_sides.extend(retained);
                    }
                } else if is_base {
                    expanded_bases.push((step.clone(), entry.clone()));
                } else {
                    expanded_sides.push((step.clone(), entry.clone()));
                }
            }
        }
        // No clean side is automatically preferred over retained uncertainty.
        // Canonical term sets deduplicate repeated provenance/value identities.
        let value = conflict(path, &expanded_bases, &expanded_sides, None)?;
        let id = value.id();
        self.charge(value.canonical_bytes()?.len())?;
        self.plan.conflicts.insert(id.clone(), value);
        Ok(Some(TreeEntry::new(
            name.clone(),
            TreeEntryTarget::conflict(id),
            false,
        )))
    }

    pub(super) fn retain_conflicts(&mut self, id: &TreeId, depth: usize) -> MergeResult<()> {
        if depth > self.plan.limits.max_depth {
            return Err(GraphMergeError::Limit("depth"));
        }
        if !self.retained_trees.insert(id.clone()) {
            return Ok(());
        }
        let tree = self
            .plan
            .trees
            .get(id)
            .or_else(|| self.objects.trees.get(id))
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?
            .clone();
        for entry in tree.entries() {
            if let Some(id) = entry.target().tree_id() {
                self.retain_conflicts(id, depth + 1)?;
            }
            if let Some(id) = entry.target().conflict_id() {
                if !self.plan.conflicts.contains_key(id) {
                    let conflict = self
                        .objects
                        .conflicts
                        .get(id)
                        .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?
                        .clone();
                    self.charge(conflict.canonical_bytes()?.len())?;
                    self.plan.conflicts.insert(id.clone(), conflict.clone());
                }
                let conflict = self.plan.conflicts[id].clone();
                for (_, _, retained) in conflict
                    .placed_entries("bases")?
                    .into_iter()
                    .chain(conflict.placed_entries("sides")?)
                {
                    if let Some(tree) = retained.as_ref().and_then(|entry| entry.target().tree_id())
                    {
                        self.retain_conflicts(tree, depth + 1)?;
                    }
                }
            }
        }
        Ok(())
    }
}
