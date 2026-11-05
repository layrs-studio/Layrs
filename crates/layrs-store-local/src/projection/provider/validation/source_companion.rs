//! Bounded outgoing-only private DATA. No recursive import-publication proof,
//! native authority, C Ready assertion or family byte permission is made here.
use super::*;
use crate::projection::provider::{
    import_validation,
    records::{
        companion_v2::CompanionHeaderRecord,
        import::ProviderTransitionReceiptRecord,
        transition::{ProviderTransitionIntentRecord, TransitionState},
    },
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct OutgoingReferences {
    pub(super) content: BTreeMap<String, (String, u64)>,
    pub(super) claims: BTreeMap<String, (String, String)>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(in crate::projection) struct SourceCompanionPrivateProof {
    groups: Vec<OutgoingCompanion>,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(in crate::projection) struct OutgoingCompanion {
    pub(in crate::projection) transaction: String,
    pub(in crate::projection) companion_id: String,
    pub(in crate::projection) plan_digest: String,
    pub(in crate::projection) prefix_id: String,
    pub(in crate::projection) header: Vec<u8>,
    files: Vec<(String, String)>,
    deletions: Vec<(String, String)>,
    pub(in crate::projection) content: BTreeMap<String, (String, u64)>,
    pub(in crate::projection) root: String,
    pub(in crate::projection) metadata_bytes: u64,
}
impl SourceCompanionPrivateProof {
    pub(in crate::projection) fn groups(&self) -> &[OutgoingCompanion] {
        &self.groups
    }
    pub(in crate::projection) fn hash_into(
        &self,
        hash: &mut blake3::Hasher,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        super::super::cancellation::check(cancel)?;
        // Stream one bounded group, avoiding an aggregate serialization.
        hash.update(b"layrs-source-companion-private-proof-v1\0");
        hash.update(&(self.groups.len() as u64).to_be_bytes());
        for group in &self.groups {
            super::super::cancellation::check(cancel)?;
            let mut count = ProofWriter {
                hash: None,
                bytes: 0,
            };
            serde_json::to_writer(&mut count, group).map_err(|e| invalid(e.to_string()))?;
            hash.update(&count.bytes.to_be_bytes());
            let mut writer = ProofWriter {
                hash: Some(&mut *hash),
                bytes: 0,
            };
            serde_json::to_writer(&mut writer, group).map_err(|e| invalid(e.to_string()))?;
        }
        Ok(())
    }
    pub(super) fn references(&self) -> Result<OutgoingReferences> {
        let mut result = OutgoingReferences::default();
        for group in &self.groups {
            for (file, mapping) in &group.content {
                if result
                    .content
                    .insert(file.clone(), mapping.clone())
                    .is_some_and(|old| old != *mapping)
                {
                    return Err(invalid("outgoing companions disagree on sticky mapping"));
                }
            }
            if result
                .claims
                .insert(
                    group.root.clone(),
                    (group.root.clone(), group.transaction.clone()),
                )
                .is_some()
            {
                return Err(invalid("outgoing companions reuse a private root claim"));
            }
        }
        Ok(result)
    }
}

// Fixed struct field order and sorted BTreeMaps form the versioned proof
// encoding. Count/hash without allocating a potentially large group buffer.
struct ProofWriter<'a> {
    hash: Option<&'a mut blake3::Hasher>,
    bytes: u64,
}
impl std::io::Write for ProofWriter<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(b.len() as u64)
            .filter(|n| *n <= 1024 * 1024 * 1024)
            .ok_or_else(|| std::io::Error::other("source private proof encoding exceeds bound"))?;
        if let Some(hash) = &mut self.hash {
            hash.update(b);
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fence(c: &Connection, table: &str, max: u64, bad: &str) -> Result<()> {
    let count: u64 = c.query_row(
        &format!(
            "SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT {})",
            max + 1
        ),
        [],
        |r| r.get(0),
    )?;
    if count > max
        || c.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {bad})"),
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(invalid("outgoing companion private relation exceeds bound"));
    }
    Ok(())
}
pub(super) fn preflight(c: &Connection, cancel: Option<&Cancellation>) -> Result<()> {
    super::super::cancellation::check(cancel)?;
    fence(c, "projection_provider_companions", 4096,
        "length(CAST(id AS BLOB))!=71 OR typeof(payload)!='blob' OR length(payload)>65536 OR file_count<0 OR file_count>100000 OR deletion_count<0 OR deletion_count>100000 OR length(CAST(files_digest AS BLOB))!=71 OR length(CAST(deletions_digest AS BLOB))!=71")?;
    fence(c, "projection_provider_companion_files", 100000,
        "length(CAST(companion_id AS BLOB))!=71 OR length(CAST(path AS BLOB))=0 OR length(CAST(path AS BLOB))>4096 OR length(CAST(file_id AS BLOB))!=71 OR length(CAST(manifest_id AS BLOB))!=71 OR length(CAST(base_file_id AS BLOB))!=71 OR length(CAST(reason AS BLOB))>64 OR length(CAST(digest AS BLOB))!=71 OR bytes<0 OR bytes>1073741824 OR executable NOT IN (0,1) OR base_executable NOT IN (0,1)")?;
    fence(c, "projection_provider_companion_deletions", 100000,
        "length(CAST(companion_id AS BLOB))!=71 OR length(CAST(path AS BLOB))=0 OR length(CAST(path AS BLOB))>4096 OR length(CAST(base_file_id AS BLOB))!=71 OR length(CAST(base_manifest_id AS BLOB))!=71 OR length(CAST(reason AS BLOB))>64 OR length(CAST(digest AS BLOB))!=71 OR base_executable NOT IN (0,1)")?;
    let total: u64 = c.query_row(
        "SELECT coalesce(sum(length(payload)),0) FROM projection_provider_companions",
        [],
        |r| r.get(0),
    )?;
    if total > 128 * 1024 * 1024 {
        return Err(invalid("outgoing companion aggregate headers exceed bound"));
    }
    Ok(())
}

pub(super) fn validate(
    c: &Connection,
    allocation: &AllocationRecord,
    transitions: &[ProviderTransitionIntentRecord],
    receipts: &[ProviderTransitionReceiptRecord],
    cancel: Option<&Cancellation>,
) -> Result<SourceCompanionPrivateProof> {
    preflight(c, cancel)?;
    let mut proof = SourceCompanionPrivateProof::default();
    let mut ids = BTreeSet::new();
    for receipt in receipts {
        super::super::cancellation::check(cancel)?;
        let Some(id) = &receipt.companion_id else {
            continue;
        };
        let intent = transitions
            .iter()
            .find(|i| i.transaction_id == receipt.transaction_id)
            .ok_or_else(|| invalid("outgoing companion has no intent"))?;
        if !matches!(
            intent.state,
            TransitionState::Ready | TransitionState::Published
        ) || !ids.insert(id.clone())
        {
            return Err(invalid("outgoing companion has no unique terminal owner"));
        }
        intent.require_receipt(receipt)?;
        let (files, deletions, fd, dd, payload): (u64,u64,String,String,Vec<u8>) = c.query_row(
            "SELECT file_count,deletion_count,files_digest,deletions_digest,payload FROM projection_provider_companions WHERE id=?1", [id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
        let CompanionHeaderRecord::V2(header) = CompanionHeaderRecord::decode(&payload)? else {
            return Err(invalid(
                "new source companion requires v2; legacy modes remain strict",
            ));
        };
        if header.digest()? != *id
            || header.file_count != files
            || header.deletion_count != deletions
            || header.files_digest != fd
            || header.deletions_digest != dd
        {
            return Err(invalid("outgoing companion header SQL projection differs"));
        }
        let required = import_validation::validate_outgoing_companion(
            c,
            &allocation.format,
            id,
            &header,
            receipt,
            cancel,
        )?;
        let mut content = BTreeMap::new();
        for file in required {
            let mapping = c.query_row(
                "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
                [&file],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)),
            )?;
            content.insert(file, mapping);
        }
        let claim: Option<(String,String,String)> = c.query_row(
            "SELECT root_id,digest,kind FROM projection_claims WHERE owner=?1 AND kind='provider-companion'",
            [&intent.transaction_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((root, digest, kind)) = claim else {
            return Err(invalid("outgoing companion private claim absent"));
        };
        let n: u64 = c.query_row(
            "SELECT count(*) FROM projection_claims WHERE owner=?1 AND kind='provider-companion'",
            [&intent.transaction_id],
            |r| r.get(0),
        )?;
        if n != 1 || root.len() != 64 || root != digest || kind != "provider-companion" {
            return Err(invalid("outgoing companion private claim differs"));
        }
        let row_digests = |table: &str| -> Result<Vec<(String, String)>> {
            Ok(c.prepare(&format!("SELECT path,digest FROM {table} WHERE companion_id=?1 ORDER BY path COLLATE BINARY"))?
                .query_map([id], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
        };
        // Reproduce the exact independently funded B envelope from actual
        // bounded SQL, including reused mappings and both terminal intents.
        let mut row_bytes:u64=c.query_row("SELECT length(payload)+length(CAST(id AS BLOB))+16+length(CAST(files_digest AS BLOB))+length(CAST(deletions_digest AS BLOB)) FROM projection_provider_companions WHERE id=?1",[id],|r|r.get(0))?;
        let fb:u64=c.query_row("SELECT coalesce(sum(length(CAST(companion_id AS BLOB))+length(CAST(path AS BLOB))+length(CAST(file_id AS BLOB))+8+8+length(CAST(manifest_id AS BLOB))+coalesce(length(CAST(base_file_id AS BLOB)),0)+(CASE WHEN base_executable IS NULL THEN 0 ELSE 8 END)+length(CAST(reason AS BLOB))+length(CAST(digest AS BLOB))),0) FROM projection_provider_companion_files WHERE companion_id=?1",[id],|r|r.get(0))?;
        let db:u64=c.query_row("SELECT coalesce(sum(length(CAST(companion_id AS BLOB))+length(CAST(path AS BLOB))+length(CAST(base_file_id AS BLOB))+8+coalesce(length(CAST(base_manifest_id AS BLOB)),0)+length(CAST(reason AS BLOB))+length(CAST(digest AS BLOB))),0) FROM projection_provider_companion_deletions WHERE companion_id=?1",[id],|r|r.get(0))?;
        row_bytes = row_bytes
            .checked_add(fb)
            .and_then(|n| n.checked_add(db))
            .ok_or_else(|| invalid("source private envelope aggregate overflow"))?;
        for (f, (m, _)) in &content {
            row_bytes = row_bytes
                .checked_add(f.len() as u64 + m.len() as u64 + 8)
                .ok_or_else(|| invalid("source private mapping envelope overflow"))?;
        }
        let mut ready = ProviderTransitionIntentRecord::decode(&intent.encode()?)?;
        ready.state = TransitionState::Ready;
        let mut published = ProviderTransitionIntentRecord::decode(&intent.encode()?)?;
        published.state = TransitionState::Published;
        let metadata_bytes = super::super::source_companion::publication_bytes(
            row_bytes, &ready, &published, receipt,
        )?;
        proof.groups.push(OutgoingCompanion {
            transaction: intent.transaction_id.clone(),
            companion_id: id.clone(),
            plan_digest: intent.plan.digest()?,
            prefix_id: intent.plan.source_prefix_id.clone(),
            header: payload,
            files: row_digests("projection_provider_companion_files")?,
            deletions: row_digests("projection_provider_companion_deletions")?,
            content,
            root,
            metadata_bytes,
        });
    }
    let n: u64 = c.query_row(
        "SELECT count(*) FROM projection_provider_companions",
        [],
        |r| r.get(0),
    )?;
    if n != ids.len() as u64 {
        return Err(invalid("source has unowned companion headers"));
    }
    // Detect orphan scalar/claim rows before capture's later FK walk.
    for table in [
        "projection_provider_companion_files",
        "projection_provider_companion_deletions",
    ] {
        if c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} s LEFT JOIN projection_provider_companions h ON h.id=s.companion_id WHERE h.id IS NULL)"),[],|r|r.get::<_,bool>(0))? {
            return Err(invalid("source has orphan companion scalars"));
        }
    }
    proof.references()?;
    Ok(proof)
}
