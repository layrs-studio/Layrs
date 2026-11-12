//! Pack consolidation (ADR 0028, section 6): once a Store holds enough
//! registered packs, the smallest are written into one new pack, published
//! durably, then one transaction registers it, remaps every admission
//! bitmap exactly from the old ordinals to the new ones (the identities each
//! Space admits are checked unchanged in the same transaction, the old ones
//! read from the old indexes and the new ones from the new index) and
//! unregisters the old packs. Their files, no longer registered, are removed
//! when the CAS is opened exclusively again, in the same session. A reader
//! holding an older catalog (the VFS between lock holds) reloads it when a
//! pack it names is gone, as git does.
use super::cas::compact::Consolidation;
use super::cas::{CasFaultInjector, NativeCas, NativeCasWriter};
use super::{NativeStoreExclusive, sqlite};
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::sync::Arc;

/// Candidates left out after a failure before giving up for this session.
const MAX_ATTEMPTS: usize = 3;
/// Packs a consolidation could not copy (damaged records, malformed
/// bitmaps), one name per line, under `metadata/`: left out by every later
/// session until they are no longer registered, so damaged packs never
/// starve the sound ones (council s6 r2). Only ever excludes candidates.
const SKIPPED_FILE: &str = "consolidation-skipped";
/// The last general consolidation failures, under `metadata/`: their count,
/// the time of the last (Unix ms) and its message. After repeated failures
/// consolidation pauses for `FAILURE_PAUSE_MS`, as git pauses its automatic
/// gc after a failure (`gc.log`); a success removes the note.
const FAILED_FILE: &str = "consolidation-failed";
/// General failures in a row before pausing: one may be transient.
const FAILURES_BEFORE_PAUSE: u64 = 2;
const FAILURE_PAUSE_MS: u64 = 60 * 60 * 1000;
/// The most bytes a consolidation note may hold.
const NOTE_MAX_BYTES: u64 = 64 * 1024;
/// Under `metadata/`: held exclusively by the one maintenance pass running
/// (council s17, B2b).
const MAINTENANCE_LOCK: &str = "maintenance.lock";
/// Under `metadata/`: the general failures in a row of maintenance passes
/// (as `FAILED_FILE`); automatic maintenance pauses as consolidation does.
const MAINTENANCE_FAILED_FILE: &str = "maintenance-failed";

/// What a maintenance pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceOutcome {
    /// Another pass holds the maintenance lock, or the Store stayed busy
    /// longer than the pass waits for it (nothing was published).
    Busy,
    /// No group of pack files to consolidate (or not a compact Store).
    NothingToDo,
    /// Nothing was published (the built file is removed): a candidate
    /// was retired while its pack was built, or was found damaged and is
    /// left out from now on. Another pass may consolidate.
    Abandoned,
    /// `packs` registered packs were written into one.
    Consolidated { packs: usize },
}

/// Consolidates the pack files a batch's own consolidation leaves out
/// (those above its budgets), as `git maintenance` repacks (council s17,
/// B2b). One pass at a time. The group is chosen in a short shared session;
/// the new pack is built and verified with no Store lock, under a temporary
/// name orphan collection ignores; then the exclusive session checks every
/// candidate is still registered as it was, publishes the pack and registers
/// it with the exact admission remap of `register` (reading the bitmaps as
/// they are then, so admissions made meanwhile are kept). A crash before
/// that commit leaves only the temporary file, removed by the next pass.
pub fn maintain(root: impl AsRef<std::path::Path>) -> Result<MaintenanceOutcome, String> {
    maintain_recorded(root.as_ref(), &mut || Ok(()))
}

/// Test support: `maintain`, running `between` once the pack is built and
/// before the exclusive session.
#[cfg(any(test, feature = "test-support"))]
pub fn maintain_with_pause(
    root: impl AsRef<std::path::Path>,
    between: &mut dyn FnMut() -> Result<(), String>,
) -> Result<MaintenanceOutcome, String> {
    maintain_recorded(root.as_ref(), between)
}

/// `maintain_pausing`, its general failures recorded in a row
/// (`MAINTENANCE_FAILED_FILE`) and cleared by a pass that completes.
fn maintain_recorded(
    root: &std::path::Path,
    between: &mut dyn FnMut() -> Result<(), String>,
) -> Result<MaintenanceOutcome, String> {
    let result = maintain_pausing(root, between);
    let metadata = root.join(super::layout::METADATA_DIR_NAME);
    match &result {
        Ok(MaintenanceOutcome::Consolidated { .. } | MaintenanceOutcome::NothingToDo) => {
            match std::fs::symlink_metadata(metadata.join(MAINTENANCE_FAILED_FILE)) {
                Ok(found) if found.is_file() => {
                    let _ = std::fs::remove_file(metadata.join(MAINTENANCE_FAILED_FILE));
                }
                _ => {}
            }
        }
        Ok(MaintenanceOutcome::Busy | MaintenanceOutcome::Abandoned) => {}
        Err(message) => {
            let count =
                failures_at(&metadata, MAINTENANCE_FAILED_FILE).map_or(0, |(count, _)| count) + 1;
            write_note_at(
                &metadata,
                MAINTENANCE_FAILED_FILE,
                &format!("{count} {}\n{message}\n", now_ms()),
            );
        }
    }
    result
}

/// Whether automatic maintenance of the Store at `root` pauses after
/// general failures in a row, as a batch's consolidation does (council s19
/// r1): an explicit `layrs maintenance` still runs.
pub fn maintenance_paused(root: impl AsRef<std::path::Path>) -> bool {
    let metadata = root.as_ref().join(super::layout::METADATA_DIR_NAME);
    failures_at(&metadata, MAINTENANCE_FAILED_FILE).is_some_and(|(count, at)| {
        count >= FAILURES_BEFORE_PAUSE && now_ms().saturating_sub(at) < FAILURE_PAUSE_MS
    })
}

/// Whether `error` is a Store acquisition that waited its whole budget.
fn timed_out(error: &super::NativeStoreError) -> bool {
    matches!(error, super::NativeStoreError::Io(error) if error.kind() == std::io::ErrorKind::TimedOut)
}

/// Opens the maintenance lock under `metadata` without following links,
/// creating it when `create`; `None` when it is absent and not created.
fn open_maintenance_lock(
    metadata: &std::path::Path,
    create: bool,
) -> std::io::Result<Option<std::fs::File>> {
    let path = metadata.join(MAINTENANCE_LOCK);
    match crate::layout::open_read_write_no_follow(&path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            match crate::layout::create_new_file_no_follow(&path) {
                Ok(file) => Ok(Some(file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    crate::layout::open_read_write_no_follow(&path).map(Some)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The maintenance lock of the Store at `root`, held, when no pass holds
/// it (never created here).
pub(crate) fn idle_maintenance_lock(root: &std::path::Path) -> Option<std::fs::File> {
    let file =
        open_maintenance_lock(&root.join(super::layout::METADATA_DIR_NAME), false).ok()??;
    fs2::FileExt::try_lock_exclusive(&file).ok()?;
    Some(file)
}

fn maintain_pausing(
    root: &std::path::Path,
    between: &mut dyn FnMut() -> Result<(), String>,
) -> Result<MaintenanceOutcome, String> {
    let _span = crate::perf::span("store.maintenance");
    // The group, from a short shared session.
    let (cas, catalog, candidates, mut damaged, lock) = {
        let store = match super::acquisition::acquire(
            "maintenance read",
            super::acquisition::SHORT_WAIT,
            || super::open(root),
        ) {
            Ok(store) => store,
            Err(error) if timed_out(&error) => return Ok(MaintenanceOutcome::Busy),
            Err(error) => return Err(error.to_string()),
        };
        if !store.compact() {
            return Ok(MaintenanceOutcome::NothingToDo);
        }
        let metadata = store.layout().metadata_dir().to_path_buf();
        let lock = open_maintenance_lock(&metadata, true)
            .map_err(|error| error.to_string())?
            .ok_or("the maintenance lock was not created")?;
        if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
            return Ok(MaintenanceOutcome::Busy);
        }
        let cas = NativeCas::open_lazy(&store).map_err(|error| error.to_string())?;
        cas.remove_stale_maintenance_packs()
            .map_err(|error| error.to_string())?;
        let catalog = cas
            .store_catalog(&store)
            .map_err(|error| error.to_string())?;
        let mut excluded = skipped_positions(&skipped_at(&metadata), &catalog);
        // As a batch's consolidation: a candidate with a malformed
        // admission bitmap is left out (registration would refuse it).
        let mut damaged = Vec::new();
        let candidates = loop {
            let candidates = catalog.maintenance_candidates(&excluded);
            if candidates.len() < 2 {
                break candidates;
            }
            match malformed_bitmap(store.layout().sqlite_path(), &catalog, &candidates)? {
                Some(position) => {
                    damaged.push(position);
                    excluded.push(position);
                }
                None => break candidates,
            }
        };
        (cas, catalog, candidates, damaged, lock)
    };
    if candidates.len() < 2 {
        remember_damaged(root, &catalog, &damaged)?;
        return Ok(MaintenanceOutcome::NothingToDo);
    }
    // The pack, with no Store lock held.
    let wanted: Vec<_> = candidates
        .iter()
        .map(|&position| {
            let pack = &catalog.packs()[position];
            (pack.seq, pack.name().to_os_string(), pack.digest())
        })
        .collect();
    let built = match cas.build_maintenance(&catalog, &candidates) {
        Ok(built) => built,
        Err(failure) => {
            // A batch may have retired a candidate meanwhile and collected
            // its file: that is a moved candidate, not a failure.
            let store = match super::acquisition::acquire(
                "maintenance read",
                super::acquisition::SHORT_WAIT,
                || super::open(root),
            ) {
                Ok(store) => store,
                Err(error) if timed_out(&error) => return Ok(MaintenanceOutcome::Busy),
                Err(error) => return Err(error.to_string()),
            };
            let current = NativeCas::open_lazy(&store)
                .and_then(|cas| cas.store_catalog(&store))
                .map_err(|error| error.to_string())?;
            drop(store);
            if !all_registered(current.packs(), &wanted) {
                crate::perf::count("store.maintenance_abandoned", 1);
                return Ok(MaintenanceOutcome::Abandoned);
            }
            // A candidate whose records cannot all be copied is damaged
            // (fsck reports it): left out from now on, as by a batch.
            let Some(position) = failure.pack else {
                return Err(failure.error.to_string());
            };
            crate::perf::count("store.maintenance_skipped_pack", 1);
            damaged.push(position);
            remember_damaged(root, &catalog, &damaged)?;
            return Ok(MaintenanceOutcome::Abandoned);
        }
    };
    between()?;
    // Published and registered by the exclusive session, if nothing moved;
    // waited for as a batch waits (the built pack is kept meanwhile).
    let store = match super::acquisition::acquire(
        "maintenance publication",
        super::acquisition::ORDINARY_WAIT,
        || super::open_exclusive(root),
    ) {
        Ok(store) => store,
        Err(error) if timed_out(&error) => return Ok(MaintenanceOutcome::Busy),
        Err(error) => return Err(error.to_string()),
    };
    for &position in &damaged {
        if let Some(name) = catalog
            .packs()
            .get(position)
            .and_then(|p| p.name().to_str())
        {
            remember_skipped(&store, &catalog, name);
        }
    }
    drop(catalog);
    let writer = NativeCas::open_exclusive(&store).map_err(|error| error.to_string())?;
    let current = writer
        .compact_catalog()
        .cloned()
        .ok_or("a compact Store has no registered catalog")?;
    if !all_registered(current.packs(), &wanted) {
        crate::perf::count("store.maintenance_abandoned", 1);
        return Ok(MaintenanceOutcome::Abandoned);
    }
    let consolidation = writer
        .publish_maintenance(built)
        .map_err(|failure| failure.error.to_string())?;
    register(&store, &consolidation)?;
    let retired: Vec<_> = consolidation
        .packs
        .iter()
        .map(|pack| pack.name.clone())
        .collect();
    writer.forget_packs(&retired);
    drop(writer);
    // Opening the CAS again removes the retired files. Best effort: the
    // pass is committed, and the next exclusive session collects them.
    drop(NativeCas::open_exclusive(&store));
    drop(lock);
    crate::perf::count("store.maintenance_packs", candidates.len() as u64);
    Ok(MaintenanceOutcome::Consolidated {
        packs: candidates.len(),
    })
}

/// Records the `damaged` packs (positions in `catalog`) as skipped, in a
/// short exclusive session; nothing when there are none.
fn remember_damaged(
    root: &std::path::Path,
    catalog: &super::cas::compact::RegisteredCatalog,
    damaged: &[usize],
) -> Result<(), String> {
    if damaged.is_empty() {
        return Ok(());
    }
    let store = super::acquisition::acquire(
        "maintenance skip record",
        super::acquisition::SHORT_WAIT,
        || super::open_exclusive(root),
    )
    .map_err(|error| error.to_string())?;
    for &position in damaged {
        if let Some(name) = catalog
            .packs()
            .get(position)
            .and_then(|p| p.name().to_str())
        {
            remember_skipped(&store, catalog, name);
        }
    }
    Ok(())
}

/// The positions in `catalog` of the `skipped` pack names.
fn skipped_positions(
    skipped: &BTreeSet<String>,
    catalog: &super::cas::compact::RegisteredCatalog,
) -> Vec<usize> {
    catalog
        .packs()
        .iter()
        .enumerate()
        .filter(|(_, pack)| {
            pack.name()
                .to_str()
                .is_some_and(|name| skipped.contains(name))
        })
        .map(|(position, _)| position)
        .collect()
}

/// Whether a batch that just committed leaves a maintenance pass to do
/// (council s17, B2c): at least `CONSOLIDATE_AT_PACKS` pack files, besides
/// the skipped ones, among which a group only maintenance may write. Read
/// from the batch's catalog; the skipped list only once there are that
/// many pack files.
pub(crate) fn maintenance_due(
    store: &NativeStoreExclusive,
    catalog: &super::cas::compact::RegisteredCatalog,
    added: Option<(bool, super::cas::compact::PackStats)>,
) -> bool {
    if !catalog.many_pack_files(added) {
        return false;
    }
    let excluded = skipped_positions(&skipped_packs(store), catalog);
    catalog.maintenance_due(&excluded, added)
}

/// The Store roots for which a batch of this process found a maintenance
/// pass due (B2c): the CLI starts it detached once its command is done.
fn due_roots() -> &'static std::sync::Mutex<BTreeSet<std::path::PathBuf>> {
    static DUE: std::sync::OnceLock<std::sync::Mutex<BTreeSet<std::path::PathBuf>>> =
        std::sync::OnceLock::new();
    DUE.get_or_init(Default::default)
}

/// Notes that a maintenance pass is due on the Store at `root`.
pub(crate) fn note_maintenance_due(root: &std::path::Path) {
    if let Ok(mut roots) = due_roots().lock() {
        roots.insert(root.to_path_buf());
    }
}

/// The Store roots a batch of this process found a maintenance pass due
/// on, forgotten here (council s17, B2c).
pub fn take_maintenance_due() -> Vec<std::path::PathBuf> {
    due_roots()
        .lock()
        .map(|mut roots| std::mem::take(&mut *roots).into_iter().collect())
        .unwrap_or_default()
}

/// Whether a maintenance pass holds the Store at `root`'s maintenance lock
/// now. Never creates the lock file.
pub fn maintenance_running(root: impl AsRef<std::path::Path>) -> bool {
    let metadata = root.as_ref().join(super::layout::METADATA_DIR_NAME);
    match open_maintenance_lock(&metadata, false) {
        Ok(Some(file)) => fs2::FileExt::try_lock_exclusive(&file).is_err(),
        _ => false,
    }
}

/// Whether every `(seq, name, digest)` of `wanted` is still registered as it
/// was.
fn all_registered(
    packs: &[super::cas::compact::RegisteredPack],
    wanted: &[(i64, std::ffi::OsString, [u8; 32])],
) -> bool {
    wanted.iter().all(|(seq, name, digest)| {
        packs.iter().any(|pack| {
            pack.seq == *seq && pack.name() == name.as_os_str() && pack.digest() == *digest
        })
    })
}

/// The packs consolidation left out (`SKIPPED_FILE`), from the metadata
/// directory itself (a maintenance pass holds no exclusive session there).
fn skipped_at(metadata: &std::path::Path) -> BTreeSet<String> {
    read_note_at(metadata, SKIPPED_FILE)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Consolidates the Store's packs if due. Returns the number of packs
/// consolidated (0 when none was due). `after_commit` runs right after the
/// registration commit (fault injection).
pub(crate) fn consolidate_if_due(
    store: &NativeStoreExclusive,
    faults: Arc<dyn CasFaultInjector>,
    after_commit: &dyn Fn() -> Result<(), String>,
) -> Result<usize, String> {
    if !store.compact() {
        return Ok(0);
    }
    let failures = recent_failures(store);
    if failures.is_some_and(|(count, at)| {
        count >= FAILURES_BEFORE_PAUSE && now_ms().saturating_sub(at) < FAILURE_PAUSE_MS
    }) {
        crate::perf::count("store.consolidation_paused", 1);
        return Ok(0);
    }
    let result = consolidate(store, faults, after_commit);
    match &result {
        Ok(count) if *count > 0 => {
            if failures.is_some() {
                let _ = std::fs::remove_file(store.layout().metadata_dir().join(FAILED_FILE));
            }
        }
        Ok(_) => {}
        Err(message) => {
            let count = failures.map_or(0, |(count, _)| count) + 1;
            write_note(
                store,
                FAILED_FILE,
                &format!("{count} {}\n{message}\n", now_ms()),
            );
        }
    }
    result
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// (count, time of the last) of the general failures in a row, if any.
fn recent_failures(store: &NativeStoreExclusive) -> Option<(u64, u64)> {
    failures_at(store.layout().metadata_dir(), FAILED_FILE)
}

/// (count, time of the last) of the failures note `name` under `metadata`.
fn failures_at(metadata: &std::path::Path, name: &str) -> Option<(u64, u64)> {
    let note = read_note_at(metadata, name)?;
    let mut fields = note.lines().next()?.split(' ');
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

/// A note under `metadata/`, read without following links: a regular file
/// of at most `NOTE_MAX_BYTES`, else absent (council s6 r3).
fn read_note(store: &NativeStoreExclusive, name: &str) -> Option<String> {
    read_note_at(store.layout().metadata_dir(), name)
}

/// `read_note` from the metadata directory `metadata`.
fn read_note_at(metadata: &std::path::Path, name: &str) -> Option<String> {
    let path = metadata.join(name);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > NOTE_MAX_BYTES {
        return None;
    }
    let file = crate::layout::open_read_no_follow(&path).ok()?;
    let mut text = String::new();
    file.take(NOTE_MAX_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= NOTE_MAX_BYTES).then_some(text)
}

/// Replaces a note with a new file renamed over it: whatever the entry was
/// (a link included), it is replaced, never written through. Best effort.
fn write_note(store: &NativeStoreExclusive, name: &str, text: &str) {
    write_note_at(store.layout().metadata_dir(), name, text);
}

/// `write_note` under the metadata directory `directory`.
fn write_note_at(directory: &std::path::Path, name: &str, text: &str) {
    let staged = directory.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = crate::layout::create_new_file_no_follow(&staged)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        super::format::atomic_replace(&staged, &directory.join(name))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
}

fn consolidate(
    store: &NativeStoreExclusive,
    faults: Arc<dyn CasFaultInjector>,
    after_commit: &dyn Fn() -> Result<(), String>,
) -> Result<usize, String> {
    let cas =
        NativeCasWriter::with_fault_injector(store, faults).map_err(|error| error.to_string())?;
    let Some(catalog) = cas.compact_catalog().cloned() else {
        return Ok(0);
    };
    let mut excluded = skipped_positions(&skipped_packs(store), &catalog);
    let remember = |position: usize| {
        if let Some(name) = catalog
            .packs()
            .get(position)
            .and_then(|p| p.name().to_str())
        {
            remember_skipped(store, &catalog, name);
        }
    };
    for _ in 0..MAX_ATTEMPTS {
        let candidates = catalog.consolidation_candidates(&excluded, cas.consolidation_budget());
        if candidates.len() < 2 {
            return Ok(0);
        }
        // A candidate with a malformed admission bitmap is left out before
        // any copy: registration would refuse it every time (council s6 r2).
        if let Some(position) =
            malformed_bitmap(store.layout().sqlite_path(), &catalog, &candidates)?
        {
            crate::perf::count("store.consolidation_skipped_pack", 1);
            remember(position);
            excluded.push(position);
            continue;
        }
        let _span = crate::perf::span("store.consolidate");
        let consolidation = match cas.consolidate_packs(&catalog, &candidates) {
            Ok(consolidation) => consolidation,
            // A pack whose records cannot all be copied is left out (it is
            // damaged: fsck reports it); the others still consolidate.
            Err(failure) => match failure.pack {
                Some(position) => {
                    crate::perf::count("store.consolidation_skipped_pack", 1);
                    remember(position);
                    excluded.push(position);
                    continue;
                }
                None => return Err(failure.error.to_string()),
            },
        };
        register(store, &consolidation)?;
        after_commit()?;
        let retired: Vec<_> = consolidation
            .packs
            .iter()
            .map(|pack| pack.name.clone())
            .collect();
        cas.forget_packs(&retired);
        drop(cas);
        // Opening the CAS again removes every unregistered pack file: the
        // retired ones.
        drop(NativeCas::open_exclusive(store).map_err(|error| error.to_string())?);
        crate::perf::count("store.consolidated_packs", candidates.len() as u64);
        return Ok(candidates.len());
    }
    Ok(0)
}

/// The packs a consolidation left out before (`SKIPPED_FILE`).
fn skipped_packs(store: &NativeStoreExclusive) -> BTreeSet<String> {
    read_note(store, SKIPPED_FILE)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Adds `name` to `SKIPPED_FILE`, keeping only packs still registered.
/// Best effort: a lost list only costs one failed attempt again.
fn remember_skipped(
    store: &NativeStoreExclusive,
    catalog: &super::cas::compact::RegisteredCatalog,
    name: &str,
) {
    let registered: BTreeSet<&str> = catalog
        .packs()
        .iter()
        .filter_map(|pack| pack.name().to_str())
        .collect();
    let mut skipped = skipped_packs(store);
    skipped.insert(name.to_owned());
    skipped.retain(|name| registered.contains(name.as_str()));
    let mut text = String::new();
    for name in &skipped {
        text.push_str(name);
        text.push('\n');
    }
    write_note(store, SKIPPED_FILE, &text);
}

/// The first candidate (position) one of whose admission bitmaps does not
/// fit its pack, read in one scan of the admissions.
fn malformed_bitmap(
    sqlite_path: &std::path::Path,
    catalog: &super::cas::compact::RegisteredCatalog,
    candidates: &[usize],
) -> Result<Option<usize>, String> {
    let entries: BTreeMap<i64, (usize, u64)> = candidates
        .iter()
        .filter_map(|position| {
            catalog
                .packs()
                .get(*position)
                .map(|pack| (pack.seq, (*position, pack.entries)))
        })
        .collect();
    let connection = sqlite::open_read_only(sqlite_path, std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT pack_seq, bitmap FROM space_pack_admissions")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (seq, bitmap) = row.map_err(|error| error.to_string())?;
        if let Some((position, count)) = entries.get(&seq) {
            if super::cas::compact::check_bitmap(&bitmap, *count).is_err() {
                return Ok(Some(*position));
            }
        }
    }
    Ok(None)
}

/// The registration transaction of a consolidation.
fn register(store: &NativeStoreExclusive, consolidation: &Consolidation) -> Result<(), String> {
    let sql = |error: rusqlite::Error| error.to_string();
    let mut connection = store
        .begin_session_writer()
        .map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    let (name, digest, entries) = &consolidation.pack;
    let name = name.to_str().ok_or("pack name is not UTF-8")?;
    let entries_sql = i64::try_from(*entries).map_err(|error| error.to_string())?;
    if consolidation.identities.len() as u64 != *entries {
        return Err("consolidated pack identities differ from its entries".into());
    }
    transaction
        .execute(
            "INSERT INTO cas_packs(name,index_digest,entries,stored_inline,created_at_ms) \
             VALUES(?1,?2,?3,?4,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![
                name,
                &digest[..],
                entries_sql,
                i64::from(consolidation.inline.is_some())
            ],
        )
        .map_err(sql)?;
    let new_seq = transaction.last_insert_rowid();
    consolidation
        .keys
        .insert(&transaction, new_seq, digest)
        .map_err(sql)?;
    if let Some(image) = &consolidation.inline {
        transaction
            .execute(
                "INSERT INTO cas_pack_inline(pack_seq,image) VALUES(?1,?2)",
                params![new_seq, image],
            )
            .map_err(sql)?;
    }
    let by_seq: BTreeMap<i64, &super::cas::compact::ConsolidatedPack> = consolidation
        .packs
        .iter()
        .map(|pack| (pack.seq, pack))
        .collect();
    let seqs: Vec<String> = by_seq.keys().map(i64::to_string).collect();
    let in_seqs = seqs.join(",");
    // The Spaces with rows on the consolidated packs, in one scan (no index
    // starts with `pack_seq`); their bitmaps are not kept.
    let spaces: Vec<String> = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT DISTINCT space_id FROM space_pack_admissions \
                 WHERE pack_seq IN ({in_seqs}) ORDER BY space_id"
            ))
            .map_err(sql)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sql)?;
        rows.collect::<Result<_, _>>().map_err(sql)?
    };
    // One Space at a time, read by its key (memory: that Space's bitmaps
    // only, council s6 r3): each admitted old ordinal maps to a new one
    // naming the same identity (the old index against the new), the new
    // bitmap is written, then read back by its key and compared byte for
    // byte.
    let length = usize::try_from(entries.div_ceil(8)).map_err(|error| error.to_string())?;
    let mut old_count = 0_usize;
    for space in &spaces {
        let rows: Vec<(i64, Vec<u8>)> = {
            let mut statement = transaction
                .prepare_cached(
                    "SELECT pack_seq, bitmap FROM space_pack_admissions WHERE space_id=?1",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map([space], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(sql)?;
            rows.filter(|row| {
                row.as_ref()
                    .map_or(true, |(seq, _)| by_seq.contains_key(seq))
            })
            .collect::<Result<_, _>>()
            .map_err(sql)?
        };
        old_count += rows.len();
        let mut expected = vec![0_u8; length.max(1)];
        for (seq, bitmap) in &rows {
            let pack = by_seq[seq];
            // A malformed bitmap is corruption: never remapped into a
            // narrower answer (council s6 r1).
            super::cas::compact::check_bitmap(bitmap, pack.ordinals.len() as u64)
                .map_err(|reason| format!("{space} admissions of pack {seq}: {reason}"))?;
            for (old, new) in pack.ordinals.iter().enumerate() {
                if !super::cas::compact::bit(bitmap, old as u64) {
                    continue;
                }
                let renamed = consolidation
                    .identities
                    .get(usize::try_from(*new).map_err(|error| error.to_string())?);
                if renamed != Some(&pack.identities[old]) {
                    return Err(format!("consolidation would change what {space} admits"));
                }
                super::cas::compact::set_bit(&mut expected, *new);
            }
        }
        transaction
            .execute(
                "INSERT INTO space_pack_admissions(space_id,pack_seq,bitmap) VALUES(?1,?2,?3)",
                params![space, new_seq, expected],
            )
            .map_err(sql)?;
        let written: Vec<u8> = transaction
            .query_row(
                "SELECT bitmap FROM space_pack_admissions WHERE space_id=?1 AND pack_seq=?2",
                params![space, new_seq],
                |row| row.get(0),
            )
            .map_err(sql)?;
        super::cas::compact::check_bitmap(&written, *entries)?;
        if written != expected {
            return Err(format!("consolidation changed what {space} admits"));
        }
    }
    // Every old admission row in one statement (one scan): exactly the rows
    // remapped above, then each old pack's row, which must be exactly the
    // one consolidated.
    let deleted = transaction
        .execute(
            &format!("DELETE FROM space_pack_admissions WHERE pack_seq IN ({in_seqs})"),
            [],
        )
        .map_err(sql)?;
    if deleted != old_count {
        return Err("consolidation would drop admissions it did not remap".into());
    }
    // The keys rows of the packs retired: one each.
    let keys_retired = transaction
        .execute(
            &format!("DELETE FROM cas_pack_keys WHERE pack_seq IN ({in_seqs})"),
            [],
        )
        .map_err(sql)?;
    // A candidate whose keys row is missing is replaced by a pack that has
    // one (council s18 r1): at most one row each (`pack_seq` is its key).
    debug_assert!(keys_retired <= consolidation.packs.len());
    // The images of the inline packs retired, before their rows: exactly
    // those of the inline candidates.
    let inline_retired = transaction
        .execute(
            &format!("DELETE FROM cas_pack_inline WHERE pack_seq IN ({in_seqs})"),
            [],
        )
        .map_err(sql)?;
    if inline_retired
        != consolidation
            .packs
            .iter()
            .filter(|pack| pack.inline)
            .count()
    {
        return Err("consolidated inline packs differ from their images".into());
    }
    for pack in &consolidation.packs {
        let name = pack.name.to_str().ok_or("pack name is not UTF-8")?;
        let removed = transaction
            .execute(
                "DELETE FROM cas_packs WHERE pack_seq=?1 AND name=?2 AND index_digest=?3",
                params![pack.seq, name, &pack.digest[..]],
            )
            .map_err(sql)?;
        if removed != 1 {
            return Err(format!("pack {} is not the one consolidated", pack.seq));
        }
    }
    transaction.commit().map_err(sql)?;
    sqlite::finish_writer(connection, store.layout().sqlite_path())
        .map_err(|error| error.to_string())
}
