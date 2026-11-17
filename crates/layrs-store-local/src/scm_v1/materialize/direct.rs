//! A fast-forward under git's contract (user decision, 2026-11-17): the
//! folder is written directly, as `git merge --ff-only` writes it, without a
//! recovery journal, backups of the replaced files or directory syncs.
//!
//! Every path the transition touches was checked to hold its exact base
//! content before this runs (a local change refuses before anything is
//! written), and each original is checked again right before it is
//! removed: its bytes are in the Store, so removing it loses nothing. New
//! files are staged (and their bytes proven) before any original is
//! removed. A crash or a failure midway leaves the folder partly at the
//! target while the Worktree's base has not moved: `status` shows those
//! paths as changes, and the next operation proceeds from there (git leaves
//! the same state). The metadata commit is the ordinary durable batch, and
//! an attempt folder left by a crash is swept like any unjournaled one.
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
    let backups = context
        .store_directory
        .ensure_child_unsynced("metadata".as_ref())?
        .ensure_child_unsynced("repair-backups".as_ref())?;
    sweep_unjournaled_attempts(&backups, context.worktree_id, context.journaled_folders);
    let staging = empty_attempt_folder(&backups, context.attempt_name.as_ref())?;
    let stage = crate::perf::span("materialize.stage");
    let staged = stage_all(&staging, paths, prior, next, sources);
    if let Some(end_staging) = sources.end_staging.as_mut() {
        end_staging();
    }
    drop(stage);
    let result = match staged {
        Ok((staged, _, written)) => write(context, paths, prior, next, sources, staged, written),
        Err(failure) => Err(failure),
    };
    // Every staged file was moved into the folder, or is removed with its
    // attempt folder (best effort: a leftover folder is swept later).
    drop(staging);
    discard_attempt(&backups, context.attempt_name.as_ref());
    result
}

fn write(
    context: &DirectCheckout<'_>,
    paths: &[&str],
    prior: &BTreeMap<&str, &CheckoutProjectionContent>,
    next: &BTreeMap<&str, &CheckoutProjectionContent>,
    sources: &mut CheckoutSources<'_>,
    staged: BTreeMap<&str, PinnedLeaf>,
    written: BTreeMap<&str, StagedStamp>,
) -> io::Result<ApplyBatchOutcome> {
    let checkout = PinnedDirectory::open(Path::new(context.checkout_root))?;
    if checkout.identity_token() != context.identity {
        return Err(error("checkout identity changed"));
    }
    // Every original still holds its base content, checked again right
    // before anything is removed: a change since the precheck refuses with
    // the folder untouched.
    let phases = crate::perf::span("materialize.direct_write");
    let mut originals = Vec::new();
    for path in paths {
        checkout.verify_path_identity()?;
        if let Some(content) = prior.get(path).copied() {
            let (parent, name) = checkout.existing_parent(Path::new(path))?;
            let original = parent.open_leaf(&name)?;
            if !matches_fingerprint(Some(&original), &fingerprint(Some(content)), &mut || Ok(()))? {
                return Err(error(
                    "local changes to paths this operation writes would be overwritten: capture or discard them first",
                ));
            }
            originals.push((parent, name, original));
        }
    }
    for (parent, name, original) in originals {
        match original.regular_file() {
            Some(file) => file.unlink()?,
            None => parent.unlink_file(&name)?,
        }
    }
    let changed = |path: &&str| paths.binary_search(path).is_ok();
    clear_obstructing_directories(
        &checkout,
        prior.keys().copied().filter(changed),
        next.keys().copied().filter(changed),
        &mut || Ok(()),
    )?;
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
    drop(phases);
    let postcheck = crate::perf::span("materialize.postcheck");
    let indexed = super::super::change_index::ChangeIndex::is_enabled(
        context.store_root,
        context.worktree_id,
    );
    let proven_at = super::super::change_index::now();
    let mut proven: Vec<(String, super::super::change_index::IndexEntry)> = Vec::new();
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
    drop(postcheck);
    let closer = close_in_background(staged.into_values().collect());
    let metadata = crate::perf::span("materialize.direct_metadata");
    let outcome = NativeScmRepository::new(context.store_root)
        .with_short_acquisition()
        .with_store_identity(context.store_identity)
        .with_metadata_identity(context.metadata_identity)
        .apply_batch_with_chunks(context.batch, sources.chunks, &mut sources.read_chunk)
        .map_err(io::Error::other)?;
    drop(metadata);
    {
        let _close = crate::perf::span("materialize.close_staged");
        drop(closer);
    }
    if indexed {
        // A cache, as on the journaled path.
        let _span = crate::perf::span("materialize.change_index");
        if let Some(mut index) =
            super::super::change_index::ChangeIndex::load(context.store_root, context.worktree_id)
        {
            index.resolve(context.before);
            for path in paths {
                index.remove_path(path);
            }
            for (path, entry) in proven {
                index.insert(path, entry);
            }
            index.bind(context.projection, context.after);
            if index.save(context.store_root).is_err() {
                crate::perf::count("materialize.change_index_unsaved", 1);
            }
        }
    }
    Ok(outcome)
}
