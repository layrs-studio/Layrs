use super::*;
use layrs_core::scm::FileId;
use std::{
    fs,
    io::{Read, Write},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
pub(super) fn unique_id() -> Result<String> {
    Ok(format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(invalid)?
            .as_nanos(),
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ))
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: String,
    preview: NamespacePreview,
    source_path: String,
    file: CapturedFile,
    identity: String,
}

pub(super) fn apply(
    store: &ProjectionStore,
    expected: &NamespacePreview,
    create_bytes: Option<&[u8]>,
    faults: &dyn NamespaceFaults,
) -> Result<CapturedPatch> {
    if expected.digest != expected.computed_digest()? {
        return Err(invalid("namespace preview digest mismatch"));
    }
    let mut access = format::access(store, SessionMode::Exclusive)?;
    database::require_mutable(&access.connection)?;
    if let Some(capture) = completed(&access.connection, store, expected, create_bytes)? {
        return Ok(capture);
    }
    let state = database::current(&access.connection)?;
    let mut actual = preview(store, &state, &expected.mutation)?;
    actual.operation_id = expected.operation_id.clone();
    actual.digest = actual.computed_digest()?;
    if &actual != expected {
        return Err(invalid("namespace preview is stale"));
    }
    let root = PinnedDirectory::open(&store.root)?;
    let (source_path, file) = match (&expected.mutation, create_bytes) {
        (NamespaceMutation::Create { .. }, Some(bytes)) => {
            if bytes.len() > MAX_PATCH_BYTES {
                return Err(invalid("create exceeds16MiB"));
            }
            let scan = super::super::capture::scan::scan(store, &state.snapshot, false)?;
            let total: u64 = scan
                .changes
                .iter()
                .filter_map(|c| c.replacement.as_ref())
                .map(|f| f.bytes)
                .sum();
            if total + bytes.len() as u64 > MAX_PATCH_BYTES as u64 || scan.changes.len() >= 1024 {
                return Err(invalid("creation would exceed capture budget"));
            }
            let staging = root
                .open_child(".layrs".as_ref())?
                .ensure_child("namespace-staging".as_ref())?;
            let name = unique_id()?;
            let mut file = staging.create_file(name.as_ref())?;
            file.file_mut().write_all(bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.file()
                    .set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.file().sync_all()?;
            staging.sync()?;
            (
                format!(".layrs/namespace-staging/{name}"),
                CapturedFile {
                    file_id: FileId::from_bytes(bytes),
                    executable: false,
                    bytes: bytes.len() as u64,
                },
            )
        }
        (NamespaceMutation::Move { source, .. }, None) => {
            let file = expected
                .source_file
                .clone()
                .ok_or_else(|| invalid("missing move source descriptor"))?;
            let scan = super::super::capture::scan::scan(store, &state.snapshot, false)?;
            let remaining: Vec<_> = scan.changes.iter().filter(|c| &c.path != source).collect();
            let total: u64 = remaining
                .iter()
                .filter_map(|c| c.replacement.as_ref())
                .map(|f| f.bytes)
                .sum();
            let source_was_remote = state
                .snapshot
                .entries
                .iter()
                .any(|entry| &entry.path == source && entry.target.is_some());
            if total + file.bytes > MAX_PATCH_BYTES as u64
                || remaining.len() + 1 + usize::from(source_was_remote) > 1024
            {
                return Err(invalid("move would exceed capture budget"));
            }
            (source.to_string(), file)
        }
        _ => {
            return Err(invalid(
                "create needs bytes; move must not carry replacement bytes",
            ));
        }
    };
    let identity = super::super::file::DatabasePin::open(&store.root.join(&source_path))?
        .identity()
        .to_owned();
    let journal = Journal {
        schema_version: "layrs.projection-namespace.v1".into(),
        preview: expected.clone(),
        source_path,
        file,
        identity,
    };
    verify_file(&root, &store.root, &journal.source_path, &journal)?;
    let bytes = canonical_json(&journal).map_err(invalid)?;
    if bytes.len() > 64 * 1024 {
        return Err(invalid("namespace journal exceeds64KiB"));
    }
    access.verify()?;
    access.connection.execute(
        "INSERT INTO projection_namespace_pending VALUES(1,?1)",
        [bytes],
    )?;
    faults.check(NamespacePoint::JournalDurable)?;
    finish(store, &mut access, &journal, faults)
}
pub(super) fn resume(store: &ProjectionStore) -> Result<CapturedPatch> {
    let mut access = format::access(store, SessionMode::Exclusive)?;
    let conflicts:bool=access.connection.query_row("SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_publication_pending) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_restore_pending)",[],|r|r.get(0))?;
    if conflicts {
        return Err(invalid(
            "incompatible simultaneous projection recovery journals",
        ));
    }
    let len: u64 = access.connection.query_row(
        "SELECT length(payload) FROM projection_namespace_pending WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if len > 64 * 1024 {
        return Err(invalid("namespace journal exceeds64KiB"));
    }
    let bytes: Vec<u8> = access.connection.query_row(
        "SELECT payload FROM projection_namespace_pending WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(invalid)?;
    if canonical_json(&journal).map_err(invalid)? != bytes {
        return Err(invalid("namespace journal is not canonical"));
    }
    finish(store, &mut access, &journal, &NoNamespaceFaults)
}
fn finish(
    store: &ProjectionStore,
    access: &mut format::Access,
    journal: &Journal,
    faults: &dyn NamespaceFaults,
) -> Result<CapturedPatch> {
    let state = database::current(&access.connection)?;
    let expected = &journal.preview;
    if journal.schema_version != "layrs.projection-namespace.v1"
        || expected.digest != expected.computed_digest()?
        || state.generation != expected.projection_generation
        || state.digest != expected.projection_digest
        || journal.file.bytes > MAX_PATCH_BYTES as u64
    {
        return Err(invalid("namespace recovery identity mismatch"));
    }
    validate_mutation(&state.snapshot, &expected.mutation)?;
    match &expected.mutation {
        NamespaceMutation::Move { source, .. }
            if source.as_str() == journal.source_path
                && expected.source_file.as_ref() == Some(&journal.file) => {}
        NamespaceMutation::Create { .. } => {
            let suffix = journal
                .source_path
                .strip_prefix(".layrs/namespace-staging/")
                .ok_or_else(|| invalid("invalid namespace staging path"))?;
            if suffix.is_empty()
                || suffix.len() > 128
                || !suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
                || journal.file.executable
                || expected.source_file.is_some()
            {
                return Err(invalid("invalid namespace create journal"));
            }
        }
        _ => return Err(invalid("namespace journal source mismatch")),
    }
    let root = PinnedDirectory::open(&store.root)?;
    let destination = expected.mutation.destination().as_str();
    match fs::symlink_metadata(store.root.join(&journal.source_path)) {
        Ok(_) => {
            let source = verify_file(&root, &store.root, &journal.source_path, journal)?;
            super::super::install::files::destination_absent(&root, &store.root, destination)?;
            let (target_parent, target_name) = root.components_to_parent(Path::new(destination))?;
            if source.rename_to(&target_parent, &target_name)? {
                return Err(invalid(
                    "namespace destination appeared; nothing overwritten",
                ));
            }
            source.sync_parent()?;
            target_parent.sync()?;
            root.verify_path_identity()?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    verify_file(&root, &store.root, destination, journal)?;
    faults.check(NamespacePoint::PathApplied)?;
    let first = super::super::capture::scan::scan(store, &state.snapshot, false)?;
    let scan = super::super::capture::scan::scan(store, &state.snapshot, true)?;
    if first.files != scan.files {
        return Err(invalid("checkout changed during namespace finalization"));
    }
    verify_file(&root, &store.root, destination, journal)?;
    access.verify()?;
    let tx = access
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let capture = super::super::capture::storage::persist_in_transaction(&tx, store, &state, scan)?;
    let bytes = canonical_json(journal).map_err(invalid)?;
    tx.execute(
        "INSERT INTO projection_namespace_operations VALUES(?1,?2,?3)",
        params![expected.digest, capture.sequence, bytes],
    )?;
    tx.execute(
        "DELETE FROM projection_namespace_pending WHERE singleton=1",
        [],
    )?;
    faults.check(NamespacePoint::BeforeCommit)?;
    tx.commit()?;
    faults.check(NamespacePoint::AfterCommit)?;
    access.verify()?;
    Ok(capture)
}
fn verify_file(
    root: &PinnedDirectory,
    base: &Path,
    path: &str,
    journal: &Journal,
) -> Result<crate::secure_fs::PinnedFile> {
    let (parent, name) = root.existing_parent(Path::new(path))?;
    let pin = parent.open_file(&name)?;
    let identity = super::super::file::DatabasePin::open(&base.join(path))?;
    if identity.identity() != journal.identity || pin.file().metadata()?.len() != journal.file.bytes
    {
        return Err(invalid("namespace file replaced or resized"));
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
        if count > journal.file.bytes {
            return Err(invalid("namespace file grew"));
        }
        hash.update(&buffer[..n]);
    }
    if count != journal.file.bytes
        || format!("blake3:{}", hash.finalize()) != journal.file.file_id.as_str()
    {
        return Err(invalid("namespace file content changed"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (pin.file().metadata()?.permissions().mode() & 0o111 != 0) != journal.file.executable {
            return Err(invalid("namespace file mode changed"));
        }
    }
    identity.verify(&base.join(path))?;
    parent.verify_path_identity()?;
    if !pin.same_file(&parent.open_file(&name)?) {
        return Err(invalid("namespace file changed during validation"));
    }
    Ok(pin)
}
fn completed(
    c: &rusqlite::Connection,
    store: &ProjectionStore,
    preview: &NamespacePreview,
    bytes: Option<&[u8]>,
) -> Result<Option<CapturedPatch>> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_namespace_operations WHERE preview_digest=?1)",
        [&preview.digest],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let length: u64 = c.query_row(
        "SELECT length(payload) FROM projection_namespace_operations WHERE preview_digest=?1",
        [&preview.digest],
        |r| r.get(0),
    )?;
    if length > 64 * 1024 {
        return Err(invalid("namespace history document exceeds64KiB"));
    }
    let (sequence,payload):(u64,Vec<u8>)=c.query_row("SELECT capture_sequence,payload FROM projection_namespace_operations WHERE preview_digest=?1",[&preview.digest],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let journal: Journal = serde_json::from_slice(&payload).map_err(invalid)?;
    if journal.preview != *preview || canonical_json(&journal).map_err(invalid)? != payload {
        return Err(invalid("namespace replay identity changed"));
    }
    match (&preview.mutation, bytes) {
        (NamespaceMutation::Create { .. }, Some(bytes))
            if bytes.len() as u64 == journal.file.bytes
                && FileId::from_bytes(bytes) == journal.file.file_id => {}
        (NamespaceMutation::Move { .. }, None) => (),
        _ => return Err(invalid("namespace replay bytes differ")),
    }
    Ok(Some(super::super::capture::storage::load(
        c, store, sequence,
    )?))
}

/// Verify immutable namespace history without touching live checkout files.
pub(in crate::projection) fn verify_history(
    c: &rusqlite::Connection,
    store: &ProjectionStore,
) -> Result<u64> {
    let count: u64 = c.query_row(
        "SELECT COUNT(*) FROM projection_namespace_operations",
        [],
        |r| r.get(0),
    )?;
    if count > 100_000 {
        return Err(invalid("namespace history exceeds100000 operations"));
    }
    let mut statement=c.prepare("SELECT preview_digest,capture_sequence,length(payload) FROM projection_namespace_operations ORDER BY preview_digest")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let sequence: u64 = row.get(1)?;
        let length: u64 = row.get(2)?;
        if length > 64 * 1024 {
            return Err(invalid("namespace history document exceeds64KiB"));
        }
        let payload: Vec<u8> = c.query_row(
            "SELECT payload FROM projection_namespace_operations WHERE preview_digest=?1",
            [&key],
            |r| r.get(0),
        )?;
        let journal: Journal = serde_json::from_slice(&payload).map_err(invalid)?;
        if journal.schema_version != "layrs.projection-namespace.v1"
            || canonical_json(&journal).map_err(invalid)? != payload
            || journal.preview.computed_digest()? != key
            || journal.preview.digest != key
        {
            return Err(invalid("namespace history canonical identity mismatch"));
        }
        let capture = super::super::capture::storage::load(c, store, sequence)?;
        if capture.projection_generation != journal.preview.projection_generation
            || capture.projection_digest != journal.preview.projection_digest
        {
            return Err(invalid("namespace history capture base mismatch"));
        }
        let dest = capture
            .changes
            .iter()
            .find(|change| &change.path == journal.preview.mutation.destination())
            .ok_or_else(|| invalid("namespace capture missing destination"))?;
        if dest.expected.is_some() || dest.replacement.as_ref() != Some(&journal.file) {
            return Err(invalid("namespace capture destination mismatch"));
        }
        if let NamespaceMutation::Move { source, .. } = &journal.preview.mutation {
            // A locally created source has no remote entry to remove. Its
            // explicit writable absence observation remains in the base.
            let base_bytes: Vec<u8> = c.query_row(
                "SELECT payload FROM projection_snapshots WHERE generation=?1",
                [capture.projection_generation],
                |r| r.get(0),
            )?;
            let base: ProjectionSnapshot = serde_json::from_slice(&base_bytes).map_err(invalid)?;
            validate_mutation(&base, &journal.preview.mutation)?;
            let source_entry = base
                .entries
                .iter()
                .find(|entry| &entry.path == source)
                .ok_or_else(|| invalid("namespace history source missing"))?;
            let from = capture.changes.iter().find(|change| &change.path == source);
            let source_matches = if source_entry.state == EntryState::Absent {
                from.is_none()
            } else {
                from.is_some_and(|change| change.expected.is_some() && change.replacement.is_none())
            };
            if !source_matches
                || journal.source_path != source.as_str()
                || journal.preview.source_file.as_ref() != Some(&journal.file)
            {
                return Err(invalid("namespace capture source mismatch"));
            }
        }
    }
    Ok(count)
}
