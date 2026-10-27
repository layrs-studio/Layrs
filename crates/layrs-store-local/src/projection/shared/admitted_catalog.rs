//! Read-only admitted import family policy. No private opener, owner lock,
//! recovery, COMMIT, release or content I/O is permitted beneath this gate.
use super::{
    exact_content::{ExactBinding, ExactRecord},
    exact_protection::ExactProtection,
    FamilyMarker,
};
use crate::projection::{
    format,
    provider::{
        admitted_ordinary::AdmittedOrdinaryData,
        cancellation,
        recovery::error::{invalid, Result},
    },
};
use layrs_core::scm::{canonical_json, FileManifest};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};

const MAX_GROUPS: usize = 4096;
const MAX_MANIFESTS: u64 = 100_000;

#[derive(Clone)]
struct ImportContext {
    marker: format::Marker,
    binding: ExactBinding,
    root_id: String,
    required: BTreeSet<(String, String, u64)>,
    admission_digest: String,
}

/// Owned immutable commitments copied only from the real schema9 interpreter.
/// The owner/lifecycle scopes are retained by the physical caller, never here.
#[derive(Clone)]
pub(in crate::projection) struct AdmittedFamilyContext {
    primary: ImportContext,
    additional: Vec<ImportContext>,
}

/// DATA about the exact imported union at this catalog snapshot. This is not
/// CAS byte authority and must be revalidated before/after ordinary effects.
pub(in crate::projection) struct AdmittedCatalogProof {
    manifests: Vec<FileManifest>,
}
impl AdmittedCatalogProof {
    pub(in crate::projection) fn manifests(&self) -> &[FileManifest] {
        &self.manifests
    }
    pub(in crate::projection) fn into_manifests(self) -> Vec<FileManifest> {
        self.manifests
    }
}

impl AdmittedFamilyContext {
    pub(in crate::projection) fn new(data: &AdmittedOrdinaryData) -> Result<Self> {
        let binding = data.binding().clone();
        binding.validate()?;
        let required: BTreeSet<_> = data
            .required()
            .iter()
            .map(|r| (r.file_id.to_string(), r.manifest_id.to_string(), r.bytes))
            .collect();
        if required.len() != data.required().len() || required.len() > MAX_MANIFESTS as usize {
            return Err(invalid("admitted family import union differs"));
        }
        Ok(Self {
            primary: ImportContext {
                marker: data.marker().clone(),
                binding,
                root_id: data.root_id().into(),
                required,
                admission_digest: data.admission_digest().into(),
            },
            additional: Vec::new(),
        })
    }

    /// Supply another genuine schema9 commitment obtained BEFORE entering the
    /// catalog gate. This also permits retained empty imports, whose absent
    /// journals cannot independently identify their exact binding/quota.
    pub(in crate::projection) fn include_retained(&mut self, other: &Self) -> Result<()> {
        let mut additional = self.additional.clone();
        let mut manifests = self.primary.required.len()
            + additional.iter().map(|g| g.required.len()).sum::<usize>();
        for group in std::iter::once(&other.primary).chain(other.additional.iter()) {
            if additional.len() + 1 >= MAX_GROUPS {
                return Err(invalid("admitted family context count exceeds bound"));
            }
            manifests = manifests
                .checked_add(group.required.len())
                .filter(|n| *n <= MAX_MANIFESTS as usize)
                .ok_or_else(|| invalid("admitted family context union exceeds bound"))?;
            if group.binding.family_incarnation != self.primary.binding.family_incarnation
                || group.binding.family_marker_digest != self.primary.binding.family_marker_digest
                || group.binding.family_root_identity != self.primary.binding.family_root_identity
                || group.binding.family_catalog_identity
                    != self.primary.binding.family_catalog_identity
                || std::iter::once(&self.primary)
                    .chain(additional.iter())
                    .any(|g| {
                        g.root_id == group.root_id
                            || g.binding.reservation == group.binding.reservation
                    })
            {
                return Err(invalid(
                    "admitted retained context is duplicate or belongs to another family",
                ));
            }
            additional.push(group.clone());
        }
        self.additional = additional;
        Ok(())
    }

    pub(in crate::projection) fn marker(&self) -> &format::Marker {
        &self.primary.marker
    }
    pub(in crate::projection) fn binding(&self) -> &ExactBinding {
        &self.primary.binding
    }
    pub(in crate::projection) fn admission_digest(&self) -> &str {
        &self.primary.admission_digest
    }

    /// Caller holds catalog gate and a short active SQL transaction. Expected
    /// family bytes come from the retained physical family marker pins.
    pub(in crate::projection) fn validate_catalog(
        &self,
        c: &Connection,
        family_marker: &[u8],
        cancel: Option<&Cancellation>,
    ) -> Result<AdmittedCatalogProof> {
        cancellation::check(cancel)?;
        if c.is_autocommit() || family_marker.len() > 65536 {
            return Err(invalid(
                "admitted family requires bounded marker and caller transaction",
            ));
        }
        if super::provider_access::require_provider_catalog_schema(c, cancel)?
            != super::CatalogSchema::V2
        {
            return Err(invalid("admitted family requires schema2"));
        }
        let marker: FamilyMarker =
            serde_json::from_slice(family_marker).map_err(|e| invalid(e.to_string()))?;
        let stored: Vec<u8> = c.query_row(
            "SELECT marker FROM family WHERE singleton=1 AND length(marker)<=65536",
            [],
            |r| r.get(0),
        )?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-projection-recovery-family-v1\0");
        hash.update(family_marker);
        let digest = format!("blake3:{}", hash.finalize());
        if canonical_json(&marker).map_err(|e| invalid(e.to_string()))? != family_marker
            || stored != family_marker
            || digest != self.primary.binding.family_marker_digest
            || marker.incarnation != self.primary.binding.family_incarnation
            || marker.root_identity != self.primary.binding.family_root_identity
            || marker.database_identity != self.primary.binding.family_catalog_identity
        {
            return Err(invalid(
                "admitted family marker differs from immutable import",
            ));
        }
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("admitted family foreign key violation"));
        }
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM exact_installations LIMIT 100001)",
            [],
            |r| r.get(0),
        )?;
        if count > MAX_MANIFESTS {
            return Err(invalid("admitted family exact journal count exceeds bound"));
        }
        let mut known = BTreeMap::new();
        for group in std::iter::once(&self.primary).chain(self.additional.iter()) {
            known.insert(group.root_id.as_str(), group);
        }
        let mut groups = BTreeSet::new();
        let mut roots = BTreeSet::new();
        let mut owned = None;
        let mut total_manifests = 0u64;
        let mut charged_chunks = BTreeSet::new();
        let mut charged_manifests = BTreeSet::new();
        // Process one bounded root payload at a time rather than accumulating
        // 4096 potentially large unions in memory.
        let mut query = c.prepare(
            "SELECT id FROM roots WHERE operation LIKE 'provider-import%' ORDER BY id LIMIT 4097",
        )?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            cancellation::check(cancel)?;
            if roots.len() >= MAX_GROUPS {
                return Err(invalid("admitted family import group count exceeds bound"));
            }
            let id = row
                .get_ref(0)?
                .as_str()
                .map_err(|e| invalid(e.to_string()))?;
            if id.len() != 64 {
                return Err(invalid("admitted family import root key differs"));
            }
            roots.insert(id.to_owned());
            let (binding, required, member) = read_group(c, id, known.get(id).copied(), cancel)?;
            require_member(c, &marker, &self.primary.marker, &binding, &member)?;
            if binding.family_marker_digest != digest || !groups.insert(binding.reservation.clone())
            {
                return Err(invalid(
                    "admitted family import group binding differs or repeats",
                ));
            }
            total_manifests = total_manifests
                .checked_add(required.len() as u64)
                .filter(|n| *n <= MAX_MANIFESTS)
                .ok_or_else(|| invalid("admitted family import union exceeds bound"))?;
            let protection = ExactProtection::new(
                &binding.family_incarnation,
                &binding.owner,
                &binding.database_identity,
                &binding.transaction_id,
                &required,
                cancel,
            )?;
            binding.validate_retained_installations(
                c,
                &required,
                &protection,
                &mut charged_chunks,
                &mut charged_manifests,
                cancel,
            )?;
            if id == self.primary.root_id {
                owned = Some(required);
            }
        }
        if known.keys().any(|id| !roots.contains(*id)) {
            return Err(invalid("admitted family prevalidated import root absent"));
        }
        // Every retained exact journal AND every import reservation must map
        // to precisely one validated root. Ordinary reservations coexist.
        let mut query = c.prepare("SELECT id FROM reservations WHERE operation LIKE 'provider-transition%' OR id IN (SELECT reservation FROM exact_installations) LIMIT 4097")?;
        let mut rows = query.query([])?;
        let mut seen = BTreeSet::new();
        while let Some(row) = rows.next()? {
            cancellation::check(cancel)?;
            let value = row
                .get_ref(0)?
                .as_str()
                .map_err(|e| invalid(e.to_string()))?;
            if value.len() > 128 || seen.len() >= MAX_GROUPS || !groups.contains(value) {
                return Err(invalid("admitted family has unknown retained exact group"));
            }
            seen.insert(value.to_owned());
        }
        if seen != groups {
            return Err(invalid("admitted family retained reservation absent"));
        }
        cancellation::check(cancel)?;
        Ok(AdmittedCatalogProof {
            manifests: owned
                .ok_or_else(|| invalid("admitted family primary import root absent"))?,
        })
    }
}

#[path = "admitted_catalog/groups.rs"]
mod groups;
use groups::{read_group, require_member};

#[cfg(test)]
#[path = "admitted_catalog/tests.rs"]
mod tests;
