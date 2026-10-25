//! Explicit copy conversion. No source SQL, marker, history, or checkout is rewritten.
use super::*;
use crate::{lock::StoreLock, secure_fs::PinnedDirectory};
use layrs_core::scm::{FileId, canonical_json};
use rusqlite::{Connection, params};
use std::{
    io::{Read, Seek, SeekFrom},
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversionReceipt {
    pub source_checkout: PathBuf,
    pub source_identity: ProjectionIdentity,
    pub source_marker: Vec<u8>,
    pub source_database_digest: String,
    pub source_generation: u64,
    pub source_projection_digest: String,
    pub imported_capture_count: u64,
    pub imported_capture_head: Option<String>,
    pub checkout_digest: String,
    pub publication_history_digest: String,
    pub destination_identity: ProjectionIdentity,
    pub source_retained: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionPoint {
    DestinationCreated,
    DatabaseCopied,
    ContentInstalled,
    CheckoutCopied,
    BeforeReady,
    ReadyRegistered,
}
pub trait ConversionFaults {
    fn check(&self, point: ConversionPoint) -> io::Result<()>;
}
pub struct NoConversionFaults;
impl ConversionFaults for NoConversionFaults {
    fn check(&self, _: ConversionPoint) -> io::Result<()> {
        Ok(())
    }
}

fn database_digest(file: &std::fs::File) -> Result<String> {
    let mut input = file;
    input.seek(SeekFrom::Start(0))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-import-database-v1\0");
    let mut buffer = [0u8; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().to_hex().to_string())
}
fn receipt(
    source: &ProjectionStore,
    access: &format::Access,
    destination: ProjectionIdentity,
) -> Result<ConversionReceipt> {
    if source.marker.shared.is_some() {
        return Err(invalid("conversion currently accepts format3 sources only"));
    }
    database::require_mutable(&access.connection)?;
    let state = database::current(&access.connection)?;
    let scan = capture::scan::scan(source, &access.connection, &state.snapshot, false)?;
    let count: u64 =
        access
            .connection
            .query_row("SELECT COUNT(*) FROM projection_captures", [], |r| r.get(0))?;
    let mut head = None;
    for n in 1..=count {
        head = Some(capture::storage::load(&access.connection, source, n)?.digest);
    }
    for n in 0..=state.generation {
        offline::snapshot_at(&access.connection, n, None)?;
    }
    namespace::verify_history(&access.connection, source)?;
    refresh::verify_history(&access.connection)?;
    restore::verify_history(&access.connection, source)?;
    rebase::verify_history(&access.connection, source)?;
    let mut q=access.connection.prepare("SELECT sequence,request_digest,receipt_digest FROM projection_publications ORDER BY sequence")?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in &rows {
        publication::storage::completed_record(&access.connection, source, row.0)?;
    }
    Ok(ConversionReceipt {
        source_checkout: source.root.clone(),
        source_identity: source.identity().clone(),
        source_marker: source.marker.bytes()?,
        source_database_digest: database_digest(&access.database.file)?,
        source_generation: state.generation,
        source_projection_digest: state.digest,
        imported_capture_count: count,
        imported_capture_head: head,
        checkout_digest: shared::digest(&scan.files)?,
        publication_history_digest: shared::digest(&rows)?,
        destination_identity: destination,
        source_retained: true,
    })
}

pub(super) fn provenance(
    c: &Connection,
    store: &ProjectionStore,
) -> Result<Option<ConversionReceipt>> {
    let Some(expected) = &store.marker.import_digest else {
        return Ok(None);
    };
    let (digest,payload):(String,Vec<u8>)=c.query_row("SELECT digest,payload FROM projection_imports WHERE singleton=1 AND length(payload)<=16777216",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let value: ConversionReceipt = serde_json::from_slice(&payload).map_err(invalid)?;
    if digest != *expected
        || shared::digest(&value)? != digest
        || canonical_json(&value).map_err(invalid)? != payload
        || value.destination_identity != *store.identity()
        || !value.source_retained
    {
        return Err(invalid("import provenance mismatch"));
    }
    let marker: format::Marker = serde_json::from_slice(&value.source_marker).map_err(invalid)?;
    marker.validate()?;
    if marker.identity != value.source_identity
        || marker.bytes()? != value.source_marker
        || marker.shared.is_some()
    {
        return Err(invalid("import source identity mismatch"));
    }
    let state = offline::snapshot_at(
        c,
        value.source_generation,
        Some(&value.source_projection_digest),
    )?;
    let initial = offline::snapshot_at(c, 0, None)?;
    if initial.digest != marker.initial_snapshot_digest
        || state.generation != value.source_generation
    {
        return Err(invalid("import base boundary mismatch"));
    }
    if value.imported_capture_count > 0 {
        let digest: String = c.query_row(
            "SELECT digest FROM projection_captures WHERE sequence=?1",
            [value.imported_capture_count],
            |r| r.get(0),
        )?;
        if Some(digest) != value.imported_capture_head {
            return Err(invalid("import capture boundary changed"));
        }
    } else if value.imported_capture_head.is_some() {
        return Err(invalid("empty imported history has a head"));
    }
    let mut q = c.prepare("SELECT sequence,request_digest,receipt_digest FROM projection_publications WHERE sequence<=?1 ORDER BY sequence")?;
    let imported_receipts = q
        .query_map([value.imported_capture_count], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if shared::digest(&imported_receipts)? != value.publication_history_digest {
        return Err(invalid(
            "imported publication prefix differs from source provenance",
        ));
    }
    Ok(Some(value))
}

fn require_conversion_source_schema(store: &ProjectionStore) -> Result<()> {
    if store.marker.shared.is_some() {
        return Err(invalid("conversion requires format3 source"));
    }
    let c = database::connect(
        &store
            .marker
            .metadata_path(&store.root)
            .join("projection.sqlite3"),
        false,
    )?;
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 6 {
        return Err(invalid(
            "source-preserving conversion supports private schema6 only; no automatic source migration",
        ));
    }
    Ok(())
}
pub(super) fn validate_capture_identity(
    c: &Connection,
    store: &ProjectionStore,
    patch: &CapturedPatch,
) -> Result<()> {
    if let Some(import) = provenance(c, store)? {
        if patch.sequence <= import.imported_capture_count {
            if patch.identity != import.source_identity
                || patch.projection_generation > import.source_generation
            {
                return Err(invalid("imported capture escaped provenance boundary"));
            }
            return Ok(());
        }
    }
    if patch.identity != *store.identity() {
        return Err(invalid("capture Worktree identity mismatch"));
    }
    Ok(())
}
pub(super) fn imported(c: &Connection, store: &ProjectionStore, sequence: u64) -> Result<bool> {
    Ok(provenance(c, store)?.is_some_and(|p| sequence > 0 && sequence <= p.imported_capture_count))
}

impl ProjectionStore {
    /// Read-only conversion entry point. Ordinary `open` may migrate older
    /// format3 databases; conversion must explicitly refuse those sources first.
    pub fn open_conversion_source(checkout_root: impl AsRef<Path>) -> Result<Self> {
        let root = std::path::absolute(checkout_root)?;
        let marker = format::read_marker(&root)?;
        let store = Self { root, marker };
        require_conversion_source_schema(&store)?;
        let access = format::access(&store, SessionMode::Shared)?;
        database::require_mutable(&access.connection)?;
        access.verify()?;
        drop(access);
        Ok(store)
    }
    pub fn is_imported_capture(&self, sequence: u64) -> Result<bool> {
        let access = format::access(self, SessionMode::Shared)?;
        imported(&access.connection, self, sequence)
    }
    pub fn require_publishable_capture(&self, sequence: u64) -> Result<()> {
        if self.is_imported_capture(sequence)? {
            Err(invalid(
                "imported_capture_not_publishable: create a destination capture first",
            ))
        } else {
            Ok(())
        }
    }
    pub fn imported_publication(
        &self,
        sequence: u64,
    ) -> Result<Option<(ProjectionIdentity, CompletedPublication)>> {
        let access = format::access(self, SessionMode::Shared)?;
        let Some(p) = provenance(&access.connection, self)? else {
            return Ok(None);
        };
        if sequence == 0 || sequence > p.imported_capture_count {
            return Ok(None);
        }
        Ok(
            publication::storage::completed_record(&access.connection, self, sequence)?
                .map(|(v, _)| (p.source_identity, v)),
        )
    }
    pub fn convert_into_family(
        &self,
        family: &PartialStoreFamily,
        destination: impl AsRef<Path>,
        worktree_id: WorktreeId,
    ) -> Result<(ProjectionStore, ConversionReceipt)> {
        self.convert_into_family_with_faults(family, destination, worktree_id, &NoConversionFaults)
    }
    pub fn convert_into_family_with_faults(
        &self,
        family: &PartialStoreFamily,
        destination: impl AsRef<Path>,
        worktree_id: WorktreeId,
        faults: &dyn ConversionFaults,
    ) -> Result<(ProjectionStore, ConversionReceipt)> {
        require_conversion_source_schema(self)?;
        let destination = std::path::absolute(destination)?;
        if destination.starts_with(&self.root)
            || self.root.starts_with(&destination)
            || std::fs::read_dir(&destination)?.next().is_some()
        {
            return Err(invalid("conversion requires a separate empty destination"));
        }
        if family.identity().space_id != self.identity().space_id
            || family.identity().origin != self.identity().origin
            || family.identity().principal_id != self.identity().principal_id
            || worktree_id == self.identity().worktree_id
        {
            return Err(invalid(
                "conversion identity context or fresh Worktree requirement mismatch",
            ));
        }
        let source_access = format::access(self, SessionMode::Exclusive)?;
        let identity = ProjectionIdentity {
            store_id: family.identity().store_id.clone(),
            worktree_id,
            space_id: family.identity().space_id.clone(),
            origin: family.identity().origin.clone(),
            principal_id: family.identity().principal_id.clone(),
        };
        let proof = receipt(self, &source_access, identity.clone())?;
        let initial = offline::snapshot_at(&source_access.connection, 0, None)?;
        let selection = selection::at(&source_access.connection, 0)?;
        let target = format::create_shared_import(
            &destination,
            family,
            identity.worktree_id,
            initial.snapshot,
            &selection,
            shared::digest(&proof)?,
        )?;
        faults.check(ConversionPoint::DestinationCreated)?;
        convert(self, &source_access, target, proof, faults)
    }
    pub fn resume_conversion_into(
        &self,
        destination: impl AsRef<Path>,
    ) -> Result<(ProjectionStore, ConversionReceipt)> {
        require_conversion_source_schema(self)?;
        let root = std::path::absolute(destination)?;
        let marker = format::read_marker(&root)?;
        if marker.shared.is_none() || marker.import_digest.is_none() {
            return Err(invalid("destination has no recognizable conversion intent"));
        }
        let source_access = format::access(self, SessionMode::Exclusive)?;
        let proof = receipt(self, &source_access, marker.identity.clone())?;
        if marker.import_digest.as_deref() != Some(shared::digest(&proof)?.as_str()) {
            return Err(invalid(
                "conversion source changed; retained source and destination require inspection",
            ));
        }
        convert(
            self,
            &source_access,
            ProjectionStore { root, marker },
            proof,
            &NoConversionFaults,
        )
    }
}

fn convert(
    source: &ProjectionStore,
    source_access: &format::Access,
    target: ProjectionStore,
    proof: ConversionReceipt,
    faults: &dyn ConversionFaults,
) -> Result<(ProjectionStore, ConversionReceipt)> {
    let lease = shared::for_store_discovery(&target)?
        .ok_or_else(|| invalid("missing conversion family"))?;
    drop(lease.inspection_catalog(false)?);
    lease.registered_state(&target.marker)?;
    let metadata_path = target.marker.metadata_path(&target.root);
    let _lock =
        StoreLock::acquire_existing(&metadata_path.join("layrs.lock"), SessionMode::Exclusive)
            .map_err(invalid)?;
    let metadata = PinnedDirectory::open(&metadata_path)?;
    let pin = super::file::DatabasePin::open(&metadata_path.join("projection.sqlite3"))?;
    let boundaries = format::preliminary_pins(&target.root, &target.marker, &metadata, &pin)?;
    let read = database::connect(&metadata_path.join("projection.sqlite3"), false)?;
    let read_version: u32 = read.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    // SQLite backup may have completed before the private identity/import
    // transaction. That exact source schema6 remains a recoverable copy.
    database::validate(
        &read,
        if read_version == 6 {
            &source.marker
        } else {
            &target.marker
        },
    )?;
    drop(read);
    boundaries.verify(&target.root, &target.marker)?;
    let mut c = database::connect(&metadata_path.join("projection.sqlite3"), true)?;
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let copied = matches!(version, 7 | 8)
        && c.query_row("SELECT EXISTS(SELECT 1 FROM projection_imports)", [], |r| {
            r.get::<_, bool>(0)
        })?;
    if !copied {
        if !matches!(version, 6 | 7) {
            return Err(invalid("conversion cannot replace an upgraded destination"));
        }
        rusqlite::backup::Backup::new(&source_access.connection, &mut c)?.run_to_completion(
            128,
            Duration::from_millis(1),
            None,
        )?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(retention::SQL)?;
        tx.execute_batch("PRAGMA user_version=7;")?;
        tx.execute(
            "UPDATE projection_meta SET identity=?1 WHERE singleton=1",
            [target.marker.bytes()?],
        )?;
        let bytes = canonical_json(&proof).map_err(invalid)?;
        tx.execute(
            "INSERT INTO projection_imports VALUES(1,?1,?2)",
            params![shared::digest(&proof)?, bytes],
        )?;
        tx.execute("INSERT INTO projection_conversion VALUES(1,?1,0)", [bytes])?;
        tx.commit()?;
    }
    database::validate(&c, &target.marker)?;
    if provenance(&c, &target)?.as_ref() != Some(&proof) {
        return Err(invalid("conversion provenance differs"));
    }
    let ready: bool = c.query_row(
        "SELECT ready FROM projection_conversion WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if ready {
        // Private acceptance may have committed just before registry publication
        // or the caller's response failed. Never allocate or copy again here.
        retention::fsck(&c, &target)?;
        lease.register(&target.marker, true)?;
        retention::settle(&c, &target)?;
        source_access.verify()?;
        drop(c);
        drop(_lock);
        drop(lease);
        return Ok((ProjectionStore::open(&target.root)?, proof));
    }
    faults.check(ConversionPoint::DatabaseCopied)?;
    let mut q=source_access.connection.prepare("SELECT file_id,length(bytes) FROM projection_capture_files UNION ALL SELECT file_id,SUM(length(bytes)) FROM projection_capture_chunks GROUP BY file_id ORDER BY file_id")?;
    let stored = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    let state = database::current(&source_access.connection)?;
    let scan = capture::scan::scan(source, &source_access.connection, &state.snapshot, false)?;
    let mut files = stored
        .iter()
        .map(|(id, bytes)| {
            Ok(CapturedFile {
                file_id: FileId::parse(id).map_err(invalid)?,
                bytes: *bytes,
                executable: false,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    files.extend(scan.files.values().filter_map(Clone::clone));
    let checkout_bytes =
        scan.files
            .values()
            .filter_map(Option::as_ref)
            .try_fold(0u64, |n, f| {
                n.checked_add(f.bytes)
                    .ok_or_else(|| invalid("conversion size overflow"))
            })?;
    retention::reserve(
        &c,
        &target,
        &format!("conversion:{}", shared::digest(&proof)?),
        &proof.checkout_digest,
        &files,
        checkout_bytes,
    )?;
    let reservation = retention::active_budget(&c)?;
    for (id, bytes) in stored {
        let file = CapturedFile {
            file_id: FileId::parse(id).map_err(invalid)?,
            bytes,
            executable: false,
        };
        let mut spool = crate::private_spool_file()?;
        capture::storage::stream_file(&source_access.connection, source, &file, &mut spool)?;
        spool.seek(SeekFrom::Start(0))?;
        let manifest = lease.install_file(&mut spool, &file, &reservation)?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        retention::accept_claim(
            &tx,
            &target,
            &lease,
            "import",
            &proof.source_database_digest,
            &[manifest],
        )?;
        tx.commit()?;
    }
    faults.check(ConversionPoint::ContentInstalled)?;
    let src = PinnedDirectory::open(&source.root)?;
    let dst = PinnedDirectory::open(&target.root)?;
    for (path, actual) in &scan.files {
        let Some(actual) = actual else {
            continue;
        };
        let input = src.open_relative_file(Path::new(path))?;
        let (parent, name) = dst.components_to_parent(Path::new(path))?;
        let destination_path = target.root.join(path);
        if destination_path.exists() {
            let pin = parent.open_file(&name)?;
            let mut reader = pin.file();
            let mut hash = blake3::Hasher::new();
            hash.update(b"layrs-file-v1\0");
            let mut buffer = [0u8; 65536];
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            if format!("blake3:{}", hash.finalize()) != actual.file_id.as_str() {
                return Err(invalid(
                    "conversion destination differs; no overwrite performed",
                ));
            }
        } else {
            let mut out = parent.create_file(&name)?;
            let mut reader = input.file();
            let mut writer = install::files::FileWriter::new(out.file_mut(), actual.bytes);
            io::copy(&mut reader, &mut writer)?;
            if writer.finish(&actual.file_id)? != actual.bytes {
                return Err(invalid("conversion checkout changed"));
            }
            install::files::set_mode(out.file(), actual.executable)?;
            out.file().sync_all()?;
            parent.sync()?;
        }
        let mut reader = input.file();
        reader.seek(SeekFrom::Start(0))?;
        let manifest = lease.install_file(&mut reader, actual, &reservation)?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        retention::accept_claim(
            &tx,
            &target,
            &lease,
            "conversion-checkout",
            &proof.checkout_digest,
            &[manifest],
        )?;
        tx.commit()?;
    }
    faults.check(ConversionPoint::CheckoutCopied)?;
    if receipt(source, source_access, proof.destination_identity.clone())? != proof {
        return Err(invalid(
            "conversion source changed; destination remains unready",
        ));
    }
    if shared::digest(&capture::scan::scan(&target, &c, &state.snapshot, false)?.files)?
        != proof.checkout_digest
    {
        return Err(invalid("converted checkout differs from source witness"));
    }
    retention::fsck(&c, &target)?;
    for sequence in 1..=proof.imported_capture_count {
        capture::storage::load(&c, &target, sequence)?;
        publication::storage::completed_record(&c, &target, sequence)?;
    }
    faults.check(ConversionPoint::BeforeReady)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // Destination-only temporary duplicate bytes; original source is untouched.
    tx.execute("DELETE FROM projection_capture_chunks", [])?;
    tx.execute("DELETE FROM projection_capture_files", [])?;
    tx.execute(
        "UPDATE projection_conversion SET ready=1 WHERE singleton=1",
        [],
    )?;
    retention::finish_budget(&tx, &target)?;
    tx.commit()?;
    lease.register(&target.marker, true)?;
    faults.check(ConversionPoint::ReadyRegistered)?;
    retention::settle(&c, &target)?;
    source_access.verify()?;
    drop(c);
    drop(_lock);
    drop(lease);
    Ok((ProjectionStore::open(&target.root)?, proof))
}

impl PartialStoreFamily {
    /// Abandon only an unready imported destination. Physical source, destination
    /// checkout and private database survive; registry/claims cease blocking GC.
    /// A ready destination or any accepted new local history cannot be abandoned.
    pub fn abandon_conversion(&self, destination: impl AsRef<Path>) -> Result<()> {
        let root = std::path::absolute(destination)?;
        let marker = format::read_marker(&root)?;
        if marker.import_digest.is_none() {
            return Err(invalid("destination is not a conversion"));
        }
        self.abandon_registered(&marker.identity.worktree_id, Some(&marker))
    }
    /// Abandon only a proven unready bootstrap/import, even if its checkout or
    /// FORMAT is missing. Physical files survive. Ready owners, new captures,
    /// and unreadable private identities are never forgotten.
    pub fn abandon_initializing(&self, worktree_id: &WorktreeId) -> Result<()> {
        self.abandon_registered(worktree_id, None)
    }
    fn abandon_registered(
        &self,
        worktree_id: &WorktreeId,
        expected: Option<&format::Marker>,
    ) -> Result<()> {
        let lease = self.lease(SessionMode::Exclusive)?;
        let catalog = lease.catalog(false)?;
        let (payload, state): (Vec<u8>, String) = catalog.connection.query_row(
            "SELECT marker,state FROM worktrees WHERE id=?1 AND length(marker)<=65536",
            [worktree_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        drop(catalog);
        let marker: format::Marker = serde_json::from_slice(&payload).map_err(invalid)?;
        marker.validate()?;
        if marker.bytes()? != payload
            || &marker.identity.worktree_id != worktree_id
            || expected.is_some_and(|e| e != &marker)
            || state != "initializing"
        {
            return Err(invalid(
                "accepted or mismatched destination cannot be abandoned",
            ));
        }
        let binding = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("conversion has no shared binding"))?;
        if binding != &self.binding(worktree_id, binding.database_identity.clone())
            || marker.identity.store_id != self.identity().store_id
            || marker.identity.space_id != self.identity().space_id
            || marker.identity.origin != self.identity().origin
            || marker.identity.principal_id != self.identity().principal_id
        {
            return Err(invalid("conversion does not belong to this family"));
        }
        if lease.registered_state(&marker)? != "initializing" {
            return Err(invalid("accepted destination cannot be abandoned"));
        }
        let _lock = StoreLock::acquire_existing(
            &binding.metadata_path.join("layrs.lock"),
            SessionMode::Exclusive,
        )
        .map_err(invalid)?;
        let metadata = PinnedDirectory::open(&binding.metadata_path)?;
        let pin =
            super::file::DatabasePin::open(&binding.metadata_path.join("projection.sqlite3"))?;
        if pin.identity() != binding.database_identity {
            return Err(invalid("conversion database replaced"));
        }
        if metadata.identity_token() != marker.metadata_identity {
            return Err(invalid(
                "private bootstrap directory replaced; restore backup before recovery",
            ));
        }
        pin.verify(&binding.metadata_path.join("projection.sqlite3"))?;
        let target = ProjectionStore {
            root: PathBuf::from(&marker.checkout_root),
            marker: marker.clone(),
        };
        let read = database::connect(&binding.metadata_path.join("projection.sqlite3"), false)?;
        database::validate(&read, &marker)?;
        drop(read);
        let mut c = database::connect(&binding.metadata_path.join("projection.sqlite3"), true)?;
        let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if matches!(version, 7 | 8) {
            database::validate(&c, &marker)?;
            let imported: bool =
                c.query_row("SELECT EXISTS(SELECT 1 FROM projection_imports)", [], |r| {
                    r.get(0)
                })?;
            let limit = if imported {
                provenance(&c, &target)?
                    .ok_or_else(|| invalid("missing import proof"))?
                    .imported_capture_count
            } else {
                if database::current(&c)?.generation != 0 || c.query_row("SELECT EXISTS(SELECT 1 FROM projection_claims) OR EXISTS(SELECT 1 FROM projection_preparations) OR EXISTS(SELECT 1 FROM projection_budgets) OR EXISTS(SELECT 1 FROM projection_prejournal) OR EXISTS(SELECT 1 FROM projection_publications)",[],|r|r.get::<_,bool>(0))? {
                    return Err(invalid("bootstrap contains accepted dependencies; retained"));
                }
                0
            };
            let max: u64 = c.query_row(
                "SELECT COALESCE(MAX(sequence),0) FROM projection_captures",
                [],
                |r| r.get(0),
            )?;
            if max > limit {
                return Err(invalid("destination has new local captures; retained"));
            }
            let ready: bool = c.query_row(
                "SELECT COALESCE((SELECT ready FROM projection_conversion WHERE singleton=1),0)",
                [],
                |r| r.get(0),
            )?;
            if ready {
                return Err(invalid("destination acceptance committed; resume instead"));
            }
            database::require_mutable(&c)?;
        } else {
            return Err(invalid(
                "unverified interrupted private database; retention preserved, restore its backup before recovery",
            ));
        }
        let catalog = lease.catalog(false)?;
        let mut q = catalog
            .connection
            .prepare("SELECT id FROM reservations WHERE owner=?1 ORDER BY id")?;
        let reservations = q
            .query_map([marker.identity.worktree_id.as_str()], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        drop(catalog);
        for reservation in &reservations {
            lease.cancel_installations(reservation, &marker.identity.worktree_id)?;
        }
        // Even DELETE on an empty table can advance SQLite's header counters.
        // A fresh bootstrap has no private ownership to retire: preserve its
        // database byte-for-byte and remove only the shared registry intent.
        let private_ownership: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_claims) OR EXISTS(SELECT 1 FROM projection_budgets)",
            [],
            |r| r.get(0),
        )?;
        if private_ownership {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM projection_claims", [])?;
            tx.execute("DELETE FROM projection_budgets", [])?;
            tx.commit()?;
        }
        pin.verify(&binding.metadata_path.join("projection.sqlite3"))?;
        metadata.verify_path_identity()?;
        let mut catalog = lease.catalog(true)?;
        let tx = catalog
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM roots WHERE owner=?1",
            [marker.identity.worktree_id.as_str()],
        )?;
        tx.execute(
            "DELETE FROM reservations WHERE owner=?1",
            [marker.identity.worktree_id.as_str()],
        )?;
        tx.execute(
            "DELETE FROM worktrees WHERE id=?1 AND state='initializing'",
            [marker.identity.worktree_id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }
}
