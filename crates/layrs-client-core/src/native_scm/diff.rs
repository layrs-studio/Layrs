use super::{references::base_step, NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::StepId;
pub use layrs_core::scm::{NativeDiffLine, NativeFileChange};
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{
    checkout::{
        CheckoutContent, CheckoutEntry, CheckoutLimits, CheckoutProjectionContent,
        CheckoutProjectionEntry, CheckoutReader,
    },
    NativeScmReader,
};

/// What `NativeWorkspace::status_report` observed.
pub struct StatusReport {
    pub needs_repair: bool,
    /// `None` when a repair is pending (the folder was not compared).
    pub dirty: Option<bool>,
    pub state: super::WorkspaceState,
}

impl NativeWorkspace {
    pub fn working_diff(&self) -> Result<Vec<NativeFileChange>> {
        self.working_diff_with(true)
    }

    /// `trust_index: false` reads every file (ADR 0025 `--full`).
    pub fn working_diff_with(&self, trust_index: bool) -> Result<Vec<NativeFileChange>> {
        Ok(self.working_changes(trust_index, true)?.unwrap_or_default())
    }

    /// Whether the folder differs from its base, from the projection
    /// comparison alone (no diff text, no extra content read).
    pub(super) fn working_tree_changed(&self, trust_index: bool) -> Result<bool> {
        Ok(self
            .working_changes(trust_index, false)?
            .is_none_or(|changes| !changes.is_empty()))
    }

    /// Everything `layrs status` reports, reading the Store in one session
    /// before the folder scan and once more to confirm the base did not move:
    /// whether a repair is pending, else whether the folder changed, and the
    /// Workspace state.
    pub fn status_report(&self, trust_index: bool) -> Result<StatusReport> {
        let span = layrs_store_local::perf::span("status.session");
        let store = self.open_store()?;
        let state = self.state_with(&store)?;
        if layrs_store_local::scm_v1::materialize::recovery_pending(&store, self.worktree_id())? {
            return Ok(StatusReport {
                needs_repair: true,
                dirty: None,
                state,
            });
        }
        let base = self.client().capture_base_with(&store, self.worktree_id())?;
        let before = self.read_step_projection_with(&store, state.space.id(), &base_step(&state)?)?;
        drop(store);
        drop(span);
        let changes = self.working_changes_from(trust_index, false, base, &state, before)?;
        Ok(StatusReport {
            needs_repair: false,
            dirty: Some(changes.is_none_or(|changes| !changes.is_empty())),
            state,
        })
    }

    /// `None` when the folder changed and `details` was not requested.
    fn working_changes(
        &self,
        trust_index: bool,
        details: bool,
    ) -> Result<Option<Vec<NativeFileChange>>> {
        let span = layrs_store_local::perf::span("dirty.base_state");
        let (base, state, before) = {
            let store = self.open_store()?;
            let base = self.client().capture_base_with(&store, self.worktree_id())?;
            let state = self.state_with(&store)?;
            let before =
                self.read_step_projection_with(&store, state.space.id(), &base_step(&state)?)?;
            (base, state, before)
        };
        drop(span);
        self.working_changes_from(trust_index, details, base, &state, before)
    }

    fn working_changes_from(
        &self,
        trust_index: bool,
        details: bool,
        base: super::CaptureBase,
        state: &super::WorkspaceState,
        before: Vec<CheckoutProjectionEntry>,
    ) -> Result<Option<Vec<NativeFileChange>>> {
        let checkout = CheckoutReader::open(
            state.worktree.checkout_root(),
            Some(state.worktree.filesystem_identity()),
        )?;
        let limits = CheckoutLimits {
            max_entries: 100_000,
            max_depth: 128,
            max_file_bytes: usize::MAX,
            max_total_bytes: usize::MAX,
        };
        let span = layrs_store_local::perf::span("status.tracked_index");
        let tracked = before.iter().map(|e| e.path.clone()).collect();
        // A clean checkout (equal to its base) is accepted after one pass. A
        // Worktree with a change index (ADR 0025) reads only the files whose
        // listing changed; this read-only view never records new entries
        // (their content is not admitted to the Store).
        let index = layrs_store_local::scm_v1::change_index::ChangeIndex::load(
            self.store_root(),
            self.worktree_id(),
        )
        .filter(|_| trust_index);
        drop(span);
        let after = match &index {
            Some(index) => {
                let mut trust = |path: &str,
                                 _full: &std::path::Path,
                                 listed: &std::fs::Metadata| {
                    let stat = layrs_store_local::scm_v1::change_index::StatKey::of(listed)?;
                    index.trusted(path, &stat).map(|entry| {
                        layrs_store_local::scm_v1::checkout::TrustedFile {
                            file_id: entry.file_id.clone(),
                            length: entry.length,
                            executable: entry.executable,
                        }
                    })
                };
                // Read-only: one stable pass, as git status (council r5, D4).
                checkout
                    .capture_streaming_trusting_single_pass(
                        limits,
                        &tracked,
                        Some(&before),
                        &mut trust,
                        &mut |_| Ok(()),
                        &mut || Ok(()),
                    )?
                    .entries
            }
            None => {
                checkout
                    .capture_streaming_expecting(
                        limits,
                        &tracked,
                        Some(&before),
                        &mut |_| Ok(()),
                        &mut || Ok(()),
                    )?
                    .0
            }
        };
        let span = layrs_store_local::perf::span("status.recheck_compare");
        if self.client().capture_base(self.worktree_id())? != base {
            return Err(NativeClientError::StaleCapture);
        }
        if before == after {
            return Ok(Some(vec![]));
        }
        drop(span);
        if !details {
            return Ok(None);
        }
        if small_projection(&before, &after) {
            // Only paths whose projection differs can produce diff lines; read
            // (and verify) base content for those alone, not the whole Step.
            let prior: std::collections::BTreeMap<_, _> =
                before.iter().map(|e| (e.path.as_str(), &e.content)).collect();
            let next: std::collections::BTreeMap<_, _> =
                after.iter().map(|e| (e.path.as_str(), &e.content)).collect();
            let changed: std::collections::BTreeSet<String> = prior
                .keys()
                .chain(next.keys())
                .filter(|path| prior.get(*path) != next.get(*path))
                .map(|path| (*path).to_owned())
                .collect();
            let old = self.read_step_files_selected(&base_step(state)?, &changed)?;
            // Current bytes of the changed paths only, each proven equal to
            // the projection just observed (otherwise the folder moved on).
            let current = read_changed_files(checkout.root(), &after, &changed)?;
            if self.client().capture_base(self.worktree_id())? != base {
                return Err(NativeClientError::StaleCapture);
            }
            return Ok(Some(diff_entries(&old, &current)));
        }
        Ok(Some(diff_projection_entries(&before, &after)))
    }
    pub fn step_diff(
        &self,
        step: &StepId,
        parent: Option<&StepId>,
    ) -> Result<Vec<NativeFileChange>> {
        let target_step = step;
        let after = self.read_step_projection(step)?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let step = reader
            .get_step(step)?
            .ok_or(NativeClientError::Missing("Step"))?;
        let selected = match parent {
            Some(parent) if step.parent_step_ids().contains(parent) => Some(parent),
            Some(_) => {
                return Err(NativeClientError::InvalidState(
                    "selected base is not a Step parent",
                ));
            }
            None => match step.parent_step_ids() {
                [] => None,
                [single] => Some(single),
                _ => {
                    return Err(NativeClientError::InvalidState(
                        "merge Step diff requires explicit parent",
                    ));
                }
            },
        };
        let before = selected
            .map(|p| self.read_step_projection(p))
            .transpose()?
            .unwrap_or_default();
        if small_projection(&before, &after) {
            let old = selected
                .map(|p| self.read_step_files(p))
                .transpose()?
                .unwrap_or_default();
            return Ok(diff_entries(&old, &self.read_step_files(target_step)?));
        }
        Ok(diff_projection_entries(&before, &after))
    }
}

/// Reads the changed paths of `after` from the checkout, each verified against
/// its observed projection entry (content id, length, link target).
fn read_changed_files(
    root: &std::path::Path,
    after: &[CheckoutProjectionEntry],
    changed: &std::collections::BTreeSet<String>,
) -> Result<Vec<CheckoutEntry>> {
    let mut current = Vec::new();
    for entry in after.iter().filter(|entry| changed.contains(&entry.path)) {
        let full = entry
            .path
            .split('/')
            .fold(root.to_path_buf(), |full, part| full.join(part));
        let content = match &entry.content {
            CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            } => {
                let bytes = std::fs::read(&full)?;
                if bytes.len() as u64 != *byte_length
                    || &layrs_core::scm::FileId::from_bytes(&bytes) != file_id
                {
                    return Err(NativeClientError::StaleCapture);
                }
                CheckoutContent::File {
                    bytes,
                    executable: *executable,
                }
            }
            CheckoutProjectionContent::Symlink { target } => {
                let link = std::fs::read_link(&full)?;
                if link.to_str() != Some(target.as_str()) {
                    return Err(NativeClientError::StaleCapture);
                }
                CheckoutContent::Symlink {
                    target: target.clone(),
                }
            }
        };
        current.push(CheckoutEntry {
            path: entry.path.clone(),
            content,
        });
    }
    Ok(current)
}

fn small_projection(before: &[CheckoutProjectionEntry], after: &[CheckoutProjectionEntry]) -> bool {
    before
        .iter()
        .chain(after)
        .try_fold(0u64, |total, entry| {
            let length = match &entry.content {
                CheckoutProjectionContent::File { byte_length, .. } => *byte_length,
                CheckoutProjectionContent::Symlink { target } => target.len() as u64,
            };
            total
                .checked_add(length)
                .filter(|total| *total <= 8 * 1024 * 1024)
        })
        .is_some()
}

/// Content classification is deliberately omitted along with text for metadata
/// summaries. `binary=false,text_omitted=true` never promises an UTF-8 preview.
pub(super) fn diff_projection_entries(
    before: &[CheckoutProjectionEntry],
    after: &[CheckoutProjectionEntry],
) -> Vec<NativeFileChange> {
    use std::collections::{BTreeMap, BTreeSet};
    let before: BTreeMap<_, _> = before
        .iter()
        .map(|e| (e.path.as_str(), &e.content))
        .collect();
    let after: BTreeMap<_, _> = after
        .iter()
        .map(|e| (e.path.as_str(), &e.content))
        .collect();
    let paths: BTreeSet<_> = before.keys().chain(after.keys()).copied().collect();
    let length = |value: Option<&&CheckoutProjectionContent>| {
        value.map(|v| match v {
            CheckoutProjectionContent::File { byte_length, .. } => *byte_length as usize,
            CheckoutProjectionContent::Symlink { target } => target.len(),
        })
    };
    let digest = |value: Option<&&CheckoutProjectionContent>| {
        value.map(|v| match v {
            CheckoutProjectionContent::File { file_id, .. } => file_id.to_string(),
            CheckoutProjectionContent::Symlink { target } => {
                format!("symlink:{}", blake3::hash(target.as_bytes()).to_hex())
            }
        })
    };
    let executable = |value: Option<&&CheckoutProjectionContent>| {
        matches!(
            value,
            Some(CheckoutProjectionContent::File {
                executable: true,
                ..
            })
        )
    };
    paths
        .into_iter()
        .filter_map(|path| {
            let old = before.get(path);
            let new = after.get(path);
            if old == new {
                return None;
            }
            Some(NativeFileChange {
                path: path.into(),
                status: if old.is_none() {
                    "added"
                } else if new.is_none() {
                    "deleted"
                } else {
                    "modified"
                },
                before_bytes: length(old),
                after_bytes: length(new),
                before_digest: digest(old),
                after_digest: digest(new),
                executable_changed: executable(old) != executable(new),
                binary: false,
                text_diff: None,
                text_omitted: true,
            })
        })
        .collect()
}

pub(super) fn diff_entries(
    before: &[CheckoutEntry],
    after: &[CheckoutEntry],
) -> Vec<NativeFileChange> {
    fn entries(input: &[CheckoutEntry]) -> Vec<layrs_core::scm::DiffEntry<'_>> {
        input
            .iter()
            .map(|entry| layrs_core::scm::DiffEntry {
                path: &entry.path,
                content: match &entry.content {
                    CheckoutContent::File { bytes, executable } => {
                        layrs_core::scm::DiffContent::File {
                            bytes,
                            executable: *executable,
                        }
                    }
                    CheckoutContent::Symlink { target } => {
                        layrs_core::scm::DiffContent::Symlink { target }
                    }
                },
            })
            .collect()
    }
    layrs_core::scm::diff_entries(&entries(before), &entries(after))
}
