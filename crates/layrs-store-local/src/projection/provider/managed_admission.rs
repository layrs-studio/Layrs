//! Permanent admitted-import SQL DATA and caller-transaction writers. No opener,
//! physical authority or reclamation policy is activated here.
use super::{
    cancellation,
    records::{
        import::{ImportSource, ProviderImportPrefixRecord, ProviderTransitionReceiptRecord},
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        Record,
    },
    recovery::error::{invalid, Result},
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
pub(super) use read::read_admitted_import;
pub(super) use read::read_record;
pub(in crate::projection) use read::VerifiedAdmittedImport;
pub(super) use record::{ArchiveCommitment, ManagedAdmissionRecord};
pub(super) use write::{admit_ready, AdmissionWriteEffect};

/// A committed exact representation reference, not a canonical manifest body
/// or proof of resident CAS bytes. Family verification remains separate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RequiredRepresentation {
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
}
