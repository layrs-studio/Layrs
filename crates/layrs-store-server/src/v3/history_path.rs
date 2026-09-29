//! Exact-path history reads only authenticated, Space-admitted metadata.
use super::*;
use layrs_core::scm::{ObjectId, RepoPath, Step, StepId, Tree, TreeEntry, TreeEntryTarget};
use layrs_sync::v3::ObjectKind;
use std::collections::BTreeMap;

pub(super) struct PathHistory<'a> {
    repo: &'a PgV3Repository,
    principal: &'a PrincipalContext,
    authority: &'a SpaceAuthority,
    path: &'a RepoPath,
    bytes: u64,
    reads: usize,
    entries: BTreeMap<StepId, Option<TreeEntry>>,
}
impl<'a> PathHistory<'a> {
    pub(super) fn byte_count(&self) -> u64 {
        self.bytes
    }
    pub(super) fn new(
        repo: &'a PgV3Repository,
        principal: &'a PrincipalContext,
        authority: &'a SpaceAuthority,
        path: &'a RepoPath,
    ) -> Self {
        Self {
            repo,
            principal,
            authority,
            path,
            bytes: 0,
            reads: 0,
            entries: BTreeMap::new(),
        }
    }
    async fn read(&mut self, id: &str, kind: ObjectKind) -> V3StoreResult<Vec<u8>> {
        self.reads += 1;
        if self.reads > 100_000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "history path objects",
                limit: 100_000,
            });
        }
        let id = ObjectId::parse(id).map_err(|_| V3StoreError::IdentityMismatch)?;
        self.repo
            .history_allowed(
                self.principal,
                self.authority,
                AuthorizationTarget::Object {
                    space_id: self.authority.space_id.clone(),
                    object_id: id.clone(),
                },
            )
            .await?;
        let descriptor =
            load_space_descriptor(self.repo.pool(), &self.authority.space_id, &id).await?;
        if descriptor.kind != kind {
            return Err(V3StoreError::IdentityMismatch);
        }
        self.bytes = self
            .bytes
            .checked_add(descriptor.canonical_byte_len)
            .ok_or(V3StoreError::IdentityMismatch)?;
        if self.bytes > 64 * 1024 * 1024 {
            return Err(V3StoreError::LimitExceeded {
                resource: "history path bytes",
                limit: 64 * 1024 * 1024,
            });
        }
        read_canonical_object(self.repo.cas().as_ref(), &descriptor, 1024 * 1024).await
    }
    async fn entry(&mut self, step: &Step) -> V3StoreResult<Option<TreeEntry>> {
        if let Some(entry) = self.entries.get(step.id()) {
            return Ok(entry.clone());
        }
        let parts: Vec<_> = self.path.as_str().split('/').map(str::to_owned).collect();
        if parts.len() > 128 {
            return Err(V3StoreError::LimitExceeded {
                resource: "history path depth",
                limit: 128,
            });
        }
        let mut tree_id = step.root_tree_id().clone();
        let mut found = None;
        for (index, part) in parts.iter().enumerate() {
            let prefix = RepoPath::parse(parts[..=index].join("/"))
                .map_err(|_| V3StoreError::IdentityMismatch)?;
            self.repo
                .history_allowed(
                    self.principal,
                    self.authority,
                    AuthorizationTarget::Path {
                        space_id: self.authority.space_id.clone(),
                        repo_path: prefix,
                    },
                )
                .await?;
            let tree: Tree =
                serde_json::from_slice(&self.read(tree_id.as_str(), ObjectKind::Tree).await?)
                    .map_err(|_| V3StoreError::IdentityMismatch)?;
            if tree.id() != &tree_id {
                return Err(V3StoreError::IdentityMismatch);
            }
            let Some(entry) = tree
                .entries()
                .iter()
                .find(|entry| entry.name().as_str() == part)
            else {
                break;
            };
            if index + 1 == parts.len() {
                found = Some(entry.clone());
                break;
            }
            let TreeEntryTarget::Tree { tree_id: next } = entry.target() else {
                break;
            };
            tree_id = next.clone();
        }
        self.entries.insert(step.id().clone(), found.clone());
        Ok(found)
    }
    pub(super) async fn changed(&mut self, step: &Step) -> V3StoreResult<bool> {
        let current = self.entry(step).await?;
        let mut changed = step.parent_step_ids().is_empty() && current.is_some();
        for parent in step.parent_step_ids() {
            let previous = if let Some(entry) = self.entries.get(parent) {
                entry.clone()
            } else {
                let prior: Step =
                    serde_json::from_slice(&self.read(parent.as_str(), ObjectKind::Step).await?)
                        .map_err(|_| V3StoreError::IdentityMismatch)?;
                if prior.id() != parent {
                    return Err(V3StoreError::IdentityMismatch);
                }
                self.entry(&prior).await?
            };
            changed |= current != previous;
        }
        Ok(changed)
    }
}
