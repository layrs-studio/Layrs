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
mod fresh;
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
    /// An existing checkout, before its files are staged (no journal yet).
    BeforeStaging,
    /// An existing checkout, its files staged, before its journal: a stop
    /// here leaves an attempt folder no journal names (swept later).
    AfterStaging,
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

#[allow(clippy::too_many_arguments)]
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
            end_staging: None,
            write_file_shared: None,
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
#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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
    // One shared session takes the checkout lock and reads the Worktree.
    // One shared session takes the checkout lock, reads the Worktree and
    // counts this batch's earlier attempts: a rolled-back attempt keeps its
    // journal and backups, so a retry of the same immutable batch needs
    // fresh physical paths. The checkout lock serializes attempts; the
    // journal session counts again.
    let (_checkout_lock, worktree, attempt, journaled_folders) = {
        let store = open(store_root)
            .map_err(error)?
            .with_filesystem_identity(store_identity)?
            .with_metadata_identity(metadata_identity)?;
        let lock = lock_checkout_in(&store, worktree_id)?;
        let worktree = super::NativeScmReader::new(&store)
            .map_err(error)?
            .get_worktree(worktree_id)
            .map_err(error)?
            .ok_or_else(|| error("missing Worktree"))?;
        let connection = store.connection().map_err(error)?;
        let attempt = attempts_of(&connection, worktree_id, batch)?
            .checked_add(1)
            .ok_or_else(|| error("materialization attempt overflow"))?;
        let journaled_folders = journaled_attempt_folders(&connection, worktree_id)?;
        drop(connection);
        (lock, worktree, attempt, journaled_folders)
    };
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
    let checkout = CheckoutReader::open(
        worktree.checkout_root(),
        Some(worktree.filesystem_identity()),
    )?;
    // Only the paths this transition touches are checked, like a git
    // checkout: every original it will move must still hold its exact `before`
    // content (and phase one rechecks each one right before moving it), and
    // every new leaf must be free. Untouched paths are never written, so a
    // concurrent edit there stays a working change. A conflict refuses here,
    // before any journal.
    let precheck = crate::perf::span("materialize.precheck");
    let pinned_checkout = PinnedDirectory::open(checkout.root())?;
    if pinned_checkout.identity_token() != worktree.filesystem_identity() {
        return Err(error("checkout identity changed"));
    }
    // A first materialization into a folder holding only `.layrs` writes in
    // place (council r5, D2): emptiness, proven again under the journal,
    // replaces the per-path precheck, as every path is new.
    let fresh =
        prior.is_empty() && !next.is_empty() && fresh::is_fresh(&pinned_checkout, checkout.root())?;
    if fresh {
        super::checkout_paths::validate_paths(paths.iter().copied(), cfg!(windows))?;
    } else if !touched_paths_match(&pinned_checkout, checkout.root(), &paths, &prior)? {
        return Err(error("checkout changed after capture"));
    }
    drop(pinned_checkout);
    drop(precheck);
    crate::perf::count("materialize.changed_paths", paths.len() as u64);
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
        if fresh {
            // Nothing can collide in a fresh folder: it is proven empty again
            // under the journal and every entry is created exclusively.
        } else if let Ok(meta) = fs::symlink_metadata(checkout.root().join(path)) {
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
    let recovery_id = RecoveryId::parse(format!(
        "recovery:{}:attempt:{attempt}",
        batch.request_digest()
    ))
    .map_err(error)?;
    let owner = format!("materializer:{}:{attempt}", batch.transaction_id().as_str());
    let attempt_name = format!(
        "{}-{}-attempt-{attempt}",
        attempt_prefix(worktree_id),
        batch.request_digest().to_string().replace(':', "-")
    );
    let backup_root = super::NativeStoreLayout::new(store_root)
        .map_err(error)?
        .repair_backups_dir()
        .join(&attempt_name);
    let checkout = PinnedDirectory::open(Path::new(worktree.checkout_root()))?;
    if checkout.identity_token() != worktree.filesystem_identity() {
        return Err(error("checkout identity changed"));
    }
    store_directory.verify_path_identity()?;
    metadata_directory.verify_path_identity()?;
    let store_dir = &store_directory;
    // On one NTFS volume (checkout and Store), directory updates are ordered
    // by the NTFS journal: none needs its own flush; one barrier before the
    // metadata commit makes every earlier one durable (as the CAS batches,
    // and git's batch mode, do). Elsewhere each directory is synced as before.
    #[cfg(windows)]
    let ordered = checkout.same_volume(store_dir)
        && super::change_index::local_ntfs(Path::new(worktree.checkout_root()));
    #[cfg(not(windows))]
    let ordered = false;
    let ensure = |parent: &PinnedDirectory, name: &std::ffi::OsStr| {
        if ordered {
            parent.ensure_child_unsynced(name)
        } else {
            parent.ensure_child(name)
        }
    };
    let sync = |directory: &PinnedDirectory| {
        if ordered { Ok(()) } else { directory.sync() }
    };
    // An existing checkout's files are staged BEFORE its journal (council s4,
    // Q1), so the journal is written directly applying with the evidence of
    // every staged file, in the session that validates the batch. Checkout
    // files are a projection of content that is already durable in the CAS
    // (or, for a received graph, installed durably before the commit).
    // Git-like contract (decided 2026-11-04): staged files are NOT flushed one
    // by one. Each staged file's identity is journaled, so after a crash a
    // file of ours whose bytes did not reach the disk is recognised: before
    // the commit it is quarantined like any installed leaf, after the commit
    // it is kept and shows as a working change. Originals moved in phase one
    // keep their per-move syncs, except on one NTFS volume where the journal
    // orders them and one barrier precedes the metadata commit (`ordered`).
    // A staging failure (missing or corrupt source, disk full) touched no
    // checkout path and wrote no journal: its attempt folder is removed.
    let (mut prestaged, mut evidence) = if fresh {
        (None, None)
    } else {
        faults.check(MaterializationPoint::BeforeStaging)?;
        let backups = ensure(
            &ensure(store_dir, "metadata".as_ref())?,
            "repair-backups".as_ref(),
        )?;
        sweep_unjournaled_attempts(&backups, worktree_id, &journaled_folders);
        // The attempt's own folder is always created durably (a synced
        // creation), before the journal is applying: recovery of an applying
        // journal needs it (council s2 r2).
        let backup = empty_attempt_folder(&backups, attempt_name.as_ref())?;
        let stage = crate::perf::span("materialize.stage");
        let staging = stage_all(&backup, &paths, &prior, &next, sources);
        if let Some(end_staging) = sources.end_staging.as_mut() {
            end_staging();
        }
        match staging.and_then(|staged| {
            sync(&backup)?;
            Ok(staged)
        }) {
            Ok((staged, evidence, written)) => {
                drop(stage);
                (Some((backups, backup, staged, written)), Some(evidence))
            }
            Err(failure) => {
                drop(backup);
                discard_attempt(&backups, attempt_name.as_ref());
                return Err(failure);
            }
        }
    };
    if !fresh {
        faults.check(MaterializationPoint::AfterStaging)?;
    }
    let (staged_root, staged_identity) = (
        worktree.checkout_root().to_owned(),
        worktree.filesystem_identity().to_owned(),
    );
    let journal_committed = std::cell::Cell::new(false);
    let journaled = (|| -> io::Result<_> {
        let _session = crate::perf::span("materialize.session_journal");
        let store = open_exclusive(store_root)
            .map_err(error)?
            .with_filesystem_identity(store_identity)?
            .with_metadata_identity(metadata_identity)?;
        // The journal is created on this writer, which ends with a checked
        // checkpoint (`journal.finish()`) before any checkout path moves.
        let connection = store.begin_session_writer().map_err(error)?;
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
        let scope = super::admission::memo_scope_of(store.store_id(), store.layout().root());
        CompositeAdmissionResolver::new(&reader, batch)
            .and_then(|r| {
                r.with_chunk_source(&resolve)
                    .with_memo_scope(scope)
                    .validate_batch()
            })
            .map_err(error)?;
        let active = worktree.active_operation_id().clone();
        let target = change.new_operation_id().clone();
        // The attempt counted under the checkout lock still holds.
        if attempts_of(&*connection.lock().map_err(error)?, worktree_id, batch)? + 1 != attempt {
            return Err(error("concurrent materialization attempt"));
        }
        let payload = if fresh {
            serde_json::json!({ "schema":fresh::SCHEMA, "batchDigest":batch.request_digest().to_string(), "attempt":attempt, "oldOperation":active, "newOperation":target, "checkoutRoot":worktree.checkout_root(), "filesystemIdentity":worktree.filesystem_identity(), "leaseScope":"worktree" })
        } else {
            serde_json::json!({ "schema":CHECKOUT_WRITE_STAGED_FIRST, "batchDigest":batch.request_digest().to_string(), "attempt":attempt, "oldOperation":active, "newOperation":target, "backupRoot":backup_root, "checkoutRoot":worktree.checkout_root(), "filesystemIdentity":worktree.filesystem_identity(), "leaseScope":"worktree" })
        };
        // A fresh checkout journals the top-level entries it creates: there is
        // no original to restore, and recovery quarantines the folder's content.
        let planned = if fresh {
            fresh::top_level(&paths)
        } else {
            paths.clone()
        };
        let path_plans = planned
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
        if !fresh && (root != staged_root || identity != staged_identity) {
            return Err(error("checkout changed between staging and journal"));
        }
        drop(reader);
        let connection = connection.into_inner().map_err(error)?;
        connection.execute_batch("ROLLBACK").map_err(error)?;
        // The journal is created on the same writer (council s3, D2).
        let mut journal = NativeRecovery::with_writer(&store, connection, cas);
        // Another failed or pending checkout write must be recovered first.
        if journal
            .list_pending()
            .map_err(error)?
            .iter()
            .any(|r| r.worktree_id == *worktree_id)
        {
            return Err(error("worktree_needs_repair: pending checkout journal"));
        }
        let leases = match evidence.take() {
            Some(evidence) => Some(
                journal
                    .create_and_claim(
                        &plan,
                        &RecoveryPreparation::new(serde_json::json!({"staged":true}), evidence)
                            .map_err(error)?,
                        &owner,
                        faults.lease_time_ms()?,
                        LEASE_MS,
                    )
                    .map_err(error)?
                    .leases,
            ),
            None => {
                journal.create(&plan).map_err(error)?;
                None
            }
        };
        journal_committed.set(true);
        // Durable before any checkout path moves.
        journal.finish().map_err(error)?;
        Ok((root, identity, active, target, leases))
    })();
    let (checkout_root, identity, active, target, leases) = match journaled {
        Ok(journaled) => journaled,
        Err(failure) => {
            // Without a journal nothing refers to the staged files.
            if let Some((backups, backup, staged, written)) =
                prestaged.take().filter(|_| !journal_committed.get())
            {
                drop((backup, staged, written));
                discard_attempt(&backups, attempt_name.as_ref());
            }
            return Err(failure);
        }
    };
    faults.check(MaterializationPoint::AfterJournal)?;
    if fresh {
        let (old_operation, new_operation) = (active.to_string(), target.to_string());
        return fresh::write(
            &fresh::FreshCheckout {
                store_root,
                store_identity,
                metadata_identity,
                store_directory: &store_directory,
                metadata_directory: &metadata_directory,
                worktree_id,
                checkout_root: &checkout_root,
                identity: &identity,
                recovery_id: &recovery_id,
                owner: &owner,
                old_operation: &old_operation,
                new_operation: &new_operation,
                batch,
                faults,
            },
            &paths,
            &next,
            sources,
        );
    }
    let (backups, backup, staged, written) =
        prestaged.ok_or_else(|| error("existing checkout was not staged"))?;
    let leases = leases.ok_or_else(|| error("staged journal holds no lease"))?;
    debug_assert_eq!(
        (&checkout_root, &identity),
        (&staged_root, &staged_identity)
    );
    let mut lease = LeaseKeeper::new(
        store_root,
        &store_directory,
        &metadata_directory,
        &recovery_id,
        &owner,
        leases,
        faults,
    )?;
    let phases = crate::perf::span("materialize.phases");
    // What each moved original looked like once proven equal to its tracked
    // content: the completed attempt deletes only backups still exactly so.
    let mut sealed: Vec<(String, BackupSeal)> = Vec::new();
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
            sync(&parent)?;
            sync(&backup)?;
            let protected = backup.open_leaf(format!("old-{index}").as_ref())?;
            if !matches_fingerprint(Some(&protected), &fingerprint(Some(content)), &mut || {
                lease.tick()
            })? {
                return Err(error(
                    "checkout leaf changed during backup; moved entry retained",
                ));
            }
            if let Some(seal) = BackupSeal::of(&protected) {
                sealed.push((format!("old-{index}"), seal));
            }
            faults.check(MaterializationPoint::AfterBackup)?;
        }
    }
    // Only remove empty directories that obstruct a desired leaf. Unexpected
    // children make the native nonrecursive removal fail with backups retained.
    // Only changed paths can be obstructed: an unchanged desired leaf is
    // already that exact leaf (precheck), and a directory replaced by a leaf
    // only held changed paths. Recovery passes its changed paths the same way.
    let changed = |path: &&str| paths.binary_search(path).is_ok();
    clear_obstructing_directories(
        &checkout,
        prior.keys().copied().filter(changed),
        next.keys().copied().filter(changed),
        &mut || lease.tick(),
    )?;
    faults.check(MaterializationPoint::AfterDirectoryRemoval)?;
    // Installs are made durable together, before the postcheck and the
    // metadata commit. Recovery decides each path independently (staged leaf
    // still in the backup, or installed leaf at its path, both fingerprinted),
    // so it never relies on one install being durable before the next; the
    // commit is what needs every install durable: each directory is synced
    // below, or (`ordered`, one NTFS volume) the one barrier after the last
    // install makes every earlier rename durable. Staged bytes are not
    // flushed (git-like contract above).
    let mut installed_into: BTreeMap<&str, PinnedDirectory> = BTreeMap::new();
    let mut installed_any = false;
    for path in &paths {
        let _install = crate::perf::span("phases.install");
        lease.tick()?;
        checkout.verify_path_identity()?;
        if let Some(file) = staged.get(path) {
            let parent_span = crate::perf::span("phases.parent");
            let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
            // One ensured (created and synced) parent per directory; its path
            // binding is still verified before every install into it.
            if !installed_into.contains_key(directory) {
                // Bound open handles: make the installs so far durable early.
                if installed_into.len() >= 64 {
                    for parent in installed_into.values() {
                        sync(parent)?;
                    }
                    installed_into.clear();
                }
                let (parent, _) = checkout.components_to_parent(Path::new(path))?;
                installed_into.insert(directory, parent);
            }
            let parent = &installed_into[directory];
            parent.verify_path_identity()?;
            drop(parent_span);
            if file.rename_to(parent, std::ffi::OsStr::new(name))? {
                return Err(error(
                    "checkout path appeared during materialization; original backup retained",
                ));
            }
            installed_any = true;
        }
        faults.check(MaterializationPoint::AfterPath)?;
    }
    for parent in installed_into.values() {
        lease.tick()?;
        sync(parent)?;
    }
    if installed_any {
        sync(&backup)?;
    }
    if ordered {
        // The one barrier: flushing a new file on the volume persists the
        // NTFS journal through every rename above, and the device cache.
        let _span = crate::perf::span("materialize.ntfs_barrier");
        let name = format!("barrier-{}", std::process::id());
        let mut marker = backup.create_file(name.as_ref())?;
        use std::io::Write as _;
        let flushed = marker
            .file_mut()
            .write_all(b"layrs materialization barrier\n")
            .and_then(|()| marker.file().sync_all());
        // A marker that cannot be removed (held open by a scanner) stays in
        // the attempt folder like any unpurged backup: only the flush counts.
        let _ = marker.unlink();
        flushed?;
    }
    drop(installed_into);
    drop(phases);
    let postcheck = crate::perf::span("materialize.postcheck");
    // A change index (ADR 0025) learns each file proven below, with the
    // handle's metadata, as of no later than this instant.
    let indexed = super::change_index::ChangeIndex::is_enabled(store_root, worktree_id);
    let proven_at = super::change_index::now();
    let mut proven: Vec<(String, super::change_index::IndexEntry)> = Vec::new();
    // A file this materialization installed is vouched for through the very
    // handle that wrote its proven bytes: still that file at exactly that
    // path, same length and modification time as right after the write.
    // Reopening and rehashing it by path is what made fresh checkouts slow
    // (the antivirus scans every new file on its first open). Untouched
    // paths, listings and identities are rechecked as before.
    let mut trust = |path: &str, full: &Path| -> Option<super::checkout::TrustedFile> {
        let file = staged.get(path)?.regular_file()?;
        let CheckoutProjectionContent::File {
            file_id,
            byte_length,
            executable,
        } = next.get(path).copied()?
        else {
            return None;
        };
        let stamp = written.get(path)?;
        let metadata = file.file().metadata().ok()?;
        let now = StagedStamp {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        };
        let trusted = stamp.modified.is_some()
            && *stamp == now
            && now.length == *byte_length
            && file.is_at(full);
        if trusted && indexed {
            if let Some(stat) = super::change_index::StatKey::of(&metadata) {
                proven.push((
                    path.to_owned(),
                    super::change_index::IndexEntry {
                        file_id: file_id.clone(),
                        length: *byte_length,
                        executable: *executable,
                        stat,
                        verified_at: proven_at,
                        native: None,
                    },
                ));
            }
        }
        trusted.then(|| super::checkout::TrustedFile {
            file_id: file_id.clone(),
            length: *byte_length,
            executable: *executable,
        })
    };
    if !touched_paths_installed(
        &checkout,
        Path::new(&checkout_root),
        &paths,
        &next,
        &mut trust,
        &mut || lease.tick(),
    )? {
        return Err(error(
            "checkout changed during materialization; backup retained",
        ));
    }
    drop(postcheck);
    // Closing a freshly written file is where the antivirus scans it: close
    // the staged handles on a few threads while the metadata commits. They
    // are joined before this function returns.
    let closer = close_in_background(staged.into_values().collect());
    faults.check(MaterializationPoint::BeforeMetadata)?;
    let metadata = crate::perf::span("materialize.metadata_and_complete");
    // The metadata transaction itself checks the permit, the `applying`
    // state, the batch digest and that no lease expired or was released,
    // which is stronger than a heartbeat: renew only when one is due
    // (council r5, D3).
    lease.tick()?;
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
    let completion_evidence =
        serde_json::json!({"oldOperation":active,"newOperation":target,"verified":true});
    // The metadata and the completed journal commit in one transaction (council
    // s2 r1, B2): a crash leaves either the old state with an Applying
    // journal, or the new state with a completed one.
    let completion = super::repository::MaterializationCompletion {
        leases: lease.leases.clone(),
        evidence: completion_evidence.clone(),
        path_results: results.clone(),
    };
    let clock = || {
        faults
            .lease_time_ms()
            .map_err(|failure| layrs_store::scm::StoreError::Backend {
                message: format!("lease clock: {failure}"),
            })
    };
    let outcome =
        NativeScmRepository::for_materialization(store_root, recovery_id.as_str(), &owner)
            .with_store_identity(store_identity)
            .with_metadata_identity(metadata_identity)
            .apply_batch_completing(
                batch,
                sources.chunks,
                &mut sources.read_chunk,
                &completion,
                &clock,
            )
            .map_err(io::Error::other)?;
    faults.check(MaterializationPoint::AfterMetadata)?;
    if matches!(outcome, ApplyBatchOutcome::AlreadyApplied { .. }) {
        // A batch applied earlier (a replayed request): its journal is
        // completed as before, in its own transaction.
        lease.tick()?;
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
                completion_evidence,
                &results,
                faults.lease_time_ms()?,
            )
            .map_err(error)?;
    }
    drop(metadata);
    {
        let _close = crate::perf::span("materialize.close_staged");
        drop(closer);
    }
    {
        // The completed journal no longer needs its originals: each was proven
        // equal to tracked content (durable in the CAS) before it moved, so
        // keeping them only doubled the disk use of every replaced file.
        // Best effort: a leftover backup is harmless and never read again.
        let _purge = crate::perf::span("materialize.purge_backup");
        purge_completed_backup(&backups, backup, &backup_root, &sealed);
    }
    if indexed {
        // A cache: failing to record it only makes the next capture read more.
        let _span = crate::perf::span("materialize.change_index");
        if let Some(mut index) = super::change_index::ChangeIndex::load(store_root, worktree_id) {
            // Entries written against the prior projection (council s3, D1)
            // are resolved by it, then the index is bound to the new one.
            index.resolve(before);
            for path in &paths {
                index.remove_path(path);
            }
            for (path, entry) in proven {
                index.insert(path, entry);
            }
            index.bind(&projection, after);
            if index.save(store_root).is_err() {
                crate::perf::count("materialize.change_index_unsaved", 1);
            }
        }
    }
    Ok(outcome)
}

/// Journal of an existing checkout's write, staged after its journal
/// (older attempts; still recovered).
const CHECKOUT_WRITE: &str = "layrs.checkout-write.v1";
/// Journal of an existing checkout's write staged before it (council s4,
/// Q1), its attempt folder named after the Worktree. A binary that knows
/// only v1 leaves it pending (captures refuse) instead of looking for the
/// folder under the other name.
const CHECKOUT_WRITE_STAGED_FIRST: &str = "layrs.checkout-write.v2";

/// Journals of `batch` already recorded for `worktree` (its attempts).
fn attempts_of(
    connection: &rusqlite::Connection,
    worktree: &WorktreeId,
    batch: &WriteBatch,
) -> io::Result<u64> {
    connection
        .query_row(
            "SELECT count(*) FROM recovery_operations WHERE worktree_id=?1 \
             AND json_extract(payload_json,'$.batchDigest')=?2",
            rusqlite::params![worktree.as_str(), batch.request_digest().to_string()],
            |row| row.get(0),
        )
        .map_err(error)
}

/// The file whose exclusive lock serializes a Worktree's checkout writes.
fn checkout_lock_name(worktree: &WorktreeId) -> String {
    format!(
        "checkout-{}.lock",
        blake3::hash(worktree.as_str().as_bytes()).to_hex()
    )
}

/// The name prefix of this Worktree's attempt folders: only a holder of
/// its checkout lock creates or sweeps them.
fn attempt_prefix(worktree: &WorktreeId) -> String {
    let digest = blake3::hash(worktree.as_str().as_bytes()).to_hex();
    format!("w{}", &digest.as_str()[..16])
}

/// The folder names of every journal of `worktree` (`backupRoot`).
fn journaled_attempt_folders(
    connection: &rusqlite::Connection,
    worktree: &WorktreeId,
) -> io::Result<std::collections::BTreeSet<std::ffi::OsString>> {
    let mut statement = connection
        .prepare(
            "SELECT json_extract(payload_json,'$.backupRoot') FROM recovery_operations \
             WHERE worktree_id=?1",
        )
        .map_err(error)?;
    let rows = statement
        .query_map([worktree.as_str()], |row| row.get::<_, Option<String>>(0))
        .map_err(error)?;
    let mut names = std::collections::BTreeSet::new();
    for row in rows {
        if let Some(name) = row
            .map_err(error)?
            .and_then(|root| Path::new(&root).file_name().map(ToOwned::to_owned))
        {
            names.insert(name);
        }
    }
    Ok(names)
}

/// Best effort, for a retired Worktree (`room drop`): its attempt folders
/// that no journal names, staged by a process stopped before writing its
/// journal (council s5 r2). Skipped while another process holds the
/// Worktree's checkout lock.
pub fn sweep_retired_attempts(
    store_root: &Path,
    store_identity: &str,
    metadata_identity: &str,
    worktree_id: &WorktreeId,
) -> io::Result<()> {
    // Staging only ever runs under this lock: a Worktree that never took it
    // has no attempt folder, and its lock file is not created now.
    if !store_root
        .join("metadata")
        .join(checkout_lock_name(worktree_id))
        .is_file()
    {
        return Ok(());
    }
    let store_directory = pin_store(store_root, store_identity)?;
    let metadata_directory = pin_store(&store_root.join("metadata"), metadata_identity)?;
    let _checkout_lock = lock_checkout(store_root, store_identity, metadata_identity, worktree_id)?;
    sweep_after_recovery(
        &store_directory,
        &metadata_directory,
        store_root,
        worktree_id,
    );
    Ok(())
}

/// Best effort, during a recovery (under the checkout lock): this
/// Worktree's attempt folders that no journal names, as a materialization
/// does before staging. A Worktree never materialized again is still
/// cleaned when it is recovered.
fn sweep_after_recovery(
    store_directory: &PinnedDirectory,
    metadata_directory: &PinnedDirectory,
    store_root: &Path,
    worktree: &WorktreeId,
) {
    let journaled = open(store_root)
        .ok()
        .and_then(|store| journaled_attempt_folders(&*store.connection().ok()?, worktree).ok());
    let Some(journaled) = journaled else {
        return;
    };
    if store_directory.verify_path_identity().is_err()
        || metadata_directory.verify_path_identity().is_err()
    {
        return;
    }
    if let Ok(backups) = metadata_directory.open_child("repair-backups".as_ref()) {
        sweep_unjournaled_attempts(&backups, worktree, &journaled);
    }
}

/// Best effort, under the checkout lock: removes this Worktree's attempt
/// folders that no journal names (a process stopped between staging and
/// writing its journal) and that hold only staged files. Any other folder,
/// of another Worktree or with other content, is left alone.
fn sweep_unjournaled_attempts(
    backups: &PinnedDirectory,
    worktree: &WorktreeId,
    journaled: &std::collections::BTreeSet<std::ffi::OsString>,
) {
    let prefix = format!("{}-", attempt_prefix(worktree));
    let Ok(entries) = fs::read_dir(backups.display_path()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let ours = name
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix) && name.contains("-attempt-"));
        if ours && !journaled.contains(&name) {
            crate::perf::count("materialize.swept_attempt", 1);
            discard_attempt(backups, &name);
        }
    }
}

/// The attempt's folder, empty and created durably (a synced creation). A
/// folder of the same attempt left by a process that stopped before writing
/// its journal holds only staged files and barrier markers, which are
/// removed first. Anything else refuses: an original is only ever moved
/// into the folder of an attempt whose journal is applying.
fn empty_attempt_folder(
    backups: &PinnedDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<PinnedDirectory> {
    match backups.open_child(name) {
        Ok(existing) => {
            clear_staged(&existing)?;
            drop(existing);
            backups.remove_empty_child(name)?;
        }
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
        Err(failure) => return Err(failure),
    }
    backups.create_new_child(name)
}

/// Removes the staged files (`new-*`) and barrier markers of an attempt
/// folder that no journal refers to; refuses anything else.
fn clear_staged(folder: &PinnedDirectory) -> io::Result<()> {
    for entry in fs::read_dir(folder.display_path())? {
        let name = entry?.file_name();
        let staged = name
            .to_str()
            .is_some_and(|name| name.starts_with("new-") || name.starts_with("barrier-"));
        if !staged {
            return Err(error(
                "worktree_needs_repair: unexpected entry in an attempt folder without a journal",
            ));
        }
        folder.unlink_file(&name)?;
    }
    Ok(())
}

/// Best effort: the attempt folder of a materialization that wrote no
/// journal. Left in place on failure (the next attempt clears it).
fn discard_attempt(backups: &PinnedDirectory, name: &std::ffi::OsStr) {
    if let Ok(folder) = backups.open_child(name) {
        if clear_staged(&folder).is_ok() {
            drop(folder);
            let _ = backups.remove_empty_child(name);
        }
    }
}

/// Identity and metadata of a moved original, observed right after it was
/// proven equal to its tracked content.
struct BackupSeal {
    identity: String,
    length: u64,
    modified: Option<std::time::SystemTime>,
}
impl BackupSeal {
    fn of(leaf: &PinnedLeaf) -> Option<Self> {
        #[allow(irrefutable_let_patterns)]
        let PinnedLeaf::File(file) = leaf else {
            return None;
        };
        let metadata = file.file().metadata().ok()?;
        Some(Self {
            identity: file.identity_token(),
            length: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// Removes a completed attempt's backups: only the journal's own `old-N`
/// entries, through the pinned backup directory (never a path that could be
/// substituted), each reopened and still the very file proven equal to its
/// tracked content (same identity, length and modification time). Anything
/// else, an entry that cannot be checked or deleted included, is retained,
/// and so is the attempt's directory. A writer still holding the file makes
/// the check fail (Windows); one that wrote and closed changed its metadata.
/// The remaining window between check and deletion is the one git's
/// checkout also has.
fn purge_completed_backup(
    backups: &PinnedDirectory,
    backup: PinnedDirectory,
    root: &Path,
    sealed: &[(String, BackupSeal)],
) {
    let mut deleted = 0usize;
    for (name, seal) in sealed {
        // On Windows the check holds the file against writers: an open that
        // must refuse any existing write handle fails if another process
        // still holds one, and the backup is then retained.
        #[cfg(windows)]
        let checked = backup
            .open_immutable_file(name.as_ref())
            .map(PinnedLeaf::File);
        #[cfg(not(windows))]
        let checked = backup.open_leaf(name.as_ref());
        let unchanged = checked.is_ok_and(|leaf| {
            BackupSeal::of(&leaf).is_some_and(|now| {
                now.identity == seal.identity
                    && now.length == seal.length
                    && now.modified == seal.modified
            })
        });
        if unchanged && backup.unlink_file(name.as_ref()).is_ok() {
            deleted += 1;
        }
    }
    drop(backup);
    if deleted == sealed.len() {
        if let Some(name) = root.file_name() {
            let _ = backups.remove_empty_child(name);
        }
    }
}

/// Drops `leaves` on a few threads; the returned guard joins them on drop.
fn close_in_background(leaves: Vec<PinnedLeaf>) -> BackgroundClose {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    let per = leaves.len().div_ceil(threads).max(1);
    let mut leaves = leaves;
    let mut workers = Vec::new();
    while !leaves.is_empty() {
        let batch = leaves.split_off(leaves.len().saturating_sub(per));
        match std::thread::Builder::new().spawn(move || drop(batch)) {
            Ok(worker) => workers.push(worker),
            // Could not spawn: the batch was moved into the failed closure
            // and is dropped (closed) here, on this thread.
            Err(_) => {}
        }
    }
    BackgroundClose(workers)
}

struct BackgroundClose(Vec<std::thread::JoinHandle<()>>);

impl Drop for BackgroundClose {
    fn drop(&mut self) {
        for worker in self.0.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Length and modification time of a staged file right after its bytes were
/// written; the postcheck trusts an installed file only while both still
/// hold through the handle that wrote it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct StagedStamp {
    length: u64,
    modified: Option<std::time::SystemTime>,
}

type Staged<'a> = (
    BTreeMap<&'a str, PinnedLeaf>,
    Vec<PathEvidence>,
    BTreeMap<&'a str, StagedStamp>,
);

/// Writes every new leaf into the backup directory as `new-{index}` (file
/// bytes verified against their FileId while written) and builds the journal
/// evidence, including each staged file's identity.
fn stage_all<'a>(
    backup: &PinnedDirectory,
    paths: &[&'a str],
    prior: &BTreeMap<&'a str, &CheckoutProjectionContent>,
    next: &BTreeMap<&'a str, &CheckoutProjectionContent>,
    sources: &mut CheckoutSources<'_>,
) -> io::Result<Staged<'a>> {
    let jobs: Vec<sources::StageJob<'_>> = paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| match next.get(path).copied() {
            Some(CheckoutProjectionContent::File {
                file_id,
                byte_length,
                ..
            }) => Some(sources::StageJob {
                index,
                path,
                file_id,
                length: *byte_length,
            }),
            _ => None,
        })
        .collect();
    let mut written = sources::stage_files(backup, &jobs, sources)?;
    let mut staged = BTreeMap::new();
    let mut evidence = Vec::with_capacity(paths.len());
    let mut stamps = BTreeMap::new();
    for (index, path) in paths.iter().enumerate() {
        let mut staged_identity = serde_json::Value::Null;
        match next.get(path).copied() {
            Some(CheckoutProjectionContent::File { executable, .. }) => {
                let file = written
                    .remove(&index)
                    .ok_or_else(|| error("staged file is missing"))?;
                set_executable(file.file(), *executable)?;
                let metadata = file.file().metadata()?;
                stamps.insert(
                    *path,
                    StagedStamp {
                        length: metadata.len(),
                        modified: metadata.modified().ok(),
                    },
                );
                staged_identity = serde_json::json!({
                    "identity": file.identity_token(),
                    "length": metadata.len(),
                    "modified": modified_nanos(metadata.modified().ok()),
                });
                staged.insert(*path, PinnedLeaf::File(file));
            }
            Some(CheckoutProjectionContent::Symlink { target }) => {
                staged.insert(
                    *path,
                    backup.create_symlink(format!("new-{index}").as_ref(), target)?,
                );
            }
            None => {}
        }
        evidence.push(
            PathEvidence::new(
                *path,
                RecoveryPathRole::Materialized,
                serde_json::json!({
                    "backup": format!("old-{index}"),
                    "before": fingerprint(prior.get(path).copied()),
                    "after": fingerprint(next.get(path).copied()),
                    "staged": staged_identity,
                }),
            )
            .map_err(error)?,
        );
    }
    Ok((staged, evidence, stamps))
}

/// Pre-journal check of exactly the touched paths (see the precheck).
fn touched_paths_match(
    checkout: &PinnedDirectory,
    root: &Path,
    paths: &[&str],
    prior: &BTreeMap<&str, &CheckoutProjectionContent>,
) -> io::Result<bool> {
    // A fresh, empty checkout (worktree creation) has nothing to compare.
    if prior.is_empty() && fs::read_dir(root)?.next().is_none() {
        return Ok(true);
    }
    for path in paths {
        let current = current_file(checkout, path)?;
        if !matches_fingerprint(
            current.as_ref(),
            &fingerprint(prior.get(path).copied()),
            &mut || Ok(()),
        )? {
            return Ok(false);
        }
        if prior.contains_key(path) {
            continue;
        }
        // A new leaf may only take the place of a directory that is empty or
        // whose tracked content this transition removes (the obstruction pass
        // then removes it), and every existing ancestor must be a directory
        // or a tracked file this transition removes.
        let tracked_below = |directory: &str| {
            let prefix = format!("{directory}/");
            prior
                .range::<&str, _>(prefix.as_str()..)
                .next()
                .is_some_and(|(p, _)| p.starts_with(&prefix))
        };
        if let Ok(meta) = fs::symlink_metadata(root.join(path)) {
            if !meta.is_dir()
                || (!tracked_below(path) && fs::read_dir(root.join(path))?.next().is_some())
            {
                return Ok(false);
            }
        }
        for (end, _) in path.match_indices('/') {
            let ancestor = &path[..end];
            if let Ok(meta) = fs::symlink_metadata(root.join(ancestor)) {
                if !meta.is_dir() && !prior.contains_key(ancestor) {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

/// Post-install check of exactly the touched paths: each new leaf holds its
/// `after` content (vouched for by the handle that wrote it, else read and
/// fingerprinted) and each removed path is absent.
fn touched_paths_installed(
    checkout: &PinnedDirectory,
    root: &Path,
    paths: &[&str],
    next: &BTreeMap<&str, &CheckoutProjectionContent>,
    trust: &mut dyn FnMut(&str, &Path) -> Option<super::checkout::TrustedFile>,
    progress: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<bool> {
    checkout.verify_path_identity()?;
    for path in paths {
        progress()?;
        if matches!(next.get(path), Some(CheckoutProjectionContent::File { .. }))
            && trust(
                path,
                &path
                    .split('/')
                    .fold(root.to_path_buf(), |full, part| full.join(part)),
            )
            .is_some()
        {
            continue;
        }
        let current = current_file(checkout, path)?;
        if !matches_fingerprint(
            current.as_ref(),
            &fingerprint(next.get(path).copied()),
            &mut *progress,
        )? {
            return Ok(false);
        }
    }
    checkout.verify_path_identity()?;
    Ok(true)
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
        // Materialization streams one chunk at a time (ADR 0027).
        max_file_bytes: usize::MAX,
        max_total_bytes: usize::MAX,
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
    lock_checkout_in(&store, worktree)
}

/// `lock_checkout` in an open, bound session.
fn lock_checkout_in(
    store: &super::NativeStoreOpen,
    worktree: &WorktreeId,
) -> io::Result<PinnedFile> {
    let metadata = PinnedDirectory::open(store.layout().metadata_dir())?;
    let name = checkout_lock_name(worktree);
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
    for record in pending
        .iter()
        .filter(|r| r.worktree_id == *worktree_id && r.payload["schema"] == fresh::SCHEMA)
    {
        fresh::recover(
            store_root,
            store_identity,
            metadata_identity,
            &store_directory,
            &metadata_directory,
            &worktree,
            record,
            faults,
        )?;
        count += 1;
    }
    sweep_after_recovery(
        &store_directory,
        &metadata_directory,
        store_root,
        worktree_id,
    );
    for record in pending.into_iter().filter(|r| {
        r.worktree_id == *worktree_id
            && (r.payload["schema"] == CHECKOUT_WRITE
                || r.payload["schema"] == CHECKOUT_WRITE_STAGED_FIRST)
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
                    let name = format!("{}-attempt-{attempt}", digest.replace(':', "-"));
                    // Staged before their journal, attempts are named after
                    // their Worktree (swept under its checkout lock).
                    // (`worktreePrefixed`: unreleased v1 journals of the
                    // same order, council s5 r2.)
                    if record.payload["schema"] == CHECKOUT_WRITE_STAGED_FIRST
                        || record.payload.get("worktreePrefixed")
                            == Some(&serde_json::Value::Bool(true))
                    {
                        format!("{}-{name}", attempt_prefix(&record.worktree_id))
                    } else {
                        name
                    }
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
                    // Our own installed file whose unflushed bytes did not
                    // survive (checkout durability contract) is kept: it shows
                    // as a working change, its content stays durable in the
                    // CAS. Anything else that differs is someone's edit.
                    if !matches_fingerprint(current.as_ref(), &path.evidence["after"], &mut || {
                        lease.tick()
                    })? && !staged_by_us(current.as_ref(), &path.evidence, false)
                    {
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
                    // Our own installed file whose unflushed bytes did not
                    // survive is quarantined (moved, never deleted) only if
                    // nothing wrote to it after us: same identity, length and
                    // modification time as journaled right after our write.
                    // Identity alone is provenance, not authority to move an
                    // edit made after the interruption.
                    if !matches_fingerprint(Some(&current), &path.evidence["after"], &mut || {
                        lease.tick()
                    })? && !staged_by_us(Some(&current), &path.evidence, true)
                    {
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

/// Whether `current` is the very file this materialization staged for the
/// path (identity journaled with the evidence before any install) and, with
/// `unchanged`, still has the length and modification time journaled right
/// after our write (nothing wrote to it since).
fn staged_by_us(
    current: Option<&PinnedLeaf>,
    evidence: &serde_json::Value,
    unchanged: bool,
) -> bool {
    let staged = &evidence["staged"];
    let Some(current) = current else {
        return false;
    };
    if current.identity_token().as_deref() != staged["identity"].as_str()
        || staged["identity"].is_null()
    {
        return false;
    }
    if !unchanged {
        return true;
    }
    let Some(file) = current.regular_file() else {
        return false;
    };
    let Ok(metadata) = file.file().metadata() else {
        return false;
    };
    let modified = modified_nanos(metadata.modified().ok());
    !modified.is_null()
        && staged["modified"] == modified
        && staged["length"].as_u64() == Some(metadata.len())
}

/// A modification time as decimal nanoseconds since the Unix epoch (JSON
/// string, exact), or null when unavailable.
fn modified_nanos(modified: Option<std::time::SystemTime>) -> serde_json::Value {
    modified
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(serde_json::Value::Null, |since| {
            serde_json::Value::String(since.as_nanos().to_string())
        })
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
