use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use layrs_core::scm::{
    ActorId, Chunk, ChunkId, ConflictId, DeviceId, FileId, FileManifest, FileManifestId, Layer,
    LayerId, Marker, MarkerId, ObservationId, Operation, OperationId, ProofId, Release, ReleaseId,
    ReleasePublication, ReleasePublicationId, RepositoryView, RepositoryViewId, Space, SpaceId,
    Step, StepId, StepObservation, StoreId, Symlink, SymlinkId, Tombstone, TombstoneId, Tree,
    TreeId, Worktree, WorktreeId, canonical_json,
};
use layrs_store::scm::{
    ApplyBatchOutcome, CanonicalConflict, CanonicalObject, CurrentRepositoryView, EntityRecord,
    FileAvailability, Result, ScmReader, ScmRepository, StoreError, StoreMutation, StoreObjectId,
    WriteBatch,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::acquisition::{ORDINARY_WAIT, SHORT_WAIT, acquire};
use super::cas::{CasFaultInjector, CasFaultPoint, NativeCasWriter, PutItem};
use super::sqlite::{
    canonical_receipt_json, rfc3339_to_epoch_millis, validate_canonical_receipt_json,
};
use super::write_batch::{
    ApplyBatchReceipt, RepositoryFaultInjector, RepositoryFaultPoint, no_faults,
};
use super::{
    CompositeAdmissionResolver, NativeScmReader, NativeTransactionalScmReader, open, open_exclusive,
};

mod streaming;
pub(super) use streaming::preflight as streaming_preflight;

/// Path-owned native repository. Store capabilities are deliberately short lived:
/// reads take a shared capability and each write takes one exclusive capability.
pub struct NativeScmRepository {
    root: PathBuf,
    faults: Arc<dyn RepositoryFaultInjector>,
    cas_faults: Arc<dyn CasFaultInjector>,
    materialization_permit: Option<(String, String)>,
    expected_identity: Option<String>,
    expected_metadata_identity: Option<String>,
    expected_store_id: Option<StoreId>,
    acquisition_budget: Duration,
}

/// The materializer's lease (`materialize::lease::LEASE_MS`): what a lease
/// still live when a batch took the Store is extended to, from now.
const MATERIALIZATION_LEASE_MS: i64 = 300_000;

fn wall_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// What completes a materialization journal (`completing`).
pub(crate) struct MaterializationCompletion {
    pub(crate) leases: Vec<super::recovery::ResourceLease>,
    pub(crate) evidence: serde_json::Value,
    pub(crate) path_results: Vec<super::recovery::RecoveryPathResult>,
}

/// A completion and the lease clock it reads when it runs, inside the
/// batch's transaction (council s2 r3).
type Completing<'c> = Option<(&'c MaterializationCompletion, &'c dyn Fn() -> Result<i64>)>;

struct NoCasFaults;
impl CasFaultInjector for NoCasFaults {
    fn check(&self, _point: CasFaultPoint) -> std::result::Result<(), String> {
        Ok(())
    }
}

impl NativeScmRepository {
    /// Filesystem-only optimistic guard, executed with the exclusive Store held
    /// before CAS installation and immediately before COMMIT. It must not reopen
    /// this Store or acquire another Store capability.
    pub fn apply_batch_guarded(
        &mut self,
        batch: &WriteBatch,
        guard: impl FnMut() -> Result<()>,
    ) -> Result<ApplyBatchOutcome> {
        self.apply_batch_with_chunks_guarded(
            batch,
            &[],
            |_| unreachable!("no external chunks"),
            guard,
            SHORT_WAIT,
            None,
        )
    }

    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            faults: no_faults(),
            cas_faults: Arc::new(NoCasFaults),
            materialization_permit: None,
            expected_identity: None,
            expected_metadata_identity: None,
            expected_store_id: None,
            acquisition_budget: ORDINARY_WAIT,
        }
    }

    pub fn with_fault_injector(
        root: impl AsRef<Path>,
        faults: Arc<dyn RepositoryFaultInjector>,
    ) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            faults,
            cas_faults: Arc::new(NoCasFaults),
            materialization_permit: None,
            expected_identity: None,
            expected_metadata_identity: None,
            expected_store_id: None,
            acquisition_budget: ORDINARY_WAIT,
        }
    }

    pub fn with_fault_injectors(
        root: impl AsRef<Path>,
        faults: Arc<dyn RepositoryFaultInjector>,
        cas_faults: Arc<dyn CasFaultInjector>,
    ) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            faults,
            cas_faults,
            materialization_permit: None,
            expected_identity: None,
            expected_metadata_identity: None,
            expected_store_id: None,
            acquisition_budget: ORDINARY_WAIT,
        }
    }

    pub fn with_cas_fault_injector(
        root: impl AsRef<Path>,
        cas_faults: Arc<dyn CasFaultInjector>,
    ) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            faults: no_faults(),
            cas_faults,
            materialization_permit: None,
            expected_identity: None,
            expected_metadata_identity: None,
            expected_store_id: None,
            acquisition_budget: ORDINARY_WAIT,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_store_identity(mut self, identity: impl Into<String>) -> Self {
        self.expected_identity = Some(identity.into());
        self
    }
    pub fn with_metadata_identity(mut self, identity: impl Into<String>) -> Self {
        self.expected_metadata_identity = Some(identity.into());
        self
    }

    pub fn with_store_id(mut self, store_id: StoreId) -> Self {
        self.expected_store_id = Some(store_id);
        self
    }

    fn validate_store_id(&self, store_id: &StoreId) -> Result<()> {
        if self
            .expected_store_id
            .as_ref()
            .is_some_and(|expected| expected != store_id)
        {
            return Err(native_backend("Store identity changed"));
        }
        Ok(())
    }

    pub(crate) fn for_materialization(root: &Path, recovery_id: &str, owner: &str) -> Self {
        let mut repository = Self::new(root).with_short_acquisition();
        repository.materialization_permit = Some((recovery_id.into(), owner.into()));
        repository
    }

    /// `apply_batch_with_chunks` that also completes the permit's journal in
    /// the batch's own transaction: the new metadata and the completed
    /// journal commit together, or neither. `clock` is the lease clock, read
    /// when the completion runs.
    pub(crate) fn apply_batch_completing(
        &mut self,
        batch: &WriteBatch,
        chunks: &[(ChunkId, u64)],
        read_chunk: impl FnMut(&ChunkId) -> Result<Vec<u8>>,
        completion: &MaterializationCompletion,
        clock: &dyn Fn() -> Result<i64>,
    ) -> Result<ApplyBatchOutcome> {
        self.apply_batch_with_chunks_guarded(
            batch,
            chunks,
            read_chunk,
            || Ok(()),
            self.acquisition_budget,
            Some((completion, clock)),
        )
    }

    /// `apply_batch_completing` on the exclusive capability `store` that the
    /// caller has held since it validated `batch` and committed its journal
    /// on `writer` (council s10, Q1: one held session for a small
    /// materialization). The batch's admission is not validated again: no
    /// other writer could run since. `locked_at_ms` is when `store` was
    /// taken. Ends with the checked checkpoint, as every writer session.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_batch_completing_held(
        &mut self,
        store: &super::NativeStoreExclusive,
        writer: Connection,
        locked_at_ms: i64,
        batch: &WriteBatch,
        chunks: &[(ChunkId, u64)],
        read_chunk: impl FnMut(&ChunkId) -> Result<Vec<u8>>,
        completion: &MaterializationCompletion,
        clock: &dyn Fn() -> Result<i64>,
    ) -> Result<ApplyBatchOutcome> {
        Self::preflight_batch(batch, chunks)?;
        self.validate_store_id(store.store_id())?;
        self.apply_on_store(
            store,
            Some(writer),
            locked_at_ms,
            batch,
            chunks,
            read_chunk,
            || Ok(()),
            Some((completion, clock)),
        )
    }

    pub(crate) fn with_short_acquisition(mut self) -> Self {
        self.acquisition_budget = SHORT_WAIT;
        self
    }

    fn hit(&self, point: RepositoryFaultPoint) -> Result<()> {
        self.faults
            .check(point)
            .map_err(|message| StoreError::Backend {
                message: format!("injected repository fault at {point:?}: {message}"),
            })
    }

    fn with_reader<T>(&self, f: impl FnOnce(&NativeScmReader<'_>) -> Result<T>) -> Result<T> {
        let store = open_read_retry(&self.root)?;
        let store = match &self.expected_identity {
            Some(identity) => store
                .with_filesystem_identity(identity)
                .map_err(native_backend)?,
            None => store,
        };
        let store = match &self.expected_metadata_identity {
            Some(identity) => store
                .with_metadata_identity(identity)
                .map_err(native_backend)?,
            None => store,
        };
        self.validate_store_id(store.store_id())?;
        let reader = NativeScmReader::new(&store)?;
        f(&reader)
    }
}

impl ScmRepository for NativeScmRepository {
    fn apply_batch(&mut self, batch: &WriteBatch) -> Result<ApplyBatchOutcome> {
        self.apply_batch_with_chunks(batch, &[], |_| {
            unreachable!("an empty external chunk set has no content callback")
        })
    }
}

impl NativeScmRepository {
    /// Atomically admits metadata and separately prepared, locally readable chunks.
    /// The callback MUST perform local I/O only: one exclusive Store capability
    /// covers all staging and SQL validation/commit. No network may occur here.
    /// Every chunk identity must be bound into the batch digest by an admission.
    /// Failures may leave verified unreferenced CAS files, never partial metadata.
    pub fn apply_batch_with_chunks(
        &mut self,
        batch: &WriteBatch,
        chunks: &[(ChunkId, u64)],
        read_chunk: impl FnMut(&ChunkId) -> Result<Vec<u8>>,
    ) -> Result<ApplyBatchOutcome> {
        self.apply_batch_with_chunks_guarded(
            batch,
            chunks,
            read_chunk,
            || Ok(()),
            self.acquisition_budget,
            None,
        )
    }

    fn apply_batch_with_chunks_guarded(
        &mut self,
        batch: &WriteBatch,
        chunks: &[(ChunkId, u64)],
        read_chunk: impl FnMut(&ChunkId) -> Result<Vec<u8>>,
        mut guard: impl FnMut() -> Result<()>,
        acquisition_budget: Duration,
        completing: Completing<'_>,
    ) -> Result<ApplyBatchOutcome> {
        Self::preflight_batch(batch, chunks)?;
        let store = acquire("repository acceptance", acquisition_budget, || {
            open_exclusive(&self.root)
        })
        .map_err(native_backend)?;
        // From here until COMMIT no other process can take this Store, so
        // none can claim a materialization lease: one valid now stays ours
        // however long the import below takes (a receive of any size).
        let locked_at_ms = wall_time_ms();
        let store = match &self.expected_identity {
            Some(identity) => store
                .with_filesystem_identity(identity)
                .map_err(native_backend)?,
            None => store,
        };
        let store = match &self.expected_metadata_identity {
            Some(identity) => store
                .with_metadata_identity(identity)
                .map_err(native_backend)?,
            None => store,
        };
        self.validate_store_id(store.store_id())?;
        guard()?;
        self.apply_on_store(
            &store,
            None,
            locked_at_ms,
            batch,
            chunks,
            read_chunk,
            guard,
            completing,
        )
    }

    /// Checks a batch before any Store is taken: budgets, streaming chunk
    /// descriptors, and the digest rebuilt through the public constructor
    /// (ordering, duplicate targets, publication coupling, canonical JSON).
    fn preflight_batch(batch: &WriteBatch, chunks: &[(ChunkId, u64)]) -> Result<()> {
        super::admission::preflight_batch_budgets(batch)?;
        streaming::preflight(batch, chunks)?;
        let verified = WriteBatch::new(
            batch.transaction_id().clone(),
            batch.canonical_objects().to_vec(),
            batch.entity_records().to_vec(),
            batch.admissions().to_vec(),
            batch.mutations().to_vec(),
        )?;
        if verified.request_digest() != batch.request_digest() {
            return Err(StoreError::Corruption {
                message: "WriteBatch digest changed after construction".into(),
            });
        }
        Ok(())
    }

    /// The batch applied under the exclusive capability `store`, taken at
    /// `locked_at_ms`. `held`: the writer of a held session (council s10,
    /// Q1), which already validated the batch's admission and holds a
    /// committed journal; else a writer is opened here and the batch is
    /// validated in its transaction.
    #[allow(clippy::too_many_arguments)]
    fn apply_on_store(
        &mut self,
        store: &super::NativeStoreExclusive,
        held: Option<Connection>,
        locked_at_ms: i64,
        batch: &WriteBatch,
        chunks: &[(ChunkId, u64)],
        mut read_chunk: impl FnMut(&ChunkId) -> Result<Vec<u8>>,
        mut guard: impl FnMut() -> Result<()>,
        completing: Completing<'_>,
    ) -> Result<ApplyBatchOutcome> {
        let validated = held.is_some();
        // Reject an already-stale new request before installing any bytes. The
        // immutable reader uses this same exclusive capability, never another
        // Store lock (a held session reads through its own writer: its WAL
        // holds the journal). Any existing receipt preserves the original
        // CAS-before-replay path, including digest collisions and corrupt
        // receipts.
        store.verify_filesystem_identity().map_err(native_backend)?;
        let held = match held {
            Some(writer) => {
                let writer = Mutex::new(writer);
                Self::precheck_new(&writer, batch)?;
                Some(writer.into_inner().map_err(|_| poisoned())?)
            }
            None => {
                let connection =
                    super::sqlite::open_read_only(store.layout().sqlite_path(), SHORT_WAIT)
                        .map_err(schema_backend)?;
                super::sqlite::validate_store_identity(&connection, store.store_id())
                    .map_err(schema_backend)?;
                Self::precheck_new(&Mutex::new(connection), batch)?;
                None
            }
        };
        store.verify_filesystem_identity().map_err(native_backend)?;
        let cas = match &held {
            Some(writer) => {
                NativeCasWriter::with_session_writer(store, Arc::clone(&self.cas_faults), writer)
            }
            None => NativeCasWriter::with_fault_injector(store, Arc::clone(&self.cas_faults)),
        }
        .map_err(cas_backend)?;
        // A chunk a `whole` manifest of the batch names, or whose registered
        // record is whole, is checked in the file domain and planned as its
        // File's record.
        let whole = whole_external_chunks(
            batch,
            chunks,
            cas.compact_catalog().filter(|_| store.whole_files()),
        );
        let compact = store.compact();
        if compact {
            plan_file_records(&cas, batch, chunks, &whole)?;
        }
        let mut installed = Vec::with_capacity(batch.canonical_objects().len() + chunks.len());
        // External chunks are read and checked here in order, then every
        // object is installed as by `put`, concurrently; outcomes and fault
        // boundaries are consumed in the original order.
        let mut external = chunks.iter();
        let mut objects = batch.canonical_objects().iter();
        let outcomes = cas.put_ordered(|| {
            if let Some((id, length)) = external.next() {
                let checked = (|| {
                    let bytes = read_chunk(id)?;
                    if bytes.len() as u64 != *length {
                        return Err(StoreError::Corruption {
                            message: "external chunk length differs from its descriptor".into(),
                        });
                    }
                    let chunk =
                        Chunk::verified(id, bytes, whole.contains(id)).map_err(|error| {
                            StoreError::Corruption {
                                message: format!("invalid external chunk: {error}"),
                            }
                        })?;
                    Ok(PutItem::Owned(CanonicalObject::ChunkRaw(chunk)))
                })();
                return Some(checked);
            }
            objects.next().map(|object| Ok(PutItem::Borrowed(object)))
        })?;
        for outcome in outcomes {
            installed.push(outcome.map_err(cas_backend)?);
            self.hit(RepositoryFaultPoint::AfterEachCas)?;
        }
        // Format 6: the pack and loose objects just published and made
        // durable; SQLite registers them in the batch transaction below.
        let publication = if compact {
            Some(cas.take_compact_publication().map_err(cas_backend)?)
        } else {
            None
        };
        self.hit(RepositoryFaultPoint::AfterAllCas)?;

        // Ends with a checked checkpoint below (durable before the caller
        // acts on the commit).
        let connection = match held {
            Some(writer) => writer,
            None => store.begin_session_writer().map_err(schema_backend)?,
        };
        self.hit(RepositoryFaultPoint::AfterWriterOpen)?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(sql_backend)?;
        self.hit(RepositoryFaultPoint::AfterBeginImmediate)?;
        let connection = Mutex::new(connection);

        let result = self
            .apply_inside_transaction(
                &connection,
                &cas,
                store.layout().root(),
                batch,
                &installed,
                chunks.len(),
                publication.as_ref(),
                completing,
                locked_at_ms,
                &super::admission::memo_scope_of(store.store_id(), store.layout().root()),
                validated,
            )
            .and_then(|outcome| {
                store.verify_filesystem_identity().map_err(native_backend)?;
                guard()?;
                Ok(outcome)
            });
        match result {
            Ok(outcome) => {
                store.verify_filesystem_identity().map_err(native_backend)?;
                let guard = connection.lock().map_err(|_| poisoned())?;
                guard.execute_batch("COMMIT").map_err(sql_backend)?;
                drop(guard);
                self.hit(RepositoryFaultPoint::AfterCommit)?;
                // The commit is durable once this checked checkpoint
                // returns, before the caller acts on it.
                super::sqlite::finish_writer(
                    connection.into_inner().map_err(|_| poisoned())?,
                    store.layout().sqlite_path(),
                )
                .map_err(schema_backend)?;
                // Best effort, still under the exclusive lock: a failed
                // consolidation changes nothing (ADR 0028, section 6). Due
                // is decided from this batch's catalog (no reopening).
                let added = publication
                    .as_ref()
                    .filter(|publication| publication.pack.is_some())
                    .and_then(|publication| {
                        publication
                            .stats
                            .map(|stats| (publication.inline.is_some(), stats))
                    });
                let due = compact
                    && cas.compact_catalog().is_some_and(|catalog| {
                        catalog.consolidation_due(added, cas.consolidation_budget())
                    });
                // Else a group only maintenance may write: started detached
                // by the CLI once its command is done (council s17, B2c).
                if compact
                    && !due
                    && cas.compact_catalog().is_some_and(|catalog| {
                        super::consolidation::maintenance_due(store, catalog)
                    })
                {
                    super::consolidation::note_maintenance_due(store.layout().root());
                }
                drop(cas);
                if due {
                    let after_commit = || {
                        self.hit(RepositoryFaultPoint::AfterConsolidationCommit)
                            .map_err(|error| error.to_string())
                    };
                    if super::consolidation::consolidate_if_due(
                        store,
                        Arc::clone(&self.cas_faults),
                        &after_commit,
                    )
                    .is_err()
                    {
                        crate::perf::count("store.consolidation_failed", 1);
                    }
                }
                Ok(outcome)
            }
            Err(error) => {
                if let Ok(guard) = connection.lock() {
                    let _ = guard.execute_batch("ROLLBACK");
                }
                Err(error)
            }
        }
    }
}

impl NativeScmRepository {
    #[allow(clippy::too_many_arguments)]
    fn apply_inside_transaction(
        &self,
        connection: &Mutex<Connection>,
        cas: &NativeCasWriter<'_>,
        store_root: &Path,
        batch: &WriteBatch,
        installed: &[super::InstallOutcome],
        external_chunks: usize,
        publication: Option<&super::cas::CompactPublication>,
        completing: Completing<'_>,
        locked_at_ms: i64,
        memo_scope: &str,
        validated: bool,
    ) -> Result<ApplyBatchOutcome> {
        // CAS publication intentionally precedes replay inspection. Therefore a
        // transaction-ID/digest mismatch may leave verified, unreferenced CAS
        // objects supplied by the losing request. This is safe by contract:
        // no metadata points at them and normal reachability GC may remove them.
        if let Some(outcome) = replay(connection, batch)? {
            return Ok(outcome);
        }
        self.hit(RepositoryFaultPoint::AfterReplayCheck)?;

        self.check_materializations(connection, batch, locked_at_ms)?;

        precheck_mutations(connection, batch)?;
        // These rows are visible only inside this transaction. The admission
        // resolver can read and rehash streamed chunks without retaining their
        // bytes in WriteBatch; any later failure rolls back all these rows.
        if let Some(publication) = publication {
            // Format 6: the batch's pack and loose objects are registered
            // (present) before validation, as format 5 inserts the object
            // rows of streamed chunks; admission bits come after it.
            let guard = connection.lock().map_err(|_| poisoned())?;
            register_compact(&guard, publication)?;
        } else if external_chunks != 0 {
            let guard = connection.lock().map_err(|_| poisoned())?;
            insert_object_rows(&guard, store_root, &installed[..external_chunks])?;
        }
        let validation = crate::perf::span("batch.validate");
        // A writer's reads: positive evidence only (council s17, B1a).
        let reader = NativeTransactionalScmReader::new(connection, cas)?.with_positive_evidence();
        if publication.is_some() {
            // Objects this batch just wrote (`existed == false`) had no
            // registered copy to disagree with; rereading them from the new
            // pack would only reread the batch.
            let new: std::collections::HashSet<&StoreObjectId> = installed
                .iter()
                .filter(|outcome| !outcome.existed)
                .map(|outcome| &outcome.object_id)
                .collect();
            ensure_immutables_except(&reader, batch, &new)?;
        } else {
            ensure_immutables(&reader, batch)?;
        }
        // A held session validated this batch under the same exclusive
        // capability, before its journal (council s10, Q1).
        if !validated {
            let resolver = CompositeAdmissionResolver::new(&reader, batch)?
                .with_memo_scope(memo_scope.to_owned());
            resolver.validate_batch()?;
        }
        drop(reader);
        drop(validation);
        self.hit(RepositoryFaultPoint::AfterValidation)?;

        let rows = crate::perf::span("batch.insert_rows");
        let guard = connection.lock().map_err(|_| poisoned())?;
        if publication.is_some() {
            insert_object_rows_compact(&guard, installed)?;
        } else {
            insert_object_rows(&guard, store_root, installed)?;
        }
        insert_object_projections_first(&guard, batch, publication.is_some())?;
        self.hit(RepositoryFaultPoint::AfterObjects)?;
        insert_entities(&guard, batch)?;
        insert_object_projections_views(&guard, batch)?;
        self.hit(RepositoryFaultPoint::AfterEntitiesAndProjections)?;
        if publication.is_some() {
            let catalog =
                super::cas::RegisteredCatalog::load(&guard, cas.reader()).map_err(cas_backend)?;
            insert_admissions_compact(&guard, batch, &catalog)?;
        } else {
            insert_admissions(&guard, batch)?;
        }
        self.hit(RepositoryFaultPoint::AfterAdmissions)?;
        apply_mutations(&guard, batch, self)?;
        drop(rows);
        let gc_roots = crate::perf::span("batch.rebuild_gc_roots");
        rebuild_gc_roots(&guard)?;
        drop(gc_roots);
        self.hit(RepositoryFaultPoint::BeforeReceipt)?;
        let receipt = ApplyBatchReceipt::new(batch);
        let receipt_json = canonical_receipt_json(&receipt).map_err(schema_backend)?;
        guard.execute(
            "INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![batch.transaction_id().as_str(), batch.request_digest().to_string(), receipt_json],
        ).map_err(sql_backend)?;
        if let Some((completion, clock)) = completing {
            let (id, owner) = self
                .materialization_permit
                .as_ref()
                .ok_or_else(|| native_backend("a journal completion needs a permit"))?;
            let id = super::recovery::RecoveryId::parse(id.clone())
                .map_err(|error| native_backend(error.to_string()))?;
            // The lease clock read now, inside the transaction: a lease that
            // expired while the batch was validated is refused.
            let now_ms = clock()?;
            super::recovery::complete_in_transaction(
                &guard,
                &id,
                owner,
                &completion.leases,
                completion.evidence.clone(),
                &completion.path_results,
                now_ms,
            )
            .map_err(|error| native_backend(error.to_string()))?;
        }
        self.hit(RepositoryFaultPoint::AfterReceiptBeforeCommit)?;
        Ok(ApplyBatchOutcome::Applied {
            request_digest: batch.request_digest(),
        })
    }
}

impl NativeScmRepository {
    /// Before any byte is installed: a batch with no receipt yet must still
    /// match every compare-and-swap target (a stale request installs
    /// nothing).
    fn precheck_new(connection: &Mutex<Connection>, batch: &WriteBatch) -> Result<()> {
        let has_receipt: bool = connection
            .lock()
            .map_err(|_| poisoned())?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM applied_transactions WHERE transaction_id=?1)",
                [batch.transaction_id().as_str()],
                |row| row.get(0),
            )
            .map_err(sql_backend)?;
        if !has_receipt {
            precheck_mutations(connection, batch)?;
        }
        Ok(())
    }

    /// A pending materialization of a Worktree this batch moves must be ours
    /// (permit, owner, `applying`, digest) with every lease live when this
    /// batch took the exclusive Store (`locked_at_ms`). Those leases are then
    /// extended from now: nothing could claim them since, and the journal
    /// completion later in this transaction checks them at its own time.
    fn check_materializations(
        &self,
        connection: &Mutex<Connection>,
        batch: &WriteBatch,
        locked_at_ms: i64,
    ) -> Result<()> {
        let guard = connection.lock().map_err(|_| poisoned())?;
        for mutation in batch.mutations() {
            let StoreMutation::WorktreeActiveOperation(change) = mutation else {
                continue;
            };
            let mut query = guard.prepare("SELECT recovery_operation_id,state,owner_token,json_extract(payload_json,'$.batchDigest') FROM recovery_operations WHERE worktree_id=?1 AND recovery_type IN ('materialize','restore') AND state <> 'completed'").map_err(sql_backend)?;
            let rows = query
                .query_map([change.worktree_id().as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(sql_backend)?;
            for row in rows {
                let (id, state, owner, digest) = row.map_err(sql_backend)?;
                let permitted =
                    self.materialization_permit
                        .as_ref()
                        .is_some_and(|(permit_id, token)| {
                            permit_id == &id && owner.as_ref() == Some(token)
                        })
                        && state == "applying"
                        && digest.as_deref() == Some(batch.request_digest().to_string().as_str());
                let (total, expired): (i64,i64) = guard.query_row("SELECT count(*),COALESCE(sum(CASE WHEN released_at_ms IS NOT NULL OR expires_at_ms <= ?2 THEN 1 ELSE 0 END),0) FROM materialization_leases WHERE recovery_operation_id=?1", params![&id, locked_at_ms], |r| Ok((r.get(0)?,r.get(1)?))).map_err(sql_backend)?;
                if !permitted || total == 0 || expired != 0 {
                    return Err(StoreError::Backend {
                        message: format!("worktree_needs_repair: pending materialization {id}"),
                    });
                }
                guard
                    .execute(
                        "UPDATE materialization_leases SET expires_at_ms = MAX(expires_at_ms, ?2) \
                         WHERE recovery_operation_id = ?1 AND released_at_ms IS NULL",
                        params![&id, wall_time_ms().saturating_add(MATERIALIZATION_LEASE_MS)],
                    )
                    .map_err(sql_backend)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn precheck_mutations(connection: &Mutex<Connection>, batch: &WriteBatch) -> Result<()> {
    let guard = connection.lock().map_err(|_| poisoned())?;
    for mutation in batch.mutations() {
        match mutation {
            StoreMutation::RepositoryView(cas) => {
                let current: Option<(u64, String)> = guard
                    .query_row(
                        "SELECT generation,repository_view_id FROM current_repository_views WHERE space_id=?1",
                        [cas.space_id().as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(sql_backend)?;
                let matches = current
                    .as_ref()
                    .map(|(generation, view_id)| {
                        *generation == cas.expected_generation()
                            && cas
                                .expected_view_id()
                                .is_some_and(|id| id.as_str() == view_id)
                    })
                    .unwrap_or(cas.expected_generation() == 0 && cas.expected_view_id().is_none());
                if !matches {
                    return Err(StoreError::CasMismatch {
                        target: format!("repository_view:{}", cas.space_id()),
                    });
                }
            }
            StoreMutation::OperationHeads(cas) => {
                let mut statement = guard
                    .prepare("SELECT operation_id FROM operation_heads WHERE space_id=?1 ORDER BY ordinal")
                    .map_err(sql_backend)?;
                let actual = statement
                    .query_map([cas.space_id().as_str()], |row| row.get::<_, String>(0))
                    .map_err(sql_backend)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(sql_backend)?;
                if actual
                    != cas
                        .expected_head_ids()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                {
                    return Err(StoreError::CasMismatch {
                        target: format!("operation_heads:{}", cas.space_id()),
                    });
                }
            }
            StoreMutation::WorktreeActiveOperation(cas) => {
                let actual: Option<String> = guard
                    .query_row(
                        "SELECT active_operation_id FROM worktrees WHERE worktree_id=?1",
                        [cas.worktree_id().as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sql_backend)?;
                if actual.as_deref() != Some(cas.expected_operation_id().as_str()) {
                    return Err(StoreError::CasMismatch {
                        target: format!("worktree_active_operation:{}", cas.worktree_id()),
                    });
                }
            }
        }
    }
    Ok(())
}

fn replay(connection: &Mutex<Connection>, batch: &WriteBatch) -> Result<Option<ApplyBatchOutcome>> {
    let guard = connection.lock().map_err(|_| poisoned())?;
    let row: Option<(String, String)> = guard
        .query_row(
            "SELECT request_digest,receipt_json FROM applied_transactions WHERE transaction_id=?1",
            [batch.transaction_id().as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_backend)?;
    let Some((digest, receipt_json)) = row else {
        return Ok(None);
    };
    if digest != batch.request_digest().to_string() {
        return Err(StoreError::TransactionReplayMismatch {
            transaction_id: batch.transaction_id().clone(),
        });
    }
    let value = validate_canonical_receipt_json(&receipt_json).map_err(schema_backend)?;
    let receipt: ApplyBatchReceipt =
        serde_json::from_value(value).map_err(|error| StoreError::Corruption {
            message: format!("invalid typed applied transaction receipt: {error}"),
        })?;
    receipt.validate(batch.transaction_id(), batch.request_digest(), batch)?;
    Ok(Some(ApplyBatchOutcome::AlreadyApplied {
        request_digest: batch.request_digest(),
    }))
}

fn ensure_immutables(reader: &dyn ScmReader, batch: &WriteBatch) -> Result<()> {
    ensure_immutables_except(reader, batch, &std::collections::HashSet::new())
}

fn ensure_immutables_except(
    reader: &dyn ScmReader,
    batch: &WriteBatch,
    new: &std::collections::HashSet<&StoreObjectId>,
) -> Result<()> {
    for object in batch.canonical_objects() {
        if !new.is_empty() && new.contains(&object.object_id()) {
            continue;
        }
        let differs = match object {
            CanonicalObject::ChunkRaw(v) => reader.get_chunk(v.id())?.is_some_and(|x| x != *v),
            CanonicalObject::FileManifest(v) => {
                reader.get_file_manifest(v.id())?.is_some_and(|x| x != *v)
            }
            CanonicalObject::Tree(v) => reader.get_tree(v.id())?.is_some_and(|x| x != *v),
            CanonicalObject::Symlink(v) => reader.get_symlink(v.id())?.is_some_and(|x| x != *v),
            CanonicalObject::Tombstone(v) => reader.get_tombstone(v.id())?.is_some_and(|x| x != *v),
            CanonicalObject::Step(v) => reader.get_step(v.id())?.is_some_and(|x| x != *v),
            CanonicalObject::RepositoryView(v) => {
                reader.get_repository_view(v.id())?.is_some_and(|x| x != *v)
            }
            CanonicalObject::Conflict(v) => reader.get_conflict(v.id())?.is_some_and(|x| x != *v),
            CanonicalObject::Product(v) => reader.get_product(v.id())?.is_some_and(|x| x != *v),
        };
        if differs {
            return Err(StoreError::Corruption {
                message: format!(
                    "immutable object {} has conflicting payload",
                    object.object_id()
                ),
            });
        }
    }
    for entity in batch.entity_records() {
        let differs = match entity {
            EntityRecord::Space(v) => reader.get_space(v.id())?.is_some_and(|x| x != *v),
            EntityRecord::Layer(v) => reader.get_layer(v.id())?.is_some_and(|x| x != *v),
            EntityRecord::StepObservation(v) => reader
                .get_step_observation(v.id())?
                .is_some_and(|x| x != *v),
            EntityRecord::Operation(v) => reader.get_operation(v.id())?.is_some_and(|x| x != *v),
            EntityRecord::Worktree(v) => reader.get_worktree(v.id())?.is_some_and(|x| x != *v),
            EntityRecord::Marker(v) => reader.get_marker(v.id())?.is_some_and(|x| x != *v),
            EntityRecord::Release(v) => reader.get_release(v.id())?.is_some_and(|x| x != *v),
            EntityRecord::ReleasePublication(v) => reader
                .get_release_publication(v.id())?
                .is_some_and(|x| x != *v),
        };
        if differs {
            return Err(StoreError::Corruption {
                message: format!(
                    "immutable entity {} has conflicting payload",
                    entity.id_str()
                ),
            });
        }
    }
    Ok(())
}

fn insert_object_rows(
    connection: &Connection,
    store_root: &Path,
    installed: &[super::InstallOutcome],
) -> Result<()> {
    for outcome in installed {
        let relative = outcome
            .path
            .strip_prefix(store_root)
            .map_err(|_| StoreError::Corruption {
                message: format!(
                    "installed CAS path {} escapes Store root {}",
                    outcome.path.display(),
                    store_root.display()
                ),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let inserted = connection.execute(
            "INSERT OR IGNORE INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,?2,?3,?4,1,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![outcome.object_id.as_str(), kind_name(outcome.object_id.kind()), relative, outcome.bytes_len],
        ).map_err(sql_backend)?;
        if inserted == 0 {
            // An existing row must describe exactly this object: a different
            // kind, path or length is never silently kept.
            let existing: (String, String, i64, i64) = connection
                .query_row(
                    "SELECT object_kind,storage_path,byte_length,verified FROM objects WHERE object_id=?1",
                    params![outcome.object_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(sql_backend)?;
            let same = existing.0 == kind_name(outcome.object_id.kind())
                && existing.1.replace('\\', "/") == relative
                && u64::try_from(existing.2).ok() == Some(outcome.bytes_len)
                && existing.3 == 1;
            if !same {
                return Err(StoreError::Corruption {
                    message: format!(
                        "objects row for {} differs from the installed object",
                        outcome.object_id
                    ),
                });
            }
        }
    }
    Ok(())
}

fn insert_object_projections_first(
    connection: &Connection,
    batch: &WriteBatch,
    compact: bool,
) -> Result<()> {
    for object in batch.canonical_objects() {
        match object {
            // Format 6: no file_* projections and no manifest_chunk edges;
            // the FileId section and the manifest itself answer them.
            CanonicalObject::FileManifest(_) if compact => {}
            CanonicalObject::Product(value) => {
                connection.execute("INSERT OR IGNORE INTO products(product_object_id,product_id) VALUES(?1,?2)",params![value.id().as_str(),value.envelope().logical_id()]).map_err(sql_backend)?;
                for (index, dependency) in value.direct_dependencies().iter().enumerate() {
                    edge(
                        connection,
                        value.id().as_str(),
                        "product_dependency",
                        index,
                        dependency.as_str(),
                    )?;
                }
            }
            CanonicalObject::FileManifest(v) => {
                connection.execute("INSERT OR IGNORE INTO file_availability(file_id,verified_at_ms) VALUES(?1,CAST(unixepoch('subsec')*1000 AS INTEGER))", [v.file_id().as_str()]).map_err(sql_backend)?;
                connection.execute("INSERT OR IGNORE INTO file_manifests(manifest_id,file_id,file_byte_length,chunking_json) VALUES(?1,?2,?3,?4)", params![v.id().as_str(),v.file_id().as_str(),v.byte_length(),json(v.chunking())?]).map_err(sql_backend)?;
                for (i, c) in v.chunks().iter().enumerate() {
                    connection.execute("INSERT OR IGNORE INTO file_manifest_chunks(manifest_id,ordinal,chunk_id,byte_offset,byte_length) VALUES(?1,?2,?3,?4,?5)",params![v.id().as_str(),i,c.chunk_id().as_str(),c.offset(),c.byte_length()]).map_err(sql_backend)?;
                    edge(
                        connection,
                        v.id().as_str(),
                        "manifest_chunk",
                        i,
                        c.chunk_id().as_str(),
                    )?;
                }
            }
            CanonicalObject::Tree(v) => {
                for (i, e) in v.entries().iter().enumerate() {
                    let target = e.target();
                    let id = target
                        .tree_id()
                        .map(ToString::to_string)
                        .or_else(|| target.conflict_id().map(ToString::to_string))
                        .or_else(|| target.symlink_id().map(ToString::to_string))
                        .or_else(|| target.tombstone_id().map(ToString::to_string));
                    if let Some(id) = id {
                        edge(connection, v.id().as_str(), "tree_entry", i, &id)?;
                    }
                }
            }
            CanonicalObject::Step(v) => {
                connection.execute("INSERT OR IGNORE INTO steps(step_id,root_tree_id,parent_count) VALUES(?1,?2,?3)",params![v.id().as_str(),v.root_tree_id().as_str(),v.parent_step_ids().len()]).map_err(sql_backend)?;
                for (i, p) in v.parent_step_ids().iter().enumerate() {
                    connection.execute("INSERT OR IGNORE INTO step_parents(step_id,parent_step_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),p.as_str(),i]).map_err(sql_backend)?;
                    edge(connection, v.id().as_str(), "step_parent", i, p.as_str())?;
                }
                edge(
                    connection,
                    v.id().as_str(),
                    "step_tree",
                    0,
                    v.root_tree_id().as_str(),
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn insert_entities(connection: &Connection, batch: &WriteBatch) -> Result<()> {
    for e in batch
        .entity_records()
        .iter()
        .filter(|e| matches!(e, EntityRecord::Space(_)))
    {
        insert_entity(connection, e)?;
    }
    for e in batch
        .entity_records()
        .iter()
        .filter(|e| matches!(e, EntityRecord::Layer(_)))
    {
        insert_entity(connection, e)?;
    }
    for object in batch.canonical_objects() {
        if let CanonicalObject::RepositoryView(v) = object {
            connection.execute("INSERT OR IGNORE INTO repository_views(repository_view_id,space_id) VALUES(?1,?2)",params![v.id().as_str(),v.space_id().as_str()]).map_err(sql_backend)?;
        }
    }
    for kind in 0..6 {
        for e in batch.entity_records() {
            let order = match e {
                EntityRecord::Operation(_) => 0,
                EntityRecord::Worktree(_) => 1,
                EntityRecord::Marker(_) => 2,
                EntityRecord::Release(_) => 3,
                EntityRecord::ReleasePublication(_) => 4,
                EntityRecord::StepObservation(_) => 5,
                _ => 99,
            };
            if order == kind {
                insert_entity(connection, e)?;
            }
        }
    }
    Ok(())
}

fn insert_entity(c: &Connection, e: &EntityRecord) -> Result<()> {
    match e {
        EntityRecord::Space(v) => {
            let created_at_ms =
                rfc3339_to_epoch_millis(v.created_at().as_str()).map_err(schema_backend)?;
            c.execute("INSERT OR IGNORE INTO spaces(space_id,payload_json,created_at_ms) VALUES(?1,?2,?3)",params![v.id().as_str(),json(v)?,created_at_ms]).map_err(sql_backend)?;
        }
        EntityRecord::Layer(v) => {
            let created_at_ms =
                rfc3339_to_epoch_millis(v.created_at().as_str()).map_err(schema_backend)?;
            c.execute("INSERT OR IGNORE INTO layers(layer_id,space_id,layer_kind,layer_status,payload_json,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",params![v.id().as_str(),v.space_id().as_str(),enum_name(v.kind())?,enum_name(v.status())?,json(v)?,created_at_ms]).map_err(sql_backend)?;
            for (i, p) in v.parent_layer_ids().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO layer_parents(layer_id,parent_layer_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),p.as_str(),i]).map_err(sql_backend)?;
            }
        }
        EntityRecord::Operation(v) => {
            c.execute("INSERT OR IGNORE INTO operations(operation_id,space_id,resulting_view_id,operation_kind,actor_id,device_id,created_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![v.id().as_str(),v.space_id().as_str(),v.resulting_view_id().as_str(),enum_name(v.kind())?,v.actor_id().map(|x|x.as_str()),v.device_id().map(|x|x.as_str()),v.created_at().as_str(),json(v)?]).map_err(sql_backend)?;
            for (i, p) in v.parent_operation_ids().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO operation_parents(operation_id,parent_operation_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),p.as_str(),i]).map_err(sql_backend)?;
            }
            for (i, r) in v.rewrite_map().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO operation_rewrites(operation_id,old_step_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),r.old_step_id().as_str(),i]).map_err(sql_backend)?;
                for (j, n) in r.new_step_ids().iter().enumerate() {
                    c.execute("INSERT OR IGNORE INTO operation_rewrite_targets(operation_id,old_step_id,new_step_id,ordinal) VALUES(?1,?2,?3,?4)",params![v.id().as_str(),r.old_step_id().as_str(),n.as_str(),j]).map_err(sql_backend)?;
                }
            }
        }
        EntityRecord::Worktree(v) => {
            let created_at_ms =
                rfc3339_to_epoch_millis(v.created_at().as_str()).map_err(schema_backend)?;
            c.execute("INSERT OR IGNORE INTO worktrees(worktree_id,store_id,space_id,active_operation_id,checkout_root,filesystem_identity,payload_json,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![v.id().as_str(),v.store_id().as_str(),v.space_id().as_str(),v.active_operation_id().as_str(),v.checkout_root(),v.filesystem_identity(),json(v)?,created_at_ms]).map_err(sql_backend)?;
        }
        EntityRecord::Marker(v) => {
            c.execute("INSERT OR IGNORE INTO markers(marker_id,space_id,target_step_id,marker_kind,created_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",params![v.id().as_str(),v.space_id().as_str(),v.target_step_id().as_str(),enum_name(v.kind())?,v.created_at().as_str(),json(v)?]).map_err(sql_backend)?;
            for (i, p) in v.proof_ids().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO marker_proofs(marker_id,proof_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),p.as_str(),i]).map_err(sql_backend)?;
            }
        }
        EntityRecord::Release(v) => {
            c.execute("INSERT OR IGNORE INTO releases(release_id,space_id,target_step_id,version,created_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",params![v.id().as_str(),v.space_id().as_str(),v.target_step_id().as_str(),v.version(),v.created_at().as_str(),json(v)?]).map_err(sql_backend)?;
            for (i, p) in v.proof_ids().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO release_proofs(release_id,proof_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),p.as_str(),i]).map_err(sql_backend)?;
            }
        }
        EntityRecord::ReleasePublication(v) => {
            c.execute("INSERT OR IGNORE INTO release_publications(release_publication_id,release_id,space_id,operation_id,published_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",params![v.id().as_str(),v.release_id().as_str(),v.space_id().as_str(),v.operation_id().map(|x|x.as_str()),v.published_at().as_str(),json(v)?]).map_err(sql_backend)?;
        }
        EntityRecord::StepObservation(v) => {
            c.execute("INSERT OR IGNORE INTO step_observations(observation_id,step_id,space_id,operation_id,worktree_id,actor_id,device_id,observed_at,capture_reason,payload_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![v.id().as_str(),v.step_id().as_str(),v.space_id().map(|x|x.as_str()),v.operation_id().map(|x|x.as_str()),v.worktree_id().map(|x|x.as_str()),v.actor_id().map(|x|x.as_str()),v.device_id().map(|x|x.as_str()),v.observed_at().as_str(),enum_name(v.capture_reason())?,json(v)?]).map_err(sql_backend)?;
        }
    };
    Ok(())
}

fn insert_object_projections_views(c: &Connection, b: &WriteBatch) -> Result<()> {
    for o in b.canonical_objects() {
        if let CanonicalObject::RepositoryView(v) = o {
            let mut edge_ordinal = 0_usize;
            for h in v.layer_heads() {
                for (i, s) in h.step_ids().iter().enumerate() {
                    c.execute("INSERT OR IGNORE INTO repository_view_layer_heads(repository_view_id,layer_id,step_id,ordinal) VALUES(?1,?2,?3,?4)",params![v.id().as_str(),h.layer_id().as_str(),s.as_str(),i]).map_err(sql_backend)?;
                    edge(c, v.id().as_str(), "view_step", edge_ordinal, s.as_str())?;
                    edge_ordinal += 1;
                }
            }
            for (i, m) in v.marker_ids().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO repository_view_markers(repository_view_id,marker_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),m.as_str(),i]).map_err(sql_backend)?;
            }
            for (i, r) in v.release_ids().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO repository_view_releases(repository_view_id,release_id,ordinal) VALUES(?1,?2,?3)",params![v.id().as_str(),r.as_str(),i]).map_err(sql_backend)?;
            }
            for (i, w) in v.worktrees().iter().enumerate() {
                c.execute("INSERT OR IGNORE INTO repository_view_worktrees(repository_view_id,worktree_id,active_layer_id,base_step_id,materialized_tree_id,status,ordinal) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![v.id().as_str(),w.worktree_id().as_str(),w.active_layer_id().map(|x|x.as_str()),w.base_step_id().map(|x|x.as_str()),w.materialized_tree_id().map(|x|x.as_str()),enum_name(w.status())?,i]).map_err(sql_backend)?;
            }
        }
    }
    Ok(())
}

/// The external `chunks` of `batch` in the file domain: those its whole
/// manifests name and, in a Store that keeps whole files (`catalog`), those
/// no manifest of the batch names whose registered record is whole (the
/// domain that record declares; council s16 r1, r2). Shared by every path
/// that installs external chunks.
pub(super) fn whole_external_chunks(
    batch: &WriteBatch,
    chunks: &[(ChunkId, u64)],
    catalog: Option<&std::sync::Arc<super::cas::RegisteredCatalog>>,
) -> std::collections::HashSet<ChunkId> {
    let mut whole = whole_chunk_ids(batch);
    if let Some(catalog) = catalog {
        let named: std::collections::HashSet<&ChunkId> = batch
            .canonical_objects()
            .iter()
            .filter_map(|object| match object {
                CanonicalObject::FileManifest(manifest) => Some(manifest.chunks()),
                _ => None,
            })
            .flatten()
            .map(|chunk| chunk.chunk_id())
            .collect();
        for (id, _) in chunks {
            if !named.contains(id) && catalog.whole_record(id) {
                whole.insert(id.clone());
            }
        }
    }
    whole
}

/// The chunks the `whole` manifests of `batch` name (council s13 Q5): their
/// bytes are verified in the file domain.
pub(super) fn whole_chunk_ids(batch: &WriteBatch) -> std::collections::HashSet<ChunkId> {
    batch
        .canonical_objects()
        .iter()
        .filter_map(|object| match object {
            CanonicalObject::FileManifest(manifest) if manifest.chunks_are_whole() => Some(
                manifest
                    .chunks()
                    .iter()
                    .map(|chunk| chunk.chunk_id().clone()),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Format 6 (ADR 0028): what a CAS item needs to be planned as a file
/// record, from the writer's registered snapshot.
fn plan_file_records(
    cas: &NativeCasWriter<'_>,
    batch: &WriteBatch,
    chunks: &[(ChunkId, u64)],
    whole_external: &std::collections::HashSet<ChunkId>,
) -> Result<()> {
    let catalog = cas
        .compact_catalog()
        .cloned()
        .ok_or_else(|| native_backend("format 6 CAS writer has no registered catalog"))?;
    let mut supplied: std::collections::HashSet<ChunkId> =
        chunks.iter().map(|(id, _)| id.clone()).collect();
    let mut manifests = Vec::new();
    // Files whose whole chunk the batch supplies: their file record is the
    // whole one, even without its manifest (council s16 r2), and also when
    // the chunk is external and takes its domain from its registered record
    // (council s16 r3).
    let mut whole_files: std::collections::HashSet<[u8; 32]> =
        whole_external.iter().map(ChunkId::digest_bytes).collect();
    for object in batch.canonical_objects() {
        match object {
            CanonicalObject::ChunkRaw(chunk) => {
                supplied.insert(chunk.id().clone());
                if chunk.is_whole() {
                    whole_files.insert(chunk.id().digest_bytes());
                }
            }
            CanonicalObject::FileManifest(manifest) => manifests.push(manifest),
            _ => {}
        }
    }
    let plan = super::cas::FileRecordPlan::new(&catalog, manifests, &supplied, &whole_files)
        .map_err(cas_backend)?;
    cas.plan_file_records(plan).map_err(cas_backend)
}

/// Format 6: registers the batch's pack and loose content objects. Their
/// bytes were published and made durable by `put_ordered` before this
/// transaction began (CAS before reference).
///
/// Safety rule of orphan collection (`NativeCas::collect_orphan_packs`):
/// a pack is registered only here, by the exclusive session that published
/// it, so a pack unregistered when an exclusive session starts never will
/// be and is removed. Anything that would register a pack another session
/// published (consolidation, receiving packs) must keep that rule, or give
/// orphan collection a grace period first.
fn register_compact(c: &Connection, publication: &super::cas::CompactPublication) -> Result<()> {
    if let Some((name, digest, entries)) = &publication.pack {
        let name = name
            .to_str()
            .ok_or_else(|| native_backend("pack name is not UTF-8"))?;
        c.execute(
            "INSERT INTO cas_packs(name,index_digest,entries,stored_inline,created_at_ms) VALUES(?1,?2,?3,?4,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![
                name,
                &digest[..],
                i64::try_from(*entries).map_err(native_backend)?,
                i64::from(publication.inline.is_some())
            ],
        )
        .map_err(sql_backend)?;
        let seq = c.last_insert_rowid();
        // A small pack's image commits with its registration (council s7
        // Q1): durable with the batch, no file.
        if let Some(image) = &publication.inline {
            c.execute(
                "INSERT INTO cas_pack_inline(pack_seq,image) VALUES(?1,?2)",
                params![seq, image],
            )
            .map_err(sql_backend)?;
        }
        // And its keys row (council s17, B1b).
        let keys = publication
            .keys
            .as_ref()
            .ok_or_else(|| native_backend("a registered pack has no keys row"))?;
        keys.insert(c, seq, digest).map_err(sql_backend)?;
    }
    for loose in &publication.loose {
        let id = super::cas::raw_object_digest(&loose.object_id).map_err(cas_backend)?;
        let kind = super::cas::content_kind_code(loose.object_id.kind());
        let length = i64::try_from(loose.byte_length).map_err(native_backend)?;
        let file = match &loose.file {
            Some((file_id, file_length)) => Some((
                super::cas::raw_file_digest(file_id)
                    .ok_or_else(|| native_backend("invalid FileId"))?
                    .to_vec(),
                i64::try_from(*file_length).map_err(native_backend)?,
            )),
            None => None,
        };
        let (file_id, file_length) = file.unzip();
        let inserted = c
            .execute(
                "INSERT OR IGNORE INTO loose_objects(object_id,object_kind,byte_length,file_id,file_byte_length) VALUES(?1,?2,?3,?4,?5)",
                params![&id[..], kind, length, file_id, file_length],
            )
            .map_err(sql_backend)?;
        if inserted == 0 {
            let same: bool = c
                .query_row(
                    "SELECT object_kind=?2 AND byte_length=?3 AND file_id IS ?4 AND file_byte_length IS ?5 FROM loose_objects WHERE object_id=?1",
                    params![&id[..], kind, length, file_id, file_length],
                    |row| row.get(0),
                )
                .map_err(sql_backend)?;
            if !same {
                return Err(StoreError::Corruption {
                    message: format!(
                        "loose_objects row for {} differs from the installed object",
                        loose.object_id
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Format 6: `objects` rows of the kinds that keep one (never content).
fn insert_object_rows_compact(
    connection: &Connection,
    installed: &[super::InstallOutcome],
) -> Result<()> {
    for outcome in installed {
        let kind = outcome.object_id.kind();
        if super::cas::is_content(kind) {
            continue;
        }
        let inserted = connection.execute(
            "INSERT OR IGNORE INTO objects(object_id,object_kind,byte_length,verified,created_at_ms) VALUES(?1,?2,?3,1,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![outcome.object_id.as_str(), kind_name(kind), outcome.bytes_len],
        ).map_err(sql_backend)?;
        if inserted == 0 {
            let (existing_kind, length, verified): (String, i64, i64) = connection
                .query_row(
                    "SELECT object_kind,byte_length,verified FROM objects WHERE object_id=?1",
                    params![outcome.object_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(sql_backend)?;
            if existing_kind != kind_name(kind)
                || u64::try_from(length).ok() != Some(outcome.bytes_len)
                || verified != 1
            {
                return Err(StoreError::Corruption {
                    message: format!(
                        "objects row for {} differs from the installed object",
                        outcome.object_id
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Format 6: admissions of content set the bit of each registered copy in
/// its (Space, pack) bitmap (or a loose admission row); bitmaps only grow.
/// Other kinds keep their `space_object_admissions` rows.
fn insert_admissions_compact(
    c: &Connection,
    b: &WriteBatch,
    catalog: &super::cas::RegisteredCatalog,
) -> Result<()> {
    let entries: std::collections::HashMap<i64, u64> = catalog
        .packs()
        .iter()
        .map(|pack| (pack.seq, pack.entries))
        .collect();
    let mut bitmaps: std::collections::BTreeMap<(String, i64), Vec<u8>> = Default::default();
    for a in b.admissions() {
        let object = a.object_id();
        if !super::cas::is_content(object.kind()) {
            c.execute("INSERT OR IGNORE INTO space_object_admissions(space_id,object_id,admitted_by_operation_id,admitted_at_ms) VALUES(?1,?2,NULL,CAST(unixepoch('subsec')*1000 AS INTEGER))",params![a.space_id().as_str(),object.as_str()]).map_err(sql_backend)?;
            continue;
        }
        let (ordinals, loose) = catalog.logical_ordinals(object).map_err(cas_backend)?;
        if ordinals.is_empty() && !loose {
            return Err(StoreError::Corruption {
                message: format!("admitted content {object} has no registered copy"),
            });
        }
        for (seq, ordinal) in ordinals {
            let key = (a.space_id().as_str().to_owned(), seq);
            if !bitmaps.contains_key(&key) {
                let pack_entries = *entries
                    .get(&seq)
                    .ok_or_else(|| native_backend("admitted copy in an unknown pack"))?;
                let existing: Option<Vec<u8>> = c
                    .query_row(
                        "SELECT bitmap FROM space_pack_admissions WHERE space_id=?1 AND pack_seq=?2",
                        params![key.0, seq],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sql_backend)?;
                let bitmap = match existing {
                    Some(bitmap) => {
                        super::cas::check_bitmap(&bitmap, pack_entries).map_err(|reason| {
                            StoreError::Corruption {
                                message: format!("space_pack_admissions: {reason}"),
                            }
                        })?;
                        bitmap
                    }
                    None => vec![0; pack_entries.div_ceil(8) as usize],
                };
                bitmaps.insert(key.clone(), bitmap);
            }
            if let Some(bitmap) = bitmaps.get_mut(&key) {
                super::cas::set_bit(bitmap, ordinal);
            }
        }
        if loose {
            let id = super::cas::raw_object_digest(object).map_err(cas_backend)?;
            c.execute(
                "INSERT OR IGNORE INTO space_loose_admissions(space_id,object_id) VALUES(?1,?2)",
                params![a.space_id().as_str(), &id[..]],
            )
            .map_err(sql_backend)?;
        }
    }
    for ((space, seq), bitmap) in bitmaps {
        c.execute(
            "INSERT INTO space_pack_admissions(space_id,pack_seq,bitmap) VALUES(?1,?2,?3) ON CONFLICT(space_id,pack_seq) DO UPDATE SET bitmap=excluded.bitmap",
            params![space, seq, bitmap],
        )
        .map_err(sql_backend)?;
    }
    Ok(())
}

fn insert_admissions(c: &Connection, b: &WriteBatch) -> Result<()> {
    for a in b.admissions() {
        c.execute("INSERT OR IGNORE INTO space_object_admissions(space_id,object_id,admitted_by_operation_id,admitted_at_ms) VALUES(?1,?2,NULL,CAST(unixepoch('subsec')*1000 AS INTEGER))",params![a.space_id().as_str(),a.object_id().as_str()]).map_err(sql_backend)?;
    }
    Ok(())
}

fn apply_mutations(c: &Connection, b: &WriteBatch, repo: &NativeScmRepository) -> Result<()> {
    for m in b.mutations() {
        match m {
            StoreMutation::RepositoryView(v) => {
                let current:Option<(u64,String)>=c.query_row("SELECT generation,repository_view_id FROM current_repository_views WHERE space_id=?1",[v.space_id().as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql_backend)?;
                let matches = current
                    .as_ref()
                    .map(|(g, id)| {
                        *g == v.expected_generation()
                            && v.expected_view_id().is_some_and(|x| x.as_str() == id)
                    })
                    .unwrap_or(v.expected_generation() == 0 && v.expected_view_id().is_none());
                if !matches {
                    return Err(StoreError::CasMismatch {
                        target: format!("repository_view:{}", v.space_id()),
                    });
                }
                c.execute("INSERT INTO current_repository_views(space_id,generation,repository_view_id,updated_by_transaction_id) VALUES(?1,?2,?3,?4) ON CONFLICT(space_id) DO UPDATE SET generation=excluded.generation,repository_view_id=excluded.repository_view_id,updated_by_transaction_id=excluded.updated_by_transaction_id",params![v.space_id().as_str(),v.new_generation(),v.new_view_id().as_str(),b.transaction_id().as_str()]).map_err(sql_backend)?;
                repo.hit(RepositoryFaultPoint::AfterRepositoryViewCas)?;
            }
            StoreMutation::OperationHeads(v) => {
                let mut stmt=c.prepare("SELECT operation_id FROM operation_heads WHERE space_id=?1 ORDER BY ordinal").map_err(sql_backend)?;
                let actual = stmt
                    .query_map([v.space_id().as_str()], |r| r.get::<_, String>(0))
                    .map_err(sql_backend)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(sql_backend)?;
                if actual
                    != v.expected_head_ids()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                {
                    return Err(StoreError::CasMismatch {
                        target: format!("operation_heads:{}", v.space_id()),
                    });
                }
                c.execute(
                    "DELETE FROM operation_heads WHERE space_id=?1",
                    [v.space_id().as_str()],
                )
                .map_err(sql_backend)?;
                for (i, h) in v.new_head_ids().iter().enumerate() {
                    c.execute("INSERT INTO operation_heads(space_id,operation_id,ordinal,updated_by_transaction_id) VALUES(?1,?2,?3,?4)",params![v.space_id().as_str(),h.as_str(),i,b.transaction_id().as_str()]).map_err(sql_backend)?;
                }
                repo.hit(RepositoryFaultPoint::AfterOperationHeadsCas)?;
            }
            StoreMutation::WorktreeActiveOperation(v) => {
                let payload:String=c.query_row("SELECT payload_json FROM worktrees WHERE worktree_id=?1 AND active_operation_id=?2",params![v.worktree_id().as_str(),v.expected_operation_id().as_str()],|r|r.get(0)).optional().map_err(sql_backend)?.ok_or_else(||StoreError::CasMismatch{target:format!("worktree_active_operation:{}",v.worktree_id())})?;
                let mut value: serde_json::Value =
                    serde_json::from_str(&payload).map_err(|e| StoreError::Corruption {
                        message: e.to_string(),
                    })?;
                value
                    .as_object_mut()
                    .ok_or_else(|| StoreError::Corruption {
                        message: "Worktree payload is not an object".into(),
                    })?
                    .insert(
                        "activeOperationId".into(),
                        serde_json::Value::String(v.new_operation_id().to_string()),
                    );
                let payload = String::from_utf8(canonical_json(&value).map_err(|e| {
                    StoreError::Serialization {
                        message: e.to_string(),
                    }
                })?)
                .map_err(|e| StoreError::Serialization {
                    message: e.to_string(),
                })?;
                c.execute("UPDATE worktrees SET active_operation_id=?1,payload_json=?2 WHERE worktree_id=?3",params![v.new_operation_id().as_str(),payload,v.worktree_id().as_str()]).map_err(sql_backend)?;
                repo.hit(RepositoryFaultPoint::AfterWorktreeCas)?;
            }
        }
    }
    Ok(())
}

fn rebuild_gc_roots(c: &Connection) -> Result<()> {
    c.execute("DELETE FROM gc_roots", []).map_err(sql_backend)?;
    c.execute("INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms) SELECT 'product',p.product_object_id,p.product_object_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM products p JOIN space_object_admissions a ON a.object_id=p.product_object_id",[]).map_err(sql_backend)?;
    c.execute_batch(
        "INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'current_view',space_id,repository_view_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM current_repository_views;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'operation_head',operation_id,resulting_view_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM operation_heads JOIN operations USING(operation_id);
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'retained_operation',operation_id,resulting_view_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM operations;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'retained_operation',operation_id,old_step_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM operation_rewrites;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'retained_operation',operation_id,new_step_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM operation_rewrite_targets;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'worktree',w.worktree_id,o.resulting_view_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM worktrees w JOIN operations o ON o.operation_id=w.active_operation_id;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'worktree',w.worktree_id,rvw.base_step_id,CAST(unixepoch('subsec')*1000 AS INTEGER)
             FROM worktrees w JOIN operations o ON o.operation_id=w.active_operation_id
             JOIN repository_view_worktrees rvw ON rvw.repository_view_id=o.resulting_view_id AND rvw.worktree_id=w.worktree_id
             WHERE rvw.base_step_id IS NOT NULL;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'worktree',w.worktree_id,rvw.materialized_tree_id,CAST(unixepoch('subsec')*1000 AS INTEGER)
             FROM worktrees w JOIN operations o ON o.operation_id=w.active_operation_id
             JOIN repository_view_worktrees rvw ON rvw.repository_view_id=o.resulting_view_id AND rvw.worktree_id=w.worktree_id
             WHERE rvw.materialized_tree_id IS NOT NULL;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'marker',marker_id,target_step_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM markers;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'release',release_id,target_step_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM releases;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'observation',observation_id,step_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM step_observations;
         INSERT OR IGNORE INTO gc_roots(root_kind,root_id,object_id,created_at_ms)
           SELECT 'pin',pin_id,object_id,CAST(unixepoch('subsec')*1000 AS INTEGER) FROM object_pins;",
    )
    .map_err(sql_backend)?;
    Ok(())
}

fn edge(c: &Connection, s: &str, k: &str, i: usize, t: &str) -> Result<()> {
    let existing: Option<String> = c
        .query_row(
            "SELECT target_object_id FROM object_edges WHERE source_object_id=?1 AND edge_kind=?2 AND ordinal=?3",
            params![s, k, i],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_backend)?;
    match existing {
        Some(existing) if existing == t => {}
        Some(existing) => {
            return Err(StoreError::Corruption {
                message: format!("object edge {s}:{k}[{i}] points to {existing}, expected {t}"),
            });
        }
        None => {
            c.execute(
                "INSERT INTO object_edges(source_object_id,target_object_id,edge_kind,ordinal) VALUES(?1,?2,?3,?4)",
                params![s, t, k, i],
            )
            .map_err(sql_backend)?;
        }
    }
    Ok(())
}
fn json<T: Serialize>(v: &T) -> Result<String> {
    String::from_utf8(canonical_json(v).map_err(|e| StoreError::Serialization {
        message: e.to_string(),
    })?)
    .map_err(|e| StoreError::Serialization {
        message: e.to_string(),
    })
}
fn enum_name<T: Serialize>(v: T) -> Result<String> {
    serde_json::to_value(v)
        .map_err(|e| StoreError::Serialization {
            message: e.to_string(),
        })?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| StoreError::Serialization {
            message: "enum did not serialize as string".into(),
        })
}
fn kind_name(k: layrs_store::scm::StoreObjectKind) -> &'static str {
    match k {
        layrs_store::scm::StoreObjectKind::ChunkRaw => "chunk_raw",
        layrs_store::scm::StoreObjectKind::FileManifest => "file_manifest",
        layrs_store::scm::StoreObjectKind::Tree => "tree",
        layrs_store::scm::StoreObjectKind::Symlink => "symlink",
        layrs_store::scm::StoreObjectKind::Tombstone => "tombstone",
        layrs_store::scm::StoreObjectKind::Step => "step",
        layrs_store::scm::StoreObjectKind::RepositoryView => "repository_view",
        layrs_store::scm::StoreObjectKind::Conflict => "conflict",
        layrs_store::scm::StoreObjectKind::Product => "product",
    }
}
fn native_backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend {
        message: e.to_string(),
    }
}
pub(super) fn open_read_retry(root: &Path) -> Result<super::NativeStoreOpen> {
    acquire("short repository read", SHORT_WAIT, || open(root)).map_err(native_backend)
}

pub(super) fn open_exclusive_retry(root: &Path) -> Result<super::NativeStoreExclusive> {
    acquire("short repository write", SHORT_WAIT, || {
        open_exclusive(root)
    })
    .map_err(native_backend)
}
fn cas_backend(e: super::CasError) -> StoreError {
    let message = e.to_string();
    match e {
        super::CasError::InjectedFault { .. } | super::CasError::Io { .. } => {
            StoreError::Backend { message }
        }
        _ => StoreError::Corruption { message },
    }
}
fn schema_backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Corruption {
        message: e.to_string(),
    }
}
fn sql_backend(e: rusqlite::Error) -> StoreError {
    StoreError::Backend {
        message: e.to_string(),
    }
}
fn poisoned() -> StoreError {
    StoreError::Backend {
        message: "SQLite connection mutex poisoned".into(),
    }
}

macro_rules! delegate {($($name:ident($id:ident:$ty:ty)->$out:ty);* $(;)?)=>{$(fn $name(&self,$id:$ty)->Result<$out>{self.with_reader(|r|r.$name($id))})*}}
impl ScmReader for NativeScmRepository {
    fn products_in_space(&self, space: &SpaceId) -> Result<Vec<layrs_core::scm::ObjectId>> {
        self.with_reader(|reader| reader.products_in_space(space))
    }
    fn get_product(
        &self,
        id: &layrs_core::scm::ObjectId,
    ) -> Result<Option<layrs_store::scm::CanonicalProduct>> {
        self.with_reader(|reader| reader.get_product(id))
    }
    delegate! {get_chunk(id:&ChunkId)->Option<Chunk>;get_file_manifest(id:&FileManifestId)->Option<FileManifest>;get_tree(id:&TreeId)->Option<Tree>;get_symlink(id:&SymlinkId)->Option<Symlink>;get_tombstone(id:&TombstoneId)->Option<Tombstone>;get_step(id:&StepId)->Option<Step>;get_repository_view(id:&RepositoryViewId)->Option<RepositoryView>;get_conflict(id:&ConflictId)->Option<CanonicalConflict>;get_space(id:&SpaceId)->Option<Space>;get_layer(id:&LayerId)->Option<Layer>;get_step_observation(id:&ObservationId)->Option<StepObservation>;get_operation(id:&OperationId)->Option<Operation>;get_worktree(id:&WorktreeId)->Option<Worktree>;get_marker(id:&MarkerId)->Option<Marker>;get_release(id:&ReleaseId)->Option<Release>;get_release_publication(id:&ReleasePublicationId)->Option<ReleasePublication>;file_availability(id:&FileId)->FileAvailability;store_exists(id:&StoreId)->bool;conflict_exists(id:&ConflictId)->bool;actor_exists(id:&ActorId)->bool;device_exists(id:&DeviceId)->bool;current_repository_view(id:&SpaceId)->Option<CurrentRepositoryView>;operation_heads(id:&SpaceId)->Vec<OperationId>;initializer_in_space(id:&SpaceId)->Option<Operation>;worktree_active_operation(id:&WorktreeId)->Option<OperationId>;}
    fn object_admitted_to_space(&self, o: &StoreObjectId, s: &SpaceId) -> Result<bool> {
        self.with_reader(|r| r.object_admitted_to_space(o, s))
    }
    fn is_step_ancestor(&self, a: &StepId, d: &StepId) -> Result<bool> {
        self.with_reader(|r| r.is_step_ancestor(a, d))
    }
    fn operation_reaches_step(&self, o: &OperationId, s: &StepId) -> Result<bool> {
        self.with_reader(|r| r.operation_reaches_step(o, s))
    }
    fn proof_in_space(&self, p: &ProofId, s: &SpaceId) -> Result<bool> {
        self.with_reader(|r| r.proof_in_space(p, s))
    }
}
