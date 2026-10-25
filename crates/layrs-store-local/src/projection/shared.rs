//! Format-4 content family. This catalog never admits native SCM graph objects.
mod content;
mod maintenance;
mod staging;
#[cfg(windows)]
pub(super) mod provider_access;
use super::*;
use crate::{lock::StoreLock, secure_fs::PinnedDirectory};
#[cfg(test)]
pub(in crate::projection) use content::InstallationPoint;
use layrs_core::scm::{FileManifest, canonical_json};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs,
    io::{Read, Write},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const SQL: &str = r#"
PRAGMA user_version=1;
CREATE TABLE family(singleton INTEGER PRIMARY KEY CHECK(singleton=1),marker BLOB NOT NULL) STRICT;
CREATE TABLE quota(singleton INTEGER PRIMARY KEY CHECK(singleton=1),bytes INTEGER NOT NULL CHECK(bytes>=0)) STRICT;
CREATE TABLE worktrees(id TEXT PRIMARY KEY,metadata_path TEXT NOT NULL UNIQUE,database_identity TEXT NOT NULL,marker BLOB NOT NULL,state TEXT NOT NULL CHECK(state IN ('initializing','ready'))) STRICT;
CREATE TABLE chunks(id TEXT PRIMARY KEY,bytes INTEGER NOT NULL CHECK(bytes>0 AND bytes<=1048576)) STRICT;
CREATE TABLE manifests(id TEXT PRIMARY KEY,file_id TEXT NOT NULL UNIQUE,bytes INTEGER NOT NULL,payload BLOB NOT NULL CHECK(length(payload)<=262144)) STRICT;
CREATE TABLE manifest_chunks(manifest_id TEXT NOT NULL REFERENCES manifests(id),ordinal INTEGER NOT NULL,chunk_id TEXT NOT NULL REFERENCES chunks(id),PRIMARY KEY(manifest_id,ordinal)) STRICT;
CREATE TABLE roots(id TEXT PRIMARY KEY,owner TEXT NOT NULL REFERENCES worktrees(id),database_identity TEXT NOT NULL,operation TEXT NOT NULL,digest TEXT NOT NULL,payload BLOB NOT NULL CHECK(length(payload)<=16777216)) STRICT;
CREATE TABLE root_manifests(root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,manifest_id TEXT NOT NULL REFERENCES manifests(id),PRIMARY KEY(root_id,manifest_id)) STRICT;
CREATE TABLE reservations(id TEXT PRIMARY KEY,owner TEXT NOT NULL REFERENCES worktrees(id),operation TEXT NOT NULL,remaining INTEGER NOT NULL CHECK(remaining>=0)) STRICT;
CREATE TABLE installations(reservation TEXT NOT NULL REFERENCES reservations(id),file_id TEXT NOT NULL,payload BLOB NOT NULL CHECK(length(payload)<=8192),manifest BLOB CHECK(length(manifest)<=262144),published INTEGER NOT NULL CHECK(published IN (0,1)),PRIMARY KEY(reservation,file_id)) STRICT;
CREATE TABLE sweep(kind TEXT NOT NULL CHECK(kind IN ('chunks','manifests')),id TEXT NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>=0),PRIMARY KEY(kind,id)) STRICT;
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartialStoreIdentity {
    pub store_id: StoreId,
    pub space_id: SpaceId,
    pub origin: String,
    pub principal_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedQuota {
    pub bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedUsage {
    pub content_bytes: u64,
    pub reserved_bytes: u64,
    pub limit_bytes: u64,
    pub protection_roots: u64,
    /// Process-interruption safety does not qualify Windows power-loss durability.
    pub windows_power_loss_qualified: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FamilyMarker {
    schema_version: String,
    magic: String,
    format_version: u32,
    min_reader_version: u32,
    incarnation: String,
    identity: PartialStoreIdentity,
    root_identity: String,
    metadata_identity: String,
    database_identity: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub root: PathBuf,
    pub incarnation: String,
    pub root_identity: String,
    pub metadata_path: PathBuf,
    pub database_identity: String,
}
#[derive(Clone)]
pub struct PartialStoreFamily {
    root: PathBuf,
    marker: FamilyMarker,
}
pub(super) struct Lease {
    family: PartialStoreFamily,
    _lock: StoreLock,
}
pub(super) struct Catalog {
    pub connection: Connection,
    pin: super::file::DatabasePin,
    _lock: StoreLock,
}

impl ProjectionStore {
    /// Format capability only; cache membership never grants remote authority.
    pub fn is_shared(&self) -> bool {
        self.marker.shared.is_some()
    }

    /// Return the bound family after validating the checkout, registry, private
    /// database and family identities through the ordinary reader access path.
    pub fn shared_family(&self) -> Result<Option<PartialStoreFamily>> {
        let access = format::access(self, SessionMode::Shared)?;
        let family = access.family.as_ref().map(|lease| lease.family.clone());
        access.verify()?;
        Ok(family)
    }
}

pub(super) fn unique_id() -> Result<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    Ok(format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(invalid)?
            .as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
pub(super) fn digest<T: Serialize>(value: &T) -> Result<String> {
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-partial-retention-v1\0");
    h.update(&canonical_json(value).map_err(invalid)?);
    Ok(h.finalize().to_hex().to_string())
}
fn bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = crate::layout::open_read_no_follow(path)?;
    if file.metadata()?.len() > limit {
        return Err(invalid("shared metadata exceeds bound"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("shared metadata grew beyond bound"));
    }
    Ok(bytes)
}

impl PartialStoreFamily {
    pub fn create(
        root: impl AsRef<Path>,
        identity: PartialStoreIdentity,
        quota: SharedQuota,
    ) -> Result<Self> {
        if quota.bytes > i64::MAX as u64 {
            return Err(invalid("quota exceeds SQLite integer range"));
        }
        format::validate_identity(&ProjectionIdentity {
            store_id: identity.store_id.clone(),
            worktree_id: WorktreeId::parse("worktree:validation").map_err(invalid)?,
            space_id: identity.space_id.clone(),
            origin: identity.origin.clone(),
            principal_id: identity.principal_id.clone(),
        })?;
        let root = std::path::absolute(root)?;
        let directory = PinnedDirectory::open(&root)?;
        if fs::read_dir(&root)?.next().is_some() {
            return Err(invalid("shared Store requires an empty directory"));
        }
        let metadata = directory.ensure_private_child("metadata".as_ref())?;
        for name in ["lifecycle.lock", "catalog.lock", "catalog.sqlite3"] {
            metadata.create_file(name.as_ref())?.file().sync_all()?;
        }
        let objects = directory.ensure_private_child("objects".as_ref())?;
        objects.ensure_private_child("chunks".as_ref())?;
        objects.ensure_private_child("manifests".as_ref())?;
        directory.ensure_private_child("staging".as_ref())?;
        directory.ensure_private_child("worktrees".as_ref())?;
        let marker = FamilyMarker {
            schema_version: "layrs.partial-store-family.v1".into(),
            magic: "LAYRS_NATIVE_STORE".into(),
            format_version: 4,
            min_reader_version: 4,
            incarnation: unique_id()?,
            identity,
            root_identity: directory.identity_token(),
            metadata_identity: metadata.identity_token(),
            database_identity: super::file::DatabasePin::open(
                &root.join("metadata/catalog.sqlite3"),
            )?
            .identity()
            .to_owned(),
        };
        let bytes = canonical_json(&marker).map_err(invalid)?;
        let mut c = database::connect(&root.join("metadata/catalog.sqlite3"), true)?;
        c.execute_batch("PRAGMA journal_mode=DELETE;")?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(SQL)?;
        tx.execute("INSERT INTO family VALUES(1,?1)", [&bytes])?;
        tx.execute("INSERT INTO quota VALUES(1,?1)", [quota.bytes])?;
        tx.commit()?;
        drop(c);
        metadata.sync()?;
        let mut marker_file = directory.create_file("FORMAT".as_ref())?;
        marker_file.file_mut().write_all(&bytes)?;
        marker_file.file().sync_all()?;
        directory.sync()?;
        Self::open(root)
    }
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(root.as_ref(), true)
    }
    fn open_inner(root: &Path, recover: bool) -> Result<Self> {
        let family = Self::read_bound(root)?;
        let lease = family.lease(SessionMode::Shared)?;
        drop(lease.inspection_catalog(recover)?);
        Ok(family)
    }
    /// Filesystem identities only; does not touch SQLite, even for hot journals.
    fn read_bound(root: &Path) -> Result<Self> {
        let root = std::path::absolute(root)?;
        let bytes = bounded_file(&root.join("FORMAT"), 65536)?;
        let marker: FamilyMarker = serde_json::from_slice(&bytes).map_err(invalid)?;
        if marker.schema_version != "layrs.partial-store-family.v1"
            || marker.magic != "LAYRS_NATIVE_STORE"
            || marker.format_version != 4
            || marker.min_reader_version != 4
            || canonical_json(&marker).map_err(invalid)? != bytes
        {
            return Err(invalid("unsupported shared PARTIAL Store format"));
        }
        let family = Self { root, marker };
        family.verify()?;
        let pin = super::file::DatabasePin::open(&family.root.join("metadata/catalog.sqlite3"))?;
        if pin.identity() != family.marker.database_identity {
            return Err(invalid("shared catalog replaced"));
        }
        Ok(family)
    }
    pub fn identity(&self) -> &PartialStoreIdentity {
        &self.marker.identity
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn create_worktree(
        &self,
        root: impl AsRef<Path>,
        worktree_id: WorktreeId,
        initial: ProjectionSnapshot,
        selection: layrs_core::projection::ProjectionSelection,
    ) -> Result<ProjectionStore> {
        format::create_shared(root.as_ref(), self, worktree_id, initial, &selection)
    }
    pub fn usage(&self) -> Result<SharedUsage> {
        let lease = self.lease(SessionMode::Shared)?;
        usage(&lease.catalog(false)?.connection)
    }
    /// Fault hooks bracket durable registry/bootstrap and checkout marker publication.
    pub fn create_worktree_with_faults(
        &self,
        root: impl AsRef<Path>,
        worktree_id: WorktreeId,
        initial: ProjectionSnapshot,
        selection: layrs_core::projection::ProjectionSelection,
        faults: &dyn CreationFaults,
    ) -> Result<ProjectionStore> {
        format::create_shared_with_faults(
            root.as_ref(),
            self,
            worktree_id,
            initial,
            &selection,
            faults,
        )
    }
    /// Resume only a marker-bound bootstrap without accepted Worktree history.
    pub fn resume_worktree(
        &self,
        root: impl AsRef<Path>,
        initial: &ProjectionSnapshot,
        selection: &layrs_core::projection::ProjectionSelection,
    ) -> Result<ProjectionStore> {
        format::resume_shared(root.as_ref(), self, initial, selection)
    }
    pub fn set_quota(&self, quota: SharedQuota) -> Result<()> {
        if quota.bytes > i64::MAX as u64 {
            return Err(invalid("quota out of range"));
        }
        let lease = self.lease(SessionMode::Shared)?;
        lease
            .catalog(true)?
            .connection
            .execute("UPDATE quota SET bytes=?1 WHERE singleton=1", [quota.bytes])?;
        Ok(())
    }
    pub(super) fn lease(&self, mode: SessionMode) -> Result<Lease> {
        let lock = StoreLock::acquire_existing(&self.root.join("metadata/lifecycle.lock"), mode)
            .map_err(invalid)?;
        self.verify()?;
        Ok(Lease {
            family: self.clone(),
            _lock: lock,
        })
    }
    fn verify(&self) -> Result<()> {
        if PinnedDirectory::open(&self.root)?.identity_token() != self.marker.root_identity
            || PinnedDirectory::open(&self.root.join("metadata"))?.identity_token()
                != self.marker.metadata_identity
            || bounded_file(&self.root.join("FORMAT"), 65536)?
                != canonical_json(&self.marker).map_err(invalid)?
        {
            return Err(invalid("shared Store identity changed"));
        }
        Ok(())
    }
    pub(super) fn binding(&self, worktree: &WorktreeId, database_identity: String) -> Binding {
        Binding {
            root: self.root.clone(),
            incarnation: self.marker.incarnation.clone(),
            root_identity: self.marker.root_identity.clone(),
            metadata_path: self.member_metadata(worktree),
            database_identity,
        }
    }
    pub(super) fn member_metadata(&self, id: &WorktreeId) -> PathBuf {
        // IDs are not filesystem components (':' is invalid on Windows).
        let key = blake3::hash(id.as_str().as_bytes()).to_hex().to_string();
        self.root.join("worktrees").join(key).join("metadata")
    }
}
pub(super) fn for_store(store: &ProjectionStore) -> Result<Option<Lease>> {
    for_store_inner(store, true)
}
fn for_store_inner(store: &ProjectionStore, recover: bool) -> Result<Option<Lease>> {
    let lease = for_store_discovery(store)?;
    if let Some(lease) = &lease {
        drop(lease.inspection_catalog(recover)?);
    }
    Ok(lease)
}
pub(super) fn for_store_discovery(store: &ProjectionStore) -> Result<Option<Lease>> {
    let Some(binding) = &store.marker.shared else {
        return Ok(None);
    };
    let family = PartialStoreFamily::read_bound(&binding.root)?;
    let id = family.identity();
    if id.store_id != store.identity().store_id
        || id.space_id != store.identity().space_id
        || id.origin != store.identity().origin
        || id.principal_id != store.identity().principal_id
        || family.marker.incarnation != binding.incarnation
        || family.marker.root_identity != binding.root_identity
        || family.member_metadata(&store.identity().worktree_id) != binding.metadata_path
    {
        return Err(invalid("shared Worktree binding mismatch"));
    }
    Ok(Some(family.lease(SessionMode::Shared)?))
}
impl Lease {
    pub(super) fn recovery_marker_digest(&self) -> Result<String> {
        self.verify()?;
        let mut digest = blake3::Hasher::new();
        digest.update(b"layrs-projection-recovery-family-v1\0");
        digest.update(&canonical_json(&self.family.marker).map_err(invalid)?);
        Ok(format!("blake3:{}", digest.finalize()))
    }
    pub(super) fn inspection_catalog(&self, recover: bool) -> Result<Catalog> {
        let catalog = if recover {
            self.catalog(false)?
        } else {
            self.catalog_once(false)?
        };
        let reference = Connection::open_in_memory()?;
        reference.execute_batch(SQL)?;
        if database::schema(&catalog.connection)? != database::schema(&reference)?
            || catalog
                .connection
                .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?
                != 1
        {
            return Err(invalid("shared content schema mismatch"));
        }
        Ok(catalog)
    }
    pub(in crate::projection) fn family_root(&self) -> &Path {
        &self.family.root
    }
    pub(super) fn usage(&self) -> Result<SharedUsage> {
        usage(&self.catalog(false)?.connection)
    }
    pub(super) fn catalog(&self, write: bool) -> Result<Catalog> {
        match self.catalog_once(write) {
            Err(error) if !write && database::readonly_rollback(&error) => {
                // catalog_once dropped its read connection and shared lock on
                // error. Revalidate paths/inodes before opening a write handle.
                let recovered = self.catalog_once(true)?;
                let reference = Connection::open_in_memory()?;
                reference.execute_batch(SQL)?;
                if database::schema(&recovered.connection)? != database::schema(&reference)?
                    || recovered
                        .connection
                        .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?
                        != 1
                {
                    return Err(invalid("recovered shared catalog schema mismatch"));
                }
                recovered
                    .pin
                    .verify(&self.family.root.join("metadata/catalog.sqlite3"))?;
                self.family.verify()?;
                drop(recovered);
                self.catalog_once(false)
            }
            result => result,
        }
    }
    pub(super) fn catalog_once(&self, write: bool) -> Result<Catalog> {
        let mode = if write {
            SessionMode::Exclusive
        } else {
            SessionMode::Shared
        };
        let lock =
            StoreLock::acquire_existing(&self.family.root.join("metadata/catalog.lock"), mode)
                .map_err(invalid)?;
        self.family.verify()?;
        let path = self.family.root.join("metadata/catalog.sqlite3");
        let pin = super::file::DatabasePin::open(&path)?;
        if pin.identity() != self.family.marker.database_identity {
            return Err(invalid("shared catalog replaced"));
        }
        let c = database::connect(&path, write)?;
        let bytes: Vec<u8> = c.query_row(
            "SELECT marker FROM family WHERE singleton=1 AND length(marker)<=65536",
            [],
            |r| r.get(0),
        )?;
        if bytes != canonical_json(&self.family.marker).map_err(invalid)? {
            return Err(invalid("shared catalog marker mismatch"));
        }
        Ok(Catalog {
            connection: c,
            pin,
            _lock: lock,
        })
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.family.verify()
    }
    pub(super) fn register(&self, marker: &format::Marker, ready: bool) -> Result<()> {
        let binding = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("missing family binding"))?;
        let catalog = self.catalog(true)?;
        let payload = marker.bytes()?;
        catalog.connection.execute("INSERT INTO worktrees(id,metadata_path,database_identity,marker,state) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO NOTHING", params![marker.identity.worktree_id.as_str(), binding.metadata_path.to_string_lossy(), binding.database_identity, payload, if ready { "ready" } else { "initializing" }])?;
        let (path, identity, old): (String, String, Vec<u8>) = catalog.connection.query_row(
            "SELECT metadata_path,database_identity,marker FROM worktrees WHERE id=?1",
            [marker.identity.worktree_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if path != binding.metadata_path.to_string_lossy()
            || identity != binding.database_identity
            || old != payload
        {
            return Err(invalid("Worktree registration collision"));
        }
        if ready {
            catalog.connection.execute(
                "UPDATE worktrees SET state='ready' WHERE id=?1",
                [marker.identity.worktree_id.as_str()],
            )?;
        }
        Ok(())
    }
    pub(super) fn registered_state(&self, marker: &format::Marker) -> Result<String> {
        let catalog = self.catalog(false)?;
        let (payload, state): (Vec<u8>, String) = catalog.connection.query_row(
            "SELECT marker,state FROM worktrees WHERE id=?1 AND length(marker)<=65536",
            [marker.identity.worktree_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if payload != marker.bytes()? {
            return Err(invalid("Worktree registration differs from locator"));
        }
        Ok(state)
    }
    pub(super) fn reserve(
        &self,
        owner: &WorktreeId,
        operation: &str,
        bytes: u64,
    ) -> Result<String> {
        let catalog = self.catalog(true)?;
        let current = usage(&catalog.connection)?;
        if current
            .content_bytes
            .checked_add(current.reserved_bytes)
            .and_then(|n| n.checked_add(bytes))
            .is_none_or(|n| n > current.limit_bytes)
            || fs2::available_space(&self.family.root)?
                < current.reserved_bytes.saturating_add(bytes)
        {
            return Err(invalid(
                "partial_store_quota: insufficient unreserved capacity",
            ));
        }
        let id = unique_id()?;
        catalog.connection.execute(
            "INSERT INTO reservations VALUES(?1,?2,?3,?4)",
            params![id, owner.as_str(), operation, bytes],
        )?;
        Ok(id)
    }
    pub(in crate::projection) fn ensure_reservation_capacity(
        &self,
        id: &str,
        owner: &WorktreeId,
        minimum: u64,
    ) -> Result<()> {
        let catalog = self.catalog(true)?;
        let remaining: u64 = catalog.connection.query_row(
            "SELECT remaining FROM reservations WHERE id=?1 AND owner=?2",
            params![id, owner.as_str()],
            |r| r.get(0),
        )?;
        self.grow_reservation(&catalog.connection, id, remaining, minimum)
    }
    fn grow_reservation(
        &self,
        c: &Connection,
        id: &str,
        remaining: u64,
        minimum: u64,
    ) -> Result<()> {
        let delta = minimum.saturating_sub(remaining);
        if delta == 0 {
            return Ok(());
        }
        let used = usage(c)?;
        if used
            .content_bytes
            .checked_add(used.reserved_bytes)
            .and_then(|n| n.checked_add(delta))
            .is_none_or(|n| n > used.limit_bytes)
            || fs2::available_space(&self.family.root)? < used.reserved_bytes.saturating_add(delta)
        {
            return Err(invalid(
                "partial_store_quota: acquisition requires additional reserved capacity; resume after increasing capacity or release preparation",
            ));
        }
        c.execute(
            "UPDATE reservations SET remaining=remaining+?1 WHERE id=?2",
            params![delta, id],
        )?;
        Ok(())
    }
    pub(super) fn release_reservation(&self, id: &str, owner: &WorktreeId) -> Result<()> {
        let catalog = self.catalog(true)?;
        if catalog.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM installations WHERE reservation=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid(
                "shared installation remains pending; reservation retained",
            ));
        }
        catalog.connection.execute(
            "DELETE FROM reservations WHERE id=?1 AND owner=?2",
            params![id, owner.as_str()],
        )?;
        Ok(())
    }
}
fn usage(c: &Connection) -> Result<SharedUsage> {
    let content_bytes = c.query_row("SELECT COALESCE((SELECT SUM(bytes) FROM chunks),0)+COALESCE((SELECT SUM(length(payload)) FROM manifests),0)+COALESCE((SELECT SUM(bytes) FROM sweep),0)", [], |r| r.get(0))?;
    Ok(SharedUsage {
        content_bytes,
        reserved_bytes: c.query_row(
            "SELECT COALESCE(SUM(remaining),0) FROM reservations",
            [],
            |r| r.get(0),
        )?,
        limit_bytes: c.query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
            r.get(0)
        })?,
        protection_roots: c.query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0))?,
        windows_power_loss_qualified: false,
    })
}
