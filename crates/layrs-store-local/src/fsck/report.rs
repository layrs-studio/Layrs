use std::fs;

use crate::error::{StoreError, StoreResult};
use crate::layout::{StoreLayout, normalize_path_key};

pub(super) const MAX_FSCK_ISSUES: usize = 100;
pub(super) const MAX_FSCK_TRAVERSAL_ROWS: usize = 10_000;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FsckCode {
    Integrity,
    ForeignKey,
    Migration,
    SchemaVersion,
    RefInvariant,
    StepInvariant,
    PendingInvariant,
    JournalInvariant,
    TimelineInvariant,
    WeaveInvariant,
    GcInvariant,
    ObjectMissing,
    ObjectCorrupt,
    ObjectPathUnsafe,
    ObjectGraphCycle,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FsckIssue {
    pub code: FsckCode,
    pub entity: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsckReport {
    pub(super) sqlite_version: String,
    pub(super) schema_version: Option<u32>,
    pub(super) checked_objects: u64,
    pub(super) issues: Vec<FsckIssue>,
    pub(super) omitted_issues: u64,
    pub(super) truncated_checks: u64,
    pub(super) binding: FsckBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FsckBinding {
    pub(super) store_identity: String,
    pub(super) quarantine_generation: Option<u64>,
    pub(super) snapshot: String,
}

impl FsckReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty() && self.omitted_issues == 0 && self.truncated_checks == 0
    }

    pub fn issues_with_code(&self, code: FsckCode) -> impl Iterator<Item = &FsckIssue> {
        self.issues.iter().filter(move |issue| issue.code == code)
    }

    pub fn sqlite_version(&self) -> &str {
        &self.sqlite_version
    }

    pub fn schema_version(&self) -> Option<u32> {
        self.schema_version
    }

    pub fn checked_objects(&self) -> u64 {
        self.checked_objects
    }

    pub fn issues(&self) -> &[FsckIssue] {
        &self.issues
    }

    pub fn omitted_issues(&self) -> u64 {
        self.omitted_issues
    }

    pub fn truncated_checks(&self) -> u64 {
        self.truncated_checks
    }

    pub fn is_truncated(&self) -> bool {
        self.omitted_issues != 0 || self.truncated_checks != 0
    }

    pub(super) fn new(
        layout: &StoreLayout,
        sqlite_version: String,
        schema_version: Option<u32>,
        quarantine_generation: Option<u64>,
        created_at_ms: Option<i64>,
    ) -> StoreResult<Self> {
        Ok(Self {
            sqlite_version,
            schema_version,
            checked_objects: 0,
            issues: Vec::new(),
            omitted_issues: 0,
            truncated_checks: 0,
            binding: FsckBinding {
                store_identity: store_identity(layout, created_at_ms)?,
                quarantine_generation,
                snapshot: String::new(),
            },
        })
    }

    pub(super) fn bind_snapshot(&mut self, schema_cookie: u32, updated_at_ms: Option<i64>) {
        self.binding.snapshot = snapshot_digest(self, schema_cookie, updated_at_ms);
    }

    pub(super) fn push_issue(&mut self, issue: FsckIssue) {
        if self.issues.len() < MAX_FSCK_ISSUES {
            self.issues.push(issue);
        } else {
            self.omitted_issues = self.omitted_issues.saturating_add(1);
        }
    }

    pub(super) fn remaining_issue_capacity(&self) -> usize {
        MAX_FSCK_ISSUES.saturating_sub(self.issues.len())
    }

    pub(super) fn issue_query_limit(&self) -> i64 {
        self.remaining_issue_capacity().saturating_add(1) as i64
    }

    pub(super) fn note_issue_query_truncated(&mut self) {
        self.omitted_issues = self.omitted_issues.saturating_add(1);
        self.note_truncated_checks(1);
    }

    pub(super) fn note_truncated_checks(&mut self, count: u64) {
        self.truncated_checks = self.truncated_checks.saturating_add(count);
    }
}

pub(super) fn validate_header(
    layout: &StoreLayout,
    connection: &rusqlite::Connection,
    findings: &FsckReport,
) -> StoreResult<()> {
    let (generation, created_at_ms): (u64, i64) = connection.query_row(
        "SELECT quarantine_generation, created_at_ms FROM store_meta WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let identity = store_identity(layout, Some(created_at_ms))?;
    if findings.binding.store_identity != identity {
        return Err(StoreError::InvalidFsckReport(
            "report belongs to a different store identity".to_string(),
        ));
    }
    if findings.binding.quarantine_generation != Some(generation) {
        return Err(StoreError::InvalidFsckReport(format!(
            "report generation is stale: expected {:?}, got {generation}",
            findings.binding.quarantine_generation
        )));
    }
    Ok(())
}

pub(super) fn validate_snapshot(findings: &FsckReport, fresh: &FsckReport) -> StoreResult<()> {
    if findings.binding.store_identity != fresh.binding.store_identity {
        return Err(StoreError::InvalidFsckReport(
            "report belongs to a different store identity".to_string(),
        ));
    }
    if findings.binding.quarantine_generation != fresh.binding.quarantine_generation {
        return Err(StoreError::InvalidFsckReport(
            "report quarantine generation is stale".to_string(),
        ));
    }
    for finding in &findings.issues {
        if !fresh.issues.contains(finding) {
            return Err(StoreError::InvalidFsckReport(format!(
                "finding is stale or no longer reproducible: {:?} {:?}",
                finding.code, finding.entity
            )));
        }
    }
    if findings.binding.snapshot != fresh.binding.snapshot {
        return Err(StoreError::InvalidFsckReport(
            "store snapshot changed after fsck".to_string(),
        ));
    }
    Ok(())
}

fn store_identity(layout: &StoreLayout, created_at_ms: Option<i64>) -> StoreResult<String> {
    let canonical_root = fs::canonicalize(&layout.root)?;
    let canonical_database = fs::canonicalize(&layout.sqlite_path)?;
    let database_metadata = fs::metadata(&canonical_database)?;
    let mut hasher = blake3::Hasher::new();
    hash_field(
        &mut hasher,
        normalize_path_key(&canonical_root, cfg!(not(windows))).as_bytes(),
    );
    hash_field(
        &mut hasher,
        normalize_path_key(&canonical_database, cfg!(not(windows))).as_bytes(),
    );
    hash_field(
        &mut hasher,
        &created_at_ms.unwrap_or(i64::MIN).to_le_bytes(),
    );
    hash_file_identity(&mut hasher, &database_metadata);
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_file_identity(hasher: &mut blake3::Hasher, metadata: &fs::Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash_field(hasher, &metadata.dev().to_le_bytes());
        hash_field(hasher, &metadata.ino().to_le_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        hash_field(hasher, &metadata.creation_time().to_le_bytes());
    }
    #[cfg(not(any(unix, windows)))]
    {
        hash_field(hasher, &metadata.len().to_le_bytes());
    }
}

fn snapshot_digest(report: &FsckReport, schema_cookie: u32, updated_at_ms: Option<i64>) -> String {
    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, b"layrs-fsck-snapshot-v1");
    hash_field(&mut hasher, &schema_cookie.to_le_bytes());
    hash_field(
        &mut hasher,
        &updated_at_ms.unwrap_or(i64::MIN).to_le_bytes(),
    );
    hash_field(&mut hasher, &report.checked_objects.to_le_bytes());
    hash_field(&mut hasher, &report.omitted_issues.to_le_bytes());
    hash_field(&mut hasher, &report.truncated_checks.to_le_bytes());
    let mut issues = report.issues.clone();
    issues.sort();
    for issue in issues {
        hash_field(&mut hasher, &[issue.code as u8]);
        hash_field(
            &mut hasher,
            issue.entity.as_deref().unwrap_or_default().as_bytes(),
        );
        hash_field(&mut hasher, issue.detail.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}
