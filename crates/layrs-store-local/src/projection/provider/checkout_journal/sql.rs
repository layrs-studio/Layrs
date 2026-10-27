use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum DeclareEffect {
    Inserted,
    AlreadyPresent,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum AdvanceEffect {
    Updated,
    AlreadyPresent,
}

fn transaction(
    c: &Connection,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid("checkout journal requires retained transaction"));
    }
    import_validation::require_schema(c, &expected.marker, ImportDatabaseKind::Provider, cancel)
}
fn writer(c: &Connection) -> Result<()> {
    if c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1 {
        return Err(invalid("checkout journal writer requires FK ON"));
    }
    Ok(())
}
fn count(c: &Connection) -> Result<usize> {
    let count: usize = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_prejournal LIMIT ?1)",
        [MAX_ROWS + 1],
        |r| r.get(0),
    )?;
    if count > MAX_ROWS {
        return Err(invalid("checkout journal row count exceeds bound"));
    }
    Ok(count)
}
fn preflight(c: &Connection, expected: &ExpectedCheckout) -> Result<()> {
    if count(c)? != expected.directories.len() + expected.files.len() + 2 {
        return Err(invalid(
            "checkout journal is incomplete or has foreign rows",
        ));
    }
    let bad: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_prejournal WHERE \
         length(CAST(path AS BLOB))>128 OR length(CAST(operation AS BLOB))>64 \
         OR length(CAST(digest AS BLOB))!=71 OR length(payload)>8192 OR length(payload)=0 \
         OR state!='owned' OR operation!=?1)",
        [&expected.operation],
        |r| r.get(0),
    )?;
    if bad {
        return Err(invalid("checkout journal SQL scalar or operation differs"));
    }
    let bytes:u64=c.query_row(
        "SELECT coalesce(sum(length(CAST(path AS BLOB))+length(CAST(operation AS BLOB))+length(CAST(digest AS BLOB))+length(payload)+length(CAST(state AS BLOB))),0) FROM projection_prejournal",[],|r|r.get(0))?;
    if bytes > expected.maximum_encoded_bytes || bytes > MAX_JOURNAL_BYTES as u64 {
        return Err(invalid("checkout journal aggregate bytes exceed bound"));
    }
    Ok(())
}

/// Strict complete inventory read, including every row projection and all
/// ownership/parent relationships. Bounds precede observed String/Vec reads.
pub(in crate::projection::provider) fn read(
    c: &Connection,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<CheckoutJournal> {
    transaction(c, expected, cancel)?;
    preflight(c, expected)?;
    let mut statement=c.prepare("SELECT path,operation,digest,payload,state FROM projection_prejournal ORDER BY path COLLATE BINARY")?;
    let mut rows = statement.query([])?;
    let mut values = Vec::with_capacity(expected.directories.len() + expected.files.len() + 2);
    for key in inventory::ordered_keys(expected) {
        cancellation::check(cancel)?;
        let row = rows
            .next()?
            .ok_or_else(|| invalid("checkout journal row disappeared"))?;
        let path: String = row.get(0)?;
        let operation: String = row.get(1)?;
        let digest: String = row.get(2)?;
        let payload: Vec<u8> = row.get(3)?;
        let state: String = row.get(4)?;
        let record = CheckoutRow::decode(&payload)?;
        if path != expected.sql_key(key)?
            || operation != expected.operation
            || state != "owned"
            || record.key() != key
            || record.digest()? != digest
        {
            return Err(invalid("checkout journal SQL projection differs"));
        }
        inventory::require_expected(expected, &record)?;
        values.push(record);
    }
    if rows.next()?.is_some() {
        return Err(invalid("checkout journal has unexpected tail"));
    }
    let journal = CheckoutJournal {
        rows: values,
        directories: expected.directories.len(),
        files: expected.files.len(),
    };
    relationships(expected, &journal, cancel)?;
    Ok(journal)
}

/// All initial declarations are inserted in one caller transaction before
/// filesystem allocation. Exact inventory replay preserves typed progress and
/// performs no mutation, including on an RO/query_only connection with FK OFF.
pub(in crate::projection::provider) fn declare(
    c: &Connection,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<DeclareEffect> {
    transaction(c, expected, cancel)?;
    if count(c)? != 0 {
        read(c, expected, cancel)?;
        return Ok(DeclareEffect::AlreadyPresent);
    }
    writer(c)?;
    for key in inventory::ordered_keys(expected) {
        cancellation::check(cancel)?;
        let row = expected.declared_row(key)?;
        if c.execute("INSERT INTO projection_prejournal(path,operation,digest,payload,state) VALUES(?1,?2,?3,?4,'owned')",params![expected.sql_key(key)?,expected.operation,row.digest()?,row.encode()?])?!=1{return Err(invalid("checkout declaration insert differs"));}
    }
    read(c, expected, cancel)?;
    Ok(DeclareEffect::Inserted)
}

/// Exact whole-row CAS. The caller rolls back on any error after mutation.
/// For simplicity each CAS reads the full bounded journal before and after:
/// O(rows) per CAS, hence quadratic total work for a large checkout. No native
/// authority or positive-close acknowledgement is implied by the result.
pub(in crate::projection::provider) fn advance(
    c: &Connection,
    expected: &ExpectedCheckout,
    previous: &CheckoutRow,
    next: &CheckoutRow,
    cancel: Option<&Cancellation>,
) -> Result<AdvanceEffect> {
    transaction(c, expected, cancel)?;
    inventory::require_expected(expected, previous)?;
    inventory::require_expected(expected, next)?;
    require_transition(previous, next)?;
    let mut journal = read(c, expected, cancel)?;
    let current = journal.row(previous.key())?;
    if current == next {
        return Ok(AdvanceEffect::AlreadyPresent);
    }
    if current != previous {
        return Err(invalid("checkout journal exact predecessor differs"));
    }
    writer(c)?;
    let position = journal
        .rows
        .iter()
        .position(|r| r.key() == previous.key())
        .ok_or_else(|| invalid("checkout CAS key absent"))?;
    journal.rows[position] = next.clone();
    relationships(expected, &journal, cancel)?;
    if c.execute(
        "UPDATE projection_prejournal SET digest=?1,payload=?2 WHERE path=?3 AND operation=?4 AND digest=?5 AND payload=?6 AND state='owned'",
        params![next.digest()?,next.encode()?,expected.sql_key(previous.key())?,expected.operation,previous.digest()?,previous.encode()?],
    )?!=1{return Err(invalid("checkout journal exact CAS differs"));}
    let fresh = read(c, expected, cancel)?;
    if fresh.row(next.key())? != next {
        return Err(invalid("checkout journal CAS readback differs"));
    }
    Ok(AdvanceEffect::Updated)
}

fn require_transition(previous: &CheckoutRow, next: &CheckoutRow) -> Result<()> {
    if previous == next {
        return Ok(());
    }
    if previous.header_digest != next.header_digest || previous.key() != next.key() {
        return Err(invalid("checkout transition binding changed"));
    }
    let legal = match (&previous.entry, &next.entry) {
        (CheckoutEntry::Stage { state: a }, CheckoutEntry::Stage { state: b })
        | (CheckoutEntry::Directory { state: a, .. }, CheckoutEntry::Directory { state: b, .. }) => {
            matches!(
                (a, b),
                (DirectoryState::Declared, DirectoryState::Owned { .. })
            )
        }
        (CheckoutEntry::File { state: a, .. }, CheckoutEntry::File { state: b, .. }) => {
            match (a, b) {
                (FileState::Declared, FileState::Owned { .. }) => true,
                (FileState::Owned { inode: a }, FileState::Verified { inode: b }) => a == b,
                (FileState::Verified { inode: a }, FileState::Publishing { inode: b, .. }) => {
                    a == b
                }
                (
                    FileState::Publishing {
                        inode: a,
                        parent_identity: ap,
                    },
                    FileState::Published {
                        inode: b,
                        parent_identity: bp,
                    },
                ) => a == b && ap == bp,
                _ => false,
            }
        }
        _ => false,
    };
    if !legal {
        return Err(invalid("checkout phase transition or inode changed"));
    }
    Ok(())
}

fn directory_identity<'a>(journal: &'a CheckoutJournal, key: RowKey) -> Result<Option<&'a str>> {
    match &journal.row(key)?.entry {
        CheckoutEntry::Stage { state } | CheckoutEntry::Directory { state, .. } => {
            Ok(match state {
                DirectoryState::Declared => None,
                DirectoryState::Owned { identity } => Some(identity),
            })
        }
        _ => Err(invalid("checkout expected directory row differs")),
    }
}
fn parent_identity<'a>(
    expected: &'a ExpectedCheckout,
    journal: &'a CheckoutJournal,
    path: &RepoPath,
) -> Result<&'a str> {
    match path.as_str().rsplit_once('/') {
        None => Ok(&expected.header.checkout_identity),
        Some((parent, _)) => {
            let index = expected
                .directories
                .binary_search_by(|p| p.as_str().cmp(parent))
                .map_err(|_| invalid("checkout parent not in inventory"))?;
            directory_identity(journal, RowKey::Directory(index as u32))?
                .ok_or_else(|| invalid("checkout parent has no durable identity"))
        }
    }
}
fn relationships(
    expected: &ExpectedCheckout,
    journal: &CheckoutJournal,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let h = &expected.header;
    let mut identities = BTreeSet::new();
    for id in [
        &h.checkout_identity,
        &h.control_identity,
        &h.metadata_identity,
        &h.parent_identity,
        &h.family_root_identity,
    ] {
        identities.insert(id.as_str());
    }
    let database_identity = codec::file_as_directory_identity(&h.database_identity)?;
    let format_identity = codec::file_as_directory_identity(&h.format_file_identity)?;
    identities.insert(database_identity.as_str());
    identities.insert(format_identity.as_str());
    let mut directories = BTreeSet::new();
    for row in &journal.rows {
        cancellation::check(cancel)?;
        match &row.entry {
            CheckoutEntry::Stage {
                state: DirectoryState::Owned { identity },
            }
            | CheckoutEntry::Directory {
                state: DirectoryState::Owned { identity },
                ..
            } => {
                if identities.contains(identity.as_str()) || !directories.insert(identity.as_str())
                {
                    return Err(invalid("checkout directory identity aliases another owner"));
                }
                if let CheckoutEntry::Directory { path, .. } = &row.entry {
                    parent_identity(expected, journal, path)?;
                }
            }
            _ => {}
        }
    }
    let stage_owned = directory_identity(journal, RowKey::Stage)?.is_some();
    let mut files = BTreeSet::new();
    for row in &journal.rows {
        cancellation::check(cancel)?;
        if let CheckoutEntry::File { file, state, .. } = &row.entry {
            if let Some(inode) = state.inode() {
                if !stage_owned {
                    return Err(invalid("checkout file has no durable stage directory"));
                }
                let directory_form = codec::file_as_directory_identity(inode)?;
                if inode == h.database_identity
                    || inode == h.format_file_identity
                    || identities.contains(directory_form.as_str())
                    || directories.contains(directory_form.as_str())
                    || !files.insert(inode)
                {
                    return Err(invalid("checkout file inode aliases another owner"));
                }
            }
            if let FileState::Publishing {
                parent_identity: claimed,
                ..
            }
            | FileState::Published {
                parent_identity: claimed,
                ..
            } = state
            {
                if claimed != parent_identity(expected, journal, &file.path)? {
                    return Err(invalid("checkout publication parent identity differs"));
                }
            }
        }
    }
    Ok(())
}
