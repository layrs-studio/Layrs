//! Authorized, bounded, immutable Step comparison. No checkout or history writes.
use super::*;
use layrs_core::scm::{
    DiffContent, DiffEntry, FileId, FileManifest, NativeFileChange, ObjectId, RepoPath, Step,
    StepId, Symlink, Tree, TreeEntryTarget, diff_entries,
};
use layrs_sync::v3::ObjectKind;
use serde::{Deserialize, Serialize};
use sqlx::Row;

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompareStepsRequest {
    pub before: StepId,
    pub after: StepId,
}
#[derive(Serialize)]
pub struct CompareStepsResponse {
    pub before: StepId,
    pub after: StepId,
    pub changes: Vec<NativeFileChange>,
}
enum Content {
    File(Vec<u8>, bool),
    Symlink(String),
}
struct Entry {
    path: String,
    content: Content,
}
fn entries(values: &[Entry]) -> Vec<DiffEntry<'_>> {
    values
        .iter()
        .map(|e| DiffEntry {
            path: &e.path,
            content: match &e.content {
                Content::File(bytes, executable) => DiffContent::File {
                    bytes,
                    executable: *executable,
                },
                Content::Symlink(target) => DiffContent::Symlink { target },
            },
        })
        .collect()
}
struct Reader<'a> {
    repo: &'a PgV3Repository,
    principal: &'a PrincipalContext,
    authority: &'a SpaceAuthority,
    bytes: u64,
    objects: usize,
}
impl Reader<'_> {
    async fn allowed(&self, target: AuthorizationTarget) -> V3StoreResult<()> {
        require_allowed(
            self.repo
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: self.principal.clone(),
                    action: V3Action::FetchObject,
                    target,
                    policy_set_id: self.authority.policy_set_id.clone(),
                    authorization_epoch: self.authority.authorization_epoch,
                })
                .await?,
        )
    }
    async fn read(&mut self, id: &str, kind: ObjectKind) -> V3StoreResult<Vec<u8>> {
        self.objects += 1;
        if self.objects > 100_000 {
            return Err(limit("comparison objects", 100_000));
        }
        let id = ObjectId::parse(id).map_err(|_| V3StoreError::IdentityMismatch)?;
        let descriptor =
            load_space_descriptor(self.repo.pool(), &self.authority.space_id, &id).await?;
        if descriptor.kind != kind {
            return Err(V3StoreError::IdentityMismatch);
        }
        self.bytes = self
            .bytes
            .checked_add(descriptor.canonical_byte_len)
            .ok_or_else(|| limit("comparison bytes", MAX_BYTES))?;
        if self.bytes > MAX_BYTES {
            return Err(limit("comparison bytes", MAX_BYTES));
        }
        read_canonical_object(self.repo.cas().as_ref(), &descriptor, MAX_FILE).await
    }
    async fn step(&mut self, id: &StepId) -> V3StoreResult<Vec<Entry>> {
        self.allowed(AuthorizationTarget::Object {
            space_id: self.authority.space_id.clone(),
            object_id: ObjectId::parse(id.as_str()).expect("typed Step ID"),
        })
        .await?;
        let admitted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM space_steps WHERE space_id=$1 AND step_id=$2)",
        )
        .bind(self.authority.space_id.as_str())
        .bind(id.as_str())
        .fetch_one(self.repo.pool())
        .await
        .map_err(pg_error)?;
        if !admitted {
            return Err(V3StoreError::NotFound);
        }
        let step: Step = parse(&self.read(id.as_str(), ObjectKind::Step).await?)?;
        if step.id() != id {
            return Err(V3StoreError::IdentityMismatch);
        }
        let mut stack = vec![(step.root_tree_id().clone(), String::new(), 0usize)];
        let mut result = Vec::new();
        let mut count = 0usize;
        while let Some((tree, prefix, depth)) = stack.pop() {
            if depth > 128 {
                return Err(limit("comparison depth", 128));
            }
            let tree: Tree = parse(&self.read(tree.as_str(), ObjectKind::Tree).await?)?;
            for entry in tree.entries() {
                count += 1;
                if count > MAX_ENTRIES {
                    return Err(limit("comparison entries", MAX_ENTRIES as u64));
                }
                let path = if prefix.is_empty() {
                    entry.name().as_str().to_owned()
                } else {
                    format!("{prefix}/{}", entry.name().as_str())
                };
                let repo_path = RepoPath::parse(&path)
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                self.allowed(AuthorizationTarget::Path {
                    space_id: self.authority.space_id.clone(),
                    repo_path,
                })
                .await?;
                let content = match entry.target() {
                    TreeEntryTarget::Tree { tree_id } => {
                        stack.push((tree_id.clone(), path, depth + 1));
                        continue;
                    }
                    TreeEntryTarget::File { file_id } => {
                        Content::File(self.file(file_id).await?, entry.executable())
                    }
                    TreeEntryTarget::Symlink { symlink_id } => {
                        let link: Symlink =
                            parse(&self.read(symlink_id.as_str(), ObjectKind::Symlink).await?)?;
                        Content::Symlink(link.target().to_owned())
                    }
                    TreeEntryTarget::Tombstone { tombstone_id } => {
                        self.read(tombstone_id.as_str(), ObjectKind::Tombstone)
                            .await?;
                        continue;
                    }
                    TreeEntryTarget::Conflict { .. } => {
                        return Err(V3StoreError::InvalidDescriptor(
                            "comparison of unresolved conflicts is not supported".into(),
                        ));
                    }
                };
                result.push(Entry { path, content });
            }
        }
        Ok(result)
    }
    async fn file(&mut self, id: &FileId) -> V3StoreResult<Vec<u8>> {
        let selected: Option<String> = sqlx::query_scalar("SELECT m.file_manifest_id FROM file_manifests m JOIN space_object_availability a ON a.object_id=m.file_manifest_id WHERE a.space_id=$1 AND m.file_id=$2 AND a.availability_state='available' AND m.reconstruction_verified_at IS NOT NULL ORDER BY m.file_manifest_id LIMIT 1")
            .bind(self.authority.space_id.as_str()).bind(id.as_str()).fetch_optional(self.repo.pool()).await.map_err(pg_error)?;
        let manifest: FileManifest = parse(
            &self
                .read(
                    &selected.ok_or(V3StoreError::NotFound)?,
                    ObjectKind::FileManifest,
                )
                .await?,
        )?;
        if manifest.file_id() != id {
            return Err(V3StoreError::IdentityMismatch);
        }
        if manifest.byte_length() > MAX_FILE {
            return Err(limit("comparison file", MAX_FILE));
        }
        let mut bytes = Vec::new();
        for chunk in manifest.chunks() {
            let value = self
                .read(chunk.chunk_id().as_str(), ObjectKind::Chunk)
                .await?;
            if value.len() as u64 != chunk.byte_length()
                || bytes.len() as u64 + value.len() as u64 > manifest.byte_length()
            {
                return Err(V3StoreError::IdentityMismatch);
            }
            bytes.extend_from_slice(&value);
        }
        if bytes.len() as u64 != manifest.byte_length() || FileId::from_bytes(&bytes) != *id {
            return Err(V3StoreError::IdentityMismatch);
        }
        Ok(bytes)
    }
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> V3StoreResult<T> {
    serde_json::from_slice(bytes).map_err(|e| V3StoreError::CorruptObject(e.to_string()))
}
fn limit(resource: &'static str, limit: u64) -> V3StoreError {
    V3StoreError::LimitExceeded { resource, limit }
}

impl PgV3Repository {
    pub async fn compare_steps(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        request: CompareStepsRequest,
    ) -> V3StoreResult<CompareStepsResponse> {
        let decision = self
            .authorizer()
            .authorize(&AuthorizationCheck {
                principal: principal.clone(),
                action: V3Action::FetchObject,
                target: AuthorizationTarget::Space(authority.space_id.clone()),
                policy_set_id: authority.policy_set_id.clone(),
                authorization_epoch: authority.authorization_epoch,
            })
            .await?;
        if decision.content_restricted {
            return Err(V3StoreError::InvalidCapability);
        }
        require_allowed(decision)?;
        let mut reader = Reader {
            repo: self,
            principal,
            authority,
            bytes: 0,
            objects: 0,
        };
        reader
            .allowed(AuthorizationTarget::Space(authority.space_id.clone()))
            .await?;
        let before = reader.step(&request.before).await?;
        let after = reader.step(&request.after).await?;
        let changes = diff_entries(&entries(&before), &entries(&after));
        // Recheck active identity/membership and policy epoch under the same lock order as writes.
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        super::transaction_store::lock_identity(&mut tx, principal, &authority.space_id).await?;
        let row = sqlx::query("SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR SHARE")
            .bind(authority.space_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        reader
            .allowed(AuthorizationTarget::Space(authority.space_id.clone()))
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(CompareStepsResponse {
            before: request.before,
            after: request.after,
            changes,
        })
    }
}
