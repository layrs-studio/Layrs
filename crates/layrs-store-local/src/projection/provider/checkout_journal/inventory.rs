use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlProjection {
    store_identity: String,
}
#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
enum InventoryItem<'a> {
    Directory(&'a RepoPath),
    File(&'a CheckoutFile),
    Deletion(&'a RepoPath),
}

pub(super) fn expected(
    staging: &ProviderTransitionIntentRecord,
    files: &[CheckoutFile],
    deletions: &[RepoPath],
    image_digest: &str,
    cancel: Option<&Cancellation>,
) -> Result<ExpectedCheckout> {
    cancellation::check(cancel)?;
    staging.validate_fields()?;
    if staging.state != TransitionState::Staging
        || staging.plan.kind != ProviderImportKind::ManagedOrdinary
        || staging.receipt_id.is_some()
        || staging.reservation.as_deref() != Some(staging.reservation_operation()?.as_str())
    {
        return Err(invalid(
            "checkout expected inventory requires exact managed Staging",
        ));
    }
    expected_parts(
        staging,
        files,
        deletions,
        image_digest,
        SourceKind::Provider,
        cancel,
    )
}

pub(super) fn expected_ordinary(
    staging: &ProviderTransitionIntentRecord,
    files: &[CheckoutFile],
    deletions: &[RepoPath],
    image_digest: &str,
    cancel: Option<&Cancellation>,
) -> Result<ExpectedCheckout> {
    cancellation::check(cancel)?;
    staging.validate_fields()?;
    if staging.state != TransitionState::Staging
        || staging.plan.kind != ProviderImportKind::ManagedOrdinary
        || staging.receipt_id.is_some()
        || staging.reservation.as_deref() != Some(staging.reservation_operation()?.as_str())
    {
        return Err(invalid(
            "ordinary checkout inventory requires exact managed Staging",
        ));
    }
    expected_parts(
        staging,
        files,
        deletions,
        image_digest,
        SourceKind::AdmittedOrdinary,
        cancel,
    )
}

pub(super) fn expected_ready(
    ready: &ProviderTransitionIntentRecord,
    files: &[CheckoutFile],
    deletions: &[RepoPath],
    image_digest: &str,
    cancel: Option<&Cancellation>,
) -> Result<ExpectedCheckout> {
    expected_ready_parts(
        ready,
        files,
        deletions,
        image_digest,
        SourceKind::Provider,
        cancel,
    )
}

fn expected_ready_parts(
    ready: &ProviderTransitionIntentRecord,
    files: &[CheckoutFile],
    deletions: &[RepoPath],
    image_digest: &str,
    source: SourceKind,
    cancel: Option<&Cancellation>,
) -> Result<ExpectedCheckout> {
    cancellation::check(cancel)?;
    ready.validate_fields()?;
    if ready.role != super::super::records::transition::TransitionRole::Destination
        || ready.state != TransitionState::Ready
        || ready.plan.kind != ProviderImportKind::ManagedOrdinary
        || ready.receipt_id.is_none()
        || ready.reservation.as_deref() != Some(ready.reservation_operation()?.as_str())
    {
        return Err(invalid(
            "checkout terminal inventory requires destination Ready",
        ));
    }
    expected_parts(ready, files, deletions, image_digest, source, cancel)
}

#[derive(Clone, Copy)]
enum SourceKind {
    Provider,
    AdmittedOrdinary,
}

fn expected_parts(
    staging: &ProviderTransitionIntentRecord,
    files: &[CheckoutFile],
    deletions: &[RepoPath],
    image_digest: &str,
    source_kind: SourceKind,
    cancel: Option<&Cancellation>,
) -> Result<ExpectedCheckout> {
    let version = match (&staging.plan.source_prefix.source, source_kind) {
        (ImportSource::Provider { version, .. }, SourceKind::Provider) => version,
        (
            ImportSource::Ordinary {
                private_schema: 9 | 10,
                admission: Some(_),
                checkout_identity: Some(identity),
                version,
                ..
            },
            SourceKind::AdmittedOrdinary,
        ) => {
            codec::directory_identity(identity)?;
            version
        }
        (_, SourceKind::Provider) => {
            return Err(invalid("checkout requires provider source prefix"));
        }
        (_, SourceKind::AdmittedOrdinary) => {
            return Err(invalid(
                "ordinary checkout requires admitted source and checkout identity",
            ));
        }
    };
    codec::raw_digest(image_digest)?;
    if image_digest != version.effective_image_digest {
        return Err(invalid("checkout image digest differs from planned image"));
    }
    let proof = staging
        .destination
        .as_ref()
        .ok_or_else(|| invalid("checkout destination proof absent"))?;
    if proof.backend != TransitionBackendKind::Ordinary {
        return Err(invalid("checkout destination is not ordinary"));
    }
    if files.len() > MAX_FILES
        || deletions.len() > MAX_FILES
        || files
            .len()
            .checked_add(deletions.len())
            .is_none_or(|n| n > MAX_FILES)
    {
        return Err(invalid("checkout inventory count exceeds bound"));
    }
    // A borrowed pass settles lengths/limits and modes before cloning paths,
    // building alias keys, serializing payloads, or allocating inventory arrays.
    let mut path_bytes = 0usize;
    let mut file_bytes = 0u64;
    for f in files {
        cancellation::check(cancel)?;
        codec::file(f)?;
        path_bytes = add_path_bytes(path_bytes, f.path.as_str().len())?;
        file_bytes = file_bytes
            .checked_add(f.bytes)
            .filter(|n| *n <= 4 * 1073741824)
            .ok_or_else(|| invalid("checkout image exceeds 4GiB"))?;
    }
    for path in deletions {
        cancellation::check(cancel)?;
        codec::path(path)?;
        path_bytes = add_path_bytes(path_bytes, path.as_str().len())?;
    }
    let mut leaves = BTreeMap::<String, &str>::new();
    for path in files.iter().map(|f| &f.path).chain(deletions) {
        cancellation::check(cancel)?;
        if leaves
            .insert(path.as_str().to_uppercase(), path.as_str())
            .is_some()
        {
            return Err(invalid(
                "checkout inventory has duplicate or aliased leaves",
            ));
        }
    }
    for (key, _) in &leaves {
        for (n, _) in key.match_indices('/') {
            if leaves.contains_key(&key[..n]) {
                return Err(invalid("checkout leaf contains another checkout leaf"));
            }
        }
    }
    let mut dirs = BTreeMap::<String, RepoPath>::new();
    let mut directory_bytes = 0usize;
    for file in files {
        cancellation::check(cancel)?;
        for (end, _) in file.path.as_str().match_indices('/') {
            let original = &file.path.as_str()[..end];
            let key = original.to_uppercase();
            if let Some(prior) = dirs.get(&key) {
                if prior.as_str() != original {
                    return Err(invalid(
                        "checkout directory spelling aliases another parent",
                    ));
                }
                continue;
            }
            if dirs.len() >= MAX_DIRECTORIES {
                return Err(invalid("checkout directory count exceeds bound"));
            }
            directory_bytes = directory_bytes
                .checked_add(original.len())
                .filter(|n| *n <= MAX_JOURNAL_BYTES)
                .ok_or_else(|| invalid("checkout derived directory metadata exceeds bound"))?;
            dirs.insert(
                key,
                RepoPath::parse(original).map_err(|e| invalid(e.to_string()))?,
            );
        }
    }
    let mut directories: Vec<_> = dirs.into_values().collect();
    directories.sort();
    let mut files = files.to_vec();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut deletions = deletions.to_vec();
    deletions.sort();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-checkout-inventory-v1\0");
    hash.update(&(directories.len() as u64).to_be_bytes());
    hash.update(&(files.len() as u64).to_be_bytes());
    hash.update(&(deletions.len() as u64).to_be_bytes());
    for item in directories
        .iter()
        .map(InventoryItem::Directory)
        .chain(files.iter().map(InventoryItem::File))
        .chain(deletions.iter().map(InventoryItem::Deletion))
    {
        cancellation::check(cancel)?;
        let encoded = codec::canonical(&item)?;
        hash.update(&(encoded.len() as u64).to_be_bytes());
        hash.update(&encoded);
    }
    let control: ControlProjection =
        serde_json::from_slice(&proof.format.bytes()?).map_err(|e| invalid(e.to_string()))?;
    let shared = proof
        .format
        .shared
        .as_ref()
        .ok_or_else(|| invalid("checkout family binding absent"))?;
    let header = CheckoutHeader {
        schema_version: HEADER_SCHEMA.into(),
        transaction_id: staging.transaction_id.clone(),
        plan_digest: staging.plan.digest()?,
        prefix_id: staging.plan.source_prefix_id.clone(),
        destination_proof_digest: proof.digest(&staging.plan)?,
        image_digest: image_digest.into(),
        checkout_identity: proof.format.checkout_identity().into(),
        control_identity: control.store_identity,
        metadata_identity: proof.format.metadata_identity.clone(),
        parent_identity: staging.plan.destination_parent_identity.clone(),
        family_root_identity: shared.root_identity.clone(),
        database_identity: proof.private_database_identity.clone(),
        format_file_identity: proof.format_file_identity.clone(),
        inventory_digest: format!("blake3:{}", hash.finalize()),
        files: files.len() as u64,
        directories: directories.len() as u64,
        deletions: deletions.len() as u64,
        file_bytes,
        path_bytes: path_bytes as u64,
    };
    codec::header(&header)?;
    let header_digest = codec::hash(
        "layrs-provider-checkout-header-v1",
        &codec::canonical(&header)?,
    );
    let operation = format!("provider-checkout:{}", header.transaction_id);
    let mut expected = ExpectedCheckout {
        marker: proof.format.clone(),
        header,
        header_digest,
        operation,
        files,
        directories,
        deletions,
        declared_encoded_bytes: 0,
        maximum_encoded_bytes: 0,
    };
    let mut declared = 0usize;
    let mut maximum = 0usize;
    for key in ordered_keys(&expected) {
        cancellation::check(cancel)?;
        let row = declared_row(&expected, key)?;
        declared = add_encoded(declared, row.encode()?.len(), &expected, key)?;
        let mut worst = row;
        match &mut worst.entry {
            CheckoutEntry::Stage { state } | CheckoutEntry::Directory { state, .. } => {
                *state = DirectoryState::Owned {
                    identity: "fs:v1:ffffffffffffffff:ffffffffffffffff".into(),
                }
            }
            CheckoutEntry::File { state, .. } => {
                *state = FileState::Publishing {
                    inode: "4294967295:4294967295:4294967295".into(),
                    parent_identity: "fs:v1:ffffffffffffffff:ffffffffffffffff".into(),
                }
            }
            _ => {}
        }
        maximum = add_encoded(maximum, worst.encode()?.len(), &expected, key)?;
    }
    expected.declared_encoded_bytes = declared as u64;
    expected.maximum_encoded_bytes = maximum as u64;
    Ok(expected)
}

pub(super) fn read_ready(
    c: &Connection,
    ready: &ProviderTransitionIntentRecord,
    deletions: &[RepoPath],
    cancel: Option<&Cancellation>,
) -> Result<(ExpectedCheckout, CheckoutJournal)> {
    read_ready_parts(c, ready, deletions, SourceKind::Provider, cancel)
}

impl ExpectedCheckout {
    /// Terminal ordinary inventory only. No assertion of actual B Staging or
    /// Provider bootstrap identity is made by this calculation constructor.
    pub(in crate::projection::provider) fn from_ready_ordinary(
        ready: &ProviderTransitionIntentRecord,
        files: &[CheckoutFile],
        deletions: &[RepoPath],
        image_digest: &str,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        expected_ready_parts(
            ready,
            files,
            deletions,
            image_digest,
            SourceKind::AdmittedOrdinary,
            cancel,
        )
    }

    pub(in crate::projection::provider) fn read_ready_ordinary(
        c: &Connection,
        ready: &ProviderTransitionIntentRecord,
        deletions: &[RepoPath],
        cancel: Option<&Cancellation>,
    ) -> Result<(Self, CheckoutJournal)> {
        read_ready_parts(c, ready, deletions, SourceKind::AdmittedOrdinary, cancel)
    }
}

fn read_ready_parts(
    c: &Connection,
    ready: &ProviderTransitionIntentRecord,
    deletions: &[RepoPath],
    source: SourceKind,
    cancel: Option<&Cancellation>,
) -> Result<(ExpectedCheckout, CheckoutJournal)> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "checkout terminal reader requires retained transaction",
        ));
    }
    ready.validate_fields()?;
    let marker = &ready
        .destination
        .as_ref()
        .ok_or_else(|| invalid("checkout Ready destination absent"))?
        .format;
    import_validation::require_schema(c, marker, ImportDatabaseKind::Provider, cancel)?;
    let operation = format!("provider-checkout:{}", ready.transaction_id);
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_prejournal LIMIT 200003)",
        [],
        |r| r.get(0),
    )?;
    if count > MAX_ROWS as u64 {
        return Err(invalid("checkout terminal row bound exceeded"));
    }
    if c.query_row("SELECT EXISTS(SELECT 1 FROM projection_prejournal WHERE length(CAST(path AS BLOB))>128 OR length(CAST(operation AS BLOB))>64 OR length(CAST(digest AS BLOB))!=71 OR length(payload)>8192 OR length(payload)=0 OR state!='owned' OR operation!=?1)",[&operation],|r|r.get::<_,bool>(0))? {
        return Err(invalid("checkout terminal SQL scalar or operation differs"));
    }
    let bytes:u64=c.query_row("SELECT coalesce(sum(length(CAST(path AS BLOB))+length(CAST(operation AS BLOB))+length(CAST(digest AS BLOB))+length(payload)+length(CAST(state AS BLOB))),0) FROM projection_prejournal",[],|r|r.get(0))?;
    if bytes > MAX_JOURNAL_BYTES as u64 {
        return Err(invalid("checkout terminal aggregate bytes exceed bound"));
    }
    let mut statement =
        c.prepare("SELECT digest,payload FROM projection_prejournal ORDER BY path COLLATE BINARY")?;
    let mut rows = statement.query([])?;
    let mut files = Vec::new();
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let digest: String = row.get(0)?;
        let payload: Vec<u8> = row.get(1)?;
        let record = CheckoutRow::decode(&payload)?;
        if record.digest()? != digest {
            return Err(invalid("checkout terminal row digest differs"));
        }
        if let CheckoutEntry::File { file, .. } = record.entry {
            if files.len() >= MAX_FILES {
                return Err(invalid("checkout terminal files exceed bound"));
            }
            files.push(file);
        }
    }
    let image = match (&ready.plan.source_prefix.source, source) {
        (ImportSource::Provider { version, .. }, SourceKind::Provider)
        | (
            ImportSource::Ordinary {
                private_schema: 9 | 10,
                admission: Some(_),
                version,
                ..
            },
            SourceKind::AdmittedOrdinary,
        ) => &version.effective_image_digest,
        _ => {
            return Err(invalid(
                "checkout Ready origin differs from terminal reader",
            ));
        }
    };
    let expected = expected_ready_parts(ready, &files, deletions, image, source, cancel)?;
    let journal = super::sql::read(c, &expected, cancel)?;
    if !journal.is_complete() {
        return Err(invalid("checkout terminal journal is incomplete"));
    }
    Ok((expected, journal))
}
fn add_path_bytes(sum: usize, bytes: usize) -> Result<usize> {
    sum.checked_add(bytes)
        .filter(|n| *n <= MAX_PATH_BYTES)
        .ok_or_else(|| invalid("checkout aggregate paths exceed 8MiB"))
}
fn add_encoded(
    sum: usize,
    payload: usize,
    expected: &ExpectedCheckout,
    key: RowKey,
) -> Result<usize> {
    let columns = payload + sql_key(expected, key)?.len() + expected.operation.len() + 71 + 5;
    sum.checked_add(columns)
        .filter(|n| *n <= MAX_JOURNAL_BYTES)
        .ok_or_else(|| invalid("checkout encoded journal exceeds 64MiB"))
}
pub(super) fn ordered_keys(expected: &ExpectedCheckout) -> impl Iterator<Item = RowKey> + '_ {
    (0..expected.directories.len())
        .map(|n| RowKey::Directory(n as u32))
        .chain((0..expected.files.len()).map(|n| RowKey::File(n as u32)))
        .chain([RowKey::Header, RowKey::Stage])
}
pub(super) fn declared_row(expected: &ExpectedCheckout, key: RowKey) -> Result<CheckoutRow> {
    let entry = match key {
        RowKey::Header => CheckoutEntry::Header {
            header: expected.header.clone(),
        },
        RowKey::Stage => CheckoutEntry::Stage {
            state: DirectoryState::Declared,
        },
        RowKey::Directory(n) => CheckoutEntry::Directory {
            ordinal: n,
            path: expected
                .directories
                .get(n as usize)
                .ok_or_else(|| invalid("checkout directory ordinal absent"))?
                .clone(),
            state: DirectoryState::Declared,
        },
        RowKey::File(n) => CheckoutEntry::File {
            ordinal: n,
            file: expected
                .files
                .get(n as usize)
                .ok_or_else(|| invalid("checkout file ordinal absent"))?
                .clone(),
            state: FileState::Declared,
        },
    };
    Ok(CheckoutRow {
        schema_version: ROW_SCHEMA.into(),
        header_digest: expected.header_digest.clone(),
        entry,
    })
}
pub(super) fn sql_key(expected: &ExpectedCheckout, key: RowKey) -> Result<String> {
    let suffix = match key {
        RowKey::Header => "header".into(),
        RowKey::Stage => "stage".into(),
        RowKey::Directory(n) if (n as usize) < expected.directories.len() => format!("dir:{n:06}"),
        RowKey::File(n) if (n as usize) < expected.files.len() => format!("file:{n:06}"),
        _ => return Err(invalid("checkout SQL key ordinal absent")),
    };
    Ok(format!("{}:{suffix}", expected.operation))
}
pub(super) fn require_expected(expected: &ExpectedCheckout, row: &CheckoutRow) -> Result<()> {
    codec::validate(row)?;
    let mut normalized = row.clone();
    match &mut normalized.entry {
        CheckoutEntry::Stage { state } | CheckoutEntry::Directory { state, .. } => {
            *state = DirectoryState::Declared
        }
        CheckoutEntry::File { state, .. } => *state = FileState::Declared,
        _ => {}
    }
    if normalized != declared_row(expected, row.key())? {
        return Err(invalid("checkout row differs from expected inventory"));
    }
    Ok(())
}
