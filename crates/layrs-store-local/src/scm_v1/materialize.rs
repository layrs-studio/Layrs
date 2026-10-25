//! Recoverable checkout writes. Originals are moved without replacement into a
//! durable backup; failures leave the journal pending and block further captures.
use super::repository::{open_exclusive_retry as open_exclusive, open_read_retry as open};
use super::{
    checkout::{
        CheckoutContent, CheckoutEntry, CheckoutLimits, CheckoutProjectionContent,
        CheckoutProjectionEntry, CheckoutReader,
    },
    *,
};
use crate::secure_fs::{PinnedDirectory, PinnedFile, PinnedLeaf};
use layrs_core::scm::WorktreeId;
use layrs_store::scm::{ApplyBatchOutcome, ScmReader, StoreMutation, WriteBatch};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};
mod lease;
mod sources;
use lease::{LEASE_MS, LeaseKeeper};
pub use sources::CheckoutSources;

pub trait MaterializationFaults {
    fn check(&self, point: MaterializationPoint) -> io::Result<()>;
    /// Deterministic clock seam for lease-expiry recipes; production callers
    /// use wall time. A supplied time cannot bypass owner/generation fencing.
    fn lease_time_ms(&self) -> io::Result<i64> {
        now_ms()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializationPoint {
    AfterJournal,
    AfterBackup,
    AfterDirectoryRemoval,
    AfterPath,
    BeforeMetadata,
    AfterMetadata,
}
pub struct NoMaterializationFaults;
impl MaterializationFaults for NoMaterializationFaults {
    fn check(&self, _: MaterializationPoint) -> io::Result<()> {
        Ok(())
    }
}

pub fn recovery_pending(store: &NativeStoreOpen, worktree: &WorktreeId) -> io::Result<bool> {
    store.connection().map_err(error)?.query_row("SELECT EXISTS(SELECT 1 FROM recovery_operations WHERE worktree_id=?1 AND recovery_type IN ('materialize','restore') AND state <> 'completed')",[worktree.as_str()],|r|r.get(0)).map_err(error)
}

/// All paths are relative canonical repository paths. The caller must capture
/// current work into CAS first and prepare a batch against that exact state.
pub fn apply_checkout(
    store_root: &Path,
    worktree_id: &WorktreeId,
    before: &[CheckoutEntry],
    after: &[CheckoutEntry],
    batch: &WriteBatch,
    faults: &dyn MaterializationFaults,
) -> io::Result<ApplyBatchOutcome> {
    let identity = PinnedDirectory::open(store_root)?.identity_token();
    apply_checkout_bound(
        store_root,
        &identity,
        worktree_id,
        before,
        after,
        batch,
        faults,
    )
}

pub fn apply_checkout_bound(
    store_root: &Path,
    store_identity: &str,
    worktree_id: &WorktreeId,
    before: &[CheckoutEntry],
    after: &[CheckoutEntry],
    batch: &WriteBatch,
    faults: &dyn MaterializationFaults,
) -> io::Result<ApplyBatchOutcome> {
    let metadata_identity = PinnedDirectory::open(&store_root.join("metadata"))?.identity_token();
    apply_checkout_bound_metadata(
        store_root,
        store_identity,
        &metadata_identity,
        worktree_id,
        before,
        after,
        batch,
        faults,
    )
}

pub fn apply_checkout_bound_metadata(
    store_root: &Path,
    store_identity: &str,
    metadata_identity: &str,
    worktree_id: &WorktreeId,
    before: &[CheckoutEntry],
    after: &[CheckoutEntry],
    batch: &WriteBatch,
    faults: &dyn MaterializationFaults,
) -> io::Result<ApplyBatchOutcome> {
    // Keep the original in-memory API's existing limits and exact projection.
    super::checkout::entries_tree_id(before)?;
    super::checkout::entries_tree_id(after)?;
    let before_projection: Vec<_> = before.iter().map(CheckoutProjectionEntry::from).collect();
    let after_projection: Vec<_> = after.iter().map(CheckoutProjectionEntry::from).collect();
    let files: BTreeMap<_, _> = after
        .iter()
        .filter_map(|e| match &e.content {
            CheckoutContent::File { bytes, .. } => Some((e.path.as_str(), bytes)),
            _ => None,
        })
        .collect();
    let mut write_file = |path: &str, out: &mut dyn Write| {
        out.write_all(
            files
                .get(path)
                .ok_or_else(|| error("prepared file absent"))?,
        )
    };
    let mut read_chunk = |_: &layrs_core::scm::ChunkId| {
        Err(layrs_store::scm::StoreError::Corruption {
            message: "unexpected external chunk".into(),
        })
    };
    apply_checkout_with_sources_bound_metadata(
        store_root,
        store_identity,
        metadata_identity,
        worktree_id,
        &before_projection,
        &after_projection,
        batch,
        faults,
        &mut CheckoutSources {
            chunks: &[],
            read_chunk: &mut read_chunk,
            write_file: &mut write_file,
        },
    )
}

/// Recoverable streaming materialization; callbacks supply local prepared data.
pub fn apply_checkout_with_sources(
    store_root: &Path,
    worktree_id: &WorktreeId,
    before: &[CheckoutProjectionEntry],
    after: &[CheckoutProjectionEntry],
    batch: &WriteBatch,
    faults: &dyn MaterializationFaults,
    sources: &mut CheckoutSources<'_>,
) -> io::Result<ApplyBatchOutcome> {
    let identity = PinnedDirectory::open(store_root)?.identity_token();
    apply_checkout_with_sources_bound(
        store_root,
        &identity,
        worktree_id,
        before,
        after,
        batch,
        faults,
        sources,
    )
}

/// Bind every journal, lease and publication capability to the caller's Store.
pub fn apply_checkout_with_sources_bound(
    store_root: &Path,
    store_identity: &str,
    worktree_id: &WorktreeId,
    before: &[CheckoutProjectionEntry],
    after: &[CheckoutProjectionEntry],
    batch: &WriteBatch,
    faults: &dyn MaterializationFaults,
    sources: &mut CheckoutSources<'_>,
) -> io::Result<ApplyBatchOutcome> {
    let metadata_identity = PinnedDirectory::open(&store_root.join("metadata"))?.identity_token();
    apply_checkout_with_sources_bound_metadata(
        store_root,
        store_identity,
        &metadata_identity,
        worktree_id,
        before,
        after,
        batch,
        faults,
        sources,
    )
}

pub fn apply_checkout_with_sources_bound_metadata(
    store_root: &Path,
    store_identity: &str,
    metadata_identity: &str,
    worktree_id: &WorktreeId,
    before: &[CheckoutProjectionEntry],
    after: &[CheckoutProjectionEntry],
    batch: &WriteBatch,
    faults: &dyn MaterializationFaults,
    sources: &mut CheckoutSources<'_>,
) -> io::Result<ApplyBatchOutcome> {
    let store_directory = pin_store(store_root, store_identity)?;
    let metadata_directory = pin_store(&store_root.join("metadata"), metadata_identity)?;
    let _checkout_lock = lock_checkout(store_root, store_identity, metadata_identity, worktree_id)?;
    super::checkout::projection_tree_id(before)?;
    let projection = super::checkout::projection_tree_id(after)?;
    super::repository::streaming_preflight(batch, sources.chunks).map_err(error)?;
    let claimed_projection = batch.canonical_objects().iter().find_map(|o| match o {
        layrs_store::scm::CanonicalObject::RepositoryView(view) => view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == worktree_id)
            .and_then(|w| w.materialized_tree_id()),
        _ => None,
    });
    if claimed_projection != Some(&projection) {
        return Err(error(
            "checkout projection does not match prepared RepositoryView",
        ));
    }
    let prior: BTreeMap<_, _> = before
        .iter()
        .map(|e| (e.path.as_str(), &e.content))
        .collect();
    let next: BTreeMap<_, _> = after
        .iter()
        .map(|e| (e.path.as_str(), &e.content))
        .collect();
    let mut paths: Vec<_> = prior.keys().chain(next.keys()).copied().collect();
    paths.sort();
    paths.dedup();
    paths.retain(|p| prior.get(p) != next.get(p));
    let worktree = NativeScmRepository::new(store_root)
        .with_store_identity(store_identity)
        .with_metadata_identity(metadata_identity)
        .get_worktree(worktree_id)
        .map_err(error)?
        .ok_or_else(|| error("missing Worktree"))?;
    let checkout = CheckoutReader::open(
        worktree.checkout_root(),
        Some(worktree.filesystem_identity()),
    )?;
    let tracked = before.iter().chain(after).map(|e| e.path.clone()).collect();
    if checkout
        .capture_streaming(limits(), &tracked, &mut |_| Ok(()), &mut || Ok(()))?
        .0
        != before
    {
        return Err(error("checkout changed after capture"));
    }
    if paths.is_empty() {
        return NativeScmRepository::new(store_root)
            .with_short_acquisition()
            .with_store_identity(store_identity)
            .with_metadata_identity(metadata_identity)
            .apply_batch_with_chunks(batch, sources.chunks, &mut sources.read_chunk)
            .map_err(io::Error::other);
    }
    for path in &paths {
        let parsed = layrs_core::scm::RepoPath::parse(*path).map_err(error)?;
        if parsed.as_str() != *path
            || path
                .split('/')
                .any(|p| p.eq_ignore_ascii_case(".layrs") || p.eq_ignore_ascii_case(".git"))
        {
            return Err(error("invalid materialization path"));
        }
        #[cfg(not(target_os = "linux"))]
        if [prior.get(path), next.get(path)]
            .into_iter()
            .flatten()
            .any(|c| matches!(c, CheckoutProjectionContent::Symlink { .. }))
        {
            return Err(error(
                "symlink materialization requires Linux pinned-link primitives",
            ));
        }
        if [prior.get(path), next.get(path)].into_iter().flatten().any(|content|matches!(content,CheckoutProjectionContent::Symlink{target} if target.is_empty()||target.len()>4095||target.contains('\0'))){return Err(error("symlink target cannot be materialized exactly"));}
        if let Ok(meta) = fs::symlink_metadata(checkout.root().join(path)) {
            if !(meta.is_file()
                || meta.is_dir()
                || cfg!(target_os = "linux") && meta.file_type().is_symlink())
            {
                return Err(error(
                    "materialization collides with an unsupported entry or link",
                ));
            }
        }
        #[cfg(windows)]
        if next.get(path).is_some_and(|c| {
            matches!(
                c,
                CheckoutProjectionContent::File {
                    executable: true,
                    ..
                }
            )
        }) {
            return Err(error(
                "executable bit cannot be materialized on this platform",
            ));
        }
    }
    let (checkout_root, identity, active, target, recovery_id, owner, backup_root) = {
        let mut store = open_exclusive(store_root)
            .map_err(error)?
            .with_filesystem_identity(store_identity)?
            .with_metadata_identity(metadata_identity)?;
        let connection = store.begin_write_connection().map_err(error)?;
        connection.execute_batch("BEGIN IMMEDIATE").map_err(error)?;
        let connection = std::sync::Mutex::new(connection);
        let cas = NativeCas::open_exclusive(&store).map_err(error)?;
        let reader = NativeTransactionalScmReader::new(&connection, &cas).map_err(error)?;
        let worktree = reader
            .get_worktree(worktree_id)
            .map_err(error)?
            .ok_or_else(|| error("missing Worktree"))?;
        let change = batch
            .mutations()
            .iter()
            .find_map(|m| match m {
                StoreMutation::WorktreeActiveOperation(c) if c.worktree_id() == worktree_id => {
                    Some(c)
                }
                _ => None,
            })
            .ok_or_else(|| error("materialization requires Worktree CAS"))?;
        if worktree.active_operation_id() != change.expected_operation_id() {
            return Err(error("stale materialization base"));
        }
        // Preserve the typed CAS failure if another Worktree advanced the heads.
        // A stale prepared batch is never retried or admitted against new heads.
        super::repository::precheck_mutations(&connection, batch).map_err(io::Error::other)?;
        let descriptors: BTreeMap<_, _> = sources.chunks.iter().cloned().collect();
        let source = std::cell::RefCell::new(&mut sources.read_chunk);
        let resolve = |id: &layrs_core::scm::ChunkId| {
            let Some(length) = descriptors.get(id) else {
                return Ok(None);
            };
            let bytes = source.borrow_mut()(id)?;
            if bytes.len() as u64 != *length {
                return Err(layrs_store::scm::StoreError::Corruption {
                    message: "prepared chunk length changed".into(),
                });
            }
            layrs_core::scm::Chunk::new(bytes).map(Some).map_err(|e| {
                layrs_store::scm::StoreError::Corruption {
                    message: e.to_string(),
                }
            })
        };
        CompositeAdmissionResolver::new(&reader, batch)
            .and_then(|r| r.with_chunk_source(&resolve).validate_batch())
            .map_err(error)?;
        let active = worktree.active_operation_id().clone();
        let target = change.new_operation_id().clone();
        // A rolled-back attempt retains its journal and backups. Retrying the
        // same immutable batch needs fresh physical paths, while batch-level
        // idempotency remains unchanged. Both locks serialize this allocation.
        let attempts: u64 = connection.lock().map_err(error)?.query_row(
            "SELECT count(*) FROM recovery_operations WHERE worktree_id=?1 AND json_extract(payload_json,'$.batchDigest')=?2",
            rusqlite::params![worktree_id.as_str(), batch.request_digest().to_string()], |r|r.get(0),
        ).map_err(error)?;
        let attempt = attempts
            .checked_add(1)
            .ok_or_else(|| error("materialization attempt overflow"))?;
        let recovery_id = RecoveryId::parse(format!(
            "recovery:{}:attempt:{attempt}",
            batch.request_digest()
        ))
        .map_err(error)?;
        let owner = format!("materializer:{}:{attempt}", batch.transaction_id().as_str());
        let backup_root = store.layout().repair_backups_dir().join(format!(
            "{}-attempt-{attempt}",
            batch.request_digest().to_string().replace(':', "-")
        ));
        let payload = serde_json::json!({ "schema":"layrs.checkout-write.v1", "batchDigest":batch.request_digest().to_string(), "attempt":attempt, "oldOperation":active, "newOperation":target, "backupRoot":backup_root, "checkoutRoot":worktree.checkout_root(), "filesystemIdentity":worktree.filesystem_identity() });
        let path_plans = paths
            .iter()
            .map(|p| RecoveryPathPlan::new(*p, RecoveryPathRole::Materialized, None))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        let plan = RecoveryPlan::new(
            recovery_id.clone(),
            format!("checkout:{}:{attempt}", batch.request_digest()),
            worktree_id.clone(),
            Some(active.clone()),
            RecoveryType::Materialize,
            payload,
            path_plans,
            faults.lease_time_ms()?,
        )
        .map_err(error)?;
        let root = worktree.checkout_root().to_owned();
        let identity = worktree.filesystem_identity().to_owned();
        drop(reader);
        drop(cas);
        connection
            .into_inner()
            .map_err(error)?
            .execute_batch("ROLLBACK")
            .map_err(error)?;
        let mut journal = NativeRecovery::new(&mut store).map_err(error)?;
        // Another failed or pending checkout write must be recovered first.
        if journal
            .list_pending()
            .map_err(error)?
            .iter()
            .any(|r| r.worktree_id == *worktree_id)
        {
            return Err(error("worktree_needs_repair: pending checkout journal"));
        }
        journal.create(&plan).map_err(error)?;
        (
            root,
            identity,
            active,
            target,
            recovery_id,
            owner,
            backup_root,
        )
    };
    faults.check(MaterializationPoint::AfterJournal)?;
    let checkout = PinnedDirectory::open(Path::new(&checkout_root))?;
    if checkout.identity_token() != identity {
        return Err(error("checkout identity changed"));
    }
    store_directory.verify_path_identity()?;
    metadata_directory.verify_path_identity()?;
    let store_dir = &store_directory;
    let backups = store_dir
        .ensure_child("metadata".as_ref())?
        .ensure_child("repair-backups".as_ref())?;
    let backup = backups.ensure_child(
        backup_root
            .file_name()
            .ok_or_else(|| error("missing backup name"))?,
    )?;
    let mut staged = BTreeMap::new();
    let mut evidence = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if let Some(CheckoutProjectionContent::File {
            file_id,
            byte_length,
            executable,
        }) = next.get(path).copied()
        {
            let mut file = backup.create_file(format!("new-{index}").as_ref())?;
            let mut verified = sources::VerifiedWriter::new(file.file_mut(), file_id, *byte_length);
            (sources.write_file)(path, &mut verified)?;
            verified.finish()?;
            set_executable(file.file(), *executable)?;
            file.file().sync_all()?;
            staged.insert(*path, PinnedLeaf::File(file));
        } else if let Some(CheckoutProjectionContent::Symlink { target }) = next.get(path).copied()
        {
            staged.insert(
                *path,
                backup.create_symlink(format!("new-{index}").as_ref(), target)?,
            );
        }
        evidence.push(PathEvidence::new(*path, RecoveryPathRole::Materialized, serde_json::json!({"backup":format!("old-{index}"), "before":fingerprint(prior.get(path).copied()), "after":fingerprint(next.get(path).copied())})).map_err(error)?);
    }
    backup.sync()?;
    let leases = {
        let mut store = open_exclusive(store_root)
            .map_err(error)?
            .with_filesystem_identity(store_identity)?
            .with_metadata_identity(metadata_identity)?;
        let mut journal = NativeRecovery::new(&mut store).map_err(error)?;
        journal
            .prepare(
                &recovery_id,
                &RecoveryPreparation::new(serde_json::json!({"staged":true}), evidence)
                    .map_err(error)?,
                faults.lease_time_ms()?,
            )
            .map_err(error)?;
        journal
            .claim(&recovery_id, &owner, faults.lease_time_ms()?, LEASE_MS)
            .map_err(error)?
            .leases
    };
    let mut lease = LeaseKeeper::new(
        store_root,
        &store_directory,
        &metadata_directory,
        &recovery_id,
        &owner,
        leases,
        faults,
    )?;
    // Phase one moves every changed original out of the way before any new
    // directory/file is created. A prefix may change from file to directory.
    for (index, path) in paths.iter().enumerate() {
        lease.tick()?;
        checkout.verify_path_identity()?;
        if let Some(content) = prior.get(path).copied() {
            let (parent, name) = checkout.existing_parent(Path::new(path))?;
            let original = parent.open_leaf(&name)?;
            if !matches_fingerprint(Some(&original), &fingerprint(Some(content)), &mut || {
                lease.tick()
            })? {
                return Err(error(
                    "checkout leaf changed before replacement; original retained",
                ));
            }
            if original.rename_to(&backup, format!("old-{index}").as_ref())? {
                return Err(error("backup already exists"));
            }
            parent.sync()?;
            backup.sync()?;
            let protected = backup.open_leaf(format!("old-{index}").as_ref())?;
            if !matches_fingerprint(Some(&protected), &fingerprint(Some(content)), &mut || {
                lease.tick()
            })? {
                return Err(error(
                    "checkout leaf changed during backup; moved entry retained",
                ));
            }
            faults.check(MaterializationPoint::AfterBackup)?;
        }
    }
    // Only remove empty directories that obstruct a desired leaf. Unexpected
    // children make the native nonrecursive removal fail with backups retained.
    clear_obstructing_directories(
        &checkout,
        prior.keys().copied(),
        next.keys().copied(),
        &mut || lease.tick(),
    )?;
    faults.check(MaterializationPoint::AfterDirectoryRemoval)?;
    for path in &paths {
        lease.tick()?;
        checkout.verify_path_identity()?;
        if let Some(file) = staged.get(path) {
            let (parent, name) = checkout.components_to_parent(Path::new(path))?;
            parent.verify_path_identity()?;
            if file.rename_to(&parent, &name)? {
                return Err(error(
                    "checkout path appeared during materialization; original backup retained",
                ));
            }
            parent.sync()?;
            backup.sync()?;
        }
        faults.check(MaterializationPoint::AfterPath)?;
    }
    let actual = CheckoutReader::open(&checkout_root, Some(&identity))?
        .capture_streaming(
            limits(),
            &after.iter().map(|e| e.path.clone()).collect(),
            &mut |_| Ok(()),
            &mut || lease.tick(),
        )?
        .0;
    if actual != after {
        return Err(error(
            "checkout changed during materialization; backup retained",
        ));
    }
    faults.check(MaterializationPoint::BeforeMetadata)?;
    lease.force()?;
    let outcome =
        NativeScmRepository::for_materialization(store_root, recovery_id.as_str(), &owner)
            .with_store_identity(store_identity)
            .with_metadata_identity(metadata_identity)
            .apply_batch_with_chunks(batch, sources.chunks, &mut sources.read_chunk)
            .map_err(io::Error::other)?;
    faults.check(MaterializationPoint::AfterMetadata)?;
    lease.force()?;
    let results = paths
        .iter()
        .map(|p| {
            RecoveryPathResult::new(
                *p,
                RecoveryPathRole::Materialized,
                RecoveryPathState::Applied,
                serde_json::json!({"verified":true}),
            )
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(error)?;
    let mut store = open_exclusive(store_root)
        .map_err(error)?
        .with_filesystem_identity(store_identity)?
        .with_metadata_identity(metadata_identity)?;
    NativeRecovery::new(&mut store)
        .map_err(error)?
        .complete(
            &recovery_id,
            &owner,
            &lease.leases,
            serde_json::json!({"oldOperation":active,"newOperation":target,"verified":true}),
            &results,
            faults.lease_time_ms()?,
        )
        .map_err(error)?;
    Ok(outcome)
}

/// Missing parent or a regular-file ancestor means this leaf is absent. An
/// actual directory at this leaf is also absence in the flattened file view.
fn current_file(checkout: &PinnedDirectory, path: &str) -> io::Result<Option<PinnedLeaf>> {
    let (parent, name) = match checkout.existing_parent(Path::new(path)) {
        Ok(value) => value,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    match parent.open_leaf(&name) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => match parent.open_child_for_traversal(&name) {
            Ok(_) => Ok(None),
            Err(_) => Err(e),
        },
    }
}

fn clear_obstructing_directories<'a>(
    checkout: &PinnedDirectory,
    old: impl Iterator<Item = &'a str>,
    desired: impl Iterator<Item = &'a str>,
    progress: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let leaves = desired.collect::<std::collections::BTreeSet<_>>();
    let mut candidates = leaves
        .iter()
        .map(|s| (*s).to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    for path in old {
        progress()?;
        for (index, _) in path.match_indices('/') {
            let directory = &path[..index];
            if leaves.contains(directory)
                || directory
                    .match_indices('/')
                    .any(|(i, _)| leaves.contains(&directory[..i]))
            {
                candidates.insert(directory.to_owned());
            }
        }
    }
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.matches('/')
            .count()
            .cmp(&a.matches('/').count())
            .then_with(|| b.cmp(a))
    });
    for path in candidates {
        progress()?;
        checkout.verify_path_identity()?;
        let (parent, name) = match checkout.existing_parent(Path::new(&path)) {
            Ok(value) => value,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        match parent.open_child_for_traversal(&name) {
            Ok(directory) => {
                directory.verify_path_identity()?;
                drop(directory);
                parent.remove_empty_child(&name)?;
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn fingerprint(content: Option<&CheckoutProjectionContent>) -> serde_json::Value {
    match content {
        None => serde_json::Value::Null,
        Some(CheckoutProjectionContent::File {
            file_id,
            byte_length,
            executable,
        }) => {
            serde_json::json!({"kind":"file","fileId":file_id,"byteLength":byte_length,"executable":executable})
        }
        Some(CheckoutProjectionContent::Symlink { target }) => {
            serde_json::json!({"kind":"symlink","target":target})
        }
    }
}
fn limits() -> CheckoutLimits {
    CheckoutLimits {
        max_entries: 100_000,
        max_depth: 128,
        max_file_bytes: 1024 * 1024 * 1024,
        max_total_bytes: 4 * 1024 * 1024 * 1024,
    }
}
fn now_ms() -> io::Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(error)?
        .as_millis()
        .try_into()
        .map_err(error)
}
fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn set_executable(file: &fs::File, executable: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(if executable {
            0o755
        } else {
            0o644
        }))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        if executable {
            Err(error(
                "executable bit cannot be materialized on this platform",
            ))
        } else {
            Ok(())
        }
    }
}

fn pin_store(root: &Path, expected: &str) -> io::Result<PinnedDirectory> {
    let directory = PinnedDirectory::open(root)?;
    if directory.identity_token() != expected {
        return Err(error("Store filesystem identity changed"));
    }
    Ok(directory)
}

fn lock_checkout(
    store_root: &Path,
    store_identity: &str,
    metadata_identity: &str,
    worktree: &WorktreeId,
) -> io::Result<PinnedFile> {
    let store = open(store_root)
        .map_err(error)?
        .with_filesystem_identity(store_identity)?
        .with_metadata_identity(metadata_identity)?;
    let metadata = PinnedDirectory::open(store.layout().metadata_dir())?;
    let name = format!(
        "checkout-{}.lock",
        blake3::hash(worktree.as_str().as_bytes()).to_hex()
    );
    let file = match metadata.create_file(name.as_ref()) {
        Ok(file) => {
            file.file().sync_all()?;
            metadata.sync()?;
            file
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => metadata.open_file(name.as_ref())?,
        Err(e) => return Err(e),
    };
    fs2::FileExt::try_lock_exclusive(file.file())?;
    Ok(file)
}

/// Restores originals after an interrupted pre-commit write, or finalizes an
/// already committed result. Unexpected editor changes are preserved and cause
/// an explicit repair error; backups are retained after successful recovery.
pub fn recover_checkout(store_root: &Path, worktree_id: &WorktreeId) -> io::Result<usize> {
    recover_checkout_with_faults(store_root, worktree_id, &NoMaterializationFaults)
}

/// Fault boundaries are also available during quarantine/restoration so a
/// second interruption can be tested against the same durable journal.
pub fn recover_checkout_with_faults(
    store_root: &Path,
    worktree_id: &WorktreeId,
    faults: &dyn MaterializationFaults,
) -> io::Result<usize> {
    let identity = PinnedDirectory::open(store_root)?.identity_token();
    recover_checkout_bound(store_root, &identity, worktree_id, faults)
}

pub fn recover_checkout_bound(
    store_root: &Path,
    store_identity: &str,
    worktree_id: &WorktreeId,
    faults: &dyn MaterializationFaults,
) -> io::Result<usize> {
    let metadata_identity = PinnedDirectory::open(&store_root.join("metadata"))?.identity_token();
    recover_checkout_bound_metadata(
        store_root,
        store_identity,
        &metadata_identity,
        worktree_id,
        faults,
    )
}

pub fn recover_checkout_bound_metadata(
    store_root: &Path,
    store_identity: &str,
    metadata_identity: &str,
    worktree_id: &WorktreeId,
    faults: &dyn MaterializationFaults,
) -> io::Result<usize> {
    let store_directory = pin_store(store_root, store_identity)?;
    let metadata_directory = pin_store(&store_root.join("metadata"), metadata_identity)?;
    let _checkout_lock = lock_checkout(store_root, store_identity, metadata_identity, worktree_id)?;
    let worktree = NativeScmRepository::new(store_root)
        .with_store_identity(store_identity)
        .with_metadata_identity(metadata_identity)
        .get_worktree(worktree_id)
        .map_err(error)?
        .ok_or_else(|| error("missing Worktree"))?;
    let mut store = open_exclusive(store_root)
        .map_err(error)?
        .with_filesystem_identity(store_identity)?
        .with_metadata_identity(metadata_identity)?;
    let pending = NativeRecovery::new(&mut store)
        .map_err(error)?
        .list_pending()
        .map_err(error)?;
    drop(store);
    let mut count = 0;
    for record in pending.into_iter().filter(|r| {
        r.worktree_id == *worktree_id && r.payload["schema"] == "layrs.checkout-write.v1"
    }) {
        if record.payload["checkoutRoot"].as_str() != Some(worktree.checkout_root())
            || record.payload["filesystemIdentity"].as_str() != Some(worktree.filesystem_identity())
        {
            return Err(error("recovery checkout identity differs from Worktree"));
        }
        let checkout_reader = CheckoutReader::open(
            worktree.checkout_root(),
            Some(worktree.filesystem_identity()),
        )?;
        let checkout = PinnedDirectory::open(checkout_reader.root())?;
        let committed = record.payload["newOperation"].as_str()
            == Some(worktree.active_operation_id().as_str());
        if !committed
            && record.payload["oldOperation"].as_str()
                != Some(worktree.active_operation_id().as_str())
        {
            return Err(error("Worktree moved beyond pending recovery"));
        }
        let untouched = matches!(
            record.state,
            RecoveryState::Pending | RecoveryState::Prepared
        );
        let owner = record
            .owner_token
            .clone()
            .unwrap_or_else(|| format!("recovery-owner:{}", record.id));
        let leases = {
            let mut store = open_exclusive(store_root)
                .map_err(error)?
                .with_filesystem_identity(store_identity)?
                .with_metadata_identity(metadata_identity)?;
            let mut journal = NativeRecovery::new(&mut store).map_err(error)?;
            if record.state == RecoveryState::Pending {
                let evidence = record
                    .paths
                    .iter()
                    .map(|p| {
                        PathEvidence::new(&p.path, p.role, serde_json::json!({"untouched":true}))
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(error)?;
                journal
                    .prepare(
                        &record.id,
                        &RecoveryPreparation::new(serde_json::json!({"untouched":true}), evidence)
                            .map_err(error)?,
                        faults.lease_time_ms()?,
                    )
                    .map_err(error)?;
            }
            let leases = journal
                .claim(&record.id, &owner, faults.lease_time_ms()?, LEASE_MS)
                .map_err(error)?
                .leases;
            journal
                .heartbeat(
                    &record.id,
                    &owner,
                    &leases,
                    faults.lease_time_ms()?,
                    LEASE_MS,
                )
                .map_err(error)?
        };
        let mut lease = LeaseKeeper::new(
            store_root,
            &store_directory,
            &metadata_directory,
            &record.id,
            &owner,
            leases,
            faults,
        )?;
        if !untouched {
            let digest = record.payload["batchDigest"]
                .as_str()
                .ok_or_else(|| error("missing recovery digest"))?;
            if !digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b.is_ascii_lowercase())
            {
                return Err(error("invalid recovery digest"));
            }
            store_directory.verify_path_identity()?;
            metadata_directory.verify_path_identity()?;
            let backups = store_directory
                .open_child_for_traversal("metadata".as_ref())?
                .open_child_for_traversal("repair-backups".as_ref())?;
            let backup_name = match record.payload.get("attempt") {
                Some(value) => {
                    let attempt = value
                        .as_u64()
                        .filter(|v| *v > 0)
                        .ok_or_else(|| error("invalid recovery attempt"))?;
                    format!("{}-attempt-{attempt}", digest.replace(':', "-"))
                }
                None => digest.replace(':', "-"),
            };
            let backup = backups.open_child(backup_name.as_ref())?;
            // Recovery phase one quarantines all installed leaves before any
            // original is restored, so file/directory prefixes cannot collide.
            for (index, path) in record.paths.iter().enumerate() {
                lease.tick()?;
                checkout.verify_path_identity()?;
                let current = current_file(&checkout, &path.path)?;
                if committed {
                    if !matches_fingerprint(current.as_ref(), &path.evidence["after"], &mut || {
                        lease.tick()
                    })? {
                        return Err(error(
                            "committed checkout was edited; recovery needs manual inspection",
                        ));
                    }
                    continue;
                }
                let original = match backup.open_leaf(format!("old-{index}").as_ref()) {
                    Ok(file) => Some(file),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e),
                };
                if let Some(original) = original.as_ref() {
                    if !matches_fingerprint(Some(original), &path.evidence["before"], &mut || {
                        lease.tick()
                    })? {
                        return Err(error("original backup changed; manual inspection required"));
                    }
                }
                // No old backup means the old path was never moved, or it has
                // already been restored by an interrupted recovery attempt.
                if original.is_none() && !path.evidence["before"].is_null() {
                    if !matches_fingerprint(
                        current.as_ref(),
                        &path.evidence["before"],
                        &mut || lease.tick(),
                    )? {
                        return Err(error(
                            "original checkout changed during recovery; manual inspection required",
                        ));
                    }
                    continue;
                }
                if let Some(current) = current {
                    if !matches_fingerprint(Some(&current), &path.evidence["after"], &mut || {
                        lease.tick()
                    })? {
                        return Err(error(
                            "checkout edited after interruption; both current and backup preserved",
                        ));
                    }
                    if current.rename_to(&backup, format!("aborted-new-{index}").as_ref())? {
                        return Err(error("recovery quarantine already occupied"));
                    }
                    current.sync_parent()?;
                    backup.sync()?;
                    faults.check(MaterializationPoint::AfterBackup)?;
                }
            }
            if !committed {
                clear_obstructing_directories(
                    &checkout,
                    record
                        .paths
                        .iter()
                        .filter(|p| !p.evidence["after"].is_null())
                        .map(|p| p.path.as_str()),
                    record
                        .paths
                        .iter()
                        .filter(|p| !p.evidence["before"].is_null())
                        .map(|p| p.path.as_str()),
                    &mut || lease.tick(),
                )?;
                faults.check(MaterializationPoint::AfterDirectoryRemoval)?;
                // Phase two restores originals after every new leaf is removed.
                for (index, path) in record.paths.iter().enumerate() {
                    lease.tick()?;
                    let original = match backup.open_leaf(format!("old-{index}").as_ref()) {
                        Ok(file) => Some(file),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                        Err(e) => return Err(e),
                    };
                    if let Some(original) = original {
                        checkout.verify_path_identity()?;
                        let (parent, name) =
                            checkout.components_to_parent(Path::new(&path.path))?;
                        if original.rename_to(&parent, &name)? {
                            return Err(error("checkout path appeared during recovery"));
                        }
                        parent.sync()?;
                        backup.sync()?;
                        faults.check(MaterializationPoint::AfterPath)?;
                    }
                }
                for path in &record.paths {
                    lease.tick()?;
                    let current = current_file(&checkout, &path.path)?;
                    if !matches_fingerprint(
                        current.as_ref(),
                        &path.evidence["before"],
                        &mut || lease.tick(),
                    )? {
                        return Err(error(
                            "recovered checkout differs from original; backup retained",
                        ));
                    }
                }
            }
        }
        let path_results = record
            .paths
            .iter()
            .map(|p| {
                RecoveryPathResult::new(
                    &p.path,
                    p.role,
                    if committed {
                        RecoveryPathState::Applied
                    } else {
                        RecoveryPathState::RolledBack
                    },
                    serde_json::json!({"verified":true,"originalsRetained":true}),
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        lease.force()?;
        let mut store = open_exclusive(store_root)
            .map_err(error)?
            .with_filesystem_identity(store_identity)?
            .with_metadata_identity(metadata_identity)?;
        NativeRecovery::new(&mut store)
            .map_err(error)?
            .complete(
                &record.id,
                &owner,
                &lease.leases,
                serde_json::json!({"committed":committed,"recovered":true}),
                &path_results,
                faults.lease_time_ms()?,
            )
            .map_err(error)?;
        count += 1;
    }
    Ok(count)
}

fn matches_fingerprint(
    file: Option<&PinnedLeaf>,
    expected: &serde_json::Value,
    progress: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<bool> {
    let Some(file) = file else {
        return Ok(expected.is_null());
    };
    if let Some(target) = file.link_target()? {
        return Ok(
            expected["kind"] == "symlink" && expected["target"].as_str() == Some(target.as_str())
        );
    }
    let file = file
        .regular_file()
        .ok_or_else(|| error("unsupported checkout leaf"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if expected["executable"].as_bool()
            != Some(file.file().metadata()?.permissions().mode() & 0o111 != 0)
        {
            return Ok(false);
        }
    }
    if expected["kind"] != "file" || file.file().metadata()?.len() > limits().max_file_bytes as u64
    {
        return Ok(false);
    }
    let mut source = file.file();
    source.seek(SeekFrom::Start(0))?;
    let mut hash = blake3::Hasher::new();
    let v2 = expected["fileId"].as_str();
    if v2.is_some() {
        hash.update(b"layrs-file-v1\0");
    }
    let mut buffer = [0u8; 64 * 1024];
    let mut length = 0u64;
    loop {
        progress()?;
        let n = source.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        length = length
            .checked_add(n as u64)
            .ok_or_else(|| error("fingerprint size overflow"))?;
        if length > limits().max_file_bytes as u64 {
            return Ok(false);
        }
        hash.update(&buffer[..n]);
    }
    if let Some(id) = v2 {
        Ok(expected["byteLength"].as_u64() == Some(length)
            && id == format!("blake3:{}", hash.finalize().to_hex()))
    } else {
        // Existing durable journals use unprefixed BLAKE3. Preserve recovery
        // without loading the full original/new file into memory.
        Ok(expected["blake3"].as_str() == Some(hash.finalize().to_hex().as_str()))
    }
}
