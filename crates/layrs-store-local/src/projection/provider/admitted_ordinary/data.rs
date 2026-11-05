use super::*;

/// An exact representation reference; no canonical manifest or resident bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct AdmittedRepresentation {
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct AdmittedCheckoutFile {
    pub path: RepoPath,
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
    pub executable: bool,
}

/// Initial checkout inventory DATA committed by the permanent archive. It is
/// neither the current writable checkout nor proof of any archived inode.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection) struct AdmittedCheckoutData {
    header_bytes: Vec<u8>,
    files: Vec<AdmittedCheckoutFile>,
    directories: Vec<RepoPath>,
    deletions: Vec<RepoPath>,
}
impl AdmittedCheckoutData {
    pub(in crate::projection) fn header_bytes(&self) -> &[u8] {
        &self.header_bytes
    }
    pub(in crate::projection) fn files(&self) -> &[AdmittedCheckoutFile] {
        &self.files
    }
    pub(in crate::projection) fn directories(&self) -> &[RepoPath] {
        &self.directories
    }
    pub(in crate::projection) fn deletions(&self) -> &[RepoPath] {
        &self.deletions
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection) struct AdmittedArchiveData {
    count: u64,
    bytes: u64,
    digest: String,
}
impl AdmittedArchiveData {
    pub(in crate::projection) fn count(&self) -> u64 {
        self.count
    }
    pub(in crate::projection) fn bytes(&self) -> u64 {
        self.bytes
    }
    pub(in crate::projection) fn digest(&self) -> &str {
        &self.digest
    }
}

/// Detached immutable commitments only. Construction is closed to actual
/// private9 inspection; retaining this DATA does not retain its transaction.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection) struct AdmittedOrdinaryData {
    marker: format::Marker,
    private_schema: u32,
    admission_record_bytes: Vec<u8>,
    admission_digest: String,
    record: ManagedAdmissionRecord,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    source_published: ProviderTransitionIntentRecord,
    required: Vec<AdmittedRepresentation>,
    checkout: AdmittedCheckoutData,
    archive: AdmittedArchiveData,
    admission_column_bytes: u64,
    witness_column_bytes: u64,
}
impl AdmittedOrdinaryData {
    pub(in crate::projection) fn private_schema(&self) -> u32 {
        self.private_schema
    }
    pub(in crate::projection) fn witness_column_bytes(&self) -> u64 {
        self.witness_column_bytes
    }
    pub(in crate::projection) fn marker(&self) -> &format::Marker {
        &self.marker
    }
    pub(in crate::projection) fn admission_record_bytes(&self) -> &[u8] {
        &self.admission_record_bytes
    }
    pub(in crate::projection) fn admission_digest(&self) -> &str {
        &self.admission_digest
    }
    pub(in crate::projection) fn binding(&self) -> &ExactBinding {
        &self.record.binding
    }
    pub(in crate::projection) fn root_id(&self) -> &str {
        &self.record.root_id
    }
    pub(in crate::projection) fn transaction_id(&self) -> &str {
        &self.record.transaction_id
    }
    pub(in crate::projection) fn prefix_id(&self) -> &str {
        &self.record.prefix_id
    }
    pub(in crate::projection) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(in crate::projection) fn source_published(&self) -> &ProviderTransitionIntentRecord {
        &self.source_published
    }
    pub(in crate::projection) fn required(&self) -> &[AdmittedRepresentation] {
        &self.required
    }
    pub(in crate::projection) fn checkout(&self) -> &AdmittedCheckoutData {
        &self.checkout
    }
    pub(in crate::projection) fn archive(&self) -> &AdmittedArchiveData {
        &self.archive
    }
    pub(in crate::projection) fn admission_column_bytes(&self) -> u64 {
        self.admission_column_bytes
    }
    pub(in crate::projection) fn imported_generation(&self) -> u64 {
        self.record.boundaries.imported_generation
    }
    pub(in crate::projection) fn imported_capture_count(&self) -> u64 {
        self.record.boundaries.imported_capture_count
    }
    pub(in crate::projection) fn ready_generation(&self) -> u64 {
        self.record.boundaries.ready_generation
    }
    pub(in crate::projection) fn ready_capture_sequence(&self) -> u64 {
        self.record.boundaries.ready_capture_sequence
    }
    /// Compare a fresh interpretation after caller I/O. This comparison itself
    /// proves neither a live SQL snapshot nor family/checkout/CAS ownership.
    pub(in crate::projection) fn require_same_import(&self, fresh: &Self) -> Result<()> {
        if self != fresh {
            return Err(invalid("detached admitted import commitments changed"));
        }
        Ok(())
    }
}

pub(super) fn detach(
    marker: &format::Marker,
    proof: &VerifiedAdmittedImport<'_>,
    cancel: Option<&Cancellation>,
) -> Result<AdmittedOrdinaryData> {
    cancellation::check(cancel)?;
    let record = proof.record().clone();
    let admission_record_bytes = record.encode()?;
    let admission_digest =
        super::super::records::codec::hash("layrs-managed-admission-v1", &admission_record_bytes);
    let mut required = Vec::with_capacity(proof.required().len());
    for row in proof.required() {
        cancellation::check(cancel)?;
        required.push(AdmittedRepresentation {
            file_id: row.file_id.clone(),
            manifest_id: row.manifest_id.clone(),
            bytes: row.bytes,
        });
    }
    let expected = proof.expected();
    let mut files = Vec::with_capacity(expected.files().len());
    for file in expected.files() {
        cancellation::check(cancel)?;
        files.push(AdmittedCheckoutFile {
            path: file.path.clone(),
            file_id: file.file_id.clone(),
            manifest_id: file.manifest_id.clone(),
            bytes: file.bytes,
            executable: file.executable,
        });
    }
    let copy_paths = |paths: &[RepoPath]| -> Result<Vec<RepoPath>> {
        let mut result = Vec::with_capacity(paths.len());
        for path in paths {
            cancellation::check(cancel)?;
            result.push(path.clone());
        }
        Ok(result)
    };
    let checkout = AdmittedCheckoutData {
        header_bytes: canonical_json(expected.header()).map_err(|e| invalid(e.to_string()))?,
        files,
        directories: copy_paths(expected.directories())?,
        deletions: copy_paths(expected.deletions())?,
    };
    let archive = AdmittedArchiveData {
        count: record.archive.count,
        bytes: record.archive.bytes,
        digest: record.archive.digest.clone(),
    };
    let ready = ProviderTransitionIntentRecord::decode(&proof.ready().encode()?)?;
    let receipt = ProviderTransitionReceiptRecord::decode(&proof.receipt().encode()?)?;
    let source_published =
        ProviderTransitionIntentRecord::decode(&proof.source_published().encode()?)?;
    cancellation::check(cancel)?;
    Ok(AdmittedOrdinaryData {
        marker: marker.clone(),
        private_schema: proof.private_schema(),
        admission_record_bytes,
        admission_digest,
        record,
        ready,
        receipt,
        source_published,
        required,
        checkout,
        archive,
        admission_column_bytes: proof.admission_column_bytes(),
        witness_column_bytes: proof.witness_column_bytes(),
    })
}
