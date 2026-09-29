//! Transactional reader delegates through the same validated reads.
use super::*;
impl ScmReader for NativeTransactionalScmReader<'_> {
    fn products_in_space(&self, space: &SpaceId) -> Result<Vec<layrs_core::scm::ObjectId>> {
        self.active_reader()?.products_in_space(space)
    }
    fn get_product(
        &self,
        id: &layrs_core::scm::ObjectId,
    ) -> Result<Option<layrs_store::scm::CanonicalProduct>> {
        self.active_reader()?.get_product(id)
    }
    fn get_chunk(&self, id: &ChunkId) -> Result<Option<Chunk>> {
        self.active_reader()?.get_chunk(id)
    }

    fn get_file_manifest(&self, id: &FileManifestId) -> Result<Option<FileManifest>> {
        self.active_reader()?.get_file_manifest(id)
    }

    fn get_tree(&self, id: &TreeId) -> Result<Option<Tree>> {
        self.active_reader()?.get_tree(id)
    }

    fn get_symlink(&self, id: &SymlinkId) -> Result<Option<Symlink>> {
        self.active_reader()?.get_symlink(id)
    }

    fn get_tombstone(&self, id: &TombstoneId) -> Result<Option<Tombstone>> {
        self.active_reader()?.get_tombstone(id)
    }

    fn get_step(&self, id: &StepId) -> Result<Option<Step>> {
        self.active_reader()?.get_step(id)
    }

    fn get_repository_view(&self, id: &RepositoryViewId) -> Result<Option<RepositoryView>> {
        self.active_reader()?.get_repository_view(id)
    }

    fn get_conflict(&self, id: &ConflictId) -> Result<Option<CanonicalConflict>> {
        self.active_reader()?.get_conflict(id)
    }

    fn get_space(&self, id: &SpaceId) -> Result<Option<Space>> {
        self.active_reader()?.get_space(id)
    }

    fn get_layer(&self, id: &LayerId) -> Result<Option<Layer>> {
        self.active_reader()?.get_layer(id)
    }

    fn get_step_observation(&self, id: &ObservationId) -> Result<Option<StepObservation>> {
        self.active_reader()?.get_step_observation(id)
    }

    fn get_operation(&self, id: &OperationId) -> Result<Option<Operation>> {
        self.active_reader()?.get_operation(id)
    }

    fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>> {
        self.active_reader()?.get_worktree(id)
    }

    fn get_marker(&self, id: &MarkerId) -> Result<Option<Marker>> {
        self.active_reader()?.get_marker(id)
    }

    fn get_release(&self, id: &ReleaseId) -> Result<Option<Release>> {
        self.active_reader()?.get_release(id)
    }

    fn get_release_publication(
        &self,
        id: &ReleasePublicationId,
    ) -> Result<Option<ReleasePublication>> {
        self.active_reader()?.get_release_publication(id)
    }

    fn file_availability(&self, id: &FileId) -> Result<FileAvailability> {
        self.active_reader()?.file_availability(id)
    }

    fn object_admitted_to_space(
        &self,
        object_id: &StoreObjectId,
        space_id: &SpaceId,
    ) -> Result<bool> {
        self.active_reader()?
            .object_admitted_to_space(object_id, space_id)
    }

    fn is_step_ancestor(&self, ancestor: &StepId, descendant: &StepId) -> Result<bool> {
        self.active_reader()?.is_step_ancestor(ancestor, descendant)
    }

    fn operation_reaches_step(&self, operation_id: &OperationId, step_id: &StepId) -> Result<bool> {
        self.active_reader()?
            .operation_reaches_step(operation_id, step_id)
    }

    fn store_exists(&self, id: &StoreId) -> Result<bool> {
        self.active_reader()?.store_exists(id)
    }

    fn conflict_exists(&self, id: &ConflictId) -> Result<bool> {
        self.active_reader()?.conflict_exists(id)
    }

    fn proof_in_space(&self, id: &ProofId, space_id: &SpaceId) -> Result<bool> {
        self.active_reader()?.proof_in_space(id, space_id)
    }

    fn actor_exists(&self, id: &ActorId) -> Result<bool> {
        self.active_reader()?.actor_exists(id)
    }

    fn device_exists(&self, id: &DeviceId) -> Result<bool> {
        self.active_reader()?.device_exists(id)
    }

    fn current_repository_view(&self, space_id: &SpaceId) -> Result<Option<CurrentRepositoryView>> {
        self.active_reader()?.current_repository_view(space_id)
    }

    fn operation_heads(&self, space_id: &SpaceId) -> Result<Vec<OperationId>> {
        self.active_reader()?.operation_heads(space_id)
    }

    fn initializer_in_space(&self, space_id: &SpaceId) -> Result<Option<Operation>> {
        self.active_reader()?.initializer_in_space(space_id)
    }

    fn worktree_active_operation(&self, worktree_id: &WorktreeId) -> Result<Option<OperationId>> {
        self.active_reader()?.worktree_active_operation(worktree_id)
    }
}
