//! Existing family protection metadata only; no content bytes are proved here.
use super::*;
use crate::projection::shared::{
    self, CatalogSchema, provider_access::require_provider_catalog_schema,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FamilyMarker {
    pub schema_version: String,
    pub magic: String,
    pub format_version: u32,
    pub min_reader_version: u32,
    pub incarnation: String,
    pub identity: shared::PartialStoreIdentity,
    pub root_identity: String,
    pub metadata_identity: String,
    pub database_identity: String,
}

#[derive(Clone, Copy)]
pub(super) enum MemberPhase {
    Initializing,
    Ready,
}
impl MemberPhase {
    fn sql(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::Ready => "ready",
        }
    }
}
pub(super) fn protection(inputs: &ManagedReadyInputs<'_>) -> Result<(String, Vec<u8>)> {
    protection_data(
        inputs.marker,
        &inputs.staging.transaction_id,
        inputs.data.required,
        None,
    )
}

pub(super) fn protection_data(
    marker: &format::Marker,
    transaction_id: &str,
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<(String, Vec<u8>)> {
    let binding = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("Ready family binding absent"))?;
    let protection = shared::exact_protection::ExactProtection::new(
        &binding.incarnation,
        &marker.identity.worktree_id,
        &binding.database_identity,
        transaction_id,
        required,
        cancel,
    )?;
    Ok((protection.id().to_owned(), protection.payload().to_vec()))
}
pub(super) fn require(
    c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    require_at_member(c, inputs, MemberPhase::Initializing, cancel)
}

pub(super) fn require_at_member(
    c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    member_phase: MemberPhase,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    if require_provider_catalog_schema(c, cancel)? != CatalogSchema::V2 {
        return Err(invalid("destination Ready requires family schema2"));
    }
    let bytes = inputs.family_marker_bytes;
    if bytes.len() > 65536 {
        return Err(invalid("destination Ready family marker exceeds bound"));
    }
    let marker: FamilyMarker = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-recovery-family-v1\0");
    hash.update(bytes);
    let a = inputs.allocation;
    let identity = &inputs.marker.identity;
    if marker.schema_version != "layrs.partial-store-family.v1"
        || marker.magic != "LAYRS_NATIVE_STORE"
        || marker.format_version != 4
        || marker.min_reader_version != 4
        || canonical_json(&marker).map_err(|e| invalid(e.to_string()))? != bytes
        || format!("blake3:{}", hash.finalize()) != a.family.marker_digest
        || marker.incarnation != a.family.incarnation
        || marker.root_identity != a.family.root_identity
        || marker.metadata_identity != a.family.metadata_identity
        || marker.database_identity != a.family.catalog_file_identity
        || marker.identity.store_id != identity.store_id
        || marker.identity.space_id != identity.space_id
        || marker.identity.origin != identity.origin
        || marker.identity.principal_id != identity.principal_id
    {
        return Err(invalid("destination Ready family identity differs"));
    }
    let (count, length): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(marker)),0) FROM family",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1
        || length > 65536
        || c.query_row("SELECT marker FROM family WHERE singleton=1", [], |r| {
            r.get::<_, Vec<u8>>(0)
        })? != bytes
    {
        return Err(invalid("destination Ready family SQL marker differs"));
    }
    for (format, state) in [(&a.format, "ready"), (inputs.marker, member_phase.sql())] {
        cancellation::check(cancel)?;
        let binding = format
            .shared
            .as_ref()
            .ok_or_else(|| invalid("Ready member binding absent"))?;
        let valid: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND metadata_path=?2 AND database_identity=?3 AND marker=?4 AND state=?5)",
            params![format.identity.worktree_id.as_str(),binding.metadata_path.to_str()
                .ok_or_else(|| invalid("Ready metadata path non-UTF8"))?,binding.database_identity,format.bytes()?,state],
            |r| r.get(0),
        )?;
        if !valid {
            return Err(invalid("destination Ready family member differs"));
        }
    }
    require_protection(
        c,
        inputs.marker,
        &inputs.staging.transaction_id,
        inputs.data.required,
        inputs.data.claims,
        cancel,
    )
}

pub(super) fn require_protection(
    c: &Connection,
    marker: &format::Marker,
    transaction_id: &str,
    required: &[FileManifest],
    claims: &[ManagedReadyClaim],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    if claims.len() != 1 {
        return Err(invalid(
            "destination Ready durable import claim cardinality differs",
        ));
    }
    let (id, payload) = protection_data(marker, transaction_id, required, cancel)?;
    let claim = &claims[0];
    if claim.root_id != id
        || claim.digest != id
        || claim.kind != "provider-import"
        || claim.owner != transaction_id
    {
        return Err(invalid("destination Ready durable import claim differs"));
    }
    let owner = marker.identity.worktree_id.as_str();
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM roots WHERE owner=?1 LIMIT 2)",
        [owner],
        |r| r.get(0),
    )?;
    let valid: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM roots WHERE id=?1 AND owner=?2 AND database_identity=?3 AND operation=?4 AND digest=?1 AND payload=?5)",
        params![id,owner,marker.shared.as_ref().unwrap().database_identity,
            format!("provider-import:{}",transaction_id),payload], |r| r.get(0),
    )?;
    if count != 1 || !valid {
        return Err(invalid("destination Ready exact family protection differs"));
    }
    let mut ids: Vec<_> = required.iter().map(|m| m.id().as_str()).collect();
    ids.sort_unstable();
    // root_manifests has no text-length constraint. Check both cardinality and
    // byte shape before query_map can allocate any observed String, even when
    // a previously corrupted catalog was written with foreign keys disabled.
    let membership_count: usize = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM root_manifests WHERE root_id=?1 LIMIT ?2)",
        params![id, ids.len() + 1],
        |r| r.get(0),
    )?;
    if membership_count != ids.len() {
        return Err(invalid("destination Ready protection membership differs"));
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM root_manifests WHERE root_id=?1 AND length(CAST(manifest_id AS BLOB))!=71)",
        [&id], |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("destination Ready protection membership identity exceeds bound"));
    }
    let mut statement = c.prepare(
        "SELECT manifest_id FROM root_manifests WHERE root_id=?1 ORDER BY manifest_id LIMIT ?2",
    )?;
    let actual = statement
        .query_map(params![id, ids.len() + 1], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if actual.iter().map(String::as_str).collect::<Vec<_>>() != ids {
        return Err(invalid("destination Ready protection membership differs"));
    }
    for manifest in required {
        cancellation::check(cancel)?;
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sweep WHERE kind='manifests' AND id=?1)",
            [manifest.id().as_str()],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("destination Ready protected manifest is swept"));
        }
        let valid: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM manifests m JOIN files f USING(file_id) WHERE m.id=?1 AND m.file_id=?2 AND m.bytes=?3 AND f.bytes=?3 AND m.payload=?4)",
            params![manifest.id().as_str(),manifest.file_id().as_str(),manifest.byte_length(),
                canonical_json(manifest).map_err(|e| invalid(e.to_string()))?], |r| r.get(0),
        )?;
        if !valid {
            return Err(invalid(
                "destination Ready protected manifest metadata differs",
            ));
        }
        let count: usize = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM manifest_chunks WHERE manifest_id=?1 LIMIT 1025)",
            [manifest.id().as_str()],
            |r| r.get(0),
        )?;
        if count != manifest.chunks().len() {
            return Err(invalid(
                "destination Ready protected chunks cardinality differs",
            ));
        }
        for (ordinal, chunk) in manifest.chunks().iter().enumerate() {
            cancellation::check(cancel)?;
            if c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sweep WHERE kind='chunks' AND id=?1)",
                [chunk.chunk_id().as_str()],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(invalid("destination Ready protected chunk is swept"));
            }
            let valid: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM manifest_chunks m JOIN chunks c ON c.id=m.chunk_id WHERE m.manifest_id=?1 AND m.ordinal=?2 AND m.chunk_id=?3 AND c.bytes=?4)",
                params![manifest.id().as_str(),ordinal,chunk.chunk_id().as_str(),chunk.byte_length()], |r| r.get(0),
            )?;
            if !valid {
                return Err(invalid(
                    "destination Ready protected chunk metadata differs",
                ));
            }
        }
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("destination Ready family foreign key violation"));
    }
    cancellation::check(cancel)
}
