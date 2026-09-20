//! Composite admission view over durable state and one immutable WriteBatch.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use layrs_core::scm::{
    ActorId, AdmissionResolver, ChunkId, ConflictId, DeviceId, FileId, FileManifestId, LayerId,
    MarkerId, Operation, OperationId, OperationKind, ProofId, ReleaseId, RepositoryView,
    RepositoryViewId, SpaceId, Step, StepId, StoreId, SymlinkId, TombstoneId, TreeId, WorktreeId,
    validate_file_manifest_admission, validate_layer_admission, validate_marker_admission,
    validate_operation_admission, validate_release_admission,
    validate_release_publication_admission, validate_repository_view_admission,
    validate_step_admission, validate_step_observation_admission, validate_tree_admission,
    validate_worktree_admission,
};
use layrs_graph::scm::{OperationDag, StepDag};
use layrs_store::scm::{
    CanonicalObject, EntityRecord, FileAvailability, Result, ScmReader, StoreError, StoreMutation,
    StoreObjectId, WriteBatch,
};

pub(super) const MAX_BATCH_OBJECTS: usize = 16_384;
const MAX_BATCH_ENTITIES: usize = 16_384;
const MAX_BATCH_ADMISSIONS: usize = 65_536;
const MAX_BATCH_MUTATIONS: usize = 4_096;
const MAX_GRAPH_NODES: usize = 16_384;
const MAX_GRAPH_EDGES: usize = 16_384;
// Object references scale with file paths/chunks, independently from history
// parent edges. Keep this bounded by the existing closure traversal budget.
const MAX_OBJECT_DEPENDENCIES: usize = 262_144;
const MAX_ANCESTOR_QUERIES: usize = 4_096;
const MAX_ANCESTOR_RELATIONS: usize = 1_000_000;
const MAX_ADMISSION_VISITS: usize = 65_536;
const MAX_ADMISSION_EDGE_VISITS: usize = 262_144;
#[path = "product_admission.rs"]
mod products;

/// Read-through resolver whose overlay is the entire batch, independent of
/// serialization order. Candidate forward references are therefore visible.
type PreparedChunkSource<'a> = dyn Fn(&ChunkId) -> Result<Option<layrs_core::scm::Chunk>> + 'a;
pub struct CompositeAdmissionResolver<'a> {
    reader: &'a dyn ScmReader,
    chunk_source: Option<&'a PreparedChunkSource<'a>>,
    batch: &'a WriteBatch,
    objects: BTreeMap<StoreObjectId, &'a CanonicalObject>,
    entities: BTreeMap<(&'static str, String), &'a EntityRecord>,
    admissions: BTreeSet<(SpaceId, StoreObjectId)>,
    manifests_by_file: BTreeMap<FileId, BTreeSet<FileManifestId>>,
    step_dag: StepDag,
    step_ranks: BTreeMap<StepId, usize>,
    operation_dag: OperationDag,
    durable_operation_dag: OperationDag,
    durable_operation_heads: BTreeMap<SpaceId, Vec<OperationId>>,
    new_operations: BTreeMap<OperationId, Operation>,
    published_by_space: BTreeMap<SpaceId, OperationId>,
    object_cache: Mutex<BTreeMap<StoreObjectId, Option<CanonicalObject>>>,
    admitted_cache: Mutex<BTreeMap<(SpaceId, StoreObjectId), bool>>,
    closed_admissions: Mutex<BTreeSet<(SpaceId, StoreObjectId)>>,
    file_manifest_cache: Mutex<BTreeMap<(SpaceId, FileId), Option<FileManifestId>>>,
    verified_files: Mutex<BTreeMap<FileId, bool>>,
    ancestor_cache: Mutex<BTreeMap<(StepId, StepId), bool>>,
    ancestor_sets: Mutex<BTreeMap<StepId, BTreeSet<StepId>>>,
    operation_step_closures: Mutex<BTreeMap<OperationId, BTreeSet<StepId>>>,
    ancestor_queries: AtomicUsize,
    ancestor_relations: AtomicUsize,
    admission_visits: AtomicUsize,
    admission_edge_visits: AtomicUsize,
    failure: Mutex<Option<StoreError>>,
}

impl<'a> CompositeAdmissionResolver<'a> {
    pub fn new(reader: &'a dyn ScmReader, batch: &'a WriteBatch) -> Result<Self> {
        preflight_batch_budgets(batch)?;
        let objects = batch
            .canonical_objects()
            .iter()
            .map(|object| (object.object_id(), object))
            .collect();
        let entities = batch
            .entity_records()
            .iter()
            .map(|record| ((entity_kind(record), record.id_str().to_owned()), record))
            .collect();
        let admissions = batch
            .admissions()
            .iter()
            .map(|value| (value.space_id().clone(), value.object_id().clone()))
            .collect();
        let mut manifests_by_file = BTreeMap::<FileId, BTreeSet<FileManifestId>>::new();
        for object in batch.canonical_objects() {
            if let CanonicalObject::FileManifest(manifest) = object {
                manifests_by_file
                    .entry(manifest.file_id().clone())
                    .or_default()
                    .insert(manifest.id().clone());
            }
        }
        let steps = load_step_closure(reader, batch)?;
        let batch_operations: BTreeMap<_, _> = batch
            .entity_records()
            .iter()
            .filter_map(|record| match record {
                EntityRecord::Operation(value) => Some((value.id().clone(), value.clone())),
                _ => None,
            })
            .collect();
        let mut new_operations = BTreeMap::new();
        for (id, operation) in &batch_operations {
            match reader.get_operation(id)? {
                Some(durable) if durable != *operation => {
                    return Err(StoreError::Corruption {
                        message: format!(
                            "batch Operation {id} disagrees with the durable immutable payload"
                        ),
                    });
                }
                Some(_) => {}
                None => {
                    new_operations.insert(id.clone(), operation.clone());
                }
            }
        }
        for operation in new_operations
            .values()
            .filter(|operation| operation.kind() == OperationKind::Initialize)
        {
            if let Some(durable) = reader.initializer_in_space(operation.space_id())? {
                if durable.id() != operation.id() {
                    return Err(candidate_graph_rejected(format!(
                        "Space {} already has durable Initialize Operation {}; cannot import {}",
                        operation.space_id(),
                        durable.id(),
                        operation.id()
                    )));
                }
            }
        }
        let published_by_space: BTreeMap<_, _> = batch
            .mutations()
            .iter()
            .filter_map(|mutation| match mutation {
                StoreMutation::OperationHeads(cas) => {
                    Some((cas.space_id().clone(), cas.new_operation_id().clone()))
                }
                _ => None,
            })
            .collect();
        let mut durable_operation_heads = BTreeMap::new();
        let relevant_spaces: BTreeSet<_> = new_operations
            .values()
            .map(|operation| operation.space_id().clone())
            .chain(published_by_space.keys().cloned())
            .collect();
        for space_id in relevant_spaces {
            let heads = reader.operation_heads(&space_id)?;
            if !is_strictly_sorted(&heads) {
                return Err(StoreError::Corruption {
                    message: format!(
                        "durable Operation heads for {} are not strictly sorted and unique",
                        space_id
                    ),
                });
            }
            durable_operation_heads.insert(space_id, heads);
        }
        let operations = load_operation_closure(reader, batch, &durable_operation_heads)?;
        let durable_operations = load_durable_frontier_closure(reader, &durable_operation_heads)?;
        let step_dag = StepDag::from_steps(steps.values()).map_err(graph_corruption)?;
        let step_ranks = step_dag
            .topological()
            .map_err(graph_corruption)?
            .into_iter()
            .enumerate()
            .map(|(rank, id)| (id, rank))
            .collect();
        let durable_operation_dag = build_operation_dag(&durable_operations)?;
        // Global history validation includes every loaded durable ancestor and
        // every new Operation, published or passive. It checks only immutable
        // DAG structure: publication-frontier semantics are validated below
        // against the strictly head-rooted durable DAG. The two graphs must
        // never be conflated, because disconnected historical imports are not
        // current heads and passive children may legitimately follow the
        // Operation published by this same batch.
        validate_global_operation_history(&operations)?;
        let operation_dag = durable_operation_dag.clone();
        Ok(Self {
            reader,
            chunk_source: None,
            batch,
            objects,
            entities,
            admissions,
            manifests_by_file,
            step_dag,
            step_ranks,
            operation_dag,
            durable_operation_dag,
            durable_operation_heads,
            new_operations,
            published_by_space,
            object_cache: Mutex::new(BTreeMap::new()),
            admitted_cache: Mutex::new(BTreeMap::new()),
            closed_admissions: Mutex::new(BTreeSet::new()),
            file_manifest_cache: Mutex::new(BTreeMap::new()),
            verified_files: Mutex::new(BTreeMap::new()),
            ancestor_cache: Mutex::new(BTreeMap::new()),
            ancestor_sets: Mutex::new(BTreeMap::new()),
            operation_step_closures: Mutex::new(BTreeMap::new()),
            ancestor_queries: AtomicUsize::new(0),
            ancestor_relations: AtomicUsize::new(0),
            admission_visits: AtomicUsize::new(0),
            admission_edge_visits: AtomicUsize::new(0),
            failure: Mutex::new(None),
        })
    }

    pub fn step_dag(&self) -> &StepDag {
        &self.step_dag
    }

    /// Read-only prepared content used to validate a materialization before its
    /// journal is created. IDs are independently checked here; it grants no
    /// admission or durable availability by itself.
    pub fn with_chunk_source(
        mut self,
        source: &'a dyn Fn(&ChunkId) -> Result<Option<layrs_core::scm::Chunk>>,
    ) -> Self {
        self.chunk_source = Some(source);
        self
    }

    /// Durable Operation DAG reachable exclusively from current heads.
    pub fn operation_dag(&self) -> &OperationDag {
        &self.operation_dag
    }

    /// Validates all contextual references without writing or opening a
    /// transaction. Backend failures take precedence over an induced false
    /// result from the infallible Core resolver adapter.
    pub fn validate_batch(&self) -> Result<()> {
        for object in self.batch.canonical_objects() {
            if let CanonicalObject::Product(value) = object {
                value.validate_native_subset()?;
            }
            let validation = match object {
                CanonicalObject::FileManifest(value) => {
                    validate_file_manifest_admission(value, self)
                }
                CanonicalObject::Tree(value) => validate_tree_admission(value, self),
                CanonicalObject::Step(value) => validate_step_admission(value, self),
                CanonicalObject::RepositoryView(value) => {
                    validate_repository_view_admission(value, self)
                }
                CanonicalObject::ChunkRaw(_)
                | CanonicalObject::Symlink(_)
                | CanonicalObject::Tombstone(_)
                | CanonicalObject::Conflict(_)
                | CanonicalObject::Product(_) => Ok(()),
            };
            self.finish_validation(validation, object_kind(object), object.object_id().as_str())?;
        }

        for record in self.batch.entity_records() {
            let validation = match record {
                EntityRecord::Space(_) => Ok(()),
                EntityRecord::Layer(value) => validate_layer_admission(value, self),
                EntityRecord::StepObservation(value) => {
                    validate_step_observation_admission(value, self)
                }
                EntityRecord::Operation(value) => validate_operation_admission(value, self),
                EntityRecord::Worktree(value) => validate_worktree_admission(value, self),
                EntityRecord::Marker(value) => validate_marker_admission(value, self),
                EntityRecord::Release(value) => validate_release_admission(value, self),
                EntityRecord::ReleasePublication(value) => {
                    validate_release_publication_admission(value, self)
                }
            };
            self.finish_validation(validation, entity_kind(record), record.id_str())?;
        }

        self.validate_operation_frontiers()?;
        self.validate_weave_operations()?;

        for admission in self.batch.admissions() {
            self.validate_admission(admission.space_id(), admission.object_id())?;
        }
        for mutation in self.batch.mutations() {
            self.validate_mutation(mutation)?;
        }
        self.take_failure().map_or(Ok(()), Err)
    }

    /// Validates only explicitly published Operations against the complete
    /// durable Space frontier. New Operations without a heads CAS are passive
    /// historical imports: they are in `operation_dag`, but never in this
    /// frontier candidate.
    fn validate_operation_frontiers(&self) -> Result<()> {
        for (space_id, operation_id) in &self.published_by_space {
            let cas = self
                .batch
                .mutations()
                .iter()
                .find_map(|mutation| match mutation {
                    StoreMutation::OperationHeads(cas) if cas.space_id() == space_id => Some(cas),
                    _ => None,
                })
                .ok_or_else(|| StoreError::Corruption {
                    message: format!("published Operation index lost heads CAS for {space_id}"),
                })?;
            let operation = self
                .new_operations
                .get(operation_id)
                .ok_or_else(|| StoreError::InvalidOperationHeadsTransition {
                    operation_id: operation_id.clone(),
                    reason: "published Operation is already durable; a publication must introduce a new immutable Operation".into(),
                })?;
            let durable_heads = self
                .durable_operation_heads
                .get(space_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if cas.expected_head_ids() != durable_heads {
                return Err(invalid_operation_frontier(
                    operation,
                    "expected heads must equal the complete durable Space head set",
                ));
            }

            let mut candidate = self.durable_operation_dag.clone();
            let insertion =
                if operation.kind() == layrs_core::scm::OperationKind::IntegrateOperations {
                    candidate.insert_integration(operation, durable_heads)
                } else {
                    candidate.insert_batch([operation])
                };
            insertion.map_err(|error| invalid_operation_frontier(operation, error.to_string()))?;
            if candidate.heads_in_space(space_id) != cas.new_head_ids() {
                return Err(invalid_operation_frontier(
                    operation,
                    "new heads do not equal the complete candidate Space frontier",
                ));
            }
        }
        Ok(())
    }

    fn validate_mutation(&self, mutation: &StoreMutation) -> Result<()> {
        match mutation {
            StoreMutation::RepositoryView(cas) => {
                if !self.repository_view_in_space_result(cas.new_view_id(), cas.space_id())? {
                    return Err(mutation_rejected("RepositoryViewCas", cas.new_view_id()));
                }
                if let Some(expected) = cas.expected_view_id() {
                    if !self.repository_view_in_space_result(expected, cas.space_id())? {
                        return Err(mutation_rejected("RepositoryViewCas", expected));
                    }
                }
            }
            StoreMutation::OperationHeads(cas) => {
                for operation in cas
                    .expected_head_ids()
                    .iter()
                    .chain(std::iter::once(cas.new_operation_id()))
                    .chain(cas.new_head_ids())
                {
                    if !self.operation_in_space_result(operation, cas.space_id())? {
                        return Err(mutation_rejected("OperationHeadsCas", operation));
                    }
                }
            }
            StoreMutation::WorktreeActiveOperation(cas) => {
                // Mutable Worktree state is anchored in the durable row. A
                // same-batch immutable Worktree record cannot manufacture a
                // mutable CAS target or redefine its Space.
                let Some(worktree) = self.reader.get_worktree(cas.worktree_id())? else {
                    return Err(mutation_rejected(
                        "WorktreeActiveOperationCas",
                        cas.worktree_id(),
                    ));
                };
                let Some(published) = self.published_by_space.get(worktree.space_id()) else {
                    return Err(mutation_rejected(
                        "WorktreeActiveOperationCas",
                        cas.new_operation_id(),
                    ));
                };
                if published != cas.new_operation_id() {
                    return Err(mutation_rejected(
                        "WorktreeActiveOperationCas",
                        cas.new_operation_id(),
                    ));
                }
                let durable_active = self.reader.worktree_active_operation(cas.worktree_id())?;
                if durable_active.as_ref() != Some(cas.expected_operation_id()) {
                    return Err(mutation_rejected(
                        "WorktreeActiveOperationCas",
                        cas.expected_operation_id(),
                    ));
                }
                for operation in [cas.expected_operation_id(), published] {
                    if !self.operation_in_space_result(operation, worktree.space_id())? {
                        return Err(mutation_rejected("WorktreeActiveOperationCas", operation));
                    }
                }
            }
        }
        Ok(())
    }

    fn finish_validation(
        &self,
        validation: layrs_core::scm::Result<()>,
        kind: &'static str,
        id: &str,
    ) -> Result<()> {
        if let Some(error) = self.take_failure() {
            return Err(error);
        }
        validation.map_err(|_| StoreError::AdmissionRejected {
            kind,
            id: id.to_owned(),
        })
    }

    fn resolve<T>(&self, result: Result<T>, fallback: T) -> T {
        match result {
            Ok(value) => value,
            Err(error) => {
                if let Ok(mut failure) = self.failure.lock() {
                    if failure.is_none() {
                        *failure = Some(error);
                    }
                }
                fallback
            }
        }
    }

    fn take_failure(&self) -> Option<StoreError> {
        self.failure.lock().ok().and_then(|mut value| value.take())
    }

    fn overlay_object(&self, id: &StoreObjectId) -> Option<&CanonicalObject> {
        self.objects.get(id).copied()
    }

    fn overlay_entity(&self, kind: &'static str, id: &str) -> Option<&EntityRecord> {
        self.entities.get(&(kind, id.to_owned())).copied()
    }

    fn view(&self, id: &RepositoryViewId) -> Result<Option<RepositoryView>> {
        match self.object(&StoreObjectId::RepositoryView(id.clone()))? {
            Some(CanonicalObject::RepositoryView(value)) => Ok(Some(value)),
            Some(_) => Err(StoreError::Corruption {
                message: format!("RepositoryView {id} resolved as another object kind"),
            }),
            None => Ok(None),
        }
    }

    fn object(&self, id: &StoreObjectId) -> Result<Option<CanonicalObject>> {
        if let Some(value) = self.overlay_object(id) {
            return Ok(Some(value.clone()));
        }
        if let (StoreObjectId::ChunkRaw(id), Some(source)) = (id, self.chunk_source) {
            if let Some(chunk) = source(id)? {
                if chunk.id() != id {
                    return Err(StoreError::Corruption {
                        message: "prepared chunk identity mismatch".into(),
                    });
                }
                return Ok(Some(CanonicalObject::ChunkRaw(chunk)));
            }
        }
        if let Some(value) = self
            .object_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(id).cloned())
        {
            return Ok(value);
        }
        let value = match id {
            StoreObjectId::ChunkRaw(id) => self
                .reader
                .get_chunk(id)
                .map(|value| value.map(CanonicalObject::ChunkRaw)),
            StoreObjectId::FileManifest(id) => self
                .reader
                .get_file_manifest(id)
                .map(|value| value.map(CanonicalObject::FileManifest)),
            StoreObjectId::Tree(id) => self
                .reader
                .get_tree(id)
                .map(|value| value.map(CanonicalObject::Tree)),
            StoreObjectId::Symlink(id) => self
                .reader
                .get_symlink(id)
                .map(|value| value.map(CanonicalObject::Symlink)),
            StoreObjectId::Tombstone(id) => self
                .reader
                .get_tombstone(id)
                .map(|value| value.map(CanonicalObject::Tombstone)),
            StoreObjectId::Step(id) => self
                .reader
                .get_step(id)
                .map(|value| value.map(CanonicalObject::Step)),
            StoreObjectId::RepositoryView(id) => self
                .reader
                .get_repository_view(id)
                .map(|value| value.map(CanonicalObject::RepositoryView)),
            StoreObjectId::Conflict(id) => self
                .reader
                .get_conflict(id)
                .map(|value| value.map(CanonicalObject::Conflict)),
            StoreObjectId::Product(id) => self
                .reader
                .get_product(id)
                .map(|value| value.map(CanonicalObject::Product)),
        }?;
        // Content must be released between chunks during streaming admission.
        // Cache only metadata; caching ChunkRaw would retain entire large files.
        if !matches!(id, StoreObjectId::ChunkRaw(_)) {
            if let Ok(mut cache) = self.object_cache.lock() {
                cache.insert(id.clone(), value.clone());
            }
        }
        Ok(value)
    }

    fn validate_admission(&self, space: &SpaceId, root: &StoreObjectId) -> Result<()> {
        if !self.space_exists(space) {
            if let Some(error) = self.take_failure() {
                return Err(error);
            }
            return Err(admission_rejected(root));
        }
        let mut pending = vec![root.clone()];
        let mut visited = BTreeSet::new();
        while let Some(object_id) = pending.pop() {
            if self
                .closed_admissions
                .lock()
                .is_ok_and(|closed| closed.contains(&(space.clone(), object_id.clone())))
            {
                continue;
            }
            if !visited.insert(object_id.clone()) {
                continue;
            }
            let visits = self.admission_visits.fetch_add(1, Ordering::Relaxed) + 1;
            require_budget("admission object visits", visits, MAX_ADMISSION_VISITS)?;
            let object = self
                .object(&object_id)?
                .ok_or_else(|| admission_rejected(&object_id))?;
            if let CanonicalObject::Product(product) = &object {
                self.validate_product_context(space, product)?;
            }
            if !self.object_admitted_result(&object_id, space)? {
                return Err(admission_rejected(&object_id));
            }
            let dependencies = admission_dependencies(&object);
            let edge_visits = self
                .admission_edge_visits
                .fetch_add(dependencies.len(), Ordering::Relaxed)
                .saturating_add(dependencies.len());
            require_budget(
                "admission edge visits",
                edge_visits,
                MAX_ADMISSION_EDGE_VISITS,
            )?;
            pending.extend(dependencies);
            if let CanonicalObject::Tree(tree) = &object {
                let file_ids: Vec<_> = tree
                    .entries()
                    .iter()
                    .filter_map(|entry| entry.target().file_id())
                    .collect();
                let edge_visits = self
                    .admission_edge_visits
                    .fetch_add(file_ids.len(), Ordering::Relaxed)
                    .saturating_add(file_ids.len());
                require_budget(
                    "admission edge visits",
                    edge_visits,
                    MAX_ADMISSION_EDGE_VISITS,
                )?;
                for file_id in file_ids {
                    let manifest = self
                        .admissible_manifest_for_file(file_id, space)?
                        .ok_or_else(|| admission_rejected(&object_id))?;
                    pending.push(StoreObjectId::FileManifest(manifest));
                }
            }
        }
        if let Ok(mut closed) = self.closed_admissions.lock() {
            closed.extend(visited.into_iter().map(|object| (space.clone(), object)));
        }
        Ok(())
    }

    /// A FileId in an admitted Tree is existentially closed by the
    /// lexicographically smallest verified manifest that is itself admitted to
    /// the same Space and whose complete chunk list is admitted there.
    fn admissible_manifest_for_file(
        &self,
        file_id: &FileId,
        space: &SpaceId,
    ) -> Result<Option<FileManifestId>> {
        let key = (space.clone(), file_id.clone());
        if let Some(cached) = self
            .file_manifest_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key).cloned())
        {
            return Ok(cached);
        }
        let mut candidates = self
            .manifests_by_file
            .get(file_id)
            .cloned()
            .unwrap_or_default();
        if let FileAvailability::Present(available) = self.reader.file_availability(file_id)? {
            candidates.extend(available.verified_manifest_ids().iter().cloned());
        }
        let mut selected = None;
        for manifest_id in candidates {
            let object_id = StoreObjectId::FileManifest(manifest_id.clone());
            if !self.object_admitted_result(&object_id, space)? {
                continue;
            }
            let Some(CanonicalObject::FileManifest(manifest)) = self.object(&object_id)? else {
                continue;
            };
            if manifest.file_id() != file_id
                || validate_file_manifest_admission(&manifest, self).is_err()
            {
                if let Some(error) = self.take_failure() {
                    return Err(error);
                }
                continue;
            }
            let mut chunks_closed = true;
            for chunk in manifest.chunks() {
                if !self.object_admitted_result(
                    &StoreObjectId::ChunkRaw(chunk.chunk_id().clone()),
                    space,
                )? {
                    chunks_closed = false;
                    break;
                }
            }
            if chunks_closed {
                selected = Some(manifest_id);
                break;
            }
        }
        if let Ok(mut cache) = self.file_manifest_cache.lock() {
            cache.insert(key, selected.clone());
        }
        Ok(selected)
    }

    fn operation_reaches_step_result(
        &self,
        operation: &OperationId,
        step: &StepId,
    ) -> Result<bool> {
        if let Some(result) = self
            .operation_step_closures
            .lock()
            .ok()
            .and_then(|cache| cache.get(operation).map(|closure| closure.contains(step)))
        {
            return Ok(result);
        }
        let overlay = match self.overlay_entity("Operation", operation.as_str()) {
            Some(EntityRecord::Operation(value)) => Some(value.clone()),
            _ => None,
        };
        let operation_value = match overlay {
            Some(value) => Some(value),
            None => self.reader.get_operation(operation)?,
        };
        let Some(operation_value) = operation_value else {
            return Ok(false);
        };
        let Some(view) = self.view(operation_value.resulting_view_id())? else {
            return Ok(false);
        };
        let heads = view.layer_heads().iter().flat_map(|heads| heads.step_ids());
        if heads.clone().all(|head| self.step_dag.contains(head)) {
            // Observations share the Operation's immutable resulting View. Walk
            // its union closure once, rather than charging every observation x
            // head pair. Both cached relations and graph work stay bounded.
            let query = self.ancestor_queries.fetch_add(1, Ordering::Relaxed) + 1;
            require_budget("ancestor queries", query, MAX_ANCESTOR_QUERIES)?;
            let mut closure = BTreeSet::new();
            let mut pending: Vec<_> = heads.cloned().collect();
            while let Some(id) = pending.pop() {
                if !closure.insert(id.clone()) {
                    continue;
                }
                let relations = self.ancestor_relations.fetch_add(1, Ordering::Relaxed) + 1;
                require_budget("ancestor relations", relations, MAX_ANCESTOR_RELATIONS)?;
                pending.extend(self.step_dag.parents(&id).map_err(graph_corruption)?);
            }
            let result = closure.contains(step);
            if let Ok(mut cache) = self.operation_step_closures.lock() {
                cache.insert(operation.clone(), closure);
            }
            return Ok(result);
        }
        for head in view.layer_heads().iter().flat_map(|heads| heads.step_ids()) {
            if head == step || self.is_step_ancestor_result(step, head)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn is_step_ancestor_result(&self, ancestor: &StepId, descendant: &StepId) -> Result<bool> {
        let key = (ancestor.clone(), descendant.clone());
        if let Some(value) = self
            .ancestor_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key).copied())
        {
            return Ok(value);
        }
        let query = self.ancestor_queries.fetch_add(1, Ordering::Relaxed) + 1;
        require_budget("ancestor queries", query, MAX_ANCESTOR_QUERIES)?;
        let value = if let (Some(ancestor_rank), Some(descendant_rank)) = (
            self.step_ranks.get(ancestor),
            self.step_ranks.get(descendant),
        ) {
            if ancestor_rank >= descendant_rank {
                false
            } else if let Some(value) = self
                .ancestor_sets
                .lock()
                .ok()
                .and_then(|cache| cache.get(descendant).map(|set| set.contains(ancestor)))
            {
                value
            } else {
                let ancestors: BTreeSet<_> = self
                    .step_dag
                    .ancestors(descendant)
                    .map_err(graph_corruption)?
                    .into_iter()
                    .collect();
                let relations = self
                    .ancestor_relations
                    .fetch_add(ancestors.len(), Ordering::Relaxed)
                    .saturating_add(ancestors.len());
                require_budget("ancestor relations", relations, MAX_ANCESTOR_RELATIONS)?;
                let value = ancestors.contains(ancestor);
                if let Ok(mut cache) = self.ancestor_sets.lock() {
                    cache.insert(descendant.clone(), ancestors);
                }
                value
            }
        } else {
            self.reader.is_step_ancestor(ancestor, descendant)?
        };
        if let Ok(mut cache) = self.ancestor_cache.lock() {
            cache.insert(key, value);
        }
        Ok(value)
    }

    fn operation(&self, id: &OperationId) -> Result<Option<Operation>> {
        if let Some(EntityRecord::Operation(value)) = self.overlay_entity("Operation", id.as_str())
        {
            return Ok(Some(value.clone()));
        }
        self.reader.get_operation(id)
    }

    fn operation_in_space_result(&self, id: &OperationId, space: &SpaceId) -> Result<bool> {
        Ok(self
            .operation(id)?
            .is_some_and(|value| value.space_id() == space))
    }

    fn repository_view_in_space_result(
        &self,
        id: &RepositoryViewId,
        space: &SpaceId,
    ) -> Result<bool> {
        let object_id = StoreObjectId::RepositoryView(id.clone());
        Ok(self
            .view(id)?
            .is_some_and(|value| value.space_id() == space)
            && self.object_admitted_result(&object_id, space)?)
    }
}

/// Pure static preflight, before CAS preparation as well as inside admission.
/// This does not relax Step/Operation DAG limits or validate authority/closure.
pub(crate) fn preflight_batch_budgets(batch: &WriteBatch) -> Result<()> {
    require_budget(
        "batch objects",
        batch.canonical_objects().len(),
        MAX_BATCH_OBJECTS,
    )?;
    require_budget(
        "batch entities",
        batch.entity_records().len(),
        MAX_BATCH_ENTITIES,
    )?;
    require_budget(
        "batch admissions",
        batch.admissions().len(),
        MAX_BATCH_ADMISSIONS,
    )?;
    require_budget(
        "batch mutations",
        batch.mutations().len(),
        MAX_BATCH_MUTATIONS,
    )?;
    let object_edges = batch
        .canonical_objects()
        .iter()
        .try_fold(0_usize, |count, object| {
            count.checked_add(admission_dependency_count(object)).ok_or(
                StoreError::AdmissionBudgetExceeded {
                    resource: "object graph edges",
                    limit: MAX_OBJECT_DEPENDENCIES,
                },
            )
        })?;
    require_budget("object graph edges", object_edges, MAX_OBJECT_DEPENDENCIES)
}

impl CompositeAdmissionResolver<'_> {
    fn object_admitted(&self, object: &StoreObjectId, space: &SpaceId) -> bool {
        let result = self.object_admitted_result(object, space);
        self.resolve(result, false)
    }

    fn object_admitted_result(&self, object: &StoreObjectId, space: &SpaceId) -> Result<bool> {
        let key = (space.clone(), object.clone());
        if let Some(value) = self
            .admitted_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key).copied())
        {
            return Ok(value);
        }
        if self.object(object)?.is_none() {
            return Ok(false);
        }
        let value = self.admissions.contains(&key)
            || self.reader.object_admitted_to_space(object, space)?;
        if let Ok(mut cache) = self.admitted_cache.lock() {
            cache.insert(key, value);
        }
        Ok(value)
    }
}

mod closure;
mod resolver;
use closure::*;
