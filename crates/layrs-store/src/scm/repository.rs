use super::{
    CanonicalConflict, FileAvailability, RequestDigest, Result, StoreObjectId, WriteBatch,
};
use layrs_core::scm::{
    ActorId, Chunk, ChunkId, ConflictId, DeviceId, FileId, FileManifest, FileManifestId, Layer,
    LayerId, Marker, MarkerId, ObservationId, Operation, OperationId, ProofId, Release, ReleaseId,
    ReleasePublication, ReleasePublicationId, RepositoryView, RepositoryViewId, Space, SpaceId,
    Step, StepId, StepObservation, StoreId, Symlink, SymlinkId, Tombstone, TombstoneId, Tree,
    TreeId, Worktree, WorktreeId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRepositoryView {
    generation: u64,
    view_id: Option<RepositoryViewId>,
}

impl CurrentRepositoryView {
    pub fn new(generation: u64, view_id: Option<RepositoryViewId>) -> Self {
        Self {
            generation,
            view_id,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn view_id(&self) -> Option<&RepositoryViewId> {
        self.view_id.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyBatchOutcome {
    Applied { request_digest: RequestDigest },
    AlreadyApplied { request_digest: RequestDigest },
}

impl ApplyBatchOutcome {
    pub const fn request_digest(self) -> RequestDigest {
        match self {
            Self::Applied { request_digest } | Self::AlreadyApplied { request_digest } => {
                request_digest
            }
        }
    }

    pub const fn was_replay(self) -> bool {
        matches!(self, Self::AlreadyApplied { .. })
    }
}

/// Synchronous, typed read contract for native SCM objects and entities.
///
/// Boolean membership methods correspond to the contextual questions required
/// by `layrs_core::scm::AdmissionResolver`, but remain fallible because a
/// persistent backend must not turn I/O or corruption into a false negative.
pub trait ScmReader: Send + Sync {
    fn get_chunk(&self, id: &ChunkId) -> Result<Option<Chunk>>;
    fn get_file_manifest(&self, id: &FileManifestId) -> Result<Option<FileManifest>>;
    fn get_tree(&self, id: &TreeId) -> Result<Option<Tree>>;
    fn get_symlink(&self, id: &SymlinkId) -> Result<Option<Symlink>>;
    fn get_tombstone(&self, id: &TombstoneId) -> Result<Option<Tombstone>>;
    fn get_step(&self, id: &StepId) -> Result<Option<Step>>;
    fn get_repository_view(&self, id: &RepositoryViewId) -> Result<Option<RepositoryView>>;
    fn get_conflict(&self, id: &ConflictId) -> Result<Option<CanonicalConflict>>;
    fn get_product(
        &self,
        _id: &layrs_core::scm::ObjectId,
    ) -> Result<Option<super::CanonicalProduct>> {
        Err(super::StoreError::Backend {
            message: "Product reads unsupported by this backend".into(),
        })
    }
    fn products_in_space(&self, _space: &SpaceId) -> Result<Vec<layrs_core::scm::ObjectId>> {
        Err(super::StoreError::Backend {
            message: "Product listing unsupported by this backend".into(),
        })
    }

    fn get_space(&self, id: &SpaceId) -> Result<Option<Space>>;
    fn get_layer(&self, id: &LayerId) -> Result<Option<Layer>>;
    fn get_step_observation(&self, id: &ObservationId) -> Result<Option<StepObservation>>;
    fn get_operation(&self, id: &OperationId) -> Result<Option<Operation>>;
    fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>>;
    fn get_marker(&self, id: &MarkerId) -> Result<Option<Marker>>;
    fn get_release(&self, id: &ReleaseId) -> Result<Option<Release>>;
    fn get_release_publication(
        &self,
        id: &ReleasePublicationId,
    ) -> Result<Option<ReleasePublication>>;

    fn file_availability(&self, id: &FileId) -> Result<FileAvailability>;
    fn object_admitted_to_space(
        &self,
        object_id: &StoreObjectId,
        space_id: &SpaceId,
    ) -> Result<bool>;
    fn is_step_ancestor(&self, ancestor: &StepId, descendant: &StepId) -> Result<bool>;
    fn operation_reaches_step(&self, operation_id: &OperationId, step_id: &StepId) -> Result<bool>;

    fn store_exists(&self, id: &StoreId) -> Result<bool>;
    fn conflict_exists(&self, id: &ConflictId) -> Result<bool>;
    fn proof_in_space(&self, id: &ProofId, space_id: &SpaceId) -> Result<bool>;
    fn actor_exists(&self, id: &ActorId) -> Result<bool>;
    fn device_exists(&self, id: &DeviceId) -> Result<bool>;

    fn current_repository_view(&self, space_id: &SpaceId) -> Result<Option<CurrentRepositoryView>>;
    fn operation_heads(&self, space_id: &SpaceId) -> Result<Vec<OperationId>>;
    /// Returns the unique authenticated Initialize Operation in a Space,
    /// independent of current-head reachability. Multiple durable initializers
    /// are store corruption, not an arbitrary selection.
    fn initializer_in_space(&self, space_id: &SpaceId) -> Result<Option<Operation>>;
    fn worktree_active_operation(&self, worktree_id: &WorktreeId) -> Result<Option<OperationId>>;
}

/// Atomic write boundary implemented by a concrete SCM repository backend.
pub trait ScmRepository: ScmReader {
    /// Applies the complete batch or leaves the repository byte-for-byte at
    /// its previous logical state.
    ///
    /// Implementations validate existence, closure, same-Space admission, and
    /// every CAS precondition inside the write transaction. Before mutating
    /// heads they build the candidate Operation DAG, use
    /// `OperationDag::insert_integration` for `IntegrateOperations`, and verify
    /// that the candidate's complete Space head set exactly equals the CAS
    /// `new_head_ids`. Historical Operation imports without a heads CAS are
    /// stored without changing current heads.
    /// A single retained non-head parent is allowed only for a CAS-bound
    /// durable Worktree's current Operation, or for an explicitly bound new
    /// Worktree registration from the durable source Worktree's own ancestry.
    /// Backend validation must reject same-batch, passive-only, foreign-Space
    /// and unbound parent claims and preserve every unrelated frontier head.
    /// A replay of the
    /// same transaction ID and request digest returns `AlreadyApplied`; reuse
    /// of the transaction ID with another digest returns
    /// `transaction_replay_mismatch` and applies nothing.
    fn apply_batch(&mut self, batch: &WriteBatch) -> Result<ApplyBatchOutcome>;
}
