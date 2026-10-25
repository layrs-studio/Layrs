use super::*;
mod family;
mod recovery;
use crate::{
    layout::{create_new_file_no_follow, open_read_no_follow, sync_directory},
    lock::StoreLock,
    secure_fs::PinnedDirectory,
};
pub(super) use family::{
    create as create_shared, create_import as create_shared_import,
    create_with_faults as create_shared_with_faults, resume as resume_shared,
};
pub(super) use recovery::access as recovery_access;
pub use recovery::{
    ProjectionInspection, ProjectionRecovery, ProjectionRecoveryReason, RecoveryBinding,
};
use std::{
    fs,
    io::{Read, Write},
};
const DATABASE: &str = "projection.sqlite3";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Marker {
    schema_version: String,
    magic: String,
    format_version: u32,
    min_reader_version: u32,
    pub identity: ProjectionIdentity,
    pub(super) checkout_root: String,
    checkout_identity: String,
    store_identity: String,
    pub(super) metadata_identity: String,
    pub initial_snapshot_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initial_selection_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) shared: Option<shared::Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) import_digest: Option<String>,
}
impl Marker {
    pub(super) fn checkout_identity(&self) -> &str {
        &self.checkout_identity
    }
    pub(super) fn metadata_path(&self, root: &Path) -> PathBuf {
        self.shared
            .as_ref()
            .map(|b| b.metadata_path.clone())
            .unwrap_or_else(|| root.join(".layrs/metadata"))
    }
    pub(super) fn validate_initial_selection(
        &self,
        selection: &layrs_core::projection::ProjectionSelection,
    ) -> Result<()> {
        let digest = if matches!(selection, layrs_core::projection::ProjectionSelection::All) {
            None
        } else {
            Some(selection::initial_digest(selection)?)
        };
        if self.initial_selection_digest != digest {
            return Err(invalid("initial selection differs from FORMAT"));
        }
        Ok(())
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        layrs_core::scm::canonical_json(self).map_err(invalid)
    }
    pub(super) fn validate(&self) -> Result<()> {
        if self.schema_version != "layrs.projection-store-format.v1"
            || self.magic != "LAYRS_NATIVE_STORE"
            || !matches!(
                (
                    self.format_version,
                    self.min_reader_version,
                    self.shared.is_some()
                ),
                (3, 3, false) | (4, 4, true)
            )
            || (self.import_digest.is_some() && self.shared.is_none())
        {
            return Err(invalid("unsupported projection FORMAT"));
        }
        validate_identity(&self.identity)
    }
}
pub(super) fn validate_identity(i: &ProjectionIdentity) -> Result<()> {
    let authority = i
        .origin
        .strip_prefix("https://")
        .or_else(|| i.origin.strip_prefix("http://127.0.0.1:"))
        .ok_or_else(|| invalid("projection origin must be HTTPS or explicit loopback HTTP"))?;
    if authority.is_empty()
        || authority.contains(['@', '/', '?', '#'])
        || i.origin.len() > 4096
        || i.origin
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid(
            "projection origin must be an exact origin without credentials/path",
        ));
    }
    if i.principal_id.is_empty()
        || i.principal_id.len() > 256
        || i.principal_id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid("invalid projection Principal identity"));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationPoint {
    MarkerDurable,
    DatabaseDurable,
}
pub trait CreationFaults {
    fn check(&self, point: CreationPoint) -> io::Result<()>;
}
pub struct NoCreationFaults;
impl CreationFaults for NoCreationFaults {
    fn check(&self, _: CreationPoint) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn create(
    root: &Path,
    identity: ProjectionIdentity,
    initial: ProjectionSnapshot,
    faults: &dyn CreationFaults,
) -> Result<ProjectionStore> {
    create_selected(
        root,
        identity,
        initial,
        &layrs_core::projection::ProjectionSelection::All,
        faults,
    )
}
pub(super) fn create_selected(
    root: &Path,
    identity: ProjectionIdentity,
    initial: ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
    faults: &dyn CreationFaults,
) -> Result<ProjectionStore> {
    validate_identity(&identity)?;
    initial.canonical_bytes()?;
    selection.validate_snapshot(&initial)?;
    if initial
        .entries
        .iter()
        .any(|e| e.state == layrs_core::projection::EntryState::Materialized)
    {
        return Err(invalid(
            "initial projection cannot claim materialized files before physical installation",
        ));
    }
    validate_platform(&initial)?;
    require_fresh_control(root)?;
    let checkout = crate::scm_v1::checkout::CheckoutReader::open(root, None)?;
    let root = checkout.root().to_owned();
    let store_path = root.join(".layrs");
    // create_dir is exclusive: no existing metadata, including an empty directory,
    // is adopted. All subsequent failure remnants are retained for explicit resume.
    fs::create_dir(&store_path)?;
    sync_directory(&root)?;
    let store = PinnedDirectory::open(&store_path)?;
    let metadata = store.ensure_child("metadata".as_ref())?;
    let lock = metadata.create_file("layrs.lock".as_ref())?;
    lock.file().sync_all()?;
    metadata.sync()?;
    let _lock = StoreLock::acquire_existing(
        &store_path.join("metadata/layrs.lock"),
        SessionMode::Exclusive,
    )
    .map_err(invalid)?;
    let marker = Marker {
        schema_version: "layrs.projection-store-format.v1".into(),
        magic: "LAYRS_NATIVE_STORE".into(),
        format_version: 3,
        min_reader_version: 3,
        shared: None,
        import_digest: None,
        identity,
        checkout_root: root
            .to_str()
            .ok_or_else(|| invalid("non-UTF8 checkout root"))?
            .into(),
        checkout_identity: checkout.identity(),
        store_identity: store.identity_token(),
        metadata_identity: metadata.identity_token(),
        initial_snapshot_digest: database::digest(0, None, &initial.canonical_bytes()?),
        initial_selection_digest: if matches!(
            selection,
            layrs_core::projection::ProjectionSelection::All
        ) {
            None
        } else {
            Some(selection::initial_digest(selection)?)
        },
    };
    let mut file = store.create_file("FORMAT".as_ref())?;
    file.file_mut().write_all(&marker.bytes()?)?;
    file.file().sync_all()?;
    store.sync()?;
    faults.check(CreationPoint::MarkerDurable)?;
    let db = metadata.create_file(DATABASE.as_ref())?;
    db.file().sync_all()?;
    drop(db);
    metadata.sync()?;
    let mut connection = database::connect(&store_path.join("metadata").join(DATABASE), true)?;
    database::initialize(&mut connection, &marker, &initial, selection)?;
    drop(connection);
    crate::layout::open_read_write_no_follow(&store_path.join("metadata").join(DATABASE))?
        .sync_all()?;
    metadata.sync()?;
    faults.check(CreationPoint::DatabaseDurable)?;
    store.verify_path_identity()?;
    metadata.verify_path_identity()?;
    drop(_lock);
    open(&root)
}
fn validate_platform(snapshot: &ProjectionSnapshot) -> Result<()> {
    if cfg!(windows)
        && snapshot.path_semantics != layrs_core::projection::PathSemantics::PortableWindows
    {
        return Err(invalid(
            "Windows projection requires portable Windows path semantics",
        ));
    }
    Ok(())
}
pub(super) fn read_marker(root: &Path) -> Result<Marker> {
    let file = open_read_no_follow(&root.join(".layrs/FORMAT"))?;
    if file.metadata()?.len() > 64 * 1024 {
        return Err(invalid("projection FORMAT too large"));
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(invalid("projection FORMAT grew beyond limit"));
    }
    parse_marker_bytes(&bytes)
}

/// Parse bounded canonical FORMAT data only. This neither observes nor binds a
/// filesystem object and grants no database, backend, or readiness authority.
pub(super) fn parse_marker_bytes(bytes: &[u8]) -> Result<Marker> {
    if bytes.len() > 64 * 1024 {
        return Err(invalid("projection FORMAT too large"));
    }
    let marker: Marker = serde_json::from_slice(bytes).map_err(invalid)?;
    marker.validate()?;
    if marker.bytes()? != bytes {
        return Err(invalid("projection FORMAT is not canonical"));
    }
    Ok(marker)
}
pub(super) fn open(root: &Path) -> Result<ProjectionStore> {
    let root = std::path::absolute(root)?;
    let marker = read_marker(&root)?;
    let result = ProjectionStore { root, marker };
    // Upgrade the exact legacy schema under the store's exclusive lock before
    // any shared reader validates the new schema.
    drop(access(&result, SessionMode::Exclusive)?);
    let current = result.state()?;
    validate_platform(&current.snapshot)?;
    Ok(result)
}
pub(super) fn open_for_recovery(root: &Path) -> Result<ProjectionStore> {
    let root = std::path::absolute(root)?;
    let marker = read_marker(&root)?;
    let result = ProjectionStore { root, marker };
    // Discovery must not migrate, settle claims, or replay a SQLite/filesystem
    // journal. In particular a hot rollback journal remains an explicit error.
    let family = shared::for_store_discovery(&result)?;
    let path = result.marker.metadata_path(&result.root);
    let _lock = StoreLock::acquire_existing(&path.join("layrs.lock"), SessionMode::Shared)
        .map_err(invalid)?;
    let metadata = PinnedDirectory::open(&path)?;
    let pin = super::file::DatabasePin::open(&path.join(DATABASE))?;
    let boundaries = preliminary_pins(&result.root, &result.marker, &metadata, &pin)?;
    if read_marker(&result.root)? != result.marker {
        return Err(invalid("projection identity changed during inspection"));
    }
    if let Some(binding) = &result.marker.shared {
        if pin.identity() != binding.database_identity {
            return Err(invalid("shared Worktree database replaced"));
        }
        let catalog = family
            .as_ref()
            .ok_or_else(|| invalid("missing family lease"))?
            .catalog_once(false)?;
        let (payload, state): (Vec<u8>, String) = catalog.connection.query_row(
            "SELECT marker,state FROM worktrees WHERE id=?1 AND length(marker)<=65536",
            [result.identity().worktree_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if payload != result.marker.bytes()? || state != "ready" {
            return Err(invalid("Worktree registration differs or is not ready"));
        }
    }
    let c = database::connect(&path.join(DATABASE), false)?;
    database::validate(&c, &result.marker)?;
    boundaries.verify(&result.root, &result.marker)?;
    verify_boundaries(&result.root, &result.marker, &metadata, &pin)?;
    if result.marker.import_digest.is_some() {
        if !c.query_row(
            "SELECT ready FROM projection_conversion WHERE singleton=1",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("conversion pending; source is retained"));
        }
        conversion::provenance(&c, &result)?;
    }
    validate_platform(&database::current(&c)?.snapshot)?;
    verify_boundaries(&result.root, &result.marker, &metadata, &pin)?;
    if let Some(family) = &family {
        family.verify()?;
    }
    Ok(result)
}
pub(super) fn resume(
    root: &Path,
    identity: &ProjectionIdentity,
    initial: &ProjectionSnapshot,
) -> Result<ProjectionStore> {
    resume_selected(
        root,
        identity,
        initial,
        &layrs_core::projection::ProjectionSelection::All,
    )
}
pub(super) fn resume_selected(
    root: &Path,
    identity: &ProjectionIdentity,
    initial: &ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
) -> Result<ProjectionStore> {
    initial.canonical_bytes()?;
    selection.validate_snapshot(initial)?;
    validate_platform(initial)?;
    let root = std::path::absolute(root)?;
    let marker = read_marker(&root)?;
    if &marker.identity != identity {
        return Err(invalid("bootstrap identity mismatch"));
    }
    if marker.shared.is_some() {
        return Err(invalid("format4 creation uses explicit family recovery"));
    }
    marker.validate_initial_selection(selection)?;
    let _lock = StoreLock::acquire_existing(
        &root.join(".layrs/metadata/layrs.lock"),
        SessionMode::Exclusive,
    )
    .map_err(invalid)?;
    let metadata = PinnedDirectory::open(&root.join(".layrs/metadata"))?;
    verify_directories(&root, &marker, &metadata)?;
    let path = root.join(".layrs/metadata").join(DATABASE);
    if !path.exists() {
        create_new_file_no_follow(&path)?.sync_all()?;
        metadata.sync()?;
    }
    let db = super::file::DatabasePin::open(&path)?;
    let mut connection = database::connect(&path, true)?;
    database::initialize(&mut connection, &marker, initial, selection)?;
    drop(connection);
    crate::layout::open_read_write_no_follow(&path)?.sync_all()?;
    metadata.sync()?;
    verify_boundaries(&root, &marker, &metadata, &db)?;
    drop(_lock);
    open(&root)
}
pub(super) struct Access {
    pub connection: rusqlite::Connection,
    pub metadata: PinnedDirectory,
    pub database: super::file::DatabasePin,
    root: PathBuf,
    marker: Marker,
    recovery_binding: Option<RecoveryBinding>,
    _boundaries: PreliminaryPins,
    _lock: StoreLock,
    // Fields drop in declaration order: private connection and lock must be
    // released before an exclusive family reconciler can acquire the lifecycle.
    pub family: Option<shared::Lease>,
}
impl Access {
    pub fn verify(&self) -> Result<()> {
        self._boundaries.verify(&self.root, &self.marker)?;
        provider::require_ordinary(&self.connection)?;
        if let Some(family) = &self.family {
            family.verify()?;
        }
        verify_boundaries(&self.root, &self.marker, &self.metadata, &self.database)?;
        if let Some(expected) = &self.recovery_binding {
            recovery::verify_binding(
                &self.root,
                &self.marker,
                &self.database,
                self.family.as_ref(),
                expected,
            )?;
        }
        Ok(())
    }
}
pub(super) fn access(store: &ProjectionStore, mode: SessionMode) -> Result<Access> {
    let family = shared::for_store_discovery(store)?;
    let metadata_path = store.marker.metadata_path(&store.root);
    let _lock =
        StoreLock::acquire_existing(&metadata_path.join("layrs.lock"), mode).map_err(invalid)?;
    let marker = read_marker(&store.root)?;
    if marker != store.marker {
        return Err(invalid("projection identity changed after open"));
    }
    let metadata = PinnedDirectory::open(&metadata_path)?;
    let database = super::file::DatabasePin::open(&metadata_path.join(DATABASE))?;
    let boundaries = preliminary_pins(&store.root, &marker, &metadata, &database)?;
    if let Some(binding) = &marker.shared {
        if database.identity() != binding.database_identity {
            return Err(invalid("shared Worktree database replaced"));
        }
        // Discovery must never open a writer merely to discover ownership.
        let catalog = family
            .as_ref()
            .ok_or_else(|| invalid("missing family lease"))?
            .inspection_catalog(false)?;
        let (payload, state): (Vec<u8>, String) = catalog.connection.query_row(
            "SELECT marker,state FROM worktrees WHERE id=?1 AND length(marker)<=65536",
            [marker.identity.worktree_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if payload != marker.bytes()? || state != "ready" {
            return Err(invalid("Worktree registration differs or is not ready"));
        }
    }
    let read = database::connect(&metadata_path.join(DATABASE), false)?;
    let version: u32 = read.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if marker.shared.is_some() || !(3..=5).contains(&version) {
        database::validate(&read, &marker)?;
    }
    drop(read);
    boundaries.verify(&store.root, &marker)?;
    verify_boundaries(&store.root, &marker, &metadata, &database)?;
    let mut connection = database::connect(
        &metadata_path.join(DATABASE),
        mode == SessionMode::Exclusive,
    )?;
    if mode == SessionMode::Exclusive && marker.shared.is_none() {
        database::migrate(&mut connection)?;
    }
    database::validate(&connection, &marker)?;
    if marker.import_digest.is_some() {
        let ready: bool = connection.query_row(
            "SELECT ready FROM projection_conversion WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if !ready {
            return Err(invalid("conversion pending; source is retained"));
        }
        conversion::provenance(&connection, store)?;
    }
    if mode == SessionMode::Exclusive && marker.shared.is_some() {
        retention::settle(&connection, store)?;
    }
    let result = Access {
        connection,
        metadata,
        database,
        family,
        root: store.root.clone(),
        marker,
        recovery_binding: None,
        _boundaries: boundaries,
        _lock,
    };
    result.verify()?;
    Ok(result)
}
/// Filesystem identity only: deliberately contains no CheckoutReader or SQL.
/// Generic directory pins refuse CFAPI roots until the provider control guard
/// joins in Stage1b. A refusal does not manufacture ordinary namespace proof.
pub(super) struct PreliminaryPins {
    root: PinnedDirectory,
    control: PinnedDirectory,
    format: super::file::DatabasePin,
}
fn require_fresh_control(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root.join(".layrs")) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => Err(invalid("creation requires a fresh control directory")),
    }
}
impl PreliminaryPins {
    pub(super) fn verify(&self, root: &Path, marker: &Marker) -> Result<()> {
        self.root.verify_path_identity()?;
        self.control.verify_path_identity()?;
        self.format.verify(&root.join(".layrs/FORMAT"))?;
        if read_marker(root)? != *marker {
            return Err(invalid("projection FORMAT changed"));
        }
        Ok(())
    }
}
pub(super) fn preliminary_pins(
    root: &Path,
    marker: &Marker,
    metadata: &PinnedDirectory,
    database: &super::file::DatabasePin,
) -> Result<PreliminaryPins> {
    if root.to_str() != Some(marker.checkout_root.as_str()) {
        return Err(invalid("projection checkout root moved"));
    }
    let pins = PreliminaryPins {
        root: PinnedDirectory::open(root)?,
        control: PinnedDirectory::open(&root.join(".layrs"))?,
        format: super::file::DatabasePin::open(&root.join(".layrs/FORMAT"))?,
    };
    if pins.root.identity_token() != marker.checkout_identity
        || pins.control.identity_token() != marker.store_identity
        || metadata.identity_token() != marker.metadata_identity
    {
        return Err(invalid("projection metadata directory identity changed"));
    }
    metadata.verify_path_identity()?;
    database.verify(&marker.metadata_path(root).join(DATABASE))?;
    if marker
        .shared
        .as_ref()
        .is_some_and(|b| b.database_identity != database.identity())
    {
        return Err(invalid("shared Worktree database replaced"));
    }
    pins.verify(root, marker)?;
    Ok(pins)
}
fn verify_directories(root: &Path, marker: &Marker, metadata: &PinnedDirectory) -> Result<()> {
    if root.to_str() != Some(marker.checkout_root.as_str()) {
        return Err(invalid("projection checkout root moved"));
    }
    crate::scm_v1::checkout::CheckoutReader::open(root, Some(&marker.checkout_identity))?;
    let store = PinnedDirectory::open(&root.join(".layrs"))?;
    if store.identity_token() != marker.store_identity
        || metadata.identity_token() != marker.metadata_identity
    {
        return Err(invalid("projection metadata directory identity changed"));
    }
    metadata.verify_path_identity()?;
    Ok(())
}
pub(super) fn verify_boundaries(
    root: &Path,
    marker: &Marker,
    metadata: &PinnedDirectory,
    database: &super::file::DatabasePin,
) -> Result<()> {
    verify_directories(root, marker, metadata)?;
    database.verify(&marker.metadata_path(root).join(DATABASE))?;
    Ok(())
}
