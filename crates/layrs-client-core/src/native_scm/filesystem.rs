use super::{
    CapturePlan, MutationStamp, NativeClient, PreparedSnapshot, Result, SnapshotBuilder,
    SnapshotLimits,
};
use layrs_core::scm::{RepoPath, WorktreeId};
use layrs_store_local::scm_v1::checkout::{CheckoutContent, CheckoutLimits, CheckoutReader};
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
    let checkout = CheckoutReader::open(root, expected_identity)?;
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

/// File content is retained in a private bounded spool, never a whole-file Vec.
pub fn capture_checkout_streaming(
    root: impl AsRef<Path>,
    expected_identity: Option<&str>,
    limits: SnapshotLimits,
    tracked: &std::collections::BTreeSet<String>,
) -> Result<(String, PreparedSnapshot)> {
    let checkout = CheckoutReader::open(root, expected_identity)?;
    let mut chunks = super::snapshot::SnapshotChunkSpool::new()?;
    let (entries, manifests) = checkout.capture_streaming(
        CheckoutLimits {
            max_entries: limits.max_entries,
            max_depth: limits.max_depth,
            max_file_bytes: limits.max_file_bytes,
            max_total_bytes: limits.max_total_bytes,
        },
        tracked,
        &mut |chunk| chunks.insert(chunk),
        &mut || Ok(()),
    )?;
    let snapshot = PreparedSnapshot::from_streaming(&entries, manifests, chunks, limits)?;
    Ok((checkout.identity(), snapshot))
}

impl super::NativeWorkspace {
    pub(super) fn capture_projection_streaming(
        &self,
        state: &super::WorkspaceState,
        limits: SnapshotLimits,
    ) -> Result<(String, PreparedSnapshot)> {
        let tracked = self
            .read_step_projection(&super::references::base_step(state)?)?
            .into_iter()
            .map(|e| e.path)
            .collect();
        capture_checkout_streaming(
            state.worktree.checkout_root(),
            Some(state.worktree.filesystem_identity()),
            limits,
            &tracked,
        )
    }
    pub(super) fn capture_projection(
        &self,
        state: &super::WorkspaceState,
        limits: SnapshotLimits,
    ) -> Result<(String, PreparedSnapshot)> {
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
        let base = self.capture_base(worktree_id)?;
        let workspace = super::NativeWorkspace {
            client: NativeClient {
                root: self.root.clone(),
            },
            worktree_id: worktree_id.clone(),
        };
        let (_, snapshot) = workspace.capture_projection_streaming(&workspace.state()?, limits)?;
        self.prepare_capture_with_reason(&base, snapshot, stamp, reason)
    }
}
