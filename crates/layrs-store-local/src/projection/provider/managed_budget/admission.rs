//! Pure admitted private9/private10 sizing DATA. Planners opt into each envelope;
//! historical Ready8 readers retain their original accounting. No function here
//! grows a reservation, validates SQL rows or authenticates a SourcePublished.
use super::{add_metadata, invalid, Result, MAX_METADATA};

const MAX_FILES: u64 = 100_000;
const MAX_DIRECTORIES: u64 = 100_000;
const MAX_ROWS: u64 = MAX_FILES + MAX_DIRECTORIES + 2;
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
// payload + SQL path + operation + digest + state ('owned').
const MAX_ARCHIVE_ROW_BYTES: u64 = 8192 + 128 + 64 + 71 + 5;
const MIN_ARCHIVE_ROW_BYTES: u64 = 1 + 1 + 71 + 1 + 5;
const MAX_SOURCE_PUBLISHED_BYTES: u64 = 262144;
const MAX_ADMISSION_RECORD_BYTES: u64 = 65536;
// Four digest/reference TEXT columns and four INTEGER columns. Their values
// must separately pass the typed admission interpreter before use by a writer.
const ADMISSION_FIXED_BYTES: u64 = 4 * 71 + 4 * 8;
const MAX_ADMISSION_BYTES: u64 =
    ADMISSION_FIXED_BYTES + MAX_SOURCE_PUBLISHED_BYTES + MAX_ADMISSION_RECORD_BYTES;
const MAX_WITNESS_ROWS: u64 = 100_000;
const WITNESS_ROW_BYTES: u64 = 71 + 71 + 8;
const MAX_WITNESS_BYTES: u64 = 15_000_000;

/// Normalized logical values only, excluding SQLite pages, indices and WAL.
/// Closed fields ensure every instance was checked with the same layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) struct AdmissionBudget {
    archive_rows: u64,
    archive_bound: u64,
    minimum_metadata: u64,
    witness_rows: u64,
    witness_bound: u64,
}

/// Actual bounded column lengths supplied by a future SQL writer. This DATA
/// never substitutes for canonical bodies, exact archive rows or their digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) struct AdmissionCost {
    archive_rows: u64,
    archive_bytes: u64,
    admission_bytes: u64,
    witness_rows: u64,
    witness_bytes: u64,
}

/// F_ready_bound excludes checkout journal/archive columns and the admission
/// record. Count the original journal J and its copy J simultaneously: INSERT
/// SELECT precedes deleting the operational journal in the caller transaction.
pub(in crate::projection::provider) fn admission_budget(
    ready_metadata_bound: u64,
    checkout_files: u64,
    checkout_directories: u64,
) -> Result<AdmissionBudget> {
    if checkout_files > MAX_FILES || checkout_directories > MAX_DIRECTORIES {
        return Err(invalid("admission checkout inventory count exceeds bound"));
    }
    let archive_rows = checkout_files
        .checked_add(checkout_directories)
        .and_then(|n| n.checked_add(2))
        .filter(|n| *n <= MAX_ROWS)
        .ok_or_else(|| invalid("admission archive row count overflow"))?;
    let archive_bound = archive_rows
        .checked_mul(MAX_ARCHIVE_ROW_BYTES)
        .ok_or_else(|| invalid("admission archive column bound overflow"))?
        .min(MAX_ARCHIVE_BYTES);
    let two_archives = archive_bound
        .checked_mul(2)
        .ok_or_else(|| invalid("admission archive copy peak overflow"))?;
    let minimum_metadata = add_metadata(
        add_metadata(ready_metadata_bound, two_archives)?,
        MAX_ADMISSION_BYTES,
    )?;
    Ok(AdmissionBudget {
        archive_rows,
        archive_bound,
        minimum_metadata,
        witness_rows: 0,
        witness_bound: 0,
    })
}

/// Future C14 archive peak from actual selected placement and overlay DATA.
/// Count only final checkout files and their unique parents; no destination
/// identity, journal, receipt or physical proof is synthesized for sizing.
pub(super) fn denial_admission_budget(
    ready_metadata_bound: u64,
    placement: &layrs_core::projection::ProjectionSnapshot,
    selected: &layrs_core::projection::ProjectionSelection,
    capture: &crate::projection::capture::CapturedPatch,
    cancel: Option<&layrs_vfs::Cancellation>,
) -> Result<AdmissionBudget> {
    use layrs_core::{projection::EntryState, scm::TreeEntryTarget};
    use std::collections::{BTreeMap, BTreeSet};
    let mut files = BTreeSet::new();
    for entry in &placement.entries {
        super::cancellation::check(cancel)?;
        if selected.includes(&entry.path)
            && entry.state != EntryState::Reserved
            && matches!(entry.target, Some(TreeEntryTarget::File { .. }))
        {
            files.insert(entry.path.as_str());
        }
        if files.len() as u64 > MAX_FILES {
            return Err(invalid("admission checkout inventory count exceeds bound"));
        }
    }
    for change in &capture.changes {
        super::cancellation::check(cancel)?;
        if change.replacement.is_some() {
            files.insert(change.path.as_str());
        } else {
            files.remove(change.path.as_str());
        }
    }
    if files.len() as u64 > MAX_FILES {
        return Err(invalid("admission checkout inventory count exceeds bound"));
    }
    let mut dirs = BTreeMap::new();
    let mut path_bytes = 0u64;
    for path in &files {
        super::cancellation::check(cancel)?;
        for (end, _) in path.match_indices('/') {
            let parent = &path[..end];
            let key = parent.to_uppercase();
            if let Some(prior) = dirs.get(&key) {
                if prior != &parent {
                    return Err(invalid("admission checkout parent spelling aliases"));
                }
                continue;
            }
            if dirs.len() as u64 >= MAX_DIRECTORIES {
                return Err(invalid("admission checkout inventory count exceeds bound"));
            }
            path_bytes = path_bytes
                .checked_add(parent.len() as u64)
                .filter(|bytes| *bytes <= MAX_ARCHIVE_BYTES)
                .ok_or_else(|| invalid("admission checkout parent metadata exceeds bound"))?;
            dirs.insert(key, parent);
        }
    }
    admission_budget(ready_metadata_bound, files.len() as u64, dirs.len() as u64)
}

/// Ordinary10 reserves its complete immutable representation witness in M.
/// This does not consume the separately funded family protection margin.
pub(in crate::projection::provider) fn ordinary_admission_budget(
    ready_metadata_bound: u64,
    checkout_files: u64,
    checkout_directories: u64,
    representations: u64,
) -> Result<AdmissionBudget> {
    let witness_bound = witness_bytes(representations)?;
    let mut budget = admission_budget(ready_metadata_bound, checkout_files, checkout_directories)?;
    budget.minimum_metadata = add_metadata(budget.minimum_metadata, witness_bound)?;
    budget.witness_rows = representations;
    budget.witness_bound = witness_bound;
    Ok(budget)
}

fn witness_bytes(rows: u64) -> Result<u64> {
    rows.checked_mul(WITNESS_ROW_BYTES)
        .filter(|bytes| rows <= MAX_WITNESS_ROWS && *bytes <= MAX_WITNESS_BYTES)
        .ok_or_else(|| invalid("ordinary admission witness exceeds bounded inventory"))
}

/// Compute A_actual and J_actual after a separately bounded SQL inventory.
/// Every fixed-width column is charged in full; receipt/ACK/record semantics
/// remain the responsibility of the typed provenance reader.
pub(in crate::projection::provider) fn admission_cost(
    source_published_bytes: u64,
    admission_record_bytes: u64,
    archive_rows: u64,
    archive_column_bytes: u64,
) -> Result<AdmissionCost> {
    if !(1..=MAX_SOURCE_PUBLISHED_BYTES).contains(&source_published_bytes)
        || !(1..=MAX_ADMISSION_RECORD_BYTES).contains(&admission_record_bytes)
        || !(2..=MAX_ROWS).contains(&archive_rows)
    {
        return Err(invalid("admission actual column or row bound exceeded"));
    }
    let lower = archive_rows
        .checked_mul(MIN_ARCHIVE_ROW_BYTES)
        .ok_or_else(|| invalid("admission actual archive lower bound overflow"))?;
    let upper = archive_rows
        .checked_mul(MAX_ARCHIVE_ROW_BYTES)
        .ok_or_else(|| invalid("admission actual archive upper bound overflow"))?
        .min(MAX_ARCHIVE_BYTES);
    if !(lower..=upper).contains(&archive_column_bytes) {
        return Err(invalid(
            "admission actual archive columns exceed row envelope",
        ));
    }
    let admission_bytes = ADMISSION_FIXED_BYTES
        .checked_add(source_published_bytes)
        .and_then(|n| n.checked_add(admission_record_bytes))
        .filter(|n| *n <= MAX_ADMISSION_BYTES)
        .ok_or_else(|| invalid("admission actual record columns overflow"))?;
    Ok(AdmissionCost {
        archive_rows,
        archive_bytes: archive_column_bytes,
        admission_bytes,
        witness_rows: 0,
        witness_bytes: 0,
    })
}

pub(in crate::projection::provider) fn ordinary_admission_cost(
    source_published_bytes: u64,
    admission_record_bytes: u64,
    archive_rows: u64,
    archive_column_bytes: u64,
    witness_rows: u64,
    witness_column_bytes: u64,
) -> Result<AdmissionCost> {
    if witness_column_bytes != witness_bytes(witness_rows)? {
        return Err(invalid("ordinary admission actual witness bytes differ"));
    }
    let mut cost = admission_cost(
        source_published_bytes,
        admission_record_bytes,
        archive_rows,
        archive_column_bytes,
    )?;
    cost.witness_rows = witness_rows;
    cost.witness_bytes = witness_column_bytes;
    Ok(cost)
}

impl AdmissionBudget {
    pub(in crate::projection::provider) fn witness_rows(&self) -> u64 {
        self.witness_rows
    }
    pub(in crate::projection::provider) fn witness_bound(&self) -> u64 {
        self.witness_bound
    }
    pub(in crate::projection::provider) fn archive_rows(&self) -> u64 {
        self.archive_rows
    }
    pub(in crate::projection::provider) fn archive_bound(&self) -> u64 {
        self.archive_bound
    }
    pub(in crate::projection::provider) fn admission_bound(&self) -> u64 {
        MAX_ADMISSION_BYTES
    }
    /// Minimum M before the separate family root/relations/frame margin.
    pub(in crate::projection::provider) fn minimum_metadata(&self) -> u64 {
        self.minimum_metadata
    }
    pub(in crate::projection::provider) fn require_metadata(&self, metadata: u64) -> Result<()> {
        if metadata < self.minimum_metadata || metadata > MAX_METADATA {
            return Err(invalid(
                "admission fixed metadata envelope insufficient or exceeds 1GiB",
            ));
        }
        Ok(())
    }
    /// Q0 is the identical plan's quota with M=0. Never replace Q with a
    /// computed minimum: old reservations lacking capacity must be refused.
    pub(in crate::projection::provider) fn require_quota(
        &self,
        durable_quota: u64,
        baseline_quota: u64,
    ) -> Result<u64> {
        if durable_quota > i64::MAX as u64 || baseline_quota > i64::MAX as u64 {
            return Err(invalid("admission quota exceeds SQLite range"));
        }
        let metadata = durable_quota
            .checked_sub(baseline_quota)
            .ok_or_else(|| invalid("admission quota is below its exact baseline"))?;
        self.require_metadata(metadata)?;
        Ok(metadata)
    }
    /// Preserve the existing family envelope independently of private M:
    /// root payload + 256*N_manifest + 65536 + F_ready + 2J_bound + Amax.
    pub(in crate::projection::provider) fn require_remaining(
        &self,
        remaining: u64,
        root_payload_bytes: u64,
        manifest_count: u64,
    ) -> Result<u64> {
        if root_payload_bytes > 16 * 1024 * 1024 || manifest_count > 100_000 {
            return Err(invalid("admission protection metadata exceeds bound"));
        }
        let needed = manifest_count
            .checked_mul(256)
            .and_then(|n| n.checked_add(root_payload_bytes))
            .and_then(|n| n.checked_add(65536))
            .and_then(|n| n.checked_add(self.minimum_metadata))
            .ok_or_else(|| invalid("admission retained envelope overflow"))?;
        if remaining > i64::MAX as u64 || remaining < needed {
            return Err(invalid(
                "admission retained reservation envelope insufficient",
            ));
        }
        Ok(needed)
    }
    /// An inexpensive actual archive does not authorize shrinking the original
    /// fixed envelope, nor a different inventory with the same aggregate size.
    pub(in crate::projection::provider) fn require_actual(
        &self,
        metadata: u64,
        actual: &AdmissionCost,
    ) -> Result<()> {
        self.require_metadata(metadata)?;
        if actual.archive_rows != self.archive_rows
            || actual.archive_bytes > self.archive_bound
            || actual.admission_bytes > MAX_ADMISSION_BYTES
            || actual.witness_rows != self.witness_rows
            || actual.witness_bytes != self.witness_bound
        {
            return Err(invalid(
                "admission actual costs differ from planned inventory",
            ));
        }
        Ok(())
    }
}
impl AdmissionCost {
    pub(in crate::projection::provider) fn witness_bytes(&self) -> u64 {
        self.witness_bytes
    }
    pub(in crate::projection::provider) fn archive_bytes(&self) -> u64 {
        self.archive_bytes
    }
    pub(in crate::projection::provider) fn admission_bytes(&self) -> u64 {
        self.admission_bytes
    }
    pub(in crate::projection::provider) fn copy_peak_bytes(&self) -> Result<u64> {
        self.archive_bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(self.admission_bytes))
            .and_then(|n| n.checked_add(self.witness_bytes))
            .ok_or_else(|| invalid("admission actual copy peak overflow"))
    }
}

#[cfg(test)]
mod tests;
