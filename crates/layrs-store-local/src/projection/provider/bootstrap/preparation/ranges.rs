//! Retained read path for resident immutable family content. No local miss
//! causes acquisition, Store recovery, native activation or buffer mutation.
use super::*;
use crate::projection::shared::provider_content;
use layrs_core::scm::{FileId, FileManifestId};
use layrs_vfs::Cancellation;
use std::{error::Error, fmt};

#[derive(Debug)]
pub enum ProviderRangeFailure {
    NeedsHydration {
        file_id: FileId,
        offset: u64,
        length: usize,
    },
    Cancelled,
    InvalidRange,
    Unavailable(ProviderPreparedOpenFailure),
}
impl fmt::Display for ProviderRangeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsHydration {
                file_id,
                offset,
                length,
            } => {
                write!(
                    f,
                    "resident content missing for {file_id} at {offset}+{length}"
                )
            }
            Self::Cancelled => f.write_str("resident range read cancelled"),
            Self::InvalidRange => f.write_str("resident range outside the fixed source"),
            Self::Unavailable(cause) => cause.fmt(f),
        }
    }
}
impl Error for ProviderRangeFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(error) => Some(error),
            _ => None,
        }
    }
}

impl ProviderPreparedReopened {
    /// Reads verified bytes from this reopened root's fixed source.
    pub fn read_resident_range(
        &self,
        file_id: &FileId,
        offset: u64,
        buffer: &mut [u8],
        cancel: &Cancellation,
    ) -> std::result::Result<(), ProviderRangeFailure> {
        read(
            &self.held,
            &self.record,
            &self.root,
            file_id,
            offset,
            buffer,
            cancel,
        )
    }
}

fn unavailable(error: report::Failure) -> ProviderRangeFailure {
    if matches!(&error.cause, Cause::Vfs(layrs_vfs::Error::Cancelled)) {
        return ProviderRangeFailure::Cancelled;
    }
    ProviderRangeFailure::Unavailable(ProviderPreparedOpenFailure::from_failure(
        "read prepared resident range",
        error,
    ))
}

/// The caller keeps this holder alive until the native callback has drained.
/// The shared lifecycle/owner/catalog gates span every validation, SQL close,
/// immutable read and the final copy. SQLite is opened only by the restricted
/// read-only observer; its hot journal is never recovered here.
pub(super) fn read(
    held: &outcome::PublicationPins,
    expected: &ProviderPreparationRecord,
    native: &layrs_vfs::ProviderRootGuard,
    file_id: &FileId,
    offset: u64,
    output: &mut [u8],
    cancel: &Cancellation,
) -> std::result::Result<(), ProviderRangeFailure> {
    cancel
        .check()
        .map_err(|_| ProviderRangeFailure::Cancelled)?;
    if output.len() > 256 * 1024 {
        return Err(ProviderRangeFailure::InvalidRange);
    }
    let mut size = None;
    for entry in native
        .catalog()
        .entries()
        .iter()
        .filter(|entry| &entry.file_id == file_id)
    {
        if size.is_some_and(|old| old != entry.size) {
            return Err(unavailable(report::Failure::private(invalid(
                "provider FileId has inconsistent catalog sizes",
            ))));
        }
        size = Some(entry.size);
    }
    let size = size.ok_or_else(|| {
        unavailable(report::Failure::private(invalid(
            "provider FileId is outside the fixed catalog",
        )))
    })?;
    let end = offset
        .checked_add(output.len() as u64)
        .ok_or(ProviderRangeFailure::InvalidRange)?;
    if end > size {
        return Err(ProviderRangeFailure::InvalidRange);
    }
    let locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .map_err(|cause| {
            if cancel.is_cancelled() {
                ProviderRangeFailure::Cancelled
            } else {
                unavailable(report::Failure::from(cause))
            }
        })?;
    cancel
        .check()
        .map_err(|_| ProviderRangeFailure::Cancelled)?;
    sql::final_check_cancellable(
        held,
        expected,
        native,
        native.registration().root_id(),
        cancel,
    )
    .map_err(unavailable)?;
    cancel
        .check()
        .map_err(|_| ProviderRangeFailure::Cancelled)?;

    // The private source is read again under the same gates. If multiple
    // selected paths share a FileId but have different source representations,
    // require a complete local FileId hash instead of a manifest fast path.
    let private = database_io::read_cancellable(&held.artifacts.database, cancel, |c| {
        let p = held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("provider P absent"))?;
        let inspected = validation::private_preparation_with_source_cancellable(
            c,
            &held.artifacts.allocation,
            p,
            &held.binding,
            native.registration().root_id(),
            cancel,
        )?;
        if inspected.mount != *expected || inspected.source.commitment() != &expected.source {
            return Err(invalid("provider range source changed"));
        }
        let mut found = false;
        let mut manifest_id: Option<FileManifestId> = None;
        let mut ambiguous = false;
        for descriptor in inspected
            .source
            .descriptors()
            .iter()
            .filter(|row| &row.file_id == file_id)
        {
            if descriptor.bytes != size {
                return Err(invalid("provider range descriptor size differs"));
            }
            found = true;
            if manifest_id
                .as_ref()
                .is_some_and(|id| id != &descriptor.manifest_id)
            {
                ambiguous = true;
            } else if manifest_id.is_none() {
                manifest_id = Some(descriptor.manifest_id.clone());
            }
        }
        if !found {
            return Err(invalid("provider range lacks a fixed source descriptor"));
        }
        Ok(if ambiguous { None } else { manifest_id })
    })
    .map_err(|cause| unavailable(report::Failure::private(cause)))?;
    let trusted_manifest = sql::read_state(private).map_err(unavailable)?;
    cancel
        .check()
        .map_err(|_| ProviderRangeFailure::Cancelled)?;

    let family = database_io::read_cancellable(&held.family.catalog, cancel, |c| {
        if held
            .family
            .validate_catalog_cancellable(c, &held.artifacts.allocation, cancel)?
            != Registration::Initializing
        {
            return Err(invalid("provider family is not an initializing owner"));
        }
        // No bytes are needed at exact EOF, even when this known file is cold.
        if output.is_empty() {
            return Ok(Some(Vec::new()));
        }
        provider_content::resident_range(
            &held.family,
            &locks,
            c,
            file_id,
            trusted_manifest.as_ref(),
            size,
            offset,
            output.len(),
            cancel,
        )
    })
    .map_err(|cause| {
        unavailable(report::Failure::new(
            cause,
            Some(ProviderDatabaseScope::FamilyCatalog),
        ))
    })?;
    let scratch = match family {
        database_io::ReadState::Valid(Some(bytes)) => bytes,
        database_io::ReadState::Valid(None) => {
            sql::final_check_cancellable(
                held,
                expected,
                native,
                native.registration().root_id(),
                cancel,
            )
            .map_err(unavailable)?;
            cancel
                .check()
                .map_err(|_| ProviderRangeFailure::Cancelled)?;
            return Err(ProviderRangeFailure::NeedsHydration {
                file_id: file_id.clone(),
                offset,
                length: output.len(),
            });
        }
        database_io::ReadState::Hot(hot) => {
            return Err(unavailable(report::Failure::new(
                Cause::ReadonlyRollback(hot.trigger),
                Some(ProviderDatabaseScope::FamilyCatalog),
            )));
        }
        database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::ReadButUnavailable(cause)
        | database_io::ReadState::Unavailable(cause) => {
            return Err(unavailable(report::Failure::new(
                cause,
                Some(ProviderDatabaseScope::FamilyCatalog),
            )));
        }
        database_io::ReadState::Uninitialized => {
            return Err(unavailable(report::Failure::new(
                invalid("provider family catalog uninitialized"),
                Some(ProviderDatabaseScope::FamilyCatalog),
            )));
        }
        database_io::ReadState::UnboundRollback { trigger, cause } => {
            let mut failure =
                report::Failure::new(cause, Some(ProviderDatabaseScope::FamilyCatalog));
            failure.rollback = Some(trigger);
            return Err(unavailable(failure));
        }
    };
    sql::final_check_cancellable(
        held,
        expected,
        native,
        native.registration().root_id(),
        cancel,
    )
    .map_err(unavailable)?;
    cancel
        .check()
        .map_err(|_| ProviderRangeFailure::Cancelled)?;
    if scratch.len() != output.len() {
        return Err(unavailable(report::Failure::private(invalid(
            "provider resident range returned a short buffer",
        ))));
    }
    output.copy_from_slice(&scratch);
    Ok(())
}
