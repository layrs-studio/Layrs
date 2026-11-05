//! Explicit local restore artifacts. Digest computation and mutation stay in the Store.
use super::{absolute, error};
use crate::{error::CliError, render::Rendered};
use layrs_core::scm::ObjectId;
use layrs_store_local::projection::{CapturedFile, RestoreDestination, RestorePreview};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::Path,
};

const MAX_PREVIEW_BYTES: usize = 1024 * 1024;

pub(super) fn read_preview(
    path: &Path,
    expected_digest: &ObjectId,
) -> Result<RestorePreview, CliError> {
    let metadata = std::fs::symlink_metadata(path).map_err(error)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_PREVIEW_BYTES as u64
    {
        return Err(CliError::usage(
            "restore preview must be a regular file no larger than 1 MiB",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(error)?
        .take(MAX_PREVIEW_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > MAX_PREVIEW_BYTES {
        return Err(CliError::usage("restore preview exceeds 1 MiB"));
    }
    // The artifact is the exact typed preview, not a request or an error envelope.
    let preview: RestorePreview = serde_json::from_slice(&bytes).map_err(error)?;
    require_destination(&preview)?;
    if preview.digest != expected_digest.as_str() {
        return Err(CliError::usage(
            "restore preview does not match --expected-digest; review the intended artifact before applying",
        ));
    }
    // The engine recomputes this digest and compares all fresh checkout expectations.
    Ok(preview)
}

pub(super) fn save_preview(mut data: Value, output: &Path) -> Result<Rendered, CliError> {
    let preview: RestorePreview = serde_json::from_value(data["preview"].clone()).map_err(error)?;
    let destination = require_destination(&preview)?;
    let digest = ObjectId::parse(&preview.digest).map_err(error)?;
    let bytes = serde_json::to_vec_pretty(&preview).map_err(error)?;
    if bytes.len() > MAX_PREVIEW_BYTES {
        return Err(CliError::usage(
            "restore preview exceeds the 1 MiB artifact limit; select fewer paths",
        ));
    }
    let output = absolute(output)?;
    let human = describe(&preview, destination, &output, &digest);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(error)?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| error(format!(
        "could not finish writing restore preview {}: {e}; a partial artifact is retained at this path", output.display()
    )))?;
    data["output"] = json!(output);
    Ok(Rendered { human, data })
}

fn require_destination(preview: &RestorePreview) -> Result<&RestoreDestination, CliError> {
    preview.destination.as_ref().ok_or_else(|| CliError::usage(
        "restore preview has no bound destination; generate and review a new preview for this checkout",
    ))
}

fn describe(
    preview: &RestorePreview,
    destination: &RestoreDestination,
    output: &Path,
    digest: &ObjectId,
) -> String {
    let mut lines = vec![
        "Read-only local restore preview. No restore was applied or recorded in history.".into(),
        format!(
            "Destination Store: {}. Worktree: {}.",
            destination.identity.store_id, destination.identity.worktree_id
        ),
        format!(
            "Destination Space: {}. Origin: {}. Principal: {}.",
            destination.identity.space_id,
            destination.identity.origin,
            destination.identity.principal_id
        ),
        format!(
            "Generation: {}. Operation: {}.",
            preview.projection_generation, preview.operation_id
        ),
        format!(
            "Source: {}",
            serde_json::to_string(&preview.source).expect("typed restore source serializes")
        ),
    ];
    let paths = preview
        .before
        .keys()
        .chain(preview.after.keys())
        .collect::<BTreeSet<_>>();
    for path in paths {
        let before = preview.before.get(path).and_then(Option::as_ref);
        let after = preview.after.get(path).and_then(Option::as_ref);
        let action = match (before, after) {
            (None, Some(_)) => "create",
            (Some(_), None) => "delete",
            _ => "replace",
        };
        lines.push(format!(
            "{action} {}\n  before: {}\n  after: {}",
            path.as_str(),
            target(before),
            target(after)
        ));
    }
    lines.push(format!(
        "Preview artifact: {}\nDigest: {digest}",
        output.display()
    ));
    lines.push(format!("Review these changes, then run restore-apply with this artifact and --expected-digest {digest}."));
    lines.join("\n")
}

fn target(file: Option<&CapturedFile>) -> String {
    file.map_or_else(
        || "absent".into(),
        |f| {
            format!(
                "{} ({} bytes, executable={})",
                f.file_id, f.bytes, f.executable
            )
        },
    )
}
