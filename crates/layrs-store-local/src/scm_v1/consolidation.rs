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
use std::sync::Arc;

/// Candidates left out after a failure before giving up for this session.
const MAX_ATTEMPTS: usize = 3;
/// Packs a consolidation could not copy (damaged records, malformed
/// bitmaps), one name per line, under `metadata/`: left out by every later
/// session until they are no longer registered, so damaged packs never
/// starve the sound ones (council s6 r2). Only ever excludes candidates.
const SKIPPED_FILE: &str = "consolidation-skipped";

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
    std::fs::read_to_string(store.layout().metadata_dir().join(SKIPPED_FILE))
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
    let _ = std::fs::write(store.layout().metadata_dir().join(SKIPPED_FILE), text);
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
    // The old rows of the consolidated packs, in one scan (no index starts
    // with `pack_seq`), kept as bitmaps (one bit per object), by Space.
    let mut old_rows: BTreeMap<String, Vec<(i64, Vec<u8>)>> = BTreeMap::new();
    {
        let mut statement = transaction
            .prepare("SELECT space_id, pack_seq, bitmap FROM space_pack_admissions")
            .map_err(sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(sql)?;
        for row in rows {
            let (space, seq, bitmap) = row.map_err(sql)?;
            if by_seq.contains_key(&seq) {
                old_rows.entry(space).or_default().push((seq, bitmap));
            }
        }
    }
    // One Space at a time (memory: its bitmaps only): each admitted old
    // ordinal maps to a new one naming the same identity (the old index
    // against the new), the new bitmap is written, then read back by its
    // key and compared byte for byte.
    let length = usize::try_from(entries.div_ceil(8)).map_err(|error| error.to_string())?;
    for (space, rows) in &old_rows {
        let mut expected = vec![0_u8; length.max(1)];
        for (seq, bitmap) in rows {
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
    // Every old admission row in one statement (one scan), then each old
    // pack's row, which must be exactly the one consolidated.
    let seqs: Vec<String> = by_seq.keys().map(i64::to_string).collect();
    transaction
        .execute(
            &format!(
                "DELETE FROM space_pack_admissions WHERE pack_seq IN ({})",
                seqs.join(",")
            ),
            [],
        )
        .map_err(sql)?;
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
    let rows: i64 = transaction
        .query_row(
            "SELECT count(*) FROM space_pack_admissions WHERE pack_seq=?1",
            [new_seq],
            |row| row.get(0),
        )
        .map_err(sql)?;
    if rows != old_rows.len() as i64 {
        return Err("consolidation lost a Space's admissions".into());
    }
    transaction.commit().map_err(sql)?;
    sqlite::finish_writer(connection, store.layout().sqlite_path())
        .map_err(|error| error.to_string())
}
