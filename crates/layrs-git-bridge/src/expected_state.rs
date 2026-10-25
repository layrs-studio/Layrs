use super::{Result, invalid};
use layrs_client_core::native_scm::{NativeWorkspace, WorkspaceState};
use layrs_core::scm::OperationId;
use layrs_store::scm::{CurrentRepositoryView, ScmReader};
use layrs_store_local::anchored_export::NativeBridgeFiles;
use layrs_store_local::scm_v1::NativeScmReader;
use std::path::PathBuf;

/// A native review captures the exact target, Operation and immutable View.
/// This is deliberately not deserializable from an untrusted UI assertion.
#[derive(Debug, Clone)]
pub struct ExpectedBridgeState {
    pub(crate) state: WorkspaceState,
    pub(crate) repository_view: CurrentRepositoryView,
    pub(crate) operation_heads: Vec<OperationId>,
    store_root: PathBuf,
    store_identity: String,
    metadata_identity: String,
    files: NativeBridgeFiles,
}

impl ExpectedBridgeState {
    pub fn capture(workspace: &NativeWorkspace) -> Result<Self> {
        let state = workspace.state().map_err(invalid)?;
        let store = workspace.open_store().map_err(invalid)?;
        let reader = NativeScmReader::new(&store).map_err(invalid)?;
        Ok(Self {
            files: NativeBridgeFiles::capture(
                std::path::Path::new(state.worktree.checkout_root()),
                state.worktree.filesystem_identity(),
                workspace.store_root(),
                workspace.store_identity(),
            )
            .map_err(invalid)?,
            repository_view: reader
                .current_repository_view(state.space.id())
                .map_err(invalid)?
                .ok_or_else(|| invalid("missing native View"))?,
            operation_heads: reader.operation_heads(state.space.id()).map_err(invalid)?,
            state,
            store_root: workspace.store_root().to_path_buf(),
            store_identity: workspace.store_identity().into(),
            metadata_identity: workspace.metadata_identity().into(),
        })
    }

    pub fn state(&self) -> &WorkspaceState {
        &self.state
    }

    pub fn validate(&self, workspace: &NativeWorkspace) -> Result<()> {
        self.files.verify().map_err(invalid)?;
        // Reopening also checks the persisted directory identity and locator.
        let reopened =
            NativeWorkspace::open(self.state.worktree.checkout_root()).map_err(invalid)?;
        let actual = workspace.state().map_err(invalid)?;
        let store = workspace.open_store().map_err(invalid)?;
        let reader = NativeScmReader::new(&store).map_err(invalid)?;
        if workspace.store_root() != self.store_root
            || workspace.store_identity() != self.store_identity
            || reopened.store_identity() != self.store_identity
            || workspace.metadata_identity() != self.metadata_identity
            || reopened.metadata_identity() != self.metadata_identity
            || reopened.store_root() != self.store_root
            || reopened.worktree_id() != self.state.worktree.id()
            || actual.worktree != self.state.worktree
            || actual.space != self.state.space
            || actual.operation != self.state.operation
            || actual.view != self.state.view
            || actual.layers != self.state.layers
            || reader
                .current_repository_view(self.state.space.id())
                .map_err(invalid)?
                .as_ref()
                != Some(&self.repository_view)
            || reader
                .operation_heads(self.state.space.id())
                .map_err(invalid)?
                != self.operation_heads
        {
            return Err(invalid(
                "Git bridge review is stale: target Worktree, Operation or View changed",
            ));
        }
        Ok(())
    }

    pub(crate) fn apply_batch(
        &self,
        workspace: &NativeWorkspace,
        batch: &layrs_store::scm::WriteBatch,
    ) -> Result<layrs_store::scm::ApplyBatchOutcome> {
        layrs_store_local::scm_v1::NativeScmRepository::new(workspace.store_root())
            .with_store_identity(&self.store_identity)
            .with_metadata_identity(&self.metadata_identity)
            .with_store_id(self.state.worktree.store_id().clone())
            .apply_batch_guarded(batch, || self.accept_guard())
            .map_err(invalid)
    }
    pub(crate) fn accept_guard(&self) -> layrs_store::scm::Result<()> {
        self.files
            .verify()
            .map_err(|error| layrs_store::scm::StoreError::Backend {
                message: error.to_string(),
            })
    }
}
