use super::*;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Journal {
    schema_version: String,
    pub(super) preview: RestorePreview,
    directory: String,
    directory_identity: String,
    old_identity: BTreeMap<RepoPath, String>,
    new_identity: BTreeMap<RepoPath, String>,
}
fn encode(journal: &Journal) -> Result<Vec<u8>> {
    let bytes = canonical_json(journal).map_err(invalid)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(invalid("restore journal exceeds8MiB"));
    }
    Ok(bytes)
}
fn decode(bytes: &[u8]) -> Result<Journal> {
    let journal: Journal = serde_json::from_slice(bytes).map_err(invalid)?;
    if encode(&journal)? != bytes
        || journal.schema_version != "layrs.projection-restore.v1"
        || journal.directory.len() > 200
        || !journal
            .directory
            .strip_prefix("restore-")
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            })
        || preview_digest(&journal.preview)? != journal.preview.digest
        || journal
            .preview
            .before
            .keys()
            .ne(journal.preview.after.keys())
    {
        return Err(invalid("restore journal identity mismatch"));
    }
    for (files, identities) in [
        (&journal.preview.before, &journal.old_identity),
        (&journal.preview.after, &journal.new_identity),
    ] {
        if files
            .iter()
            .filter(|(_, f)| f.is_some())
            .map(|(p, _)| p)
            .ne(identities.keys())
            || identities.values().any(String::is_empty)
        {
            return Err(invalid("restore file witnesses mismatch"));
        }
    }
    Ok(journal)
}
pub(super) fn apply(
    store: &ProjectionStore,
    expected: &RestorePreview,
    faults: &dyn RestoreFaults,
) -> Result<RestoreReceipt> {
    if preview_digest(expected)? != expected.digest {
        return Err(invalid("restore preview digest mismatch"));
    }
    let mut access = format::access(store, SessionMode::Exclusive)?;
    database::require_mutable(&access.connection)?;
    if let Some((journal, capture)) = history(&access.connection, store, &expected.operation_id)? {
        if journal.preview != *expected {
            return Err(invalid("restore replay differs"));
        }
        access.verify()?;
        return Ok(RestoreReceipt {
            operation_id: expected.operation_id.clone(),
            capture,
        });
    }
    let state = database::current(&access.connection)?;
    let mut actual = preview(store, &access.connection, &state, &expected.source)?;
    actual.operation_id = expected.operation_id.clone();
    actual.digest = preview_digest(&actual)?;
    if actual != *expected {
        return Err(invalid("restore preview is stale"));
    }
    let root = PinnedDirectory::open(&store.root)?;
    let parent = root
        .open_child(".layrs".as_ref())?
        .ensure_private_child("projection-quarantine".as_ref())?;
    // An interrupted pre-journal staging attempt may leave private files. Each
    // retry uses its own directory and never deletes or adopts that attempt.
    let directory_name = format!(
        "restore-{}-{}",
        expected.operation_id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(invalid)?
            .as_nanos()
    );
    let directory = parent.ensure_private_child(directory_name.as_ref())?;
    let mut journal = Journal {
        schema_version: "layrs.projection-restore.v1".into(),
        preview: expected.clone(),
        directory: directory_name,
        directory_identity: directory.identity_token(),
        old_identity: BTreeMap::new(),
        new_identity: BTreeMap::new(),
    };
    let mut old_bytes = BTreeMap::new();
    for (index, (path, old)) in expected.before.iter().enumerate() {
        if let Some(old) = old {
            let pin = root.open_relative_file(Path::new(path.as_str()))?;
            let identity = file::identity(pin.file())?;
            old_bytes.insert(
                old.file_id.clone(),
                files::bytes(&root, path.as_str(), old, &identity)?,
            );
            journal.old_identity.insert(path.clone(), identity);
        }
        if let Some(new) = &expected.after[path] {
            let data = capture::storage::file_bytes(&access.connection, new)?;
            let mut staged = directory.create_file(format!("new-{index:04}").as_ref())?;
            staged.file_mut().write_all(&data)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                staged
                    .file()
                    .set_permissions(std::fs::Permissions::from_mode(if new.executable {
                        0o700
                    } else {
                        0o600
                    }))?;
            }
            staged.file().sync_all()?;
            journal
                .new_identity
                .insert(path.clone(), file::identity(staged.file())?);
        }
    }
    directory.sync()?;
    parent.sync()?;
    faults.check(RestorePoint::Staged)?;
    if checkout_digest(&capture::scan::scan(store, &state.snapshot, false)?.files)?
        != expected.checkout_digest
    {
        return Err(invalid("checkout changed during restore staging"));
    }
    access.verify()?;
    let tx = access
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (id, bytes) in old_bytes {
        tx.execute(
            "INSERT INTO projection_capture_files VALUES(?1,?2) ON CONFLICT(file_id) DO NOTHING",
            params![id.as_str(), bytes],
        )?;
    }
    for old in expected.before.values().flatten() {
        capture::storage::file_bytes(&tx, old)?;
    }
    tx.execute(
        "INSERT INTO projection_restore_pending VALUES(1,?1)",
        [encode(&journal)?],
    )?;
    tx.commit()?;
    faults.check(RestorePoint::JournalDurable)?;
    finish(store, &mut access, &journal, faults)
}
pub(super) fn resume(store: &ProjectionStore) -> Result<RestoreReceipt> {
    let mut access = format::access(store, SessionMode::Exclusive)?;
    let length: u64 = access.connection.query_row(
        "SELECT length(payload) FROM projection_restore_pending WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if length > 8 * 1024 * 1024 {
        return Err(invalid("restore journal exceeds8MiB"));
    }
    let bytes: Vec<u8> = access.connection.query_row(
        "SELECT payload FROM projection_restore_pending WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    finish(store, &mut access, &decode(&bytes)?, &NoRestoreFaults)
}
fn finish(
    store: &ProjectionStore,
    access: &mut format::Access,
    journal: &Journal,
    faults: &dyn RestoreFaults,
) -> Result<RestoreReceipt> {
    let conflicts: bool = access.connection.query_row("SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_publication_pending)", [], |r| r.get(0))?;
    if conflicts {
        return Err(invalid("another operation blocks restore recovery"));
    }
    let state = database::current(&access.connection)?;
    let expected = &journal.preview;
    decode(&encode(journal)?)?;
    if state.generation != expected.projection_generation
        || state.digest != expected.projection_digest
        || targets(&access.connection, store, &expected.source)? != expected.after
    {
        return Err(invalid("restore base or source changed"));
    }
    validate_paths(&state.snapshot, &expected.before)?;
    validate_paths(&state.snapshot, &expected.after)?;
    let root = PinnedDirectory::open(&store.root)?;
    let directory_path = store
        .root
        .join(".layrs/projection-quarantine")
        .join(&journal.directory);
    let directory = PinnedDirectory::open(&directory_path)?;
    if directory.identity_token() != journal.directory_identity {
        return Err(invalid("restore quarantine replaced"));
    }
    // Validate all old bytes and replacement stages before touching any path.
    for (index, (path, old)) in expected.before.iter().enumerate() {
        let old_name = format!("old-{index:04}");
        let new_name = format!("new-{index:04}");
        if let Some(old) = old {
            let (base, name) = if files::present(&directory_path.join(&old_name))? {
                (&directory, old_name.as_str())
            } else {
                (&root, path.as_str())
            };
            files::verified(base, name, old, &journal.old_identity[path])?;
        }
        if let Some(new) = &expected.after[path] {
            let (base, name) = if files::present(&directory_path.join(&new_name))? {
                (&directory, new_name.as_str())
            } else {
                (&root, path.as_str())
            };
            files::verified(base, name, new, &journal.new_identity[path])?;
        }
    }
    for (index, (path, old)) in expected.before.iter().enumerate() {
        let old_name = format!("old-{index:04}");
        if let Some(old) = old {
            if !files::present(&directory_path.join(&old_name))? {
                let pin = files::verified(&root, path.as_str(), old, &journal.old_identity[path])?;
                if pin.rename_to(&directory, old_name.as_ref())? {
                    return Err(invalid("restore quarantine collision"));
                }
                pin.sync_parent()?;
                directory.sync()?;
            }
            files::verified(&directory, &old_name, old, &journal.old_identity[path])?;
            access.verify()?;
            faults.check(RestorePoint::PathQuarantined)?;
        }
    }
    for (index, (path, new)) in expected.after.iter().enumerate() {
        let new_name = format!("new-{index:04}");
        if let Some(new) = new {
            if files::present(&directory_path.join(&new_name))? {
                install::files::destination_absent(&root, &store.root, path.as_str())?;
                let pin = files::verified(&directory, &new_name, new, &journal.new_identity[path])?;
                let (parent, name) = root.components_to_parent(Path::new(path.as_str()))?;
                if pin.rename_to(&parent, &name)? {
                    return Err(invalid("restore destination collision"));
                }
                directory.sync()?;
                parent.sync()?;
            }
            files::verified(&root, path.as_str(), new, &journal.new_identity[path])?;
        } else if files::present(&store.root.join(path.as_str()))? {
            return Err(invalid("restored deletion reappeared"));
        }
        access.verify()?;
        faults.check(RestorePoint::PathInstalled)?;
    }
    let scan = capture::scan::scan(store, &state.snapshot, true)?;
    if checkout_digest(&scan.files)? != expected.after_checkout_digest {
        return Err(invalid("checkout changed during restoration"));
    }
    access.verify()?;
    let tx = access
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let capture = capture::storage::persist_in_transaction(&tx, store, &state, scan)?;
    tx.execute(
        "INSERT INTO projection_restore_history VALUES(?1,?2,?3)",
        params![expected.operation_id, capture.sequence, encode(journal)?],
    )?;
    tx.execute(
        "DELETE FROM projection_restore_pending WHERE singleton=1",
        [],
    )?;
    faults.check(RestorePoint::BeforeCommit)?;
    tx.commit()?;
    faults.check(RestorePoint::AfterCommit)?;
    access.verify()?;
    Ok(RestoreReceipt {
        operation_id: expected.operation_id.clone(),
        capture,
    })
}
pub(super) fn history(
    c: &Connection,
    store: &ProjectionStore,
    id: &str,
) -> Result<Option<(Journal, CapturedPatch)>> {
    let length: Option<u64> = c
        .query_row(
            "SELECT length(payload) FROM projection_restore_history WHERE operation_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(length) = length else {
        return Ok(None);
    };
    if length > 8 * 1024 * 1024 {
        return Err(invalid("restore history exceeds8MiB"));
    }
    let (sequence, bytes): (u64, Vec<u8>) = c.query_row(
        "SELECT capture_sequence,payload FROM projection_restore_history WHERE operation_id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let journal = decode(&bytes)?;
    let capture = capture::storage::load(c, store, sequence)?;
    if journal.preview.operation_id != id
        || capture.projection_generation != journal.preview.projection_generation
        || capture.projection_digest != journal.preview.projection_digest
    {
        return Err(invalid("restore history capture mismatch"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_snapshots WHERE generation=?1",
        [capture.projection_generation],
        |r| r.get(0),
    )?;
    let base: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
    validate_paths(&base, &journal.preview.before)?;
    validate_paths(&base, &journal.preview.after)?;
    for (path, expected) in &journal.preview.after {
        let actual = if let Some(change) = capture.changes.iter().find(|ch| &ch.path == path) {
            change
                .replacement
                .as_ref()
                .map(|f| (&f.file_id, f.executable))
        } else {
            let entry = base
                .entries
                .iter()
                .find(|e| &e.path == path)
                .ok_or_else(|| invalid("restore path vanished"))?;
            match &entry.target {
                Some(layrs_core::scm::TreeEntryTarget::File { file_id }) => {
                    Some((file_id, entry.executable))
                }
                _ => None,
            }
        };
        if actual != expected.as_ref().map(|f| (&f.file_id, f.executable)) {
            return Err(invalid("restore capture result mismatch"));
        }
    }
    for file in journal
        .preview
        .before
        .values()
        .chain(journal.preview.after.values())
        .flatten()
    {
        capture::storage::file_bytes(c, file)?;
    }
    Ok(Some((journal, capture)))
}
pub(in crate::projection) fn verify_history(
    c: &Connection,
    store: &ProjectionStore,
) -> Result<u64> {
    let mut statement =
        c.prepare("SELECT operation_id FROM projection_restore_history ORDER BY operation_id")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        history(c, store, &id)?.ok_or_else(|| invalid("restore history missing"))?;
        count += 1;
    }
    Ok(count)
}
