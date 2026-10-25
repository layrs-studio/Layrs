//! Native merge previews and proposed CAS objects. No checkout writes, head
//! moves or fictitious accepted Weave receipts occur during preparation.
use super::{NativeClientError, NativeWorkspace, Result, WorkspaceState};
use layrs_core::scm::*;
use layrs_store::scm::*;
#[cfg(test)]
use layrs_store_local::scm_v1::open;
use layrs_store_local::scm_v1::NativeScmReader;
use layrs_weave::graph_merge::{
    self, GraphMergeLimits, GraphMergePlan, InstallationBundle, MergeObjects, PlannedConflict,
};
use layrs_weave::reconciliation::FileMergePolicy;
use std::collections::BTreeMap;
#[path = "merge_accept.rs"]
mod accept;
pub use accept::NativeAcceptedMerge;
#[path = "merge_resolution.rs"]
mod resolution;
pub use resolution::NativeAcceptedResolution;
#[path = "merge_blocks.rs"]
mod blocks;
pub use blocks::BlockResolutionChoice;

pub struct NativeMergePreview {
    state: WorkspaceState,
    target_layer: LayerId,
    plan: GraphMergePlan,
    stamp: super::MutationStamp,
}
impl NativeMergePreview {
    pub fn expected_state(&self) -> &WorkspaceState {
        &self.state
    }
    pub fn target_layer(&self) -> &LayerId {
        &self.target_layer
    }
    pub fn plan(&self) -> &GraphMergePlan {
        &self.plan
    }
    pub fn canonical_objects(&self) -> Result<Vec<CanonicalObject>> {
        bundle_objects(self.plan.installation_bundle().map_err(merge_error)?)
    }
}

pub struct NativeResolutionPreview {
    state: WorkspaceState,
    target_layer: LayerId,
    bundle: InstallationBundle,
    limits: GraphMergeLimits,
    stamp: super::MutationStamp,
}
impl NativeResolutionPreview {
    pub fn expected_state(&self) -> &WorkspaceState {
        &self.state
    }
    pub fn target_layer(&self) -> &LayerId {
        &self.target_layer
    }
    pub fn bundle(&self) -> &InstallationBundle {
        &self.bundle
    }
    pub fn canonical_objects(&self) -> Result<Vec<CanonicalObject>> {
        bundle_objects(self.bundle.clone())
    }
}

fn merge_error(error: impl std::fmt::Display) -> NativeClientError {
    NativeClientError::InvalidSnapshot(format!("native reconciliation: {error}"))
}

impl NativeWorkspace {
    /// Preview explicit text-block resolutions, retaining all unresolved blocks
    /// and original terms in a new immutable Conflict. Acceptance uses the same
    /// exact state/checkout guards as whole-file conflict resolution.
    pub fn preview_conflict_blocks(
        &self,
        target_layer: &LayerId,
        choices: &BTreeMap<ConflictId, BTreeMap<ObjectId, graph_merge::ConflictBlockResolution>>,
        limits: GraphMergeLimits,
    ) -> Result<NativeResolutionPreview> {
        let count = choices
            .values()
            .try_fold(0usize, |n, c| n.checked_add(c.len()))
            .ok_or(NativeClientError::InvalidState("block count overflow"))?;
        let bytes = choices
            .values()
            .flat_map(|c| c.values())
            .try_fold(0usize, |n, c| {
                n.checked_add(match c {
                    graph_merge::ConflictBlockResolution::Manual { bytes, .. } => bytes.len(),
                    _ => 0,
                })
            })
            .ok_or(NativeClientError::InvalidState("block bytes overflow"))?;
        if count > limits.max_paths
            || bytes > limits.max_input_bytes
            || bytes > limits.max_output_bytes
        {
            return Err(NativeClientError::InvalidState(
                "block resolution budget exceeded",
            ));
        }
        let state = self.state()?;
        let heads = state
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == target_layer)
            .ok_or(NativeClientError::Missing("target Layer heads"))?
            .step_ids();
        let [parent] = heads else {
            return Err(NativeClientError::NeedsIntegration);
        };
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let objects = load_objects(&reader, state.space.id(), vec![parent.clone()], limits)?;
        let bundle = graph_merge::resolve_conflict_blocks(&objects, parent, choices, limits)
            .map_err(merge_error)?;
        Ok(NativeResolutionPreview {
            state,
            target_layer: target_layer.clone(),
            bundle,
            limits,
            stamp: super::MutationStamp::fresh()?,
        })
    }

    /// Explicit replacement bytes are a new user edit. The prior conflicted
    /// Step/receipt remains immutable and reachable through the descendant.
    pub fn preview_conflict_replacements(
        &self,
        target_layer: &LayerId,
        replacements: &BTreeMap<ConflictId, graph_merge::ConflictReplacement>,
        limits: GraphMergeLimits,
    ) -> Result<NativeResolutionPreview> {
        let bytes = replacements
            .values()
            .try_fold(0usize, |total, value| total.checked_add(value.bytes.len()))
            .ok_or(NativeClientError::InvalidState(
                "replacement bytes overflow",
            ))?;
        if replacements.len() > limits.max_paths
            || bytes > limits.max_input_bytes
            || bytes > limits.max_output_bytes
        {
            return Err(NativeClientError::InvalidState(
                "replacement budget exceeded",
            ));
        }
        let state = self.state()?;
        let heads = state
            .view
            .layer_heads()
            .iter()
            .find(|heads| heads.layer_id() == target_layer)
            .ok_or(NativeClientError::Missing("target Layer heads"))?
            .step_ids();
        let [parent] = heads else {
            return Err(NativeClientError::NeedsIntegration);
        };
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let objects = load_objects(&reader, state.space.id(), vec![parent.clone()], limits)?;
        let bundle =
            graph_merge::resolve_conflict_replacements(&objects, parent, replacements, limits)
                .map_err(merge_error)?;
        Ok(NativeResolutionPreview {
            state,
            target_layer: target_layer.clone(),
            bundle,
            limits,
            stamp: super::MutationStamp::fresh()?,
        })
    }

    /// Exact built-in module executed with enforced process and WASM limits.
    pub fn preview_sandboxed_merge(
        &self,
        source_heads: &[StepId],
        target_layer: &LayerId,
        file_policy: FileMergePolicy,
        limits: GraphMergeLimits,
        host: &layrs_weave::wasm_host::WasmHost,
    ) -> Result<NativeMergePreview> {
        let state = self.state()?;
        let targets = state
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == target_layer)
            .ok_or(NativeClientError::Missing("target Layer heads"))?
            .step_ids();
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let objects = load_objects(
            &reader,
            state.space.id(),
            source_heads.iter().chain(targets).cloned().collect(),
            limits,
        )?;
        let plan = graph_merge::plan_sandboxed_merge(
            &objects,
            source_heads,
            targets,
            file_policy,
            limits,
            host,
        )
        .map_err(merge_error)?;
        Ok(NativeMergePreview {
            state,
            target_layer: target_layer.clone(),
            plan,
            stamp: super::MutationStamp::fresh()?,
        })
    }

    /// Source selectors are resolved before this boundary. Read-only preview is
    /// pinned to the returned exact View/Operation and explicit Layer head sets.
    pub fn preview_merge(
        &self,
        source_heads: &[StepId],
        target_layer: &LayerId,
        file_policy: FileMergePolicy,
        limits: GraphMergeLimits,
    ) -> Result<NativeMergePreview> {
        self.preview_merge_mode(source_heads, target_layer, file_policy, limits, false)
    }

    /// Explicitly disables semantic Lens execution. Structural conflicts remain
    /// successful immutable merge results, suitable for the zero-Lens host.
    pub fn preview_structural_merge(
        &self,
        source_heads: &[StepId],
        target_layer: &LayerId,
        file_policy: FileMergePolicy,
        limits: GraphMergeLimits,
    ) -> Result<NativeMergePreview> {
        self.preview_merge_mode(source_heads, target_layer, file_policy, limits, true)
    }

    fn preview_merge_mode(
        &self,
        source_heads: &[StepId],
        target_layer: &LayerId,
        file_policy: FileMergePolicy,
        limits: GraphMergeLimits,
        structural_only: bool,
    ) -> Result<NativeMergePreview> {
        let state = self.state()?;
        let targets = state
            .view
            .layer_heads()
            .iter()
            .find(|heads| heads.layer_id() == target_layer)
            .ok_or(NativeClientError::Missing("target Layer heads"))?
            .step_ids();
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let objects = load_objects(
            &reader,
            state.space.id(),
            source_heads.iter().chain(targets).cloned().collect(),
            limits,
        )?;
        let plan = if structural_only {
            graph_merge::plan_structural_merge(&objects, source_heads, targets, file_policy, limits)
        } else {
            graph_merge::plan_graph_merge(&objects, source_heads, targets, file_policy, limits)
        }
        .map_err(merge_error)?;
        Ok(NativeMergePreview {
            state,
            target_layer: target_layer.clone(),
            plan,
            stamp: super::MutationStamp::fresh()?,
        })
    }

    /// Whole-term resolution creates a new Step; callers may choose only some
    /// conflicts and retain the rest. Acceptance/materialization remain separate.
    pub fn preview_conflict_resolution(
        &self,
        target_layer: &LayerId,
        choices: &BTreeMap<ConflictId, ObjectId>,
        limits: GraphMergeLimits,
    ) -> Result<NativeResolutionPreview> {
        let state = self.state()?;
        let heads = state
            .view
            .layer_heads()
            .iter()
            .find(|heads| heads.layer_id() == target_layer)
            .ok_or(NativeClientError::Missing("target Layer heads"))?
            .step_ids();
        let [parent] = heads else {
            return Err(NativeClientError::NeedsIntegration);
        };
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let objects = load_objects(&reader, state.space.id(), vec![parent.clone()], limits)?;
        let bundle = graph_merge::resolve_conflict_terms(&objects, parent, choices, limits)
            .map_err(merge_error)?;
        Ok(NativeResolutionPreview {
            state,
            target_layer: target_layer.clone(),
            bundle,
            limits,
            stamp: super::MutationStamp::fresh()?,
        })
    }
}

fn bundle_objects(bundle: InstallationBundle) -> Result<Vec<CanonicalObject>> {
    let mut result = Vec::new();
    result.extend(bundle.chunks.into_iter().map(CanonicalObject::ChunkRaw));
    result.extend(
        bundle
            .file_manifests
            .into_iter()
            .map(CanonicalObject::FileManifest),
    );
    result.extend(bundle.trees.into_iter().map(CanonicalObject::Tree));
    for conflict in bundle.conflicts {
        result.push(CanonicalObject::Conflict(CanonicalConflict::new(
            conflict.id(),
            conflict.canonical_bytes().map_err(merge_error)?,
        )?));
    }
    result.push(CanonicalObject::Step(bundle.step));
    Ok(result)
}

pub(super) fn load_objects(
    reader: &dyn ScmReader,
    space: &SpaceId,
    heads: Vec<StepId>,
    limits: GraphMergeLimits,
) -> Result<MergeObjects> {
    let mut loader = Loader {
        reader,
        space,
        objects: MergeObjects::default(),
        pending_steps: heads,
        limits,
        bytes: 0,
        paths: 0,
    };
    while let Some(id) = loader.pending_steps.pop() {
        if loader.objects.steps.contains_key(&id) {
            continue;
        }
        if loader.objects.steps.len() >= limits.max_steps {
            return Err(merge_error("Step input limit"));
        }
        loader.admitted(StoreObjectId::Step(id.clone()))?;
        let step = reader
            .get_step(&id)?
            .ok_or(NativeClientError::Missing("merge Step"))?;
        loader
            .pending_steps
            .extend_from_slice(step.parent_step_ids());
        loader.objects.steps.insert(id, step.clone());
        loader.tree(step.root_tree_id(), 0)?;
    }
    Ok(loader.objects)
}

struct Loader<'a> {
    reader: &'a dyn ScmReader,
    space: &'a SpaceId,
    objects: MergeObjects,
    pending_steps: Vec<StepId>,
    limits: GraphMergeLimits,
    bytes: usize,
    paths: usize,
}
impl Loader<'_> {
    fn admitted(&self, object: StoreObjectId) -> Result<()> {
        if !self.reader.object_admitted_to_space(&object, self.space)? {
            return Err(merge_error("object is outside the native Space"));
        }
        Ok(())
    }
    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| merge_error("input byte limit"))?;
        if self.bytes > self.limits.max_input_bytes {
            return Err(merge_error("input byte limit"));
        }
        Ok(())
    }
    fn tree(&mut self, id: &TreeId, depth: usize) -> Result<()> {
        if depth > self.limits.max_depth {
            return Err(merge_error("tree depth limit"));
        }
        if self.objects.trees.contains_key(id) {
            return Ok(());
        }
        self.admitted(StoreObjectId::Tree(id.clone()))?;
        let tree = self
            .reader
            .get_tree(id)?
            .ok_or(NativeClientError::Missing("merge Tree"))?;
        self.charge(tree.canonical_payload()?.len())?;
        self.objects.trees.insert(id.clone(), tree.clone());
        for entry in tree.entries() {
            self.paths += 1;
            if self.paths > self.limits.max_paths {
                return Err(merge_error("tree path limit"));
            }
            self.target(entry.target(), depth + 1)?;
        }
        Ok(())
    }
    fn target(&mut self, target: &TreeEntryTarget, depth: usize) -> Result<()> {
        if depth > self.limits.max_depth {
            return Err(merge_error("object depth limit"));
        }
        match target {
            TreeEntryTarget::File { file_id } => self.file(file_id)?,
            TreeEntryTarget::Tree { tree_id } => self.tree(tree_id, depth)?,
            TreeEntryTarget::Symlink { symlink_id } => {
                self.admitted(StoreObjectId::Symlink(symlink_id.clone()))?;
                let value = self
                    .reader
                    .get_symlink(symlink_id)?
                    .ok_or(NativeClientError::Missing("merge Symlink"))?;
                self.objects.symlinks.insert(symlink_id.clone(), value);
            }
            TreeEntryTarget::Tombstone { tombstone_id } => {
                self.admitted(StoreObjectId::Tombstone(tombstone_id.clone()))?;
                let value = self
                    .reader
                    .get_tombstone(tombstone_id)?
                    .ok_or(NativeClientError::Missing("merge Tombstone"))?;
                self.objects.tombstones.insert(tombstone_id.clone(), value);
            }
            TreeEntryTarget::Conflict { conflict_id } => {
                if self.objects.conflicts.contains_key(conflict_id) {
                    return Ok(());
                }
                self.admitted(StoreObjectId::Conflict(conflict_id.clone()))?;
                let value = self
                    .reader
                    .get_conflict(conflict_id)?
                    .ok_or(NativeClientError::Missing("merge Conflict"))?;
                self.charge(value.canonical_json().len())?;
                let parsed = PlannedConflict::from_canonical_bytes(value.canonical_json())
                    .map_err(merge_error)?;
                self.objects
                    .conflicts
                    .insert(conflict_id.clone(), parsed.clone());
                for id in parsed.payload_file_ids().map_err(merge_error)? {
                    self.file(&id)?;
                }
                for role in ["bases", "sides"] {
                    for term in parsed.document()[role].as_array().expect("validated terms") {
                        self.pending_steps.push(StepId::parse(
                            term["provenance"]["stepId"].as_str().unwrap(),
                        )?);
                        for placed in term["values"].as_array().expect("validated placed values") {
                            let value = &placed["value"];
                            let id = value["objectId"].as_str();
                            let target = match value["kind"].as_str().unwrap() {
                                "file" => Some(TreeEntryTarget::file(FileId::parse(id.unwrap())?)),
                                "tree" => Some(TreeEntryTarget::tree(TreeId::parse(id.unwrap())?)),
                                "symlink" => {
                                    Some(TreeEntryTarget::symlink(SymlinkId::parse(id.unwrap())?))
                                }
                                _ => None,
                            };
                            if let Some(target) = target {
                                self.target(&target, depth + 1)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn file(&mut self, id: &FileId) -> Result<()> {
        if self.objects.files.contains_key(id) {
            return Ok(());
        }
        let FileAvailability::Present(available) = self.reader.file_availability(id)? else {
            return Err(NativeClientError::Missing("merge File"));
        };
        let mut manifest = None;
        for id in available.verified_manifest_ids() {
            if self
                .reader
                .object_admitted_to_space(&StoreObjectId::FileManifest(id.clone()), self.space)?
            {
                manifest = self.reader.get_file_manifest(id)?;
                if manifest.is_some() {
                    break;
                }
            }
        }
        let manifest = manifest.ok_or(NativeClientError::Missing("admitted FileManifest"))?;
        self.charge(usize::try_from(manifest.byte_length()).map_err(merge_error)?)?;
        let mut chunks = BTreeMap::new();
        for chunk in manifest.chunks() {
            self.admitted(StoreObjectId::ChunkRaw(chunk.chunk_id().clone()))?;
            chunks.insert(
                chunk.chunk_id().clone(),
                self.reader
                    .get_chunk(chunk.chunk_id())?
                    .ok_or(NativeClientError::Missing("merge Chunk"))?,
            );
        }
        let bytes =
            manifest.reconstruct(|id| chunks.get(id).map(|chunk| chunk.bytes().to_vec()))?;
        if FileId::from_bytes(&bytes) != *id {
            return Err(merge_error("File content mismatch"));
        }
        self.objects.files.insert(id.clone(), bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_scm::{prepare, MutationStamp, SnapshotBuilder, SnapshotLimits};
    use layrs_store_local::scm_v1::NativeScmRepository;
    use layrs_weave::reconciliation::{FailureAction, FileMergeLimits};

    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("layrs-native-merge-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            if self.0.parent() == Some(std::env::temp_dir().as_path()) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
    fn policy() -> FileMergePolicy {
        FileMergePolicy {
            unsupported: FailureAction::StructuralFallback,
            quota: FailureAction::Reject,
            limits: FileMergeLimits::default(),
        }
    }
    fn import(workspace: &NativeWorkspace, objects: Vec<CanonicalObject>) {
        let state = workspace.state().unwrap();
        let batch = WriteBatch::new(
            MutationStamp::fresh().unwrap().transaction_id,
            objects.clone(),
            vec![],
            prepare::admissions(state.space.id(), &objects),
            vec![],
        )
        .unwrap();
        NativeScmRepository::new(workspace.store_root())
            .apply_batch(&batch)
            .unwrap();
    }
    fn fork(workspace: &NativeWorkspace, parent: StepId, bytes: &[u8]) -> StepId {
        let mut builder = SnapshotBuilder::new(SnapshotLimits::default());
        builder
            .add_file(RepoPath::parse("code.ts").unwrap(), bytes, false)
            .unwrap();
        let snapshot = builder.finish().unwrap();
        let step = Step::new(vec![parent], snapshot.root_tree_id().clone()).unwrap();
        let id = step.id().clone();
        let mut objects = snapshot.objects().to_vec();
        objects.push(CanonicalObject::Step(step));
        import(workspace, objects);
        id
    }

    #[test]
    fn native_merge_preview_reads_admitted_closure_without_mutating_checkout_or_heads() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("code.ts"), b"a\nb\nc\n").unwrap();
        let workspace = NativeWorkspace::initialize(&temp.0, "merge test").unwrap();
        let initial = workspace.state().unwrap();
        let layer = initial.view.layer_heads()[0].layer_id().clone();
        let base = initial.view.layer_heads()[0].step_ids()[0].clone();
        let source = fork(&workspace, base, b"a\nb\nC\n");
        std::fs::write(temp.0.join("code.ts"), b"A\nb\nc\n").unwrap();
        workspace.capture().unwrap();
        let before = workspace.state().unwrap();
        let preview = workspace
            .preview_merge(&[source], &layer, policy(), GraphMergeLimits::default())
            .unwrap();
        assert!(preview.plan().conflicts().is_empty());
        assert!(preview
            .plan()
            .files()
            .values()
            .any(|bytes| bytes == b"A\nb\nC\n"));
        assert_eq!(workspace.state().unwrap().view.id(), before.view.id());
        assert_eq!(std::fs::read(temp.0.join("code.ts")).unwrap(), b"A\nb\nc\n");
        let store = open(workspace.store_root()).unwrap();
        assert!(NativeScmReader::new(&store)
            .unwrap()
            .get_step(preview.plan().step().id())
            .unwrap()
            .is_none());
        drop(store);
        // Importing the proposed CAS objects is independently valid. This does
        // not claim to accept a Weave or move Layer/Worktree references.
        import(&workspace, preview.canonical_objects().unwrap());
        let reopened = NativeWorkspace::open(&temp.0).unwrap();
        let store = open(reopened.store_root()).unwrap();
        assert!(NativeScmReader::new(&store)
            .unwrap()
            .get_step(preview.plan().step().id())
            .unwrap()
            .is_some());
    }

    #[test]
    fn persisted_native_conflict_roundtrips_and_resolves_after_reopen() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("code.ts"), b"base").unwrap();
        let workspace = NativeWorkspace::initialize(&temp.0, "conflict test").unwrap();
        let initial = workspace.state().unwrap();
        let layer = initial.view.layer_heads()[0].layer_id().clone();
        let source = fork(
            &workspace,
            initial.view.layer_heads()[0].step_ids()[0].clone(),
            b"incoming",
        );
        std::fs::write(temp.0.join("code.ts"), b"existing").unwrap();
        workspace.capture().unwrap();
        let preview = workspace
            .preview_merge(&[source], &layer, policy(), GraphMergeLimits::default())
            .unwrap();
        import(&workspace, preview.canonical_objects().unwrap());
        let reopened = NativeWorkspace::open(&temp.0).unwrap();
        let state = reopened.state().unwrap();
        let store = open(reopened.store_root()).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        let objects = load_objects(
            &reader,
            state.space.id(),
            vec![preview.plan().step().id().clone()],
            GraphMergeLimits::default(),
        )
        .unwrap();
        let conflict = objects.conflicts.values().next().unwrap();
        let choice =
            ObjectId::parse(conflict.document()["sides"][0]["termId"].as_str().unwrap()).unwrap();
        let result = graph_merge::resolve_conflict_terms(
            &objects,
            preview.plan().step().id(),
            &BTreeMap::from([(conflict.id(), choice)]),
            GraphMergeLimits::default(),
        )
        .unwrap();
        assert!(result.conflicts.is_empty());
        assert_eq!(
            result.step.parent_step_ids(),
            std::slice::from_ref(preview.plan().step().id())
        );
        assert_eq!(std::fs::read(temp.0.join("code.ts")).unwrap(), b"existing");
    }
}
