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
    let note = read_note(store, FAILED_FILE)?;
    let mut fields = note.lines().next()?.split(' ');
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

/// A note under `metadata/`, read without following links: a regular file
/// of at most `NOTE_MAX_BYTES`, else absent (council s6 r3).
fn read_note(store: &NativeStoreExclusive, name: &str) -> Option<String> {
    let path = store.layout().metadata_dir().join(name);
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
    let directory = store.layout().metadata_dir();
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
    let skipped = skipped_packs(store);
    let mut excluded: Vec<usize> = catalog
        .packs()
        .iter()
        .enumerate()
        .filter(|(_, pack)| {
            pack.name()
                .to_str()
                .is_some_and(|name| skipped.contains(name))
        })
        .map(|(position, _)| position)
        .collect();
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
        let candidates = catalog.consolidation_candidates(&excluded);
        if candidates.len() < 2 {
            return Ok(0);
        }
        // A candidate with a malformed admission bitmap is left out before
        // any copy: registration would refuse it every time (council s6 r2).
        if let Some(position) = malformed_bitmap(store, &catalog, &candidates)? {
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
    store: &NativeStoreExclusive,
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
    let connection = sqlite::open_read_only(
        store.layout().sqlite_path(),
        std::time::Duration::from_secs(5),
    )
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
            "INSERT INTO cas_packs(name,index_digest,entries,created_at_ms) \
             VALUES(?1,?2,?3,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![name, &digest[..], entries_sql],
        )
        .map_err(sql)?;
    let new_seq = transaction.last_insert_rowid();
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
