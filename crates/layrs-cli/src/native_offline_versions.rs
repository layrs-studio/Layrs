use super::error;
use crate::{args::offline::VersionSelector, error::CliError};
use layrs_core::projection::ProjectionVersion;
use layrs_store_local::projection::ProjectionStore;
use serde_json::Value;
use std::{io::Read, path::Path};

pub(super) fn resolve(
    store: &ProjectionStore,
    selector: &VersionSelector,
    current: &mut Option<ProjectionVersion>,
) -> Result<ProjectionVersion, CliError> {
    match selector {
        VersionSelector::Current => {
            if current.is_none() {
                *current = Some(store.current_version().map_err(error)?);
            }
            Ok(current.as_ref().expect("current version loaded").clone())
        }
        VersionSelector::Generation(generation) => {
            store.generation_version(*generation).map_err(error)
        }
        VersionSelector::Capture(sequence) => {
            let patch = store.captured_patch(*sequence).map_err(error)?;
            Ok(ProjectionVersion::Capture {
                // Imported captures keep their original publication identity;
                // local version operands belong to the destination Worktree.
                worktree_id: store.identity().worktree_id.clone(),
                sequence: patch.sequence,
                digest: patch.digest,
            })
        }
        VersionSelector::File(path) => read_file(path),
    }
}

pub(super) fn read_file(path: &Path) -> Result<ProjectionVersion, CliError> {
    const LIMIT: u64 = 64 * 1024;
    let metadata = std::fs::symlink_metadata(path).map_err(error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > LIMIT {
        return Err(CliError::usage(
            "version file must be a regular file no larger than 64 KiB",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(error)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > LIMIT {
        return Err(CliError::usage("version file exceeds 64 KiB"));
    }
    let document: Value = serde_json::from_slice(&bytes).map_err(error)?;
    let version = if document.get("ok").is_some() {
        if document["ok"] != true {
            return Err(CliError::usage("version file contains a failed command"));
        }
        document
            .get("data")
            .and_then(|v| v.get("version"))
            .ok_or_else(|| CliError::usage("command result has no exact version"))?
    } else {
        &document
    };
    // The Store checks this exact descriptor against immutable local metadata;
    // the file supplies neither a remote grant nor an inferred version.
    serde_json::from_value(version.clone()).map_err(error)
}
