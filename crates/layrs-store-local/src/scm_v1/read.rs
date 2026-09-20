//! Typed, format-gated native SCM reads.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use layrs_core::scm::{
    ActorId, Chunk, ChunkId, ConflictId, DeviceId, FileId, FileManifest, FileManifestId, Layer,
    LayerId, Marker, MarkerId, ObservationId, Operation, OperationId, OperationKind, ProofId,
    Release, ReleaseId, ReleasePublication, ReleasePublicationId, RepositoryView, RepositoryViewId,
    Space, SpaceId, Step, StepId, StepObservation, StoreId, Symlink, SymlinkId, Timestamp,
    Tombstone, TombstoneId, Tree, TreeId, Worktree, WorktreeId, canonical_json,
};
use layrs_store::scm::{
    CanonicalConflict, CanonicalObject, CurrentRepositoryView, FileAvailability, Result, ScmReader,
    StoreError, StoreObjectId, StoreObjectKind,
};
use rusqlite::{Connection, DatabaseName, OptionalExtension, TransactionState, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::sqlite::rfc3339_to_epoch_millis;
use super::{CasError, NativeCas, NativeCasWriter, NativeStoreOpen};
mod history;
pub use history::{ObservationProjection, ObservationSnapshot};

const DEFAULT_FILE_AVAILABILITY_ROWS: usize = 66_561;
// Logical verification work, not an allocation: availability streams one
// bounded chunk at a time. Reserve metadata overhead beside four 1GiB files.
const DEFAULT_FILE_AVAILABILITY_BYTES: usize = 4 * 1024 * 1024 * 1024 + 256 * 1024 * 1024;
const DEFAULT_FILE_AVAILABILITY_MANIFESTS: usize = 1_024;
const DEFAULT_FILE_AVAILABILITY_CHUNKS: usize = 65_536;

/// Reader bound to a validated Ready Store capability.
pub struct NativeScmReader<'store> {
    identities: super::identity::IdentityRegistry,
    source: SqlReadSource<'store>,
    cas: CasReadSource<'store>,
}

enum SqlReadSource<'source> {
    Ready(&'source NativeStoreOpen),
    Transaction(&'source Mutex<Connection>),
}

enum CasReadSource<'source> {
    Owned(NativeCas),
    Transaction(&'source NativeCas),
}

impl Deref for CasReadSource<'_> {
    type Target = NativeCas;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(cas) => cas,
            Self::Transaction(cas) => cas,
        }
    }
}

struct ReadConnection<'source>(MutexGuard<'source, Connection>);

impl Deref for ReadConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// SCM reader pinned to a writer connection that is already inside one
/// `BEGIN IMMEDIATE` transaction.
///
/// Both the connection and CAS reader are borrowed. Consequently this reader
/// cannot outlive either the transaction owner or the exclusive Store/CAS
/// capability. It never acquires a second Store lock and never opens another
/// SQLite connection.
pub struct NativeTransactionalScmReader<'transaction> {
    reader: NativeScmReader<'transaction>,
}

impl<'transaction> NativeTransactionalScmReader<'transaction> {
    /// Binds reads to `connection` after its owner has executed
    /// `BEGIN IMMEDIATE`. Construction fails while the connection is in
    /// autocommit mode, preventing accidental validation outside the write
    /// transaction.
    pub fn new(
        connection: &'transaction Mutex<Connection>,
        cas: &'transaction NativeCasWriter<'_>,
    ) -> Result<Self> {
        NativeScmReader::transactional(connection, cas).map(|reader| Self { reader })
    }

    /// Returns the shared implementation only after proving that the write
    /// transaction is still active. Every `ScmReader` entry point delegates
    /// through this gate, including methods whose implementation can return
    /// without touching SQLite.
    fn active_reader(&self) -> Result<&NativeScmReader<'transaction>> {
        drop(self.reader.connection()?);
        Ok(&self.reader)
    }
}

impl<'store> NativeScmReader<'store> {
    pub fn new(store: &'store NativeStoreOpen) -> Result<Self> {
        let cas = NativeCas::open(store).map_err(cas_open_backend)?;
        Ok(Self {
            source: SqlReadSource::Ready(store),
            identities: super::identity::read_registry(store.layout().root(), store.store_id())?,
            cas: CasReadSource::Owned(cas),
        })
    }

    pub fn store(&self) -> &'store NativeStoreOpen {
        match self.source {
            SqlReadSource::Ready(store) => store,
            SqlReadSource::Transaction(_) => {
                unreachable!("transactional readers do not expose a Ready Store handle")
            }
        }
    }

    fn transactional(
        connection: &'store Mutex<Connection>,
        cas: &'store NativeCasWriter<'_>,
    ) -> Result<Self> {
        let connection_guard = connection
            .lock()
            .map_err(|_| backend("transactional SQLite connection lock is poisoned"))?;
        require_write_transaction(&connection_guard)?;
        let actual_database = connection_guard
            .path()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| backend("transactional SCM reader requires a file-backed database"))?;
        let actual_database = std::fs::canonicalize(actual_database)
            .map_err(|error| backend(format!("resolve transactional SQLite path: {error}")))?;
        let expected_database = std::fs::canonicalize(cas.root().join("metadata/layrs.sqlite3"))
            .map_err(|error| backend(format!("resolve native Store SQLite path: {error}")))?;
        if actual_database != expected_database {
            return Err(backend(
                "transactional SQLite connection and CAS writer belong to different Stores",
            ));
        }
        drop(connection_guard);
        Ok(Self {
            source: SqlReadSource::Transaction(connection),
            identities: super::identity::read_registry(cas.root(), cas.store_id())?,
            cas: CasReadSource::Transaction(cas.reader()),
        })
    }

    fn connection(&self) -> Result<ReadConnection<'_>> {
        match self.source {
            SqlReadSource::Ready(store) => store.connection().map(ReadConnection),
            SqlReadSource::Transaction(connection) => {
                let connection = connection
                    .lock()
                    .map_err(|_| backend("transactional SQLite connection lock is poisoned"))?;
                // The transaction is represented by SQLite connection state,
                // so revalidate it for every read. Even if a caller issues a
                // raw rollback while this borrowed reader exists, no later
                // read can escape into autocommit mode.
                require_write_transaction(&connection)?;
                Ok(ReadConnection(connection))
            }
        }
    }

    fn root(&self) -> &Path {
        match self.source {
            SqlReadSource::Ready(store) => store.layout().root(),
            SqlReadSource::Transaction(_) => self.cas.root(),
        }
    }

    fn read_object(&self, id: &StoreObjectId) -> Result<Option<CanonicalObject>> {
        let connection = self.connection()?;
        let row: Option<(String, String, i64, i64)> = connection
            .query_row(
                "SELECT object_kind, storage_path, byte_length, verified \
                 FROM objects WHERE object_id = ?1",
                [id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sqlite_backend)?;
        drop(connection);
        let Some((kind, storage_path, byte_length, verified)) = row else {
            return Ok(None);
        };
        if kind != kind_name(id.kind()) || verified != 1 || byte_length <= 0 {
            return Err(corruption(format!(
                "objects row for {id} disagrees with requested kind or verification state"
            )));
        }
        let expected_path = self.cas.object_path(id).map_err(cas_read_error)?;
        let expected_relative = expected_path
            .strip_prefix(self.root())
            .map_err(|_| corruption(format!("CAS path for {id} escapes Store root")))?;
        if normalize_storage_path(expected_relative) != storage_path.replace('\\', "/") {
            return Err(corruption(format!(
                "objects.storage_path for {id} does not match its canonical CAS path"
            )));
        }
        let metadata = std::fs::symlink_metadata(&expected_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                corruption(format!("verified CAS object {id} is missing"))
            } else {
                backend(format!("inspect CAS object {id}: {error}"))
            }
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(corruption(format!(
                "verified CAS object {id} is not a regular file"
            )));
        }
        if metadata.len() != u64::try_from(byte_length).unwrap_or_default() {
            return Err(corruption(format!(
                "objects.byte_length for {id} does not match stored bytes"
            )));
        }
        self.cas.read(id).map(Some).map_err(cas_read_error)
    }

    fn entity_payload<T>(
        &self,
        table: &'static str,
        id_column: &'static str,
        id: &str,
        kind: &'static str,
    ) -> Result<Option<T>>
    where
        T: DeserializeOwned + Serialize,
    {
        let connection = self.connection()?;
        let sql = format!("SELECT payload_json FROM {table} WHERE {id_column} = ?1");
        let payload: Option<String> = connection
            .query_row(&sql, [id], |row| row.get(0))
            .optional()
            .map_err(sqlite_backend)?;
        payload
            .map(|payload| decode_payload::<T>(&payload, kind, id))
            .transpose()
    }

    fn validate_scalar_projections<T: Serialize>(
        &self,
        table: &'static str,
        id_column: &'static str,
        id: &str,
        value: &T,
        projections: &[(&'static str, &'static str)],
    ) -> Result<()> {
        let payload = serde_json::to_value(value).map_err(core_corruption)?;
        let connection = self.connection()?;
        for (column, field) in projections {
            let sql = format!("SELECT CAST({column} AS TEXT) FROM {table} WHERE {id_column} = ?1");
            let projected: Option<Option<String>> = connection
                .query_row(&sql, [id], |row| row.get(0))
                .optional()
                .map_err(sqlite_backend)?;
            let Some(projected) = projected else {
                return Err(corruption(format!("missing {table} projection for {id}")));
            };
            let expected = projection_text(payload.get(*field));
            if projected != expected {
                return Err(corruption(format!(
                    "{table}.{column} for {id} disagrees with payload field {field}"
                )));
            }
        }
        Ok(())
    }

    fn validate_timestamp_ms(
        &self,
        table: &'static str,
        id_column: &'static str,
        id: &str,
        column: &'static str,
        timestamp: &Timestamp,
    ) -> Result<()> {
        let expected = rfc3339_to_epoch_millis(timestamp.as_str()).map_err(|error| {
            corruption(format!(
                "invalid payload timestamp {} for {table}.{column} ({id}): {error}",
                timestamp.as_str()
            ))
        })?;
        let connection = self.connection()?;
        let sql = format!("SELECT {column} FROM {table} WHERE {id_column} = ?1");
        let projected: Option<i64> = connection
            .query_row(&sql, [id], |row| row.get(0))
            .optional()
            .map_err(sqlite_backend)?;
        if projected != Some(expected) {
            return Err(corruption(format!(
                "{table}.{column} for {id} disagrees with payload timestamp {}",
                timestamp.as_str()
            )));
        }
        Ok(())
    }

    fn ordered_strings(&self, sql: &str, id: &str) -> Result<Vec<String>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(sql).map_err(sqlite_backend)?;
        let rows = statement
            .query_map([id], |row| {
                Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_backend)?
            .map(|row| row.map_err(sqlite_backend))
            .collect::<Result<Vec<_>>>()?;
        let mut values = Vec::with_capacity(rows.len());
        for (expected, (ordinal, value)) in rows.into_iter().enumerate() {
            if ordinal != expected as u64 {
                return Err(corruption(format!(
                    "non-contiguous projection ordinals for {id}"
                )));
            }
            values.push(value);
        }
        Ok(values)
    }

    fn tagged_ids(&self, sql: &str, id: &str) -> Result<Vec<(String, String)>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(sql).map_err(sqlite_backend)?;
        statement
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_backend)?
            .map(|row| row.map_err(sqlite_backend))
            .collect()
    }
}

mod availability;
mod projections;
mod reader;
mod transactional;

fn decode_payload<T>(payload: &str, kind: &'static str, id: &str) -> Result<T>
where
    T: DeserializeOwned + Serialize,
{
    let value: T = serde_json::from_str(payload)
        .map_err(|error| corruption(format!("invalid {kind} payload {id}: {error}")))?;
    let canonical = canonical_json(&value).map_err(core_corruption)?;
    if canonical != payload.as_bytes() {
        return Err(corruption(format!("non-canonical {kind} payload {id}")));
    }
    Ok(value)
}

fn require_write_transaction(connection: &Connection) -> Result<()> {
    let transaction_state = connection
        .transaction_state(Some(DatabaseName::Main))
        .map_err(sqlite_backend)?;
    if transaction_state != TransactionState::Write {
        return Err(backend(
            "transactional SCM reader requires an active SQLite write transaction",
        ));
    }
    Ok(())
}

fn projection_text(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(if *value { "1" } else { "0" }.to_owned()),
        Some(value) => Some(value.to_string()),
    }
}

fn canonical_text(value: &impl Serialize) -> Result<String> {
    String::from_utf8(canonical_json(value).map_err(core_corruption)?)
        .map_err(|error| corruption(error.to_string()))
}

fn enum_name(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn kind_name(kind: StoreObjectKind) -> &'static str {
    match kind {
        StoreObjectKind::ChunkRaw => "chunk_raw",
        StoreObjectKind::FileManifest => "file_manifest",
        StoreObjectKind::Tree => "tree",
        StoreObjectKind::Symlink => "symlink",
        StoreObjectKind::Tombstone => "tombstone",
        StoreObjectKind::Step => "step",
        StoreObjectKind::RepositoryView => "repository_view",
        StoreObjectKind::Conflict => "conflict",
        StoreObjectKind::Product => "product",
    }
}

fn normalize_storage_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[derive(Clone, Copy)]
struct FileAvailabilityLimits {
    rows: usize,
    bytes: usize,
    manifests: usize,
    chunks: usize,
}

#[derive(Default)]
struct FileAvailabilityConsumption {
    rows: usize,
    bytes: usize,
    manifests: usize,
    chunks: usize,
}

impl FileAvailabilityConsumption {
    fn charge_rows(&mut self, amount: usize, limit: usize) -> Result<()> {
        charge_file_budget(&mut self.rows, amount, limit, "File availability rows")
    }

    fn charge_bytes(&mut self, amount: usize, limit: usize) -> Result<()> {
        charge_file_budget(&mut self.bytes, amount, limit, "File availability bytes")
    }

    fn charge_manifests(&mut self, amount: usize, limit: usize) -> Result<()> {
        charge_file_budget(
            &mut self.manifests,
            amount,
            limit,
            "File availability manifests",
        )
    }

    fn charge_chunks(&mut self, amount: usize, limit: usize) -> Result<()> {
        charge_file_budget(&mut self.chunks, amount, limit, "File availability chunks")
    }
}

fn charge_file_budget(
    consumed: &mut usize,
    amount: usize,
    limit: usize,
    resource: &'static str,
) -> Result<()> {
    let next = consumed
        .checked_add(amount)
        .ok_or_else(|| file_budget_exceeded(resource, limit))?;
    if next > limit {
        return Err(file_budget_exceeded(resource, limit));
    }
    *consumed = next;
    Ok(())
}

fn file_budget_exceeded(resource: &'static str, limit: usize) -> StoreError {
    StoreError::AdmissionBudgetExceeded { resource, limit }
}

fn positive_usize(value: i64, kind: &str, id: &impl std::fmt::Display) -> Result<usize> {
    if value <= 0 {
        return Err(corruption(format!(
            "objects.byte_length for {kind} {id} is not a positive platform length"
        )));
    }
    usize::try_from(value).map_err(|_| {
        corruption(format!(
            "objects.byte_length for {kind} {id} is not a positive platform length"
        ))
    })
}

fn corruption(message: impl Into<String>) -> StoreError {
    StoreError::Corruption {
        message: message.into(),
    }
}

fn backend(message: impl Into<String>) -> StoreError {
    StoreError::Backend {
        message: message.into(),
    }
}

fn sqlite_backend(error: rusqlite::Error) -> StoreError {
    backend(format!("SQLite read failed: {error}"))
}

fn cas_open_backend(error: CasError) -> StoreError {
    backend(format!("open CAS reader: {error}"))
}

fn cas_read_error(error: CasError) -> StoreError {
    match error {
        CasError::Io { .. } | CasError::InjectedFault { .. } | CasError::UnsupportedPublication => {
            backend(format!("CAS read failed: {error}"))
        }
        CasError::InvalidObjectId { .. }
        | CasError::NotFound { .. }
        | CasError::KindMismatch { .. }
        | CasError::IdMismatch { .. }
        | CasError::Corruption { .. }
        | CasError::ExistingObjectMismatch { .. }
        | CasError::InvalidStagingPath(_) => corruption(format!("CAS read failed: {error}")),
    }
}

fn core_corruption(error: impl std::fmt::Display) -> StoreError {
    corruption(error.to_string())
}
