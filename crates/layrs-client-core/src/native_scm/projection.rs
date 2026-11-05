//! Verified local checkout descriptors without materializing every file in RAM.
use super::*;
use layrs_core::scm::{FileId, SpaceId, StepId, TreeEntryTarget};
use layrs_store::scm::{ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::checkout::{
    projection_tree_id, CheckoutProjectionContent, CheckoutProjectionEntry,
};
use layrs_store_local::scm_v1::NativeScmReader;
use std::collections::{BTreeMap, BTreeSet};

impl NativeWorkspace {
    /// The Step must be admitted to this Space. Paths, kinds and sizes come
    /// from validated metadata (Trees, FileManifests); file bytes are NOT
    /// re-read here. Anything that writes or publishes bytes verifies them at
    /// use; `verify_changed_content` front-loads that check for a transition.
    pub fn read_step_projection(&self, step: &StepId) -> Result<Vec<CheckoutProjectionEntry>> {
        let store = self.open_store()?;
        let state = self.state_with(&store)?;
        self.read_step_projection_with(&store, state.space.id(), step)
    }

    /// `read_step_projection` on an already open Store, in `space`.
    pub(super) fn read_step_projection_with(
        &self,
        store: &layrs_store_local::scm_v1::NativeStoreOpen,
        space: &SpaceId,
        step: &StepId,
    ) -> Result<Vec<CheckoutProjectionEntry>> {
        // A Step and its Trees are immutable, a Space admission and a
        // `present` file availability are never withdrawn (no deletion of
        // admissions or CAS objects), so an admitted Step's projection is the
        // same for the life of the process: one transition reads the same
        // base and target several times.
        let key = (
            self.store_identity().to_owned(),
            self.metadata_identity().to_owned(),
            space.clone(),
            step.clone(),
        );
        if let Some(cached) = projection_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()))
        {
            return Ok(cached.as_ref().clone());
        }
        let reader = NativeScmReader::new(store)?;
        let projection = read_admitted_step_projection(
            &reader,
            space,
            step,
            Some(store.layout().root()),
        )?;
        if let Ok(mut cache) = projection_cache().lock() {
            if cache.len() >= 4 {
                cache.remove(0);
            }
            cache.push((key, std::sync::Arc::new(projection.clone())));
        }
        Ok(projection)
    }

    /// Full content verification of exactly the files a transition from
    /// `before` to `after` will write or displace, before any journal,
    /// registration, capture or staging: a missing or corrupt target, or a
    /// base file whose Store copy is damaged, refuses cleanly in O(changed).
    /// Files in `provided` come from the transition's own new objects (e.g. a
    /// rewrite's created Step); the batch supplies and admission verifies them.
    pub(super) fn verify_changed_content(
        &self,
        before: &[CheckoutProjectionEntry],
        after: &[CheckoutProjectionEntry],
        provided: &BTreeSet<FileId>,
    ) -> Result<()> {
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        verify_changed_content(&reader, before, after, provided, true)
    }

    /// Base side only: the Store copy of every file a transition moves to
    /// backups or replaces must be intact before the journal, otherwise the
    /// backup would become its only copy. Target bytes need no pre-read: the
    /// materializer proves every staged byte, and a staging failure closes
    /// the untouched journal without touching the checkout.
    pub(super) fn verify_displaced_content(
        &self,
        before: &[CheckoutProjectionEntry],
        after: &[CheckoutProjectionEntry],
    ) -> Result<()> {
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        verify_changed_content(&reader, before, after, &BTreeSet::new(), false)
    }
}

pub(super) fn verify_changed_content(
    reader: &NativeScmReader<'_>,
    before: &[CheckoutProjectionEntry],
    after: &[CheckoutProjectionEntry],
    provided: &BTreeSet<FileId>,
    target_side: bool,
) -> Result<()> {
    let _span = layrs_store_local::perf::span("transition.verify_changed_content");
    let prior: BTreeMap<&str, &CheckoutProjectionContent> =
        before.iter().map(|e| (e.path.as_str(), &e.content)).collect();
    let next: BTreeMap<&str, &CheckoutProjectionContent> =
        after.iter().map(|e| (e.path.as_str(), &e.content)).collect();
    // Target side: the bytes this transition writes. Base side: the Store copy
    // of every file the transition moves to backups or replaces must be intact
    // too, otherwise the backup would become its only copy.
    let changed_after = after
        .iter()
        .filter(|entry| target_side && prior.get(entry.path.as_str()) != Some(&&entry.content));
    let changed_before = before
        .iter()
        .filter(|entry| next.get(entry.path.as_str()) != Some(&&entry.content));
    let mut verified = BTreeSet::new();
    for entry in changed_after.chain(changed_before) {
        if let CheckoutProjectionContent::File { file_id, .. } = &entry.content {
            if !provided.contains(file_id) && verified.insert(file_id.clone()) {
                let layrs_store::scm::FileAvailability::Present(_) = reader
                    .file_availability_with_budget(
                        file_id,
                        (1 << 24) + 2_048,
                        usize::MAX,
                        128,
                        1 << 24,
                    )?
                else {
                    return Err(NativeClientError::Missing("File"));
                };
            }
        }
    }
    Ok(())
}

type ProjectionKey = (String, String, SpaceId, StepId);

/// Last few admitted Step projections read by this process (see
/// `read_step_projection`).
fn projection_cache(
) -> &'static std::sync::Mutex<Vec<(ProjectionKey, std::sync::Arc<Vec<CheckoutProjectionEntry>>)>>
{
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<Vec<(ProjectionKey, std::sync::Arc<Vec<CheckoutProjectionEntry>>)>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// The admitted Step's checkout projection. With `cache` (the Store root),
/// the persistent projection cache answers by root Tree when it holds a
/// sound entry; a sample of hits (every hit in debug builds) is checked
/// against a recomputation and a differing entry is deleted.
pub(super) fn read_admitted_step_projection(
    reader: &NativeScmReader<'_>,
    space: &SpaceId,
    step: &StepId,
    cache: Option<&std::path::Path>,
) -> Result<Vec<CheckoutProjectionEntry>> {
    let _span = layrs_store_local::perf::span("projection.read_step");
    if !reader.object_admitted_to_space(&StoreObjectId::Step(step.clone()), space)? {
        return Err(NativeClientError::Missing("Step admitted to this Space"));
    }
    let step = reader
        .get_step(step)?
        .ok_or(NativeClientError::Missing("Step"))?;
    let root_tree = step.root_tree_id();
    use layrs_store_local::scm_v1::projection_cache;
    if let Some(root) = cache {
        if let Some(cached) = projection_cache::load(root, root_tree) {
            if !sample_cache_hit() {
                layrs_store_local::perf::count("projection_cache.hit", 1);
                return Ok(cached);
            }
            let computed = project_tree(reader, root_tree)?;
            if computed == cached {
                layrs_store_local::perf::count("projection_cache.sampled_ok", 1);
            } else {
                layrs_store_local::perf::count("projection_cache.mismatch", 1);
                projection_cache::remove(root, root_tree);
            }
            return Ok(computed);
        }
    }
    let computed = project_tree(reader, root_tree)?;
    if let Some(root) = cache {
        // Memoized: the walk just validated this projection's Tree.
        let projection = projection_tree_id(&computed)?;
        projection_cache::save(root, root_tree, &projection, &computed);
    }
    Ok(computed)
}

/// Whether this cache hit is checked against a recomputation: one in 64,
/// every one in debug builds (tests).
fn sample_cache_hit() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static HITS: AtomicU64 = AtomicU64::new(0);
    if cfg!(debug_assertions) {
        return true;
    }
    // Clock ticks can be coarse (100 ns on Windows): mix every bit, then
    // take the top six bits of a multiplicative hash.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos() as u64)
        ^ std::process::id() as u64
        ^ HITS.fetch_add(1, Ordering::Relaxed).rotate_left(32);
    let mixed = (seed ^ (seed >> 29)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    mixed >> 58 == 0
}

/// Walks a Tree into its checkout projection (metadata only).
fn project_tree(
    reader: &NativeScmReader<'_>,
    root_tree: &layrs_core::scm::TreeId,
) -> Result<Vec<CheckoutProjectionEntry>> {
    let _span = layrs_store_local::perf::span("projection.walk");
    let mut pending = vec![(String::new(), root_tree.clone(), 0usize)];
    let mut seen = BTreeSet::new();
    let mut files = BTreeMap::<FileId, u64>::new();
    let mut result = Vec::new();
    let mut total = 0u64;
    while let Some((prefix, id, depth)) = pending.pop() {
        if depth > 128 || seen.len() + result.len() + pending.len() >= 100_000 {
            return Err(NativeClientError::InvalidState(
                "checkout projection exceeds limits",
            ));
        }
        if !seen.insert((prefix.clone(), id.clone())) {
            return Err(NativeClientError::InvalidState(
                "duplicate checkout Tree placement",
            ));
        }
        let tree = reader
            .get_tree(&id)?
            .ok_or(NativeClientError::Missing("Tree"))?;
        // Metadata only: a projection names files and sizes. Bytes are
        // verified where they are used (materialization's VerifiedWriter,
        // readers). One batched query per Tree, not two per file.
        let unknown: Vec<&FileId> = tree
            .entries()
            .iter()
            .filter_map(|entry| match entry.target() {
                TreeEntryTarget::File { file_id } if !files.contains_key(file_id) => Some(file_id),
                _ => None,
            })
            .collect();
        if !unknown.is_empty() {
            files.extend(reader.file_lengths_metadata(unknown)?);
        }
        for entry in tree.entries() {
            let path = if prefix.is_empty() {
                entry.name().as_str().to_owned()
            } else {
                format!("{prefix}/{}", entry.name().as_str())
            };
            let content = match entry.target() {
                TreeEntryTarget::Tree { tree_id } => {
                    pending.push((path, tree_id.clone(), depth + 1));
                    continue;
                }
                TreeEntryTarget::File { file_id } => {
                    let length = *files
                        .get(file_id)
                        .ok_or(NativeClientError::Missing("File"))?;
                    CheckoutProjectionContent::File {
                        file_id: file_id.clone(),
                        byte_length: length,
                        executable: entry.executable(),
                    }
                }
                TreeEntryTarget::Symlink { symlink_id } => CheckoutProjectionContent::Symlink {
                    target: reader
                        .get_symlink(symlink_id)?
                        .ok_or(NativeClientError::Missing("Symlink"))?
                        .target()
                        .to_owned(),
                },
                TreeEntryTarget::Tombstone { .. } => continue,
                TreeEntryTarget::Conflict { .. } => {
                    return Err(NativeClientError::InvalidState(
                        "unresolved conflict cannot be projected",
                    ))
                }
            };
            total = total
                .checked_add(match &content {
                    CheckoutProjectionContent::File { byte_length, .. } => *byte_length,
                    CheckoutProjectionContent::Symlink { target } => target.len() as u64,
                })
                .ok_or(NativeClientError::InvalidState(
                    "checkout projection size overflow",
                ))?;
            result.push(CheckoutProjectionEntry { path, content });
            if result.len() + pending.len() > 100_000 {
                return Err(NativeClientError::InvalidState(
                    "checkout projection exceeds limits",
                ));
            }
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    projection_tree_id(&result)?;
    Ok(result)
}
