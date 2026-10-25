//! Two-database acceptance: protect first, accept a private claim second.
//! A failed/ambiguous acceptance leaks protection rather than losing bytes.
use super::*;
mod staging;
use crate::secure_fs::PinnedFile;
use layrs_core::scm::{FileId, FileManifest, FileManifestId};
use rusqlite::{Connection, OptionalExtension, params};
pub(super) use staging::{accept_staging, require_no_staging, track_directory, track_file};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, SeekFrom},
};

pub(super) const SQL: &str = r#"
CREATE TABLE projection_content(file_id TEXT PRIMARY KEY,manifest_id TEXT NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>=0)) STRICT;
CREATE TABLE projection_claims(root_id TEXT PRIMARY KEY,digest TEXT NOT NULL,kind TEXT NOT NULL,owner TEXT NOT NULL) STRICT;
CREATE TABLE projection_budgets(singleton INTEGER PRIMARY KEY CHECK(singleton=1),operation TEXT NOT NULL,reservation TEXT NOT NULL,bytes INTEGER NOT NULL,witness TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('active','committed'))) STRICT;
CREATE TABLE projection_prejournal(path TEXT PRIMARY KEY,operation TEXT NOT NULL,digest TEXT NOT NULL,payload BLOB NOT NULL CHECK(length(payload)<=524288),state TEXT NOT NULL CHECK(state IN ('owned','cleaning'))) STRICT;
CREATE TABLE projection_preparations(id TEXT PRIMARY KEY,payload BLOB NOT NULL CHECK(length(payload)<=16777216),status TEXT NOT NULL CHECK(status IN ('acquiring','ready','released')),root_id TEXT,reservation TEXT NOT NULL) STRICT;
CREATE TABLE projection_observations(digest TEXT PRIMARY KEY,payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
CREATE TABLE projection_imports(singleton INTEGER PRIMARY KEY CHECK(singleton=1),digest TEXT NOT NULL,payload BLOB NOT NULL CHECK(length(payload)<=16777216)) STRICT;
CREATE TABLE projection_conversion(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=16777216),ready INTEGER NOT NULL CHECK(ready IN (0,1))) STRICT;
"#;

pub(super) fn budget_for(files: &[CapturedFile], extra: u64) -> Result<u64> {
    let mut ids = BTreeSet::new();
    let mut total = extra
        .checked_add(65536)
        .ok_or_else(|| invalid("quota arithmetic overflow"))?;
    for file in files {
        if file.bytes > MAX_CAPTURE_FILE_BYTES {
            return Err(invalid("file exceeds shared content bound"));
        }
        if ids.insert(file.file_id.clone()) {
            total = total
                .checked_add(file.bytes)
                .and_then(|n| {
                    n.checked_add(2048 + file.bytes.div_ceil(CAPTURE_CHUNK_BYTES as u64) * 256)
                })
                .ok_or_else(|| invalid("quota arithmetic overflow"))?;
        }
    }
    Ok(total)
}
pub(super) fn reserve(
    c: &Connection,
    store: &ProjectionStore,
    operation: &str,
    witness: &str,
    files: &[CapturedFile],
    extra: u64,
) -> Result<()> {
    let Some(lease) = shared::for_store(store)? else {
        return Ok(());
    };
    settle(c, store)?;
    if fs2::available_space(&store.root)? < extra.saturating_add(65536) {
        return Err(invalid(
            "partial_store_quota: insufficient checkout staging space",
        ));
    }
    let mut missing = Vec::new();
    for file in files {
        if let Some(manifest) = lease.manifest_for(&file.file_id)? {
            lease.verify_manifest(&manifest)?;
        } else {
            missing.push(file.clone());
        }
    }
    let budget = budget_for(&missing, extra)?;
    let prior: Option<(String, String, u64)> = c
        .query_row(
            "SELECT operation,witness,bytes FROM projection_budgets WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((op, old, bytes)) = prior {
        if op != operation || old != witness || bytes < budget {
            return Err(invalid(
                "shared quota reservation pending; explicit recovery required",
            ));
        }
        return Ok(());
    }
    let reservation = lease.reserve(&store.identity().worktree_id, operation, budget)?;
    c.execute(
        "INSERT INTO projection_budgets VALUES(1,?1,?2,?3,?4,'active')",
        params![operation, reservation, budget, witness],
    )?;
    Ok(())
}
pub(super) fn active_budget(c: &Connection) -> Result<String> {
    Ok(c.query_row(
        "SELECT reservation FROM projection_budgets WHERE singleton=1 AND state='active'",
        [],
        |r| r.get(0),
    )?)
}
pub(super) fn finish_budget(c: &Connection, store: &ProjectionStore) -> Result<()> {
    if store.marker.shared.is_some() {
        c.execute(
            "UPDATE projection_budgets SET state='committed' WHERE singleton=1",
            [],
        )?;
    }
    Ok(())
}
pub(super) fn settle(c: &Connection, store: &ProjectionStore) -> Result<()> {
    let Some(lease) = shared::for_store(store)? else {
        return Ok(());
    };
    let id: Option<String> = c
        .query_row(
            "SELECT reservation FROM projection_budgets WHERE singleton=1 AND state='committed'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = id {
        lease.release_reservation(&id, &store.identity().worktree_id)?;
        c.execute(
            "DELETE FROM projection_budgets WHERE singleton=1 AND state='committed'",
            [],
        )?;
    }
    Ok(())
}
pub(super) fn accept_claim(
    c: &Connection,
    store: &ProjectionStore,
    lease: &shared::Lease,
    kind: &str,
    owner: &str,
    manifests: &[FileManifest],
) -> Result<String> {
    let operation = format!("{kind}:{owner}");
    let (id, digest) = lease.protect(&store.marker, &operation, manifests)?;
    c.execute(
        "INSERT INTO projection_claims VALUES(?1,?2,?3,?4) ON CONFLICT(root_id) DO NOTHING",
        params![id, digest, kind, owner],
    )?;
    for file in manifests {
        c.execute(
            "INSERT INTO projection_content VALUES(?1,?2,?3) ON CONFLICT(file_id) DO NOTHING",
            params![
                file.file_id().as_str(),
                file.id().as_str(),
                file.byte_length()
            ],
        )?;
        let existing: String = c.query_row(
            "SELECT manifest_id FROM projection_content WHERE file_id=?1",
            [file.file_id().as_str()],
            |r| r.get(0),
        )?;
        if existing != file.id().as_str() {
            return Err(invalid("private file representation changed"));
        }
    }
    Ok(id)
}
pub(super) fn transfer(
    c: &Connection,
    store: &ProjectionStore,
    from_kind: &str,
    from_owner: &str,
    kind: &str,
    owner: &str,
) -> Result<()> {
    if store.marker.shared.is_some() {
        c.execute(
            "UPDATE projection_claims SET kind=?1,owner=?2 WHERE kind=?3 AND owner=?4",
            params![kind, owner, from_kind, from_owner],
        )?;
    }
    Ok(())
}
pub(super) fn retain_pinned(
    c: &Connection,
    store: &ProjectionStore,
    pinned: &PinnedFile,
    file: &CapturedFile,
    kind: &str,
    owner: &str,
) -> Result<()> {
    let lease =
        shared::for_store(store)?.ok_or_else(|| invalid("shared content requires format4"))?;
    let mut input = pinned.file();
    input.seek(SeekFrom::Start(0))?;
    if input.metadata()?.len() != file.bytes
        || capture::scan::executable(&input.metadata()?) != file.executable
    {
        return Err(invalid("shared source size or mode changed"));
    }
    let manifest = lease.install_file(&mut input, file, &active_budget(c)?)?;
    pinned.verify_parent()?;
    accept_claim(c, store, &lease, kind, owner, &[manifest])?;
    Ok(())
}
pub(super) fn manifest(
    c: &Connection,
    store: &ProjectionStore,
    file: &CapturedFile,
) -> Result<FileManifest> {
    let id: String = c.query_row(
        "SELECT manifest_id FROM projection_content WHERE file_id=?1 AND bytes=?2",
        params![file.file_id.as_str(), file.bytes],
        |r| r.get(0),
    )?;
    let lease = shared::for_store(store)?
        .ok_or_else(|| invalid("shared representation requires format4"))?;
    let manifest = lease.manifest(&FileManifestId::parse(id).map_err(invalid)?)?;
    if manifest.file_id() != &file.file_id || manifest.byte_length() != file.bytes {
        return Err(invalid("private representation mismatch"));
    }
    lease.verify_manifest(&manifest)?;
    Ok(manifest)
}
pub(super) fn lookup(
    c: &Connection,
    store: &ProjectionStore,
    lease: &shared::Lease,
    file: &FileId,
) -> Result<Option<FileManifest>> {
    let roots = root_metadata(c, store, lease)?;
    lookup_with_roots(c, lease, file, &roots)
}
pub(super) fn lookup_with_roots(
    c: &Connection,
    lease: &shared::Lease,
    file: &FileId,
    roots: &BTreeMap<FileManifestId, FileManifest>,
) -> Result<Option<FileManifest>> {
    let id: Option<String> = c
        .query_row(
            "SELECT manifest_id FROM projection_content WHERE file_id=?1",
            [file.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        if roots.values().any(|m| m.file_id() == file) {
            return Err(invalid("protected file lost its private representation"));
        }
        return Ok(None);
    };
    let Some(manifest) = lease.manifest_for(file)? else {
        if roots.keys().any(|m| m.as_str() == id) {
            return Err(invalid(
                "protected representation missing from shared index",
            ));
        }
        return Ok(None);
    };
    if manifest.id().as_str() != id {
        return Err(invalid("private representation changed after resolution"));
    }
    lease.check_manifest_resident(&manifest)?;
    Ok(Some(manifest))
}
pub(super) fn root_metadata(
    c: &Connection,
    store: &ProjectionStore,
    lease: &shared::Lease,
) -> Result<BTreeMap<FileManifestId, FileManifest>> {
    let mut query = c.prepare("SELECT root_id,digest FROM projection_claims ORDER BY root_id")?;
    let roots = query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut result = BTreeMap::new();
    for (root, digest) in roots {
        for manifest in lease.verify_protection_metadata(&store.marker, &root, &digest)? {
            if result
                .insert(manifest.id().clone(), manifest.clone())
                .is_some_and(|old| old != manifest)
            {
                return Err(invalid("protected representation conflict"));
            }
        }
    }
    Ok(result)
}
pub(super) fn fsck(c: &Connection, store: &ProjectionStore) -> Result<()> {
    let Some(lease) = shared::for_store(store)? else {
        return Ok(());
    };
    let mut query = c.prepare("SELECT root_id,digest FROM projection_claims")?;
    let roots = query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, digest) in roots {
        lease.verify_protection(&store.marker, &id, &digest)?;
    }
    Ok(())
}

/// Includes every final dirty replacement, not just the selected restore/move.
/// Checkout witness and budget are durable before staging or moving any leaf.
pub(super) fn reserve_checkout(
    c: &Connection,
    store: &ProjectionStore,
    operation: &str,
    snapshot: &ProjectionSnapshot,
    changes: &BTreeMap<String, Option<CapturedFile>>,
) -> Result<()> {
    reserve_checkout_against(c, store, operation, snapshot, snapshot, changes)
}
pub(super) fn reserve_checkout_against(
    c: &Connection,
    store: &ProjectionStore,
    operation: &str,
    snapshot: &ProjectionSnapshot,
    resulting_base: &ProjectionSnapshot,
    changes: &BTreeMap<String, Option<CapturedFile>>,
) -> Result<()> {
    if store.marker.shared.is_none() {
        return Ok(());
    }
    let scan = capture::scan::scan(store, c, snapshot, false)?;
    let witness = shared::digest(&scan.files)?;
    let (files, staging) = checkout_requirements(&scan.files, resulting_base, changes)?;
    reserve(c, store, operation, &witness, &files, staging)?;
    if shared::digest(&capture::scan::scan(store, c, snapshot, false)?.files)? != witness {
        return Err(invalid(
            "checkout changed during whole-capture quota preflight",
        ));
    }
    Ok(())
}

pub(super) fn checkout_requirements(
    before: &BTreeMap<String, Option<CapturedFile>>,
    resulting_base: &ProjectionSnapshot,
    changes: &BTreeMap<String, Option<CapturedFile>>,
) -> Result<(Vec<CapturedFile>, u64)> {
    let mut after = before.clone();
    for (path, file) in changes {
        after.insert(path.clone(), file.clone());
    }
    let mut files = Vec::new();
    for (path, file) in &after {
        let expected = resulting_base
            .entries
            .iter()
            .find(|e| e.path.as_str() == path)
            .and_then(|e| match &e.target {
                Some(layrs_core::scm::TreeEntryTarget::File { file_id }) => {
                    Some((file_id, e.executable))
                }
                _ => None,
            });
        if file.as_ref().map(|f| (&f.file_id, f.executable)) != expected {
            if let Some(file) = file {
                files.push(file.clone());
            }
        }
    }
    let mut staging = 0u64;
    for (path, new) in changes {
        if let Some(old) = before.get(path).and_then(Option::as_ref) {
            files.push(old.clone());
            staging = staging
                .checked_add(old.bytes)
                .ok_or_else(|| invalid("preimage budget overflow"))?;
        }
        if let Some(new) = new {
            files.push(new.clone());
            staging = staging
                .checked_add(new.bytes)
                .ok_or_else(|| invalid("staging budget overflow"))?;
        }
    }
    Ok((files, staging))
}

pub(super) fn capacity_available(
    store: &ProjectionStore,
    lease: &shared::Lease,
    files: &[CapturedFile],
    extra: u64,
) -> Result<bool> {
    let mut missing = Vec::new();
    for file in files {
        if let Some(manifest) = lease.manifest_for(&file.file_id)? {
            lease.check_manifest_resident(&manifest)?;
        } else {
            missing.push(file.clone());
        }
    }
    let need = budget_for(&missing, extra)?;
    let used = lease.usage()?;
    Ok(used
        .content_bytes
        .checked_add(used.reserved_bytes)
        .and_then(|n| n.checked_add(need))
        .is_some_and(|n| n <= used.limit_bytes)
        && fs2::available_space(&store.root)? >= extra.saturating_add(65536)
        && fs2::available_space(lease.family_root())? >= used.reserved_bytes.saturating_add(need))
}

impl ProjectionStore {
    /// Retire old generation claims after atomically accepting exact required
    /// manifest subsets for the current base and retained capture bases. Existing
    /// capture, preimage, operation-history and preparation claims are untouched.
    pub fn release_generation_retention(&self, generation: u64) -> Result<u64> {
        if !self.is_shared() {
            return Err(invalid("generation retention requires format4"));
        }
        let mut access = format::access(self, SessionMode::Exclusive)?;
        let c = &access.connection;
        database::require_mutable(c)?;
        let version = offline::snapshot_at(c, generation, None)?;
        if database::current(c)?.generation == generation {
            return Err(invalid(
                "generation is the active base; retain it or advance the Worktree first",
            ));
        }
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing release lease"))?;
        let mut q = c.prepare(
            "SELECT root_id,digest FROM projection_claims WHERE kind='generation' AND owner=?1",
        )?;
        let claims = q
            .query_map([&version.digest], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        let mut exact = BTreeMap::new();
        for (id, digest) in &claims {
            for manifest in lease.verify_protection_metadata(&self.marker, id, digest)? {
                exact.insert(manifest.id().clone(), manifest);
            }
        }
        let mut q = c.prepare("SELECT DISTINCT projection_generation FROM projection_captures")?;
        let dependencies = q
            .query_map([], |r| r.get::<_, u64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        let mut historical_ids = BTreeSet::new();
        for dependency in dependencies {
            historical_ids.extend(
                offline::snapshot_at(c, dependency, None)?
                    .snapshot
                    .entries
                    .into_iter()
                    .filter_map(|e| match e.target {
                        Some(layrs_core::scm::TreeEntryTarget::File { file_id }) => Some(file_id),
                        _ => None,
                    }),
            );
        }
        let current = database::current(c)?;
        let active_ids = current
            .snapshot
            .entries
            .iter()
            .filter_map(|e| match &e.target {
                Some(layrs_core::scm::TreeEntryTarget::File { file_id }) => Some(file_id.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let active = exact
            .values()
            .filter(|m| active_ids.contains(m.file_id()))
            .cloned()
            .collect::<Vec<_>>();
        let historical = exact
            .values()
            .filter(|m| historical_ids.contains(m.file_id()))
            .cloned()
            .collect::<Vec<_>>();
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if !active.is_empty() {
            accept_claim(&tx, self, lease, "generation", &current.digest, &active)?;
        }
        if !historical.is_empty() {
            accept_claim(
                &tx,
                self,
                lease,
                "capture-base",
                &version.digest,
                &historical,
            )?;
        }
        let removed = tx.execute(
            "DELETE FROM projection_claims WHERE kind='generation' AND owner=?1",
            [&version.digest],
        )? as u64;
        tx.commit()?;
        access.verify()?;
        Ok(removed)
    }
    /// Cancel a journal-free operation using exact recorded staging ownership.
    /// Accepted roots and user checkout files are never removed by cancellation.
    pub fn cancel_shared_reservation(&self) -> Result<()> {
        if self.marker.shared.is_none() {
            return Err(invalid("reservation recovery requires format4"));
        }
        cancel_with_access(self, format::recovery_access(self, None, false)?)
    }
}

pub(super) fn cancel_with_access(store: &ProjectionStore, access: format::Access) -> Result<()> {
    database::require_mutable(&access.connection)?;
    if store.marker.shared.is_none() {
        return Err(invalid("reservation recovery requires format4"));
    }
    staging::cancel(&access.connection, store)?;
    let active: Option<(String, String)> = access
        .connection
        .query_row(
            "SELECT operation,reservation FROM projection_budgets WHERE state='active'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((operation, reservation)) = active {
        let _ = operation;
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing cancellation lease"))?;
        lease.cancel_installations(&reservation, &store.identity().worktree_id)?;
        access.connection.execute(
            "DELETE FROM projection_budgets WHERE singleton=1 AND reservation=?1",
            [&reservation],
        )?;
        lease.release_reservation(&reservation, &store.identity().worktree_id)?;
    }
    settle(&access.connection, store)?;
    access.verify()
}
