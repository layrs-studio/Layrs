use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

use super::cas::{CasFaultInjector, CasFaultPoint, NativeCasWriter};
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
}

struct NoCasFaults;
impl CasFaultInjector for NoCasFaults {
    fn check(&self, _point: CasFaultPoint) -> std::result::Result<(), String> {
        Ok(())
    }
}

impl NativeScmRepository {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            faults: no_faults(),
            cas_faults: Arc::new(NoCasFaults),
            materialization_permit: None,
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
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn for_materialization(root: &Path, recovery_id: &str, owner: &str) -> Self {
        let mut repository = Self::new(root);
        repository.materialization_permit = Some((recovery_id.into(), owner.into()));
        repository
    }

    fn hit(&self, point: RepositoryFaultPoint) -> Result<()> {
        self.faults
            .check(point)
            .map_err(|message| StoreError::Backend {
                message: format!("injected repository fault at {point:?}: {message}"),
            })
    }

    fn with_reader<T>(&self, f: impl FnOnce(&NativeScmReader<'_>) -> Result<T>) -> Result<T> {
        let store = open(&self.root).map_err(native_backend)?;
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
        mut read_chunk: impl FnMut(&ChunkId) -> Result<Vec<u8>>,
    ) -> Result<ApplyBatchOutcome> {
        super::admission::preflight_batch_budgets(batch)?;
        streaming::preflight(batch, chunks)?;
        // Rebuild through the public constructor: this rechecks ordering,
        // duplicate targets, publication coupling, canonical JSON and digest.
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

        let store = open_exclusive_retry(&self.root)?;
        let cas = NativeCasWriter::with_fault_injector(&store, Arc::clone(&self.cas_faults))
            .map_err(cas_backend)?;
        let mut installed = Vec::with_capacity(batch.canonical_objects().len() + chunks.len());
        for (id, length) in chunks {
            let bytes = read_chunk(id)?;
            if bytes.len() as u64 != *length {
                return Err(StoreError::Corruption {
                    message: "external chunk length differs from its descriptor".into(),
                });
            }
            let chunk = Chunk::new(bytes).map_err(|error| StoreError::Corruption {
                message: format!("invalid external chunk: {error}"),
            })?;
            if chunk.id() != id {
                return Err(StoreError::Corruption {
                    message: "external chunk identity differs from its descriptor".into(),
                });
            }
            installed.push(
                cas.put(&CanonicalObject::ChunkRaw(chunk))
                    .map_err(cas_backend)?,
            );
            self.hit(RepositoryFaultPoint::AfterEachCas)?;
        }
        for object in batch.canonical_objects() {
            installed.push(cas.put(object).map_err(cas_backend)?);
            self.hit(RepositoryFaultPoint::AfterEachCas)?;
        }
        self.hit(RepositoryFaultPoint::AfterAllCas)?;

        let connection = store.begin_write_connection().map_err(schema_backend)?;
        self.hit(RepositoryFaultPoint::AfterWriterOpen)?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(sql_backend)?;
        self.hit(RepositoryFaultPoint::AfterBeginImmediate)?;
        let connection = Mutex::new(connection);

        let result = self.apply_inside_transaction(
            &connection,
            &cas,
            store.layout().root(),
            batch,
            &installed,
            chunks.len(),
        );
        match result {
            Ok(outcome) => {
                let guard = connection.lock().map_err(|_| poisoned())?;
                guard.execute_batch("COMMIT").map_err(sql_backend)?;
                drop(guard);
                self.hit(RepositoryFaultPoint::AfterCommit)?;
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
    fn apply_inside_transaction(
        &self,
        connection: &Mutex<Connection>,
        cas: &NativeCasWriter<'_>,
        store_root: &Path,
        batch: &WriteBatch,
        installed: &[super::InstallOutcome],
        external_chunks: usize,
    ) -> Result<ApplyBatchOutcome> {
        // CAS publication intentionally precedes replay inspection. Therefore a
        // transaction-ID/digest mismatch may leave verified, unreferenced CAS
        // objects supplied by the losing request. This is safe by contract:
        // no metadata points at them and normal reachability GC may remove them.
        if let Some(outcome) = replay(connection, batch)? {
            return Ok(outcome);
        }
        self.hit(RepositoryFaultPoint::AfterReplayCheck)?;

        self.check_materializations(connection, batch)?;

        precheck_mutations(connection, batch)?;
        // These rows are visible only inside this transaction. The admission
        // resolver can read and rehash streamed chunks without retaining their
        // bytes in WriteBatch; any later failure rolls back all these rows.
        if external_chunks != 0 {
            let guard = connection.lock().map_err(|_| poisoned())?;
            insert_object_rows(&guard, store_root, &installed[..external_chunks])?;
        }
        let reader = NativeTransactionalScmReader::new(connection, cas)?;
        ensure_immutables(&reader, batch)?;
        let resolver = CompositeAdmissionResolver::new(&reader, batch)?;
        resolver.validate_batch()?;
        drop(resolver);
        drop(reader);
        self.hit(RepositoryFaultPoint::AfterValidation)?;

        let guard = connection.lock().map_err(|_| poisoned())?;
        insert_object_rows(&guard, store_root, installed)?;
        insert_object_projections_first(&guard, batch)?;
        self.hit(RepositoryFaultPoint::AfterObjects)?;
        insert_entities(&guard, batch)?;
        insert_object_projections_views(&guard, batch)?;
        self.hit(RepositoryFaultPoint::AfterEntitiesAndProjections)?;
        insert_admissions(&guard, batch)?;
        self.hit(RepositoryFaultPoint::AfterAdmissions)?;
        apply_mutations(&guard, batch, self)?;
        rebuild_gc_roots(&guard)?;
        self.hit(RepositoryFaultPoint::BeforeReceipt)?;
        let receipt = ApplyBatchReceipt::new(batch);
        let receipt_json = canonical_receipt_json(&receipt).map_err(schema_backend)?;
        guard.execute(
            "INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![batch.transaction_id().as_str(), batch.request_digest().to_string(), receipt_json],
        ).map_err(sql_backend)?;
        self.hit(RepositoryFaultPoint::AfterReceiptBeforeCommit)?;
        Ok(ApplyBatchOutcome::Applied {
            request_digest: batch.request_digest(),
        })
    }
}

impl NativeScmRepository {
    fn check_materializations(
        &self,
        connection: &Mutex<Connection>,
        batch: &WriteBatch,
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
                let (total, expired): (i64,i64) = guard.query_row("SELECT count(*),COALESCE(sum(CASE WHEN released_at_ms IS NOT NULL OR expires_at_ms <= CAST(unixepoch('subsec')*1000 AS INTEGER) THEN 1 ELSE 0 END),0) FROM materialization_leases WHERE recovery_operation_id=?1", [&id], |r| Ok((r.get(0)?,r.get(1)?))).map_err(sql_backend)?;
                if !permitted || total == 0 || expired != 0 {
                    return Err(StoreError::Backend {
                        message: format!("worktree_needs_repair: pending materialization {id}"),
                    });
                }
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
    for object in batch.canonical_objects() {
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
        connection.execute(
            "INSERT OR IGNORE INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,?2,?3,?4,1,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![outcome.object_id.as_str(), kind_name(outcome.object_id.kind()), relative, outcome.bytes_len],
        ).map_err(sql_backend)?;
    }
    Ok(())
}

fn insert_object_projections_first(connection: &Connection, batch: &WriteBatch) -> Result<()> {
    for object in batch.canonical_objects() {
        match object {
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
fn open_exclusive_retry(root: &Path) -> Result<super::NativeStoreExclusive> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match open_exclusive(root) {
            Ok(store) => return Ok(store),
            Err(super::NativeStoreError::Lock(
                crate::StoreError::LockBusy(_) | crate::StoreError::ReentrantLock(_),
            )) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(native_backend(error)),
        }
    }
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
