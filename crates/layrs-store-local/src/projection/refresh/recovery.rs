use super::*;
use rusqlite::{OptionalExtension, params};

pub(super) fn resume(store: &ProjectionStore) -> Result<VersionedProjection> {
    let mut access = format::access(store, SessionMode::Exclusive)?;
    let length: Option<u64> = access
        .connection
        .query_row(
            "SELECT length(payload) FROM projection_refresh_pending WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(length) = length else {
        database::require_idle(&access.connection)?;
        access.verify()?;
        return database::current(&access.connection);
    };
    if length > 64 * 1024 * 1024 {
        return Err(invalid("refresh journal exceeds64MiB"));
    }
    let bytes: Vec<u8> = access.connection.query_row(
        "SELECT payload FROM projection_refresh_pending WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(invalid)?;
    if encode(&journal)? != bytes {
        return Err(invalid("refresh journal is not canonical"));
    }
    let current = validate(&access, &journal)?;
    if !journal.ready {
        // Only private staging may exist. No checkout operation precedes Ready.
        // Incomplete staged bytes are retained privately; retry supplies bytes again.
        access.verify()?;
        access.connection.execute(
            "DELETE FROM projection_refresh_pending WHERE singleton=1",
            [],
        )?;
        return Ok(current);
    }
    finish(store, &mut access, &journal, &NoRefreshFaults)
}

fn validate(access: &format::Access, journal: &Journal) -> Result<VersionedProjection> {
    let current = database::current(&access.connection)?;
    validate_snapshot(&current, journal)?;
    let other: bool = access.connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_publication_pending) OR EXISTS(SELECT 1 FROM projection_restore_pending)", [], |r| r.get(0))?;
    if other {
        return Err(invalid("another local operation blocks refresh recovery"));
    }
    Ok(current)
}

pub(super) fn validate_snapshot(current: &VersionedProjection, journal: &Journal) -> Result<()> {
    let suffix = journal.directory.strip_prefix("refresh-").unwrap_or("");
    if journal.schema_version != "layrs.projection-refresh.v1"
        || journal.expected_generation != current.generation
        || journal.expected_digest != current.digest
        || suffix.is_empty()
        || journal.directory.len() > 96
        || !suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
    {
        return Err(invalid("refresh journal identity or generation mismatch"));
    }
    if plan::normalize(&current.snapshot, &journal.target)? != journal.target {
        return Err(invalid("refresh journal target is not normalized"));
    }
    let old: Vec<_> = current
        .snapshot
        .entries
        .iter()
        .filter(|e| plan::materialized_file(e))
        .collect();
    if old.len() != journal.old.len() {
        return Err(invalid("refresh old files mismatch"));
    }
    let mut total = 0u64;
    for (index, (entry, record)) in old.iter().zip(&journal.old).enumerate() {
        if record.path != entry.path.as_str()
            || record.name != format!("old-{index:08}")
            || entry.target != Some(TreeEntryTarget::file(record.file_id.clone()))
            || record.executable != entry.executable
            || record.bytes > MAX_FILE
            || record.identity.is_empty()
        {
            return Err(invalid("refresh old file journal mismatch"));
        }
        total = total
            .checked_add(record.bytes)
            .filter(|n| *n <= MAX_TOTAL)
            .ok_or_else(|| invalid("refresh old files exceed4GiB"))?;
    }
    if journal.ready {
        let (_, install) = plan::operations(&current.snapshot, &journal.target);
        if install.len() != journal.new.len() || journal.directory_identity.is_none() {
            return Err(invalid("refresh staged files mismatch"));
        }
        let target: BTreeMap<_, _> = journal
            .target
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e))
            .collect();
        let mut total = 0u64;
        for (index, (path, record)) in install.iter().zip(&journal.new).enumerate() {
            let entry = target[path.as_str()];
            if record.path != *path
                || record.name != format!("new-{index:08}")
                || record.bytes > MAX_FILE
                || entry.target != Some(TreeEntryTarget::file(record.file_id.clone()))
                || entry.executable != record.executable
                || record.identity.is_empty()
            {
                return Err(invalid("refresh new file journal mismatch"));
            }
            total = total
                .checked_add(record.bytes)
                .filter(|n| *n <= MAX_TOTAL)
                .ok_or_else(|| invalid("refresh staged files exceed4GiB"))?;
        }
    } else if !journal.new.is_empty() || journal.directory_identity.is_some() {
        return Err(invalid("incomplete refresh intent changed"));
    }
    Ok(())
}

pub(super) fn finish(
    store: &ProjectionStore,
    access: &mut format::Access,
    journal: &Journal,
    faults: &dyn RefreshFaults,
) -> Result<VersionedProjection> {
    let current = validate(access, journal)?;
    if !journal.ready {
        return Err(invalid("refresh staging incomplete"));
    }
    let root = PinnedDirectory::open(&store.root)?;
    let parent = PinnedDirectory::open(&store.root.join(".layrs"))?
        .ensure_private_child("projection-quarantine".as_ref())?;
    let directory = parent.ensure_private_child(journal.directory.as_ref())?;
    if Some(directory.identity_token()) != journal.directory_identity {
        return Err(invalid("refresh quarantine directory replaced"));
    }
    let directory_path = store
        .root
        .join(".layrs/projection-quarantine")
        .join(&journal.directory);
    let (remove, _) = plan::operations(&current.snapshot, &journal.target);
    let removed = remove.iter().map(String::as_str).collect();
    // Validate the entire surviving source/stage set before each recovery phase.
    for record in &journal.old {
        if remove.contains(&record.path) && files::present(&directory_path.join(&record.name))? {
            files::read_verify(&directory, &record.name, record)?;
        } else {
            files::read_verify(&root, &record.path, record)?;
        }
    }
    for record in &journal.new {
        if files::present(&directory_path.join(&record.name))? {
            files::read_verify(&directory, &record.name, record)?;
            files::destination(store, &record.path, &removed)?;
        } else {
            files::read_verify(&root, &record.path, record)?;
        }
    }
    for record in journal.old.iter().filter(|r| remove.contains(&r.path)) {
        if !files::present(&directory_path.join(&record.name))? {
            let source = files::read_verify(&root, &record.path, record)?;
            let (source_parent, _) = root.existing_parent(Path::new(&record.path))?;
            if source.rename_to(&directory, record.name.as_ref())? {
                return Err(invalid("refresh quarantine collision; no overwrite"));
            }
            source_parent.sync()?;
            directory.sync()?;
        }
        files::read_verify(&directory, &record.name, record)?;
        root.verify_path_identity()?;
        access.verify()?;
        faults.check(RefreshPoint::PathQuarantined)?;
    }
    for record in &journal.new {
        if files::present(&directory_path.join(&record.name))? {
            install::files::destination_absent(&root, &store.root, &record.path)?;
            let source = files::read_verify(&directory, &record.name, record)?;
            let (parent, name) = root.components_to_parent(Path::new(&record.path))?;
            if source.rename_to(&parent, &name)? {
                return Err(invalid("refresh destination appeared; no overwrite"));
            }
            parent.sync()?;
            directory.sync()?;
        }
        files::read_verify(&root, &record.path, record)?;
        root.verify_path_identity()?;
        access.verify()?;
        faults.check(RefreshPoint::PathInstalled)?;
    }
    faults.check(RefreshPoint::BeforeCommit)?;
    for record in &journal.old {
        if remove.contains(&record.path) {
            files::read_verify(&directory, &record.name, record)?;
        } else {
            files::read_verify(&root, &record.path, record)?;
        }
    }
    for record in &journal.new {
        files::read_verify(&root, &record.path, record)?;
    }
    // A revoked/deleted leaf must not have reappeared while other files moved.
    for path in &remove {
        if !journal
            .new
            .iter()
            .any(|r| r.path == *path || r.path.starts_with(&format!("{path}/")))
            && files::present(&store.root.join(path))?
        {
            return Err(invalid("revoked path reappeared during refresh"));
        }
    }
    access.verify()?;
    let generation = current
        .generation
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("projection generation exhausted"))?;
    let bytes = journal.target.canonical_bytes()?;
    let digest = database::digest(generation, Some(&current.digest), &bytes);
    let tx = access
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
        params![generation, digest, current.digest, bytes],
    )?;
    tx.execute(
        "UPDATE projection_head SET generation=?1 WHERE singleton=1",
        [generation],
    )?;
    tx.execute(
        "INSERT INTO projection_refresh_history VALUES(?1,?2)",
        params![generation, encode(journal)?],
    )?;
    tx.execute(
        "DELETE FROM projection_refresh_pending WHERE singleton=1",
        [],
    )?;
    tx.commit()?;
    faults.check(RefreshPoint::AfterCommit)?;
    access.verify()?;
    Ok(VersionedProjection {
        generation,
        digest,
        snapshot: journal.target.clone(),
    })
}
