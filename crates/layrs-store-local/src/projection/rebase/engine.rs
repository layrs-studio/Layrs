use super::*;
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use rusqlite::{OptionalExtension, params};
use std::io::Read;

fn encode(journal: &Journal) -> Result<Vec<u8>> {
    let bytes = canonical_json(journal).map_err(invalid)?;
    if bytes.len() > MAX_JOURNAL {
        return Err(invalid("rebase journal exceeds64MiB"));
    }
    Ok(bytes)
}
pub(super) fn decode(bytes: &[u8]) -> Result<Journal> {
    let journal: Journal = serde_json::from_slice(bytes).map_err(invalid)?;
    if encode(&journal)? != bytes || journal.schema_version != JOURNAL_SCHEMA {
        return Err(invalid("rebase journal is not canonical"));
    }
    Ok(journal)
}
pub(super) fn apply(
    store: &ProjectionStore,
    expected: &RebasePreview,
    write_file: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
    faults: &dyn RebaseFaults,
) -> Result<CapturedPatch> {
    if expected.digest != expected.computed_digest()? {
        return Err(invalid("rebase preview digest mismatch"));
    }
    let mut access = format::access(store, SessionMode::Exclusive)?;
    database::require_mutable(&access.connection)?;
    if let Some(capture) = history::completed(&access.connection, store, expected)? {
        return Ok(capture);
    }
    let state = database::current(&access.connection)?;
    if state.generation != expected.projection_generation
        || state.digest != expected.projection_digest
    {
        return Err(invalid("rebase projection changed"));
    }
    let mut actual = preview(
        store,
        &access.connection,
        &state,
        expected.capture_sequence,
        &expected.target,
        &expected.resolutions,
    )?;
    actual.operation_id = expected.operation_id.clone();
    actual.digest = actual.computed_digest()?;
    if actual != *expected {
        return Err(invalid("rebase preview is stale"));
    }
    let root = PinnedDirectory::open(&store.root)?;
    let directory_name = format!(
        "rebase-{}-{}-{}",
        &expected.digest[7..23],
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(invalid)?
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    let mut journal = Journal {
        schema_version: JOURNAL_SCHEMA.into(),
        preview: expected.clone(),
        directory: directory_name,
        directory_identity: None,
        ready: false,
        old: Vec::new(),
        new: Vec::new(),
    };
    for (path, before) in &expected.before {
        if before == &expected.resolutions[path] {
            continue;
        }
        if let Some(file) = before {
            let pin = root.open_relative_file(Path::new(path.as_str()))?;
            let identity = file::identity(pin.file())?;
            let record = FileRecord {
                path: path.clone(),
                file: file.clone(),
                identity,
                name: format!("old-{:08}", journal.old.len()),
            };
            verify_pin(&pin, &record)?;
            journal.old.push(record);
        } else if expected.resolutions[path].is_some() {
            install::files::destination_absent(&root, &store.root, path.as_str())?;
        }
    }
    root.verify_path_identity()?;
    access.verify()?;
    access.connection.execute(
        "INSERT INTO projection_rebase_pending VALUES(1,?1)",
        [encode(&journal)?],
    )?;
    faults.check(RebasePoint::IntentDurable)?;
    let parent = root
        .open_child(".layrs".as_ref())?
        .ensure_private_child("projection-quarantine".as_ref())?;
    let directory = parent.ensure_private_child(journal.directory.as_ref())?;
    journal.directory_identity = Some(directory.identity_token());
    let mut total = 0u64;
    for (path, resolved) in &expected.resolutions {
        if expected.before[path] == *resolved {
            continue;
        }
        if let Some(file) = resolved {
            let name = format!("new-{:08}", journal.new.len());
            let mut staged = directory.create_file(name.as_ref())?;
            let mut writer = install::files::FileWriter::new(staged.file_mut(), file.bytes);
            write_file(path.as_str(), &mut writer)?;
            let bytes = writer.finish(&file.file_id)?;
            if bytes != file.bytes {
                return Err(invalid("rebase staged file length differs"));
            }
            total = total
                .checked_add(bytes)
                .filter(|n| *n <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("rebase staged bytes exceed4GiB"))?;
            install::files::set_mode(staged.file(), file.executable)?;
            staged.file().sync_all()?;
            journal.new.push(FileRecord {
                path: path.clone(),
                file: file.clone(),
                identity: file::identity(staged.file())?,
                name,
            });
        }
    }
    directory.sync()?;
    parent.sync()?;
    faults.check(RebasePoint::Staged)?;
    let scan = capture::scan::scan(store, &state.snapshot, false)?;
    if checkout_digest(&scan.files)? != expected.checkout_digest
        || scan.changes
            != capture::storage::load(&access.connection, store, expected.capture_sequence)?.changes
    {
        return Err(invalid("checkout changed while staging rebase"));
    }
    journal.ready = true;
    access.verify()?;
    access.connection.execute(
        "UPDATE projection_rebase_pending SET payload=?1 WHERE singleton=1",
        [encode(&journal)?],
    )?;
    faults.check(RebasePoint::JournalDurable)?;
    finish(store, &mut access, &journal, faults)
}

pub(super) fn resume(store: &ProjectionStore) -> Result<CapturedPatch> {
    let mut access = format::access(store, SessionMode::Exclusive)?;
    let conflicts: bool = access.connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_restore_pending) OR EXISTS(SELECT 1 FROM projection_publication_pending)", [], |r| r.get(0))?;
    if conflicts {
        return Err(invalid(
            "another projection operation blocks rebase recovery",
        ));
    }
    let length: Option<u64> = access
        .connection
        .query_row(
            "SELECT length(payload) FROM projection_rebase_pending WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let length = length.ok_or_else(|| invalid("no rebase operation is pending"))?;
    if length > MAX_JOURNAL as u64 {
        return Err(invalid("rebase journal exceeds64MiB"));
    }
    let bytes: Vec<u8> = access.connection.query_row(
        "SELECT payload FROM projection_rebase_pending WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let journal = decode(&bytes)?;
    let current = database::current(&access.connection)?;
    validate_journal(&access.connection, store, &current, &journal)?;
    if !journal.ready {
        let scan = capture::scan::scan(store, &current.snapshot, false)?;
        if checkout_digest(&scan.files)? != journal.preview.checkout_digest {
            return Err(invalid("incomplete rebase staging changed checkout"));
        }
        let root = PinnedDirectory::open(&store.root)?;
        for old in &journal.old {
            verify_path(&root, old.path.as_str(), old)?;
        }
        access.verify()?;
        access.connection.execute(
            "DELETE FROM projection_rebase_pending WHERE singleton=1",
            [],
        )?;
        return Err(invalid(
            "rebase staging incomplete; retry with prepared bytes",
        ));
    }
    finish(store, &mut access, &journal, &NoRebaseFaults)
}

fn finish(
    store: &ProjectionStore,
    access: &mut format::Access,
    journal: &Journal,
    faults: &dyn RebaseFaults,
) -> Result<CapturedPatch> {
    let current = database::current(&access.connection)?;
    validate_journal(&access.connection, store, &current, journal)?;
    if !journal.ready {
        return Err(invalid("rebase staging incomplete"));
    }
    let root = PinnedDirectory::open(&store.root)?;
    let parent = root
        .open_child(".layrs".as_ref())?
        .open_child("projection-quarantine".as_ref())?;
    let directory = parent.open_child(journal.directory.as_ref())?;
    if Some(directory.identity_token()) != journal.directory_identity {
        return Err(invalid("rebase quarantine directory replaced"));
    }
    let directory_path = store
        .root
        .join(".layrs/projection-quarantine")
        .join(&journal.directory);
    for record in &journal.old {
        if present(&directory_path.join(&record.name))? {
            verify_path(&directory, &record.name, record)?;
        } else {
            verify_path(&root, record.path.as_str(), record)?;
        }
    }
    for record in &journal.new {
        if present(&directory_path.join(&record.name))? {
            verify_path(&directory, &record.name, record)?;
            if !journal.old.iter().any(|old| old.path == record.path) {
                install::files::destination_absent(&root, &store.root, record.path.as_str())?;
            }
        } else {
            verify_path(&root, record.path.as_str(), record)?;
        }
    }
    for record in &journal.old {
        if !present(&directory_path.join(&record.name))? {
            let pin = verify_path(&root, record.path.as_str(), record)?;
            if pin.rename_to(&directory, record.name.as_ref())? {
                return Err(invalid("rebase quarantine collision; no overwrite"));
            }
            root.verify_path_identity()?;
            directory.sync()?;
        }
        verify_path(&directory, &record.name, record)?;
        access.verify()?;
        faults.check(RebasePoint::PathQuarantined)?;
    }
    for record in &journal.new {
        if present(&directory_path.join(&record.name))? {
            install::files::destination_absent(&root, &store.root, record.path.as_str())?;
            let pin = verify_path(&directory, &record.name, record)?;
            let (target_parent, target_name) =
                root.components_to_parent(Path::new(record.path.as_str()))?;
            if pin.rename_to(&target_parent, &target_name)? {
                return Err(invalid("rebase destination collision; no overwrite"));
            }
            target_parent.sync()?;
            directory.sync()?;
        }
        verify_path(&root, record.path.as_str(), record)?;
        access.verify()?;
        faults.check(RebasePoint::PathInstalled)?;
    }
    for record in &journal.old {
        verify_path(&directory, &record.name, record)?;
    }
    for record in &journal.new {
        verify_path(&root, record.path.as_str(), record)?;
    }
    for (path, old) in &journal.preview.before {
        if old.is_some()
            && journal.preview.resolutions[path].is_none()
            && present(&store.root.join(path.as_str()))?
        {
            return Err(invalid("rebase deleted path reappeared"));
        }
    }
    let first = capture::scan::scan(store, &journal.preview.target, false)?;
    if checkout_digest(&first.files)? != journal.preview.after_checkout_digest {
        return Err(invalid("rebase resulting checkout differs from preview"));
    }
    let second = capture::scan::scan(store, &journal.preview.target, true)?;
    if first.files != second.files
        || checkout_digest(&second.files)? != journal.preview.after_checkout_digest
    {
        return Err(invalid("rebase checkout changed during finalization"));
    }
    access.verify()?;
    let generation = current
        .generation
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("rebase generation exhausted"))?;
    let target_bytes = journal.preview.target.canonical_bytes()?;
    let digest = database::digest(generation, Some(&current.digest), &target_bytes);
    let state = VersionedProjection {
        generation,
        digest: digest.clone(),
        snapshot: journal.preview.target.clone(),
    };
    let tx = access
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for old in &journal.old {
        let pin = verify_path(&directory, &old.name, old)?;
        capture::storage::persist_pinned_file(&tx, &pin, &old.file)?;
    }
    tx.execute(
        "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
        params![generation, digest, current.digest, target_bytes],
    )?;
    tx.execute(
        "UPDATE projection_head SET generation=?1 WHERE singleton=1",
        [generation],
    )?;
    let captured = capture::storage::persist_in_transaction(&tx, store, &state, second)?;
    for old in &journal.old {
        verify_path(&directory, &old.name, old)?;
    }
    for new in &journal.new {
        verify_path(&root, new.path.as_str(), new)?;
    }
    if checkout_digest(&capture::scan::scan(store, &state.snapshot, false)?.files)?
        != journal.preview.after_checkout_digest
    {
        return Err(invalid("rebase checkout changed before commit"));
    }
    tx.execute(
        "INSERT INTO projection_rebase_history VALUES(?1,?2,?3,?4)",
        params![
            journal.preview.operation_id,
            generation,
            captured.sequence,
            encode(journal)?
        ],
    )?;
    tx.execute(
        "DELETE FROM projection_rebase_pending WHERE singleton=1",
        [],
    )?;
    faults.check(RebasePoint::BeforeCommit)?;
    // The final callback models an editor racing the durable commit. Recheck
    // installed identities as well as bytes; equal-content replacements still
    // belong to that editor and must not be adopted by this operation.
    for old in &journal.old {
        verify_path(&directory, &old.name, old)?;
    }
    for new in &journal.new {
        verify_path(&root, new.path.as_str(), new)?;
    }
    for (path, value) in &journal.preview.resolutions {
        if value.is_none() {
            install::files::destination_absent(&root, &store.root, path.as_str())?;
        }
    }
    if checkout_digest(&capture::scan::scan(store, &state.snapshot, false)?.files)?
        != journal.preview.after_checkout_digest
    {
        return Err(invalid("rebase checkout changed at commit boundary"));
    }
    root.verify_path_identity()?;
    format::verify_boundaries(
        &store.root,
        &store.marker,
        &access.metadata,
        &access.database,
    )?;
    tx.commit()?;
    faults.check(RebasePoint::AfterCommit)?;
    access.verify()?;
    Ok(captured)
}

fn validate_journal(
    c: &Connection,
    store: &ProjectionStore,
    current: &VersionedProjection,
    journal: &Journal,
) -> Result<()> {
    let expected = &journal.preview;
    if journal.schema_version != JOURNAL_SCHEMA
        || expected.digest != expected.computed_digest()?
        || expected.projection_generation != current.generation
        || expected.projection_digest != current.digest
        || !journal
            .directory
            .starts_with(&format!("rebase-{}-", &expected.digest[7..23]))
        || journal.directory.len() > 96
    {
        return Err(invalid("rebase journal identity mismatch"));
    }
    validate_target(&current.snapshot, &expected.target)?;
    validate_resolutions(&expected.target, &expected.resolutions)?;
    if expected.before.keys().ne(expected.resolutions.keys()) {
        return Err(invalid("rebase before/resolution paths differ"));
    }
    let patch = capture::storage::load(c, store, expected.capture_sequence)?;
    let latest: Option<u64> =
        c.query_row("SELECT MAX(sequence) FROM projection_captures", [], |r| {
            r.get(0)
        })?;
    if latest != Some(expected.capture_sequence)
        || patch.digest != expected.capture_digest
        || patch.projection_generation != current.generation
        || patch.projection_digest != current.digest
    {
        return Err(invalid("rebase source capture changed"));
    }
    let mut old_paths = Vec::new();
    let mut new_paths = Vec::new();
    for (path, before) in &expected.before {
        if before == &expected.resolutions[path] {
            continue;
        }
        if let Some(file) = before {
            old_paths.push((path, file));
        }
        if let Some(file) = &expected.resolutions[path] {
            new_paths.push((path, file));
        }
    }
    if old_paths.len() != journal.old.len() {
        return Err(invalid("rebase old witness count mismatch"));
    }
    for (index, ((path, file), record)) in old_paths.iter().zip(&journal.old).enumerate() {
        if record.path != **path
            || record.file != **file
            || record.name != format!("old-{index:08}")
            || record.identity.is_empty()
        {
            return Err(invalid("rebase old witness mismatch"));
        }
    }
    if journal.ready {
        if journal
            .directory_identity
            .as_ref()
            .is_none_or(String::is_empty)
            || new_paths.len() != journal.new.len()
        {
            return Err(invalid("rebase staged witness mismatch"));
        }
        for (index, ((path, file), record)) in new_paths.iter().zip(&journal.new).enumerate() {
            if record.path != **path
                || record.file != **file
                || record.name != format!("new-{index:08}")
                || record.identity.is_empty()
            {
                return Err(invalid("rebase new witness mismatch"));
            }
        }
    } else if journal.directory_identity.is_some() || !journal.new.is_empty() {
        return Err(invalid("rebase incomplete intent changed"));
    }
    Ok(())
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn verify_path(root: &PinnedDirectory, path: &str, record: &FileRecord) -> Result<PinnedFile> {
    let pin = root.open_relative_file(Path::new(path))?;
    verify_pin(&pin, record)?;
    let (parent, name) = root.existing_parent(Path::new(path))?;
    parent.verify_path_identity()?;
    if !pin.same_file(&parent.open_file(&name)?) {
        return Err(invalid("rebase file replaced during verification"));
    }
    Ok(pin)
}
fn verify_pin(pin: &PinnedFile, record: &FileRecord) -> Result<()> {
    if file::identity(pin.file())? != record.identity
        || pin.file().metadata()?.len() != record.file.bytes
        || record.file.bytes > MAX_CAPTURE_FILE_BYTES
        || capture::scan::executable(&pin.file().metadata()?) != record.file.executable
    {
        return Err(invalid("rebase file identity, mode or length changed"));
    }
    let mut input = pin.file();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > record.file.bytes {
            return Err(invalid("rebase file grew during verification"));
        }
        hash.update(&buffer[..n]);
    }
    if count != record.file.bytes
        || format!("blake3:{}", hash.finalize()) != record.file.file_id.as_str()
    {
        return Err(invalid("rebase file bytes changed"));
    }
    pin.verify_parent()?;
    Ok(())
}
