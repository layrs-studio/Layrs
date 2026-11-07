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
use super::cas::compact::{Consolidation, Identity};
use super::cas::{CasFaultInjector, NativeCas, NativeCasWriter};
use super::{NativeStoreExclusive, sqlite};
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Candidates left out after a failure before giving up for this session.
const MAX_ATTEMPTS: usize = 3;

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
    let mut excluded = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        let candidates = catalog.consolidation_candidates(&excluded);
        if candidates.len() < 2 {
            return Ok(0);
        }
        let _span = crate::perf::span("store.consolidate");
        let consolidation = match cas.consolidate_packs(&catalog, &candidates) {
            Ok(consolidation) => consolidation,
            // A pack whose records cannot all be copied is left out (it is
            // damaged: fsck reports it); the others still consolidate.
            Err(failure) => match failure.pack {
                Some(position) => {
                    crate::perf::count("store.consolidation_skipped_pack", 1);
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
    // Per Space: the new ordinals it admits, and the identities it admitted
    // through the old packs (from their own indexes).
    let mut admitted: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    let mut identities: BTreeMap<String, BTreeSet<Identity>> = BTreeMap::new();
    for pack in &consolidation.packs {
        let mut statement = transaction
            .prepare("SELECT space_id, bitmap FROM space_pack_admissions WHERE pack_seq=?1")
            .map_err(sql)?;
        let rows = statement
            .query_map([pack.seq], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql)?;
        for row in rows {
            let (space, bitmap) = row.map_err(sql)?;
            // A malformed bitmap is corruption: never remapped into a
            // narrower answer (council s6 r1).
            super::cas::compact::check_bitmap(&bitmap, pack.ordinals.len() as u64)
                .map_err(|reason| format!("{space} admissions of pack {}: {reason}", pack.seq))?;
            let set = admitted.entry(space.clone()).or_default();
            let names = identities.entry(space).or_default();
            for (old, new) in pack.ordinals.iter().enumerate() {
                if super::cas::compact::bit(&bitmap, old as u64) {
                    set.insert(*new);
                    names.insert(pack.identities[old]);
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
    for pack in &consolidation.packs {
        transaction
            .execute(
                "DELETE FROM space_pack_admissions WHERE pack_seq=?1",
                params![pack.seq],
            )
            .map_err(sql)?;
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
    // Each Space admits through the new pack exactly the identities it
    // admitted through the old ones: read back, named by the new index.
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
            let read: BTreeSet<Identity> = (0..*entries)
                .filter(|ordinal| super::cas::compact::bit(&bitmap, *ordinal))
                .map(|ordinal| consolidation.identities[ordinal as usize])
                .collect();
            if identities.get(&space) != Some(&read) {
                return Err(format!("consolidation changed what {space} admits"));
            }
            seen += 1;
        }
        if seen != identities.len() {
            return Err("consolidation lost a Space's admissions".into());
        }
    }
    transaction.commit().map_err(sql)?;
    sqlite::finish_writer(connection, store.layout().sqlite_path())
        .map_err(|error| error.to_string())
}
