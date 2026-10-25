//! Provider-only causal reporting. Ordinary projection errors keep their existing behavior.
use super::super::{journal, process_profile::ProviderSqliteProfileError};
use crate::projection::ProjectionStoreError;
use std::{error::Error, fmt, io, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDatabaseScope {
    FamilyCatalog,
    PrivateMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRecoveryEffectState {
    /// Writable recovery was entered and may have opened SQLite; completion
    /// has not been established. This is deliberately conservative on refusal.
    Uncertain,
    /// The rollback read, observable close, journal absence and fresh nonhot
    /// read-only pager read succeeded. Metadata validation may still refuse;
    /// its cause remains in the inspection/failure and no phase is inferred.
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRecoveryEffect {
    pub database: ProviderDatabaseScope,
    pub state: ProviderRecoveryEffectState,
}

/// The original extended READONLY_ROLLBACK error, retained without rebuilding
/// it from a message. At most one initial trigger per database in an operation.
#[derive(Debug, Clone)]
pub struct ProviderRollbackTrigger {
    pub database: ProviderDatabaseScope,
    pub(super) error: Arc<rusqlite::Error>,
}
impl ProviderRollbackTrigger {
    pub fn error(&self) -> &rusqlite::Error {
        self.error.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapFailureKind {
    ProcessProfileUnavailable,
    ControlInUse,
    ControlUnavailable,
    DatabaseBusy,
    NeedsRepair,
    BindingChanged,
    UnsupportedJournal,
    ObservableCloseFailure,
}

#[derive(Debug)]
pub enum ProviderBootstrapCause {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    ReadonlyRollback(Arc<rusqlite::Error>),
    Store(crate::StoreError),
    Vfs(layrs_vfs::Error),
    Journal(journal::Refusal),
    Profile(ProviderSqliteProfileError),
    Validation(Box<ProjectionStoreError>),
    /// Opening/probing failed and its separate, explicit cleanup also failed.
    Cleanup {
        primary: Box<ProviderBootstrapCause>,
        close: Box<ProviderBootstrapCause>,
    },
}
impl ProviderBootstrapCause {
    pub(super) fn rollback_trigger(&self) -> Option<&Arc<rusqlite::Error>> {
        match self {
            Self::ReadonlyRollback(trigger) => Some(trigger),
            Self::Cleanup { primary, .. } => primary.rollback_trigger(),
            _ => None,
        }
    }
}

impl fmt::Display for ProviderBootstrapCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Sqlite(e) => e.fmt(f),
            Self::ReadonlyRollback(e) => e.fmt(f),
            Self::Store(e) => e.fmt(f),
            Self::Vfs(e) => e.fmt(f),
            Self::Journal(e) => write!(f, "restricted rollback journal refused: {e:?}"),
            Self::Profile(e) => e.fmt(f),
            Self::Validation(e) => e.fmt(f),
            Self::Cleanup { primary, close } => write!(f, "{primary}; explicit close: {close}"),
        }
    }
}
impl Error for ProviderBootstrapCause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Sqlite(e) => Some(e),
            Self::ReadonlyRollback(e) => Some(e.as_ref()),
            Self::Store(e) => Some(e),
            Self::Vfs(e) => Some(e),
            Self::Profile(e) => Some(e),
            Self::Validation(e) => Some(e.as_ref()),
            Self::Cleanup { primary, .. } => Some(primary.as_ref()),
            Self::Journal(_) => None,
        }
    }
}
impl From<io::Error> for ProviderBootstrapCause {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for ProviderBootstrapCause {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}
impl From<crate::StoreError> for ProviderBootstrapCause {
    fn from(e: crate::StoreError) -> Self {
        Self::Store(e)
    }
}
impl From<layrs_vfs::Error> for ProviderBootstrapCause {
    fn from(e: layrs_vfs::Error) -> Self {
        Self::Vfs(e)
    }
}
impl From<ProviderSqliteProfileError> for ProviderBootstrapCause {
    fn from(e: ProviderSqliteProfileError) -> Self {
        Self::Profile(e)
    }
}
impl From<ProjectionStoreError> for ProviderBootstrapCause {
    fn from(e: ProjectionStoreError) -> Self {
        match e {
            ProjectionStoreError::Io(e) => Self::Io(e),
            ProjectionStoreError::Database(e) => Self::Sqlite(e),
            e => Self::Validation(Box::new(e)),
        }
    }
}
impl From<journal::JournalError> for ProviderBootstrapCause {
    fn from(e: journal::JournalError) -> Self {
        match e {
            journal::JournalError::Io(e) => Self::Io(e),
            journal::JournalError::Refused(e) => Self::Journal(e),
        }
    }
}

pub(in crate::projection) type Result<T> = std::result::Result<T, ProviderBootstrapCause>;
pub(in crate::projection) fn invalid(message: impl Into<String>) -> ProviderBootstrapCause {
    ProviderBootstrapCause::Validation(Box::new(ProjectionStoreError::Invalid(message.into())))
}

#[derive(Debug)]
pub struct ProviderBootstrapFailure {
    pub kind: ProviderBootstrapFailureKind,
    pub operation: &'static str,
    pub database: Option<ProviderDatabaseScope>,
    pub effects: Vec<ProviderRecoveryEffect>,
    pub cause: ProviderBootstrapCause,
    /// All initial hot observations, including completed earlier database work.
    pub rollback_triggers: Vec<ProviderRollbackTrigger>,
    /// R1 reporting shortcut: the failing DB's supplied original trigger, or
    /// the last observed original when the later failure is not DB-specific.
    /// The complete scoped history is rollback_triggers. R2 shares ownership
    /// so later failures/successes cannot discard those errors.
    pub readonly_rollback: Option<Arc<rusqlite::Error>>,
    pub write_report: Option<super::super::bootstrap::ProviderBootstrapWriteReport>,
}
impl fmt::Display for ProviderBootstrapFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider bootstrap {} ({:?}): {}",
            self.operation, self.kind, self.cause
        )
    }
}
impl Error for ProviderBootstrapFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

pub(super) fn kind(cause: &ProviderBootstrapCause) -> ProviderBootstrapFailureKind {
    use ProviderBootstrapFailureKind as K;
    match cause {
        ProviderBootstrapCause::Profile(_) => return K::ProcessProfileUnavailable,
        ProviderBootstrapCause::Journal(_) => return K::UnsupportedJournal,
        ProviderBootstrapCause::Cleanup { .. } => return K::ObservableCloseFailure,
        ProviderBootstrapCause::Sqlite(rusqlite::Error::SqliteFailure(e, _))
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            return K::DatabaseBusy;
        }
        _ => {}
    }
    let mut next: Option<&(dyn Error + 'static)> = Some(cause);
    while let Some(e) = next {
        if e.downcast_ref::<io::Error>()
            .is_some_and(|e| e.raw_os_error() == Some(32))
        {
            return K::ControlInUse;
        }
        if let Some(layrs_vfs::Error::Windows { hresult, .. }) =
            e.downcast_ref::<layrs_vfs::Error>()
        {
            if *hresult as u32 == 0x80070020 {
                return K::ControlInUse;
            }
        }
        next = e.source();
    }
    if matches!(
        cause,
        ProviderBootstrapCause::Io(_)
            | ProviderBootstrapCause::Vfs(_)
            | ProviderBootstrapCause::Store(_)
    ) {
        K::ControlUnavailable
    } else {
        K::NeedsRepair
    }
}
