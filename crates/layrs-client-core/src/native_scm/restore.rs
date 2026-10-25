use super::Result;
use super::{prepare, references::base_step, *};
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{
    self,
    checkout::{CheckoutContent, CheckoutEntry, CheckoutProjectionEntry},
    materialize::{MaterializationFaults, NoMaterializationFaults},
    NativeScmReader,
};
use std::collections::{BTreeMap, BTreeSet};

impl NativeWorkspace {
    pub fn recover(&self) -> Result<usize> {
        Ok(scm_v1::materialize::recover_checkout_bound_metadata(
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            self.worktree_id(),
            &NoMaterializationFaults,
        )?)
    }

    pub fn undo(&self) -> Result<OperationId> {
        let source = self.state()?;
        let [parent] = source.operation.parent_operation_ids() else {
            return Err(NativeClientError::InvalidState(
                "undo requires one parent; select an Operation explicitly",
            ));
        };
        self.restore_operation(parent, OperationKind::Undo)
    }

    pub fn redo(&self, target: Option<&OperationId>) -> Result<OperationId> {
        let state = self.state()?;
        let target = match target {
            Some(target) => target.clone(),
            None if state.operation.kind() == OperationKind::Undo
                && state.operation.parent_operation_ids().len() == 1 =>
            {
                state.operation.parent_operation_ids()[0].clone()
            }
            None => {
                return Err(NativeClientError::InvalidState(
                    "redo requires an explicit Operation after new work",
                ))
            }
        };
        self.restore_operation(&target, OperationKind::Redo)
    }

    pub fn restore_operation(
        &self,
        target: &OperationId,
        kind: OperationKind,
    ) -> Result<OperationId> {
        if !matches!(
            kind,
            OperationKind::Undo | OperationKind::Redo | OperationKind::Restore
        ) {
            return Err(NativeClientError::InvalidState(
                "invalid restore Operation kind",
            ));
        }
        let initial = self.state()?;
        let current = initial
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("current Worktree state"))?;
        if current.status() == WorktreeStatus::Conflicted {
            // A conflicted Layer has no writable projection of its result Tree.
            // Restoring is safe only while the retained old projection is exact.
            let (_, snapshot) =
                self.capture_projection_streaming(&initial, restore_projection::limits())?;
            if current.materialized_tree_id() != Some(snapshot.root_tree_id()) {
                return Err(NativeClientError::InvalidState(
                    "capture or preserve local edits before restoring a conflicted Worktree",
                ));
            }
        } else {
            self.capture_with_reason(CaptureReason::BeforeOperation)?;
        }
        let source = self.state()?;
        self.restore_operation_from_state(&source, target, kind)
    }

    pub(super) fn restore_operation_from_state(
        &self,
        source: &WorkspaceState,
        target: &OperationId,
        kind: OperationKind,
    ) -> Result<OperationId> {
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let operation = reader
            .get_operation(target)?
            .ok_or(NativeClientError::Missing("target Operation"))?;
        if operation.space_id() != source.space.id() {
            return Err(NativeClientError::InvalidState(
                "target Operation belongs to another Space",
            ));
        }
        let view = reader
            .get_repository_view(operation.resulting_view_id())?
            .ok_or(NativeClientError::Missing("target View"))?;
        for current in source
            .view
            .worktrees()
            .iter()
            .filter(|w| w.worktree_id() != self.worktree_id())
        {
            if view
                .worktrees()
                .iter()
                .find(|w| w.worktree_id() == current.worktree_id())
                != Some(current)
            {
                return Err(NativeClientError::InvalidState(
                    "restore would affect another Worktree; acquire all leases first",
                ));
            }
        }
        if view.worktrees().len() != source.view.worktrees().len() {
            return Err(NativeClientError::InvalidState(
                "restore would add or remove Worktrees",
            ));
        }
        let state = view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("historical Worktree state"))?;
        let step = state
            .base_step_id()
            .ok_or(NativeClientError::Missing("historical base Step"))?
            .clone();
        let layer = state.active_layer_id().cloned();
        drop(reader);
        drop(store);
        let after = self.read_step_projection(&step)?;
        self.apply_transition(
            source,
            step,
            layer,
            after,
            None,
            Some(view),
            kind,
            "Restore Operation View",
            &NoMaterializationFaults,
        )?;
        Ok(self.state()?.operation.id().clone())
    }
    pub fn switch_layer(&self, name_or_id: &str) -> Result<StepId> {
        self.capture_with_reason(CaptureReason::BeforeLayerSwitch)?;
        let source = self.state()?;
        let matches: Vec<_> = source
            .layers
            .iter()
            .filter(|l| l.id().as_str() == name_or_id || l.name() == name_or_id)
            .collect();
        let [layer] = matches.as_slice() else {
            return Err(NativeClientError::InvalidState(
                "Layer is missing or ambiguous; use its ID",
            ));
        };
        let heads = source
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == layer.id())
            .ok_or(NativeClientError::Missing("Layer heads"))?;
        let [step] = heads.step_ids() else {
            return Err(NativeClientError::NeedsIntegration);
        };
        let target = self.read_step_projection(step)?;
        self.apply_transition(
            &source,
            step.clone(),
            Some(layer.id().clone()),
            target,
            None,
            None,
            OperationKind::SwitchLayer,
            "Switch Layer",
            &NoMaterializationFaults,
        )?;
        Ok(step.clone())
    }

    pub fn restore_step(&self, step: &StepId, path: Option<&str>) -> Result<StepId> {
        self.restore_step_with_faults(step, path, &NoMaterializationFaults)
    }

    /// Fault injection is shared with integration tests of the durable protocol.
    pub fn restore_step_with_faults(
        &self,
        step: &StepId,
        path: Option<&str>,
        faults: &dyn MaterializationFaults,
    ) -> Result<StepId> {
        self.capture_with_reason(CaptureReason::BeforeOperation)?;
        let source = self.state()?;
        self.restore_step_from_state(&source, step, path, faults)
    }

    pub(super) fn restore_step_from_state(
        &self,
        source: &WorkspaceState,
        step: &StepId,
        path: Option<&str>,
        faults: &dyn MaterializationFaults,
    ) -> Result<StepId> {
        let mut target = self.read_step_projection(step)?;
        if let Some(path) = path {
            let selected = RepoPath::parse(path)?;
            let prefix = format!("{}/", selected.as_str());
            let matches = |p: &str| p == selected.as_str() || p.starts_with(&prefix);
            target.retain(|e| matches(&e.path));
            let mut current = self.read_step_projection(&base_step(source)?)?;
            current.retain(|e| !matches(&e.path));
            current.extend(target);
            target = current;
            target.sort_by(|a, b| a.path.cmp(&b.path));
        }
        let snapshot = restore_projection::snapshot(&target)?;
        let step = Step::new(vec![base_step(source)?], snapshot.root_tree_id().clone())?;
        let active_layer = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .and_then(|w| w.active_layer_id())
            .cloned();
        self.apply_transition(
            source,
            step.id().clone(),
            active_layer,
            target,
            Some((step.clone(), snapshot)),
            None,
            OperationKind::Restore,
            "Restore Step contents",
            faults,
        )?;
        Ok(step.id().clone())
    }

    pub fn read_step_files(&self, step: &StepId) -> Result<Vec<CheckoutEntry>> {
        let state = self.state()?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        if !reader.object_admitted_to_space(&StoreObjectId::Step(step.clone()), state.space.id())? {
            return Err(NativeClientError::Missing("Step admitted to this Space"));
        }
        let step = reader
            .get_step(step)?
            .ok_or(NativeClientError::Missing("Step"))?;
        read_tree(&reader, step.root_tree_id())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_transition(
        &self,
        source: &WorkspaceState,
        target_step: StepId,
        active_layer: Option<LayerId>,
        after: Vec<CheckoutProjectionEntry>,
        created: Option<(Step, PreparedSnapshot)>,
        restored_view: Option<RepositoryView>,
        kind: OperationKind,
        summary: &str,
        faults: &dyn MaterializationFaults,
    ) -> Result<()> {
        self.apply_projection_transition(
            source,
            target_step,
            active_layer,
            after,
            created,
            restored_view,
            kind,
            summary,
            vec![],
            faults,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_rewrite_transition(
        &self,
        source: &WorkspaceState,
        target_step: StepId,
        active_layer: Option<LayerId>,
        after: Vec<CheckoutEntry>,
        created: Option<(Step, PreparedSnapshot)>,
        restored_view: Option<RepositoryView>,
        kind: OperationKind,
        summary: &str,
        rewrite_map: Vec<RewriteTarget>,
        faults: &dyn MaterializationFaults,
    ) -> Result<()> {
        self.apply_projection_transition(
            source,
            target_step,
            active_layer,
            after.iter().map(CheckoutProjectionEntry::from).collect(),
            created,
            restored_view,
            kind,
            summary,
            rewrite_map,
            faults,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_projection_transition(
        &self,
        source: &WorkspaceState,
        target_step: StepId,
        active_layer: Option<LayerId>,
        after: Vec<CheckoutProjectionEntry>,
        created: Option<(Step, PreparedSnapshot)>,
        restored_view: Option<RepositoryView>,
        kind: OperationKind,
        summary: &str,
        rewrite_map: Vec<RewriteTarget>,
        faults: &dyn MaterializationFaults,
    ) -> Result<()> {
        let before = self.read_step_projection(&base_step(source)?)?;
        let stamp = MutationStamp::fresh()?;
        let snapshot = restore_projection::snapshot(&after)?;
        let reference_view = restored_view.as_ref().unwrap_or(&source.view);
        let states = source
            .view
            .worktrees()
            .iter()
            .map(|w| {
                if w.worktree_id() == self.worktree_id() {
                    if let Some(historical) = restored_view.as_ref().and_then(|view| {
                        view.worktrees()
                            .iter()
                            .find(|s| s.worktree_id() == self.worktree_id())
                    }) {
                        return historical.clone();
                    }
                    WorktreeViewState::new(
                        w.worktree_id().clone(),
                        active_layer.clone(),
                        Some(target_step.clone()),
                        Some(snapshot.root_tree_id().clone()),
                        WorktreeStatus::Ready,
                    )
                } else {
                    w.clone()
                }
            })
            .collect();
        let heads = reference_view
            .layer_heads()
            .iter()
            .map(|h| {
                if created.is_some() && Some(h.layer_id()) == active_layer.as_ref() {
                    LayerHeadSet::new(h.layer_id().clone(), vec![target_step.clone()])
                } else {
                    Ok(h.clone())
                }
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next = RepositoryView::new(
            source.space.id().clone(),
            heads,
            reference_view.marker_ids().to_vec(),
            reference_view.release_ids().to_vec(),
            states,
        )?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let current = reader
            .current_repository_view(source.space.id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        let heads = reader.operation_heads(source.space.id())?;
        if reader
            .worktree_active_operation(self.worktree_id())?
            .as_ref()
            != Some(source.operation.id())
        {
            return Err(NativeClientError::StaleCapture);
        }
        let operation = Operation::new(
            stamp.operation_id.clone(),
            source.space.id().clone(),
            vec![source.operation.id().clone()],
            next.id().clone(),
            kind,
            summary,
            rewrite_map,
            None,
            None,
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        let mut objects = vec![CanonicalObject::RepositoryView(next.clone())];
        if let Some((step, snapshot)) = created {
            objects.extend_from_slice(snapshot.objects());
            objects.push(CanonicalObject::Step(step));
        }
        let admissions = prepare::admissions(source.space.id(), &objects);
        let batch = WriteBatch::new(
            stamp.transaction_id,
            objects,
            vec![EntityRecord::Operation(operation.clone())],
            admissions,
            vec![
                StoreMutation::RepositoryView(RepositoryViewCas::new(
                    source.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    next.id().clone(),
                )?),
                StoreMutation::OperationHeads(OperationHeadsCas::new(
                    source.space.id().clone(),
                    heads.clone(),
                    operation.id().clone(),
                    super::worktree_operations::advance_heads(
                        &heads,
                        source.operation.id(),
                        operation.id(),
                    ),
                )?),
                StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                    self.worktree_id().clone(),
                    source.operation.id().clone(),
                    operation.id().clone(),
                )?),
            ],
        )?;
        drop(reader);
        drop(store);
        let files: BTreeMap<_, _> = after.iter().map(|e| (e.path.as_str(), e)).collect();
        let mut write_file = |path: &str, output: &mut dyn std::io::Write| {
            // Staging holds the checkout lease but no exclusive Store handle.
            // Never retain this shared capability across journal/commit phases.
            let content_store = self.open_store().map_err(std::io::Error::other)?;
            let content_reader =
                NativeScmReader::new(&content_store).map_err(std::io::Error::other)?;
            let entry = files
                .get(path)
                .ok_or_else(|| std::io::Error::other("prepared restore file absent"))?;
            restore_projection::write_file(
                &content_reader,
                batch.canonical_objects(),
                entry,
                output,
            )
        };
        let mut read_chunk = |_: &ChunkId| {
            Err(StoreError::Corruption {
                message: "unexpected restore chunk source".into(),
            })
        };
        scm_v1::materialize::apply_checkout_with_sources_bound_metadata(
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            self.worktree_id(),
            &before,
            &after,
            &batch,
            faults,
            &mut scm_v1::materialize::CheckoutSources {
                chunks: &[],
                read_chunk: &mut read_chunk,
                write_file: &mut write_file,
            },
        )?;
        Ok(())
    }
}

pub(super) fn snapshot_entries(entries: &[CheckoutEntry]) -> Result<PreparedSnapshot> {
    let mut builder = SnapshotBuilder::new(SnapshotLimits::default());
    for e in entries {
        match &e.content {
            CheckoutContent::File { bytes, executable } => {
                builder.add_file(RepoPath::parse(&e.path)?, bytes, *executable)?
            }
            CheckoutContent::Symlink { target } => {
                builder.add_symlink(RepoPath::parse(&e.path)?, target)?
            }
        }
    }
    builder.finish()
}

fn read_tree(reader: &impl ScmReader, root: &TreeId) -> Result<Vec<CheckoutEntry>> {
    let mut pending = vec![(String::new(), root.clone(), 0)];
    let mut files: BTreeMap<String, CheckoutContent> = BTreeMap::new();
    // Reuse bytes already verified during this read, without a persistent cache
    // or another copy retained for every unique file. Modes belong to entries.
    let mut verified_files: BTreeMap<FileId, String> = BTreeMap::new();
    let mut total = 0_usize;
    let mut entries = 0_usize;
    let mut visited = BTreeSet::new();
    while let Some((prefix, id, depth)) = pending.pop() {
        if depth > 128
            || files.len() + pending.len() > 100_000
            || !visited.insert((prefix.clone(), id.clone()))
        {
            return Err(NativeClientError::InvalidState("Tree traversal limit"));
        }
        let tree = reader
            .get_tree(&id)?
            .ok_or(NativeClientError::Missing("Tree"))?;
        for entry in tree.entries() {
            entries += 1;
            if entries > 100_000 {
                return Err(NativeClientError::InvalidState("Tree traversal limit"));
            }
            let path = if prefix.is_empty() {
                entry.name().as_str().to_owned()
            } else {
                format!("{prefix}/{}", entry.name().as_str())
            };
            let content = match entry.target() {
                TreeEntryTarget::Tree { tree_id } => {
                    pending.push((path, tree_id.clone(), depth + 1));
                    continue;
                }
                TreeEntryTarget::File { file_id } => {
                    if let Some(CheckoutContent::File { bytes, .. }) =
                        verified_files.get(file_id).and_then(|path| files.get(path))
                    {
                        total = total.checked_add(bytes.len()).ok_or(
                            NativeClientError::InvalidState("materialization size overflow"),
                        )?;
                        if total > 256 * 1024 * 1024 {
                            return Err(NativeClientError::InvalidState(
                                "materialization total limit exceeded",
                            ));
                        }
                        let content = CheckoutContent::File {
                            bytes: bytes.clone(),
                            executable: entry.executable(),
                        };
                        files.insert(path, content);
                        continue;
                    }
                    let FileAvailability::Present(available) = reader.file_availability(file_id)?
                    else {
                        return Err(NativeClientError::Missing("File content"));
                    };
                    let manifest = reader
                        .get_file_manifest(&available.verified_manifest_ids()[0])?
                        .ok_or(NativeClientError::Missing("FileManifest"))?;
                    if manifest.byte_length() > 64 * 1024 * 1024 {
                        return Err(NativeClientError::InvalidState(
                            "materialization file limit exceeded",
                        ));
                    }
                    total = total.checked_add(manifest.byte_length() as usize).ok_or(
                        NativeClientError::InvalidState("materialization size overflow"),
                    )?;
                    if total > 256 * 1024 * 1024 {
                        return Err(NativeClientError::InvalidState(
                            "materialization total limit exceeded",
                        ));
                    }
                    let mut chunks = BTreeMap::new();
                    for part in manifest.chunks() {
                        chunks.insert(
                            part.chunk_id().clone(),
                            reader
                                .get_chunk(part.chunk_id())?
                                .ok_or(NativeClientError::Missing("Chunk"))?,
                        );
                    }
                    let bytes =
                        manifest.reconstruct(|id| chunks.get(id).map(|c| c.bytes().to_vec()))?;
                    verified_files.insert(file_id.clone(), path.clone());
                    CheckoutContent::File {
                        bytes,
                        executable: entry.executable(),
                    }
                }
                TreeEntryTarget::Symlink { symlink_id } => CheckoutContent::Symlink {
                    target: reader
                        .get_symlink(symlink_id)?
                        .ok_or(NativeClientError::Missing("Symlink"))?
                        .target()
                        .to_owned(),
                },
                TreeEntryTarget::Tombstone { .. } => continue,
                TreeEntryTarget::Conflict { .. } => {
                    return Err(NativeClientError::InvalidState(
                        "resolve structured conflicts before materialization",
                    ))
                }
            };
            files.insert(path, content);
        }
    }
    Ok(files
        .into_iter()
        .map(|(path, content)| CheckoutEntry { path, content })
        .collect())
}
