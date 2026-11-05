//! Explicit Git interoperability boundary. Native capture/admission never call
//! this crate. Plumbing commands read/write object bytes without checkout,
//! filters, credential helpers, hooks or network access.
use layrs_core::scm::*;
use layrs_store::scm::CanonicalObject;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

mod expected_state;
mod export;
mod import;
mod install;
mod pack;
mod process;
mod scan_checkpoint;
pub use expected_state::ExpectedBridgeState;
pub use export::{
    ExportMode, ExportReport, export_workspace, export_workspace_expected,
    export_workspace_with_progress, resume_export_workspace, resume_export_workspace_expected,
};
pub use export::{
    export_workspace_reviewed, resume_export_workspace_reviewed, review_export_destination,
};
pub use import::{
    scan_git, scan_git_selected, scan_git_selected_with_progress, scan_git_with_progress,
};
pub use install::ImportOutcome;
pub use layrs_store_local::anchored_export::ExportDestination;
pub use scan_checkpoint::{
    GitScanCheckpoint, prepare_git_scan_checkpoint, resume_git_scan_checkpoint,
};

#[derive(Debug)]
pub enum BridgeError {
    Io(std::io::Error),
    Invalid(String),
    Unsupported(String),
    Limit(&'static str),
    Git(String),
    Cancelled,
}
impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BridgeError {}
impl From<std::io::Error> for BridgeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
pub type Result<T> = std::result::Result<T, BridgeError>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgePhase {
    ScanReferences,
    ImportCommits,
    ImportComplete,
    ExportPlan,
    ExportCommits,
    ExportReferences,
    ExportComplete,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeProgress {
    pub phase: BridgePhase,
    pub completed: usize,
    pub total: Option<usize>,
    pub objects: usize,
    pub bytes: usize,
    pub elapsed_ms: u64,
    pub git_processes: usize,
}
pub type ProgressCallback<'a> = dyn FnMut(&BridgeProgress) -> bool + 'a;
#[allow(clippy::too_many_arguments)]
fn progress(
    callback: &mut ProgressCallback<'_>,
    start: std::time::Instant,
    phase: BridgePhase,
    completed: usize,
    total: Option<usize>,
    objects: usize,
    bytes: usize,
    git_processes: usize,
) -> Result<()> {
    if callback(&BridgeProgress {
        phase,
        completed,
        total,
        objects,
        bytes,
        elapsed_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        git_processes,
    }) {
        Ok(())
    } else {
        Err(BridgeError::Cancelled)
    }
}
fn invalid(e: impl std::fmt::Display) -> BridgeError {
    BridgeError::Invalid(e.to_string())
}

#[derive(Debug, Clone, Copy)]
pub struct GitBridgeLimits {
    pub max_commits: usize,
    pub max_objects: usize,
    pub max_paths: usize,
    pub max_depth: usize,
    pub max_object_bytes: usize,
    pub max_total_bytes: usize,
    pub max_duration_ms: u64,
}
impl Default for GitBridgeLimits {
    fn default() -> Self {
        Self {
            max_commits: 10_000,
            max_objects: 100_000,
            max_paths: 100_000,
            max_depth: 64,
            max_object_bytes: 16 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_duration_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitCommitMapping {
    pub git_id: String,
    pub step_id: StepId,
    pub parent_git_ids: Vec<String>,
    /// Exact Git commit bytes retained as provenance, not Step identity.
    pub raw_commit_base64: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitRefMapping {
    pub git_ref: String,
    pub git_id: String,
    pub step_id: StepId,
    pub annotated_tag_base64: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportReport {
    pub import_id: String,
    pub object_format: String,
    pub commits: Vec<GitCommitMapping>,
    pub references: Vec<GitRefMapping>,
    pub warnings: Vec<String>,
    pub source_bytes: usize,
}
pub struct ImportPlan {
    pub(crate) report: ImportReport,
    pub(crate) objects: Vec<CanonicalObject>,
}
#[derive(Debug, Clone)]
pub enum ImportSelection {
    AllLocalReferences,
    /// Exact full refs/heads/... or refs/tags/... names, with all ancestors.
    References(Vec<String>),
}
impl ImportPlan {
    pub fn report(&self) -> &ImportReport {
        &self.report
    }
    pub fn canonical_objects(&self) -> &[CanonicalObject] {
        &self.objects
    }
}

fn hex_id(text: &str) -> Result<&str> {
    if ![40, 64].contains(&text.len())
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("expected full lowercase Git object ID"));
    }
    Ok(text)
}
fn utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(invalid)
}
fn signature_start(bytes: &[u8]) -> Option<usize> {
    [
        b"-----BEGIN PGP SIGNATURE-----".as_slice(),
        b"-----BEGIN SSH SIGNATURE-----",
        b"-----BEGIN SIGNED MESSAGE-----",
    ]
    .iter()
    .filter_map(|marker| {
        bytes
            .windows(marker.len())
            .position(|window| window == *marker)
    })
    .min()
}
fn component(name: &[u8]) -> Result<RepoPathComponent> {
    let text = utf8(name)?;
    if [".git", ".layrs"]
        .iter()
        .any(|reserved| text.eq_ignore_ascii_case(reserved))
    {
        return Err(BridgeError::Unsupported(
            "reserved metadata path in Git tree".into(),
        ));
    }
    RepoPathComponent::parse(text).map_err(invalid)
}
fn metadata_value<'a>(metadata: &'a Metadata, key: &str) -> Option<&'a str> {
    metadata
        .entries()
        .iter()
        .find(|entry| entry.key() == key)
        .map(MetadataEntry::value)
}

#[cfg(test)]
mod tests;
