//! CLI presentation and argument adaptation only; local mutations use the shared engine.
use crate::{
    args::{
        Cli, OfflineCommand,
        offline::{OfflineRequest, RemoteOfflineCommand},
    },
    error::CliError,
    render::Rendered,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use layrs_client_core::native_scm::{
    NativeRemoteAction, ProjectionOfflineAction as Action, execute_projection_offline,
};
use layrs_core::projection::AvailabilityRequest;
use layrs_store_local::projection::ProjectionStore;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[path = "native_offline_render.rs"]
mod presentation;
#[path = "native_offline_restore.rs"]
mod restore;
#[path = "native_offline_versions.rs"]
mod versions;

pub fn run(cli: &Cli, command: &OfflineCommand) -> Result<Rendered, CliError> {
    // Abandonment validates its explicit family and retained destination. A
    // changed or missing original source must not prevent this recovery path.
    if let OfflineCommand::AbandonConversion {
        family,
        destination,
    } = command
    {
        let family_root = absolute(family)?;
        let data = execute_projection_offline(
            &family_root,
            Action::AbandonConversion {
                family_root: family_root.clone(),
                destination: absolute(destination)?,
            },
        )
        .map_err(error)?;
        return Ok(Rendered {
            human: presentation::render(command, &data),
            data,
        });
    }
    let start = cli
        .globals
        .space
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(error)?;
    if matches!(command, OfflineCommand::CancelReservation) {
        let (root, expected) = discover_validated_root(&start, |candidate| {
            ProjectionStore::inspect_recovery(candidate)
                .map(|inspection| inspection.binding)
                .map_err(error)
        })?;
        let data = execute_projection_offline(&root, Action::CancelSharedReservation { expected })
            .map_err(error)?;
        return Ok(Rendered {
            human: presentation::render(command, &data),
            data,
        });
    }
    let conversion = matches!(
        command,
        OfflineCommand::CreateFamily { .. }
            | OfflineCommand::Convert { .. }
            | OfflineCommand::ResumeConversion { .. }
    );
    let root = discover_root(&start, conversion)?;
    let action = match command {
        OfflineCommand::Status => Action::CurrentVersion {},
        OfflineCommand::Check(spec) => Action::Availability {
            request: request(&root, spec)?,
        },
        OfflineCommand::Compare(spec) => Action::Compare {
            request: request(&root, spec)?,
        },
        OfflineCommand::RestorePreview {
            version,
            paths,
            expected_generation,
            ..
        } => {
            let store = ProjectionStore::open(&root).map_err(error)?;
            Action::PreviewRestore {
                expected_generation: *expected_generation,
                version: versions::resolve(&store, version, &mut None)?,
                paths: paths.clone(),
            }
        }
        OfflineCommand::RestoreApply {
            preview,
            expected_digest,
        } => Action::ApplyRestore {
            preview: restore::read_preview(preview, expected_digest)?,
        },
        OfflineCommand::Read {
            version,
            path,
            max_bytes,
            ..
        } => {
            let store = ProjectionStore::open(&root).map_err(error)?;
            Action::ReadFile {
                version: versions::resolve(&store, version, &mut None)?,
                path: path.clone(),
                max_bytes: *max_bytes,
            }
        }
        OfflineCommand::Preparations => Action::Preparations {},
        OfflineCommand::Release { id } => Action::ReleasePreparation { id: id.clone() },
        OfflineCommand::CancelReservation => {
            unreachable!("handled through nonmigrating recovery inspection")
        }
        OfflineCommand::ReleaseGeneration { generation } => Action::ReleaseGenerationRetention {
            generation: *generation,
        },
        OfflineCommand::Quota { bytes: None } => Action::FamilyUsage {},
        OfflineCommand::Quota { bytes: Some(bytes) } => Action::SetFamilyQuota { bytes: *bytes },
        OfflineCommand::Evict => Action::EvictUnretained {},
        OfflineCommand::CreateFamily { path, quota_bytes } => Action::CreateFamily {
            path: absolute(path)?,
            quota_bytes: *quota_bytes,
        },
        OfflineCommand::Convert {
            family,
            destination,
        } => Action::ConvertIntoFamily {
            family_root: absolute(family)?,
            destination: absolute(destination)?,
        },
        OfflineCommand::ResumeConversion { destination } => Action::ResumeConversion {
            destination: absolute(destination)?,
        },
        OfflineCommand::AbandonConversion { .. } => {
            unreachable!("handled without source discovery")
        }
    };
    let data = execute_projection_offline(&root, action).map_err(error)?;
    if let OfflineCommand::RestorePreview { output, .. } = command {
        return restore::save_preview(data, output);
    }
    if let OfflineCommand::Read {
        output, max_bytes, ..
    } = command
    {
        return write_read_result(data, output, *max_bytes);
    }
    Ok(Rendered {
        human: presentation::render(command, &data),
        data,
    })
}

pub(crate) fn request(root: &Path, spec: &OfflineRequest) -> Result<AvailabilityRequest, CliError> {
    let store = ProjectionStore::open(root).map_err(error)?;
    // Repeated `current` operands name one exact generation even if another
    // process advances the checkout while the command is being assembled.
    let mut current = None;
    let request = AvailabilityRequest {
        version: versions::resolve(&store, &spec.version, &mut current)?,
        other: spec
            .other
            .as_ref()
            .map(|v| versions::resolve(&store, v, &mut current))
            .transpose()?,
        paths: spec.paths.clone(),
        operation: spec.operation,
    };
    request.validate().map_err(error)?;
    Ok(request)
}

pub(crate) fn remote_action(
    root: &Path,
    args: Vec<String>,
    generation: Option<String>,
) -> Result<(PathBuf, NativeRemoteAction), CliError> {
    let parsed = crate::args::offline::parse_remote(args, generation)
        .map_err(|e| CliError::usage(e.to_string()))?;
    let root = checkout_root(root)?;
    let action = match parsed {
        RemoteOfflineCommand::Observe {
            expected_generation,
            step,
            paths,
        } => NativeRemoteAction::ObserveProjectionVersion {
            expected_generation,
            step,
            paths,
        },
        RemoteOfflineCommand::Prepare {
            expected_generation,
            request: spec,
        } => NativeRemoteAction::PrepareProjectionOffline {
            expected_generation,
            request: request(&root, &spec)?,
        },
        RemoteOfflineCommand::Resume {
            expected_generation,
            id,
        } => NativeRemoteAction::ResumeProjectionOffline {
            expected_generation,
            preparation_id: id,
        },
    };
    Ok((root, action))
}

pub(crate) fn checkout_root(start: &Path) -> Result<PathBuf, CliError> {
    discover_root(start, false)
}

fn discover_root(start: &Path, conversion_source: bool) -> Result<PathBuf, CliError> {
    discover_validated_root(start, |candidate| {
        if conversion_source {
            ProjectionStore::open_conversion_source(candidate).map_err(error)?;
        } else {
            ProjectionStore::open(candidate).map_err(error)?;
        }
        Ok(())
    })
    .map(|(root, ())| root)
}

fn discover_validated_root<T>(
    start: &Path,
    validate: impl FnOnce(&Path) -> Result<T, CliError>,
) -> Result<(PathBuf, T), CliError> {
    crate::native_vfs::refuse_ordinary_workspace(start)?;
    let start = absolute(start)?;
    for candidate in start.ancestors() {
        match std::fs::symlink_metadata(candidate.join(".layrs")) {
            Ok(_) => {
                let validated = validate(candidate)?;
                return Ok((candidate.to_path_buf(), validated));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e)),
        }
    }
    Err(CliError::runtime(
        "No partial checkout found. Select one with --space PATH.",
    ))
}

fn write_read_result(mut data: Value, output: &Path, max_bytes: u32) -> Result<Rendered, CliError> {
    let content = data["content"]
        .as_str()
        .ok_or_else(|| error("offline read response has no content"))?;
    if data["encoding"] != "base64"
        || max_bytes == 0
        || max_bytes > 1024 * 1024
        || content.len() > ((max_bytes as usize + 2) / 3) * 4
    {
        return Err(error(
            "offline read response exceeds its encoding or size bound",
        ));
    }
    let bytes = STANDARD.decode(content).map_err(error)?;
    if bytes.len() > max_bytes as usize || data["bytes"].as_u64() != Some(bytes.len() as u64) {
        return Err(error(
            "offline read response length does not match its bound",
        ));
    }
    let output = absolute(output)?;
    // Decode and verify the complete bounded engine response before creating a file.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(error)?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| {
        error(format!("offline read could not finish writing {}: {e}; a partial output file is retained at this path", output.display()))
    })?;
    let object = data
        .as_object_mut()
        .ok_or_else(|| error("offline read response is not an object"))?;
    object.remove("content");
    object.remove("encoding");
    object.insert("output".into(), json!(output));
    Ok(Rendered {
        human: format!("Wrote {} bytes to {}", bytes.len(), output.display()),
        data,
    })
}

fn absolute(path: &Path) -> Result<PathBuf, CliError> {
    std::path::absolute(path).map_err(error)
}
fn error(value: impl std::fmt::Display) -> CliError {
    CliError::runtime(value.to_string())
}

#[cfg(test)]
#[path = "native_offline_tests.rs"]
mod tests;
