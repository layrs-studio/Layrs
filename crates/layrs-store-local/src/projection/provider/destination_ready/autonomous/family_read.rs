use super::*;
use crate::projection::shared::admitted_catalog::ExportPublicationMember;
use crate::projection::shared::{self, exact_protection::ExactProtection};
use layrs_core::scm::WorktreeId;
use rusqlite::OptionalExtension;
use serde::Deserialize;

pub(super) struct FamilyData {
    pub marker_bytes: Vec<u8>,
    pub required: Vec<FileManifest>,
    pub binding: ExactBinding,
    pub protection: ExactProtection,
}

pub(super) fn read_claim(
    c: &Connection,
    ready: &ProviderTransitionIntentRecord,
) -> Result<ManagedReadyClaim> {
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_claims LIMIT 2)",
        [],
        |r| r.get(0),
    )?;
    if count!=1 || c.query_row("SELECT EXISTS(SELECT 1 FROM projection_claims WHERE length(CAST(root_id AS BLOB))!=64 OR length(CAST(digest AS BLOB))!=64 OR length(CAST(kind AS BLOB))>32 OR length(CAST(owner AS BLOB))>128)",[],|r|r.get::<_,bool>(0))? {return Err(invalid("autonomous Ready claim missing or oversized"));}
    let claim = c.query_row(
        "SELECT root_id,digest,kind,owner FROM projection_claims",
        [],
        |r| {
            Ok(ManagedReadyClaim {
                root_id: r.get(0)?,
                digest: r.get(1)?,
                kind: r.get(2)?,
                owner: r.get(3)?,
            })
        },
    )?;
    if claim.kind != "provider-import"
        || claim.owner != ready.transaction_id
        || claim.digest != claim.root_id
    {
        return Err(invalid("autonomous Ready import claim differs"));
    }
    Ok(claim)
}

pub(super) fn read(
    c: &Connection,
    marker: &format::Marker,
    ready: &ProviderTransitionIntentRecord,
    claim: &ManagedReadyClaim,
    cancel: Option<&Cancellation>,
) -> Result<FamilyData> {
    read_at_member(
        c,
        marker,
        ready,
        claim,
        ExportPublicationMember::Ready,
        cancel,
    )
}

/// Observe only the closed publication phases. This does not validate the
/// complete family or grant installation/capacity authority.
pub(super) fn read_member(
    c: &Connection,
    marker: &format::Marker,
    cancel: Option<&Cancellation>,
) -> Result<ExportPublicationMember> {
    cancellation::check(cancel)?;
    if c.is_autocommit()
        || shared::provider_access::require_provider_catalog_schema(c, cancel)?
            != shared::CatalogSchema::V2
    {
        return Err(invalid(
            "private Ready member requires retained family schema2",
        ));
    }
    let owner = marker.identity.worktree_id.as_str();
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND length(CAST(state AS BLOB))>12)",
        [owner],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("private Ready member phase exceeds bound"));
    }
    let state: Option<String> = c
        .query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |r| {
            r.get(0)
        })
        .optional()?;
    let member = match state.as_deref() {
        Some("initializing") => ExportPublicationMember::Initializing,
        Some("ready") => ExportPublicationMember::Ready,
        _ => return Err(invalid("private Ready member absent or phase differs")),
    };
    require_member(c, marker, member)?;
    cancellation::check(cancel)?;
    Ok(member)
}

pub(super) fn require_member(
    c: &Connection,
    marker: &format::Marker,
    member: ExportPublicationMember,
) -> Result<()> {
    let binding = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("autonomous Ready shared FORMAT absent"))?;
    let state = match member {
        ExportPublicationMember::Initializing => "initializing",
        ExportPublicationMember::Ready => "ready",
    };
    if !c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND metadata_path=?2 AND database_identity=?3 AND marker=?4 AND state=?5)",params![marker.identity.worktree_id.as_str(),binding.metadata_path.to_str().ok_or_else(||invalid("autonomous Ready metadata path is not UTF8"))?,binding.database_identity,marker.bytes()?,state],|r|r.get::<_,bool>(0))? {return Err(invalid("autonomous Ready destination family member differs"));}
    Ok(())
}

pub(super) fn read_at_member(
    c: &Connection,
    marker: &format::Marker,
    ready: &ProviderTransitionIntentRecord,
    claim: &ManagedReadyClaim,
    member: ExportPublicationMember,
    cancel: Option<&Cancellation>,
) -> Result<FamilyData> {
    if shared::provider_access::require_provider_catalog_schema(c, cancel)?
        != shared::CatalogSchema::V2
    {
        return Err(invalid("autonomous Ready requires exact family schema2"));
    }
    let (count, bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(marker)),0) FROM family",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || bytes > 65536 {
        return Err(invalid(
            "autonomous Ready family marker missing or oversized",
        ));
    }
    let payload: Vec<u8> = c.query_row("SELECT marker FROM family WHERE singleton=1", [], |r| {
        r.get(0)
    })?;
    let family: super::super::family::FamilyMarker =
        serde_json::from_slice(&payload).map_err(|e| invalid(e.to_string()))?;
    let proof = ready
        .destination
        .as_ref()
        .ok_or_else(|| invalid("autonomous Ready destination absent"))?;
    let binding = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("autonomous Ready shared FORMAT absent"))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-recovery-family-v1\0");
    hash.update(&payload);
    if family.schema_version != "layrs.partial-store-family.v1"
        || family.magic != "LAYRS_NATIVE_STORE"
        || family.format_version != 4
        || family.min_reader_version != 4
        || canonical_json(&family).map_err(|e| invalid(e.to_string()))? != payload
        || format!("blake3:{}", hash.finalize()) != proof.family_marker_digest
        || family.incarnation != binding.incarnation
        || family.root_identity != binding.root_identity
        || family.identity.store_id != marker.identity.store_id
        || family.identity.space_id != marker.identity.space_id
        || family.identity.origin != marker.identity.origin
        || family.identity.principal_id != marker.identity.principal_id
    {
        return Err(invalid("autonomous Ready family marker differs"));
    }
    // No lookup of the historical source member: it may have been removed.
    require_member(c, marker, member)?;
    let marker_bytes = payload;
    let (count, bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(payload)),0) FROM roots WHERE id=?1 AND owner=?2",
        params![claim.root_id, marker.identity.worktree_id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || bytes > 16 * 1024 * 1024 {
        return Err(invalid("autonomous Ready protection missing or oversized"));
    }
    let payload: Vec<u8> = c.query_row(
        "SELECT payload FROM roots WHERE id=?1 AND owner=?2",
        params![claim.root_id, marker.identity.worktree_id.as_str()],
        |r| r.get(0),
    )?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Protection {
        incarnation: String,
        owner: WorktreeId,
        database_identity: String,
        operation: String,
        manifests: Vec<FileManifest>,
    }
    let root: Protection = serde_json::from_slice(&payload).map_err(|e| invalid(e.to_string()))?;
    if root.manifests.len() > 100_000
        || root.incarnation != binding.incarnation
        || root.owner != marker.identity.worktree_id
        || root.database_identity != binding.database_identity
        || root.operation != format!("provider-import:{}", ready.transaction_id)
    {
        return Err(invalid("autonomous Ready protection binding differs"));
    }
    for manifest in &root.manifests {
        cancellation::check(cancel)?;
        exact_content::canonical_manifest_bytes(manifest)?;
    }
    let protection = ExactProtection::new(
        &binding.incarnation,
        &marker.identity.worktree_id,
        &binding.database_identity,
        &ready.transaction_id,
        &root.manifests,
        cancel,
    )?;
    if protection.id() != claim.root_id || protection.payload() != payload {
        return Err(invalid(
            "autonomous Ready protection canonical payload differs",
        ));
    }
    let operation = ready.reservation_operation()?;
    let exact = ExactBinding {
        family_incarnation: family.incarnation,
        family_root_identity: family.root_identity,
        family_marker_digest: proof.family_marker_digest.clone(),
        family_catalog_identity: family.database_identity,
        plan_digest: ready.plan.digest()?,
        prefix_id: ready.plan.source_prefix_id.clone(),
        destination_proof_digest: proof.digest(&ready.plan)?,
        transaction_id: ready.transaction_id.clone(),
        owner: marker.identity.worktree_id.clone(),
        database_identity: proof.private_database_identity.clone(),
        reservation: operation.clone(),
        operation,
        quota_bytes: ready.plan.quota_bytes,
    };
    exact.validate()?;
    // These are family retention DATA only. No historical B database/native
    // path is reopened and C's own private claim/root remains mandatory.
    shared::source_companion::retained_namespace(c, cancel, |source| {
        let b = &source.binding.destination;
        if b.owner == exact.owner && b.transaction_id == exact.transaction_id {
            if *b != exact {
                return Err(invalid("autonomous source retention C binding differs"));
            }
            source.require_union(&root.manifests, cancel)?;
        }
        Ok(())
    })?;
    Ok(FamilyData {
        marker_bytes,
        required: root.manifests,
        binding: exact,
        protection,
    })
}
