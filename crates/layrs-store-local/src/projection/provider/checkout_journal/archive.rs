//! Permanent private9 archive interpreter. No operational prejournal selector,
//! writer, migration, filesystem check or adoption is available through this API.
use super::super::managed_admission::{
    schema::{AdmittedLayout, AdmittedSchemaPermit},
    ArchiveCommitment,
};
use super::*;

pub(in crate::projection::provider) fn read_archived(
    c: &Connection,
    ready: &ProviderTransitionIntentRecord,
    deletions: &[RepoPath],
    permit: &AdmittedSchemaPermit<'_>,
    cancel: Option<&Cancellation>,
) -> Result<(ExpectedCheckout, CheckoutJournal, ArchiveCommitment)> {
    cancellation::check(cancel)?;
    ready.validate_fields()?;
    let marker = &ready
        .destination
        .as_ref()
        .ok_or_else(|| invalid("archived checkout Ready destination absent"))?
        .format;
    permit.require(c, marker, cancel)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_import_checkout LIMIT 200003)",
        [],
        |r| r.get(0),
    )?;
    if count > MAX_ROWS as u64 {
        return Err(invalid("archived checkout row bound exceeded"));
    }
    let operation = format!("provider-checkout:{}", ready.transaction_id);
    if c.query_row("SELECT EXISTS(SELECT 1 FROM projection_import_checkout WHERE length(CAST(path AS BLOB))=0 OR length(CAST(path AS BLOB))>128 OR length(CAST(operation AS BLOB))>64 OR length(CAST(digest AS BLOB))!=71 OR length(payload)>8192 OR length(payload)=0 OR state!='owned' OR operation!=?1)",[&operation],|r|r.get::<_,bool>(0))? {
        return Err(invalid("archived checkout SQL scalar or operation differs"));
    }
    let bytes:u64=c.query_row("SELECT coalesce(sum(length(CAST(path AS BLOB))+length(CAST(operation AS BLOB))+length(CAST(digest AS BLOB))+length(payload)+length(CAST(state AS BLOB))),0) FROM projection_import_checkout",[],|r|r.get(0))?;
    if bytes > MAX_JOURNAL_BYTES as u64 {
        return Err(invalid("archived checkout aggregate byte bound exceeded"));
    }
    let mut statement=c.prepare("SELECT path,operation,digest,payload,state FROM projection_import_checkout ORDER BY path COLLATE BINARY")?;
    let mut rows = statement.query([])?;
    let mut values = Vec::with_capacity(count as usize);
    let mut files = Vec::new();
    let mut sql_keys = Vec::with_capacity(count as usize);
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-managed-checkout-archive-v1\0");
    hash.update(&count.to_be_bytes());
    let mut observed = 0u64;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let (path, op, digest, payload, state): (String, String, String, Vec<u8>, String) = (
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        );
        let record = CheckoutRow::decode(&payload)?;
        if record.digest()? != digest {
            return Err(invalid("archived checkout row digest differs"));
        }
        hash.update(&5u32.to_be_bytes());
        for (kind, column) in [
            (2u8, path.as_bytes()),
            (2, op.as_bytes()),
            (2, digest.as_bytes()),
            (3, payload.as_slice()),
            (2, state.as_bytes()),
        ] {
            hash.update(&[kind]);
            hash.update(&(column.len() as u64).to_be_bytes());
            hash.update(column);
            observed = observed
                .checked_add(column.len() as u64)
                .ok_or_else(|| invalid("archived checkout byte overflow"))?;
        }
        if let CheckoutEntry::File { file, .. } = &record.entry {
            if files.len() >= MAX_FILES {
                return Err(invalid("archived checkout file count exceeds bound"));
            }
            files.push(file.clone());
        }
        sql_keys.push(path);
        values.push(record);
    }
    if values.len() as u64 != count || observed != bytes {
        return Err(invalid("archived checkout rowset changed after preflight"));
    }
    let expected = match (&ready.plan.source_prefix.source, permit.layout()) {
        (ImportSource::Provider { version, .. }, AdmittedLayout::Provider9) => {
            ExpectedCheckout::from_ready(
                ready,
                &files,
                deletions,
                &version.effective_image_digest,
                cancel,
            )?
        }
        (
            ImportSource::Ordinary {
                private_schema: 9 | 10,
                admission: Some(_),
                version,
                ..
            },
            AdmittedLayout::Ordinary10,
        ) => ExpectedCheckout::from_ready_ordinary(
            ready,
            &files,
            deletions,
            &version.effective_image_digest,
            cancel,
        )?,
        _ => return Err(invalid("archived checkout origin/admitted layout differs")),
    };
    if values.len() != expected.directories.len() + expected.files.len() + 2 {
        return Err(invalid("archived checkout inventory count differs"));
    }
    for ((key, row), sql_key) in inventory::ordered_keys(&expected)
        .zip(&values)
        .zip(&sql_keys)
    {
        cancellation::check(cancel)?;
        if row.key() != key || expected.sql_key(key)? != *sql_key {
            return Err(invalid("archived checkout SQL key differs"));
        }
        inventory::require_expected(&expected, row)?;
    }
    let journal = CheckoutJournal {
        rows: values,
        directories: expected.directories.len(),
        files: expected.files.len(),
    };
    sql::relationships(&expected, &journal, cancel)?;
    if !journal.is_complete() {
        return Err(invalid(
            "archived checkout requires complete Published history",
        ));
    }
    let commitment = ArchiveCommitment {
        count,
        bytes,
        digest: format!("blake3:{}", hash.finalize()),
    };
    Ok((expected, journal, commitment))
}
