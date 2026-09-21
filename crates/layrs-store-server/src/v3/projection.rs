//! Bounded structural traversal. Only Step/Tree bytes are read, even when the
//! subject cannot read any file. Pages are pinned to identity and policy epoch.
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use layrs_core::scm::{ObjectId, RepoPath, Step, StepId, Tree, TreeEntryTarget, TreeId};
use layrs_sync::v3::{
    ObjectKind, ProjectionEntry, ProjectionPage, ProjectionPageRequest, ProjectionSchema,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

const MAX_ENTRIES: u32 = 100_000;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    schema_version: ProjectionSchema,
    principal_id: layrs_sync::v3::PrincipalId,
    space_id: layrs_core::scm::SpaceId,
    step_id: StepId,
    root_tree_id: TreeId,
    policy_set_id: ObjectId,
    authorization_epoch: u64,
    offset: u32,
}
fn invalid() -> V3StoreError {
    V3StoreError::InvalidDescriptor("invalid projection page or cursor".into())
}

impl PgV3Repository {
    pub async fn projection_page(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        request: ProjectionPageRequest,
    ) -> V3StoreResult<ProjectionPage> {
        if !(1..=200).contains(&request.limit) {
            return Err(invalid());
        }
        if request.expected_policy_set_id != authority.policy_set_id
            || request.expected_authorization_epoch != authority.authorization_epoch
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
        )?;
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
        let mut byte_count = 0;
        let step: Step = serde_json::from_slice(
            &self
                .projection_metadata(
                    authority,
                    request.step_id.as_str(),
                    ObjectKind::Step,
                    &mut byte_count,
                )
                .await?,
        )
        .map_err(|_| V3StoreError::IdentityMismatch)?;
        if step.id() != &request.step_id {
            return Err(V3StoreError::IdentityMismatch);
        }
        let offset = decode_cursor(&request, principal, authority, step.root_tree_id())?;
        let mut pending = vec![(
            String::new(),
            TreeEntryTarget::tree(step.root_tree_id().clone()),
            false,
        )];
        let mut traversed = 0u32;
        let mut entries = Vec::new();
        let mut has_next = false;
        while let Some((path, target, executable)) = pending.pop() {
            if !path.is_empty() {
                traversed = traversed.checked_add(1).ok_or_else(invalid)?;
                if traversed > MAX_ENTRIES {
                    return Err(V3StoreError::LimitExceeded {
                        resource: "projection entries",
                        limit: MAX_ENTRIES.into(),
                    });
                }
                if traversed > offset {
                    if entries.len() == usize::from(request.limit) {
                        has_next = true;
                        break;
                    }
                    let repo_path = RepoPath::parse(&path).map_err(|_| invalid())?;
                    let scoped = AuthorizationTarget::ScopedPath {
                        space_id: authority.space_id.clone(),
                        base_step_id: step.id().clone(),
                        root_tree_id: step.root_tree_id().clone(),
                        repo_path: repo_path.clone(),
                    };
                    let mut decisions = Vec::new();
                    for action in [V3Action::ReadProjection, V3Action::ScopedPathWrite] {
                        decisions.push(
                            self.authorizer()
                                .authorize(&AuthorizationCheck {
                                    principal: principal.clone(),
                                    action,
                                    target: scoped.clone(),
                                    policy_set_id: authority.policy_set_id.clone(),
                                    authorization_epoch: authority.authorization_epoch,
                                })
                                .await?
                                .allowed,
                        );
                    }
                    // Directory metadata is structural; this is never a claim
                    // that all descendants are readable or writable.
                    entries.push(ProjectionEntry {
                        path: repo_path,
                        target: target.clone(),
                        executable,
                        content_readable: decisions[0],
                        writable: decisions[0] && decisions[1],
                    });
                }
            }
            if let TreeEntryTarget::Tree { tree_id } = target {
                if path.split('/').count() > 128 || path.len() > 4096 {
                    return Err(invalid());
                }
                let tree: Tree = serde_json::from_slice(
                    &self
                        .projection_metadata(
                            authority,
                            tree_id.as_str(),
                            ObjectKind::Tree,
                            &mut byte_count,
                        )
                        .await?,
                )
                .map_err(|_| V3StoreError::IdentityMismatch)?;
                if tree.id() != &tree_id {
                    return Err(V3StoreError::IdentityMismatch);
                }
                for entry in tree.entries().iter().rev() {
                    let child = if path.is_empty() {
                        entry.name().to_string()
                    } else {
                        format!("{path}/{}", entry.name())
                    };
                    if child.len() > 4096 || child.split('/').count() > 128 {
                        return Err(invalid());
                    }
                    pending.push((child, entry.target().clone(), entry.executable()));
                    if pending.len() > MAX_ENTRIES as usize {
                        return Err(invalid());
                    }
                }
            }
        }
        if traversed < offset {
            return Err(invalid());
        }
        // A policy or membership change during traversal invalidates the page.
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
        )?;
        let next_cursor = if has_next {
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&Cursor {
                        schema_version: ProjectionSchema::V1,
                        principal_id: principal.principal_id.clone(),
                        space_id: authority.space_id.clone(),
                        step_id: step.id().clone(),
                        root_tree_id: step.root_tree_id().clone(),
                        policy_set_id: authority.policy_set_id.clone(),
                        authorization_epoch: authority.authorization_epoch,
                        offset: offset + entries.len() as u32,
                    })
                    .map_err(|_| invalid())?,
                ),
            )
        } else {
            None
        };
        Ok(ProjectionPage {
            schema_version: ProjectionSchema::V1,
            principal_id: principal.principal_id.clone(),
            space_id: authority.space_id.clone(),
            step_id: step.id().clone(),
            root_tree_id: step.root_tree_id().clone(),
            policy_set_id: authority.policy_set_id.clone(),
            authorization_epoch: authority.authorization_epoch,
            offset,
            entries,
            next_cursor,
        })
    }

    async fn projection_metadata(
        &self,
        authority: &SpaceAuthority,
        id: &str,
        kind: ObjectKind,
        bytes: &mut u64,
    ) -> V3StoreResult<Vec<u8>> {
        let id = ObjectId::parse(id).map_err(|_| invalid())?;
        let descriptor = load_space_descriptor(self.pool(), &authority.space_id, &id).await?;
        if descriptor.kind != kind {
            return Err(V3StoreError::IdentityMismatch);
        }
        *bytes = bytes
            .checked_add(descriptor.canonical_byte_len)
            .ok_or_else(invalid)?;
        if *bytes > MAX_BYTES {
            return Err(V3StoreError::LimitExceeded {
                resource: "projection bytes",
                limit: MAX_BYTES,
            });
        }
        read_canonical_object(self.cas().as_ref(), &descriptor, 1024 * 1024).await
    }
}

fn decode_cursor(
    request: &ProjectionPageRequest,
    principal: &PrincipalContext,
    authority: &SpaceAuthority,
    root: &TreeId,
) -> V3StoreResult<u32> {
    let Some(encoded) = &request.cursor else {
        return Ok(0);
    };
    if encoded.len() > 4096 {
        return Err(invalid());
    }
    let cursor: Cursor =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    if cursor.principal_id != principal.principal_id
        || cursor.space_id != authority.space_id
        || cursor.step_id != request.step_id
        || &cursor.root_tree_id != root
        || cursor.policy_set_id != authority.policy_set_id
        || cursor.authorization_epoch != authority.authorization_epoch
        || cursor.offset == 0
        || cursor.offset > MAX_ENTRIES
    {
        return Err(invalid());
    }
    Ok(cursor.offset)
}
