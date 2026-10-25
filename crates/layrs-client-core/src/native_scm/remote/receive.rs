//! Download, validate and accept native remote state.
use super::*;
pub(super) fn require_complete_access(bootstrap: &SpaceBootstrap) -> RemoteResult<()> {
    if bootstrap.content_access.is_some() {
        return Err(NativeRemoteError::ContentRestricted);
    }
    Ok(())
}
impl NativeRemote {
    /// Download rooted history, preserve concurrent heads, and record a local
    /// ReceiveRemoteState Operation. Only an unambiguous head is materialized.
    pub fn receive(&self, workspace: &NativeWorkspace) -> RemoteResult<ReceiveReport> {
        self.receive_with_faults(workspace, &scm_v1::materialize::NoMaterializationFaults)
    }
    /// Recovery test seam; faults cannot bypass native journal or CAS checks.
    pub fn receive_with_faults(
        &self,
        workspace: &NativeWorkspace,
        faults: &dyn scm_v1::materialize::MaterializationFaults,
    ) -> RemoteResult<ReceiveReport> {
        if self.transfer.is_some() {
            return self.receive_controlled(workspace, faults);
        }
        let _lock = remote_lock(workspace)?;
        let mut binding = self.binding(workspace)?;
        if binding.pending.is_some() {
            return Err(NativeRemoteError::Invalid(
                "retry pending publication before receive",
            ));
        }
        // Authorization precedes even an implicit local Capture or a cache shortcut.
        let mut bootstrap = self.bootstrap(workspace.state()?.space.id())?;
        require_complete_access(&bootstrap)?;
        // A divergent checkout can still receive metadata, but dirty files must
        // not be claimed by an arbitrary head. Require explicit local resolution.
        let state = workspace.state()?;
        let divergent = state
            .view
            .layer_heads()
            .iter()
            .any(|h| h.step_ids().len() > 1)
            || state.view.worktrees().iter().any(|w| {
                w.worktree_id() == workspace.worktree_id()
                    && w.status() == WorktreeStatus::Conflicted
            });
        if divergent {
            if projection_dirty(workspace, &state)? {
                return Err(NativeRemoteError::Invalid(
                    "resolve or preserve dirty divergent checkout before receive",
                ));
            }
        } else {
            workspace.capture()?;
        }
        let source = workspace.state()?;
        if self.unchanged_candidate(workspace, &source, &binding, &bootstrap)? {
            let refreshed = self.bootstrap(source.space.id())?;
            require_complete_access(&refreshed)?;
            if refreshed == bootstrap {
                self.verify_unchanged_checkout(workspace, &source)?;
                return Ok(unchanged::report(&source));
            }
            bootstrap = refreshed;
        }
        let graph = self.download_graph_from_store(&binding, &bootstrap, Some(workspace))?;
        let report = self.accept_remote(
            workspace,
            &source,
            &bootstrap,
            graph,
            binding.clone_pending,
            faults,
        )?;
        binding.clone_pending = false;
        binding.observed = Some(bootstrap);
        save_binding(workspace, &binding)?;
        Ok(report)
    }
    /// Creates a distinct local Store/Worktree; remote Space/Layer identities are
    /// retained. The target must be an existing, empty directory.
    pub fn clone_into(
        &self,
        root: impl AsRef<Path>,
        space: &SpaceId,
        name: &str,
    ) -> RemoteResult<NativeWorkspace> {
        self.transfer_check()?;
        let root = std::path::absolute(root)?;
        if std::fs::read_dir(&root)?.next().is_some() {
            return Err(NativeRemoteError::Invalid("clone target must be empty"));
        }
        let bootstrap = self.bootstrap(space)?;
        require_complete_access(&bootstrap)?;
        let bases: Vec<_> = bootstrap
            .layers
            .iter()
            .filter(|p| {
                p.record.kind() == LayerKind::Base && p.record.parent_layer_ids().is_empty()
            })
            .collect();
        if bases.len() != 1 || bases[0].head_step_ids.len() != 1 {
            return Err(NativeRemoteError::Invalid(
                "clone requires a unique base Layer with one head",
            ));
        }
        let mut binding = Binding {
            origin: self.origin.to_string(),
            space_id: space.clone(),
            client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4())).expect("UUID"),
            clone_pending: true,
            observed: Some(bootstrap.clone()),
            pending: None,
            last_receipt: None,
        };
        let graph = self.download_graph(&binding, &bootstrap)?;
        // All network/download verification precedes creation of local metadata.
        let (identity, snapshot) = capture_checkout(&root, None, SnapshotLimits::default())?;
        if snapshot.root_tree_id() != Tree::new(vec![])?.id() {
            return Err(NativeRemoteError::Invalid(
                "clone target changed during download",
            ));
        }
        let stamp = MutationStamp::fresh()?;
        let suffix = uuid::Uuid::new_v4();
        let store_id = StoreId::parse(format!("store:{suffix}"))?;
        let local_space = Space::new(
            space.clone(),
            name,
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        let worktree = Worktree::new(
            WorktreeId::parse(format!("worktree:{}", uuid::Uuid::new_v4()))?,
            store_id.clone(),
            space.clone(),
            stamp.operation_id.clone(),
            root.to_str()
                .ok_or(NativeRemoteError::Invalid("clone path must be UTF-8"))?,
            identity,
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        let initialize = prepare_initialize(
            local_space,
            bases[0].record.clone(),
            worktree.clone(),
            snapshot,
            stamp,
        )?;
        self.transfer_begin_commit()?;
        drop(scm_v1::init(root.join(".layrs"), store_id)?);
        let workspace = NativeWorkspace {
            client: NativeClient::open(root.join(".layrs"))?,
            worktree_id: worktree.id().clone(),
        };
        save_binding(&workspace, &binding)?;
        workspace.client.accept(&initialize)?;
        let _lock = remote_lock(&workspace)?;
        let source = workspace.state()?;
        self.accept_remote(
            &workspace,
            &source,
            &bootstrap,
            graph,
            true,
            &scm_v1::materialize::NoMaterializationFaults,
        )?;
        binding.clone_pending = false;
        save_binding(&workspace, &binding)?;
        Ok(workspace)
    }
    pub(super) fn download_graph(
        &self,
        binding: &Binding,
        bootstrap: &SpaceBootstrap,
    ) -> RemoteResult<GraphObjects> {
        self.download_graph_from_store(binding, bootstrap, None)
    }
    pub(super) fn download_graph_from_store(
        &self,
        binding: &Binding,
        bootstrap: &SpaceBootstrap,
        local_workspace: Option<&NativeWorkspace>,
    ) -> RemoteResult<GraphObjects> {
        require_complete_access(bootstrap)?;
        let mut graph = GraphObjects::default();
        let mut pending: Vec<_> = bootstrap
            .graph_root_step_ids
            .iter()
            .cloned()
            .map(|id| (id.clone(), id))
            .collect();
        for reference in bootstrap
            .markers
            .iter()
            .map(|p| p.record.target_step_id())
            .chain(bootstrap.releases.iter().map(|p| p.record.target_step_id()))
        {
            pending.push((reference.clone(), reference.clone()));
        }
        for product in bootstrap
            .product_roots
            .iter()
            .filter(|p| matches!(p, ProductRoot::Weave { .. }))
        {
            self.fetch_visit_from_store(
                binding,
                &[FetchClaim::Object {
                    object: ObjectReference {
                        kind: ObjectKind::Product,
                        object_id: product.object_id().clone(),
                    },
                    root: AuthorizationRoot::Product {
                        product: product.clone(),
                    },
                    repo_path: Nullable::null(),
                }],
                local_workspace,
                |object| {
                    if let CanonicalObject::Product(value) = &object {
                        pending.extend(
                            value
                                .envelope()
                                .dependencies()
                                .0
                                .iter()
                                .cloned()
                                .map(|id| (id.clone(), id)),
                        );
                    }
                    graph.insert(object)
                },
            )?;
        }
        while let Some((id, root)) = pending.pop() {
            if graph.steps.contains_key(&id) {
                continue;
            }
            let claim = FetchClaim::Object {
                object: ObjectReference {
                    kind: ObjectKind::Step,
                    object_id: ObjectId::parse(id.as_str())?,
                },
                root: AuthorizationRoot::Step {
                    step_id: root.clone(),
                },
                repo_path: Nullable::null(),
            };
            self.fetch_visit_from_store(binding, &[claim], local_workspace, |object| {
                graph.insert(object)
            })?;
            let step = graph
                .steps
                .get(&id)
                .ok_or(NativeRemoteError::Invalid("missing fetched Step"))?
                .clone();
            pending.extend(
                step.parent_step_ids()
                    .iter()
                    .cloned()
                    .map(|p| (p, root.clone())),
            );
            self.download_tree(binding, &step, &mut graph, local_workspace)?;
        }
        graph.dag()?;
        // FileId is a reconstruction identity, not the manifest hash.
        graph.verify_all_files(|| self.transfer_check())?;
        rename_validation::validate(&graph)?;
        Ok(graph)
    }
    pub(super) fn download_tree(
        &self,
        binding: &Binding,
        step: &Step,
        graph: &mut GraphObjects,
        local_workspace: Option<&NativeWorkspace>,
    ) -> RemoteResult<()> {
        let root = AuthorizationRoot::Step {
            step_id: step.id().clone(),
        };
        let mut pending = vec![(String::new(), step.root_tree_id().clone(), 0usize)];
        let mut visited = BTreeSet::new();
        while let Some((prefix, id, depth)) = pending.pop() {
            if depth > 128 || visited.len() > MAX_OBJECTS {
                return Err(NativeRemoteError::Invalid("Tree closure exceeds limits"));
            }
            if !visited.insert((prefix.clone(), id.clone())) {
                continue;
            }
            let key = StoreObjectId::Tree(id.clone());
            self.fetch_visit_from_store(
                binding,
                &[FetchClaim::Object {
                    object: ObjectReference {
                        kind: ObjectKind::Tree,
                        object_id: ObjectId::parse(id.as_str())?,
                    },
                    root: root.clone(),
                    repo_path: Nullable::null(),
                }],
                local_workspace,
                |object| graph.insert(object),
            )?;
            let Some(CanonicalObject::Tree(tree)) = graph.objects.get(&key) else {
                return Err(NativeRemoteError::Invalid("missing fetched Tree"));
            };
            let tree = tree.clone();
            let mut claims = Vec::new();
            for entry in tree.entries() {
                let path = if prefix.is_empty() {
                    entry.name().as_str().to_owned()
                } else {
                    format!("{prefix}/{}", entry.name().as_str())
                };
                let claim = match entry.target() {
                    TreeEntryTarget::Tree { tree_id } => {
                        pending.push((path, tree_id.clone(), depth + 1));
                        continue;
                    }
                    TreeEntryTarget::File { file_id } => FetchClaim::File {
                        file_id: file_id.clone(),
                        preferred_manifest_ids: vec![],
                        root: root.clone(),
                        repo_path: RepoPath::parse(path)?,
                    },
                    TreeEntryTarget::Symlink { symlink_id } => FetchClaim::Object {
                        object: ObjectReference {
                            kind: ObjectKind::Symlink,
                            object_id: ObjectId::parse(symlink_id.as_str())?,
                        },
                        root: root.clone(),
                        repo_path: Nullable::some(RepoPath::parse(path)?),
                    },
                    TreeEntryTarget::Tombstone { tombstone_id } => FetchClaim::Object {
                        object: ObjectReference {
                            kind: ObjectKind::Tombstone,
                            object_id: ObjectId::parse(tombstone_id.as_str())?,
                        },
                        root: root.clone(),
                        repo_path: Nullable::some(RepoPath::parse(path)?),
                    },
                    TreeEntryTarget::Conflict { conflict_id } => {
                        self.download_conflict(
                            binding,
                            &root,
                            &path,
                            conflict_id,
                            depth,
                            graph,
                            &mut pending,
                            local_workspace,
                        )?;
                        continue;
                    }
                };
                claims.push(claim);
                if claims.len() == download_refresh::CLAIM_BATCH {
                    self.fetch_visit_from_store(binding, &claims, local_workspace, |object| {
                        graph.insert(object)
                    })?;
                    claims.clear();
                }
            }
            self.fetch_visit_from_store(binding, &claims, local_workspace, |object| {
                graph.insert(object)
            })?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn download_conflict(
        &self,
        binding: &Binding,
        root: &AuthorizationRoot,
        path: &str,
        id: &ConflictId,
        depth: usize,
        graph: &mut GraphObjects,
        pending: &mut Vec<(String, TreeId, usize)>,
        local_workspace: Option<&NativeWorkspace>,
    ) -> RemoteResult<()> {
        self.fetch_visit_from_store(
            binding,
            &[FetchClaim::Object {
                object: ObjectReference {
                    kind: ObjectKind::Conflict,
                    object_id: ObjectId::parse(id.as_str())?,
                },
                root: root.clone(),
                repo_path: Nullable::some(RepoPath::parse(path)?),
            }],
            local_workspace,
            |object| graph.insert(object),
        )?;
        let Some(CanonicalObject::Conflict(conflict)) =
            graph.objects.get(&StoreObjectId::Conflict(id.clone()))
        else {
            return Err(NativeRemoteError::Invalid("missing fetched Conflict"));
        };
        let parsed = layrs_weave::graph_merge::PlannedConflict::from_canonical_bytes(
            conflict.canonical_json(),
        )
        .map_err(|_| NativeRemoteError::Invalid("invalid Conflict"))?;
        let (_, targets) = conflict_references(conflict)?;
        let file_ids: BTreeSet<_> = targets
            .iter()
            .filter_map(|target| target.file_id().cloned())
            .collect();
        for target in targets {
            let claim = match target {
                TreeEntryTarget::Tree { tree_id } => {
                    pending.push((path.into(), tree_id, depth + 1));
                    continue;
                }
                TreeEntryTarget::File { file_id } => FetchClaim::File {
                    file_id,
                    preferred_manifest_ids: vec![],
                    root: root.clone(),
                    repo_path: RepoPath::parse(path)?,
                },
                TreeEntryTarget::Symlink { symlink_id } => FetchClaim::Object {
                    object: ObjectReference {
                        kind: ObjectKind::Symlink,
                        object_id: ObjectId::parse(symlink_id.as_str())?,
                    },
                    root: root.clone(),
                    repo_path: Nullable::some(RepoPath::parse(path)?),
                },
                _ => return Err(NativeRemoteError::Invalid("unsupported Conflict term")),
            };
            self.fetch_visit_from_store(binding, &[claim], local_workspace, |object| {
                graph.insert(object)
            })?;
        }
        if !parsed
            .payload_file_ids()
            .map_err(|_| NativeRemoteError::Invalid("invalid Conflict payload"))?
            .is_empty()
        {
            let mut payload = BTreeMap::new();
            let mut total = 0u64;
            for id in file_ids {
                let manifest = graph
                    .objects
                    .values()
                    .find_map(|object| match object {
                        CanonicalObject::FileManifest(manifest) if manifest.file_id() == &id => {
                            Some(manifest)
                        }
                        _ => None,
                    })
                    .ok_or(NativeRemoteError::Invalid("missing Conflict File"))?;
                total = total
                    .checked_add(manifest.byte_length())
                    .ok_or(NativeRemoteError::Invalid("Conflict payload limit"))?;
                if total > MAX_TOTAL as u64 {
                    return Err(NativeRemoteError::Invalid("Conflict payload limit"));
                }
                payload.insert(id.clone(), graph.file_bytes(&id)?);
            }
            parsed.validate_payload(&payload).map_err(|_| {
                NativeRemoteError::Invalid("Conflict payload does not reconstruct its origins")
            })?;
        }
        Ok(())
    }
    pub(super) fn accept_remote(
        &self,
        workspace: &NativeWorkspace,
        source: &WorkspaceState,
        remote: &SpaceBootstrap,
        mut graph: GraphObjects,
        clone_pending: bool,
        faults: &dyn scm_v1::materialize::MaterializationFaults,
    ) -> RemoteResult<ReceiveReport> {
        let local = GraphObjects::local(workspace, source)?;
        graph.merge(local)?;
        let dag = graph.dag()?;
        let mut heads: BTreeMap<LayerId, BTreeSet<StepId>> = BTreeMap::new();
        if !clone_pending {
            for h in source.view.layer_heads() {
                heads.insert(h.layer_id().clone(), h.step_ids().iter().cloned().collect());
            }
        }
        for h in remote.current_repository_view.record.layer_heads() {
            heads
                .entry(h.layer_id().clone())
                .or_default()
                .extend(h.step_ids().iter().cloned());
        }
        let heads = heads
            .into_iter()
            .map(|(layer, steps)| {
                Ok(LayerHeadSet::new(
                    layer,
                    normalize_heads(&dag, &steps.into_iter().collect::<Vec<_>>())
                        .map_err(graph_error)?
                        .as_slice()
                        .to_vec(),
                )?)
            })
            .collect::<RemoteResult<Vec<_>>>()?;
        let divergent = heads.iter().any(|h| h.step_ids().len() > 1);
        let old_state = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == workspace.worktree_id())
            .ok_or(NativeRemoteError::Invalid("missing local Worktree state"))?;
        let active = old_state.active_layer_id().cloned();
        let target = active
            .as_ref()
            .and_then(|layer| heads.iter().find(|h| h.layer_id() == layer))
            .and_then(|h| {
                if h.step_ids().len() == 1 {
                    Some(h.step_ids()[0].clone())
                } else {
                    None
                }
            });
        let conflicted = target
            .as_ref()
            .map(|step| graph.has_conflicts(step))
            .transpose()?
            .unwrap_or(false);
        let materialize = !conflicted
            && target
                .as_ref()
                .is_some_and(|target| old_state.base_step_id() != Some(target));
        let mut states = source.view.worktrees().to_vec();
        let mut after = None;
        if conflicted {
            for state in &mut states {
                if state.worktree_id() == workspace.worktree_id() {
                    *state = WorktreeViewState::new(
                        state.worktree_id().clone(),
                        active.clone(),
                        state.base_step_id().cloned(),
                        state.materialized_tree_id().cloned(),
                        WorktreeStatus::Conflicted,
                    );
                }
            }
        }
        if materialize {
            let target = target.as_ref().expect("one head");
            let entries = graph.projection(target)?;
            let tree = scm_v1::checkout::projection_tree_id(&entries)?;
            for state in &mut states {
                if state.worktree_id() == workspace.worktree_id() {
                    *state = WorktreeViewState::new(
                        workspace.worktree_id().clone(),
                        active.clone(),
                        Some(target.clone()),
                        Some(tree.clone()),
                        WorktreeStatus::Ready,
                    );
                }
            }
            after = Some(entries);
        }
        let markers: BTreeSet<_> = source
            .view
            .marker_ids()
            .iter()
            .chain(remote.current_repository_view.record.marker_ids())
            .cloned()
            .collect();
        let releases: BTreeSet<_> = source
            .view
            .release_ids()
            .iter()
            .chain(remote.current_repository_view.record.release_ids())
            .cloned()
            .collect();
        let view = RepositoryView::new(
            source.space.id().clone(),
            heads,
            markers.into_iter().collect(),
            releases.into_iter().collect(),
            states,
        )?;
        let store = workspace.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let current = reader
            .current_repository_view(source.space.id())?
            .ok_or(NativeRemoteError::Invalid("missing current View"))?;
        let operations = reader.operation_heads(source.space.id())?;
        if reader
            .worktree_active_operation(workspace.worktree_id())?
            .as_ref()
            != Some(source.operation.id())
        {
            return Err(NativeRemoteError::Local(NativeClientError::StaleCapture));
        }
        if clone_pending
            && source.operation.kind() != OperationKind::Initialize
            && source.view.layer_heads() != view.layer_heads()
        {
            return Err(NativeRemoteError::Invalid(
                "interrupted clone has acquired local changes",
            ));
        }
        let mut new_product = false;
        for object in graph.objects.values() {
            if let CanonicalObject::Product(product) = object {
                if reader.get_product(product.id())?.is_none() {
                    new_product = true;
                }
            }
        }
        if view.id() == source.view.id() && !new_product {
            return Ok(ReceiveReport {
                operation_id: source.operation.id().clone(),
                divergent,
                materialized: false,
            });
        }
        let stamp = MutationStamp::fresh()?;
        let operation = Operation::new(
            stamp.operation_id.clone(),
            source.space.id().clone(),
            vec![source.operation.id().clone()],
            view.id().clone(),
            OperationKind::ReceiveRemoteState,
            "Receive native remote state",
            vec![],
            None,
            None,
            stamp.timestamp.clone(),
            self.receive_metadata(workspace, source, &stamp.operation_id, view.id(), remote)?,
        )?;
        let mut entities = vec![EntityRecord::Operation(operation.clone())];
        for layer in &remote.layers {
            match reader.get_layer(layer.record.id())? {
                Some(old) if old != layer.record => {
                    return Err(NativeRemoteError::Invalid(
                        "remote Layer definition conflicts with local immutable Layer",
                    ));
                }
                Some(_) => (),
                None => entities.push(EntityRecord::Layer(layer.record.clone())),
            }
        }
        for marker in &remote.markers {
            match reader.get_marker(marker.record.id())? {
                Some(old) if old != marker.record => {
                    return Err(NativeRemoteError::Invalid(
                        "remote Marker immutable record differs",
                    ));
                }
                Some(_) => (),
                None => entities.push(EntityRecord::Marker(marker.record.clone())),
            }
        }
        for release in &remote.releases {
            match reader.get_release(release.record.id())? {
                Some(old) if old != release.record => {
                    return Err(NativeRemoteError::Invalid(
                        "remote Release immutable record differs",
                    ));
                }
                Some(_) => (),
                None => entities.push(EntityRecord::Release(release.record.clone())),
            }
        }
        let external_chunks = graph.chunk_descriptors();
        let next_operations =
            worktree_operations::advance_heads(&operations, source.operation.id(), operation.id());
        let mut objects: Vec<_> = graph.objects.values().cloned().collect();
        objects.push(CanonicalObject::RepositoryView(view.clone()));
        let mut admissions = prepare::admissions(source.space.id(), &objects);
        admissions.extend(external_chunks.iter().map(|(id, _)| {
            layrs_store::scm::SpaceObjectAdmission::new(
                source.space.id().clone(),
                StoreObjectId::ChunkRaw(id.clone()),
            )
        }));
        let batch = WriteBatch::new(
            stamp.transaction_id,
            objects,
            entities,
            admissions,
            vec![
                StoreMutation::RepositoryView(RepositoryViewCas::new(
                    source.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )?),
                StoreMutation::OperationHeads(OperationHeadsCas::new(
                    source.space.id().clone(),
                    operations,
                    operation.id().clone(),
                    next_operations,
                )?),
                StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                    workspace.worktree_id().clone(),
                    source.operation.id().clone(),
                    operation.id().clone(),
                )?),
            ],
        )?;
        drop(reader);
        drop(store);
        if let Some(after) = after {
            let before = workspace.read_step_projection(
                old_state
                    .base_step_id()
                    .ok_or(NativeRemoteError::Invalid("missing Worktree base"))?,
            )?;
            let files: BTreeMap<_, _> = after
                .iter()
                .filter_map(|entry| match &entry.content {
                    scm_v1::checkout::CheckoutProjectionContent::File { file_id, .. } => {
                        Some((entry.path.as_str(), file_id))
                    }
                    scm_v1::checkout::CheckoutProjectionContent::Symlink { .. } => None,
                })
                .collect();
            let mut read_chunk = |id: &ChunkId| graph.read_chunk(id);
            let mut write_file = |path: &str, output: &mut dyn std::io::Write| {
                let id = files
                    .get(path)
                    .ok_or_else(|| std::io::Error::other("missing prepared checkout file"))?;
                graph
                    .write_file(id, output)
                    .map(|_| ())
                    .map_err(std::io::Error::other)
            };
            let mut sources = scm_v1::materialize::CheckoutSources {
                chunks: &external_chunks,
                read_chunk: &mut read_chunk,
                write_file: &mut write_file,
            };
            scm_v1::materialize::apply_checkout_with_sources_bound_metadata(
                workspace.store_root(),
                workspace.store_identity(),
                workspace.metadata_identity(),
                workspace.worktree_id(),
                &before,
                &after,
                &batch,
                faults,
                &mut sources,
            )?;
        } else {
            scm_v1::NativeScmRepository::new(workspace.store_root())
                .with_store_identity(workspace.store_identity())
                .with_metadata_identity(workspace.metadata_identity())
                .apply_batch_with_chunks(&batch, &external_chunks, |id| graph.read_chunk(id))?;
        }
        Ok(ReceiveReport {
            operation_id: operation.id().clone(),
            divergent,
            materialized: materialize,
        })
    }
}
