use super::*;

/// An explicit user edit after the Weave, never an automatically selected side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictReplacement {
    pub bytes: Vec<u8>,
    pub executable: bool,
}

pub fn resolve_conflict_replacements(
    objects: &MergeObjects,
    parent: &StepId,
    replacements: &BTreeMap<ConflictId, ConflictReplacement>,
    limits: GraphMergeLimits,
) -> MergeResult<InstallationBundle> {
    resolve(
        objects,
        parent,
        &BTreeMap::new(),
        replacements,
        &BTreeMap::new(),
        limits,
    )
}

/// Resolve exact canonical terms in an immutable conflicted Step. Resolving a
/// subset of paths creates a descendant Step retaining every other conflict.
/// No input object is changed. A multi-placement term applies to every anchor
/// atomically; block and manual choices use their separate resolution APIs.
pub fn resolve_conflict_terms(
    objects: &MergeObjects,
    parent: &StepId,
    choices: &BTreeMap<ConflictId, ObjectId>,
    limits: GraphMergeLimits,
) -> MergeResult<InstallationBundle> {
    resolve(
        objects,
        parent,
        choices,
        &BTreeMap::new(),
        &BTreeMap::new(),
        limits,
    )
}

/// Resolve a subset of immutable text blocks. Partial output remains canonical
/// Conflict data; completing the last block produces an ordinary File entry.
pub fn resolve_conflict_blocks(
    objects: &MergeObjects,
    parent: &StepId,
    choices: &BTreeMap<ConflictId, BTreeMap<ObjectId, ConflictBlockResolution>>,
    limits: GraphMergeLimits,
) -> MergeResult<InstallationBundle> {
    resolve(
        objects,
        parent,
        &BTreeMap::new(),
        &BTreeMap::new(),
        choices,
        limits,
    )
}

fn resolve(
    objects: &MergeObjects,
    parent: &StepId,
    choices: &BTreeMap<ConflictId, ObjectId>,
    replacements: &BTreeMap<ConflictId, ConflictReplacement>,
    block_choices: &BTreeMap<ConflictId, BTreeMap<ObjectId, ConflictBlockResolution>>,
    limits: GraphMergeLimits,
) -> MergeResult<InstallationBundle> {
    if choices.is_empty() && replacements.is_empty() && block_choices.is_empty() {
        return Err(GraphMergeError::NoChanges);
    }
    if choices
        .len()
        .saturating_add(replacements.len())
        .saturating_add(block_choices.len())
        > limits.max_paths
    {
        return Err(GraphMergeError::Limit("resolution choices"));
    }
    // Reject caller-supplied payloads before hashing/copying or serializing any
    // replacement content. Base64 expansion is reserved before allocation too.
    let mut replacement_bytes = 0usize;
    let mut replacement_wire = 0usize;
    let manual = block_choices
        .values()
        .flat_map(|choices| choices.values())
        .filter_map(|choice| match choice {
            ConflictBlockResolution::Manual { bytes, .. } => Some(bytes),
            _ => None,
        });
    if block_choices.values().map(BTreeMap::len).sum::<usize>() > limits.max_paths {
        return Err(GraphMergeError::Limit("block choices"));
    }
    for bytes in replacements.values().map(|r| &r.bytes).chain(manual) {
        replacement_bytes = replacement_bytes
            .checked_add(bytes.len())
            .ok_or(GraphMergeError::Limit("replacement input bytes"))?;
        let encoded = bytes
            .len()
            .checked_add(2)
            .and_then(|n| n.checked_div(3))
            .and_then(|n| n.checked_mul(4))
            .ok_or(GraphMergeError::Limit("replacement output bytes"))?;
        replacement_wire = replacement_wire
            .checked_add(encoded)
            .ok_or(GraphMergeError::Limit("replacement output bytes"))?;
        if replacement_bytes > limits.max_input_bytes || replacement_wire > limits.max_output_bytes
        {
            return Err(GraphMergeError::Limit("replacement bytes"));
        }
    }
    let parent_step = objects
        .steps
        .get(parent)
        .ok_or_else(|| GraphMergeError::Missing(parent.to_string()))?;
    if parent_step.id() != parent {
        return Err(invalid("parent Step ID mismatch"));
    }
    let mut validator = InputValidator {
        objects,
        limits,
        paths: 0,
        bytes: replacement_bytes,
        seen_files: BTreeSet::new(),
        seen_metadata: BTreeSet::new(),
        validated_conflicts: BTreeSet::new(),
    };
    validator.tree(parent_step.root_tree_id(), "", 0)?;
    let mut resolution = Resolution {
        objects,
        choices,
        replacements,
        block_choices,
        proposed_conflicts: BTreeMap::new(),
        files: BTreeMap::new(),
        limits,
        applied: BTreeSet::new(),
        trees: BTreeMap::new(),
        paths: 0,
        bytes: replacement_wire,
    };
    let root = resolution.tree(parent_step.root_tree_id(), "", 0)?;
    if resolution.applied.len() != choices.len() + replacements.len() + block_choices.len() {
        return Err(invalid(
            "choice names a conflict not reached by this resolution",
        ));
    }
    let step = Step::new(vec![parent.clone()], root).map_err(invalid)?;
    let mut conflicts = BTreeMap::new();
    resolution.conflicts(step.root_tree_id(), 0, &mut conflicts)?;
    let mut chunks = BTreeMap::new();
    let mut manifests = BTreeMap::new();
    for (id, bytes) in resolution.files {
        let manifest = if bytes.is_empty() {
            FileManifest::empty(ChunkingDescriptor::single()).map_err(invalid)?
        } else {
            let chunk = Chunk::new(bytes.to_vec()).map_err(invalid)?;
            let manifest = FileManifest::new(
                id,
                bytes.len() as u64,
                ChunkingDescriptor::single(),
                vec![FileChunk::new(chunk.id().clone(), 0, bytes.len() as u64).map_err(invalid)?],
            )
            .map_err(invalid)?;
            chunks.insert(chunk.id().clone(), chunk);
            manifest
        };
        manifests.insert(manifest.id().clone(), manifest);
    }
    let bundle = InstallationBundle {
        step,
        trees: resolution.trees.into_values().collect(),
        conflicts: conflicts.into_values().collect(),
        chunks: chunks.into_values().collect(),
        file_manifests: manifests.into_values().collect(),
    };
    if bundle.canonical_bytes()?.len() > limits.max_output_bytes {
        return Err(GraphMergeError::Limit("serialized output bytes"));
    }
    Ok(bundle)
}

struct Resolution<'a> {
    objects: &'a MergeObjects,
    choices: &'a BTreeMap<ConflictId, ObjectId>,
    replacements: &'a BTreeMap<ConflictId, ConflictReplacement>,
    block_choices: &'a BTreeMap<ConflictId, BTreeMap<ObjectId, ConflictBlockResolution>>,
    proposed_conflicts: BTreeMap<ConflictId, PlannedConflict>,
    files: BTreeMap<FileId, std::borrow::Cow<'a, [u8]>>,
    limits: GraphMergeLimits,
    applied: BTreeSet<ConflictId>,
    trees: BTreeMap<TreeId, Tree>,
    paths: usize,
    bytes: usize,
}
impl Resolution<'_> {
    fn tree(&mut self, id: &TreeId, prefix: &str, depth: usize) -> MergeResult<TreeId> {
        if depth > self.limits.max_depth {
            return Err(GraphMergeError::Limit("depth"));
        }
        let tree = self
            .objects
            .trees
            .get(id)
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        let mut entries = vec![];
        for entry in tree.entries() {
            self.paths += 1;
            if self.paths > self.limits.max_paths {
                return Err(GraphMergeError::Limit("paths"));
            }
            let mut entry = entry.clone();
            let path = joined(prefix, entry.name().as_str());
            if let Some(id) = entry.target().conflict_id() {
                let conflict = self
                    .objects
                    .conflicts
                    .get(id)
                    .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
                if conflict.is_multi_placement()
                    && (self.replacements.contains_key(id) || self.block_choices.contains_key(id))
                {
                    return Err(GraphMergeError::Unsupported("multi-placement conflicts require one complete canonical term; single-file/manual replacement is unavailable".into()));
                }
                if let Some(replacement) = self.replacements.get(id) {
                    self.applied.insert(id.clone());
                    let file = FileId::from_bytes(&replacement.bytes);
                    self.files
                        .insert(file.clone(), std::borrow::Cow::Borrowed(&replacement.bytes));
                    entry = TreeEntry::new(
                        entry.name().clone(),
                        TreeEntryTarget::file(file),
                        replacement.executable,
                    );
                } else if let Some(choices) = self.block_choices.get(id) {
                    let conflict = self
                        .objects
                        .conflicts
                        .get(id)
                        .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
                    let result = semantic_blocks::resolve_blocks(
                        conflict,
                        choices,
                        &self.objects.files,
                        self.limits.max_output_bytes.saturating_sub(self.bytes),
                    )?;
                    self.applied.insert(id.clone());
                    for (id, bytes) in result.files {
                        self.bytes = self
                            .bytes
                            .checked_add(bytes.len())
                            .ok_or(GraphMergeError::Limit("block output"))?;
                        if self.bytes > self.limits.max_output_bytes {
                            return Err(GraphMergeError::Limit("block output"));
                        }
                        self.files.insert(id, std::borrow::Cow::Owned(bytes));
                    }
                    if let Some(conflict) = result.conflict {
                        let id = conflict.id();
                        self.proposed_conflicts.insert(id.clone(), conflict);
                        entry = TreeEntry::new(
                            entry.name().clone(),
                            TreeEntryTarget::conflict(id),
                            false,
                        );
                    } else if let Some((file, mode)) = result.file {
                        entry =
                            TreeEntry::new(entry.name().clone(), TreeEntryTarget::file(file), mode);
                    }
                } else if let Some(chosen) = self.choices.get(id) {
                    let value = self
                        .objects
                        .conflicts
                        .get(id)
                        .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
                    self.applied.insert(id.clone());
                    match value.selected_at(chosen, &path)? {
                        Some(selected) => entry = selected,
                        None => continue,
                    }
                }
            }
            if let Some(id) = entry.target().tree_id() {
                let root = self.tree(id, &path, depth + 1)?;
                entry = TreeEntry::new(entry.name().clone(), TreeEntryTarget::tree(root), false);
            }
            entries.push(entry);
        }
        let tree = Tree::new(entries).map_err(invalid)?;
        let id = tree.id().clone();
        if !self.trees.contains_key(&id) {
            self.bytes = self
                .bytes
                .checked_add(tree.identity_bytes().map_err(invalid)?.len())
                .ok_or(GraphMergeError::Limit("output bytes"))?;
            if self.bytes > self.limits.max_output_bytes {
                return Err(GraphMergeError::Limit("output bytes"));
            }
            self.trees.insert(id.clone(), tree);
        }
        Ok(id)
    }
    fn conflicts(
        &self,
        id: &TreeId,
        depth: usize,
        output: &mut BTreeMap<ConflictId, PlannedConflict>,
    ) -> MergeResult<()> {
        if depth > self.limits.max_depth {
            return Err(GraphMergeError::Limit("depth"));
        }
        let tree = self
            .trees
            .get(id)
            .or_else(|| self.objects.trees.get(id))
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        for entry in tree.entries() {
            if let Some(id) = entry.target().tree_id() {
                self.conflicts(id, depth + 1, output)?;
            }
            if let Some(id) = entry.target().conflict_id() {
                let value = self
                    .proposed_conflicts
                    .get(id)
                    .or_else(|| self.objects.conflicts.get(id))
                    .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
                if output.insert(id.clone(), value.clone()).is_none() {
                    for (_, _, retained) in value
                        .placed_entries("bases")?
                        .into_iter()
                        .chain(value.placed_entries("sides")?)
                    {
                        if let Some(tree) =
                            retained.as_ref().and_then(|entry| entry.target().tree_id())
                        {
                            self.conflicts(tree, depth + 1, output)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
