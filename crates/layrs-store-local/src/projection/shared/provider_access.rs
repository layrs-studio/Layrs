//! Filesystem-only family binding and lock-held provider catalog validation.
//! Never call the ordinary catalog/open/lease helpers from this route.
use super::*;
use crate::projection::provider::{
    database_io::{self, DatabaseFile},
    records::AllocationRecord,
    recovery::error::{Result as ProviderResult, invalid as provider_invalid},
};
use std::{ffi::OsStr, fs::File};

pub(in crate::projection) struct FamilyAccess {
    family: PartialStoreFamily,
    root_pin: PinnedDirectory,
    metadata_pin: PinnedDirectory,
    marker_pin: File,
    marker_bytes: Vec<u8>,
    pub(in crate::projection) catalog: DatabaseFile,
}
pub(in crate::projection) struct OperationLocks {
    _catalog: StoreLock,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum Registration {
    Absent,
    Initializing,
}

impl FamilyAccess {
    pub(in crate::projection) fn open(root: &Path) -> ProviderResult<Self> {
        let family = PartialStoreFamily::read_bound(root)?; // filesystem only
        let root_pin = PinnedDirectory::open(&family.root)?;
        let metadata_pin = root_pin.open_child_for_traversal(OsStr::new("metadata"))?;
        let marker_pin = crate::layout::open_read_no_follow(&family.root.join("FORMAT"))?;
        let marker_bytes =
            canonical_json(&family.marker).map_err(|e| provider_invalid(e.to_string()))?;
        let catalog = DatabaseFile::open(
            family.root.join("metadata/catalog.sqlite3"),
            &family.marker.database_identity,
        )?;
        let bound = Self {
            family,
            root_pin,
            metadata_pin,
            marker_pin,
            marker_bytes,
            catalog,
        };
        bound.verify()?;
        Ok(bound)
    }
    pub(in crate::projection) fn root(&self) -> &Path {
        &self.family.root
    }
    pub(in crate::projection) fn member(&self, worktree: &WorktreeId) -> PathBuf {
        self.family.member_metadata(worktree)
    }
    pub(in crate::projection) fn verify(&self) -> ProviderResult<()> {
        self.root_pin.verify_path_identity()?;
        self.metadata_pin.verify_path_identity()?;
        self.family.verify()?;
        self.catalog.verify()?;
        let file = crate::layout::open_read_no_follow(&self.root().join("FORMAT"))?;
        if super::super::file::identity(&file)? != super::super::file::identity(&self.marker_pin)?
            || bounded_file(&self.root().join("FORMAT"), 65536)? != self.marker_bytes
        {
            return Err(provider_invalid("provider family FORMAT changed"));
        }
        Ok(())
    }
    pub(in crate::projection) fn locks(
        &self,
        worktree: &WorktreeId,
        exclusive: bool,
    ) -> ProviderResult<OperationLocks> {
        let lifecycle = StoreLock::acquire_existing(
            &self.root().join("metadata/lifecycle.lock"),
            SessionMode::Shared,
        )?;
        self.verify()?;
        let mode = if exclusive {
            SessionMode::Exclusive
        } else {
            SessionMode::Shared
        };
        let owner = StoreLock::acquire_existing(&self.member(worktree).join("layrs.lock"), mode)?;
        // Take EXCLUSIVE directly for recovery; never upgrade or reacquire it.
        let catalog =
            StoreLock::acquire_existing(&self.root().join("metadata/catalog.lock"), mode)?;
        self.verify()?;
        Ok(OperationLocks {
            _catalog: catalog,
            _owner: owner,
            _lifecycle: lifecycle,
        })
    }
    pub(in crate::projection) fn require_allocation(
        &self,
        a: &AllocationRecord,
    ) -> ProviderResult<()> {
        self.verify()?;
        let record = &a.family;
        let canonical = canonical_path(self.root())?;
        if record.root != exact_path(self.root())?
            || record.canonical_root != canonical
            || record.incarnation != self.family.marker.incarnation
            || record.root_identity != self.root_pin.identity_token()
            || record.metadata_identity != self.metadata_pin.identity_token()
            || record.catalog_file_identity != self.family.marker.database_identity
            || record.marker_file_identity != super::super::file::identity(&self.marker_pin)?
            || record.marker_digest != self.marker_digest()
            || self.family.marker.identity.store_id != a.format.identity.store_id
            || self.family.marker.identity.space_id != a.format.identity.space_id
            || self.family.marker.identity.origin != a.format.identity.origin
            || self.family.marker.identity.principal_id != a.format.identity.principal_id
        {
            return Err(provider_invalid(
                "allocation differs from pinned family binding",
            ));
        }
        Ok(())
    }
    pub(in crate::projection) fn marker_digest(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"layrs-projection-recovery-family-v1\0");
        h.update(&self.marker_bytes);
        format!("blake3:{}", h.finalize())
    }
    /// Caller owns the one catalog gate and a read transaction. No lock/open here.
    pub(in crate::projection) fn validate_catalog(
        &self,
        c: &Connection,
        a: &AllocationRecord,
    ) -> ProviderResult<Registration> {
        let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != 1 {
            return Err(provider_invalid(
                "provider family requires exact catalog schema1",
            ));
        }
        let observed = database_io::schema_inventory(c)?;
        database_io::with_reference(SQL, |reference| {
            if database_io::schema_inventory(reference)? != observed {
                return Err(provider_invalid(
                    "provider family catalog fingerprint differs",
                ));
            }
            Ok(())
        })?;
        let (count, length): (u64, u64) = c.query_row(
            "SELECT count(*),coalesce(max(length(marker)),0) FROM family",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count != 1 || length > 65536 {
            return Err(provider_invalid(
                "provider family marker cardinality/bound differs",
            ));
        }
        let marker: Vec<u8> =
            c.query_row("SELECT marker FROM family WHERE singleton=1", [], |r| {
                r.get(0)
            })?;
        if marker != self.marker_bytes {
            return Err(provider_invalid("provider family SQL identity differs"));
        }
        let quota_count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM quota LIMIT 2)",
            [],
            |r| r.get(0),
        )?;
        let quota: u64 = c.query_row(
            "SELECT count(*) FROM quota WHERE singleton=1 AND bytes>=0",
            [],
            |r| r.get(0),
        )?;
        if quota != 1 || quota_count != 1 {
            return Err(provider_invalid("provider family quota singleton differs"));
        }
        let owner = a.format.identity.worktree_id.as_str();
        let oversized: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND (length(CAST(metadata_path AS BLOB))>4096 OR length(CAST(database_identity AS BLOB))>512 OR length(CAST(state AS BLOB))>12))", [owner], |r| r.get(0))?;
        if oversized {
            return Err(provider_invalid(
                "provider target registry text exceeds bound",
            ));
        }
        // Check payload bounds before materializing a potentially corrupt target.
        let target_length: Option<u64> = c
            .query_row(
                "SELECT length(marker) FROM worktrees WHERE id=?1",
                [owner],
                |r| r.get(0),
            )
            .optional()?;
        if target_length.is_some_and(|length| length > 65536) {
            return Err(provider_invalid(
                "provider target registry marker exceeds bound",
            ));
        }
        let target: Option<(String, String, Vec<u8>, String)> = c
            .query_row(
                "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
                [owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let registration = match target {
            None => Registration::Absent,
            Some((path, id, marker, state))
                if path == a.private.metadata_path
                    && id == a.private.database_file_identity
                    && marker == a.format.bytes()?
                    && state == "initializing" =>
            {
                Registration::Initializing
            }
            Some(_) => {
                return Err(provider_invalid(
                    "provider target registration is not exact initializing ownership",
                ));
            }
        };
        for sql in [
            "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
            "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1)",
            "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
        ] {
            let present: bool = c.query_row(sql, [owner], |r| r.get(0))?;
            if present {
                return Err(provider_invalid(
                    "provider bootstrap owner has accepted roots/reservations/installations",
                ));
            }
        }
        let failure: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |r| r.get(0),
        )?;
        if failure {
            return Err(provider_invalid("provider family foreign-key corruption"));
        }
        // Other registered members and their protected content remain valid and
        // untouched; this is not GC or a demand that the whole family be empty.
        Ok(registration)
    }
}

pub(in crate::projection) fn exact_path(path: &Path) -> ProviderResult<&str> {
    let value = path
        .to_str()
        .ok_or_else(|| provider_invalid("non-Unicode provider locator"))?;
    if value.is_empty() || value.len() > 4096 || !path.is_absolute() {
        return Err(provider_invalid(
            "provider locator bound/absolute requirement",
        ));
    }
    Ok(value)
}
pub(in crate::projection) fn canonical_path(path: &Path) -> ProviderResult<String> {
    let canonical = std::fs::canonicalize(path)?;
    let value = exact_path(&canonical)?;
    let value = value.strip_prefix("\\\\?\\").ok_or_else(|| {
        provider_invalid("canonical provider locator is not an absolute native DOS path")
    })?;
    if value.starts_with("UNC\\") {
        return Err(provider_invalid(
            "provider bootstrap requires a local drive",
        ));
    }
    Ok(value.to_owned())
}

// Writer-only gates are separate: recovery keeps its existing bundled shared
// lifecycle behavior. Caller holds lifecycle -> owner before taking a catalog gate.
impl FamilyAccess {
    pub(in crate::projection) fn writer_lifecycle(
        &self,
        exclusive: bool,
    ) -> ProviderResult<StoreLock> {
        let lock = StoreLock::acquire_existing(
            &self.root().join("metadata/lifecycle.lock"),
            if exclusive {
                SessionMode::Exclusive
            } else {
                SessionMode::Shared
            },
        )?;
        self.verify()?;
        Ok(lock)
    }
    pub(in crate::projection) fn writer_owner(
        &self,
        worktree: &WorktreeId,
    ) -> ProviderResult<StoreLock> {
        let lock = StoreLock::acquire_existing(
            &self.member(worktree).join("layrs.lock"),
            SessionMode::Exclusive,
        )?;
        self.verify()?;
        Ok(lock)
    }
    pub(in crate::projection) fn writer_catalog(&self) -> ProviderResult<StoreLock> {
        let lock = StoreLock::acquire_existing(
            &self.root().join("metadata/catalog.lock"),
            SessionMode::Exclusive,
        )?;
        self.verify()?;
        Ok(lock)
    }
    pub(in crate::projection) fn writer_worktrees(&self) -> ProviderResult<PinnedDirectory> {
        self.verify()?;
        Ok(self
            .root_pin
            .open_private_child_for_write(OsStr::new("worktrees"))?)
    }
    pub(in crate::projection) fn writer_identity(
        &self,
        worktree: &WorktreeId,
    ) -> ProjectionIdentity {
        let i = &self.family.marker.identity;
        ProjectionIdentity {
            store_id: i.store_id.clone(),
            space_id: i.space_id.clone(),
            worktree_id: worktree.clone(),
            origin: i.origin.clone(),
            principal_id: i.principal_id.clone(),
        }
    }
    pub(in crate::projection) fn writer_record(
        &self,
    ) -> ProviderResult<crate::projection::provider::records::FamilyRecord> {
        self.verify()?;
        Ok(crate::projection::provider::records::FamilyRecord {
            root: exact_path(self.root())?.into(),
            canonical_root: canonical_path(self.root())?,
            incarnation: self.family.marker.incarnation.clone(),
            root_identity: self.root_pin.identity_token(),
            marker_file_identity: super::super::file::identity(&self.marker_pin)?,
            marker_digest: self.marker_digest(),
            metadata_identity: self.metadata_pin.identity_token(),
            catalog_file_identity: self.family.marker.database_identity.clone(),
        })
    }
}

impl FamilyAccess {
    pub(in crate::projection) fn writer_matches(
        &self,
        expected: &PartialStoreFamily,
    ) -> ProviderResult<()> {
        self.verify()?;
        if self.family.root.as_os_str() != expected.root.as_os_str()
            || self.family.marker != expected.marker
        {
            return Err(provider_invalid(
                "provider family handle binding changed before allocation",
            ));
        }
        Ok(())
    }
}
