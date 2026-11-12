use super::{
    CapturePlan, MutationStamp, NativeClient, PreparedSnapshot, Result, SnapshotBuilder,
    SnapshotLimits,
};
use layrs_core::scm::{RepoPath, WorktreeId};
use layrs_store_local::scm_v1;
use layrs_store_local::scm_v1::checkout::{
    CheckoutContent, CheckoutLimits, CheckoutProjectionContent, CheckoutProjectionEntry,
    CheckoutReader,
};
use std::path::Path;

/// Captures regular files and symlink targets; reserved metadata is excluded.
/// The returned identity must be recorded on the Worktree at initialization.
pub fn capture_checkout(
    root: impl AsRef<Path>,
    expected_identity: Option<&str>,
    limits: SnapshotLimits,
) -> Result<(String, PreparedSnapshot)> {
    capture_checkout_tracked(
        root,
        expected_identity,
        limits,
        &std::collections::BTreeSet::new(),
    )
}
fn capture_checkout_tracked(
    root: impl AsRef<Path>,
    expected_identity: Option<&str>,
    limits: SnapshotLimits,
    tracked: &std::collections::BTreeSet<String>,
) -> Result<(String, PreparedSnapshot)> {
    let checkout =
        CheckoutReader::open(root, expected_identity)?.with_whole_files(limits.whole_files);
    let captured = checkout.capture_tracked(
        CheckoutLimits {
            max_entries: limits.max_entries,
            max_depth: limits.max_depth,
            max_file_bytes: limits.max_file_bytes,
            max_total_bytes: limits.max_total_bytes,
        },
        tracked,
    )?;
    let mut builder = SnapshotBuilder::new(limits);
    for entry in captured {
        let path = RepoPath::parse(entry.path)?;
        match entry.content {
            CheckoutContent::File { bytes, executable } => {
                builder.add_file(path, &bytes, executable)?
            }
            CheckoutContent::Symlink { target } => builder.add_symlink(path, &target)?,
        }
    }
    Ok((checkout.identity(), builder.finish()?))
}

/// A first capture (nothing tracked; content spooled, never a whole-file Vec)
/// also returning the change-index entries (ADR 0025) it proves: each hashed
/// file with the listing metadata observed before it was read.
#[allow(clippy::type_complexity)]
pub(super) fn capture_checkout_streaming_indexed(
    root: &Path,
    limits: SnapshotLimits,
) -> Result<(
    String,
    PreparedSnapshot,
    Vec<(String, scm_v1::change_index::IndexEntry)>,
    Vec<CheckoutProjectionEntry>,
)> {
    use scm_v1::change_index::{IndexEntry, StatKey};
    let checkout = CheckoutReader::open(root, None)?.with_whole_files(limits.whole_files);
    let mut chunks = super::snapshot::SnapshotChunkSpool::new()?;
    let verified_at = scm_v1::change_index::now();
    // A first capture only records content: one stable pass (council r5, D4).
    let captured = checkout.capture_streaming_trusting_single_pass(
        CheckoutLimits {
            max_entries: limits.max_entries,
            max_depth: limits.max_depth,
            max_file_bytes: limits.max_file_bytes,
            max_total_bytes: limits.max_total_bytes,
        },
        &std::collections::BTreeSet::new(),
        None,
        &|_, _| None,
        &mut |chunk| chunks.insert(chunk),
        &mut || Ok(()),
    )?;
    let listed: std::collections::BTreeMap<&str, &std::fs::Metadata> = captured
        .hashed
        .iter()
        .map(|(path, metadata)| (path.as_str(), metadata))
        .collect();
    let mut proven = Vec::new();
    for entry in &captured.entries {
        if let CheckoutProjectionContent::File {
            file_id,
            byte_length,
            executable,
        } = &entry.content
        {
            if let Some(stat) = listed.get(entry.path.as_str()).and_then(|m| StatKey::of(m)) {
                proven.push((
                    entry.path.clone(),
                    IndexEntry {
                        file_id: file_id.clone(),
                        length: *byte_length,
                        executable: *executable,
                        stat,
                        verified_at,
                        native: None,
                    },
                ));
            }
        }
    }
    let snapshot =
        PreparedSnapshot::from_streaming(&captured.entries, captured.manifests, chunks, limits)?;
    Ok((checkout.identity(), snapshot, proven, captured.entries))
}

/// A streaming capture (content spooled, never a whole-file Vec) where chunks in `known` (already admitted to
/// the target Space: durable, verified, closed) are not spooled, so a capture
/// carries only new content instead of every chunk of the checkout.
pub(super) fn capture_checkout_streaming_known(
    root: &Path,
    expected_identity: Option<&str>,
    limits: SnapshotLimits,
    tracked: &std::collections::BTreeSet<String>,
    known: Option<&std::collections::BTreeSet<layrs_core::scm::ChunkId>>,
    expected: Option<&[CheckoutProjectionEntry]>,
    cache: Option<&Path>,
) -> Result<(String, PreparedSnapshot)> {
    let checkout =
        CheckoutReader::open(root, expected_identity)?.with_whole_files(limits.whole_files);
    let mut chunks = super::snapshot::SnapshotChunkSpool::new()?;
    let (entries, manifests) = checkout.capture_streaming_expecting(
        CheckoutLimits {
            max_entries: limits.max_entries,
            max_depth: limits.max_depth,
            max_file_bytes: limits.max_file_bytes,
            max_total_bytes: limits.max_total_bytes,
        },
        tracked,
        expected,
        &mut |chunk| {
            if known.is_some_and(|known| known.contains(chunk.id())) {
                Ok(())
            } else {
                chunks.insert(chunk)
            }
        },
        &mut || Ok(()),
    )?;
    let snapshot = PreparedSnapshot::from_streaming(&entries, manifests, chunks, limits)?;
    if let Some(cache) = cache {
        scm_v1::projection_cache::save(
            cache,
            snapshot.root_tree_id(),
            snapshot.root_tree_id(),
            &entries,
            None,
        );
    }
    Ok((checkout.identity(), snapshot))
}

impl super::NativeWorkspace {
    pub(super) fn capture_projection_streaming(
        &self,
        state: &super::WorkspaceState,
        limits: SnapshotLimits,
    ) -> Result<(String, PreparedSnapshot)> {
        let limits = limits.with_whole_files(limits.whole_files || self.client.whole_files());
        let base = self.read_step_projection(&super::references::base_step(state)?)?;
        // Loaded before hashing; no Store lock is held during the scan. Only
        // the base files' chunks: unchanged content is what a capture re-reads.
        let known = {
            let store = self.open_store()?;
            let reader =
                layrs_store_local::scm_v1::NativeScmReader::new(&store)?.with_positive_evidence();
            reader.admitted_chunk_ids_of_files(
                state.space.id(),
                base.iter().filter_map(|entry| match &entry.content {
                    CheckoutProjectionContent::File { file_id, .. } => Some(file_id),
                    CheckoutProjectionContent::Symlink { .. } => None,
                }),
                1_000_000,
            )?
        };
        let tracked = base.iter().map(|e| e.path.clone()).collect();
        // A clean checkout equal to its base is accepted after one pass.
        capture_checkout_streaming_known(
            Path::new(state.worktree.checkout_root()),
            Some(state.worktree.filesystem_identity()),
            limits,
            &tracked,
            known.as_ref(),
            Some(&base),
            Some(self.store_root()),
        )
    }
    /// A capture recognising unchanged files from the Worktree's change
    /// index (ADR 0025) without opening them, from the base Step's projection
    /// (and, if known, its root Tree), read by the caller before the scan.
    /// Returns the index to record once this capture is accepted: entries of
    /// trusted files are kept, every hashed file gets a fresh proven entry,
    /// and paths that are no longer files are forgotten. The
    /// projection of a new Tree is returned for the caller to record
    /// (`PendingProjection::save`), possibly while it accepts the capture.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn capture_projection_indexed_from(
        &self,
        state: &super::WorkspaceState,
        base: Vec<CheckoutProjectionEntry>,
        base_root: Option<&layrs_core::scm::TreeId>,
        limits: SnapshotLimits,
        index: &mut scm_v1::change_index::ChangeIndex,
        single_pass: bool,
        refs: Option<scm_v1::FileRecordRefs>,
        damaged: bool,
    ) -> Result<(
        PreparedSnapshot,
        scm_v1::change_index::ChangeIndex,
        Option<PendingProjection>,
    )> {
        use scm_v1::change_index::{IndexEntry, StatKey};
        let _span = layrs_store_local::perf::span("capture.indexed");
        // Entries written against a projection trust nothing until it is
        // supplied again (council s3, D1).
        self.resolve_change_index(
            None,
            index,
            &base,
            refs.as_ref()
                .map(|refs| refs as &dyn scm_v1::projection_cache::FileRefs),
        );
        let index = &*index;
        // The base's admitted chunks only avoid spooling bytes the Store has.
        // With an index covering most of the base, few files are read: their
        // chunks are spooled and the batch keeps only what is new, so loading
        // every base chunk id first costs more than it saves.
        let files = base
            .iter()
            .filter(|entry| matches!(entry.content, CheckoutProjectionContent::File { .. }))
            .count();
        // Large base files are the exception: re-hashing one after a small
        // edit must not spool its every unchanged chunk (a 1-byte edit to a
        // 200 GiB file would write 200 GiB to TEMP). Their few admitted chunk
        // ids are loaded so the observer skips them (ADR 0027).
        const LARGE_FILE_BYTES: u64 = 64 * 1024 * 1024;
        let wanted = |entry: &CheckoutProjectionEntry, all: bool| match &entry.content {
            CheckoutProjectionContent::File {
                file_id,
                byte_length,
                ..
            } if all || *byte_length >= LARGE_FILE_BYTES => Some(file_id.clone()),
            _ => None,
        };
        let all = index.len() * 2 < files;
        let selected: Vec<_> = base.iter().filter_map(|entry| wanted(entry, all)).collect();
        let known = if selected.is_empty() {
            None
        } else {
            let store = self.open_store()?;
            let reader =
                layrs_store_local::scm_v1::NativeScmReader::new(&store)?.with_positive_evidence();
            reader.admitted_chunk_ids_of_files(state.space.id(), selected.iter(), 1_000_000)?
        };
        let tracked_span = layrs_store_local::perf::span("capture.tracked");
        let tracked = base.iter().map(|e| e.path.clone()).collect();
        drop(tracked_span);
        let checkout = CheckoutReader::open(
            Path::new(state.worktree.checkout_root()),
            Some(state.worktree.filesystem_identity()),
        )?
        .with_whole_files(limits.whole_files || self.client.whole_files());
        let mut chunks = super::snapshot::SnapshotChunkSpool::new()?;
        // Taken before the listing: no hashed content is credited later than
        // the metadata observed for it.
        let verified_at = scm_v1::change_index::now();
        let trust =
            |path: &str, listed: &std::fs::Metadata| -> Option<scm_v1::checkout::TrustedFile> {
                let stat = StatKey::of(listed)?;
                index
                    .trusted(path, &stat)
                    .map(|entry| scm_v1::checkout::TrustedFile {
                        file_id: entry.file_id.clone(),
                        length: entry.length,
                        executable: entry.executable,
                    })
            };
        let capture = if single_pass {
            CheckoutReader::capture_streaming_trusting_single_pass
        } else {
            CheckoutReader::capture_streaming_trusting
        };
        let captured = capture(
            &checkout,
            CheckoutLimits {
                max_entries: limits.max_entries,
                max_depth: limits.max_depth,
                max_file_bytes: limits.max_file_bytes,
                max_total_bytes: limits.max_total_bytes,
            },
            &tracked,
            Some(&base),
            &trust,
            &mut |chunk| {
                if known
                    .as_ref()
                    .is_some_and(|known| known.contains(chunk.id()))
                {
                    Ok(())
                } else {
                    chunks.insert(chunk)
                }
            },
            &mut || Ok(()),
        )?;
        layrs_store_local::perf::count("capture.index_trusted", captured.trusted as u64);
        layrs_store_local::perf::count("capture.index_hashed", captured.hashed.len() as u64);
        let next_span = layrs_store_local::perf::span("capture.next_index");
        let hashed: std::collections::BTreeMap<&str, &std::fs::Metadata> = captured
            .hashed
            .iter()
            .map(|(path, listed)| (path.as_str(), listed))
            .collect();
        // Nothing read and every entry of the index trusted: each file came
        // from its own entry (same FileId), so the rebuilt index would hold
        // exactly these entries.
        let every_entry_trusted = captured.hashed.is_empty() && captured.trusted == index.len();
        let mut next = if every_entry_trusted {
            index.clone()
        } else {
            index.emptied()
        };
        for entry in captured.entries.iter().filter(|_| !every_entry_trusted) {
            let CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            } = &entry.content
            else {
                continue;
            };
            match hashed.get(entry.path.as_str()) {
                Some(listed) => {
                    if let Some(stat) = StatKey::of(listed) {
                        next.insert(
                            entry.path.clone(),
                            IndexEntry {
                                file_id: file_id.clone(),
                                length: *byte_length,
                                executable: *executable,
                                stat,
                                verified_at,
                                native: None,
                            },
                        );
                    }
                }
                None => {
                    if let Some(old) = index
                        .entry(&entry.path)
                        .filter(|old| &old.file_id == file_id)
                    {
                        next.insert(entry.path.clone(), old.clone());
                    }
                }
            }
        }
        drop(next_span);
        // Exactly the base projection (paths, FileIds, modes): its Tree is
        // the base projection's own, memoized when the projection was read,
        // whether or not some files were read again (a racy entry hashed to
        // its base content). Building and hashing the Trees again would give
        // the same id. Only when that Tree is the base Step's root: a root
        // holding what a checkout cannot represent (Tombstones, empty
        // directories) projects to another Tree, which a capture must build
        // and carry. A Store with an unreadable registered pack gets the
        // full snapshot: what it can no longer prove goes back in.
        if captured.entries == base && !damaged {
            let root = scm_v1::checkout::projection_tree_id(&base)?;
            let step_root = match base_root {
                Some(step_root) => step_root.clone(),
                None => {
                    let store = self.open_store()?;
                    let reader = layrs_store_local::scm_v1::NativeScmReader::new(&store)?;
                    layrs_store::scm::ScmReader::get_step(
                        &reader,
                        &super::references::base_step(state)?,
                    )?
                    .ok_or(super::NativeClientError::Missing("base Step"))?
                    .root_tree_id()
                    .clone()
                }
            };
            if step_root == root {
                next.bind(&step_root, &base);
                return Ok((PreparedSnapshot::unchanged(root), next, None));
            }
        }
        let snapshot = PreparedSnapshot::from_streaming_referencing(
            &captured.entries,
            captured.manifests,
            chunks,
            limits,
        )?;
        // Recorded against the projection of the Tree this capture builds,
        // the next base (council s3, D1).
        next.bind(snapshot.root_tree_id(), &captured.entries);
        let pending = PendingProjection {
            root: snapshot.root_tree_id().clone(),
            entries: captured.entries,
            refs,
        };
        Ok((snapshot, next, Some(pending)))
    }

    pub(super) fn capture_projection(
        &self,
        state: &super::WorkspaceState,
        limits: SnapshotLimits,
    ) -> Result<(String, PreparedSnapshot)> {
        let limits = limits.with_whole_files(limits.whole_files || self.client.whole_files());
        let tracked = self
            .read_step_files(&super::references::base_step(state)?)?
            .into_iter()
            .map(|e| e.path)
            .collect();
        capture_checkout_tracked(
            state.worktree.checkout_root(),
            Some(state.worktree.filesystem_identity()),
            limits,
            &tracked,
        )
    }
}

impl NativeClient {
    /// Reads CAS preconditions before touching the filesystem. No Store lock is
    /// retained while hashing; prepare_capture rechecks the whole base afterward.
    pub fn prepare_disk_capture(
        &self,
        worktree_id: &WorktreeId,
        limits: SnapshotLimits,
        stamp: MutationStamp,
    ) -> Result<CapturePlan> {
        self.prepare_disk_capture_with_reason(
            worktree_id,
            limits,
            stamp,
            layrs_core::scm::CaptureReason::ManualSnapshot,
        )
    }

    pub fn prepare_disk_capture_with_reason(
        &self,
        worktree_id: &WorktreeId,
        limits: SnapshotLimits,
        stamp: MutationStamp,
        reason: layrs_core::scm::CaptureReason,
    ) -> Result<CapturePlan> {
        self.prepare_disk_capture_recovering(worktree_id, limits, stamp, reason, false)
    }

    /// `prepare_disk_capture_with_reason`, `recovery_required` as in
    /// `prepare_capture_recovering`.
    pub(super) fn prepare_disk_capture_recovering(
        &self,
        worktree_id: &WorktreeId,
        limits: SnapshotLimits,
        stamp: MutationStamp,
        reason: layrs_core::scm::CaptureReason,
        recovery_required: bool,
    ) -> Result<CapturePlan> {
        let base = self.capture_base(worktree_id)?;
        let workspace = super::NativeWorkspace {
            client: self.clone(),
            worktree_id: worktree_id.clone(),
            held: Default::default(),
        };
        let (_, snapshot) = workspace.capture_projection_streaming(&workspace.state()?, limits)?;
        self.prepare_capture_recovering(&base, snapshot, stamp, reason, recovery_required)
    }
}

/// The projection of the Tree a capture builds, to record in the projection
/// cache: the next command finds it without re-reading the Trees (derived
/// data, by Tree id; a failed write only costs that read).
pub(super) struct PendingProjection {
    root: layrs_core::scm::TreeId,
    entries: Vec<CheckoutProjectionEntry>,
    /// File record positions of the catalog the capture read its base from
    /// (files new in this capture are named by id).
    refs: Option<scm_v1::FileRecordRefs>,
}

impl PendingProjection {
    pub(super) fn save(&self, store_root: &Path) {
        scm_v1::projection_cache::save(
            store_root,
            &self.root,
            &self.root,
            &self.entries,
            self.refs
                .as_ref()
                .map(|refs| refs as &dyn scm_v1::projection_cache::FileRefs),
        );
    }
}
