use super::*;
use crate::{
    layout::{create_new_file_no_follow, open_read_no_follow, sync_directory},
    lock::StoreLock,
    secure_fs::PinnedDirectory,
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
    checkout_root: String,
    checkout_identity: String,
    store_identity: String,
    metadata_identity: String,
    pub initial_snapshot_digest: String,
}
impl Marker {
    pub fn bytes(&self) -> Result<Vec<u8>> {
        layrs_core::scm::canonical_json(self).map_err(invalid)
    }
    fn validate(&self) -> Result<()> {
        if self.schema_version != "layrs.projection-store-format.v1"
            || self.magic != "LAYRS_NATIVE_STORE"
            || self.format_version != 3
            || self.min_reader_version != 3
        {
            return Err(invalid("unsupported projection FORMAT"));
        }
        validate_identity(&self.identity)
    }
}
fn validate_identity(i: &ProjectionIdentity) -> Result<()> {
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
    validate_identity(&identity)?;
    initial.canonical_bytes()?;
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
        identity,
        checkout_root: root
            .to_str()
            .ok_or_else(|| invalid("non-UTF8 checkout root"))?
            .into(),
        checkout_identity: checkout.identity(),
        store_identity: store.identity_token(),
        metadata_identity: metadata.identity_token(),
        initial_snapshot_digest: database::digest(0, None, &initial.canonical_bytes()?),
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
    database::initialize(&mut connection, &marker, &initial)?;
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
    let marker: Marker = serde_json::from_slice(&bytes).map_err(invalid)?;
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
    let current = result.state()?;
    validate_platform(&current.snapshot)?;
    Ok(result)
}
pub(super) fn resume(
    root: &Path,
    identity: &ProjectionIdentity,
    initial: &ProjectionSnapshot,
) -> Result<ProjectionStore> {
    initial.canonical_bytes()?;
    validate_platform(initial)?;
    let root = std::path::absolute(root)?;
    let marker = read_marker(&root)?;
    if &marker.identity != identity {
        return Err(invalid("bootstrap identity mismatch"));
    }
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
    database::initialize(&mut connection, &marker, initial)?;
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
    _lock: StoreLock,
}
impl Access {
    pub fn verify(&self) -> Result<()> {
        verify_boundaries(&self.root, &self.marker, &self.metadata, &self.database)
    }
}
pub(super) fn access(store: &ProjectionStore, mode: SessionMode) -> Result<Access> {
    let _lock = StoreLock::acquire_existing(&store.root.join(".layrs/metadata/layrs.lock"), mode)
        .map_err(invalid)?;
    let marker = read_marker(&store.root)?;
    if marker != store.marker {
        return Err(invalid("projection identity changed after open"));
    }
    let metadata = PinnedDirectory::open(&store.root.join(".layrs/metadata"))?;
    verify_directories(&store.root, &marker, &metadata)?;
    let database =
        super::file::DatabasePin::open(&store.root.join(".layrs/metadata").join(DATABASE))?;
    let connection = database::connect(
        &store.root.join(".layrs/metadata").join(DATABASE),
        mode == SessionMode::Exclusive,
    )?;
    database::validate(&connection, &marker)?;
    let result = Access {
        connection,
        metadata,
        database,
        root: store.root.clone(),
        marker,
        _lock,
    };
    result.verify()?;
    Ok(result)
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
    database.verify(&root.join(".layrs/metadata").join(DATABASE))?;
    Ok(())
}
