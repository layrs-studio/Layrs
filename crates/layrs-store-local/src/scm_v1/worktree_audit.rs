//! Checkout-to-Store resolution that remains usable during physical maintenance
//! or damaged selected Views. Identity records are still mandatory evidence.
use super::{
    NativeStoreAudit, NativeStoreInspection, checkout::CheckoutReader, inspect, sqlite, worktrees,
};
use crate::lock::{SessionMode, StoreLock};
use layrs_core::scm::{StoreId, Worktree};
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn resolve_store_for_audit(root: &Path) -> io::Result<PathBuf> {
    Ok(audit_worktree_store(root)?.store_root)
}

/// Read-only identity evidence retained across a later maintenance call. No
/// Store lease is held while idle, and callers cannot fabricate its fields.
pub struct AuditedWorktreeStore {
    store_root: PathBuf,
    store_id: StoreId,
    store_directory: CheckoutReader,
    metadata_directory: CheckoutReader,
    checkout: CheckoutReader,
    worktree: Worktree,
}
impl AuditedWorktreeStore {
    pub fn store_root(&self) -> &Path {
        &self.store_root
    }
    pub fn compact_metadata(
        &self,
    ) -> super::compaction::CompactionResult<super::compaction::MetadataCompactionReport> {
        super::compaction::compact_metadata_audited(self)
    }
    pub fn resume_metadata_compaction(
        &self,
    ) -> super::compaction::CompactionResult<super::compaction::MetadataCompactionReport> {
        super::compaction::resume_metadata_compaction_audited(self)
    }
    pub(crate) fn validate(
        &self,
        layout: &super::NativeStoreLayout,
        store_id: &StoreId,
    ) -> io::Result<()> {
        if store_id != &self.store_id {
            return Err(io::Error::other("audit Store ID changed"));
        }
        CheckoutReader::open(layout.root(), Some(&self.store_directory.identity()))?;
        CheckoutReader::open(
            layout.metadata_dir(),
            Some(&self.metadata_directory.identity()),
        )?;
        CheckoutReader::open(self.checkout.root(), Some(&self.checkout.identity()))?;
        let connection =
            sqlite::open_audit_read_only(layout.sqlite_path(), std::time::Duration::from_secs(5))
                .map_err(io::Error::other)?;
        let current = read_identity(&connection, store_id, &self.checkout.identity())?;
        if current.id() != self.worktree.id()
            || current.space_id() != self.worktree.space_id()
            || current.checkout_root() != self.worktree.checkout_root()
        {
            return Err(io::Error::other(
                "registered checkout identity changed after audit",
            ));
        }
        CheckoutReader::open(current.checkout_root(), Some(current.filesystem_identity()))?;
        Ok(())
    }
}

pub fn audit_worktree_store(root: &Path) -> io::Result<AuditedWorktreeStore> {
    let checkout = CheckoutReader::open(root, None)?;
    let root = checkout.root();
    let metadata = std::fs::symlink_metadata(root.join(".layrs"))?;
    let locator = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        None
    } else {
        Some(worktrees::read_locator(root)?)
    };
    let store_root = locator
        .as_ref()
        .map(|l| l.store_root.clone())
        .unwrap_or_else(|| root.join(".layrs"));
    let store_directory = CheckoutReader::open(
        &store_root,
        locator
            .as_ref()
            .map(|l| l.store_filesystem_identity.as_str()),
    )?;
    let metadata_directory = CheckoutReader::open(
        store_root.join("metadata"),
        locator.as_ref().map(|l| l.store_metadata_identity.as_str()),
    )?;
    let NativeStoreInspection::NativeReady { layout, marker } =
        inspect(&store_root).map_err(io::Error::other)?
    else {
        return Err(io::Error::other(
            "audit locator requires a Ready native Store format",
        ));
    };
    // This is an identity audit, not normal operational admission. In particular
    // metadata-compaction's fence must not prevent explicit operator resumption.
    let lock = StoreLock::acquire_existing(layout.lock_path(), SessionMode::Shared)
        .map_err(io::Error::other)?;
    let NativeStoreInspection::NativeReady {
        layout,
        marker: checked,
    } = inspect(&store_root).map_err(io::Error::other)?
    else {
        return Err(io::Error::other(
            "Store format changed while resolving checkout",
        ));
    };
    if checked.store_id() != marker.store_id()
        || locator
            .as_ref()
            .is_some_and(|l| &l.store_id != checked.store_id())
    {
        return Err(io::Error::other("audit locator Store ID mismatch"));
    }
    CheckoutReader::open(&store_root, Some(&store_directory.identity()))?;
    CheckoutReader::open(
        store_root.join("metadata"),
        Some(&metadata_directory.identity()),
    )?;
    let audit = NativeStoreAudit::new(layout, checked.store_id().clone(), lock);
    let connection = audit.try_connection().map_err(io::Error::other)?;
    let worktree = read_identity(&connection, audit.store_id(), &checkout.identity())?;
    if locator
        .as_ref()
        .is_some_and(|l| &l.worktree_id != worktree.id())
    {
        return Err(io::Error::other("checkout audit locator Worktree mismatch"));
    }
    CheckoutReader::open(
        worktree.checkout_root(),
        Some(worktree.filesystem_identity()),
    )?;
    CheckoutReader::open(root, Some(worktree.filesystem_identity()))?;
    CheckoutReader::open(&store_root, Some(&store_directory.identity()))?;
    CheckoutReader::open(
        store_root.join("metadata"),
        Some(&metadata_directory.identity()),
    )?;
    Ok(AuditedWorktreeStore {
        store_root,
        store_id: audit.store_id().clone(),
        store_directory,
        metadata_directory,
        checkout,
        worktree,
    })
}

fn read_identity(
    connection: &rusqlite::Connection,
    store_id: &StoreId,
    identity: &str,
) -> io::Result<Worktree> {
    sqlite::validate_store_identity(connection, store_id).map_err(io::Error::other)?;
    let mut query = connection.prepare("SELECT worktree_id,store_id,space_id,active_operation_id,checkout_root,filesystem_identity,CASE WHEN length(payload_json)<=32768 THEN payload_json ELSE NULL END FROM worktrees WHERE filesystem_identity=?1 LIMIT 2").map_err(io::Error::other)?;
    let rows = query
        .query_map([identity], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(io::Error::other)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(io::Error::other)?;
    let [row] = rows.as_slice() else {
        return Err(io::Error::other(
            "checkout has no unique registered identity",
        ));
    };
    if row.6.len() > 32 * 1024 {
        return Err(io::Error::other("Worktree identity record exceeds bound"));
    }
    let worktree: Worktree = serde_json::from_str(&row.6).map_err(io::Error::other)?;
    if worktree.id().as_str() != row.0
        || worktree.store_id().as_str() != row.1
        || worktree.space_id().as_str() != row.2
        || worktree.active_operation_id().as_str() != row.3
        || worktree.checkout_root() != row.4
        || worktree.filesystem_identity() != row.5
        || worktree.store_id() != store_id
        || worktree.filesystem_identity() != identity
    {
        return Err(io::Error::other("checkout audit identity binding mismatch"));
    }
    Ok(worktree)
}
