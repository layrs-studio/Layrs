//! Native immutable-tree planning. All proposed objects are returned in memory;
//! acceptance, authorization, recovery journaling and Worktree writes are external.

use crate::reconciliation::{self, FileMergeOutcome, FileMergePolicy, NativeLens};
use layrs_core::scm::*;
use layrs_graph::scm::StepDag;
use layrs_lens_sdk::{
    LensReconcileLabelsContract, LensReconcileRequest, LensReconcileSideContract,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod canonical;
mod conflict_layout;
pub use conflict_layout::validate_conflict_layout;
mod receipt;
use canonical::{conflict, term};
pub mod renames;
mod replay;
mod resolution;
mod semantic_blocks;
mod structural;
pub use receipt::{ReceiptContext, ReceiptHostAttestation};
pub use replay::plan_replay;
pub use resolution::{
    ConflictReplacement, resolve_conflict_blocks, resolve_conflict_replacements,
    resolve_conflict_terms,
};
pub use semantic_blocks::ConflictBlockResolution;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphMergeError {
    Invalid(String),
    Missing(String),
    Unsupported(String),
    Limit(&'static str),
    NoChanges,
}

impl std::fmt::Display for GraphMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for GraphMergeError {}

type MergeResult<T> = std::result::Result<T, GraphMergeError>;
fn invalid(error: impl std::fmt::Display) -> GraphMergeError {
    GraphMergeError::Invalid(error.to_string())
}

/// Verified immutable input closure, independent of any checkout or SQL order.
#[derive(Debug, Default, Clone)]
pub struct MergeObjects {
    pub steps: BTreeMap<StepId, Step>,
    pub trees: BTreeMap<TreeId, Tree>,
    pub files: BTreeMap<FileId, Vec<u8>>,
    pub symlinks: BTreeMap<SymlinkId, Symlink>,
    pub conflicts: BTreeMap<ConflictId, PlannedConflict>,
    pub tombstones: BTreeMap<TombstoneId, Tombstone>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphMergeLimits {
    pub max_steps: usize,
    pub max_paths: usize,
    pub max_depth: usize,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
}

impl Default for GraphMergeLimits {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            max_paths: 100_000,
            max_depth: 64,
            max_input_bytes: 64 * 1024 * 1024,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

/// A schema-conforming structural or partially resolved text ConflictObject.
/// Construction is private; semantic payloads require contextual validation too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PlannedConflict(Value);

impl PlannedConflict {
    pub fn id(&self) -> ConflictId {
        ConflictId::parse(self.0["conflictId"].as_str().expect("constructed ID")).expect("valid ID")
    }
    pub fn canonical_bytes(&self) -> MergeResult<Vec<u8>> {
        canonical_json(&self.0).map_err(invalid)
    }
    pub fn document(&self) -> &Value {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct GraphMergePlan {
    pub(crate) source_heads: Vec<StepId>,
    pub(crate) target_heads: Vec<StepId>,
    pub(crate) merge_bases: Vec<StepId>,
    pub(crate) step: Step,
    pub(crate) trees: BTreeMap<TreeId, Tree>,
    pub(crate) files: BTreeMap<FileId, Vec<u8>>,
    pub(crate) conflicts: BTreeMap<ConflictId, PlannedConflict>,
    pub(crate) executions: Vec<PendingExecution>,
    pub(crate) file_policy: FileMergePolicy,
    pub(crate) limits: GraphMergeLimits,
    pub(crate) semantic_execution: bool,
    pub(crate) enforced_host: Option<Value>,
}

impl GraphMergePlan {
    pub fn limits(&self) -> GraphMergeLimits {
        self.limits
    }
    /// Canonical objects to install alongside the pre-existing immutable input
    /// closure. This transport-neutral bundle is not an acceptance transaction.
    pub fn installation_bundle(&self) -> MergeResult<InstallationBundle> {
        let mut chunks = vec![];
        let mut manifests = vec![];
        for (id, bytes) in &self.files {
            if bytes.is_empty() {
                manifests.push(FileManifest::empty(ChunkingDescriptor::single()).map_err(invalid)?);
            } else {
                let chunk = Chunk::new(bytes.clone()).map_err(invalid)?;
                let manifest = FileManifest::new(
                    id.clone(),
                    bytes.len() as u64,
                    ChunkingDescriptor::single(),
                    vec![
                        FileChunk::new(chunk.id().clone(), 0, bytes.len() as u64)
                            .map_err(invalid)?,
                    ],
                )
                .map_err(invalid)?;
                chunks.push(chunk);
                manifests.push(manifest);
            }
        }
        chunks.sort_by(|a, b| a.id().cmp(b.id()));
        manifests.sort_by(|a, b| a.id().cmp(b.id()));
        Ok(InstallationBundle {
            step: self.step.clone(),
            trees: self.trees.values().cloned().collect(),
            conflicts: self.conflicts.values().cloned().collect(),
            chunks,
            file_manifests: manifests,
        })
    }
    pub fn step(&self) -> &Step {
        &self.step
    }
    pub fn source_heads(&self) -> &[StepId] {
        &self.source_heads
    }
    pub fn target_heads(&self) -> &[StepId] {
        &self.target_heads
    }
    pub fn merge_bases(&self) -> &[StepId] {
        &self.merge_bases
    }
    pub fn trees(&self) -> &BTreeMap<TreeId, Tree> {
        &self.trees
    }
    pub fn files(&self) -> &BTreeMap<FileId, Vec<u8>> {
        &self.files
    }
    pub fn conflicts(&self) -> &BTreeMap<ConflictId, PlannedConflict> {
        &self.conflicts
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationBundle {
    pub step: Step,
    pub trees: Vec<Tree>,
    pub conflicts: Vec<PlannedConflict>,
    pub chunks: Vec<Chunk>,
    pub file_manifests: Vec<FileManifest>,
}

impl InstallationBundle {
    pub fn canonical_bytes(&self) -> MergeResult<Vec<u8>> {
        canonical_json(self).map_err(invalid)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PendingExecution {
    path: RepoPath,
    pin: layrs_lens_sdk::LensPin,
    term_roles: Vec<layrs_sync::v3::TermRole>,
    conflict: Option<ConflictId>,
}

pub fn plan_graph_merge(
    objects: &MergeObjects,
    source_heads: &[StepId],
    target_heads: &[StepId],
    file_policy: FileMergePolicy,
    limits: GraphMergeLimits,
) -> MergeResult<GraphMergePlan> {
    plan_graph_merge_mode(
        objects,
        source_heads,
        target_heads,
        file_policy,
        limits,
        true,
        None,
    )
}

/// Explicit structural-only policy: no Lens is invoked, including for text.
pub fn plan_structural_merge(
    objects: &MergeObjects,
    source_heads: &[StepId],
    target_heads: &[StepId],
    file_policy: FileMergePolicy,
    limits: GraphMergeLimits,
) -> MergeResult<GraphMergePlan> {
    plan_graph_merge_mode(
        objects,
        source_heads,
        target_heads,
        file_policy,
        limits,
        false,
        None,
    )
}

/// Every semantic call is performed by the same capability-constrained host.
/// The private recorded profile is added only after all calls succeeded.
#[cfg(not(target_arch = "wasm32"))]
pub fn plan_sandboxed_merge(
    objects: &MergeObjects,
    source_heads: &[StepId],
    target_heads: &[StepId],
    file_policy: FileMergePolicy,
    limits: GraphMergeLimits,
    host: &crate::wasm_host::WasmHost,
) -> MergeResult<GraphMergePlan> {
    let mut plan = plan_graph_merge_mode(
        objects,
        source_heads,
        target_heads,
        file_policy,
        limits,
        true,
        Some(host),
    )?;
    plan.enforced_host = Some(serde_json::to_value(host.profile()).map_err(invalid)?);
    Ok(plan)
}

trait FileExecutor {
    fn pin(&self, lens: NativeLens) -> layrs_lens_sdk::LensPin;
    fn reconcile(
        &self,
        request: &LensReconcileRequest,
        policy: FileMergePolicy,
    ) -> MergeResult<reconciliation::FileMergePlan>;
}
#[cfg(not(target_arch = "wasm32"))]
impl FileExecutor for crate::wasm_host::WasmHost {
    fn pin(&self, lens: NativeLens) -> layrs_lens_sdk::LensPin {
        crate::wasm_host::manifest(lens).pin().unwrap()
    }
    fn reconcile(
        &self,
        request: &LensReconcileRequest,
        policy: FileMergePolicy,
    ) -> MergeResult<reconciliation::FileMergePlan> {
        crate::wasm_host::WasmHost::reconcile(self, request, policy).map_err(invalid)
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_graph_merge_mode(
    objects: &MergeObjects,
    source_heads: &[StepId],
    target_heads: &[StepId],
    file_policy: FileMergePolicy,
    limits: GraphMergeLimits,
    semantic_execution: bool,
    executor: Option<&dyn FileExecutor>,
) -> MergeResult<GraphMergePlan> {
    if objects.steps.len() > limits.max_steps {
        return Err(GraphMergeError::Limit("steps"));
    }
    let sources = checked_heads(source_heads)?;
    let targets = checked_heads(target_heads)?;
    let heads: Vec<_> = sources
        .iter()
        .chain(&targets)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if heads.len() > 64 {
        return Err(GraphMergeError::Limit("conflict sides"));
    }
    let mut step_bytes = 0usize;
    for (id, step) in &objects.steps {
        if id != step.id() {
            return Err(invalid("Step map key mismatch"));
        }
        step_bytes = step_bytes
            .checked_add(step.identity_bytes().len())
            .ok_or(GraphMergeError::Limit("input bytes"))?;
        if step_bytes > limits.max_input_bytes {
            return Err(GraphMergeError::Limit("input bytes"));
        }
    }
    let dag = StepDag::from_steps(objects.steps.values()).map_err(invalid)?;
    for head in &heads {
        if !dag.contains(head) {
            return Err(GraphMergeError::Missing(head.to_string()));
        }
    }
    for set in [&sources, &targets] {
        for (index, a) in set.iter().enumerate() {
            for b in set.iter().skip(index + 1) {
                if dag.is_ancestor(a, b).map_err(invalid)?
                    || dag.is_ancestor(b, a).map_err(invalid)?
                {
                    return Err(invalid("each input head set must be an antichain"));
                }
            }
        }
    }
    let mut contributes = false;
    for source in &sources {
        let mut present = false;
        for target in &targets {
            present |= dag.is_ancestor_or_equal(source, target).map_err(invalid)?;
        }
        contributes |= !present;
    }
    if !contributes && targets.len() == 1 {
        return Err(GraphMergeError::NoChanges);
    }
    let bases = dag.merge_bases(&heads).map_err(invalid)?;
    if bases.len() > 64 {
        return Err(GraphMergeError::Limit("conflict bases"));
    }
    let mut validator = InputValidator {
        objects,
        limits,
        paths: 0,
        bytes: step_bytes,
        seen_files: BTreeSet::new(),
        seen_metadata: BTreeSet::new(),
        validated_conflicts: BTreeSet::new(),
    };
    // Validate the complete selected closure before planning, including objects
    // a trivial rule would otherwise reuse without examining their contents.
    for step in heads.iter().chain(&bases) {
        validator.tree(objects.steps[step].root_tree_id(), "", 0)?;
    }
    let empty = Tree::new(vec![]).map_err(invalid)?;
    let placeholder = Step::new(heads.clone(), empty.id().clone()).map_err(invalid)?;
    let mut planner = Planner {
        objects,
        executor,
        plan: GraphMergePlan {
            source_heads: sources,
            target_heads: targets,
            merge_bases: bases.clone(),
            step: placeholder,
            trees: BTreeMap::new(),
            files: BTreeMap::new(),
            conflicts: BTreeMap::new(),
            executions: vec![],
            file_policy,
            limits,
            semantic_execution,
            enforced_host: None,
        },
        paths: 0,
        output_bytes: 0,
        retained_trees: BTreeSet::new(),
        rename_overrides: BTreeMap::new(),
    };
    let roots = |ids: &[StepId]| {
        ids.iter()
            .map(|id| (id.clone(), Some(objects.steps[id].root_tree_id().clone())))
            .collect::<Vec<_>>()
    };
    planner.install_renames(&bases, &heads)?;
    let root = planner.tree("", &roots(&bases), &roots(&heads), 0)?;
    planner.plan.step = Step::new(heads, root).map_err(invalid)?;
    let root = planner.plan.step.root_tree_id().clone();
    planner.retain_conflicts(&root, 0)?;
    planner.charge(planner.plan.step.identity_bytes().len())?;
    if planner.plan.installation_bundle()?.canonical_bytes()?.len() > limits.max_output_bytes {
        return Err(GraphMergeError::Limit("serialized output bytes"));
    }
    Ok(planner.plan)
}

fn checked_heads(input: &[StepId]) -> MergeResult<Vec<StepId>> {
    if input.is_empty() || input.len() > 64 {
        return Err(invalid("heads require 1..=64 entries"));
    }
    let set: BTreeSet<_> = input.iter().cloned().collect();
    if set.len() != input.len() {
        return Err(invalid("duplicate input head"));
    }
    Ok(set.into_iter().collect())
}

struct InputValidator<'a> {
    objects: &'a MergeObjects,
    limits: GraphMergeLimits,
    paths: usize,
    bytes: usize,
    seen_files: BTreeSet<FileId>,
    seen_metadata: BTreeSet<String>,
    validated_conflicts: BTreeSet<ConflictId>,
}
impl InputValidator<'_> {
    fn metadata(&mut self, id: &str, byte_length: usize) -> MergeResult<()> {
        if self.seen_metadata.insert(id.to_string()) {
            self.bytes = self
                .bytes
                .checked_add(byte_length)
                .ok_or(GraphMergeError::Limit("input bytes"))?;
            if self.bytes > self.limits.max_input_bytes {
                return Err(GraphMergeError::Limit("input bytes"));
            }
        }
        Ok(())
    }
    fn tree(&mut self, id: &TreeId, prefix: &str, depth: usize) -> MergeResult<()> {
        if depth > self.limits.max_depth {
            return Err(GraphMergeError::Limit("depth"));
        }
        let tree = self
            .objects
            .trees
            .get(id)
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        if tree.id() != id {
            return Err(invalid("Tree map key mismatch"));
        }
        if tree.entries().len() > self.limits.max_paths {
            return Err(GraphMergeError::Limit("paths"));
        }
        self.metadata(id.as_str(), tree.identity_bytes().map_err(invalid)?.len())?;
        for entry in tree.entries() {
            self.paths = self
                .paths
                .checked_add(1)
                .ok_or(GraphMergeError::Limit("paths"))?;
            if self.paths > self.limits.max_paths {
                return Err(GraphMergeError::Limit("paths"));
            }
            let path = joined(prefix, entry.name().as_str());
            RepoPath::parse(path.as_str()).map_err(invalid)?;
            if entry.executable() && !matches!(entry.target(), TreeEntryTarget::File { .. }) {
                return Err(invalid("non-file executable bit"));
            }
            match entry.target() {
                TreeEntryTarget::File { file_id } => {
                    let bytes = self
                        .objects
                        .files
                        .get(file_id)
                        .ok_or_else(|| GraphMergeError::Missing(file_id.to_string()))?;
                    if self.seen_files.insert(file_id.clone()) {
                        self.bytes = self
                            .bytes
                            .checked_add(bytes.len())
                            .ok_or(GraphMergeError::Limit("input bytes"))?;
                        if self.bytes > self.limits.max_input_bytes {
                            return Err(GraphMergeError::Limit("input bytes"));
                        }
                        if &FileId::from_bytes(bytes) != file_id {
                            return Err(invalid("File content ID mismatch"));
                        }
                    }
                }
                TreeEntryTarget::Tree { tree_id } => self.tree(tree_id, &path, depth + 1)?,
                TreeEntryTarget::Symlink { symlink_id } => {
                    let value = self
                        .objects
                        .symlinks
                        .get(symlink_id)
                        .ok_or_else(|| GraphMergeError::Missing(symlink_id.to_string()))?;
                    if value.id() != symlink_id {
                        return Err(invalid("Symlink map key mismatch"));
                    }
                    self.metadata(
                        symlink_id.as_str(),
                        value.identity_bytes().map_err(invalid)?.len(),
                    )?;
                }
                TreeEntryTarget::Conflict { conflict_id } => {
                    let conflict = self
                        .objects
                        .conflicts
                        .get(conflict_id)
                        .ok_or_else(|| GraphMergeError::Missing(conflict_id.to_string()))?;
                    if conflict.id() != *conflict_id
                        || !conflict
                            .anchor_paths()?
                            .contains(&RepoPath::parse(&path).map_err(invalid)?)
                    {
                        return Err(invalid("conflict map key/placement mismatch"));
                    }
                    if self.validated_conflicts.contains(conflict_id) {
                        continue;
                    }
                    self.metadata(conflict_id.as_str(), conflict.canonical_bytes()?.len())?;
                    for file in conflict.payload_file_ids()? {
                        self.retained_entry(
                            &TreeEntry::new(
                                entry.name().clone(),
                                TreeEntryTarget::file(file),
                                false,
                            ),
                            &path,
                            depth + 1,
                        )?;
                    }
                    conflict.validate_payload(&self.objects.files)?;
                    for (origin, retained_path, retained) in conflict
                        .placed_entries("bases")?
                        .into_iter()
                        .chain(conflict.placed_entries("sides")?)
                    {
                        if !self.objects.steps.contains_key(&origin) {
                            return Err(GraphMergeError::Missing(origin.to_string()));
                        }
                        if let Some(retained) = retained {
                            self.retained_entry(&retained, retained_path.as_str(), depth + 1)?;
                        }
                    }
                    conflict.validate_rename_context(self.objects, self.limits)?;
                    self.validated_conflicts.insert(conflict_id.clone());
                }
                TreeEntryTarget::Tombstone { tombstone_id } => {
                    let tombstone = self
                        .objects
                        .tombstones
                        .get(tombstone_id)
                        .ok_or_else(|| GraphMergeError::Missing(tombstone_id.to_string()))?;
                    if tombstone.id() != tombstone_id {
                        return Err(invalid("Tombstone map key mismatch"));
                    }
                    self.metadata(
                        tombstone_id.as_str(),
                        tombstone.identity_bytes().map_err(invalid)?.len(),
                    )?;
                }
            }
        }
        if depth == 0 {
            conflict_layout::validate_occurrences(self.objects, id, self.limits)?;
        }
        Ok(())
    }

    fn retained_entry(&mut self, entry: &TreeEntry, path: &str, depth: usize) -> MergeResult<()> {
        self.paths += 1;
        if self.paths > self.limits.max_paths {
            return Err(GraphMergeError::Limit("paths"));
        }
        if depth > self.limits.max_depth {
            return Err(GraphMergeError::Limit("depth"));
        }
        match entry.target() {
            TreeEntryTarget::Tree { tree_id } => self.tree(tree_id, path, depth)?,
            TreeEntryTarget::File { file_id } => {
                let bytes = self
                    .objects
                    .files
                    .get(file_id)
                    .ok_or_else(|| GraphMergeError::Missing(file_id.to_string()))?;
                if self.seen_files.insert(file_id.clone()) {
                    self.bytes = self
                        .bytes
                        .checked_add(bytes.len())
                        .ok_or(GraphMergeError::Limit("input bytes"))?;
                    if self.bytes > self.limits.max_input_bytes {
                        return Err(GraphMergeError::Limit("input bytes"));
                    }
                    if &FileId::from_bytes(bytes) != file_id {
                        return Err(invalid("retained File ID mismatch"));
                    }
                }
            }
            TreeEntryTarget::Symlink { symlink_id } => {
                if self
                    .objects
                    .symlinks
                    .get(symlink_id)
                    .is_none_or(|s| s.id() != symlink_id)
                {
                    return Err(GraphMergeError::Missing(symlink_id.to_string()));
                }
            }
            _ => return Err(invalid("invalid retained conflict value")),
        }
        Ok(())
    }
}

type TermEntry = (StepId, Option<TreeEntry>);
type TermTree = (StepId, Option<TreeId>);
struct Planner<'a> {
    objects: &'a MergeObjects,
    executor: Option<&'a dyn FileExecutor>,
    plan: GraphMergePlan,
    paths: usize,
    output_bytes: usize,
    retained_trees: BTreeSet<TreeId>,
    rename_overrides: BTreeMap<String, ConflictId>,
}
impl Planner<'_> {
    fn charge(&mut self, bytes: usize) -> MergeResult<()> {
        self.output_bytes = self
            .output_bytes
            .checked_add(bytes)
            .ok_or(GraphMergeError::Limit("output bytes"))?;
        if self.output_bytes > self.plan.limits.max_output_bytes {
            return Err(GraphMergeError::Limit("output bytes"));
        }
        Ok(())
    }
    fn tree(
        &mut self,
        prefix: &str,
        bases: &[TermTree],
        sides: &[TermTree],
        depth: usize,
    ) -> MergeResult<TreeId> {
        if depth > self.plan.limits.max_depth {
            return Err(GraphMergeError::Limit("depth"));
        }
        let mut names = BTreeSet::new();
        for (_, id) in bases.iter().chain(sides) {
            if let Some(id) = id {
                for entry in self.objects.trees[id].entries() {
                    names.insert(entry.name().clone());
                }
            }
        }
        let mut entries = vec![];
        for name in names {
            self.paths += 1;
            if self.paths > self.plan.limits.max_paths {
                return Err(GraphMergeError::Limit("paths"));
            }
            let terms = |trees: &[TermTree]| {
                trees
                    .iter()
                    .map(|(step, tree)| {
                        (
                            step.clone(),
                            tree.as_ref().and_then(|id| {
                                self.objects.trees[id]
                                    .entries()
                                    .iter()
                                    .find(|entry| entry.name() == &name)
                                    .filter(|entry| {
                                        !matches!(entry.target(), TreeEntryTarget::Tombstone { .. })
                                    })
                                    .cloned()
                            }),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let b = terms(bases);
            let s = terms(sides);
            if let Some(entry) = self.entry(&joined(prefix, name.as_str()), &name, &b, &s, depth)? {
                entries.push(entry);
            }
        }
        let tree = Tree::new(entries).map_err(invalid)?;
        let id = tree.id().clone();
        if !self.plan.trees.contains_key(&id) {
            self.charge(tree.identity_bytes().map_err(invalid)?.len())?;
            self.plan.trees.insert(id.clone(), tree);
        }
        Ok(id)
    }
    fn entry(
        &mut self,
        path: &str,
        name: &RepoPathComponent,
        bases: &[TermEntry],
        sides: &[TermEntry],
        depth: usize,
    ) -> MergeResult<Option<TreeEntry>> {
        if let Some(id) = self.rename_overrides.get(path) {
            return Ok(Some(TreeEntry::new(
                name.clone(),
                TreeEntryTarget::conflict(id.clone()),
                false,
            )));
        }
        if bases.iter().chain(sides).any(|(_, entry)| {
            entry
                .as_ref()
                .is_some_and(|e| e.target().conflict_id().is_some())
        }) {
            return self.expanded_conflict(path, name, bases, sides);
        }
        if sides.iter().all(|(_, entry)| entry == &sides[0].1) {
            return Ok(sides[0].1.clone());
        }
        if bases.len() <= 1 {
            let base = bases.first().and_then(|(_, entry)| entry.as_ref());
            let changed: Vec<_> = sides
                .iter()
                .filter(|(_, entry)| entry.as_ref() != base)
                .collect();
            if changed.iter().all(|(_, entry)| entry == &changed[0].1) {
                return Ok(changed[0].1.clone());
            }
        }
        if sides.iter().all(|(_, e)| {
            e.as_ref()
                .is_some_and(|e| matches!(e.target(), TreeEntryTarget::Tree { .. }))
        }) && bases.iter().all(|(_, e)| {
            e.as_ref()
                .is_none_or(|e| matches!(e.target(), TreeEntryTarget::Tree { .. }))
        }) {
            let roots = |terms: &[TermEntry]| {
                terms
                    .iter()
                    .map(|(step, entry)| {
                        (
                            step.clone(),
                            entry.as_ref().and_then(|e| e.target().tree_id().cloned()),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let id = self.tree(path, &roots(bases), &roots(sides), depth + 1)?;
            return Ok(Some(TreeEntry::new(
                name.clone(),
                TreeEntryTarget::tree(id),
                false,
            )));
        }
        let is_file =
            |e: &Option<TreeEntry>| e.as_ref().is_some_and(|e| e.target().file_id().is_some());
        let mut execution = None;
        let mut semantic_conflict = None;
        if self.plan.semantic_execution
            && bases.len() == 1
            && sides.len() == 2
            && bases.iter().chain(sides).all(|(_, e)| is_file(e))
        {
            let base_mode = bases[0].1.as_ref().unwrap().executable();
            let mode_a = sides[0].1.as_ref().unwrap().executable();
            let mode_b = sides[1].1.as_ref().unwrap().executable();
            let mode = if mode_a == mode_b || mode_b == base_mode {
                mode_a
            } else {
                mode_b
            };
            let ext = path.rsplit('.').next().unwrap_or("");
            let lens = if layrs_lens_code::code_extensions().contains(&ext) {
                Some(NativeLens::Code)
            } else if ["txt", "md", "markdown", "rst", "log"].contains(&ext) {
                Some(NativeLens::Text)
            } else {
                None
            };
            if let Some(lens) = lens {
                let pin = self
                    .executor
                    .map(|executor| executor.pin(lens))
                    .unwrap_or_else(|| {
                        reconciliation::native_manifest(lens)
                            .pin()
                            .expect("native pin")
                    });
                let side = |e: &Option<TreeEntry>| {
                    LensReconcileSideContract::present(
                        self.objects.files[e.as_ref().unwrap().target().file_id().unwrap()].clone(),
                        None,
                    )
                };
                let request = LensReconcileRequest::new(
                    pin.clone(),
                    path,
                    None,
                    side(&bases[0].1),
                    side(&sides[0].1),
                    side(&sides[1].1),
                    LensReconcileLabelsContract {
                        existing: sides[0].0.to_string(),
                        incoming: sides[1].0.to_string(),
                    },
                );
                let result = match self.executor {
                    Some(executor) => executor.reconcile(&request, self.plan.file_policy)?,
                    None => reconciliation::reconcile_file(&request, self.plan.file_policy)
                        .map_err(|e| invalid(format!("{e:?}")))?,
                };
                execution = Some(pin);
                if let FileMergeOutcome::Semantic(result) = result.outcome {
                    if let Some(ref resolved) = result.resolved {
                        let file_id = FileId::from_bytes(&resolved.bytes);
                        if !self.plan.files.contains_key(&file_id) {
                            self.charge(resolved.bytes.len())?;
                            self.plan
                                .files
                                .insert(file_id.clone(), resolved.bytes.clone());
                        }
                        self.record_execution(path, bases, sides, execution.unwrap(), None)?;
                        return Ok(Some(TreeEntry::new(
                            name.clone(),
                            TreeEntryTarget::file(file_id),
                            mode,
                        )));
                    }
                    semantic_conflict = Some(result);
                }
            }
        }
        let mut conflict = conflict(path, bases, sides, execution.as_ref())?;
        if let Some(result) = semantic_conflict {
            let p = placement(path)?;
            let terms = [
                term("base", &bases[0].0, &bases[0].1, &p)?,
                term("side", &sides[0].0, &sides[0].1, &p)?,
                term("side", &sides[1].0, &sides[1].1, &p)?,
            ];
            let ordered = terms.each_ref().map(|t| t["termId"].as_str().unwrap());
            let (upgraded, files) = semantic_blocks::upgrade(
                conflict,
                &result,
                ordered,
                &self.objects.files,
                self.plan.limits.max_output_bytes,
            )?;
            conflict = upgraded;
            for (id, bytes) in files {
                if self.plan.files.contains_key(&id) {
                    continue;
                }
                self.charge(bytes.len())?;
                self.plan.files.insert(id, bytes);
            }
        }
        let id = conflict.id();
        self.charge(conflict.canonical_bytes()?.len())?;
        self.plan.conflicts.insert(id.clone(), conflict);
        if let Some(pin) = execution {
            self.record_execution(path, bases, sides, pin, Some(id.clone()))?;
        }
        Ok(Some(TreeEntry::new(
            name.clone(),
            TreeEntryTarget::conflict(id),
            false,
        )))
    }
    fn record_execution(
        &mut self,
        path: &str,
        bases: &[TermEntry],
        sides: &[TermEntry],
        pin: layrs_lens_sdk::LensPin,
        conflict: Option<ConflictId>,
    ) -> MergeResult<()> {
        let placement = placement(path)?;
        let mut roles = vec![];
        for (kind, entries) in [("base", bases), ("side", sides)] {
            for (index, (step, entry)) in entries.iter().enumerate() {
                let term = term(kind, step, entry, &placement)?;
                roles.push(layrs_sync::v3::TermRole {
                    term_id: ObjectId::parse(term["termId"].as_str().unwrap()).map_err(invalid)?,
                    role: if kind == "base" {
                        layrs_sync::v3::TermRoleKind::Base
                    } else if index == 0 {
                        layrs_sync::v3::TermRoleKind::Existing
                    } else {
                        layrs_sync::v3::TermRoleKind::Incoming
                    },
                });
            }
        }
        roles.sort_by(|a, b| (&a.term_id, a.role).cmp(&(&b.term_id, b.role)));
        self.plan.executions.push(PendingExecution {
            path: RepoPath::parse(path).map_err(invalid)?,
            pin,
            term_roles: roles,
            conflict,
        });
        Ok(())
    }
}

fn joined(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}
fn digest(domain: &'static str, value: &Value) -> MergeResult<ObjectId> {
    let payload = canonical_json(value).map_err(invalid)?;
    ObjectId::parse(format!(
        "blake3:{}",
        blake3::hash(&domain_separated_bytes(domain, &payload))
    ))
    .map_err(invalid)
}
fn placement(path: &str) -> MergeResult<Value> {
    let path = RepoPath::parse(path).map_err(invalid)?;
    let mut value = json!({"path": path, "role": "primary"});
    value["placementId"] = json!(digest("layrs-conflict-placement-v1", &value)?);
    Ok(value)
}
#[cfg(test)]
mod tests;
