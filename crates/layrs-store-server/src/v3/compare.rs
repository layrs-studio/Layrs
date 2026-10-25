//! Authorized, bounded, immutable Step comparison. No checkout or history writes.
use super::*;
use layrs_core::scm::{
    DiffContent, DiffEntry, FileId, FileManifest, NativeFileChange, ObjectId, RepoPath, SpaceId,
    Step, StepId, Symlink, Tree, TreeEntryTarget, TreeId, diff_entries,
};
use layrs_sync::v3::{ObjectDescriptor, ObjectKind};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{collections::BTreeMap, sync::Arc};
mod reuse;
mod source;
use source::{PgSource, Source};
#[cfg(test)]
mod boundaries;
#[cfg(test)]
mod fixture;
#[cfg(test)]
mod legacy;
#[cfg(test)]
mod reuse_tests;
#[cfg(test)]
mod tests;

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
    File(Arc<[u8]>, bool),
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
struct Reader<'a, S> {
    source: &'a S,
    space_id: &'a SpaceId,
    bytes: u64,
    objects: usize,
    trees: BTreeMap<TreeId, (ObjectDescriptor, Arc<Tree>)>,
    files: BTreeMap<FileId, Arc<reuse::VerifiedFile>>,
}
impl<'a, S: Source> Reader<'a, S> {
    fn new(source: &'a S, space_id: &'a SpaceId) -> Self {
        Self {
            source,
            space_id,
            bytes: 0,
            objects: 0,
            trees: BTreeMap::new(),
            files: BTreeMap::new(),
        }
    }
    async fn allowed(&self, target: AuthorizationTarget) -> V3StoreResult<()> {
        self.source.allowed(target).await
    }
    async fn read(&mut self, id: &str, kind: ObjectKind) -> V3StoreResult<Vec<u8>> {
        let descriptor = self.descriptor(id, kind).await?;
        self.source.read(&descriptor).await
    }
    // Charge every logical occurrence, including verified reuse. Availability and
    // descriptor limits are live checks, never inherited from a prior occurrence.
    async fn descriptor(&mut self, id: &str, kind: ObjectKind) -> V3StoreResult<ObjectDescriptor> {
        self.objects += 1;
        if self.objects > 100_000 {
            return Err(limit("comparison objects", 100_000));
        }
        let id = ObjectId::parse(id).map_err(|_| V3StoreError::IdentityMismatch)?;
        let descriptor = self.source.descriptor(&id).await?;
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
        if descriptor.canonical_byte_len > MAX_FILE || descriptor.encoded_byte_len > MAX_FILE {
            return Err(limit("service object", MAX_FILE));
        }
        Ok(descriptor)
    }
    async fn step(&mut self, id: &StepId) -> V3StoreResult<Vec<Entry>> {
        self.allowed(AuthorizationTarget::Object {
            space_id: self.space_id.clone(),
            object_id: ObjectId::parse(id.as_str()).expect("typed Step ID"),
        })
        .await?;
        self.source.admitted(id).await?;
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
            let tree = self.tree(&tree).await?;
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
                    space_id: self.space_id.clone(),
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
        let source = PgSource {
            repo: self,
            principal,
            authority,
        };
        let mut reader = Reader::new(&source, &authority.space_id);
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
