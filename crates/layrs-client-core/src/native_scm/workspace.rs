//! User-facing native local workflow shared by CLI and Desktop.
use super::Result;
use super::*;
use layrs_core::scm::*;
use layrs_store::scm::{ScmReader, TransactionId};
use layrs_store_local::scm_v1::{self, NativeScmReader};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub struct NativeWorkspace {
    pub(super) client: NativeClient,
    pub(super) worktree_id: WorktreeId,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub worktree: Worktree,
    pub space: Space,
    pub operation: Operation,
    pub view: RepositoryView,
    pub layers: Vec<Layer>,
}

impl MutationStamp {
    pub fn fresh() -> Result<Self> {
        let id = uuid::Uuid::new_v4();
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| NativeClientError::InvalidState("could not format current time"))?;
        Ok(Self {
            transaction_id: TransactionId::new(format!("transaction:{id}"))?,
            operation_id: OperationId::parse(format!("operation:{id}"))?,
            observation_id: ObservationId::parse(format!("observation:{id}"))?,
            timestamp: Timestamp::parse(now)?,
        })
    }
}

impl NativeWorkspace {
    /// Resolves registered checkout identity without loading Operation/View CAS,
    /// including while a metadata-compaction journal fences normal opens.
    pub fn resolve_store_for_audit(root: impl AsRef<Path>) -> Result<PathBuf> {
        Ok(scm_v1::worktree_audit::resolve_store_for_audit(
            root.as_ref(),
        )?)
    }
    pub fn audit_worktree_store(
        root: impl AsRef<Path>,
    ) -> Result<scm_v1::worktree_audit::AuditedWorktreeStore> {
        Ok(scm_v1::worktree_audit::audit_worktree_store(root.as_ref())?)
    }

    /// Creates a native .layrs Store only in an existing checkout. Unsupported
    /// metadata is refused by init; it is never migrated or removed.
    pub fn initialize(root: impl AsRef<Path>, name: &str) -> Result<Self> {
        let root = std::path::absolute(root)?;
        let (identity, snapshot, proven, entries) =
            super::filesystem::capture_checkout_streaming_indexed(
                &root,
                SnapshotLimits::streaming(),
            )?;
        let root_tree = snapshot.root_tree_id().clone();
        let stamp = MutationStamp::fresh()?;
        let suffix = uuid::Uuid::new_v4().to_string();
        let store_id = StoreId::parse(format!("store:{suffix}"))?;
        let space = Space::new(
            SpaceId::parse(format!("space:{suffix}"))?,
            name,
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        let layer = Layer::new(
            LayerId::parse(format!("layer:{suffix}"))?,
            space.id().clone(),
            "main",
            LayerKind::Base,
            LayerStatus::Active,
            vec![],
            LayerSettings::new(true, 500)?,
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        let worktree = Worktree::new(
            WorktreeId::parse(format!("worktree:{}", uuid::Uuid::new_v4()))?,
            store_id.clone(),
            space.id().clone(),
            stamp.operation_id.clone(),
            root.to_str().ok_or(NativeClientError::InvalidState(
                "checkout root is not UTF-8",
            ))?,
            identity,
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        let change = prepare_initialize(space, layer, worktree.clone(), snapshot, stamp)?;
        drop(scm_v1::init(root.join(".layrs"), store_id)?);
        let client = NativeClient::open(root.join(".layrs"))?;
        client.accept(&change)?;
        // The folder's change index (ADR 0025) starts with what this first
        // capture proved. A cache: on failure the next capture reads all.
        let store_root = root.join(".layrs");
        if scm_v1::change_index::ChangeIndex::enable(&store_root, worktree.id(), &root).is_ok() {
            if let Some(mut index) =
                scm_v1::change_index::ChangeIndex::load(&store_root, worktree.id())
            {
                for (path, entry) in proven {
                    index.insert(path, entry);
                }
                let _ = index.save(&store_root);
            }
        }
        // The first Step's projection, so the first `status` reads no Tree.
        scm_v1::projection_cache::save(&store_root, &root_tree, &root_tree, &entries);
        Ok(Self {
            client,
            worktree_id: worktree.id().clone(),
        })
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let checkout = scm_v1::checkout::CheckoutReader::open(root, None)?;
        let root = checkout.root();
        let marker = std::fs::symlink_metadata(root.join(".layrs"))?;
        let locator = if marker.is_dir() && !marker.file_type().is_symlink() {
            None
        } else {
            Some(scm_v1::worktrees::read_locator(root)?)
        };
        let store_root = locator
            .as_ref()
            .map(|l| l.store_root.clone())
            .unwrap_or_else(|| root.join(".layrs"));
        let (client, store) = NativeClient::open_with_store(&store_root)?;
        if locator.as_ref().is_some_and(|locator| {
            locator.store_filesystem_identity != client.store_identity
                || locator.store_metadata_identity != client.metadata_identity
        }) {
            return Err(NativeClientError::InvalidState(
                "Worktree locator Store identity changed",
            ));
        }
        let candidates = scm_v1::worktrees::registered_worktrees(&store)?
            .into_iter()
            .filter(|w| w.filesystem_identity() == checkout.identity())
            .collect::<Vec<_>>();
        let [worktree] = candidates.as_slice() else {
            return Err(NativeClientError::InvalidState(
                "checkout does not identify exactly one registered Worktree",
            ));
        };
        scm_v1::checkout::CheckoutReader::open(
            worktree.checkout_root(),
            Some(worktree.filesystem_identity()),
        )?;
        if worktree.store_id() != store.store_id()
            || locator.as_ref().is_some_and(|locator| {
                &locator.store_id != store.store_id()
                    || &locator.worktree_id != worktree.id()
                    || locator.filesystem_identity != worktree.filesystem_identity()
            })
        {
            return Err(NativeClientError::InvalidState(
                "Worktree locator binding mismatch",
            ));
        }
        let worktree_id = worktree.id().clone();
        let result = Self {
            client,
            worktree_id,
        };
        // Opening validates the Workspace state (a corrupt selected View
        // refuses here), on the Store capability already held.
        let state = result.state_with(&store)?;
        drop(store);
        // Opening the workspace is read-only and checks persisted directory identity.
        layrs_store_local::scm_v1::checkout::CheckoutReader::open(
            root,
            Some(state.worktree.filesystem_identity()),
        )?;
        Ok(result)
    }

    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let start = std::path::absolute(start)?;
        let directory = if std::fs::symlink_metadata(&start)?.is_file() {
            start
                .parent()
                .ok_or(NativeClientError::InvalidState("missing parent directory"))?
        } else {
            &start
        };
        scm_v1::checkout::CheckoutReader::open(directory, None)?;
        for root in start.ancestors() {
            if root.join(".layrs").try_exists()? {
                return Self::open(root);
            }
        }
        Err(NativeClientError::Missing("checkout .layrs directory"))
    }

    pub fn client(&self) -> &NativeClient {
        &self.client
    }
    pub fn worktree_id(&self) -> &WorktreeId {
        &self.worktree_id
    }
    pub fn store_root(&self) -> &Path {
        &self.client.root
    }
    pub fn store_identity(&self) -> &str {
        &self.client.store_identity
    }
    pub fn metadata_identity(&self) -> &str {
        &self.client.metadata_identity
    }
    pub fn open_store(&self) -> Result<scm_v1::NativeStoreOpen> {
        self.client.open_store()
    }
    pub fn checkout_root(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(self.state()?.worktree.checkout_root()))
    }

    pub fn state(&self) -> Result<WorkspaceState> {
        self.state_with(&self.open_store()?)
    }

    /// `state` on an already open Store (one session for several reads).
    pub fn state_with(&self, store: &scm_v1::NativeStoreOpen) -> Result<WorkspaceState> {
        let reader = NativeScmReader::new(store)?;
        let worktree = reader
            .get_worktree(&self.worktree_id)?
            .ok_or(NativeClientError::Missing("Worktree"))?;
        let space = reader
            .get_space(worktree.space_id())?
            .ok_or(NativeClientError::Missing("Space"))?;
        let operation = reader
            .get_operation(worktree.active_operation_id())?
            .ok_or(NativeClientError::Missing("Operation"))?;
        let view = reader
            .get_repository_view(operation.resulting_view_id())?
            .ok_or(NativeClientError::Missing("RepositoryView"))?;
        let mut layers = Vec::new();
        for heads in view.layer_heads() {
            layers.push(
                reader
                    .get_layer(heads.layer_id())?
                    .ok_or(NativeClientError::Missing("Layer"))?,
            );
        }
        Ok(WorkspaceState {
            worktree,
            space,
            operation,
            view,
            layers,
        })
    }

    pub fn capture(&self) -> Result<CapturePlan> {
        self.capture_with_reason(CaptureReason::ManualSnapshot)
    }

    pub fn capture_with_reason(&self, reason: CaptureReason) -> Result<CapturePlan> {
        let index = self.change_index();
        self.capture_using(reason, index)
    }

    /// Every physical folder, the project's own included, keeps a change
    /// index (ADR 0025, decision of 4 November 2026); one without yet gets an
    /// empty one, which its next capture fills. A cache: if it cannot be
    /// created, captures read every file as before.
    fn change_index(&self) -> Option<scm_v1::change_index::ChangeIndex> {
        use scm_v1::change_index::ChangeIndex;
        if !ChangeIndex::is_enabled(self.store_root(), &self.worktree_id) {
            let root = self.checkout_root().ok()?;
            ChangeIndex::enable(self.store_root(), &self.worktree_id, &root).ok()?;
        }
        ChangeIndex::load(self.store_root(), &self.worktree_id)
    }

    /// Reads every file even when this Worktree has a change index (ADR 0025):
    /// the full content verification required before deleting a Room. The
    /// index is rebuilt from what this capture proves.
    pub fn capture_verifying_all(&self, reason: CaptureReason) -> Result<CapturePlan> {
        let index = scm_v1::change_index::ChangeIndex::load(self.store_root(), &self.worktree_id)
            .map(|mut index| {
                index.clear();
                index
            });
        self.capture_using_passes(reason, index, false)
    }

    fn capture_using(
        &self,
        reason: CaptureReason,
        index: Option<scm_v1::change_index::ChangeIndex>,
    ) -> Result<CapturePlan> {
        self.capture_using_passes(reason, index, true)
    }

    /// single_pass for a capture that only records content (council r5,
    /// D4); the verification before a deletion keeps two equal passes.
    fn capture_using_passes(
        &self,
        reason: CaptureReason,
        index: Option<scm_v1::change_index::ChangeIndex>,
        single_pass: bool,
    ) -> Result<CapturePlan> {
        let _total = layrs_store_local::perf::span("capture.total");
        for _ in 0..8 {
            let attempt = (|| {
                let (plan, next) = {
                    let _span = layrs_store_local::perf::span("capture.prepare");
                    match &index {
                        Some(index) => {
                            let base = self.client.capture_base(&self.worktree_id)?;
                            let (snapshot, next) = self.capture_projection_indexed(
                                &self.state()?,
                                SnapshotLimits::streaming(),
                                index,
                                single_pass,
                            )?;
                            let plan = self.client.prepare_capture_with_reason(
                                &base,
                                snapshot,
                                MutationStamp::fresh()?,
                                reason,
                            )?;
                            (plan, Some(next))
                        }
                        None => (
                            self.client.prepare_disk_capture_with_reason(
                                &self.worktree_id,
                                SnapshotLimits::streaming(),
                                MutationStamp::fresh()?,
                                reason,
                            )?,
                            None,
                        ),
                    }
                };
                if let CapturePlan::Change(change) = &plan {
                    let _span = layrs_store_local::perf::span("capture.accept");
                    self.client.accept(change)?;
                }
                Ok((plan, next))
            })();
            match attempt {
                Err(
                    NativeClientError::StaleCapture
                    | NativeClientError::Store(layrs_store::scm::StoreError::CasMismatch { .. }),
                ) => continue,
                Ok((plan, next)) => {
                    // Recorded only after acceptance; a cache, so a failed
                    // write only makes the next capture read more. An idle
                    // capture that proved nothing new writes nothing.
                    let unchanged = match (&next, &index) {
                        (Some(next), Some(index)) => next.same_entries(index),
                        _ => false,
                    };
                    if let Some(next) = next.filter(|_| !unchanged) {
                        if next.save(self.store_root()).is_err() {
                            layrs_store_local::perf::count("capture.index_unsaved", 1);
                        }
                    }
                    return Ok(plan);
                }
                Err(error) => return Err(error),
            }
        }
        Err(NativeClientError::StaleCapture)
    }

    pub fn is_dirty(&self) -> Result<bool> {
        self.working_tree_changed(true)
    }

    /// As `is_dirty`, reading every file instead of trusting the change index.
    pub fn is_dirty_verifying(&self) -> Result<bool> {
        self.working_tree_changed(false)
    }

    pub fn needs_repair(&self) -> Result<bool> {
        let store = self.open_store()?;
        Ok(scm_v1::materialize::recovery_pending(
            &store,
            self.worktree_id(),
        )?)
    }

    /// Walks Operation ancestry, preserving concurrent parents up to the requested
    /// display limit. Missing/cross-Space ancestors remain explicit errors.
    pub fn operations(&self, limit: usize) -> Result<Vec<Operation>> {
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let worktree = reader
            .get_worktree(&self.worktree_id)?
            .ok_or(NativeClientError::Missing("Worktree"))?;
        let mut pending = vec![worktree.active_operation_id().clone()];
        let mut visited = BTreeSet::new();
        let mut result = Vec::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            if result.len() >= limit.min(100_000) {
                break;
            }
            let operation = reader
                .get_operation(&id)?
                .ok_or(NativeClientError::Missing("Operation ancestor"))?;
            if operation.space_id() != worktree.space_id() {
                return Err(NativeClientError::InvalidState(
                    "Operation belongs to another Space",
                ));
            }
            pending.extend(operation.parent_operation_ids().iter().rev().cloned());
            result.push(operation);
        }
        Ok(result)
    }
}
