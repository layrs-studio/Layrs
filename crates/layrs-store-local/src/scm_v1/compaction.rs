//! Physical SQLite compaction. Every logical row and every CAS object is retained.
//! Publication uses an independently validated copy; a durable journal fences
//! all normal Store opens until completion or explicit resume.
mod database;
mod journal;

use super::{NativeStoreInspection, inspect, sqlite};
use crate::{
    layout::sync_directory,
    lock::{SessionMode, StoreLock},
};
use layrs_core::scm::StoreId;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub type CompactionResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub(crate) const ACTIVE: &str = "metadata-compaction.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataCompactionReport {
    pub schema_version: String,
    pub store_id: StoreId,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub logical_digest: String,
    pub retained_rows: u64,
    pub backup_directory: PathBuf,
    pub history_deleted: bool,
    /// Physical SQLite rowids are not stable identities. Existing observation
    /// snapshots are verified on reuse and may require restarting traversal.
    pub restart_history_traversal: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionFaultPoint {
    CandidateReady,
    JournalDurable,
    DatabasePublished,
    CompletionDurable,
}
pub trait CompactionFaults {
    fn check(&self, point: CompactionFaultPoint) -> CompactionResult<()>;
}
struct NoFaults;
impl CompactionFaults for NoFaults {
    fn check(&self, _: CompactionFaultPoint) -> CompactionResult<()> {
        Ok(())
    }
}

pub fn compact_metadata(root: impl AsRef<Path>) -> CompactionResult<MetadataCompactionReport> {
    compact_metadata_with_faults(root, &NoFaults)
}

/// Operator-triggered physical maintenance; refuses unfinished recovery in any
/// Worktree. Backup files remain available and are never automatically deleted.
pub fn compact_metadata_with_faults(
    root: impl AsRef<Path>,
    faults: &dyn CompactionFaults,
) -> CompactionResult<MetadataCompactionReport> {
    compact_metadata_impl(root.as_ref(), faults, None)
}

pub(crate) fn compact_metadata_audited(
    audit: &super::worktree_audit::AuditedWorktreeStore,
) -> CompactionResult<MetadataCompactionReport> {
    compact_metadata_impl(audit.store_root(), &NoFaults, Some(audit))
}

fn compact_metadata_impl(
    root: &Path,
    faults: &dyn CompactionFaults,
    audit: Option<&super::worktree_audit::AuditedWorktreeStore>,
) -> CompactionResult<MetadataCompactionReport> {
    let store = super::open_exclusive(root)?;
    let layout = store.layout();
    if let Some(audit) = audit {
        audit.validate(layout, store.store_id())?;
    }
    database::require_empty_sidecars(layout.sqlite_path())?;
    let before_hash = journal::file_hash(layout.sqlite_path())?;
    let source = sqlite::open_read_only(layout.sqlite_path(), Duration::from_secs(2))?;
    database::require_idle(&source)?;
    let original = database::snapshot(&source)?;
    let directory = journal::create_directory(layout)?;
    let backup = directory.join("before.sqlite3");
    journal::copy_new(layout.sqlite_path(), &backup)?;
    if journal::file_hash(&backup)? != before_hash {
        return Err("compaction backup changed".into());
    }
    let candidate = directory.join("compacted.sqlite3");
    // VACUUM INTO writes only its independent destination, not the immutable source.
    drop(crate::layout::create_new_file_no_follow(&candidate)?);
    source.execute(
        "VACUUM INTO ?1",
        [candidate.to_str().ok_or("non-UTF8 compaction path")?],
    )?;
    drop(source);
    database::prepare_candidate(&candidate)?;
    let reader = sqlite::open_read_only(&candidate, Duration::from_secs(2))?;
    sqlite::validate_store_identity(&reader, store.store_id())?;
    if database::snapshot(&reader)? != original {
        return Err("compaction changed logical rows".into());
    }
    drop(reader);
    let after_hash = journal::file_hash(&candidate)?;
    let report = MetadataCompactionReport {
        schema_version: "layrs.metadata-compaction.v1".into(),
        store_id: store.store_id().clone(),
        before_bytes: fs::metadata(&backup)?.len(),
        after_bytes: fs::metadata(&candidate)?.len(),
        logical_digest: original.0,
        retained_rows: original.1,
        backup_directory: directory.clone(),
        history_deleted: false,
        restart_history_traversal: true,
    };
    faults.check(CompactionFaultPoint::CandidateReady)?;
    if let Some(audit) = audit {
        audit.validate(layout, store.store_id())?;
    }
    let record = journal::Journal {
        report,
        before_hash,
        after_hash,
    };
    journal::publish(layout, &record)?;
    faults.check(CompactionFaultPoint::JournalDurable)?;
    finish(layout, &record, faults, audit)
}

/// Resumes a journaled physical replacement. An altered database, backup or
/// candidate is refused; this API never guesses which logical state is correct.
pub fn resume_metadata_compaction(
    root: impl AsRef<Path>,
) -> CompactionResult<MetadataCompactionReport> {
    resume_metadata_compaction_impl(root.as_ref(), None)
}

pub(crate) fn resume_metadata_compaction_audited(
    audit: &super::worktree_audit::AuditedWorktreeStore,
) -> CompactionResult<MetadataCompactionReport> {
    resume_metadata_compaction_impl(audit.store_root(), Some(audit))
}

fn resume_metadata_compaction_impl(
    root: &Path,
    audit: Option<&super::worktree_audit::AuditedWorktreeStore>,
) -> CompactionResult<MetadataCompactionReport> {
    let initial = inspect(root)?;
    let (layout, id) = match initial {
        NativeStoreInspection::NativeReady { layout, marker } => {
            (layout, marker.store_id().clone())
        }
        _ => return Err("compaction requires a Ready native Store".into()),
    };
    let _lock = StoreLock::acquire_existing(layout.lock_path(), SessionMode::Exclusive)?;
    if let Some(audit) = audit {
        audit.validate(&layout, &id)?;
    }
    match inspect(layout.root())? {
        NativeStoreInspection::NativeReady { marker, .. } if marker.store_id() == &id => (),
        _ => return Err("compaction Store identity changed".into()),
    }
    let record = journal::read(&layout)?;
    if record.report.store_id != id {
        return Err("compaction journal belongs to another Store".into());
    }
    finish(&layout, &record, &NoFaults, audit)
}

fn finish(
    layout: &super::NativeStoreLayout,
    record: &journal::Journal,
    faults: &dyn CompactionFaults,
    audit: Option<&super::worktree_audit::AuditedWorktreeStore>,
) -> CompactionResult<MetadataCompactionReport> {
    if let Some(audit) = audit {
        audit.validate(layout, &record.report.store_id)?;
    }
    journal::validate(layout, record)?;
    let backup = record.report.backup_directory.join("before.sqlite3");
    database::verify(&backup, &record.report)?;
    let current = journal::file_hash(layout.sqlite_path())?;
    if current != record.after_hash && current == record.before_hash {
        let candidate = record.report.backup_directory.join("compacted.sqlite3");
        if journal::file_hash(&candidate)? != record.after_hash {
            return Err("compaction candidate hash changed".into());
        }
        database::verify(&candidate, &record.report)?;
        database::require_empty_sidecars(layout.sqlite_path())?;
        super::format::atomic_replace(&candidate, layout.sqlite_path())?;
        sync_directory(record.report.backup_directory.as_path())?;
        sync_directory(layout.metadata_dir())?;
    } else if current != record.after_hash {
        return Err("compaction live database changed; preserve backup and journal".into());
    }
    faults.check(CompactionFaultPoint::DatabasePublished)?;
    if let Some(audit) = audit {
        audit.validate(layout, &record.report.store_id)?;
    }
    database::verify(layout.sqlite_path(), &record.report)?;
    journal::complete(layout, record)?;
    faults.check(CompactionFaultPoint::CompletionDurable)?;
    if let Some(audit) = audit {
        audit.validate(layout, &record.report.store_id)?;
    }
    fs::remove_file(layout.metadata_dir().join(ACTIVE))?;
    sync_directory(layout.metadata_dir())?;
    Ok(record.report.clone())
}

pub(crate) fn refuse_active(layout: &super::NativeStoreLayout) -> std::io::Result<()> {
    match fs::symlink_metadata(layout.metadata_dir().join(ACTIVE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(_) => Err(std::io::Error::other(
            "metadata compaction incomplete; run explicit compaction resume",
        )),
    }
}
