//! Explicitly observed local namespace edits. Unknown destinations never become
//! writable merely because no file currently exists there.
mod engine;
use super::*;
use crate::secure_fs::PinnedDirectory;
pub(super) use engine::verify_history;
use layrs_core::{
    projection::{EntryState, ProjectionEntry},
    scm::{RepoPath, TreeEntryTarget, canonical_json},
};
use rusqlite::params;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NamespaceMutation {
    Create {
        destination: RepoPath,
    },
    Move {
        source: RepoPath,
        destination: RepoPath,
    },
}
impl NamespaceMutation {
    fn destination(&self) -> &RepoPath {
        match self {
            Self::Create { destination } | Self::Move { destination, .. } => destination,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamespacePreview {
    pub operation_id: String,
    pub projection_generation: u64,
    pub projection_digest: String,
    pub mutation: NamespaceMutation,
    pub source_file: Option<CapturedFile>,
    pub checkout_digest: String,
    pub digest: String,
}
impl NamespacePreview {
    fn computed_digest(&self) -> Result<String> {
        if self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || !self
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid("invalid namespace operation identifier"));
        }
        let mut copy = self.clone();
        copy.digest.clear();
        let bytes = canonical_json(&copy).map_err(invalid)?;
        Ok(digest(b"layrs-projection-namespace-preview-v1\0", &bytes))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamespacePoint {
    JournalDurable,
    PathApplied,
    BeforeCommit,
    AfterCommit,
}
pub trait NamespaceFaults {
    fn check(&self, point: NamespacePoint) -> io::Result<()>;
}
pub struct NoNamespaceFaults;
impl NamespaceFaults for NoNamespaceFaults {
    fn check(&self, _: NamespacePoint) -> io::Result<()> {
        Ok(())
    }
}

impl ProjectionStore {
    /// The caller must first verify a fresh authenticated absent-path observation
    /// for this principal, exact Step/root and policy/epoch. This API records that
    /// observation locally, never creates a remote permission or a physical file.
    pub fn observe_absent_paths(
        &self,
        expected_generation: u64,
        expected_digest: &str,
        paths: &[RepoPath],
    ) -> Result<VersionedProjection> {
        if paths.is_empty() || paths.len() > 1024 {
            return Err(invalid("absence observation requires1..1024 paths"));
        }
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        let current = database::current(&access.connection)?;
        if current.generation != expected_generation || current.digest != expected_digest {
            return Err(invalid("absence observation projection changed"));
        }
        let fences = current.snapshot.reserved_paths()?;
        let root = PinnedDirectory::open(&self.root)?;
        let mut next = current.snapshot.clone();
        let mut changed = false;
        let mut seen = std::collections::BTreeSet::new();
        for path in paths {
            if !seen.insert(path) || path.as_str().len() > 4096 {
                return Err(invalid("duplicate or oversized absent path"));
            }
            fences.check_path(path.as_str())?;
            super::install::files::destination_absent(&root, &self.root, path.as_str())?;
            match next.entries.binary_search_by(|e| e.path.cmp(path)) {
                Ok(index)
                    if next.entries[index].state == EntryState::Absent
                        && next.entries[index].writable => {}
                Ok(_) => return Err(invalid("path already has a different observation")),
                Err(index) => {
                    next.entries.insert(
                        index,
                        ProjectionEntry {
                            path: path.clone(),
                            target: None,
                            executable: false,
                            writable: true,
                            state: EntryState::Absent,
                        },
                    );
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(current);
        }
        let bytes = next.canonical_bytes()?;
        let generation = current
            .generation
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or_else(|| invalid("projection generation exhausted"))?;
        let digest = database::digest(generation, Some(&current.digest), &bytes);
        access.verify()?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
            params![generation, digest, current.digest, bytes],
        )?;
        tx.execute(
            "UPDATE projection_head SET generation=?1 WHERE singleton=1",
            [generation],
        )?;
        tx.commit()?;
        access.verify()?;
        Ok(VersionedProjection {
            generation,
            digest,
            snapshot: next,
        })
    }
    pub fn preview_namespace(
        &self,
        expected_generation: u64,
        mutation: &NamespaceMutation,
    ) -> Result<NamespacePreview> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_mutable(&access.connection)?;
        let state = database::current(&access.connection)?;
        if state.generation != expected_generation {
            return Err(ProjectionStoreError::Stale {
                expected: expected_generation,
                actual: state.generation,
            });
        }
        let result = preview(self, &state, mutation)?;
        access.verify()?;
        Ok(result)
    }
    pub fn apply_namespace(
        &self,
        preview: &NamespacePreview,
        create_bytes: Option<&[u8]>,
    ) -> Result<CapturedPatch> {
        self.apply_namespace_with_faults(preview, create_bytes, &NoNamespaceFaults)
    }
    pub fn apply_namespace_with_faults(
        &self,
        preview: &NamespacePreview,
        create_bytes: Option<&[u8]>,
        faults: &dyn NamespaceFaults,
    ) -> Result<CapturedPatch> {
        engine::apply(self, preview, create_bytes, faults)
    }
    pub fn resume_namespace(checkout_root: impl AsRef<Path>) -> Result<CapturedPatch> {
        let root = std::path::absolute(checkout_root)?;
        let marker = format::read_marker(&root)?;
        engine::resume(&Self { root, marker })
    }
}
fn validate_mutation(snapshot: &ProjectionSnapshot, mutation: &NamespaceMutation) -> Result<()> {
    let fences = snapshot.reserved_paths()?;
    let destination = mutation.destination();
    fences.check_path(destination.as_str())?;
    let destination_entry = snapshot
        .entries
        .binary_search_by(|e| e.path.cmp(destination))
        .ok()
        .map(|n| &snapshot.entries[n])
        .ok_or_else(|| invalid("destination has no explicit absence observation"))?;
    if destination_entry.state != EntryState::Absent || !destination_entry.writable {
        return Err(invalid("destination is not explicitly absent and writable"));
    }
    if let NamespaceMutation::Move { source, .. } = mutation {
        fences.check_move(source.as_str(), destination.as_str())?;
        if source == destination
            || source.as_str().starts_with(&format!("{destination}/"))
            || destination.as_str().starts_with(&format!("{source}/"))
        {
            return Err(invalid("namespace move paths overlap"));
        }
        let source_entry = snapshot
            .entries
            .binary_search_by(|e| e.path.cmp(source))
            .ok()
            .map(|n| &snapshot.entries[n])
            .ok_or_else(|| invalid("namespace source is unknown"))?;
        let regular_source = matches!(
            (&source_entry.state, &source_entry.target),
            (EntryState::Materialized, Some(TreeEntryTarget::File { .. }))
                | (EntryState::Absent, None)
        );
        if !source_entry.writable || !regular_source {
            return Err(invalid(
                "namespace move requires an observed writable regular file",
            ));
        }
    }
    Ok(())
}
fn preview(
    store: &ProjectionStore,
    state: &VersionedProjection,
    mutation: &NamespaceMutation,
) -> Result<NamespacePreview> {
    validate_mutation(&state.snapshot, mutation)?;
    let root = PinnedDirectory::open(&store.root)?;
    super::install::files::destination_absent(&root, &store.root, mutation.destination().as_str())?;
    let scan = super::capture::scan::scan(store, &state.snapshot, false)?;
    let source_file = if let NamespaceMutation::Move { source, .. } = mutation {
        let file = scan
            .files
            .get(source.as_str())
            .and_then(|f| f.clone())
            .ok_or_else(|| invalid("namespace source is absent"))?;
        if file.bytes > MAX_PATCH_BYTES as u64 {
            return Err(invalid("namespace move exceeds16MiB patch limit"));
        }
        Some(file)
    } else {
        None
    };
    let checkout_digest = digest(
        b"layrs-projection-checkout-v1\0",
        &canonical_json(&scan.files).map_err(invalid)?,
    );
    let mut result = NamespacePreview {
        operation_id: engine::unique_id()?,
        projection_generation: state.generation,
        projection_digest: state.digest.clone(),
        mutation: mutation.clone(),
        source_file,
        checkout_digest,
        digest: String::new(),
    };
    result.digest = result.computed_digest()?;
    Ok(result)
}
fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(bytes);
    format!("blake3:{}", h.finalize())
}
