//! Durable, base-relative local patches; never incomplete SCM admission.
pub(super) mod scan;
pub(super) mod storage;
use super::*;
use layrs_core::scm::{canonical_json, FileId, FileManifest, RepoPath};

pub const MAX_PATCH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CAPTURE_FILE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_CAPTURE_BYTES: u64 = 4 * MAX_CAPTURE_FILE_BYTES;
pub const CAPTURE_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_DOCUMENT: usize = 8 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedFile {
    pub file_id: FileId,
    pub executable: bool,
    pub bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedTarget {
    pub file_id: FileId,
    pub executable: bool,
}
/// Full-file verification witness without an open handle or database lock.
/// Private fields prevent substituting an unverified manifest.
pub struct VerifiedCapturedFile {
    sequence: u64,
    path: RepoPath,
    capture_digest: String,
    manifest: FileManifest,
}
impl VerifiedCapturedFile {
    pub fn manifest(&self) -> &FileManifest {
        &self.manifest
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedChange {
    pub path: RepoPath,
    /// None means explicit absence, never unknown content.
    pub expected: Option<CapturedTarget>,
    pub replacement: Option<CapturedFile>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedPatch {
    pub schema_version: String,
    pub sequence: u64,
    pub previous_patch_digest: Option<String>,
    pub projection_generation: u64,
    pub projection_digest: String,
    pub identity: ProjectionIdentity,
    /// Full diff against the exact remote projection, not the previous capture.
    pub changes: Vec<CapturedChange>,
    pub digest: String,
}
impl CapturedPatch {
    pub(in crate::projection) fn computed_digest(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.digest.clear();
        let bytes = canonical_json(&copy).map_err(invalid)?;
        if bytes.len() > MAX_DOCUMENT {
            return Err(invalid("capture document exceeds 8MiB"));
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs-projection-capture-v1\0");
        hasher.update(&bytes);
        Ok(format!("blake3:{}", hasher.finalize()))
    }
}

impl ProjectionStore {
    /// Capture known writable leaves only. Unknown paths require a fresh explicit
    /// server observation; local absence never grants permission to create.
    /// This performs no network I/O and does not modify checkout files.
    /// An error can follow a durable commit (including retention settlement
    /// contention). Inspect the latest capture or retry the unchanged image;
    /// exact retry deduplicates instead of appending a second patch.
    pub fn capture_patch(&self, expected_generation: u64) -> Result<CapturedPatch> {
        self.capture_patch_with_faults(expected_generation, &NoCaptureFaults)
    }
    pub fn capture_patch_with_faults(
        &self,
        expected_generation: u64,
        faults: &dyn CaptureFaults,
    ) -> Result<CapturedPatch> {
        let access = format::access_for_local_capture(self)?;
        access.run(|access| {
            access.with_content(|c, scope| database::require_mutable_with_scope(c, self, scope))?;
            let state = database::current(&access.connection)?;
            if state.generation != expected_generation {
                return Err(ProjectionStoreError::Stale {
                    expected: expected_generation,
                    actual: state.generation,
                });
            }
            let first = access.with_content(|c, scope| {
                scan::scan_with_scope(self, c, &state.snapshot, false, Some(scope))
            })?;
            faults.check(CapturePoint::FirstScan)?;
            let captured = access.with_content(|c, scope| {
                scan::scan_with_scope(self, c, &state.snapshot, true, Some(scope))
            })?;
            if first.files != captured.files {
                return Err(invalid("checkout changed during partial capture; retry"));
            }
            access.verify()?;
            let result = access.with_content(|c, scope| {
                storage::persist_with_scope(c, self, &state, captured, faults, Some(scope))
            })?;
            access.verify()?;
            #[cfg(windows)]
            if matches!(&access.connection, format::PrivateConnection::Admitted(handle)
                if handle.admission().data().ready().plan.denial_coverage.is_some())
            {
                retention::settle_with_lease(&access.connection, self, access.family.as_ref())?;
                access.verify()?;
            }
            Ok(result)
        })
    }
    pub fn captured_patch(&self, sequence: u64) -> Result<CapturedPatch> {
        let access = format::access(self, SessionMode::Shared)?;
        access.run(|access| {
            access.with_content(|c, scope| database::require_idle_with_scope(c, self, scope))?;
            let patch = access.with_content(|c, scope| {
                storage::load_with_scope(c, self, sequence, Some(scope))
            })?;
            access.verify()?;
            Ok(patch)
        })
    }
    pub fn latest_captured_patch(&self) -> Result<Option<CapturedPatch>> {
        let access = format::access(self, SessionMode::Shared)?;
        access.run(|access| {
            access.with_content(|c, scope| database::require_idle_with_scope(c, self, scope))?;
            let sequence: Option<u64> = access.connection.query_row(
                "SELECT MAX(sequence) FROM projection_captures",
                [],
                |r| r.get(0),
            )?;
            let result = access.with_content(|c, scope| {
                sequence
                    .map(|n| storage::load_with_scope(c, self, n, Some(scope)))
                    .transpose()
            })?;
            access.verify()?;
            Ok(result)
        })
    }
    /// Reopen immutable captured bytes, not current live checkout contents.
    /// The callback runs under a local read lock; prepare bytes before network I/O.
    pub fn write_captured_file(
        &self,
        sequence: u64,
        path: &RepoPath,
        out: &mut dyn io::Write,
    ) -> Result<()> {
        let access = format::access(self, SessionMode::Shared)?;
        access.run(|access| {
            access.with_content(|c, scope| database::require_idle_with_scope(c, self, scope))?;
            let patch = access.with_content(|c, scope| {
                storage::load_with_scope(c, self, sequence, Some(scope))
            })?;
            let file = patch
                .changes
                .iter()
                .find(|c| &c.path == path)
                .and_then(|c| c.replacement.as_ref())
                .ok_or_else(|| invalid("capture has no replacement file at this path"))?;
            access.with_content(|c, scope| {
                storage::stream_file_with_scope(c, self, file, out, Some(scope))
            })?;
            access.verify()?;
            Ok(())
        })
    }
    pub fn captured_file_manifest(&self, sequence: u64, path: &RepoPath) -> Result<FileManifest> {
        Ok(self.verify_captured_file(sequence, path)?.manifest)
    }
    pub fn verify_captured_file(
        &self,
        sequence: u64,
        path: &RepoPath,
    ) -> Result<VerifiedCapturedFile> {
        let access = format::access(self, SessionMode::Shared)?;
        access.run(|access| {
            access.with_content(|c, scope| database::require_idle_with_scope(c, self, scope))?;
            let patch = storage::load_metadata(&access.connection, self, sequence)?;
            let file = replacement(&patch, path)?;
            let manifest = access.with_content(|c, scope| {
                storage::file_manifest_with_scope(c, self, file, Some(scope))
            })?;
            access.verify()?;
            Ok(VerifiedCapturedFile {
                sequence,
                path: path.clone(),
                capture_digest: patch.digest,
                manifest,
            })
        })
    }
    pub fn read_captured_chunk(
        &self,
        sequence: u64,
        path: &RepoPath,
        ordinal: usize,
        out: &mut dyn io::Write,
    ) -> Result<()> {
        let verified = self.verify_captured_file(sequence, path)?;
        self.read_verified_captured_chunk(&verified, ordinal, out)
    }
    pub fn read_verified_captured_chunk(
        &self,
        verified: &VerifiedCapturedFile,
        ordinal: usize,
        out: &mut dyn io::Write,
    ) -> Result<()> {
        let access = format::access(self, SessionMode::Shared)?;
        access.run(|access| {
            access.with_content(|c, scope| database::require_idle_with_scope(c, self, scope))?;
            let patch = storage::load_metadata(&access.connection, self, verified.sequence)?;
            let file = replacement(&patch, &verified.path)?;
            if patch.digest != verified.capture_digest
                || &file.file_id != verified.manifest.file_id()
                || file.bytes != verified.manifest.byte_length()
            {
                return Err(invalid("verified capture binding changed"));
            }
            let chunk = verified
                .manifest
                .chunks()
                .get(ordinal)
                .ok_or_else(|| invalid("capture chunk ordinal out of range"))?;
            access.with_content(|c, scope| {
                storage::read_chunk_with_scope(c, self, file, ordinal, chunk, out, Some(scope))
            })?;
            access.verify()?;
            Ok(())
        })
    }
}
fn replacement<'a>(patch: &'a CapturedPatch, path: &RepoPath) -> Result<&'a CapturedFile> {
    patch
        .changes
        .iter()
        .find(|c| &c.path == path)
        .and_then(|c| c.replacement.as_ref())
        .ok_or_else(|| invalid("capture has no replacement file at this path"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturePoint {
    FirstScan,
    BeforeCommit,
    AfterCommit,
}
pub trait CaptureFaults {
    fn check(&self, point: CapturePoint) -> io::Result<()>;
}
pub struct NoCaptureFaults;
impl CaptureFaults for NoCaptureFaults {
    fn check(&self, _: CapturePoint) -> io::Result<()> {
        Ok(())
    }
}
