//! Local-only reads bound to the exact durable activation map.
use super::*;
use crate::projection::shared::provider_content;
use layrs_core::scm::{FileId, FileManifestId};
use layrs_vfs::Cancellation;
use std::{error::Error, fmt};

#[derive(Debug)]
pub enum ProviderActiveRangeFailure {
    NeedsHydration {
        file_id: FileId,
        offset: u64,
        length: usize,
    },
    Cancelled,
    InvalidRange,
    Unavailable(ProviderActiveOpenFailure),
}
impl fmt::Display for ProviderActiveRangeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsHydration {
                file_id,
                offset,
                length,
            } => write!(
                f,
                "resident content missing for {file_id} at {offset}+{length}"
            ),
            Self::Cancelled => f.write_str("active range read cancelled"),
            Self::InvalidRange => f.write_str("range outside the fixed active source"),
            Self::Unavailable(error) => error.fmt(f),
        }
    }
}
impl Error for ProviderActiveRangeFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(error) => Some(error),
            _ => None,
        }
    }
}

macro_rules! resident_reader {
    ($holder:ty) => {
        impl $holder {
            pub fn read_resident_range(
                &self,
                file_id: &FileId,
                offset: u64,
                output: &mut [u8],
                cancel: &Cancellation,
            ) -> std::result::Result<(), ProviderActiveRangeFailure> {
                read(
                    &self.held,
                    &self.root,
                    &self.record,
                    &self.activation,
                    file_id,
                    offset,
                    output,
                    cancel,
                )
            }
        }
    };
}
resident_reader!(ProviderActive);
resident_reader!(ProviderActiveReopened);

fn unavailable(error: preparation_report::Failure) -> ProviderActiveRangeFailure {
    if matches!(&error.cause, Cause::Vfs(layrs_vfs::Error::Cancelled)) {
        return ProviderActiveRangeFailure::Cancelled;
    }
    ProviderActiveRangeFailure::Unavailable(ProviderActiveOpenFailure::from_failure(
        "read active resident range",
        error,
    ))
}

/// Caller holds all three operation gates throughout this check and copy.
fn validate(
    held: &outcome::PublicationPins,
    native: &layrs_vfs::ProviderRootGuard,
    expected: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    cancel: &Cancellation,
) -> std::result::Result<validation::PrivateActiveCaptureValidation, ProviderActiveRangeFailure> {
    cancel
        .check()
        .map_err(|_| ProviderActiveRangeFailure::Cancelled)?;
    let inspected = capture_state::read(held, Some(native), Some(cancel)).map_err(unavailable)?;
    if inspected.active.mount != *expected || inspected.active.activation != *activation {
        return Err(unavailable(preparation_report::Failure::private(invalid(
            "active range activation map changed",
        ))));
    }
    Ok(inspected)
}

pub(super) fn read(
    held: &outcome::PublicationPins,
    native: &layrs_vfs::ProviderRootGuard,
    expected: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    file_id: &FileId,
    offset: u64,
    output: &mut [u8],
    cancel: &Cancellation,
) -> std::result::Result<(), ProviderActiveRangeFailure> {
    cancel
        .check()
        .map_err(|_| ProviderActiveRangeFailure::Cancelled)?;
    if output.len() > 256 * 1024 {
        return Err(ProviderActiveRangeFailure::InvalidRange);
    }
    let mut size = None;
    for entry in native.catalog().entries() {
        cancel
            .check()
            .map_err(|_| ProviderActiveRangeFailure::Cancelled)?;
        if &entry.file_id != file_id {
            continue;
        }
        if size.is_some_and(|old| old != entry.size) {
            return Err(unavailable(preparation_report::Failure::private(invalid(
                "active FileId has inconsistent sizes",
            ))));
        }
        size = Some(entry.size);
    }
    let size = size.ok_or(ProviderActiveRangeFailure::InvalidRange)?;
    if offset
        .checked_add(output.len() as u64)
        .is_none_or(|end| end > size)
    {
        return Err(ProviderActiveRangeFailure::InvalidRange);
    }
    let locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .map_err(|e| {
            if cancel.is_cancelled() {
                ProviderActiveRangeFailure::Cancelled
            } else {
                unavailable(e.into())
            }
        })?;
    let inspected = validate(held, native, expected, activation, cancel)?;
    let mut manifest: Option<FileManifestId> = None;
    let mut ambiguous = false;
    let mut found = false;
    for row in inspected.active.source.descriptors() {
        cancel
            .check()
            .map_err(|_| ProviderActiveRangeFailure::Cancelled)?;
        if &row.file_id != file_id {
            continue;
        }
        if row.bytes != size {
            return Err(unavailable(preparation_report::Failure::private(invalid(
                "active descriptor size changed",
            ))));
        }
        found = true;
        if manifest.as_ref().is_some_and(|id| id != &row.manifest_id) {
            ambiguous = true;
        } else if manifest.is_none() {
            manifest = Some(row.manifest_id.clone());
        }
    }
    if !found {
        return Err(unavailable(preparation_report::Failure::private(invalid(
            "active descriptor absent",
        ))));
    }
    if ambiguous {
        manifest = None;
    }
    let family = database_io::read_cancellable(&held.family.catalog, cancel, |c| {
        if held.family.validate_capture_catalog(
            c,
            &held.artifacts.allocation,
            &inspected.capture,
            Some(cancel),
        )? != ActivationRegistration::Ready
        {
            return Err(invalid("active range family changed"));
        }
        if output.is_empty() {
            return Ok(Some(Vec::new()));
        }
        provider_content::resident_range(
            &held.family,
            &locks,
            c,
            file_id,
            manifest.as_ref(),
            size,
            offset,
            output.len(),
            cancel,
        )
    })
    .map_err(|e| {
        unavailable(preparation_report::Failure::new(
            e,
            Some(ProviderDatabaseScope::FamilyCatalog),
        ))
    })?;
    let bytes = read_state(family, ProviderDatabaseScope::FamilyCatalog).map_err(unavailable)?;
    validate(held, native, expected, activation, cancel)?;
    let bytes = bytes.ok_or_else(|| ProviderActiveRangeFailure::NeedsHydration {
        file_id: file_id.clone(),
        offset,
        length: output.len(),
    })?;
    if bytes.len() != output.len() {
        return Err(unavailable(preparation_report::Failure::private(invalid(
            "active resident range short read",
        ))));
    }
    cancel
        .check()
        .map_err(|_| ProviderActiveRangeFailure::Cancelled)?;
    output.copy_from_slice(&bytes);
    Ok(())
}
