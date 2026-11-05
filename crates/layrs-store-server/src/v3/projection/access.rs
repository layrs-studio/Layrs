//! Exact namespace classification; neither file contents nor bearer capabilities.
use super::*;
use layrs_sync::v3::{
    Nullable, ProjectionAccess, ProjectionAccessEntry, ProjectionAccessRequest,
    ProjectionAccessSchema,
};
use std::collections::BTreeMap;

impl PgV3Repository {
    pub async fn projection_access(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        request: ProjectionAccessRequest,
    ) -> V3StoreResult<ProjectionAccess> {
        request.validate().map_err(|_| invalid())?;
        if request.expected_policy_set_id != authority.policy_set_id
            || request.expected_authorization_epoch != authority.authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        self.access_envelope(principal, authority).await?;
        let admitted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM space_steps WHERE space_id=$1 AND step_id=$2)",
        )
        .bind(authority.space_id.as_str())
        .bind(request.step_id.as_str())
        .fetch_one(self.pool())
        .await
        .map_err(pg_error)?;
        if !admitted {
            return Err(V3StoreError::NotFound);
        }
        let mut bytes = 0;
        let step: Step = serde_json::from_slice(
            &self
                .projection_metadata(
                    authority,
                    request.step_id.as_str(),
                    ObjectKind::Step,
                    &mut bytes,
                )
                .await?,
        )
        .map_err(|_| V3StoreError::IdentityMismatch)?;
        if step.id() != &request.step_id {
            return Err(V3StoreError::IdentityMismatch);
        }
        let mut trees = BTreeMap::<TreeId, Tree>::new();
        let mut entries = Vec::with_capacity(request.paths.len());
        for path in request.paths {
            let parts: Vec<_> = path.as_str().split('/').collect();
            let mut tree_id = step.root_tree_id().clone();
            let mut target = None;
            let mut executable = false;
            for (index, part) in parts.iter().enumerate() {
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    trees.entry(tree_id.clone())
                {
                    let tree: Tree = serde_json::from_slice(
                        &self
                            .projection_metadata(
                                authority,
                                tree_id.as_str(),
                                ObjectKind::Tree,
                                &mut bytes,
                            )
                            .await?,
                    )
                    .map_err(|_| V3StoreError::IdentityMismatch)?;
                    if tree.id() != &tree_id {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    slot.insert(tree);
                }
                let entry = trees[&tree_id]
                    .entries()
                    .iter()
                    .find(|entry| entry.name().as_str() == *part);
                let Some(entry) = entry else {
                    break;
                };
                if index + 1 == parts.len() {
                    target = Some(entry.target().clone());
                    executable = entry.executable();
                } else if let TreeEntryTarget::Tree { tree_id: child } = entry.target() {
                    tree_id = child.clone();
                } else {
                    // A missing descendant below a file is not a creation slot.
                    return Err(V3StoreError::InvalidDescriptor(
                        "projection path has a non-directory ancestor".into(),
                    ));
                }
            }
            let scoped = AuthorizationTarget::ScopedPath {
                space_id: authority.space_id.clone(),
                base_step_id: step.id().clone(),
                root_tree_id: step.root_tree_id().clone(),
                repo_path: path.clone(),
            };
            let mut grants = [false; 2];
            for (index, action) in [V3Action::ReadProjection, V3Action::ScopedPathWrite]
                .into_iter()
                .enumerate()
            {
                grants[index] = self
                    .authorizer()
                    .authorize(&AuthorizationCheck {
                        principal: principal.clone(),
                        action,
                        target: scoped.clone(),
                        policy_set_id: authority.policy_set_id.clone(),
                        authorization_epoch: authority.authorization_epoch,
                    })
                    .await?
                    .allowed;
            }
            entries.push(ProjectionAccessEntry {
                // Store metadata names can never become application file slots.
                writable: grants[0]
                    && grants[1]
                    && !path.as_str().split('/').any(|part| {
                        part.eq_ignore_ascii_case(".layrs") || part.eq_ignore_ascii_case(".git")
                    }),
                path,
                target: Nullable(target),
                executable,
                content_readable: grants[0],
            });
        }
        // Re-read current authority and identity after traversal; no stale grants.
        self.access_envelope(principal, authority).await?;
        Ok(ProjectionAccess {
            schema_version: ProjectionAccessSchema::V1,
            principal_id: principal.principal_id.clone(),
            space_id: authority.space_id.clone(),
            step_id: step.id().clone(),
            root_tree_id: step.root_tree_id().clone(),
            policy_set_id: authority.policy_set_id.clone(),
            authorization_epoch: authority.authorization_epoch,
            entries,
        })
    }

    async fn access_envelope(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
    ) -> V3StoreResult<()> {
        let row = sqlx::query("SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 AND lifecycle_state='ready'")
            .bind(authority.space_id.as_str()).fetch_optional(self.pool()).await.map_err(pg_error)?
            .ok_or(V3StoreError::NotFound)?;
        if row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch")
                != i64_from_u64(authority.authorization_epoch)?
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        require_allowed(
            self.authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::ReadProjection,
                    target: AuthorizationTarget::Space(authority.space_id.clone()),
                    policy_set_id: authority.policy_set_id.clone(),
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )
    }
}
