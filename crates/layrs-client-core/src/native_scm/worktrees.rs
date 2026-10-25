//! Full-store Worktree lifecycle. Registration precedes recoverable IO; only a
//! completed materialization publishes a Ready checkout.
use super::Result;
use super::*;
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{self, NativeScmReader};
use scm_v1::checkout::{projection_tree_id, CheckoutReader};
use scm_v1::materialize::{MaterializationFaults, NoMaterializationFaults};
use scm_v1::worktrees::WorktreeCreationLease;
use std::{collections::BTreeMap, path::Path};

fn validate_root(store: &scm_v1::NativeStoreOpen, root: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        scm_v1::worktrees::validate_secondary_root_with_vfs(store, root, |worktree| {
            super::verify_registered_vfs_root(store, worktree).map_err(std::io::Error::other)
        })
    }
    #[cfg(not(windows))]
    scm_v1::worktrees::validate_secondary_root(store, root)
}

/// Fault seam at the durable reservation boundary, before metadata acceptance.
/// Materialization faults remain the existing journal protocol's own seam.
pub trait WorktreeCreationFaults {
    fn before_creation_lease(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn after_locator(&self) -> std::io::Result<()>;
    fn after_registration(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn write_staged_locator(
        &self,
        output: &mut dyn std::io::Write,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        output.write_all(bytes)
    }
    fn before_locator_publication(&self) -> std::io::Result<()> {
        Ok(())
    }
}
struct LocatorFaults<'a>(&'a dyn WorktreeCreationFaults);
impl scm_v1::worktrees::LocatorPublicationFaults for LocatorFaults<'_> {
    fn write_staged(&self, output: &mut dyn std::io::Write, bytes: &[u8]) -> std::io::Result<()> {
        self.0.write_staged_locator(output, bytes)
    }
    fn before_publication(&self) -> std::io::Result<()> {
        self.0.before_locator_publication()
    }
}
pub struct NoWorktreeCreationFaults;
impl WorktreeCreationFaults for NoWorktreeCreationFaults {
    fn after_locator(&self) -> std::io::Result<()> {
        Ok(())
    }
}

impl NativeWorkspace {
    /// Lists durable registrations in this Store and Space, including Stale
    /// interrupted creations. Inspect each Worktree's active View for status.
    pub fn list_worktrees(&self) -> Result<Vec<Worktree>> {
        let state = self.state()?;
        let store = self.open_store()?;
        Ok(scm_v1::worktrees::registered_worktrees(&store)?
            .into_iter()
            .filter(|worktree| worktree.space_id() == state.space.id())
            .collect())
    }

    /// The existing empty checkout must share the Store's filesystem volume:
    /// the durable journal protects originals with no-replace renames.
    pub fn create_worktree(
        &self,
        root: impl AsRef<Path>,
        layer_id: &LayerId,
        step_id: &StepId,
    ) -> Result<Self> {
        self.create_worktree_with_faults(root, layer_id, step_id, &NoMaterializationFaults)
    }

    /// Failure leaves the locator, registered base and any journal/backups for
    /// inspection/recovery. It never returns a usable partially written checkout.
    pub fn create_worktree_with_faults(
        &self,
        root: impl AsRef<Path>,
        layer_id: &LayerId,
        step_id: &StepId,
        faults: &dyn MaterializationFaults,
    ) -> Result<Self> {
        self.create_worktree_with_creation_faults(
            root,
            layer_id,
            step_id,
            faults,
            &NoWorktreeCreationFaults,
        )
    }

    pub fn create_worktree_with_creation_faults(
        &self,
        root: impl AsRef<Path>,
        layer_id: &LayerId,
        step_id: &StepId,
        faults: &dyn MaterializationFaults,
        creation_faults: &dyn WorktreeCreationFaults,
    ) -> Result<Self> {
        let checkout = CheckoutReader::open(root, None)?;
        let root = checkout.root().to_path_buf();
        creation_faults.before_creation_lease()?;
        let store_id = self.open_store()?.store_id().clone();
        let lease = WorktreeCreationLease::acquire(
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            &store_id,
            &root,
            &checkout.identity(),
            ordinary_identity,
        )?;
        if std::fs::read_dir(&root)?.next().transpose()?.is_some() {
            return Err(NativeClientError::InvalidState(
                "secondary Worktree directory must be empty",
            ));
        }
        let source = self.state()?;
        // Verifies admission, closure and platform representation before reserving
        // or registering anything. Conflict Steps fail explicitly if not representable.
        let after = self.read_step_projection(step_id)?;
        scm_v1::checkout::validate_streaming_projection(&after)?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        if reader.get_layer(layer_id)?.as_ref().map(Layer::space_id) != Some(source.space.id()) {
            return Err(NativeClientError::Missing("Layer in source Space"));
        }
        validate_root(&store, &root)?;
        let step = reader
            .get_step(step_id)?
            .ok_or(NativeClientError::Missing("Step"))?;
        if projection_tree_id(&after)? != *step.root_tree_id() {
            return Err(NativeClientError::InvalidState(
                "Step cannot be materialized exactly",
            ));
        }
        let stamp = MutationStamp::fresh()?;
        let worktree = Worktree::new(
            WorktreeId::parse(format!("worktree:{}", uuid::Uuid::new_v4()))?,
            store.store_id().clone(),
            source.space.id().clone(),
            stamp.operation_id.clone(),
            root.to_str().ok_or(NativeClientError::InvalidState(
                "checkout root is not UTF-8",
            ))?,
            checkout.identity(),
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        drop(reader);
        drop(store);
        let creation = scm_v1::worktrees::WorktreeCreation {
            worktree: worktree.clone(),
            source_worktree_id: self.worktree_id().clone(),
            source_operation_id: source.operation.id().clone(),
            layer_id: layer_id.clone(),
            step_id: step_id.clone(),
        };
        scm_v1::worktrees::write_locator_bound_metadata(
            &root,
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            &creation,
            &LocatorFaults(creation_faults),
        )?;
        creation_faults.after_locator()?;
        CheckoutReader::open(&root, Some(worktree.filesystem_identity()))?;
        let locator = scm_v1::worktrees::read_locator(&root)?;
        if serde_json::to_vec(&locator.creation).map_err(std::io::Error::other)?
            != serde_json::to_vec(&creation).map_err(std::io::Error::other)?
        {
            return Err(NativeClientError::InvalidState("creation locator changed"));
        }
        Self::register_reserved_worktree(&locator, &lease, Some(source.operation.id()))?;
        creation_faults.after_registration()?;
        let created = Self::open(&root)?;
        created.finish_worktree_creation(faults, &lease)?;
        Ok(created)
    }

    /// Explicitly resumes durable creation intent, including a locator written
    /// before its registration transaction committed. Never selects a new base.
    pub fn resume_worktree(root: impl AsRef<Path>) -> Result<Self> {
        let root = std::path::absolute(root)?;
        let pending = scm_v1::worktrees::read_creation_locator(&root)?;
        let lease = WorktreeCreationLease::acquire(
            &pending.store_root,
            &pending.store_filesystem_identity,
            &pending.store_metadata_identity,
            &pending.store_id,
            &root,
            &pending.filesystem_identity,
            ordinary_identity,
        )?;
        let checked = scm_v1::worktrees::read_creation_locator(&root)?;
        if serde_json::to_vec(&pending).map_err(std::io::Error::other)?
            != serde_json::to_vec(&checked).map_err(std::io::Error::other)?
        {
            return Err(NativeClientError::InvalidState(
                "creation intent changed while waiting",
            ));
        }
        let locator = scm_v1::worktrees::resume_locator(&root)?;
        let store = prepare::open_store_for_read(&locator.store_root)?
            .with_filesystem_identity(&locator.store_filesystem_identity)?
            .with_metadata_identity(&locator.store_metadata_identity)?;
        if store.store_id() != &locator.store_id {
            return Err(NativeClientError::InvalidState(
                "reservation Store ID mismatch",
            ));
        }
        let reader = NativeScmReader::new(&store)?;
        let registered = reader.get_worktree(&locator.worktree_id)?.is_some();
        drop(reader);
        drop(store);
        if !registered {
            Self::register_reserved_worktree(&locator, &lease, None)?;
        }
        let workspace = Self::open(root)?;
        let state = workspace.state()?;
        let status = state
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == workspace.worktree_id())
            .ok_or(NativeClientError::Missing("Worktree state"))?
            .status();
        if status == WorktreeStatus::Stale {
            workspace.finish_worktree_creation(&NoMaterializationFaults, &lease)?;
        } else if status != WorktreeStatus::Ready || workspace.needs_repair()? {
            return Err(NativeClientError::NeedsRepair);
        }
        Ok(workspace)
    }

    fn register_reserved_worktree(
        locator: &scm_v1::worktrees::WorktreeLocator,
        lease: &WorktreeCreationLease,
        expected_source: Option<&OperationId>,
    ) -> Result<()> {
        let intent = &locator.creation;
        let client = NativeClient::open(&locator.store_root)?;
        if client.store_identity != locator.store_filesystem_identity
            || client.metadata_identity != locator.store_metadata_identity
        {
            return Err(NativeClientError::InvalidState(
                "reservation Store directory identity changed",
            ));
        }
        let gate = lease.registration_gate()?;
        let store = client.open_store_for_registration()?;
        let reader = NativeScmReader::new(&store)?;
        let source = reader
            .get_worktree(&intent.source_worktree_id)?
            .ok_or(NativeClientError::Missing("reservation source Worktree"))?;
        if expected_source.is_some_and(|expected| source.active_operation_id() != expected) {
            return Err(StoreError::CasMismatch {
                target: format!("worktree_active_operation:{}", source.id()),
            }
            .into());
        }
        let parent = reader
            .get_operation(&intent.source_operation_id)?
            .ok_or(NativeClientError::Missing("reservation source Operation"))?;
        let source_view = reader
            .get_repository_view(parent.resulting_view_id())?
            .ok_or(NativeClientError::Missing("reservation source View"))?;
        if store.store_id() != intent.worktree.store_id()
            || source.space_id() != intent.worktree.space_id()
            || parent.space_id() != source.space_id()
            || source_view.space_id() != source.space_id()
            || !source_view
                .worktrees()
                .iter()
                .any(|w| w.worktree_id() == source.id())
            || reader
                .get_layer(&intent.layer_id)?
                .as_ref()
                .map(Layer::space_id)
                != Some(source.space_id())
            || !reader.object_admitted_to_space(
                &StoreObjectId::Step(intent.step_id.clone()),
                source.space_id(),
            )?
        {
            return Err(NativeClientError::InvalidState(
                "invalid Worktree creation intent",
            ));
        }
        validate_root(&store, &locator.checkout_root)?;
        let empty = Tree::new(vec![])?;
        let mut states = source_view.worktrees().to_vec();
        states.push(WorktreeViewState::new(
            intent.worktree.id().clone(),
            Some(intent.layer_id.clone()),
            Some(intent.step_id.clone()),
            Some(empty.id().clone()),
            WorktreeStatus::Stale,
        ));
        let view = RepositoryView::new(
            source.space_id().clone(),
            creation_layer_heads(&source_view, &intent.layer_id, &intent.step_id)?,
            source_view.marker_ids().to_vec(),
            source_view.release_ids().to_vec(),
            states,
        )?;
        let mut stamp = MutationStamp::fresh()?;
        stamp.operation_id = intent.worktree.active_operation_id().clone();
        stamp.timestamp = intent.worktree.created_at().clone();
        let operation = prepare::operation(
            &stamp,
            source.space_id().clone(),
            vec![parent.id().clone()],
            &view,
            OperationKind::Restore,
            "Register Worktree pending materialization",
        )?;
        let current = reader
            .current_repository_view(source.space_id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        let heads = reader.operation_heads(source.space_id())?;
        let objects = vec![
            CanonicalObject::Tree(empty),
            CanonicalObject::RepositoryView(view.clone()),
        ];
        let batch = WriteBatch::new(
            stamp.transaction_id,
            objects.clone(),
            vec![
                EntityRecord::Worktree(intent.worktree.clone()),
                EntityRecord::Operation(operation.clone()),
            ],
            prepare::admissions(source.space_id(), &objects),
            vec![
                StoreMutation::RepositoryView(RepositoryViewCas::new(
                    source.space_id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )?),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        source.space_id().clone(),
                        heads.clone(),
                        operation.id().clone(),
                        worktree_operations::advance_heads(&heads, parent.id(), operation.id()),
                    )?
                    .with_worktree_registration_source(intent.source_worktree_id.clone()),
                ),
            ],
        )?;
        drop(reader);
        drop(store);
        CheckoutReader::open(&locator.checkout_root, Some(&locator.filesystem_identity))?;
        client.repository().apply_batch_guarded(&batch, || {
            gate.verify()
                .and_then(|()| {
                    CheckoutReader::open(&locator.checkout_root, Some(&locator.filesystem_identity))
                        .map(|_| ())
                })
                .map_err(|error| StoreError::Backend {
                    message: error.to_string(),
                })
        })?;
        Ok(())
    }

    /// Retry an interrupted creation after `recover()` has cleared its journal.
    /// Existing external files are checked by the materializer and never erased.
    pub fn resume_worktree_creation(&self) -> Result<()> {
        let state = self.state()?;
        let lease = WorktreeCreationLease::acquire(
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            state.worktree.store_id(),
            Path::new(state.worktree.checkout_root()),
            state.worktree.filesystem_identity(),
            ordinary_identity,
        )?;
        self.finish_worktree_creation(&NoMaterializationFaults, &lease)
    }

    fn finish_worktree_creation(
        &self,
        faults: &dyn MaterializationFaults,
        lease: &WorktreeCreationLease,
    ) -> Result<()> {
        lease.verify()?;
        let source = self.state()?;
        CheckoutReader::open(
            source.worktree.checkout_root(),
            Some(lease.checkout_identity()),
        )?;
        let state = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("Worktree state"))?;
        if state.status() != WorktreeStatus::Stale
            || state.materialized_tree_id() != Some(Tree::new(vec![])?.id())
        {
            return Err(NativeClientError::InvalidState(
                "Worktree is not a pending creation",
            ));
        }
        if self.needs_repair()? {
            return Err(NativeClientError::NeedsRepair);
        }
        let step_id = state
            .base_step_id()
            .ok_or(NativeClientError::Missing("Worktree base Step"))?;
        let after = self.read_step_projection(step_id)?;
        let tree_id = projection_tree_id(&after)?;
        let states = source
            .view
            .worktrees()
            .iter()
            .map(|w| {
                if w.worktree_id() == self.worktree_id() {
                    WorktreeViewState::new(
                        w.worktree_id().clone(),
                        w.active_layer_id().cloned(),
                        Some(step_id.clone()),
                        Some(tree_id.clone()),
                        WorktreeStatus::Ready,
                    )
                } else {
                    w.clone()
                }
            })
            .collect();
        let view = RepositoryView::new(
            source.space.id().clone(),
            source.view.layer_heads().to_vec(),
            source.view.marker_ids().to_vec(),
            source.view.release_ids().to_vec(),
            states,
        )?;
        let stamp = MutationStamp::fresh()?;
        let operation = prepare::operation(
            &stamp,
            source.space.id().clone(),
            vec![source.operation.id().clone()],
            &view,
            OperationKind::Restore,
            "Materialize Worktree",
        )?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let current = reader
            .current_repository_view(source.space.id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        let heads = reader.operation_heads(source.space.id())?;
        let objects = vec![CanonicalObject::RepositoryView(view.clone())];
        let batch = WriteBatch::new(
            stamp.transaction_id,
            objects.clone(),
            vec![EntityRecord::Operation(operation.clone())],
            prepare::admissions(source.space.id(), &objects),
            vec![
                StoreMutation::RepositoryView(RepositoryViewCas::new(
                    source.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )?),
                StoreMutation::OperationHeads(OperationHeadsCas::new(
                    source.space.id().clone(),
                    heads.clone(),
                    operation.id().clone(),
                    worktree_operations::advance_heads(
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
            let store = self.open_store().map_err(std::io::Error::other)?;
            let reader = NativeScmReader::new(&store).map_err(std::io::Error::other)?;
            let entry = files
                .get(path)
                .ok_or_else(|| std::io::Error::other("Worktree file missing"))?;
            restore_projection::write_file(&reader, batch.canonical_objects(), entry, output)
        };
        let mut read_chunk = |_: &ChunkId| {
            Err(StoreError::Corruption {
                message: "unexpected Worktree chunk source".into(),
            })
        };
        scm_v1::materialize::apply_checkout_with_sources_bound_metadata(
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            self.worktree_id(),
            &[],
            &after,
            &batch,
            faults,
            &mut scm_v1::materialize::CheckoutSources {
                chunks: &[],
                read_chunk: &mut read_chunk,
                write_file: &mut write_file,
            },
        )?;
        lease.verify()?;
        Ok(())
    }
}

fn ordinary_identity(root: &Path) -> std::io::Result<String> {
    Ok(CheckoutReader::open(root, None)?.identity())
}

fn creation_layer_heads(
    view: &RepositoryView,
    layer: &LayerId,
    step: &StepId,
) -> Result<Vec<LayerHeadSet>> {
    let mut heads = view.layer_heads().to_vec();
    if !heads.iter().any(|head| head.layer_id() == layer) {
        heads.push(LayerHeadSet::new(layer.clone(), vec![step.clone()])?);
    }
    Ok(heads)
}
