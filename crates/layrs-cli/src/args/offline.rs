//! Human-facing exact-version selectors; no caller-supplied digest reconstruction.
use super::{CliParseError, take_bool_flag, take_required_positional, take_string_option};
use layrs_core::{
    projection::OfflineOperation,
    scm::{ObjectId, RepoPath, StepId},
};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSelector {
    Current,
    Generation(u64),
    Capture(u64),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineRequest {
    pub version: VersionSelector,
    pub other: Option<VersionSelector>,
    pub operation: OfflineOperation,
    pub paths: Vec<RepoPath>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineCommand {
    Status,
    Check(OfflineRequest),
    Read {
        version: VersionSelector,
        path: RepoPath,
        output: PathBuf,
        max_bytes: u32,
    },
    Compare(OfflineRequest),
    RestorePreview {
        version: VersionSelector,
        paths: Vec<RepoPath>,
        expected_generation: u64,
        output: PathBuf,
    },
    RestoreApply {
        preview: PathBuf,
        expected_digest: ObjectId,
    },
    Preparations,
    Release {
        id: String,
    },
    CancelReservation,
    ReleaseGeneration {
        generation: u64,
    },
    Quota {
        bytes: Option<u64>,
    },
    Evict,
    CreateFamily {
        path: PathBuf,
        quota_bytes: u64,
    },
    Convert {
        family: PathBuf,
        destination: PathBuf,
    },
    ResumeConversion {
        destination: PathBuf,
    },
    AbandonConversion {
        family: PathBuf,
        destination: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteOfflineCommand {
    Observe {
        expected_generation: u64,
        step: StepId,
        paths: Vec<RepoPath>,
    },
    Prepare {
        expected_generation: u64,
        request: OfflineRequest,
    },
    Resume {
        expected_generation: u64,
        id: String,
    },
}

pub(super) fn parse(mut args: Vec<String>) -> Result<OfflineCommand, CliParseError> {
    let action = first(&mut args)?;
    match action.as_str() {
        "status" => no_extra(args, OfflineCommand::Status),
        "check" => Ok(OfflineCommand::Check(request(args)?)),
        "read" => {
            let version =
                version_option(&mut args, "--version")?.unwrap_or(VersionSelector::Current);
            let output = required_option(&mut args, "--output")?;
            if output == "-" {
                return Err(CliParseError::new(
                    "offline read requires a new output file; stdout is reserved for the result",
                ));
            }
            let max_bytes = take_string_option(&mut args, "--max-bytes")?
                .map(|v| number(&v, "--max-bytes"))
                .transpose()?
                .unwrap_or(64 * 1024);
            if !(1..=1024 * 1024).contains(&max_bytes) {
                return Err(CliParseError::new(
                    "--max-bytes must be between 1 and 1048576",
                ));
            }
            let path = take_required_positional(args, "PATH", "partial offline read")?;
            let path = paths(vec![path])?.remove(0);
            Ok(OfflineCommand::Read {
                version,
                path,
                output: output.into(),
                max_bytes: max_bytes as u32,
            })
        }
        "compare" => {
            let content = take_bool_flag(&mut args, "--content")?;
            let version = selector(&first(&mut args)?)?;
            let other = selector(&first(&mut args)?)?;
            Ok(OfflineCommand::Compare(OfflineRequest {
                version,
                other: Some(other),
                paths: paths(args)?,
                operation: if content {
                    OfflineOperation::CompareContent
                } else {
                    OfflineOperation::CompareMetadata
                },
            }))
        }
        "restore-preview" => {
            let version =
                version_option(&mut args, "--version")?.unwrap_or(VersionSelector::Current);
            let expected_generation = number(
                &required_option(&mut args, "--expected-generation")?,
                "--expected-generation",
            )?;
            let output = required_option(&mut args, "--output")?;
            if output == "-" {
                return Err(CliParseError::new(
                    "restore preview requires a new artifact file",
                ));
            }
            Ok(OfflineCommand::RestorePreview {
                version,
                paths: paths(args)?,
                expected_generation,
                output: output.into(),
            })
        }
        "restore-apply" => {
            let expected_digest = ObjectId::parse(required_option(&mut args, "--expected-digest")?)
                .map_err(|e| CliParseError::new(e.to_string()))?;
            let preview =
                take_required_positional(args, "PREVIEW.json", "partial offline restore-apply")?;
            if preview.is_empty() || preview == "-" {
                return Err(CliParseError::new(
                    "restore apply requires a reviewed artifact file",
                ));
            }
            Ok(OfflineCommand::RestoreApply {
                preview: preview.into(),
                expected_digest,
            })
        }
        "preparations" => no_extra(args, OfflineCommand::Preparations),
        "release" => Ok(OfflineCommand::Release {
            id: preparation_id(args)?,
        }),
        "cancel-reservation" => no_extra(args, OfflineCommand::CancelReservation),
        "release-generation" => {
            let value =
                take_required_positional(args, "GENERATION", "partial offline release-generation")?;
            Ok(OfflineCommand::ReleaseGeneration {
                generation: number(&value, "generation")?,
            })
        }
        "quota" => {
            let bytes = take_string_option(&mut args, "--bytes")?
                .map(|v| number(&v, "--bytes"))
                .transpose()?;
            no_extra(args, OfflineCommand::Quota { bytes })
        }
        "evict" => no_extra(args, OfflineCommand::Evict),
        "family" => family(args),
        _ => Err(usage()),
    }
}

pub(crate) fn parse_remote(
    mut args: Vec<String>,
    generation: Option<String>,
) -> Result<RemoteOfflineCommand, CliParseError> {
    let expected_generation = number(
        &generation.ok_or_else(|| {
            CliParseError::new(
                "online offline-preparation commands require --expected-generation N",
            )
        })?,
        "--expected-generation",
    )?;
    match first(&mut args)?.as_str() {
        "partial-observe" => {
            let step =
                StepId::parse(first(&mut args)?).map_err(|e| CliParseError::new(e.to_string()))?;
            Ok(RemoteOfflineCommand::Observe {
                expected_generation,
                step,
                paths: paths(args)?,
            })
        }
        "partial-prepare" => Ok(RemoteOfflineCommand::Prepare {
            expected_generation,
            request: request(args)?,
        }),
        "partial-resume-preparation" => Ok(RemoteOfflineCommand::Resume {
            expected_generation,
            id: preparation_id(args)?,
        }),
        _ => Err(usage()),
    }
}

fn family(mut args: Vec<String>) -> Result<OfflineCommand, CliParseError> {
    match first(&mut args)?.as_str() {
        "create" => {
            let quota_bytes = number(
                &required_option(&mut args, "--quota-bytes")?,
                "--quota-bytes",
            )?;
            let path =
                take_required_positional(args, "EMPTY_DIR", "partial offline family create")?
                    .into();
            Ok(OfflineCommand::CreateFamily { path, quota_bytes })
        }
        "convert" => {
            let family = required_option(&mut args, "--family")?.into();
            let destination = required_option(&mut args, "--destination")?.into();
            no_extra(
                args,
                OfflineCommand::Convert {
                    family,
                    destination,
                },
            )
        }
        "resume" => {
            let destination = required_option(&mut args, "--destination")?.into();
            no_extra(args, OfflineCommand::ResumeConversion { destination })
        }
        "abandon" => {
            let family = required_option(&mut args, "--family")?.into();
            let destination = required_option(&mut args, "--destination")?.into();
            no_extra(
                args,
                OfflineCommand::AbandonConversion {
                    family,
                    destination,
                },
            )
        }
        _ => Err(usage()),
    }
}

fn request(mut args: Vec<String>) -> Result<OfflineRequest, CliParseError> {
    let version = version_option(&mut args, "--version")?.unwrap_or(VersionSelector::Current);
    let other = version_option(&mut args, "--other")?;
    let operation = match first(&mut args)?.as_str() {
        "read" => OfflineOperation::Read,
        "edit" => OfflineOperation::Edit,
        "capture" => OfflineOperation::Capture,
        "restore" => OfflineOperation::Restore,
        "compare-metadata" => OfflineOperation::CompareMetadata,
        "compare-content" => OfflineOperation::CompareContent,
        _ => {
            return Err(CliParseError::new(
                "operation must be read, edit, capture, restore, compare-metadata or compare-content",
            ));
        }
    };
    let comparison = matches!(
        operation,
        OfflineOperation::CompareMetadata | OfflineOperation::CompareContent
    );
    if comparison != other.is_some() {
        return Err(CliParseError::new(
            "--other is required only for comparison operations",
        ));
    }
    Ok(OfflineRequest {
        version,
        other,
        operation,
        paths: paths(args)?,
    })
}

fn version_option(
    args: &mut Vec<String>,
    flag: &str,
) -> Result<Option<VersionSelector>, CliParseError> {
    take_string_option(args, flag)?
        .map(|v| selector(&v))
        .transpose()
}

fn selector(value: &str) -> Result<VersionSelector, CliParseError> {
    if value == "current" {
        return Ok(VersionSelector::Current);
    }
    if let Some(number_text) = value.strip_prefix("generation:") {
        return Ok(VersionSelector::Generation(number(
            number_text,
            "generation selector",
        )?));
    }
    if let Some(number_text) = value.strip_prefix("capture:") {
        let sequence = number(number_text, "capture selector")?;
        if sequence > 0 {
            return Ok(VersionSelector::Capture(sequence));
        }
    }
    if let Some(path) = value
        .strip_prefix("version-file:")
        .filter(|p| !p.is_empty() && *p != "-")
    {
        return Ok(VersionSelector::File(path.into()));
    }
    Err(CliParseError::new(
        "version must be current, generation:N, capture:N, or version-file:FILE",
    ))
}

fn paths(values: Vec<String>) -> Result<Vec<RepoPath>, CliParseError> {
    if values.is_empty() || values.len() > 1024 {
        return Err(CliParseError::new(
            "provide 1..1024 repository-relative paths",
        ));
    }
    let mut paths = values
        .into_iter()
        .map(|v| {
            if v.starts_with('-') {
                return Err(CliParseError::new(format!("unexpected flag `{v}`")));
            }
            RepoPath::parse(v).map_err(|e| CliParseError::new(e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    if paths.windows(2).any(|p| p[0] == p[1]) {
        return Err(CliParseError::new("duplicate repository path"));
    }
    Ok(paths)
}

fn preparation_id(args: Vec<String>) -> Result<String, CliParseError> {
    let value = take_required_positional(args, "PREPARATION_ID", "offline preparation")?;
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(CliParseError::new("invalid preparation ID"));
    }
    Ok(value)
}

fn number(value: &str, label: &str) -> Result<u64, CliParseError> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CliParseError::new(format!(
            "{label} requires a nonnegative integer"
        )));
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| CliParseError::new(format!("{label} exceeds the supported integer range")))
}
fn required_option(args: &mut Vec<String>, flag: &str) -> Result<String, CliParseError> {
    take_string_option(args, flag)?
        .filter(|v| !v.is_empty())
        .ok_or_else(|| CliParseError::new(format!("offline command requires {flag}")))
}
fn first(args: &mut Vec<String>) -> Result<String, CliParseError> {
    if args.is_empty() {
        Err(usage())
    } else {
        Ok(args.remove(0))
    }
}
fn no_extra<T>(args: Vec<String>, result: T) -> Result<T, CliParseError> {
    if args.is_empty() {
        Ok(result)
    } else {
        Err(CliParseError::new(format!(
            "unexpected argument `{}`",
            args[0]
        )))
    }
}
fn usage() -> CliParseError {
    CliParseError::new(
        "partial offline status | check OP PATH... | read PATH --output NEW_FILE | compare BEFORE AFTER PATH... | restore-preview PATH... --expected-generation N --output NEW_PREVIEW.json | restore-apply PREVIEW.json --expected-digest blake3:HEX | preparations | release ID | cancel-reservation | release-generation N | quota [--bytes N] | evict | family create/convert/resume/abandon",
    )
}

#[cfg(test)]
#[path = "offline_tests.rs"]
mod tests;
