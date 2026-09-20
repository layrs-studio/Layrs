//! Restricted reads follow only canonical Step -> Tree -> leaf edges.
use super::*;
use layrs_core::scm::{FileManifest, RepoPath, Step, StepId, Tree, TreeEntryTarget};

impl PgRootedFetchResolver {
    async fn acl_bytes(
        &self,
        space: &SpaceId,
        id: &str,
        kind: ObjectKind,
    ) -> V3StoreResult<Vec<u8>> {
        let id = ObjectId::parse(id).map_err(|_| V3StoreError::IdentityMismatch)?;
        let descriptor = self
            .descriptor(space, &id)
            .await?
            .ok_or(V3StoreError::NotFound)?;
        if descriptor.kind != kind {
            return Err(V3StoreError::IdentityMismatch);
        }
        super::super::read_canonical_object(
            self.repository.cas().as_ref(),
            &descriptor,
            1024 * 1024,
        )
        .await
    }
    pub(super) async fn restricted_target(
        &self,
        space: &SpaceId,
        claim: &FetchClaim,
    ) -> V3StoreResult<Option<AuthorizationTarget>> {
        let (root, path, content_id) = match claim {
            FetchClaim::File {
                root,
                repo_path,
                file_id,
                ..
            } => (root, repo_path, file_id.as_str()),
            FetchClaim::Object {
                root,
                repo_path: Some(path),
                object,
            } => (root, path, object.object_id.as_str()),
            _ => return Ok(None),
        };
        if !matches!(
            root,
            AuthorizationRoot::Step { .. } | AuthorizationRoot::LayerHead { .. }
        ) {
            return Ok(None);
        }
        if path.as_str().len() > 4096 {
            return Ok(None);
        }
        let Some(root_id) = self.resolve_root(space, root).await? else {
            return Ok(None);
        };
        let step: Step = serde_json::from_slice(
            &self
                .acl_bytes(space, root_id.as_str(), ObjectKind::Step)
                .await?,
        )
        .map_err(|_| V3StoreError::IdentityMismatch)?;
        let step_id =
            StepId::parse(root_id.as_str()).map_err(|_| V3StoreError::IdentityMismatch)?;
        if step.id() != &step_id {
            return Err(V3StoreError::IdentityMismatch);
        }
        let parts: Vec<_> = path.as_str().split('/').collect();
        if parts.len() > 128 {
            return Ok(None);
        }
        let mut tree_id = step.root_tree_id().clone();
        let mut leaf = None;
        for (index, part) in parts.iter().enumerate() {
            let tree: Tree = serde_json::from_slice(
                &self
                    .acl_bytes(space, tree_id.as_str(), ObjectKind::Tree)
                    .await?,
            )
            .map_err(|_| V3StoreError::IdentityMismatch)?;
            if tree.id() != &tree_id {
                return Err(V3StoreError::IdentityMismatch);
            }
            let Some(entry) = tree
                .entries()
                .iter()
                .find(|entry| entry.name().as_str() == *part)
            else {
                return Ok(None);
            };
            if index + 1 == parts.len() {
                leaf = Some(entry.target().clone());
                break;
            }
            let TreeEntryTarget::Tree { tree_id: next } = entry.target() else {
                return Ok(None);
            };
            tree_id = next.clone();
        }
        let Some(leaf) = leaf else {
            return Ok(None);
        };
        let matches = match (claim, leaf) {
            (FetchClaim::File { file_id, .. }, TreeEntryTarget::File { file_id: actual }) => {
                *file_id == actual
            }
            (FetchClaim::Object { object, .. }, TreeEntryTarget::Symlink { symlink_id }) => {
                object.kind == ObjectKind::Symlink
                    && object.object_id.as_str() == symlink_id.as_str()
            }
            (FetchClaim::Object { object, .. }, TreeEntryTarget::Tombstone { tombstone_id }) => {
                object.kind == ObjectKind::Tombstone
                    && object.object_id.as_str() == tombstone_id.as_str()
            }
            (FetchClaim::Object { object, .. }, TreeEntryTarget::File { file_id })
                if matches!(object.kind, ObjectKind::FileManifest | ObjectKind::Chunk) =>
            {
                // Membership in a verified manifest of this exact FileId, never generic CAS reachability.
                let manifests:Vec<String>=sqlx::query_scalar("SELECT DISTINCT m.file_manifest_id FROM file_manifests m LEFT JOIN file_manifest_chunks c ON c.file_manifest_id=m.file_manifest_id JOIN space_object_availability a ON a.object_id=m.file_manifest_id AND a.space_id=$1 AND a.availability_state='available' WHERE m.file_id=$2 AND m.reconstruction_verified_at IS NOT NULL AND (m.file_manifest_id=$3 OR c.chunk_id=$3) ORDER BY m.file_manifest_id LIMIT 2").bind(space.as_str()).bind(file_id.as_str()).bind(object.object_id.as_str()).fetch_all(self.repository.pool()).await.map_err(pg_error)?;
                let mut found = false;
                for id in manifests {
                    let manifest: FileManifest = serde_json::from_slice(
                        &self.acl_bytes(space, &id, ObjectKind::FileManifest).await?,
                    )
                    .map_err(|_| V3StoreError::IdentityMismatch)?;
                    if manifest.file_id() != &file_id || manifest.id().as_str() != id {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    found |= match object.kind {
                        ObjectKind::FileManifest => {
                            manifest.id().as_str() == object.object_id.as_str()
                        }
                        ObjectKind::Chunk => manifest
                            .chunks()
                            .iter()
                            .any(|chunk| chunk.chunk_id().as_str() == object.object_id.as_str()),
                        _ => false,
                    };
                }
                found
            }
            _ => false,
        };
        if !matches {
            return Ok(None);
        }
        Ok(Some(AuthorizationTarget::ContentPath {
            space_id: space.clone(),
            step_id,
            root_tree_id: step.root_tree_id().clone(),
            repo_path: RepoPath::parse(path.as_str())
                .map_err(|_| V3StoreError::IdentityMismatch)?,
            content_id: content_id.into(),
        }))
    }
}
