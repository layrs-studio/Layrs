//! Bounded CFAPI prototype. This crate does not register anything at startup.
//! Sources must provide immutable, verified bytes; native Worktree capture and
//! CAS admission belong to the caller. See the provider plan for qualification.

mod catalog;
mod control;
mod control_directory;
mod evidence;
mod range;
mod registration;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

pub use catalog::{Cancellation, Catalog, CatalogEntry, SnapshotSource};
pub use control::{ControlFileIdentity, ControlPublishFaults, NoControlPublishFaults};
pub use control_directory::{
    CONTROL_DIRECTORY_MAX_ENCODED_BYTES, CONTROL_DIRECTORY_MAX_FORMAT_BYTES,
    CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES, CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES,
    ControlDirectoryFailure, ControlDirectoryIdentity, ControlDirectoryPublishFaults,
    ControlDirectoryPublishPoint, ControlDirectoryStageIdentity, NoControlDirectoryPublishFaults,
    ObservedControlDirectory, ObservedControlDirectoryIdentity,
};
pub use evidence::{CaptureEvidence, Inspection, PlaceholderEvidence, classify_placeholder};
pub use range::{HydrationRange, READ_BUFFER_SIZE, checked_range};
pub use registration::Registration;
#[cfg(windows)]
pub use windows::{
    CallbackFailure, Checkpoint, ControlDirectoryGuard, ControlRootPreflight, ExportCheckpoint,
    ExportParentGuard, HydrationRequest, ObservedAllocatedControlStage,
    ObservedAllocatedControlStageIdentity, ObservedRootNamespace, ProviderRootGuard, Session,
    capability, discover_control_directory, observe_allocated_control_stage,
    observe_control_directory_stage, publish_control_directory_recoverable, read_control_file,
};

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Unsupported(&'static str),
    InvalidCatalog(String),
    InvalidRoot(String),
    ForeignRoot,
    NeedsRepair(&'static str),
    IdentityMismatch,
    InvalidRange,
    MissingBytes {
        file_id: layrs_core::scm::FileId,
        offset: u64,
        length: usize,
    },
    Cancelled,
    Busy,
    ControlDirectory {
        kind: ControlDirectoryFailure,
        point: ControlDirectoryPublishPoint,
        operation: &'static str,
        root: std::path::PathBuf,
        stage: std::path::PathBuf,
        cause: Box<Error>,
    },
    Windows {
        operation: &'static str,
        hresult: i32,
    },
    PlaceholderCompatibility {
        operation: &'static str,
        result: i8,
    },
    ExportRead {
        path: layrs_core::scm::RepoPath,
        backing: Option<layrs_core::scm::FileId>,
        cause: Box<Error>,
    },
    ExportConsumer {
        path: layrs_core::scm::RepoPath,
        cause: Box<Error>,
    },
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ControlDirectory {
                kind,
                point,
                operation,
                root,
                stage,
                cause,
            } => {
                write!(
                    f,
                    "control directory {kind:?} at {point:?}: {operation}; root={}, stage={}: {cause}",
                    root.display(),
                    stage.display()
                )
            }
            Self::Windows { operation, hresult } => {
                write!(f, "{operation}: HRESULT 0x{:08X}", *hresult as u32)
            }
            Self::PlaceholderCompatibility { operation, result } => {
                write!(f, "{operation}: native compatibility result {result}")
            }
            Self::ExportRead {
                path,
                backing,
                cause,
            } => {
                write!(f, "export {} (backing {backing:?}): {cause}", path.as_str())
            }
            Self::ExportConsumer { path, cause } => {
                write!(f, "export consumer {}: {cause}", path.as_str())
            }
            _ => write!(f, "{self:?}"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ExportRead { cause, .. }
            | Self::ExportConsumer { cause, .. }
            | Self::ControlDirectory { cause, .. } => Some(cause.as_ref()),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Loading and platform-query evidence, not proof that registration will work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub backend: &'static str,
    pub build: u32,
    pub revision: u32,
    pub integration: u32,
}

#[cfg(not(windows))]
pub fn capability() -> Result<Capability> {
    Err(Error::Unsupported(
        "CFAPI requires Windows on an NTFS volume",
    ))
}
