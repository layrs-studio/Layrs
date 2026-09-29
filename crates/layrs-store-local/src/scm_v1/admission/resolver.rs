//! AdmissionResolver queries against the validated composite overlay.
use super::*;
impl AdmissionResolver for CompositeAdmissionResolver<'_> {
    fn chunk_bytes(&self, id: &ChunkId) -> Option<Vec<u8>> {
        let result = self.object(&StoreObjectId::ChunkRaw(id.clone()));
        self.resolve(result, None).and_then(|object| match object {
            CanonicalObject::ChunkRaw(value) => Some(value.bytes().to_vec()),
            _ => None,
        })
    }

    fn tree_exists(&self, id: &TreeId) -> bool {
        let result = self.object(&StoreObjectId::Tree(id.clone()));
        matches!(self.resolve(result, None), Some(CanonicalObject::Tree(_)))
    }

    fn tree_admitted_to_space(&self, id: &TreeId, space: &SpaceId) -> bool {
        self.object_admitted(&StoreObjectId::Tree(id.clone()), space)
    }

    fn conflict_exists(&self, id: &ConflictId) -> bool {
        let result = self.object(&StoreObjectId::Conflict(id.clone()));
        matches!(
            self.resolve(result, None),
            Some(CanonicalObject::Conflict(_))
        )
    }

    fn symlink_exists(&self, id: &SymlinkId) -> bool {
        let result = self.object(&StoreObjectId::Symlink(id.clone()));
        matches!(
            self.resolve(result, None),
            Some(CanonicalObject::Symlink(_))
        )
    }

    fn tombstone_exists(&self, id: &TombstoneId) -> bool {
        let result = self.object(&StoreObjectId::Tombstone(id.clone()));
        matches!(
            self.resolve(result, None),
            Some(CanonicalObject::Tombstone(_))
        )
    }

    fn file_has_verified_manifest(&self, id: &FileId) -> bool {
        if self.manifests_by_file.contains_key(id) {
            return true;
        }
        if let Some(value) = self
            .verified_files
            .lock()
            .ok()
            .and_then(|cache| cache.get(id).copied())
        {
            return value;
        }
        let value = matches!(
            self.resolve(self.reader.file_availability(id), FileAvailability::Absent),
            FileAvailability::Present(_)
        );
        if let Ok(mut cache) = self.verified_files.lock() {
            cache.insert(id.clone(), value);
        }
        value
    }

    fn step_exists(&self, id: &StepId) -> bool {
        if self.step_dag.contains(id) {
            return true;
        }
        let result = self.object(&StoreObjectId::Step(id.clone()));
        matches!(self.resolve(result, None), Some(CanonicalObject::Step(_)))
    }

    fn step_admitted_to_space(&self, id: &StepId, space: &SpaceId) -> bool {
        self.object_admitted(&StoreObjectId::Step(id.clone()), space)
    }

    fn is_step_ancestor(&self, ancestor: &StepId, descendant: &StepId) -> bool {
        let result = self.is_step_ancestor_result(ancestor, descendant);
        self.resolve(result, false)
    }

    fn space_exists(&self, id: &SpaceId) -> bool {
        self.overlay_entity("Space", id.as_str()).is_some()
            || self.resolve(self.reader.get_space(id), None).is_some()
    }

    fn store_exists(&self, id: &StoreId) -> bool {
        self.resolve(self.reader.store_exists(id), false)
    }

    fn layer_in_space(&self, id: &LayerId, space: &SpaceId) -> bool {
        if let Some(EntityRecord::Layer(value)) = self.overlay_entity("Layer", id.as_str()) {
            return value.space_id() == space;
        }
        self.resolve(self.reader.get_layer(id), None)
            .is_some_and(|value| value.space_id() == space)
    }

    fn marker_in_space(&self, id: &MarkerId, space: &SpaceId) -> bool {
        if let Some(EntityRecord::Marker(value)) = self.overlay_entity("Marker", id.as_str()) {
            return value.space_id() == space;
        }
        self.resolve(self.reader.get_marker(id), None)
            .is_some_and(|value| value.space_id() == space)
    }

    fn release_in_space(&self, id: &ReleaseId, space: &SpaceId) -> bool {
        if let Some(EntityRecord::Release(value)) = self.overlay_entity("Release", id.as_str()) {
            return value.space_id() == space;
        }
        self.resolve(self.reader.get_release(id), None)
            .is_some_and(|value| value.space_id() == space)
    }

    fn worktree_in_space(&self, id: &WorktreeId, space: &SpaceId) -> bool {
        if let Some(EntityRecord::Worktree(value)) = self.overlay_entity("Worktree", id.as_str()) {
            return value.space_id() == space;
        }
        self.resolve(self.reader.get_worktree(id), None)
            .is_some_and(|value| value.space_id() == space)
    }

    fn worktree_exists(&self, id: &WorktreeId) -> bool {
        self.overlay_entity("Worktree", id.as_str()).is_some()
            || self.resolve(self.reader.get_worktree(id), None).is_some()
    }

    fn operation_in_space(&self, id: &OperationId, space: &SpaceId) -> bool {
        let result = self.operation_in_space_result(id, space);
        self.resolve(result, false)
    }

    fn operation_exists(&self, id: &OperationId) -> bool {
        self.overlay_entity("Operation", id.as_str()).is_some()
            || self.operation_dag.contains(id)
            || self.resolve(self.reader.get_operation(id), None).is_some()
    }

    fn operation_reaches_step(&self, operation: &OperationId, step: &StepId) -> bool {
        let result = self.operation_reaches_step_result(operation, step);
        self.resolve(result, false)
    }

    fn repository_view_in_space(&self, id: &RepositoryViewId, space: &SpaceId) -> bool {
        let result = self.repository_view_in_space_result(id, space);
        self.resolve(result, false)
    }

    fn proof_in_space(&self, id: &ProofId, space: &SpaceId) -> bool {
        self.resolve(self.reader.proof_in_space(id, space), false)
    }

    fn actor_exists(&self, id: &ActorId) -> bool {
        self.resolve(self.reader.actor_exists(id), false)
    }

    fn device_exists(&self, id: &DeviceId) -> bool {
        self.resolve(self.reader.device_exists(id), false)
    }
}
