//! Fresh checkout (council r5, D2). A Worktree's first materialization into a
//! folder that holds nothing but its `.layrs` control directory writes every
//! file in place. Each file is created exclusively relative to its pinned
//! parent, and its bytes are proven against the FileId while written. Nothing
//! is staged in the Store and then renamed into place.
//!
//! There is no original to protect, so the journal records the top-level
//! entries it creates instead of every path. Recovery of an uncommitted fresh
//! checkout quarantines every entry but `.layrs` by renaming it into a sibling
//! folder on the same volume, and never deletes. A committed one is kept: its
//! files are then the Worktree's working copy, and any difference shows as a
//! change. Recovery decides by the Worktree's active Operation, so the
//! metadata commit and the journal completion need not share one transaction:
//! a committed fresh checkout is never quarantined.
use super::*;
use layrs_core::scm::Worktree;
use std::collections::BTreeSet;

/// Payload schema of a fresh checkout journal. An older binary does not
/// know it, sees a pending checkout journal and refuses: it fails closed.
pub(super) const SCHEMA: &str = "layrs.fresh-checkout.v1";

/// Upper bound on entries listed when proving a folder fresh.
const PROBE: usize = 1;

/// Names in `root` other than the control directory, listed between two
/// bindings of the pinned handle to its path (one path check per directory,
/// like the checkout scan). At most `limit + 1` names are returned.
pub(super) fn foreign_entries(
    checkout: &PinnedDirectory,
    root: &Path,
    limit: usize,
) -> io::Result<Vec<String>> {
    checkout.verify_path_identity()?;
    let mut names = Vec::new();
    for entry in fs::read_dir(root)? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| error("checkout filename is not UTF-8"))?;
        if name != ".layrs" {
            names.push(name);
            if names.len() > limit {
                break;
            }
        }
    }
    checkout.verify_path_identity()?;
    names.sort();
    Ok(names)
}

/// Whether the pinned checkout is a fresh folder: only `.layrs` inside.
pub(super) fn is_fresh(checkout: &PinnedDirectory, root: &Path) -> io::Result<bool> {
    Ok(foreign_entries(checkout, root, PROBE)?.is_empty())
}

/// The first component of every path, sorted and unique: the entries a fresh
/// checkout creates in its root, and the journal's path rows.
pub(super) fn top_level<'a>(paths: &[&'a str]) -> Vec<&'a str> {
    let mut top: Vec<&str> = paths
        .iter()
        .map(|path| path.split_once('/').map_or(*path, |(first, _)| first))
        .collect();
    top.sort_unstable();
    top.dedup();
    top
}

/// What the journal of an attempt already fixed, shared by every phase.
pub(super) struct FreshCheckout<'a> {
    pub store_root: &'a Path,
    pub store_identity: &'a str,
    pub metadata_identity: &'a str,
    pub store_directory: &'a PinnedDirectory,
    pub metadata_directory: &'a PinnedDirectory,
    pub worktree_id: &'a WorktreeId,
    pub checkout_root: &'a str,
    pub identity: &'a str,
    pub recovery_id: &'a RecoveryId,
    pub owner: &'a str,
    pub old_operation: &'a str,
    pub new_operation: &'a str,
    pub batch: &'a WriteBatch,
    pub faults: &'a dyn MaterializationFaults,
}

/// Every directory a fresh checkout created, still open, and the metadata of
/// every file it wrote, taken through the writing handle.
struct Written<'a> {
    directories: BTreeMap<String, PinnedDirectory>,
    files: BTreeMap<&'a str, super::super::change_index::StatKey>,
    /// Exact children of each directory ("" is the root), `.layrs` included.
    children: BTreeMap<String, BTreeSet<String>>,
}

/// Writes the checkout in place, then commits the metadata and completes the
/// journal. On a write failure the entries created so far are quarantined and
/// the journal is closed as rolled back; any later failure leaves the journal
/// for recovery, which decides by the Worktree's active Operation.
pub(super) fn write(
    fresh: &FreshCheckout<'_>,
    paths: &[&str],
    next: &BTreeMap<&str, &CheckoutProjectionContent>,
    sources: &mut CheckoutSources<'_>,
) -> io::Result<ApplyBatchOutcome> {
    let root = Path::new(fresh.checkout_root);
    let top = top_level(paths);
    let checkout = PinnedDirectory::open(root)?;
    if checkout.identity_token() != fresh.identity {
        return Err(error("checkout identity changed"));
    }
    fresh.store_directory.verify_path_identity()?;
    fresh.metadata_directory.verify_path_identity()?;
    // Under the journal, the folder must still be fresh: an entry that
    // appeared since is someone else's and is left exactly where it is.
    if !is_fresh(&checkout, root)? {
        close(
            fresh,
            &top,
            None,
            false,
            serde_json::json!({"untouched": true}),
        )?;
        return Err(error(
            "the new Worktree folder is no longer empty; nothing was written",
        ));
    }
    // With a change index on a volume with a fine timestamp margin (NTFS,
    // ReFS: 100 ms), each written file keeps its handle, which shares no
    // write access, until that margin has passed since its last write
    // (council s11 r1): no other process can change it meanwhile, and any
    // later write stamps another modification time. Its proof is then not
    // "racy", so the first capture reads none of the files just written,
    // while every rewrite stays visible to the stat comparison. A coarse
    // margin would cost seconds: its first capture rereads instead.
    let indexed =
        super::super::change_index::ChangeIndex::is_enabled(fresh.store_root, fresh.worktree_id);
    let margin = super::super::change_index::margin_nanos(root);
    let settling = (indexed && (0..=250_000_000).contains(&margin)).then(|| {
        SettlingClose::new(std::time::Duration::from_nanos(
            u64::try_from(margin).unwrap_or(0),
        ))
    });
    let span = crate::perf::span("fresh.write");
    let written = write_tree(&checkout, paths, next, sources, settling.as_ref());
    if let Some(end_staging) = sources.end_staging.as_mut() {
        end_staging();
    }
    let written = match written {
        Ok(written) => written,
        Err(failure) => {
            // Every writer has stopped and every handle is closed (held ones
            // now, without waiting): what was created is ours, moved aside,
            // never deleted.
            if let Some(settling) = settling {
                settling.end(false);
            }
            let rolled_back = quarantine(
                fresh.checkout_root,
                fresh.identity,
                &digest_tag(fresh),
                &mut || Ok(()),
            )
            .and_then(|moved| {
                close(
                    fresh,
                    &top,
                    None,
                    false,
                    serde_json::json!({"writeFailed": true, "quarantine": moved}),
                )
            });
            return Err(match rolled_back {
                Ok(()) => failure,
                Err(rollback) => error(format!(
                    "{failure}; the partial checkout was retained for recovery: {rollback}"
                )),
            });
        }
    };
    drop(span);
    fresh.faults.check(MaterializationPoint::AfterPath)?;
    // Every created directory is durable before anything relies on it (file
    // bytes follow the checkout durability contract of `materialize`).
    {
        let _span = crate::perf::span("fresh.sync");
        sync_all(
            written
                .directories
                .values()
                .chain(std::iter::once(&checkout)),
        )?;
    }
    let leases = prepare_and_claim(fresh, &top, serde_json::json!({"fresh": true}))?;
    let mut lease = LeaseKeeper::new(
        fresh.store_root,
        fresh.store_directory,
        fresh.metadata_directory,
        fresh.recovery_id,
        fresh.owner,
        leases,
        fresh.faults,
    )?;
    if let Some(settling) = settling {
        // Every handle closed once its margin passed (monotonic waits, at
        // most the margin after the last write): the proof below follows.
        let _span = crate::perf::span("fresh.settle");
        settling.end(true);
    }
    let proven_at = super::super::change_index::now();
    let proven = {
        let _span = crate::perf::span("fresh.verify");
        verify(&checkout, root, &written, next, proven_at, &mut || {
            lease.tick()
        })?
    };
    drop(written);
    fresh.faults.check(MaterializationPoint::BeforeMetadata)?;
    let metadata = crate::perf::span("materialize.metadata_and_complete");
    lease.tick()?;
    let outcome = NativeScmRepository::for_materialization(
        fresh.store_root,
        fresh.recovery_id.as_str(),
        fresh.owner,
    )
    .with_store_identity(fresh.store_identity)
    .with_metadata_identity(fresh.metadata_identity)
    .apply_batch_with_chunks(fresh.batch, sources.chunks, &mut sources.read_chunk)
    .map_err(io::Error::other)?;
    fresh.faults.check(MaterializationPoint::AfterMetadata)?;
    lease.tick()?;
    close(
        fresh,
        &top,
        Some(&lease.leases),
        true,
        serde_json::json!({
            "oldOperation": fresh.old_operation,
            "newOperation": fresh.new_operation,
            "verified": true,
            "fresh": true,
        }),
    )?;
    drop(metadata);
    if indexed {
        // A cache: failing to record it only makes the next capture read more.
        let _span = crate::perf::span("materialize.change_index");
        if let Some(mut index) =
            super::super::change_index::ChangeIndex::load(fresh.store_root, fresh.worktree_id)
        {
            for (path, entry) in proven {
                index.insert(path, entry);
            }
            if index.save(fresh.store_root).is_err() {
                crate::perf::count("materialize.change_index_unsaved", 1);
            }
        }
    }
    Ok(outcome)
}

/// Creates every directory in order, then every file concurrently, each
/// exclusively and relative to its pinned parent.
fn write_tree<'a>(
    checkout: &PinnedDirectory,
    paths: &[&'a str],
    next: &BTreeMap<&'a str, &CheckoutProjectionContent>,
    sources: &mut CheckoutSources<'_>,
    settling: Option<&SettlingClose>,
) -> io::Result<Written<'a>> {
    let mut children: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    children
        .entry(String::new())
        .or_default()
        .insert(".layrs".into());
    let mut folders = BTreeSet::new();
    for path in paths {
        let mut parent = String::new();
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            children
                .entry(parent.clone())
                .or_default()
                .insert(component.to_owned());
            if components.peek().is_some() {
                parent = if parent.is_empty() {
                    component.to_owned()
                } else {
                    format!("{parent}/{component}")
                };
                folders.insert(parent.clone());
            }
        }
    }
    let mut directories: BTreeMap<String, PinnedDirectory> = BTreeMap::new();
    {
        let _span = crate::perf::span("fresh.directories");
        // Sorted: every parent precedes its children.
        for folder in &folders {
            let (parent, name) = folder.rsplit_once('/').unwrap_or(("", folder));
            let created = match parent {
                "" => checkout.create_new_child_unsynced(name.as_ref())?,
                parent => directories[parent].create_new_child_unsynced(name.as_ref())?,
            };
            directories.insert(folder.clone(), created);
        }
    }
    let parent_of = |path: &str| -> (&PinnedDirectory, String) {
        match path.rsplit_once('/') {
            Some((parent, name)) => (&directories[parent], name.to_owned()),
            None => (checkout, path.to_owned()),
        }
    };
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
    let create = |job: &sources::StageJob<'_>| {
        let (parent, name) = parent_of(job.path);
        parent.create_file_held_lean(name.as_ref())
    };
    // Once its bytes are proven, each file's metadata is taken through the
    // writing handle, which is then closed on its writer thread: the scan an
    // antivirus runs at close overlaps the other writes. The final check
    // compares the listing to these stamps, as the change index does later.
    let finish = |job: &sources::StageJob<'_>, file: fs::File| {
        let executable = matches!(
            next.get(job.path).copied(),
            Some(CheckoutProjectionContent::File {
                executable: true,
                ..
            })
        );
        set_executable(&file, executable)?;
        // The verified writer proved exactly `job.length` bytes through this
        // handle, which shares no write access: one basic query completes the
        // stamp (the listing check later compares the actual length too).
        #[cfg(windows)]
        let stat = {
            let (created, written, attributes) = crate::secure_fs::basic_times(&file)?;
            super::super::change_index::stat_from_basic(job.length, created, written, attributes)
        };
        #[cfg(not(windows))]
        let stat = {
            let metadata = file.metadata()?;
            if metadata.len() != job.length {
                return Err(error("fresh checkout file length changed"));
            }
            super::super::change_index::StatKey::of(&metadata)
        };
        let stat = stat.ok_or_else(|| error("fresh checkout file has no usable metadata"))?;
        // Closed now, or held until its timestamp margin has passed.
        match settling {
            Some(settling) => settling.close_later(file),
            None => drop(file),
        }
        Ok(stat)
    };
    let mut created = sources::write_files(&create, &finish, &jobs, sources)?;
    let mut files = BTreeMap::new();
    for (index, path) in paths.iter().enumerate() {
        match next.get(path).copied() {
            Some(CheckoutProjectionContent::File { .. }) => {
                let stat = created
                    .remove(&index)
                    .ok_or_else(|| error("fresh checkout file is missing"))?;
                files.insert(*path, stat);
            }
            Some(CheckoutProjectionContent::Symlink { target }) => {
                let (parent, name) = parent_of(path);
                // Created exclusively and its target read back.
                parent.create_symlink(name.as_ref(), target)?;
            }
            None => {}
        }
    }
    Ok(Written {
        directories,
        files,
        children,
    })
}

/// Final check before the commit, by enumeration: every directory holds
/// exactly what was created in it, and the listing shows every file with the
/// metadata its writing handle had right after its bytes were proven (the
/// trust the change index gives a stat later; `drop` still reads every
/// byte). Returns the change index entries those checks vouch for.
fn verify(
    checkout: &PinnedDirectory,
    root: &Path,
    written: &Written<'_>,
    next: &BTreeMap<&str, &CheckoutProjectionContent>,
    proven_at: i64,
    tick: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Vec<(String, super::super::change_index::IndexEntry)>> {
    let changed =
        || error("the new Worktree folder changed while it was written; it is kept for recovery");
    let mut proven = Vec::with_capacity(written.files.len());
    for (folder, expected) in &written.children {
        tick()?;
        let (handle, disk) = if folder.is_empty() {
            (checkout, root.to_path_buf())
        } else {
            (&written.directories[folder], root.join(folder))
        };
        handle.verify_path_identity()?;
        let mut listed = BTreeMap::new();
        for entry in fs::read_dir(&disk)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| error("checkout filename is not UTF-8"))?;
            // From the enumeration itself on Windows: no open per entry.
            listed.insert(name, entry.metadata()?);
            if listed.len() > expected.len() {
                return Err(changed());
            }
        }
        handle.verify_path_identity()?;
        if !listed.keys().eq(expected.iter()) {
            return Err(changed());
        }
        for (name, metadata) in &listed {
            let path = if folder.is_empty() {
                name.clone()
            } else {
                format!("{folder}/{name}")
            };
            let Some(stat) = written.files.get(path.as_str()) else {
                continue;
            };
            let Some(CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            }) = next.get(path.as_str()).copied()
            else {
                return Err(error("fresh checkout file is not a projected file"));
            };
            if super::super::change_index::StatKey::of(metadata).as_ref() != Some(stat)
                || stat.length != *byte_length
            {
                return Err(changed());
            }
            proven.push((
                path,
                super::super::change_index::IndexEntry {
                    file_id: file_id.clone(),
                    length: *byte_length,
                    executable: *executable,
                    stat: *stat,
                    verified_at: proven_at,
                    native: None,
                },
            ));
        }
    }
    if proven.len() != written.files.len() {
        return Err(changed());
    }
    Ok(proven)
}

/// Syncs every directory, a few at a time.
fn sync_all<'a>(directories: impl Iterator<Item = &'a PinnedDirectory>) -> io::Result<()> {
    let directories: Vec<&PinnedDirectory> = directories.collect();
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8)
        .min(directories.len().max(1));
    let per = directories.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = directories
            .chunks(per)
            .map(|chunk| scope.spawn(move || chunk.iter().try_for_each(|d| d.sync())))
            .collect();
        workers.into_iter().try_for_each(|worker| {
            worker
                .join()
                .map_err(|_| error("directory sync worker panicked"))?
        })
    })
}

/// Prepares (when still pending) and claims the journal, returning its leases.
fn prepare_and_claim(
    fresh: &FreshCheckout<'_>,
    top: &[&str],
    evidence: serde_json::Value,
) -> io::Result<Vec<ResourceLease>> {
    let mut store = open_exclusive(fresh.store_root)
        .map_err(error)?
        .with_filesystem_identity(fresh.store_identity)?
        .with_metadata_identity(fresh.metadata_identity)?;
    let mut journal = NativeRecovery::new(&mut store).map_err(error)?;
    let rows = top
        .iter()
        .map(|path| PathEvidence::new(*path, RecoveryPathRole::Materialized, evidence.clone()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(error)?;
    journal
        .prepare(
            fresh.recovery_id,
            &RecoveryPreparation::new(evidence, rows).map_err(error)?,
            fresh.faults.lease_time_ms()?,
        )
        .map_err(error)?;
    Ok(journal
        .claim(
            fresh.recovery_id,
            fresh.owner,
            fresh.faults.lease_time_ms()?,
            LEASE_MS,
        )
        .map_err(error)?
        .leases)
}

/// Completes the journal: applied when committed, rolled back otherwise.
/// Without leases the journal is still pending and is prepared first.
fn close(
    fresh: &FreshCheckout<'_>,
    top: &[&str],
    leases: Option<&[ResourceLease]>,
    committed: bool,
    evidence: serde_json::Value,
) -> io::Result<()> {
    let leases = match leases {
        Some(leases) => leases.to_vec(),
        None => prepare_and_claim(fresh, top, evidence.clone())?,
    };
    let mut store = open_exclusive(fresh.store_root)
        .map_err(error)?
        .with_filesystem_identity(fresh.store_identity)?
        .with_metadata_identity(fresh.metadata_identity)?;
    let results = top
        .iter()
        .map(|path| {
            RecoveryPathResult::new(
                *path,
                RecoveryPathRole::Materialized,
                if committed {
                    RecoveryPathState::Applied
                } else {
                    RecoveryPathState::RolledBack
                },
                serde_json::json!({"verified": true, "fresh": true}),
            )
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(error)?;
    NativeRecovery::new(&mut store)
        .map_err(error)?
        .complete(
            fresh.recovery_id,
            fresh.owner,
            &leases,
            evidence,
            &results,
            fresh.faults.lease_time_ms()?,
        )
        .map_err(error)?;
    Ok(())
}

/// A short, stable tag of the attempt for its quarantine folder name.
fn digest_tag(fresh: &FreshCheckout<'_>) -> String {
    tag_of(
        &fresh.batch.request_digest().to_string(),
        fresh.recovery_id.as_str(),
    )
}

fn tag_of(batch_digest: &str, recovery_id: &str) -> String {
    let digest: String = batch_digest
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(12)
        .collect();
    let attempt = recovery_id.rsplit(':').next().unwrap_or("0");
    format!("{digest}-{attempt}")
}

/// Moves every entry of the checkout but `.layrs` into the sibling folder
/// `.<name>.layrs-abandoned-<tag>` on the same volume, never deleting.
/// Idempotent: a rerun moves what an interrupted run left. Returns the
/// quarantine folder name, or null when nothing had to move.
fn quarantine(
    checkout_root: &str,
    identity: &str,
    tag: &str,
    tick: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<serde_json::Value> {
    let root = Path::new(checkout_root);
    let checkout = PinnedDirectory::open(root)?;
    if checkout.identity_token() != identity {
        return Err(error("checkout identity changed"));
    }
    let names = foreign_entries(&checkout, root, usize::MAX - 1)?;
    if names.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    let (Some(parent_path), Some(name)) = (root.parent(), root.file_name()) else {
        return Err(error(
            "the Worktree folder has no parent for its quarantine",
        ));
    };
    let name = name
        .to_str()
        .ok_or_else(|| error("checkout folder name is not UTF-8"))?;
    let folder = format!(".{name}.layrs-abandoned-{tag}");
    let parent = PinnedDirectory::open(parent_path)?;
    let destination = parent.ensure_child(folder.as_ref())?;
    if !destination.same_volume(&checkout) {
        return Err(error("quarantine folder is on another volume"));
    }
    for entry in &names {
        tick()?;
        checkout.verify_path_identity()?;
        let full = root.join(entry);
        let kind = fs::symlink_metadata(&full)?.file_type();
        let collided = if kind.is_dir() {
            let identity = checkout
                .open_child_for_traversal(entry.as_ref())?
                .identity_token();
            checkout.move_child_directory(
                entry.as_ref(),
                &identity,
                &destination,
                entry.as_ref(),
            )?
        } else {
            checkout
                .open_leaf(entry.as_ref())?
                .rename_to(&destination, entry.as_ref())?
        };
        if collided {
            return Err(error(format!(
                "quarantine folder {folder} already holds {entry}; both are kept"
            )));
        }
    }
    checkout.sync()?;
    destination.sync()?;
    if !is_fresh(&checkout, root)? {
        return Err(error(
            "the Worktree folder gained entries during its quarantine",
        ));
    }
    Ok(serde_json::Value::String(folder))
}

/// Recovers one pending fresh checkout journal of `worktree`, under the
/// caller's checkout lock: kept when its metadata committed, quarantined
/// otherwise.
#[allow(clippy::too_many_arguments)]
pub(super) fn recover(
    store_root: &Path,
    store_identity: &str,
    metadata_identity: &str,
    store_directory: &PinnedDirectory,
    metadata_directory: &PinnedDirectory,
    worktree: &Worktree,
    record: &RecoveryRecord,
    faults: &dyn MaterializationFaults,
) -> io::Result<()> {
    if record.payload["checkoutRoot"].as_str() != Some(worktree.checkout_root())
        || record.payload["filesystemIdentity"].as_str() != Some(worktree.filesystem_identity())
    {
        return Err(error("recovery checkout identity differs from Worktree"));
    }
    let active = worktree.active_operation_id().as_str();
    let committed = record.payload["newOperation"].as_str() == Some(active);
    if !committed && record.payload["oldOperation"].as_str() != Some(active) {
        return Err(error("Worktree moved beyond pending recovery"));
    }
    let owner = record
        .owner_token
        .clone()
        .unwrap_or_else(|| format!("recovery-owner:{}", record.id));
    let top: Vec<&str> = record.paths.iter().map(|p| p.path.as_str()).collect();
    let leases = {
        let mut store = open_exclusive(store_root)
            .map_err(error)?
            .with_filesystem_identity(store_identity)?
            .with_metadata_identity(metadata_identity)?;
        let mut journal = NativeRecovery::new(&mut store).map_err(error)?;
        if record.state == RecoveryState::Pending {
            let evidence = serde_json::json!({"fresh": true, "recovering": true});
            let rows = top
                .iter()
                .map(|path| {
                    PathEvidence::new(*path, RecoveryPathRole::Materialized, evidence.clone())
                })
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            journal
                .prepare(
                    &record.id,
                    &RecoveryPreparation::new(evidence, rows).map_err(error)?,
                    faults.lease_time_ms()?,
                )
                .map_err(error)?;
        }
        journal
            .claim(&record.id, &owner, faults.lease_time_ms()?, LEASE_MS)
            .map_err(error)?
            .leases
    };
    let mut lease = LeaseKeeper::new(
        store_root,
        store_directory,
        metadata_directory,
        &record.id,
        &owner,
        leases,
        faults,
    )?;
    let moved = if committed {
        serde_json::Value::Null
    } else {
        let digest = record.payload["batchDigest"]
            .as_str()
            .ok_or_else(|| error("missing recovery digest"))?;
        let moved = quarantine(
            worktree.checkout_root(),
            worktree.filesystem_identity(),
            &tag_of(digest, record.id.as_str()),
            &mut || lease.tick(),
        )?;
        faults.check(MaterializationPoint::AfterBackup)?;
        moved
    };
    lease.force()?;
    let results = top
        .iter()
        .map(|path| {
            RecoveryPathResult::new(
                *path,
                RecoveryPathRole::Materialized,
                if committed {
                    RecoveryPathState::Applied
                } else {
                    RecoveryPathState::RolledBack
                },
                serde_json::json!({"verified": true, "fresh": true}),
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
            &record.id,
            &owner,
            &lease.leases,
            serde_json::json!({"committed": committed, "recovered": true, "quarantine": moved}),
            &results,
            faults.lease_time_ms()?,
        )
        .map_err(error)?;
    Ok(())
}

/// Closes written files once a timestamp margin has passed since their last
/// write (council s11 r1). Each handle shares no write access, so until it
/// is closed no other process can change the file; a write after the close
/// stamps a later modification time. A few threads close them as they come
/// due (closing is where an antivirus scans a new file), overlapping the
/// other writes.
struct SettlingClose {
    sender: std::sync::Mutex<Option<std::sync::mpsc::Sender<(std::time::Instant, fs::File)>>>,
    /// Set to close every remaining handle at once (a failed write).
    now: std::sync::Arc<std::sync::atomic::AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
    margin: std::time::Duration,
}

impl SettlingClose {
    fn new(margin: std::time::Duration) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<(std::time::Instant, fs::File)>();
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
        let now = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(2, 8);
        let workers = (0..threads)
            .filter_map(|_| {
                let receiver = std::sync::Arc::clone(&receiver);
                let now = std::sync::Arc::clone(&now);
                std::thread::Builder::new()
                    .spawn(move || {
                        loop {
                            let next = match receiver.lock() {
                                Ok(receiver) => receiver.recv(),
                                Err(_) => return,
                            };
                            let Ok((due, file)) = next else {
                                return;
                            };
                            let wait = due.saturating_duration_since(std::time::Instant::now());
                            if !wait.is_zero() && !now.load(std::sync::atomic::Ordering::Acquire) {
                                std::thread::sleep(wait);
                            }
                            drop(file);
                        }
                    })
                    .ok()
            })
            .collect();
        Self {
            sender: std::sync::Mutex::new(Some(sender)),
            now,
            workers,
            margin,
        }
    }

    /// Holds `file` until the margin has passed from now (its last write
    /// was just before); closes it at once if no closer is running.
    fn close_later(&self, file: fs::File) {
        let due = std::time::Instant::now() + self.margin;
        let sent = self
            .sender
            .lock()
            .ok()
            .and_then(|sender| sender.as_ref().map(|sender| sender.send((due, file))));
        if let Some(Err(std::sync::mpsc::SendError((_, file)))) = sent {
            drop(file);
        }
    }

    /// Every handle closed: as each comes due (`wait`), or at once.
    fn end(mut self, wait: bool) {
        self.finish(wait);
    }

    fn finish(&mut self, wait: bool) {
        if !wait {
            self.now.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for SettlingClose {
    fn drop(&mut self) {
        // An early return: nothing waits on these proofs any more.
        self.finish(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_entries_are_sorted_and_unique() {
        assert_eq!(
            top_level(&["b/x", "a", "b/y/z", "c/d", "a2"]),
            vec!["a", "a2", "b", "c"]
        );
    }

    #[test]
    fn quarantine_tag_is_short_and_names_the_attempt() {
        assert_eq!(
            tag_of("blake3:0123456789abcdef0123", "recovery:x:attempt:3"),
            "0123456789ab-3"
        );
    }
}
