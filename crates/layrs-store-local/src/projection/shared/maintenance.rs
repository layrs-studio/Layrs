use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl PartialStoreFamily {
    /// No TTL. Every owner must be readable and identity-valid before removing
    /// any excess protection. Durable private claims include completed journals.
    pub fn reconcile_retention(&self) -> Result<u64> {
        let lease = self.lease(SessionMode::Exclusive)?;
        reconcile(&lease)
    }
    /// Explicit bounded reclamation; no capture, state, recovery or manual pin
    /// is removed. A Windows sharing violation leaves a durable sweep intent.
    pub fn evict_unretained(&self) -> Result<u64> {
        let lease = self.lease(SessionMode::Exclusive)?;
        reconcile(&lease)?;
        let (installing_manifests, installing_chunks) = lease.installation_dependencies()?;
        let mut catalog = lease.catalog(true)?;
        let tx = catalog
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut query=tx.prepare("SELECT id,length(payload) FROM manifests WHERE NOT EXISTS(SELECT 1 FROM root_manifests WHERE manifest_id=manifests.id) ORDER BY id LIMIT 1024")?;
        let manifests = query
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        for (id, bytes) in manifests {
            if installing_manifests.contains(&id) {
                continue;
            }
            tx.execute(
                "INSERT INTO sweep VALUES('manifests',?1,?2) ON CONFLICT DO NOTHING",
                params![id, bytes],
            )?;
            tx.execute("DELETE FROM manifest_chunks WHERE manifest_id=?1", [&id])?;
            tx.execute("DELETE FROM manifests WHERE id=?1", [&id])?;
        }
        let mut query=tx.prepare("SELECT id,bytes FROM chunks WHERE NOT EXISTS(SELECT 1 FROM manifest_chunks WHERE chunk_id=chunks.id) ORDER BY id LIMIT 16384")?;
        let chunks = query
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        for (id, bytes) in chunks {
            if installing_chunks.contains(&id) {
                continue;
            }
            tx.execute(
                "INSERT INTO sweep VALUES('chunks',?1,?2) ON CONFLICT DO NOTHING",
                params![id, bytes],
            )?;
            tx.execute("DELETE FROM chunks WHERE id=?1", [&id])?;
        }
        tx.commit()?;
        let mut query = catalog
            .connection
            .prepare("SELECT kind,id FROM sweep ORDER BY kind,id LIMIT 17408")?;
        let objects = query
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        let mut count = 0;
        for (kind, id) in objects {
            if (kind == "manifests" && installing_manifests.contains(&id))
                || (kind == "chunks" && installing_chunks.contains(&id))
            {
                continue;
            }
            let name = id
                .strip_prefix("blake3:")
                .filter(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
                .ok_or_else(|| invalid("invalid sweep content ID"))?;
            let path = self.root.join("objects").join(&kind);
            let directory = PinnedDirectory::open(&path)?;
            match directory.open_file(name.as_ref()) {
                Ok(pin) => {
                    pin.unlink()?;
                    directory.sync()?;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
            catalog.connection.execute(
                "DELETE FROM sweep WHERE kind=?1 AND id=?2",
                params![kind, id],
            )?;
            count += 1;
        }
        lease.verify()?;
        Ok(count)
    }
}
fn reconcile(lease: &Lease) -> Result<u64> {
    let catalog = lease.catalog(false)?;
    let mut query=catalog.connection.prepare("SELECT id,metadata_path,database_identity,marker,state FROM worktrees ORDER BY id LIMIT 16385")?;
    let owners = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    drop(catalog);
    if owners.len() > 16384 {
        return Err(invalid("family owner reconciliation exceeds bound"));
    }
    let mut claims = BTreeMap::new();
    let mut committed_reservations = BTreeSet::new();
    let mut referenced_reservations = BTreeMap::new();
    for (id, path, db_identity, bytes, state) in owners {
        if state != "ready" {
            return Err(invalid("initializing owner prevents reclamation"));
        }
        let marker: format::Marker = serde_json::from_slice(&bytes).map_err(invalid)?;
        marker.validate()?;
        let binding = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("owner is not a shared Worktree"))?;
        if marker.identity.worktree_id.as_str() != id
            || binding.incarnation != lease.family.marker.incarnation
            || binding.root_identity != lease.family.marker.root_identity
            || binding.root != lease.family.root
            || Path::new(&path) != binding.metadata_path
            || binding.database_identity != db_identity
            || binding.metadata_path != lease.family.member_metadata(&marker.identity.worktree_id)
        {
            return Err(invalid("owner registry identity mismatch"));
        }
        let metadata = PinnedDirectory::open(&binding.metadata_path)?;
        let pin = super::super::file::DatabasePin::open(
            &binding.metadata_path.join("projection.sqlite3"),
        )?;
        if pin.identity() != db_identity || metadata.identity_token() != marker.metadata_identity {
            return Err(invalid(
                "owner metadata identity changed; retention preserved",
            ));
        }
        let _owner_lock = StoreLock::acquire_existing(
            &binding.metadata_path.join("layrs.lock"),
            SessionMode::Exclusive,
        )
        .map_err(invalid)?;
        let c = database::read_owner_with_recovery(
            &binding.metadata_path.join("projection.sqlite3"),
            &marker,
            &pin,
        )?;
        let mut q = c.prepare("SELECT root_id,digest FROM projection_claims")?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        for (root, digest) in rows {
            lease.verify_protection(&marker, &root, &digest)?;
            claims.insert(root, (id.clone(), digest));
        }
        let completed:Option<String>=c.query_row("SELECT reservation FROM projection_budgets WHERE singleton=1 AND state='committed'",[],|r|r.get(0)).optional()?;
        if let Some(id) = completed {
            committed_reservations.insert(id);
        }
        let mut q = c.prepare(
            "SELECT reservation FROM projection_preparations WHERE status IN ('ready','released')",
        )?;
        for id in q.query_map([], |r| r.get::<_, String>(0))? {
            committed_reservations.insert(id?);
        }
        drop(q);
        let mut q = c.prepare("SELECT reservation FROM projection_budgets UNION SELECT reservation FROM projection_preparations")?;
        for reservation in q.query_map([], |r| r.get::<_, String>(0))? {
            let reservation = reservation?;
            if referenced_reservations
                .insert(reservation, id.clone())
                .is_some_and(|old| old != id)
            {
                return Err(invalid("reservation claimed by multiple Worktrees"));
            }
        }
        pin.verify(&binding.metadata_path.join("projection.sqlite3"))?;
        metadata.verify_path_identity()?;
    }
    // All owners were readable under the exclusive lifecycle. A reservation with
    // no private budget/preparation cannot still be acquiring. Recover only its
    // authenticated shared installation intents before releasing its charge.
    let catalog = lease.catalog(false)?;
    let mut q = catalog
        .connection
        .prepare("SELECT id,owner FROM reservations ORDER BY id LIMIT 16385")?;
    let reservations = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    drop(catalog);
    if reservations.len() > 16384 {
        return Err(invalid("reservation reconciliation exceeds bound"));
    }
    for (reservation, owner) in reservations {
        if referenced_reservations
            .get(&reservation)
            .is_some_and(|expected| expected != &owner)
        {
            return Err(invalid("reservation ownership mismatch"));
        }
        if !referenced_reservations.contains_key(&reservation)
            || committed_reservations.contains(&reservation)
        {
            let owner = WorktreeId::parse(owner).map_err(invalid)?;
            lease.cancel_installations(&reservation, &owner)?;
            lease.release_reservation(&reservation, &owner)?;
        }
    }
    let mut catalog = lease.catalog(true)?;
    let mut query = catalog
        .connection
        .prepare("SELECT id,owner,digest FROM roots ORDER BY id")?;
    let roots = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    let tx = catalog
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut removed = 0;
    for (root, owner, digest) in roots {
        if let Some((claimed_owner, claimed_digest)) = claims.get(&root) {
            if claimed_owner != &owner || claimed_digest != &digest {
                return Err(invalid("private claim binding differs from shared root"));
            }
        } else {
            tx.execute("DELETE FROM roots WHERE id=?1", [root])?;
            removed += 1;
        }
    }
    tx.commit()?;
    Ok(removed)
}
