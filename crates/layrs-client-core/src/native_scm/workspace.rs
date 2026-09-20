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
    /// Creates a native .layrs Store only in an existing checkout. Unsupported
    /// metadata is refused by init; it is never migrated or removed.
    pub fn initialize(root: impl AsRef<Path>, name: &str) -> Result<Self> {
        let root = std::path::absolute(root)?;
        let (identity, snapshot) = super::filesystem::capture_checkout_streaming(
            &root,
            None,
            SnapshotLimits::streaming(),
            &BTreeSet::new(),
        )?;
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
            WorktreeId::parse(format!("worktree:{suffix}"))?,
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
        Ok(Self {
            client,
            worktree_id: worktree.id().clone(),
        })
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let store_root = root.as_ref().join(".layrs");
        let store = scm_v1::open(&store_root)?;
        let suffix = store
            .store_id()
            .as_str()
            .strip_prefix("store:")
            .ok_or(NativeClientError::InvalidState("invalid Store ID"))?;
        let worktree_id = WorktreeId::parse(format!("worktree:{suffix}"))?;
        drop(store);
        let result = Self {
            client: NativeClient::open(store_root)?,
            worktree_id,
        };
        let state = result.state()?;
        // Opening the workspace is read-only and checks persisted directory identity.
        layrs_store_local::scm_v1::checkout::CheckoutReader::open(
            root,
            Some(state.worktree.filesystem_identity()),
        )?;
        Ok(result)
    }

    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let start = std::path::absolute(start)?;
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
    pub fn checkout_root(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(self.state()?.worktree.checkout_root()))
    }

    pub fn state(&self) -> Result<WorkspaceState> {
        let store = scm_v1::open(&self.client.root)?;
        let reader = NativeScmReader::new(&store)?;
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
        let plan = self.client.prepare_disk_capture_with_reason(
            &self.worktree_id,
            SnapshotLimits::streaming(),
            MutationStamp::fresh()?,
            reason,
        )?;
        if let CapturePlan::Change(change) = &plan {
            self.client.accept(change)?;
        }
        Ok(plan)
    }

    pub fn is_dirty(&self) -> Result<bool> {
        Ok(!self.working_diff()?.is_empty())
    }

    pub fn needs_repair(&self) -> Result<bool> {
        let store = scm_v1::open(self.store_root())?;
        Ok(scm_v1::materialize::recovery_pending(
            &store,
            self.worktree_id(),
        )?)
    }

    /// Walks Operation ancestry, preserving concurrent parents up to the requested
    /// display limit. Missing/cross-Space ancestors remain explicit errors.
    pub fn operations(&self, limit: usize) -> Result<Vec<Operation>> {
        let store = scm_v1::open(&self.client.root)?;
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
