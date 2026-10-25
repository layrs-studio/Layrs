//! Pure comparisons of already bounded row DATA. This is not a SQL validator:
//! it cannot establish exact schema, FK validity, history/selection closure,
//! table emptiness, native identity, or that these are all rows of a database.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum BackendKind {
    Ordinary,
    Provider,
}
#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection) struct BackendRow {
    pub singleton: u64,
    pub backend: BackendKind,
    pub mount_id: Option<String>,
}
pub(in crate::projection) struct CatalogRow<'a> {
    pub id: &'a str,
    pub source_generation: u64,
    pub source_digest: &'a str,
    pub selection_digest: &'a str,
    pub record: &'a CatalogRecord,
}
pub(in crate::projection) struct MountRow<'a> {
    pub id: &'a str,
    pub catalog_id: &'a str,
    pub phase: MountPhase,
    pub digest: &'a str,
    pub record: &'a MountRecord,
}

/// Borrowed SQL-shaped DATA only. Comparing this slice cannot prove that it
/// contains every row of a database or that a native/SQL boundary was validated.
#[derive(Clone, Copy)]
pub(in crate::projection) struct DescriptorRow<'a> {
    pub catalog_id: &'a str,
    pub path: &'a str,
    pub file_id: &'a str,
    pub bytes: u64,
    pub executable: bool,
    pub manifest_id: &'a str,
    pub manifest: &'a [u8],
}

/// Cardinality/projection comparisons precede even the mount DATA phase check.
/// Future G1 must supply complete bounded rowsets inside its own validated SQL
/// boundary, alongside history/selection/FK/evidence checks. No acceptance or
/// backend switch is performed here, including for a well-formed provider row.
pub(in crate::projection) fn require_bootstrap_row_data(
    backends: &[BackendRow],
    catalogs: &[CatalogRow<'_>],
    descriptors: &[DescriptorRow<'_>],
    mounts: &[MountRow<'_>],
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: Option<&PreparedRecord>,
) -> Result<()> {
    if backends.len() != 1 || catalogs.len() != 1 || mounts.len() != 1 {
        return Err(invalid(
            "provider bootstrap requires one backend/catalog/mount row",
        ));
    }
    let backend = &backends[0];
    if backend.singleton != 1
        || backend.backend != BackendKind::Provider
        || backend.mount_id.as_deref() != Some(allocation.transaction_id.as_str())
    {
        return Err(invalid("provider backend/mount binding differs"));
    }
    source.require_commitment(&allocation.source)?;
    allocation.validate_fields()?;
    allocation
        .format
        .validate_initial_selection(source.selection())?;
    if descriptors.len() as u64 > crate::projection::provider::PROVIDER_CATALOG_MAX_DESCRIPTORS
        || descriptors.len() != source.descriptors().len()
    {
        return Err(invalid("provider descriptor row count differs"));
    }
    // Bound untrusted borrowed data before encoding any expected manifest. The
    // owned source already enforces aggregate accounting, and every projection
    // below must match that source exactly; no aggregate byte buffer is created.
    for row in descriptors {
        if row.path.is_empty()
            || row.path.len() > codec::PATH_MAX
            || row.catalog_id.len() != 71
            || row.file_id.len() != 71
            || row.manifest_id.len() != 71
            || row.manifest.len()
                > crate::projection::provider::PROVIDER_DESCRIPTOR_MAX_BYTES as usize
        {
            return Err(invalid("provider descriptor row exceeds bounds"));
        }
    }
    for (row, expected) in descriptors.iter().zip(source.descriptors()) {
        if row.catalog_id != source.commitment().catalog_id
            || row.path != expected.path.as_str()
            || row.file_id != expected.file_id.as_str()
            || row.bytes != expected.bytes
            || row.executable != expected.executable
            || row.manifest_id != expected.manifest_id.as_str()
        {
            return Err(invalid("provider descriptor row projection/order differs"));
        }
        if row.manifest != expected.manifest_bytes()? {
            return Err(invalid("provider descriptor manifest bytes differ"));
        }
    }
    let q = &catalogs[0];
    let s = source.commitment();
    if q.id != s.catalog_id
        || q.source_generation != 0
        || q.source_digest != s.snapshot_digest
        || q.selection_digest != s.selection_digest
        || q.record != source.catalog()
    {
        return Err(invalid("provider catalog row projection differs"));
    }
    let m = &mounts[0];
    if m.id != allocation.transaction_id
        || m.catalog_id != s.catalog_id
        || m.id != m.record.transaction_id
        || m.catalog_id != m.record.catalog_id
        || m.phase != m.record.phase
        || m.digest != m.record.digest()?
    {
        return Err(invalid("provider mount row projection differs"));
    }
    m.record.require_references(allocation, prepared)
}
