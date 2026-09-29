//! Verified local checkout descriptors without materializing every file in RAM.
use super::*;
use layrs_core::scm::{FileId, StepId, TreeEntryTarget};
use layrs_store::scm::{FileAvailability, ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::checkout::{
    projection_tree_id, CheckoutProjectionContent, CheckoutProjectionEntry,
};
use layrs_store_local::scm_v1::{self, NativeScmReader};
use std::collections::{BTreeMap, BTreeSet};

impl NativeWorkspace {
    /// The Step must be admitted to this Space. File bytes are verified lazily
    /// through bounded chunk reads, but are never retained by the projection.
    pub fn read_step_projection(&self, step: &StepId) -> Result<Vec<CheckoutProjectionEntry>> {
        let state = self.state()?;
        let store = scm_v1::open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        if !reader.object_admitted_to_space(&StoreObjectId::Step(step.clone()), state.space.id())? {
            return Err(NativeClientError::Missing("Step admitted to this Space"));
        }
        let step = reader
            .get_step(step)?
            .ok_or(NativeClientError::Missing("Step"))?;
        let mut pending = vec![(String::new(), step.root_tree_id().clone(), 0usize)];
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
                        let length = match files.get(file_id) {
                            Some(length) => *length,
                            None => {
                                let FileAvailability::Present(file) = reader
                                    .file_availability_with_budget(
                                        file_id,
                                        32_768,
                                        4usize * 1024 * 1024 * 1024,
                                        128,
                                        16_384,
                                    )?
                                else {
                                    return Err(NativeClientError::Missing("File"));
                                };
                                let manifest = reader
                                    .get_file_manifest(&file.verified_manifest_ids()[0])?
                                    .ok_or(NativeClientError::Missing("File manifest"))?;
                                if manifest.file_id() != file_id
                                    || manifest.byte_length() > 1024 * 1024 * 1024
                                {
                                    return Err(NativeClientError::InvalidState(
                                        "file projection exceeds limits",
                                    ));
                                }
                                let length = manifest.byte_length();
                                files.insert(file_id.clone(), length);
                                length
                            }
                        };
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
                    .filter(|length| *length <= 4 * 1024 * 1024 * 1024)
                    .ok_or(NativeClientError::InvalidState(
                        "checkout projection exceeds byte limit",
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
}
