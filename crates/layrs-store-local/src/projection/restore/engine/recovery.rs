//! Detached, bounded restore journal evidence. It grants no connection or lock.
use super::*;
use layrs_core::scm::{FileId, FileManifestId};
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection) struct RestoreRecoveryData {
    journal: Vec<u8>,
    operation: String,
    reservation: String,
    bytes: u64,
    witness: String,
    root_id: String,
    root_digest: String,
    representations: BTreeSet<(FileId, FileManifestId, u64)>,
}

impl RestoreRecoveryData {
    /// Call only after the independently closed private preflight has admitted
    /// this owner's family. No catalog writer, cancellation or settlement.
    pub(in crate::projection) fn validate_family(
        &self,
        lease: &shared::Lease,
        store: &ProjectionStore,
    ) -> Result<()> {
        if !lease.is_admitted() {
            return Err(invalid("restore recovery requires admitted family context"));
        }
        lease.with_content_catalog(false, |c| {
            lease.authorize_content_reservation(c, &self.reservation)?;
            let bounds: Option<(u64, u64)> = c.query_row(
                "SELECT length(CAST(owner AS BLOB)),length(CAST(operation AS BLOB)) FROM reservations WHERE id=?1",
                [&self.reservation], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            if !matches!(bounds, Some((owner, operation)) if owner <= 512 && operation <= 128) {
                return Err(invalid("restore reservation missing or exceeds bounds"));
            }
            let (owner, operation, remaining): (String, String, u64) = c.query_row(
                "SELECT owner,operation,remaining FROM reservations WHERE id=?1",
                [&self.reservation], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if owner != store.identity().worktree_id.as_str()
                || operation != self.operation || remaining > self.bytes
            {
                return Err(invalid("restore reservation owner, operation or budget differs"));
            }
            validate_protection_index(c, &self.root_id, self.representations.len())?;
            Ok(())
        })?;
        // Catalog gate above is closed before the independent protection read.
        let manifests =
            lease.verify_protection_metadata(&store.marker, &self.root_id, &self.root_digest)?;
        let binding = store
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("admitted restore lost shared binding"))?;
        let expected = shared::Protection {
            incarnation: binding.incarnation.clone(),
            owner: store.identity().worktree_id.clone(),
            database_identity: binding.database_identity.clone(),
            operation: self.operation.clone(),
            manifests: manifests.clone(),
        };
        if shared::digest(&expected)? != self.root_id {
            return Err(invalid("restore protection belongs to another operation"));
        }
        let actual = manifests
            .iter()
            .map(|m| (m.file_id().clone(), m.id().clone(), m.byte_length()))
            .collect::<BTreeSet<_>>();
        if actual != self.representations {
            return Err(invalid(
                "restore protection differs from exact pending representations",
            ));
        }
        for manifest in &manifests {
            lease.verify_manifest(manifest)?;
        }
        Ok(())
    }
}

/// Bound the ordinary root before the shared metadata reader materializes its
/// index. The retained exclusive owner, owner-scoped root authorization and
/// family lease's GC exclusion stabilize this root between the RO gates.
fn validate_protection_index(c: &Connection, root: &str, expected: usize) -> Result<()> {
    if expected > 2048 {
        return Err(invalid(
            "restore protection expected membership exceeds bound",
        ));
    }
    let bounds: Option<(u64, u64)> = c.query_row(
        "SELECT length(CAST(digest AS BLOB)),length(CAST(payload AS BLOB)) FROM roots WHERE id=?1",
        [root], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    if !matches!(bounds, Some((64, payload)) if payload > 0 && payload <= 16 * 1024 * 1024) {
        return Err(invalid(
            "restore protection metadata missing or exceeds bound",
        ));
    }
    let (count, oversized): (usize, bool) = c.query_row(
        "SELECT count(*),coalesce(max(bytes!=71),0) FROM (SELECT length(CAST(manifest_id AS BLOB)) AS bytes FROM root_manifests WHERE root_id=?1 LIMIT ?2)",
        rusqlite::params![root, expected + 1], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != expected || oversized {
        return Err(invalid(
            "restore protection membership index differs or exceeds bound",
        ));
    }
    Ok(())
}

/// Caller owns a genuine RO preflight transaction or the same bound recovery
/// writer. Never opens SQL, acquires locks, ends transactions or reads content.
pub(in crate::projection) fn recovery_preflight(
    c: &Connection,
    store: &ProjectionStore,
) -> Result<RestoreRecoveryData> {
    if c.is_autocommit() {
        return Err(invalid("restore recovery DATA requires an active snapshot"));
    }
    read_pending(c, store)
}

fn read_pending(c: &Connection, store: &ProjectionStore) -> Result<RestoreRecoveryData> {
    if c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))? != 9 {
        return Err(invalid("admitted restore recovery requires private9"));
    }
    crate::projection::provider::ordinary_transition::require_no_live(c)?;
    let conflicts: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_publication_pending) OR EXISTS(SELECT 1 FROM projection_rebase_pending) OR EXISTS(SELECT 1 FROM projection_prejournal) OR EXISTS(SELECT 1 FROM projection_preparations WHERE status='acquiring')",
        [], |r| r.get(0),
    )?;
    if conflicts {
        return Err(invalid(
            "another pending operation blocks admitted restore recovery",
        ));
    }
    let (count, length): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(payload)),0) FROM projection_restore_pending",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || length == 0 || length > 8 * 1024 * 1024 {
        return Err(invalid("pending restore journal absent or exceeds bound"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_restore_pending WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let journal = decode(&bytes)?;
    let expected = &journal.preview;
    validate_destination(store, expected)?;
    let state = database::current(c)?;
    if state.generation != expected.projection_generation
        || state.digest != expected.projection_digest
    {
        return Err(invalid("pending restore base changed"));
    }
    validate_paths(&state.snapshot, &expected.before)?;
    validate_paths(&state.snapshot, &expected.after)?;
    let selection = selection::current(c)?;
    selection.validate_snapshot(&state.snapshot)?;
    if expected.before.keys().any(|path| !selection.includes(path)) {
        return Err(invalid("pending restore targets an excluded path"));
    }
    let operation = format!("restore:{}", expected.operation_id);
    let (count, oversized): (u64, bool) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(operation AS BLOB))>128 OR length(CAST(reservation AS BLOB))=0 OR length(CAST(reservation AS BLOB))>128 OR length(CAST(witness AS BLOB))!=64),0) FROM projection_budgets",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || oversized {
        return Err(invalid("restore private budget absent or exceeds bound"));
    }
    let (actual, reservation, budget, witness, status): (String, String, u64, String, String) = c.query_row(
        "SELECT operation,reservation,bytes,witness,state FROM projection_budgets WHERE singleton=1",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    )?;
    if actual != operation || status != "active" || budget < 65536 || !valid_digest(&witness) {
        return Err(invalid(
            "restore private budget does not own pending operation",
        ));
    }
    let (count, oversized): (u64, bool) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(root_id AS BLOB))!=64 OR length(CAST(digest AS BLOB))!=64),0) FROM projection_claims WHERE kind='restore' AND owner=?1",
        [&expected.operation_id], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || oversized {
        return Err(invalid("restore exact protection missing or ambiguous"));
    }
    let (root_id, root_digest): (String, String) = c.query_row(
        "SELECT root_id,digest FROM projection_claims WHERE kind='restore' AND owner=?1",
        [&expected.operation_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if !valid_digest(&root_id) || root_id != root_digest {
        return Err(invalid("restore claim digest mismatch"));
    }
    let mut representations = BTreeSet::new();
    for file in expected
        .before
        .values()
        .chain(expected.after.values())
        .flatten()
    {
        let length: u64 = c.query_row(
            "SELECT length(CAST(manifest_id AS BLOB)) FROM projection_content WHERE file_id=?1",
            [file.file_id.as_str()],
            |r| r.get(0),
        )?;
        if length != 71 {
            return Err(invalid("restore representation exceeds bound"));
        }
        let (id, stored_bytes): (String, u64) = c.query_row(
            "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
            [file.file_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if stored_bytes != file.bytes {
            return Err(invalid("restore representation size differs"));
        }
        representations.insert((
            file.file_id.clone(),
            FileManifestId::parse(id).map_err(invalid)?,
            stored_bytes,
        ));
    }
    Ok(RestoreRecoveryData {
        journal: bytes,
        operation,
        reservation,
        bytes: budget,
        witness,
        root_id,
        root_digest,
        representations,
    })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::provider::owned_test as child;

    #[test]
    #[ignore = "ROOT source release and audited SQLite host; child120 / outer180"]
    fn restore_protection_index_refuses_excess_members_and_oversized_keys() {
        child::run(
            concat!(
                module_path!(),
                "::restore_protection_index_refuses_excess_members_and_oversized_keys"
            ),
            120,
            || {
                child::declare();
                let c = Connection::open_in_memory().unwrap();
                c.execute_batch("CREATE TABLE roots(id TEXT PRIMARY KEY,digest TEXT,payload BLOB); CREATE TABLE root_manifests(root_id TEXT,manifest_id TEXT,PRIMARY KEY(root_id,manifest_id));").unwrap();
                let digest = "a".repeat(64);
                let manifest = format!("blake3:{}", "b".repeat(64));
                c.execute("INSERT INTO roots VALUES('root',?1,x'7b7d')", [&digest])
                    .unwrap();
                c.execute("INSERT INTO root_manifests VALUES('root',?1)", [&manifest])
                    .unwrap();
                validate_protection_index(&c, "root", 1).unwrap();
                assert!(validate_protection_index(&c, "root", 0).is_err());
                assert!(validate_protection_index(&c, "root", 2).is_err());
                assert!(validate_protection_index(&c, "missing", 0).is_err());
                c.execute(
                    "INSERT INTO root_manifests VALUES('root',?1)",
                    ["x".repeat(4096)],
                )
                .unwrap();
                assert!(validate_protection_index(&c, "root", 1).is_err());
                assert!(validate_protection_index(&c, "root", 2).is_err());
                c.execute(
                    "DELETE FROM root_manifests WHERE manifest_id!=?1",
                    [&manifest],
                )
                .unwrap();
                c.execute("UPDATE roots SET digest=?1", ["x".repeat(4096)])
                    .unwrap();
                assert!(validate_protection_index(&c, "root", 1).is_err());
                c.execute("UPDATE roots SET digest=?1", [&digest]).unwrap();
                validate_protection_index(&c, "root", 1).unwrap();
                c.execute("DELETE FROM root_manifests", []).unwrap();
                validate_protection_index(&c, "root", 0).unwrap();
            },
        );
    }
}
