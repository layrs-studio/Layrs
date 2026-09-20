use super::{references::base_step, NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::StepId;
pub use layrs_core::scm::{NativeDiffLine, NativeFileChange};
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{
    self,
    checkout::{
        CheckoutContent, CheckoutEntry, CheckoutLimits, CheckoutProjectionContent,
        CheckoutProjectionEntry, CheckoutReader,
    },
    NativeScmReader,
};

impl NativeWorkspace {
    pub fn working_diff(&self) -> Result<Vec<NativeFileChange>> {
        let base = self.client().capture_base(self.worktree_id())?;
        let state = self.state()?;
        let before = self.read_step_projection(&base_step(&state)?)?;
        let checkout = CheckoutReader::open(
            state.worktree.checkout_root(),
            Some(state.worktree.filesystem_identity()),
        )?;
        let (after, _) = checkout.capture_streaming(
            CheckoutLimits {
                max_entries: 100_000,
                max_depth: 128,
                max_file_bytes: 1024 * 1024 * 1024,
                max_total_bytes: 4usize * 1024 * 1024 * 1024,
            },
            &before.iter().map(|e| e.path.clone()).collect(),
            &mut |_| Ok(()),
            &mut || Ok(()),
        )?;
        if self.client().capture_base(self.worktree_id())? != base {
            return Err(NativeClientError::StaleCapture);
        }
        if before == after {
            return Ok(vec![]);
        }
        if small_projection(&before, &after) {
            let old = self.read_step_files(&base_step(&state)?)?;
            let current = checkout.capture_tracked(
                CheckoutLimits {
                    max_entries: 100_000,
                    max_depth: 128,
                    max_file_bytes: 8 * 1024 * 1024,
                    max_total_bytes: 8 * 1024 * 1024,
                },
                &before.iter().map(|e| e.path.clone()).collect(),
            )?;
            if current
                .iter()
                .map(CheckoutProjectionEntry::from)
                .ne(after.iter().cloned())
                || self.client().capture_base(self.worktree_id())? != base
            {
                return Err(NativeClientError::StaleCapture);
            }
            return Ok(diff_entries(&old, &current));
        }
        Ok(diff_projection_entries(&before, &after))
    }
    pub fn step_diff(
        &self,
        step: &StepId,
        parent: Option<&StepId>,
    ) -> Result<Vec<NativeFileChange>> {
        let target_step = step;
        let after = self.read_step_projection(step)?;
        let store = scm_v1::open(self.store_root())?;
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
