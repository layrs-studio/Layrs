//! Private9 inspection DATA for ordinary integration. No family connection,
//! opener, byte proof or ordinary/reclamation admission is granted here.
use super::{
    cancellation,
    managed_admission::{read_admitted_import, ManagedAdmissionRecord, VerifiedAdmittedImport},
    records::{
        import::{ProviderImportPrefixRecord, ProviderTransitionReceiptRecord},
        transition::ProviderTransitionIntentRecord,
        Record,
    },
    recovery::error::{invalid, ProviderBootstrapCause as Cause, Result},
};
use crate::projection::{
    capture::CapturedPatch, format, publication::CompletedPublication,
    shared::exact_content::ExactBinding, ProjectionIdentity,
};
use layrs_core::scm::{canonical_json, FileId, FileManifestId, RepoPath};
use layrs_vfs::Cancellation;
use rusqlite::Connection;

mod data;
mod origins;
mod snapshot;
#[cfg(test)]
mod tests;
pub(in crate::projection) use data::{
    AdmittedArchiveData, AdmittedCheckoutData, AdmittedCheckoutFile, AdmittedOrdinaryData,
    AdmittedRepresentation,
};
pub(in crate::projection) use origins::{
    capture_origin, imported, imported_publication, restore_history_origin,
    validate_capture_identity, CaptureOrigin,
};

/// Interpret true private9 in a retained caller transaction or a short owned
/// read transaction. The owned return has no SQL lifetime/capability and must
/// be compared with a fresh inspect before a future family/physical effect.
pub(in crate::projection) fn inspect(
    c: &Connection,
    marker: &format::Marker,
    cancel: Option<&Cancellation>,
) -> Result<AdmittedOrdinaryData> {
    snapshot::with_snapshot(c, cancel, || {
        let proof = read_admitted_import(c, marker, cancel)?;
        data::detach(marker, &proof, cancel)
    })
}

/// Bounded dispatch hint only, never authority: whether the stored admission
/// record's SourcePublished declares C14 denial coverage. A caller acting on
/// `true` must still run the complete `inspect`; `false` keeps the legacy
/// live-transition guards, which themselves refuse an undeclared C14 image.
pub(in crate::projection) fn declares_denial(c: &Connection) -> Result<bool> {
    let present: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='projection_managed_admission')",
        [],
        |r| r.get(0),
    )?;
    if !present {
        return Ok(false);
    }
    let (_, published, _) = super::managed_admission::read_record(c, None)?;
    Ok(published.plan.denial_coverage.is_some())
}
