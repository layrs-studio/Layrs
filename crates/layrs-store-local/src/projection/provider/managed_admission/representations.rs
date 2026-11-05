//! Bounded immutable Ordinary10 union witness. No manifest bodies, family
//! connection or arbitrary FileId mapping can substitute for these triples.
use super::*;
use layrs_core::scm::{FileManifest, canonical_json};
use std::collections::BTreeMap;

pub(super) fn column_bytes(rows: u64) -> Result<u64> {
    rows.checked_mul(schema::REPRESENTATION_COLUMN_BYTES)
        .filter(|bytes| {
            rows <= schema::MAX_REPRESENTATIONS && *bytes <= schema::MAX_REPRESENTATION_COLUMN_BYTES
        })
        .ok_or_else(|| invalid("admitted representation witness exceeds bound"))
}

/// Scalar fencing precedes any admission/prefix decoding or FK traversal.
pub(super) fn preflight(c: &Connection, cancel: Option<&Cancellation>) -> Result<(u64, u64)> {
    cancellation::check(cancel)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_import_representations LIMIT 100001)",
        [],
        |r| r.get(0),
    )?;
    let expected_bytes = column_bytes(count)?;
    if c.query_row("SELECT EXISTS(SELECT 1 FROM projection_import_representations WHERE typeof(file_id)!='text' OR length(CAST(file_id AS BLOB))!=71 OR substr(file_id,1,7)!='blake3:' OR substr(file_id,8) GLOB '*[^0-9a-f]*' OR typeof(manifest_id)!='text' OR length(CAST(manifest_id AS BLOB))!=71 OR substr(manifest_id,1,7)!='blake3:' OR substr(manifest_id,8) GLOB '*[^0-9a-f]*' OR typeof(bytes)!='integer' OR bytes<0 OR bytes>1073741824)", [], |r|r.get::<_,bool>(0))? {
        return Err(invalid("admitted representation witness scalar differs"));
    }
    let bytes: u64 = c.query_row("SELECT coalesce(sum(length(CAST(file_id AS BLOB))+length(CAST(manifest_id AS BLOB))+8),0) FROM projection_import_representations", [], |r|r.get(0))?;
    if bytes != expected_bytes {
        return Err(invalid(
            "admitted representation witness normalized bytes differ",
        ));
    }
    cancellation::check(cancel)?;
    Ok((count, bytes))
}

pub(in crate::projection::provider) fn read_import_representations(
    c: &Connection,
    permit: &schema::AdmittedSchemaPermit<'_>,
    ready: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<Vec<RequiredRepresentation>> {
    let marker = &ready
        .destination
        .as_ref()
        .ok_or_else(|| invalid("admitted witness destination absent"))?
        .format;
    permit.require(c, marker, cancel)?;
    if permit.layout() != schema::AdmittedLayout::Ordinary10 {
        return Err(invalid(
            "admitted representation witness requires Ordinary10",
        ));
    }
    let (count, _) = preflight(c, cancel)?;
    let mut statement = c.prepare("SELECT file_id,manifest_id,bytes FROM projection_import_representations ORDER BY file_id COLLATE BINARY,manifest_id COLLATE BINARY")?;
    let mut rows = statement.query([])?;
    let mut required = Vec::with_capacity(count as usize);
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        if required.len() as u64 >= count {
            return Err(invalid("admitted representation witness row count changed"));
        }
        let file: String = row.get(0)?;
        let manifest: String = row.get(1)?;
        required.push(RequiredRepresentation {
            file_id: FileId::parse(file).map_err(|e| invalid(e.to_string()))?,
            manifest_id: FileManifestId::parse(manifest).map_err(|e| invalid(e.to_string()))?,
            bytes: row.get(2)?,
        });
    }
    if required.len() as u64 != count {
        return Err(invalid("admitted representation witness row count differs"));
    }
    require_rows(&required, &ready.plan.protected_manifests, cancel)?;
    Ok(required)
}

pub(super) fn from_manifests(
    manifests: &[FileManifest],
    ready: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<Vec<RequiredRepresentation>> {
    column_bytes(manifests.len() as u64)?;
    let mut rows: Vec<_> = manifests
        .iter()
        .map(|m| RequiredRepresentation {
            file_id: m.file_id().clone(),
            manifest_id: m.id().clone(),
            bytes: m.byte_length(),
        })
        .collect();
    rows.sort_unstable();
    require_rows(&rows, &ready.plan.protected_manifests, cancel)?;
    Ok(rows)
}

fn require_rows(
    rows: &[RequiredRepresentation],
    protected: &super::super::records::transition::TransitionRowset,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    column_bytes(rows.len() as u64)?;
    let mut lengths = BTreeMap::new();
    let mut previous = None;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
    hash.update(&(rows.len() as u64).to_be_bytes());
    for row in rows {
        cancellation::check(cancel)?;
        let key = (&row.file_id, &row.manifest_id);
        if previous.is_some_and(|prior| prior >= key)
            || row.bytes > 1073741824
            || lengths
                .insert(&row.file_id, row.bytes)
                .is_some_and(|old| old != row.bytes)
        {
            return Err(invalid(
                "admitted representation witness order, uniqueness or length differs",
            ));
        }
        previous = Some(key);
        let bytes = canonical_json(row).map_err(|e| invalid(e.to_string()))?;
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(&bytes);
    }
    if protected.count != rows.len() as u64
        || protected.digest != format!("blake3:{}", hash.finalize())
    {
        return Err(invalid(
            "admitted representation witness differs from Ready union",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::super::records::transition::TransitionRowset;
    use super::*;

    fn row(file: char, manifest: char, bytes: u64) -> RequiredRepresentation {
        RequiredRepresentation {
            file_id: FileId::parse(format!("blake3:{}", file.to_string().repeat(64))).unwrap(),
            manifest_id: FileManifestId::parse(format!(
                "blake3:{}",
                manifest.to_string().repeat(64)
            ))
            .unwrap(),
            bytes,
        }
    }

    fn commitment(rows: &[RequiredRepresentation]) -> TransitionRowset {
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
        hash.update(&(rows.len() as u64).to_be_bytes());
        for row in rows {
            let encoded = canonical_json(row).unwrap();
            hash.update(&(encoded.len() as u64).to_be_bytes());
            hash.update(&encoded);
        }
        TransitionRowset {
            count: rows.len() as u64,
            digest: format!("blake3:{}", hash.finalize()),
        }
    }

    #[test]
    fn witness_retains_two_representations_of_one_file_and_rejects_lost_or_extra_triples() {
        let rows = vec![row('a', '1', 42), row('a', '2', 42)];
        let protected = commitment(&rows);
        require_rows(&rows, &protected, None).unwrap();
        assert!(require_rows(&rows[..1], &protected, None).is_err());
        let mut extra = rows.clone();
        extra.push(row('b', '3', 42));
        assert!(require_rows(&extra, &protected, None).is_err());
        let mut changed = rows.clone();
        changed[1].manifest_id = row('a', '3', 42).manifest_id;
        assert!(require_rows(&changed, &protected, None).is_err());
    }

    #[test]
    fn a_self_consistent_digest_cannot_hide_duplicates_order_or_contradictory_lengths() {
        for rows in [
            vec![row('a', '1', 42), row('a', '1', 42)],
            vec![row('a', '2', 42), row('a', '1', 42)],
            vec![row('a', '1', 42), row('a', '2', 43)],
            vec![row('a', '1', 1073741825)],
        ] {
            assert!(require_rows(&rows, &commitment(&rows), None).is_err());
        }
        assert_eq!(column_bytes(100_000).unwrap(), 15_000_000);
        assert!(column_bytes(100_001).is_err());
        assert!(column_bytes(u64::MAX).is_err());
    }
}
