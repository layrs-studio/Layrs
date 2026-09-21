use super::{PathSemantics, ReservedPaths, Result, invalid, paths::path_key};
use crate::scm::{ObjectId, RepoPath, StepId, TreeEntryTarget, TreeId, canonical_json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Materialized,
    Reserved,
    Unavailable,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionEntry {
    pub path: RepoPath,
    /// Exact remote target, never a rewritten/filtered Tree identity.
    pub target: Option<TreeEntryTarget>,
    pub executable: bool,
    /// Observed server permission; never a persistent network capability.
    pub writable: bool,
    pub state: EntryState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionSnapshot {
    pub schema_version: String,
    pub base_step_id: StepId,
    pub base_root_tree_id: TreeId,
    pub observed_policy_set_id: ObjectId,
    pub observed_authorization_epoch: u64,
    pub path_semantics: PathSemantics,
    /// Sorted exact paths. This describes local projection, not a complete Tree.
    pub entries: Vec<ProjectionEntry>,
}
impl ProjectionSnapshot {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != "layrs.projection-snapshot.v1" || self.entries.len() > 100_000 {
            return Err(invalid("unsupported or oversized projection snapshot"));
        }
        let mut previous: Option<&str> = None;
        let mut prefixes = BTreeMap::new();
        let mut leaves = std::collections::BTreeSet::new();
        let mut upper_encoded_bytes = 1024usize;
        for entry in &self.entries {
            let path = entry.path.as_str();
            upper_encoded_bytes = upper_encoded_bytes
                .checked_add(path.len().saturating_mul(6).saturating_add(512))
                .ok_or_else(|| invalid("projection size overflow"))?;
            if upper_encoded_bytes > 64 * 1024 * 1024 {
                return Err(invalid("projection encoded size budget exceeded"));
            }
            if previous.is_some_and(|p| p >= path) {
                return Err(invalid("projection entries must be sorted and unique"));
            }
            previous = Some(path);
            let key = path_key(path, self.path_semantics)?;
            for (index, _) in path
                .match_indices('/')
                .chain(std::iter::once((path.len(), "/")))
            {
                let prefix = &path[..index];
                let key = path_key(prefix, self.path_semantics)?;
                if prefixes
                    .insert(key, prefix)
                    .is_some_and(|old| old != prefix)
                {
                    return Err(invalid("projection paths alias on the checkout platform"));
                }
            }
            if (entry.state == EntryState::Absent) != entry.target.is_none() {
                return Err(invalid("absence must be explicit and have no target"));
            }
            if entry.state == EntryState::Reserved && entry.writable {
                return Err(invalid("reserved content cannot be writable"));
            }
            if entry.executable && !matches!(entry.target, Some(TreeEntryTarget::File { .. })) {
                return Err(invalid("only files carry executable mode"));
            }
            if !matches!(entry.target, Some(TreeEntryTarget::Tree { .. })) {
                leaves.insert(key);
            }
        }
        for entry in &self.entries {
            let key = path_key(entry.path.as_str(), self.path_semantics)?;
            for (index, _) in key.match_indices('/') {
                if leaves.contains(&key[..index]) {
                    return Err(invalid("projection leaf also owns descendants"));
                }
            }
        }
        Ok(())
    }
    /// Unavailable values are also fenced: uncertainty never authorizes deletion.
    pub fn reserved_paths(&self) -> Result<ReservedPaths> {
        self.validate()?;
        ReservedPaths::new(
            self.entries
                .iter()
                .filter(|e| matches!(e.state, EntryState::Reserved | EntryState::Unavailable))
                .map(|e| e.path.as_str()),
            self.path_semantics,
        )
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = canonical_json(self).map_err(|e| invalid(e.to_string()))?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(invalid("projection snapshot exceeds 64MiB"));
        }
        Ok(bytes)
    }
}
