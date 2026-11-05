//! Borrowed-transaction SQL primitives. This module opens/commits/closes nothing
//! and grants no writer authority. A caller must retain the established source
//! or destination writer gates, rollback, positive close and fresh strict read.
//! It must protect the complete exact union before publishing either receipt.
use super::*;
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};

const HEADER_SELECT: &str = "SELECT id,file_count,deletion_count,files_digest,deletions_digest,payload FROM projection_provider_companions WHERE id=?1";
const FILE_SELECT: &str = "SELECT companion_id,path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason,digest FROM projection_provider_companion_files WHERE companion_id=?1 AND path=?2";
const DELETION_SELECT: &str = "SELECT companion_id,path,base_file_id,base_executable,base_manifest_id,reason,digest FROM projection_provider_companion_deletions WHERE companion_id=?1 AND path=?2";
const HEADER_INSERT: &str = "INSERT INTO projection_provider_companions(id,file_count,deletion_count,files_digest,deletions_digest,payload) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING";
const FILE_INSERT: &str = "INSERT INTO projection_provider_companion_files(companion_id,path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason,digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT DO NOTHING";
const DELETION_INSERT: &str = "INSERT INTO projection_provider_companion_deletions(companion_id,path,base_file_id,base_executable,base_manifest_id,reason,digest) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO NOTHING";

impl OwnedCompanionData {
    /// Source-only primitive; the caller has already authenticated the frozen
    /// prefix and absent outgoing rowset, and will insert claim/receipt/CAS in
    /// THIS transaction. Existing mappings are compared byte-for-byte.
    pub(in crate::projection::provider) fn insert_source_mappings_and_rows(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        require_transaction(c)?;
        for (file, (manifest, bytes)) in &self.source_mappings {
            cancellation::check(cancel)?;
            let values = vec![
                Value::Text(file.to_string()),
                Value::Text(manifest.to_string()),
                integer(*bytes)?,
            ];
            insert_exact(
                c,
                "INSERT INTO projection_content(file_id,manifest_id,bytes) VALUES(?1,?2,?3)",
                "SELECT file_id,manifest_id,bytes FROM projection_content WHERE file_id=?1",
                &values[..1],
                &values,
            )?;
        }
        self.insert_new_rows(c, cancel)
    }

    /// Persist identical canonical rows at SOURCE and DESTINATION, in their
    /// separately owned transactions. No receipt is written here. Before this
    /// call, the driver must acquire/source-own every listed mapping and exact
    /// retention claim; a destination claim alone does not own source history.
    /// Existing rows are accepted only byte-for-byte, with exact cardinality.
    pub(in crate::projection::provider) fn insert_new_rows(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        require_transaction(c)?;
        cancellation::check(cancel)?;
        self.require_mappings(c, cancel)?;
        let Some(header) = &self.header else {
            return Ok(());
        };
        header.validate_rows(&self.files, &self.deletions)?;
        // Compute ALL bounded encodings and expected row projections before
        // the first effect. Errors must still roll back the caller transaction.
        let id = header.digest()?;
        let header_row = header_values(
            &id,
            header.file_count,
            header.deletion_count,
            &header.files_digest,
            &header.deletions_digest,
            header.encode()?,
        )?;
        let files = self
            .files
            .iter()
            .map(|r| file_values(&id, r))
            .collect::<Result<Vec<_>>>()?;
        let deletions = self
            .deletions
            .iter()
            .map(|r| deletion_values(&id, r))
            .collect::<Result<Vec<_>>>()?;
        insert_exact(
            c,
            HEADER_INSERT,
            HEADER_SELECT,
            &header_row[..1],
            &header_row,
        )?;
        for row in &files {
            cancellation::check(cancel)?;
            insert_exact(c, FILE_INSERT, FILE_SELECT, &row[..2], row)?;
        }
        for row in &deletions {
            cancellation::check(cancel)?;
            insert_exact(c, DELETION_INSERT, DELETION_SELECT, &row[..2], row)?;
        }
        self.require_new_rows(c, cancel)
    }

    /// For the fresh strict reader AFTER positive writer close as well as the
    /// in-transaction readback. Equality includes the v2 provenance BLOB, every
    /// SQL scalar, exact mapping and absence of extra rows under this header.
    pub(in crate::projection::provider) fn require_new_rows(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        require_transaction(c)?;
        cancellation::check(cancel)?;
        self.require_mappings(c, cancel)?;
        let Some(header) = &self.header else {
            return Ok(());
        };
        let id = header.digest()?;
        let expected = header_values(
            &id,
            header.file_count,
            header.deletion_count,
            &header.files_digest,
            &header.deletions_digest,
            header.encode()?,
        )?;
        require_exact(c, HEADER_SELECT, &expected[..1], &expected)?;
        require_counts(c, &id, self.files.len(), self.deletions.len())?;
        for row in &self.files {
            cancellation::check(cancel)?;
            let expected = file_values(&id, row)?;
            require_exact(c, FILE_SELECT, &expected[..2], &expected)?;
        }
        for row in &self.deletions {
            cancellation::check(cancel)?;
            let expected = deletion_values(&id, row)?;
            require_exact(c, DELETION_SELECT, &expected[..2], &expected)?;
        }
        Ok(())
    }

    /// Validate the destination copy of original inherited v1/v2 artifacts. The
    /// existing closure copier remains responsible for their insertion order.
    /// No payload, source schema, base or receipt is rewritten to current data.
    pub(in crate::projection::provider) fn require_inherited_rows(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        require_transaction(c)?;
        for inherited in &self.inherited {
            cancellation::check(cancel)?;
            let header = inherited.header.as_ref();
            let columns = header.columns();
            let id = header.digest()?;
            let expected = header_values(
                &id,
                columns.file_count,
                columns.deletion_count,
                columns.files_digest,
                columns.deletions_digest,
                header.encode()?,
            )?;
            require_exact(c, HEADER_SELECT, &expected[..1], &expected)?;
            require_counts(c, &id, inherited.files.len(), inherited.deletions.len())?;
            for row in &inherited.files {
                cancellation::check(cancel)?;
                let expected = file_values(&id, row)?;
                require_exact(c, FILE_SELECT, &expected[..2], &expected)?;
            }
            for row in &inherited.deletions {
                cancellation::check(cancel)?;
                let expected = deletion_values(&id, row)?;
                require_exact(c, DELETION_SELECT, &expected[..2], &expected)?;
            }
        }
        Ok(())
    }

    fn require_mappings(&self, c: &Connection, cancel: Option<&Cancellation>) -> Result<()> {
        for (file, (manifest, bytes)) in &self.source_mappings {
            cancellation::check(cancel)?;
            let expected = vec![
                Value::Text(file.to_string()),
                Value::Text(manifest.to_string()),
                integer(*bytes)?,
            ];
            require_exact(
                c,
                "SELECT file_id,manifest_id,bytes FROM projection_content WHERE file_id=?1",
                &expected[..1],
                &expected,
            )?;
        }
        Ok(())
    }
}

fn require_transaction(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(invalid(
            "companion SQL requires retained caller transaction",
        ));
    }
    Ok(())
}
fn integer(value: u64) -> Result<Value> {
    Ok(Value::Integer(
        i64::try_from(value).map_err(|_| invalid("companion SQL integer overflow"))?,
    ))
}
fn reason(reason: CompanionReason) -> Result<Value> {
    let value = serde_json::to_value(reason).map_err(|e| invalid(e.to_string()))?;
    Ok(Value::Text(
        value
            .as_str()
            .ok_or_else(|| invalid("companion reason is not a token"))?
            .to_owned(),
    ))
}
fn optional_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |s| Value::Text(s.to_owned()))
}
fn header_values(
    id: &str,
    files: u64,
    deletions: u64,
    files_digest: &str,
    deletions_digest: &str,
    payload: Vec<u8>,
) -> Result<Vec<Value>> {
    // Dispatch is validated before publication; old codecs cannot accidentally
    // treat a v2 intention as a v1 effective-base deletion.
    match CompanionHeaderRecord::decode(&payload)? {
        CompanionHeaderRecord::V1(header) if header.digest()? == id => {}
        CompanionHeaderRecord::V2(header) if header.digest()? == id => {}
        _ => return Err(invalid("companion SQL payload ID differs")),
    }
    Ok(vec![
        Value::Text(id.into()),
        integer(files)?,
        integer(deletions)?,
        Value::Text(files_digest.into()),
        Value::Text(deletions_digest.into()),
        Value::Blob(payload),
    ])
}
fn file_values(id: &str, row: &ProviderCompanionFileRecord) -> Result<Vec<Value>> {
    Ok(vec![
        Value::Text(id.into()),
        Value::Text(row.path.to_string()),
        Value::Text(row.file_id.to_string()),
        integer(row.bytes)?,
        Value::Integer(i64::from(row.executable)),
        Value::Text(row.manifest_id.to_string()),
        optional_text(row.base_file_id.as_ref().map(FileId::as_str)),
        row.base_executable
            .map_or(Value::Null, |b| Value::Integer(i64::from(b))),
        reason(row.reason)?,
        Value::Text(row.digest()?),
    ])
}
fn deletion_values(id: &str, row: &ProviderCompanionDeletionRecord) -> Result<Vec<Value>> {
    Ok(vec![
        Value::Text(id.into()),
        Value::Text(row.path.to_string()),
        Value::Text(row.base_file_id.to_string()),
        Value::Integer(i64::from(row.base_executable)),
        optional_text(row.base_manifest_id.as_ref().map(FileManifestId::as_str)),
        reason(row.reason)?,
        Value::Text(row.digest()?),
    ])
}
fn existing(c: &Connection, select: &str, key: &[Value]) -> Result<Option<Vec<Value>>> {
    Ok(c.query_row(select, params_from_iter(key), |row| {
        (0..row.as_ref().column_count())
            .map(|i| row.get(i))
            .collect::<rusqlite::Result<Vec<Value>>>()
    })
    .optional()?)
}
fn require_exact(c: &Connection, select: &str, key: &[Value], expected: &[Value]) -> Result<()> {
    if existing(c, select, key)?.as_deref() != Some(expected) {
        return Err(invalid("companion SQL exact row readback differs"));
    }
    Ok(())
}
fn insert_exact(
    c: &Connection,
    insert: &str,
    select: &str,
    key: &[Value],
    expected: &[Value],
) -> Result<()> {
    if let Some(actual) = existing(c, select, key)? {
        if actual != expected {
            return Err(invalid("companion SQL existing row conflicts"));
        }
    } else {
        c.execute(insert, params_from_iter(expected))?;
    }
    require_exact(c, select, key, expected)
}
fn require_counts(c: &Connection, id: &str, files: usize, deletions: usize) -> Result<()> {
    let actual: (u64,u64) = c.query_row("SELECT (SELECT count(*) FROM projection_provider_companion_files WHERE companion_id=?1),(SELECT count(*) FROM projection_provider_companion_deletions WHERE companion_id=?1)", [id], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if actual != (files as u64, deletions as u64) {
        return Err(invalid("companion SQL exact row cardinality differs"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::provider::{database_io::with_reference, owned_test as child};

    #[test]
    #[ignore = "owned SQL scalar insertion recipe; no manufactured Ready/publication proof"]
    fn companion_deletion_first_insert_uses_actual_schema_and_replays_without_mutation() {
        child::run(
            concat!(
                module_path!(),
                "::companion_deletion_first_insert_uses_actual_schema_and_replays_without_mutation"
            ),
            120,
            || {
                child::declare();
                with_reference("", |c| {
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                c.execute_batch(crate::projection::provider::schema::SQL)?;
                let id = format!("blake3:{}", "1".repeat(64));
                // A scalar parent for this FK-only primitive fixture, not a
                // valid closed companion or authorization/Ready witness.
                c.execute(HEADER_INSERT, rusqlite::params![id, 0, 1, id, id, b"{}".as_slice()])?;
                let row = ProviderCompanionDeletionRecord {
                    schema_version: "layrs.projection-provider-companion-deletion.v1".into(),
                    path: RepoPath::parse("deleted.txt").unwrap(),
                    base_file_id: FileId::parse(&id).unwrap(),
                    base_executable: false,
                    base_manifest_id: Some(FileManifestId::parse(&id).unwrap()),
                    reason: crate::projection::provider::records::import::CompanionReason::RevokedRights,
                };
                let expected = deletion_values(&id, &row)?;
                let key = [Value::Text(id.clone()), Value::Text(row.path.to_string())];
                let changes = c.total_changes();
                insert_exact(c, DELETION_INSERT, DELETION_SELECT, &key, &expected)?;
                assert_eq!(c.total_changes(), changes + 1);
                assert_eq!(existing(c, DELETION_SELECT, &key)?.as_deref(), Some(expected.as_slice()));
                let changes = c.total_changes();
                insert_exact(c, DELETION_INSERT, DELETION_SELECT, &key, &expected)?;
                assert_eq!(c.total_changes(), changes);
                let mut conflict = expected;
                conflict[6] = Value::Text(id.clone());
                assert!(insert_exact(c, DELETION_INSERT, DELETION_SELECT, &key, &conflict).is_err());
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            }).unwrap();
            },
        );
    }
}
