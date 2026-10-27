//! Bounded normalized SQL rowset commitments. This module has no path or
//! content capability; callers first authenticate schema and rowset scope.
use super::*;
use layrs_core::scm::{FileId, FileManifestId};
use rusqlite::{types::ValueRef, Connection, ToSql};
#[cfg(test)]
#[path = "rows_tests.rs"]
mod tests;

const MAX_CLOSURE_ROWS: u64 = 1_000_000;
const MAX_CLOSURE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Snapshots,
    Selections,
    Captures,
    Publications,
    Catalogs,
    Descriptors,
    ActivationMaps,
    TransitionReceipts,
    Companions,
    CompanionFiles,
    CompanionDeletions,
    ContentMappings,
    RestoreHistory,
}
impl Kind {
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Snapshots => b"layrs-provider-import-snapshots-v1\0",
            Self::Selections => b"layrs-provider-import-selections-v1\0",
            Self::Captures => b"layrs-provider-import-captures-v1\0",
            Self::Publications => b"layrs-provider-import-publications-v1\0",
            Self::Catalogs => b"layrs-provider-import-catalogs-v1\0",
            Self::Descriptors => b"layrs-provider-import-descriptors-v1\0",
            Self::ActivationMaps => b"layrs-provider-import-activation-maps-v1\0",
            Self::TransitionReceipts => b"layrs-provider-import-transition-receipts-v1\0",
            Self::Companions => b"layrs-provider-import-companions-v1\0",
            Self::CompanionFiles => b"layrs-provider-import-companion-files-v1\0",
            Self::CompanionDeletions => b"layrs-provider-import-companion-deletions-v1\0",
            Self::ContentMappings => b"layrs-provider-import-content-mappings-v1\0",
            Self::RestoreHistory => b"layrs-provider-import-restore-history-v1\0",
        }
    }
}

/// SQL is static and explicit at the call site, with bind parameters only.
/// `preflight_sql` returns COUNT and aggregate projected byte length over the
/// *same scoped predicates* as `rows_sql`; neither may select a mutable tail.
pub(super) struct Spec<'a> {
    pub kind: Kind,
    pub preflight_sql: &'static str,
    pub rows_sql: &'static str,
    pub params: &'a [&'a dyn ToSql],
    pub max_rows: u64,
    pub max_bytes: u64,
}

pub(super) struct Preflight {
    counts: Vec<u64>,
    bytes: Vec<u64>,
}
impl Preflight {
    pub(super) fn metadata_bytes(&self) -> u64 {
        // preflight checked the aggregate against MAX_CLOSURE_BYTES.
        self.bytes.iter().sum()
    }
}

pub(super) fn preflight(
    c: &Connection,
    specs: &[Spec<'_>],
    cancel: Option<&Cancellation>,
) -> CauseResult<Preflight> {
    if !matches!(specs.len(), 12 | 13) {
        return Err(invalid(
            "import requires twelve legacy or thirteen extended rowsets",
        ));
    }
    let mut counts = Vec::with_capacity(specs.len());
    let mut sizes = Vec::with_capacity(specs.len());
    let mut total_rows = 0u64;
    let mut total_bytes = 0u64;
    for (index, spec) in specs.iter().enumerate() {
        crate::projection::provider::cancellation::check(cancel)?;
        if spec.kind as usize != index {
            return Err(invalid("import rowset order or duplicate kind differs"));
        }
        let (count, bytes): (u64, u64) = c.query_row(spec.preflight_sql, spec.params, |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        total_rows = total_rows
            .checked_add(count)
            .filter(|n| *n <= MAX_CLOSURE_ROWS)
            .ok_or_else(|| invalid("import closure row count exceeds bound"))?;
        total_bytes = total_bytes
            .checked_add(bytes)
            .filter(|n| *n <= MAX_CLOSURE_BYTES)
            .ok_or_else(|| invalid("import closure metadata exceeds bound"))?;
        if count > spec.max_rows || bytes > spec.max_bytes {
            return Err(invalid("import rowset count or bytes exceeds bound"));
        }
        counts.push(count);
        sizes.push(bytes);
    }
    Ok(Preflight {
        counts,
        bytes: sizes,
    })
}

pub(super) fn stream(
    c: &Connection,
    specs: &[Spec<'_>],
    preflight: &Preflight,
    cancel: Option<&Cancellation>,
) -> CauseResult<ImportClosure> {
    if !matches!(specs.len(), 12 | 13)
        || preflight.counts.len() != specs.len()
        || preflight.bytes.len() != specs.len()
        || specs
            .iter()
            .enumerate()
            .any(|(index, spec)| spec.kind as usize != index)
    {
        return Err(invalid("import rowset preflight shape differs"));
    }
    let mut output = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        output.push(digest_one(
            c,
            spec,
            preflight.counts[index],
            preflight.bytes[index],
            cancel,
        )?);
    }
    Ok(ImportClosure {
        snapshots: output.remove(0),
        selections: output.remove(0),
        captures: output.remove(0),
        publications: output.remove(0),
        catalogs: output.remove(0),
        descriptors: output.remove(0),
        activation_maps: output.remove(0),
        transition_receipts: output.remove(0),
        companions: output.remove(0),
        companion_files: output.remove(0),
        companion_deletions: output.remove(0),
        content_mappings: output.remove(0),
        restore_history: output.pop(),
    })
}

fn digest_one(
    c: &Connection,
    spec: &Spec<'_>,
    expected: u64,
    expected_bytes: u64,
    cancel: Option<&Cancellation>,
) -> CauseResult<ImportRowset> {
    crate::projection::provider::cancellation::check(cancel)?;
    let mut statement = c.prepare(spec.rows_sql)?;
    let columns = statement.column_count();
    if columns == 0 || columns > 32 {
        return Err(invalid("import normalized row column count differs"));
    }
    let mut rows = statement.query(spec.params)?;
    let mut hash = blake3::Hasher::new();
    hash.update(spec.kind.domain());
    hash.update(&expected.to_be_bytes());
    let mut count = 0u64;
    let mut bytes = 0u64;
    while let Some(row) = rows.next()? {
        crate::projection::provider::cancellation::check(cancel)?;
        count += 1;
        if count > expected {
            return Err(invalid("import rowset grew after preflight"));
        }
        hash.update(&(columns as u32).to_be_bytes());
        for column in 0..columns {
            hash_value(&mut hash, row.get_ref(column)?, &mut bytes, spec.max_bytes)?;
        }
    }
    if count != expected || bytes != expected_bytes {
        return Err(invalid("import rowset changed after preflight"));
    }
    Ok(ImportRowset {
        count,
        digest: format!("blake3:{}", hash.finalize()),
    })
}

/// One normalized field encoder shared by SQL streaming and detached DATA.
/// Tags, byte lengths and signed integer encoding are the import contract.
fn hash_value(
    hash: &mut blake3::Hasher,
    value: ValueRef<'_>,
    bytes: &mut u64,
    max_bytes: u64,
) -> CauseResult<()> {
    let length = match value {
        ValueRef::Null => {
            hash.update(&[0]);
            0
        }
        ValueRef::Integer(value) => {
            hash.update(&[1]);
            hash.update(&value.to_be_bytes());
            8
        }
        ValueRef::Text(value) => {
            std::str::from_utf8(value).map_err(|_| invalid("import row text is not UTF-8"))?;
            hash.update(&[2]);
            hash.update(&(value.len() as u64).to_be_bytes());
            hash.update(value);
            value.len() as u64
        }
        ValueRef::Blob(value) => {
            hash.update(&[3]);
            hash.update(&(value.len() as u64).to_be_bytes());
            hash.update(value);
            value.len() as u64
        }
        ValueRef::Real(_) => return Err(invalid("import normalized row has REAL value")),
    };
    *bytes = bytes
        .checked_add(length)
        .ok_or_else(|| invalid("import row bytes overflow"))?;
    if *bytes > max_bytes {
        return Err(invalid("import rowset bytes changed after preflight"));
    }
    Ok(())
}

/// Commit the complete normalized mapping DATA with the same encoder and
/// ordering as its scoped SQL rowset. This opens nothing and proves no scope,
/// provenance, residency or authority; the caller compares a verified prefix.
pub(in crate::projection::provider) fn content_mapping_rowset_data(
    mappings: &BTreeMap<FileId, (FileManifestId, u64)>,
    cancel: Option<&Cancellation>,
) -> CauseResult<ImportRowset> {
    crate::projection::provider::cancellation::check(cancel)?;
    let count = u64::try_from(mappings.len()).map_err(|_| invalid("import row count overflow"))?;
    if count > MAX_CLOSURE_ROWS {
        return Err(invalid("import closure row count exceeds bound"));
    }
    let mut hash = blake3::Hasher::new();
    hash.update(Kind::ContentMappings.domain());
    hash.update(&count.to_be_bytes());
    let mut bytes = 0u64;
    for (file, (manifest, length)) in mappings {
        crate::projection::provider::cancellation::check(cancel)?;
        let length = i64::try_from(*length)
            .map_err(|_| invalid("import mapping length exceeds SQL INTEGER"))?;
        hash.update(&3u32.to_be_bytes());
        for value in [
            ValueRef::Text(file.as_str().as_bytes()),
            ValueRef::Text(manifest.as_str().as_bytes()),
            ValueRef::Integer(length),
        ] {
            hash_value(&mut hash, value, &mut bytes, MAX_CLOSURE_BYTES)?;
        }
    }
    Ok(ImportRowset {
        count,
        digest: format!("blake3:{}", hash.finalize()),
    })
}
