//! Pack consolidation (ADR 0028, section 6): once a Store holds enough
//! registered packs, the smallest are written into one new pack, published
//! durably, then one transaction registers it, remaps every admission
//! bitmap exactly from the old ordinals to the new ones (the set each Space
//! admits is checked unchanged in the same transaction) and unregisters the
//! old packs. Their files, no longer registered, are removed under the same
//! exclusive lock (or, if that fails, by the next exclusive session).
use super::cas::{NativeCas, compact::Consolidation};
use super::{NativeStoreExclusive, sqlite};
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};

/// Consolidates the Store's packs if due. Returns the number of packs
/// consolidated (0 when none was due).
pub(crate) fn consolidate_if_due(store: &NativeStoreExclusive) -> Result<usize, String> {
    if !store.compact() {
        return Ok(0);
    }
    let cas = NativeCas::open_exclusive(store).map_err(|error| error.to_string())?;
    let Some(catalog) = cas.compact_catalog().cloned() else {
        return Ok(0);
    };
    let candidates = catalog.consolidation_candidates();
    if candidates.len() < 2 {
        return Ok(0);
    }
    let _span = crate::perf::span("store.consolidate");
    let consolidation = cas
        .consolidate_packs(&catalog, &candidates)
        .map_err(|error| error.to_string())?;
    register(store, &consolidation)?;
    cas.forget_packs(&consolidation.retired);
    drop(cas);
    // Opening the CAS again removes every unregistered pack file: the
    // retired ones.
    drop(NativeCas::open_exclusive(store).map_err(|error| error.to_string())?);
    crate::perf::count("store.consolidated_packs", candidates.len() as u64);
    Ok(candidates.len())
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
    transaction
        .execute(
            "INSERT INTO cas_packs(name,index_digest,entries,created_at_ms) \
             VALUES(?1,?2,?3,CAST(unixepoch('subsec')*1000 AS INTEGER))",
            params![name, &digest[..], entries_sql],
        )
        .map_err(sql)?;
    let new_seq = transaction.last_insert_rowid();
    let remap: BTreeMap<i64, &Vec<u64>> = consolidation
        .remap
        .iter()
        .map(|(seq, ordinals)| (*seq, ordinals))
        .collect();
    // Every Space's bitmaps over the consolidated packs, remapped.
    let mut admitted: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
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
            let Some(ordinals) = remap.get(&seq) else {
                continue;
            };
            let set = admitted.entry(space).or_default();
            for (old, new) in ordinals.iter().enumerate() {
                if super::cas::compact::bit(&bitmap, old as u64) {
                    set.insert(*new);
                }
            }
        }
    }
    let length = usize::try_from(entries.div_ceil(8)).map_err(|error| error.to_string())?;
    for (space, ordinals) in &admitted {
        let mut bitmap = vec![0_u8; length.max(1)];
        for ordinal in ordinals {
            super::cas::compact::set_bit(&mut bitmap, *ordinal);
        }
        transaction
            .execute(
                "INSERT INTO space_pack_admissions(space_id,pack_seq,bitmap) VALUES(?1,?2,?3)",
                params![space, new_seq, bitmap],
            )
            .map_err(sql)?;
    }
    for seq in remap.keys() {
        transaction
            .execute(
                "DELETE FROM space_pack_admissions WHERE pack_seq=?1",
                params![seq],
            )
            .map_err(sql)?;
        transaction
            .execute("DELETE FROM cas_packs WHERE pack_seq=?1", params![seq])
            .map_err(sql)?;
    }
    // The set each Space admits through the new pack is exactly the
    // remapped one (read back in this transaction).
    {
        let mut statement = transaction
            .prepare("SELECT space_id, bitmap FROM space_pack_admissions WHERE pack_seq=?1")
            .map_err(sql)?;
        let rows = statement
            .query_map([new_seq], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql)?;
        let mut seen = 0;
        for row in rows {
            let (space, bitmap) = row.map_err(sql)?;
            super::cas::compact::check_bitmap(&bitmap, *entries)?;
            let read: BTreeSet<u64> = (0..*entries)
                .filter(|ordinal| super::cas::compact::bit(&bitmap, *ordinal))
                .collect();
            if admitted.get(&space) != Some(&read) {
                return Err(format!("consolidation changed what {space} admits"));
            }
            seen += 1;
        }
        if seen != admitted.len() {
            return Err("consolidation lost a Space's admissions".into());
        }
    }
    transaction.commit().map_err(sql)?;
    sqlite::finish_writer(connection, store.layout().sqlite_path())
        .map_err(|error| error.to_string())
}
