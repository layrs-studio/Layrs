use super::*;
#[cfg(windows)]
mod recovery;
#[cfg(windows)]
pub(in crate::projection) use recovery::{RestoreRecoveryData, recovery_preflight};

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
/// Forced-append funding DATA for one local C14 Restore. Private fields: only
/// `local_restore_plan` derives it from a reviewed, recomputed preview.
pub(in crate::projection) struct LocalRestorePlan {
    operation: String,
    source: RestoreSource,
    changes: Vec<CapturedChange>,
    files: Vec<CapturedFile>,
    journal_bound: u64,
}
impl LocalRestorePlan {
    pub(in crate::projection) fn operation(&self) -> &str {
        &self.operation
    }
    pub(in crate::projection) fn source(&self) -> &RestoreSource {
        &self.source
    }
    /// Exact full-checkout diff the finish transaction appends.
    pub(in crate::projection) fn changes(&self) -> &[CapturedChange] {
        &self.changes
    }
    /// Distinct preimages, restored targets and forced-capture replacements.
    pub(in crate::projection) fn files(&self) -> &[CapturedFile] {
        &self.files
    }
    /// Upper bound of the canonical pending/history journal payload.
    pub(in crate::projection) fn journal_bound(&self) -> u64 {
        self.journal_bound
    }
}

fn local_restore_plan(
    expected: &RestorePreview,
    changes: Vec<CapturedChange>,
) -> Result<LocalRestorePlan> {
    let mut seen = std::collections::BTreeSet::new();
    let files = expected
        .before
        .values()
        .chain(expected.after.values())
        .flatten()
        .chain(
            changes
                .iter()
                .filter_map(|change| change.replacement.as_ref()),
        )
        .filter(|file| seen.insert(file.file_id.clone()))
        .cloned()
        .collect();
    Ok(LocalRestorePlan {
        operation: format!("restore:{}", expected.operation_id),
        source: expected.source.clone(),
        changes,
        files,
        journal_bound: journal_upper_bound(expected)?,
    })
}

/// Canonical journal length with every runtime witness at its maximum width:
/// u128 staging timestamp, fixed-width directory token and u64 identity pairs
/// (wider than Windows u32 triples). Digits/colons need no JSON escaping.
fn journal_upper_bound(preview: &RestorePreview) -> Result<u64> {
    let identity = format!("{}:{}", u64::MAX, u64::MAX);
    let witnesses = |files: &BTreeMap<RepoPath, Option<CapturedFile>>| {
        files
            .iter()
            .filter(|(_, file)| file.is_some())
            .map(|(path, _)| (path.clone(), identity.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let journal = Journal {
        schema_version: "layrs.projection-restore.v1".into(),
        preview: preview.clone(),
        directory: format!("restore-{}-{}", preview.operation_id, u128::MAX),
        directory_identity: format!("fs:v1:{:016x}:{:016x}", u64::MAX, u64::MAX),
        old_identity: witnesses(&preview.before),
        new_identity: witnesses(&preview.after),
    };
    Ok(encode(&journal)?.len() as u64)
}

/// DATA-only interpretation of a local C14 history journal against its own
/// authenticated forced capture and the fixed Ready snapshot.
#[cfg(windows)]
pub(in crate::projection) fn local_history_preview(
    bytes: &[u8],
    id: &str,
    capture: &CapturedPatch,
    base: &ProjectionSnapshot,
    destination: &RestoreDestination,
) -> Result<RestorePreview> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(invalid("restore history exceeds8MiB"));
    }
    let journal = decode(bytes)?;
    if journal.preview.destination.as_ref() != Some(destination) {
        return Err(invalid("local restore history destination differs"));
    }
    validate_history_preview(&journal, id, capture, base)?;
    Ok(journal.preview)
}

/// DATA-only interpretation of the one pending local C14 journal.
#[cfg(windows)]
pub(in crate::projection) fn local_pending_preview(
    bytes: &[u8],
    base: &ProjectionSnapshot,
    generation: u64,
    digest: &str,
    destination: &RestoreDestination,
) -> Result<RestorePreview> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(invalid("restore journal exceeds8MiB"));
    }
    let journal = decode(bytes)?;
    if journal.preview.destination.as_ref() != Some(destination)
        || journal.preview.projection_generation != generation
        || journal.preview.projection_digest != digest
    {
        return Err(invalid("local pending restore escaped fixed Ready scope"));
    }
    validate_paths(base, &journal.preview.before)?;
    validate_paths(base, &journal.preview.after)?;
    Ok(journal.preview)
}

/// C14 dispatch from the access's authenticated admission DATA (no SQL);
/// legacy and non-denial stores keep their unscoped mutable guards.
fn require_mutable(access: &mut format::Access, store: &ProjectionStore) -> Result<bool> {
    let local = match &access.connection {
        #[cfg(windows)]
        format::PrivateConnection::Admitted(handle) => handle
            .admission()
            .data()
            .ready()
            .plan
            .denial_coverage
            .is_some(),
        _ => false,
    };
    if local {
        access.with_content(|c, scope| database::require_mutable_with_scope(c, store, scope))?;
    } else {
        database::require_mutable(&access.connection)?;
    }
    Ok(local)
}

pub(super) fn apply(
    store: &ProjectionStore,
    expected: &RestorePreview,
    faults: &dyn RestoreFaults,
) -> Result<RestoreReceipt> {
    if preview_digest(expected)? != expected.digest {
        return Err(invalid("restore preview digest mismatch"));
    }
    let access = format::access_for_local_restore(store)?;
    access.run(|access: &mut format::Access| -> Result<_> {
        validate_destination(store, expected)?;
        let local = require_mutable(access, store)?;
        if let Some((journal, capture)) = access.with_content(|c, scope| {
            history_with_scope(c, store, &expected.operation_id, Some(scope))
        })? {
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
        let (mut actual, resulting) = access.with_content(|c, scope| {
            preview_with_changes(store, c, &state, &expected.source, Some(scope))
        })?;
        actual.operation_id = expected.operation_id.clone();
        actual.digest = preview_digest(&actual)?;
        if actual != *expected {
            return Err(invalid("restore preview is stale"));
        }
        retention::require_no_staging(&access.connection, store)?;
        if local {
            // Restore never deduplicates its capture: prove the forced append,
            // journal, history row, claims and mappings all fit before the
            // first reservation, staging or filesystem effect.
            let plan = local_restore_plan(expected, resulting)?;
            access
                .with_content(|c, scope| scope.preflight_local_restore(c, store, &state, &plan))?;
        }
        let changes = expected
            .after
            .iter()
            .map(|(p, f)| (p.to_string(), f.clone()))
            .collect();
        access.with_content(|c, scope| {
            retention::reserve_checkout_with_scope(
                c,
                store,
                &format!("restore:{}", expected.operation_id),
                &state.snapshot,
                &changes,
                Some(scope),
            )
        })?;
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
        retention::track_directory(
            &access.connection,
            store,
            &format!("restore:{}", expected.operation_id),
            &format!(".layrs/projection-quarantine/{directory_name}"),
            &directory,
            expected
                .after
                .values()
                .enumerate()
                .filter_map(|(i, file)| file.as_ref().map(|_| format!("new-{i:04}")))
                .collect(),
        )?;
        let mut journal = Journal {
            schema_version: "layrs.projection-restore.v1".into(),
            preview: expected.clone(),
            directory: directory_name,
            directory_identity: directory.identity_token(),
            old_identity: BTreeMap::new(),
            new_identity: BTreeMap::new(),
        };
        for (index, (path, old)) in expected.before.iter().enumerate() {
            if let Some(old) = old {
                let pin = root.open_relative_file(Path::new(path.as_str()))?;
                let identity = file::identity(pin.file())?;
                files::verified(&root, path.as_str(), old, &identity)?;
                journal.old_identity.insert(path.clone(), identity);
            }
            if let Some(new) = &expected.after[path] {
                let mut staged = directory.create_file(format!("new-{index:04}").as_ref())?;
                access.with_content(|c, scope| {
                    capture::storage::stream_file_with_scope(
                        c,
                        store,
                        new,
                        staged.file_mut(),
                        Some(scope),
                    )
                })?;
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
        if access.with_content(|c, scope| {
            checkout_digest(
                &capture::scan::scan_with_scope(store, c, &state.snapshot, false, Some(scope))?
                    .files,
            )
        })? != expected.checkout_digest
        {
            return Err(invalid("checkout changed during restore staging"));
        }
        access.verify()?;
        access.with_content(|c, scope| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for (path, old) in &expected.before {
                if let Some(old) = old {
                    let pin =
                        files::verified(&root, path.as_str(), old, &journal.old_identity[path])?;
                    capture::storage::persist_pinned_file_with_scope(
                        &tx,
                        store,
                        &pin,
                        old,
                        Some(scope),
                    )?;
                }
            }
            if checkout_digest(
                &capture::scan::scan_with_scope(store, &tx, &state.snapshot, false, Some(scope))?
                    .files,
            )? != expected.checkout_digest
            {
                return Err(invalid("checkout changed during restore preservation"));
            }
            tx.execute(
                "INSERT INTO projection_restore_pending VALUES(1,?1)",
                [encode(&journal)?],
            )?;
            retention::accept_staging(&tx, store, &format!("restore:{}", expected.operation_id))?;
            let family = format::content_family(&tx, store, Some(scope))?;
            if let Some(lease) = family.as_ref() {
                let manifests = expected
                    .before
                    .values()
                    .chain(expected.after.values())
                    .flatten()
                    .map(|f| retention::manifest_with_scope(&tx, store, f, Some(scope)))
                    .collect::<Result<Vec<_>>>()?;
                retention::accept_claim(
                    &tx,
                    store,
                    lease,
                    "restore",
                    &expected.operation_id,
                    &manifests,
                )?;
            }
            tx.commit()?;
            Ok(())
        })?;
        faults.check(RestorePoint::JournalDurable)?;
        let receipt = finish(store, access, &journal, faults)?;
        if local {
            // As local Capture: a committed C14 budget never lingers after an
            // acknowledged apply. Non-C14 callers keep explicit settlement.
            retention::settle_with_lease(&access.connection, store, access.family.as_ref())?;
            access.verify()?;
        }
        Ok(receipt)
    })
}
pub(super) fn resume(store: &ProjectionStore) -> Result<RestoreReceipt> {
    resume_with_access(store, format::restore_recovery_access(store, None)?)
}
pub(in crate::projection) fn resume_with_access(
    store: &ProjectionStore,
    access: format::Access,
) -> Result<RestoreReceipt> {
    access.run(|access: &mut format::Access| -> Result<_> {
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
        finish(store, access, &decode(&bytes)?, &NoRestoreFaults)
    })
}
fn finish(
    store: &ProjectionStore,
    access: &mut format::Access,
    journal: &Journal,
    faults: &dyn RestoreFaults,
) -> Result<RestoreReceipt> {
    let conflicts: bool = access.connection.query_row("SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_publication_pending) OR EXISTS(SELECT 1 FROM projection_rebase_pending)", [], |r| r.get(0))?;
    if conflicts {
        return Err(invalid("another operation blocks restore recovery"));
    }
    let state = database::current(&access.connection)?;
    let expected = &journal.preview;
    decode(&encode(journal)?)?;
    // Only finish() may recover an old unbound journal: its payload came from
    // this exclusively locked, FORMAT/inode-bound private database, and the
    // quarantine plus every old/new inode is checked below. apply() rejects
    // unbound caller artifacts before it can reach here. Preserve the original
    // digest and historical payload; do not manufacture a new approval.
    if expected.destination.is_some() {
        validate_destination(store, expected)?;
    }
    if state.generation != expected.projection_generation
        || state.digest != expected.projection_digest
        || access.with_content(|c, scope| targets(c, store, &expected.source, Some(scope)))?
            != expected.after
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
    let scan = access.with_content(|c, scope| {
        capture::scan::scan_with_scope(store, c, &state.snapshot, true, Some(scope))
    })?;
    if checkout_digest(&scan.files)? != expected.after_checkout_digest {
        return Err(invalid("checkout changed during restoration"));
    }
    verify_installed(&root, &store.root, expected, journal)?;
    access.verify()?;
    let capture = access.with_content(|c, scope| {
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let capture = capture::storage::persist_restore_in_transaction_with_scope(
            &tx,
            store,
            &state,
            scan,
            Some(scope),
        )?;
        verify_installed(&root, &store.root, expected, journal)?;
        tx.execute(
            "INSERT INTO projection_restore_history VALUES(?1,?2,?3)",
            params![expected.operation_id, capture.sequence, encode(journal)?],
        )?;
        tx.execute(
            "DELETE FROM projection_restore_pending WHERE singleton=1",
            [],
        )?;
        retention::transfer(
            &tx,
            store,
            "restore",
            &expected.operation_id,
            "restore-history",
            &expected.operation_id,
        )?;
        faults.check(RestorePoint::BeforeCommit)?;
        verify_installed(&root, &store.root, expected, journal)?;
        tx.commit()?;
        Ok(capture)
    })?;
    faults.check(RestorePoint::AfterCommit)?;
    access.verify()?;
    Ok(RestoreReceipt {
        operation_id: expected.operation_id.clone(),
        capture,
    })
}
fn verify_installed(
    root: &PinnedDirectory,
    base: &Path,
    expected: &RestorePreview,
    journal: &Journal,
) -> Result<()> {
    for (path, new) in &expected.after {
        if let Some(new) = new {
            files::verified(root, path.as_str(), new, &journal.new_identity[path])?;
        } else {
            install::files::destination_absent(root, base, path.as_str())?;
        }
    }
    Ok(())
}
pub(super) fn history(
    c: &Connection,
    store: &ProjectionStore,
    id: &str,
) -> Result<Option<(Journal, CapturedPatch)>> {
    history_with_scope(c, store, id, None)
}
pub(super) fn history_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    id: &str,
    scope: Option<&format::ContentScope<'_>>,
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
    let capture = capture::storage::load_with_scope(c, store, sequence, scope)?;
    let admitted_layout = matches!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
        9 | 10
    );
    // A later checkout can reuse an ancestor's logical identity. Only the
    // authenticated incoming capture boundary distinguishes that history from
    // this destination's local suffix.
    #[cfg(windows)]
    let admitted_import =
        admitted_layout && provider::admitted_ordinary::imported(c, &store.marker, sequence, None)?;
    #[cfg(not(windows))]
    let admitted_import = admitted_layout;
    if admitted_import {
        #[cfg(windows)]
        {
            let origin = provider::admitted_ordinary::restore_history_origin(
                c,
                &store.marker,
                id,
                sequence,
                &bytes,
                None,
            )?;
            let bound = journal
                .preview
                .destination
                .as_ref()
                .ok_or_else(|| invalid("imported restore history destination is unbound"))?;
            if bound.identity != capture.identity
                || bound.identity != origin.identity
                || bound.checkout_identity != origin.checkout_identity
                || capture.projection_generation < origin.minimum_generation
                || capture.projection_generation > origin.maximum_generation
            {
                return Err(invalid("restore history differs from admitted origin"));
            }
        }
        #[cfg(not(windows))]
        return Err(invalid("managed restore history requires Windows support"));
    } else if let Some(bound) = &journal.preview.destination {
        // Legacy imports retain their original destination. Application still
        // requires a newly reviewed preview bound to the current checkout.
        let marker = if capture.identity == *store.identity() {
            store.marker.clone()
        } else {
            let import = conversion::provenance(c, store)?
                .ok_or_else(|| invalid("restore history has no import provenance"))?;
            serde_json::from_slice::<format::Marker>(&import.source_marker).map_err(invalid)?
        };
        if bound.identity != capture.identity
            || bound.identity != marker.identity
            || bound.checkout_identity != marker.checkout_identity()
        {
            return Err(invalid(
                "restore history destination differs from capture provenance",
            ));
        }
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_snapshots WHERE generation=?1",
        [capture.projection_generation],
        |r| r.get(0),
    )?;
    let base: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
    validate_history_preview(&journal, id, &capture, &base)?;
    for file in journal
        .preview
        .before
        .values()
        .chain(journal.preview.after.values())
        .flatten()
    {
        capture::storage::file_manifest_with_scope(c, store, file, scope)?;
    }
    Ok(Some((journal, capture)))
}

/// Interpret canonical historical DATA against an independently authenticated
/// capture, snapshot and origin checkout. This does not authorize content I/O,
/// open historical filesystem witnesses, or approve a restore at a new root.
pub(in crate::projection) fn imported_history_preview(
    bytes: &[u8],
    id: &str,
    capture: &CapturedPatch,
    base: &ProjectionSnapshot,
    origin_checkout: &str,
) -> Result<RestorePreview> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(invalid("restore history exceeds8MiB"));
    }
    let journal = decode(bytes)?;
    let bound = journal
        .preview
        .destination
        .as_ref()
        .ok_or_else(|| invalid("imported restore history destination is unbound"))?;
    if bound.identity != capture.identity || bound.checkout_identity != origin_checkout {
        return Err(invalid("imported restore history origin differs"));
    }
    validate_history_preview(&journal, id, capture, base)?;
    Ok(journal.preview)
}

fn validate_history_preview(
    journal: &Journal,
    id: &str,
    capture: &CapturedPatch,
    base: &ProjectionSnapshot,
) -> Result<()> {
    if journal.preview.operation_id != id
        || capture.projection_generation != journal.preview.projection_generation
        || capture.projection_digest != journal.preview.projection_digest
    {
        return Err(invalid("restore history capture mismatch"));
    }
    validate_paths(base, &journal.preview.before)?;
    validate_paths(base, &journal.preview.after)?;
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
    Ok(())
}
pub(in crate::projection) fn verify_history(
    c: &Connection,
    store: &ProjectionStore,
) -> Result<u64> {
    verify_history_with_scope(c, store, None)
}
pub(in crate::projection) fn verify_history_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<u64> {
    let mut statement =
        c.prepare("SELECT operation_id FROM projection_restore_history ORDER BY operation_id")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let entry = if scope.is_some() {
            history_with_scope(c, store, &id, scope)?
        } else {
            history(c, store, &id)?
        };
        entry.ok_or_else(|| invalid("restore history missing"))?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod local_bound_tests {
    use super::*;
    use layrs_core::scm::{FileId, SpaceId, StoreId, WorktreeId};

    fn preview(paths: usize) -> RestorePreview {
        let identity = ProjectionIdentity {
            store_id: StoreId::parse("store:bound").unwrap(),
            worktree_id: WorktreeId::parse("worktree:bound").unwrap(),
            space_id: SpaceId::parse("space:bound").unwrap(),
            origin: "https://example.test".into(),
            principal_id: "principal:bound".into(),
        };
        let file = |n: usize| {
            Some(CapturedFile {
                file_id: FileId::from_bytes(&n.to_le_bytes()),
                executable: false,
                bytes: n as u64,
            })
        };
        let path = |n: usize| RepoPath::parse(format!("dir/file-{n:04}.txt")).unwrap();
        let mut value = RestorePreview {
            operation_id: format!("{}-{}", u128::MAX, u32::MAX),
            destination: Some(RestoreDestination {
                identity,
                checkout_identity: "checkout:bound".into(),
            }),
            source: RestoreSource::Capture {
                sequence: 2,
                paths: (0..paths).map(path).collect(),
            },
            projection_generation: 1,
            projection_digest: "a".repeat(64),
            checkout_digest: format!("blake3:{}", "b".repeat(64)),
            after_checkout_digest: format!("blake3:{}", "c".repeat(64)),
            before: (0..paths).map(|n| (path(n), file(n))).collect(),
            after: (0..paths).map(|n| (path(n), file(n + paths))).collect(),
            digest: String::new(),
        };
        value.digest = preview_digest(&value).unwrap();
        value
    }

    #[test]
    fn journal_upper_bound_dominates_windows_identities() {
        for paths in [1, 7, 1024] {
            let preview = preview(paths);
            let bound = journal_upper_bound(&preview).unwrap();
            // Widest real witnesses: u128 staging nanos, a fixed-width token
            // and Windows u32 identity triples (narrower than the u64 pairs).
            let real = Journal {
                schema_version: "layrs.projection-restore.v1".into(),
                preview: preview.clone(),
                directory: format!("restore-{}-{}", preview.operation_id, u128::MAX),
                directory_identity: format!("fs:v1:{:016x}:{:016x}", 1u64, 2u64),
                old_identity: preview
                    .before
                    .keys()
                    .map(|p| (p.clone(), format!("{}:{}:{}", u32::MAX, u32::MAX, u32::MAX)))
                    .collect(),
                new_identity: preview
                    .after
                    .keys()
                    .map(|p| (p.clone(), format!("{}:{}:{}", u32::MAX, u32::MAX, u32::MAX)))
                    .collect(),
            };
            let actual = encode(&real).unwrap().len() as u64;
            assert!(actual <= bound, "{actual} > {bound}");
            assert!(bound - actual <= 9 * 2 * paths as u64, "bound is not tight");
            decode(&encode(&real).unwrap()).unwrap();
        }
    }

    #[test]
    fn local_restore_plan_counts_distinct_files_and_forced_capture_changes() {
        let preview = preview(3);
        let changes = vec![CapturedChange {
            path: RepoPath::parse("dir/file-0000.txt").unwrap(),
            expected: None,
            replacement: preview.after.values().next().unwrap().clone(),
        }];
        let plan = local_restore_plan(&preview, changes.clone()).unwrap();
        assert_eq!(
            plan.operation(),
            format!("restore:{}", preview.operation_id)
        );
        assert_eq!(plan.changes(), changes.as_slice());
        // 3 preimages + 3 targets; the forced replacement is already a target.
        assert_eq!(plan.files().len(), 6);
        assert_eq!(plan.journal_bound(), journal_upper_bound(&preview).unwrap());
    }
}
