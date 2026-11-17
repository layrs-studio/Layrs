//! A fast-forward under git's contract (user decision, 2026-11-17): the
//! folder is written directly, as `git merge --ff-only` writes it, without a
//! recovery journal or directory syncs.
//!
//! Every path the transition touches was checked to hold its exact base
//! content before this runs (a local change refuses before anything is
//! written). New files are staged (and their bytes proven) first. The
//! folder is then written inside the batch's own transaction, once the
//! batch is validated there with the exclusive Store held (council s21 r1:
//! a stale or invalid batch refuses before any file moves), right before
//! its commit. Each original, one at a time, is checked again, moved aside
//! into the attempt folder and checked once more there: one changed meanwhile
//! goes back to its path (its edits were never stored), and the transition
//! refuses. A crash or a failure once files moved leaves the folder partly
//! at the target while the Worktree's base has not moved: `status` shows
//! those paths as changes (git leaves the same state); originals moved
//! aside held base content (in the Store), and the attempt folder is swept
//! like any unjournaled one.
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
        // One original at a time (bounded handles): checked, moved aside,
        // checked again where it now is.
        for (index, path) in paths.iter().enumerate() {
            let Some(content) = prior.get(path).copied() else {
                continue;
            };
            checkout.verify_path_identity()?;
            let expected = fingerprint(Some(content));
            let (parent, name) = checkout.existing_parent(Path::new(path))?;
            let original = parent.open_leaf(&name)?;
            if !matches_fingerprint(Some(&original), &expected, &mut || Ok(()))? {
                return Err(error(
                    "local changes to paths this operation writes would be overwritten: capture or discard them first",
                ));
            }
            let aside = format!("old-{index}");
            if original.rename_to(attempt, aside.as_ref())? {
                return Err(error("an attempt folder entry already exists"));
            }
            drop(original);
            let moved = attempt.open_leaf(aside.as_ref())?;
            if !matches_fingerprint(Some(&moved), &expected, &mut || Ok(()))? {
                // Edited between the check and the move: its bytes were
                // never stored, so they go back where they were.
                let kept = if moved.rename_to(&parent, &name)? {
                    let kept = format!("{}.layrs-kept-{}", name.to_string_lossy(), now_ms()?);
                    moved.rename_to(&parent, kept.as_ref())?;
                    format!(" (kept as {kept}: its path was taken)")
                } else {
                    String::new()
                };
                return Err(error(format!(
                    "{path} changed while this operation replaced it: the change is kept{kept}; the folder is partly updated"
                )));
            }
            if let Some(seal) = BackupSeal::of(&moved) {
                sealed.push((aside, seal));
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
                files().map_err(|failure| layrs_store::scm::StoreError::Backend {
                    message: failure.to_string(),
                })
            },
        )
        .map_err(io::Error::other)?;
    drop(metadata);
    {
        let _close = crate::perf::span("materialize.close_staged");
        drop(close_in_background(staged.into_values().collect()));
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
            if let Some(step) = context.target_step {
                index.set_base_step(step);
            }
            if index.save(context.store_root).is_err() {
                crate::perf::count("materialize.change_index_unsaved", 1);
            }
        }
    }
    Ok(outcome)
}
