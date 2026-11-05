//! Permanent admitted-import SQL DATA and caller-transaction writers. No opener,
//! physical authority or reclamation policy is activated here.
use super::{
    cancellation,
    records::{
        Record,
        import::{ImportSource, ProviderImportPrefixRecord, ProviderTransitionReceiptRecord},
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    recovery::error::{Result, invalid},
};
use crate::projection::{format, shared::exact_content::ExactBinding};
use layrs_core::scm::{FileId, FileManifestId};
use layrs_vfs::Cancellation;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

mod ordinary_write;
mod read;
pub(super) mod record;
pub(super) mod representations;
pub(super) mod schema;
#[cfg(test)]
pub(in crate::projection::provider) mod tests;
mod write;
pub(super) use ordinary_write::admit_ordinary_ready;
pub(in crate::projection) use read::VerifiedAdmittedImport;
pub(super) use read::read_admitted_import;
pub(super) use read::read_record;
pub(super) use record::{ArchiveCommitment, ManagedAdmissionRecord};
pub(super) use write::{AdmissionWriteEffect, admit_denial_ready, admit_ready};

/// Explicit protocol selection; the legacy writer never opts into C14 merely
/// because a newly decoded record happens to carry a denial reference.
#[derive(Clone, Copy)]
pub(super) enum AdmissionCoverage {
    Legacy,
    Denial,
}
impl AdmissionCoverage {
    pub(super) fn require(
        self,
        intent: &ProviderTransitionIntentRecord,
        receipt: &ProviderTransitionReceiptRecord,
    ) -> Result<()> {
        match self {
            Self::Legacy => require_supported_coverage(intent, receipt),
            Self::Denial => {
                intent.validate_fields()?;
                receipt.validate_fields()?;
                if intent.plan.denial_coverage.is_none()
                    || receipt.denial_coverage != intent.plan.denial_coverage
                    || !matches!(
                        intent.plan.source_prefix.source,
                        ImportSource::Provider {
                            private_schema: 8,
                            ..
                        }
                    )
                {
                    return Err(invalid(
                        "denial admission requires matching Provider8 coverage",
                    ));
                }
                Ok(intent.require_receipt(receipt)?)
            }
        }
    }
}

/// Same maximum copy-peak/remaining checks as the writer, callable from the
/// complete Ready read scope before opening any destination writer.
pub(super) fn require_ready_admission_capacity(
    snapshot: &super::destination_ready::OwnedReadySnapshot,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    cancellation::check(cancel)?;
    let budget = super::managed_budget::admission_budget(
        snapshot.metadata_floor,
        snapshot.expected.files().len() as u64,
        snapshot.expected.directories().len() as u64,
    )?;
    let baseline = super::managed_budget::quota(
        &snapshot.required,
        snapshot.expected.files().iter().map(|file| file.bytes),
        0,
    )?;
    budget.require_quota(snapshot.ready.plan.quota_bytes, baseline)?;
    let binding = &snapshot.binding;
    let protection = crate::projection::shared::exact_protection::ExactProtection::new(
        &binding.family_incarnation,
        &binding.owner,
        &binding.database_identity,
        &binding.transaction_id,
        &snapshot.required,
        cancel,
    )?;
    let remaining = snapshot
        .installation
        .remaining
        .checked_sub(snapshot.expected.header().file_bytes)
        .ok_or_else(|| invalid("managed admission retained checkout envelope insufficient"))?;
    budget.require_remaining(
        remaining,
        protection.payload().len() as u64,
        snapshot.required.len() as u64,
    )?;
    Ok(())
}

/// Legacy admission remains closed to denial records. C14 requires its explicit
/// entry, actual local body interpreter and complete admission budget instead.
pub(super) fn require_supported_coverage(
    intent: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) -> Result<()> {
    if intent.plan.denial_coverage.is_some() || receipt.denial_coverage.is_some() {
        return Err(invalid("admitted denial coverage writer is not integrated"));
    }
    Ok(())
}

/// A committed exact representation reference, not a canonical manifest body
/// or proof of resident CAS bytes. Family verification remains separate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RequiredRepresentation {
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
}
