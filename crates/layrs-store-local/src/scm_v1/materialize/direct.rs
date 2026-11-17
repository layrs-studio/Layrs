//! A fast-forward under git's contract (user decision, 2026-11-17): the
//! folder is written directly, as `git merge --ff-only` writes it, without a
//! recovery journal or directory syncs.
//!
//! Every path the transition touches was checked to hold its exact base
//! content before this runs (the precheck; a local change refuses before
//! anything is written). New files are staged (and their bytes proven)
//! first. The folder is then written inside the batch's own transaction,
//! once the batch is validated there with the exclusive Store held (council
//! s21 r1: a stale or invalid batch refuses before any file moves), right
//! before its commit. Each original, one at a time, is moved aside into the
//! attempt folder, sealed and proven there again (council s21 r2): one that
//! changed since the precheck goes back to its path, and the transition
//! refuses. A refusal before anything is installed puts every original
//! back, never over another entry, and names any original it could not
//! return. Once files are installed, a crash or a failure leaves the folder
//! partly at the target while the Worktree's base has not moved: `status`
//! shows those paths as changes (git leaves the same state); the originals
//! moved aside held base content (in the Store), and the attempt folder is
//! swept like any unjournaled one. Folders the transition emptied are
//! removed, as git removes them.
use super::*;

pub(super) struct DirectCheckout<'a> {
    pub(super) store_root: &'a Path,
    pub(super) store_identity: &'a str,
    pub(super) metadata_identity: &'a str,
    pub(super) store_directory: &'a PinnedDirectory,
    pub(super) worktree_id: &'a WorktreeId,
    pub(super) checkout_root: &'a str,
    pub(super) identity: &'a str,
    pub(super) attempt_name: &'a str,
    pub(super) journaled_folders: &'a std::collections::BTreeSet<std::ffi::OsString>,
    pub(super) recovery_pending: bool,
    pub(super) target_step: Option<&'a layrs_core::scm::StepId>,
    pub(super) projection: &'a layrs_core::scm::TreeId,
    pub(super) before: &'a [CheckoutProjectionEntry],
    pub(super) after: &'a [CheckoutProjectionEntry],
    pub(super) batch: &'a WriteBatch,
}

pub(super) fn apply(
    context: &DirectCheckout<'_>,
    paths: &[&str],
    prior: &BTreeMap<&str, &CheckoutProjectionContent>,
    next: &BTreeMap<&str, &CheckoutProjectionContent>,
    sources: &mut CheckoutSources<'_>,
) -> io::Result<ApplyBatchOutcome> {
    let _span = crate::perf::span("materialize.direct");
    crate::perf::count("materialize.direct_paths", paths.len() as u64);
    // Another materialization of this folder must be recovered first.
    if context.recovery_pending {
        return Err(error("worktree_needs_repair: pending checkout journal"));
    }
    let backups = context
        .store_directory
        .ensure_child_unsynced("metadata".as_ref())?
        .ensure_child_unsynced("repair-backups".as_ref())?;
    sweep_unjournaled_attempts(&backups, context.worktree_id, context.journaled_folders);
    let attempt = empty_attempt_folder(&backups, context.attempt_name.as_ref(), false)?;
    let stage = crate::perf::span("materialize.stage");
    let staged = stage_all(&attempt, paths, prior, next, sources);
    if let Some(end_staging) = sources.end_staging.as_mut() {
        end_staging();
    }
    drop(stage);
    let mut sealed = Vec::new();
    let result = match staged {
        Ok((staged, _, written)) => write(
            context,
            &attempt,
            paths,
            prior,
            next,
            sources,
            staged,
            written,
            &mut sealed,
        ),
        Err(failure) => Err(failure),
    };
    // Whatever happened: every original moved aside was proven to hold base
    // content (in the Store) and is removed while still that very file, and
    // the staged files left (the transition failed) go with the folder.
    // Best effort: a leftover is swept later; a retry of the same batch
    // needs the folder empty.
    let root = super::super::NativeStoreLayout::new(context.store_root)
        .map_err(error)?
        .repair_backups_dir()
        .join(context.attempt_name);
    purge_completed_backup(&backups, attempt, &root, &sealed, false);
    discard_attempt(&backups, context.attempt_name.as_ref());
    result
}

#[allow(clippy::too_many_arguments)]
fn write(
    context: &DirectCheckout<'_>,
    attempt: &PinnedDirectory,
    paths: &[&str],
    prior: &BTreeMap<&str, &CheckoutProjectionContent>,
    next: &BTreeMap<&str, &CheckoutProjectionContent>,
    sources: &mut CheckoutSources<'_>,
    staged: BTreeMap<&str, PinnedLeaf>,
    written: BTreeMap<&str, StagedStamp>,
    sealed: &mut Vec<(String, BackupSeal)>,
) -> io::Result<ApplyBatchOutcome> {
    let checkout = PinnedDirectory::open(Path::new(context.checkout_root))?;
    if checkout.identity_token() != context.identity {
        return Err(error("checkout identity changed"));
    }
    let indexed = super::super::change_index::ChangeIndex::is_enabled(
        context.store_root,
        context.worktree_id,
    );
    let mut proven: Vec<(String, super::super::change_index::IndexEntry)> = Vec::new();
    let mut files = || -> io::Result<()> {
        let _write = crate::perf::span("materialize.direct_write");
        // Originals moved aside so far, to put back if the transition
        // refuses before anything is installed.
        let mut moved: Vec<(String, &str)> = Vec::new();
        let mut aside_all = || -> io::Result<()> {
            // One original at a time (bounded handles): moved aside, then
            // proven where it now is. One changed meanwhile goes back.
            for (index, path) in paths.iter().enumerate() {
                let Some(content) = prior.get(path).copied() else {
                    continue;
                };
                checkout.verify_path_identity()?;
                let expected = fingerprint(Some(content));
                let (parent, name) = checkout.existing_parent(Path::new(path))?;
                let original = parent.open_leaf(&name)?;
                let aside = format!("old-{index}");
                if original.rename_to(attempt, aside.as_ref())? {
                    return Err(error("an attempt folder entry already exists"));
                }
                drop(original);
                moved.push((aside.clone(), path));
                let file = attempt.open_leaf(aside.as_ref())?;
                // Sealed before it is hashed and unchanged after (council
                // s21 r2): a write during the hash changes the stamp, so a
                // seal never vouches for bytes that were not proven.
                let seal = BackupSeal::of(&file);
                let unchanged = seal.is_some()
                    && matches_fingerprint(Some(&file), &expected, &mut || Ok(()))?
                    && BackupSeal::of(&file) == seal;
                drop(file);
                match seal {
                    Some(seal) if unchanged => sealed.push((aside, seal)),
                    _ => {
                        return Err(error(format!(
                            "{path} changed while this operation replaced it: capture or discard it first"
                        )));
                    }
                }
            }
            let changed = |path: &&str| paths.binary_search(path).is_ok();
            clear_obstructing_directories(
                &checkout,
                prior.keys().copied().filter(changed),
                next.keys().copied().filter(changed),
                &mut || Ok(()),
            )
        };
        if let Err(failure) = aside_all() {
            // Nothing installed yet: every original goes back, never over
            // another entry (one that cannot is kept under another name), so
            // the refusal leaves the folder as it was. One that cannot go
            // back at all stays in the attempt folder, never removed, and
            // the error names it (council s21 r3).
            let mut renamed = Vec::new();
            let mut kept = Vec::new();
            for (aside, path) in moved.iter().rev() {
                sealed.retain(|(name, _)| name != aside);
                match put_back(&checkout, attempt, aside, path) {
                    Ok(None) => {}
                    Ok(Some(name)) => renamed.push(format!("{path} as {name}")),
                    Err(_) => kept.push(format!("{path} as {aside}")),
                }
            }
            // Every original not back at its own path is named (council
            // s21 r3).
            let mut message = failure.to_string();
            if !renamed.is_empty() {
                message.push_str(&format!(
                    "; originals whose path was taken are kept beside it: {}",
                    renamed.join(", ")
                ));
            }
            if !kept.is_empty() {
                message.push_str(&format!(
                    "; originals that could not be put back are kept in {}: {}",
                    attempt.display_path().display(),
                    kept.join(", ")
                ));
            }
            return Err(io::Error::new(failure.kind(), message));
        }
        let mut parents: BTreeMap<&str, PinnedDirectory> = BTreeMap::new();
        for path in paths {
            let Some(file) = staged.get(path) else {
                continue;
            };
            let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
            if !parents.contains_key(directory) {
                // Bounded open handles.
                if parents.len() >= 64 {
                    parents.clear();
                }
                let (parent, _) = checkout.components_to_parent_with(Path::new(path), false)?;
                parents.insert(directory, parent);
            }
            let parent = &parents[directory];
            parent.verify_path_identity()?;
            if file.rename_to(parent, std::ffi::OsStr::new(name))? {
                return Err(error(
                    "checkout path appeared during materialization: the folder is partly updated",
                ));
            }
        }
        drop(parents);
        remove_emptied_folders(&checkout, paths, prior, next);
        let proven_at = super::super::change_index::now();
        // As the journaled path: an installed file is vouched for through the
        // very handle that wrote its proven bytes.
        let mut trust = |path: &str, full: &Path| -> Option<super::super::checkout::TrustedFile> {
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
                if let Some(stat) = super::super::change_index::StatKey::of(&metadata) {
                    proven.push((
                        path.to_owned(),
                        super::super::change_index::IndexEntry {
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
            trusted.then(|| super::super::checkout::TrustedFile {
                file_id: file_id.clone(),
                length: *byte_length,
                executable: *executable,
            })
        };
        if !touched_paths_installed(
            &checkout,
            Path::new(context.checkout_root),
            paths,
            next,
            &mut trust,
            &mut || Ok(()),
        )? {
            return Err(error(
                "checkout changed during materialization: the folder is partly updated",
            ));
        }
        Ok(())
    };
    // The folder's own failure, reported as itself (not as the Store's).
    let mut failure: Option<io::Error> = None;
    // The change index (a cache) is read while the batch commits; it is
    // resolved against `before` below, as after the commit.
    let (outcome, loaded) = std::thread::scope(|scope| {
        let loading = indexed.then(|| {
            scope.spawn(|| {
                let _span = crate::perf::span("materialize.change_index_load");
                super::super::change_index::ChangeIndex::load(
                    context.store_root,
                    context.worktree_id,
                )
            })
        });
        let metadata = crate::perf::span("materialize.direct_metadata");
        let outcome = NativeScmRepository::new(context.store_root)
            .with_short_acquisition()
            .with_store_identity(context.store_identity)
            .with_metadata_identity(context.metadata_identity)
            .apply_batch_with_chunks_before_commit(
                context.batch,
                sources.chunks,
                &mut sources.read_chunk,
                || {
                    files().map_err(|error| {
                        let message = error.to_string();
                        failure = Some(error);
                        layrs_store::scm::StoreError::Backend { message }
                    })
                },
            );
        drop(metadata);
        let loaded = loading.and_then(|loading| loading.join().ok().flatten());
        (outcome, loaded)
    });
    if let Some(failure) = failure {
        return Err(failure);
    }
    let outcome = outcome.map_err(io::Error::other)?;
    {
        let _close = crate::perf::span("materialize.close_staged");
        drop(close_in_background(staged.into_values().collect()));
    }
    if let Some(mut index) = loaded {
        // A cache, as on the journaled path.
        let _span = crate::perf::span("materialize.change_index");
        index.resolve(context.before);
        for path in paths {
            index.remove_path(path);
        }
        for (path, entry) in proven {
            index.insert(path, entry);
        }
        index.bind(context.projection, context.after);
        if let Some(step) = context.target_step {
            index.set_base_step(step);
        }
        if index.save(context.store_root).is_err() {
            crate::perf::count("materialize.change_index_unsaved", 1);
        }
    }
    Ok(outcome)
}

/// Puts the original moved aside as `aside` back at `path`, never over
/// another entry: when the path was taken meanwhile, it is kept next to it
/// as `<name>.layrs-kept-<ms>`, whose name is returned.
fn put_back(
    checkout: &PinnedDirectory,
    attempt: &PinnedDirectory,
    aside: &str,
    path: &str,
) -> io::Result<Option<String>> {
    // Its folders are made again if the attempt removed them (obstruction
    // cleanup), never over another entry.
    let (parent, name) = checkout.components_to_parent_with(Path::new(path), false)?;
    parent.verify_path_identity()?;
    let original = attempt.open_leaf(aside.as_ref())?;
    if !original.rename_to(&parent, &name)? {
        return Ok(None);
    }
    let kept = format!("{}.layrs-kept-{}", name.to_string_lossy(), now_ms()?);
    if original.rename_to(&parent, kept.as_ref())? {
        return Err(error("a kept original's name is taken"));
    }
    Ok(Some(kept))
}

/// Removes the folders this transition emptied, as git does (council s21
/// r2: an emptied folder would otherwise keep every later `status` off the
/// status view): deepest first, each only while it is empty and no longer
/// holds a tracked path. Best effort, without syncs.
fn remove_emptied_folders(
    checkout: &PinnedDirectory,
    paths: &[&str],
    prior: &BTreeMap<&str, &CheckoutProjectionContent>,
    next: &BTreeMap<&str, &CheckoutProjectionContent>,
) {
    let mut folders = std::collections::BTreeSet::new();
    for path in paths {
        if !prior.contains_key(path) || next.contains_key(path) {
            continue;
        }
        let mut rest = *path;
        while let Some((folder, _)) = rest.rsplit_once('/') {
            if !folders.insert(folder) {
                break;
            }
            rest = folder;
        }
    }
    let mut folders: Vec<&str> = folders.into_iter().collect();
    folders.sort_by(|a, b| {
        b.matches('/')
            .count()
            .cmp(&a.matches('/').count())
            .then_with(|| b.cmp(a))
    });
    for folder in folders {
        let prefix = format!("{folder}/");
        let tracked = next
            .range(prefix.as_str()..)
            .next()
            .is_some_and(|(path, _)| path.starts_with(&prefix));
        if tracked {
            continue;
        }
        if let Ok((parent, name)) = checkout.existing_parent(Path::new(folder)) {
            if parent.remove_empty_child_unsynced(&name).is_ok() {
                crate::perf::count("materialize.emptied_folders", 1);
            }
        }
    }
}
