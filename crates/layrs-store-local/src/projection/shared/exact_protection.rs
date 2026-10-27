//! Exact managed-import retention DATA. SQL effects have no opener, COMMIT,
//! content reader or cleanup; the retained managed caller owns those scopes.
use crate::projection::{
    provider::{
        cancellation,
        recovery::error::{invalid, Result},
    },
    shared::exact_content::ExactBinding,
};
use layrs_core::scm::{canonical_json, FileManifest, WorktreeId};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_ROOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_MANIFESTS: usize = 100_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Protection<'a> {
    incarnation: &'a str,
    owner: &'a WorktreeId,
    database_identity: &'a str,
    operation: &'a str,
    manifests: Vec<&'a FileManifest>,
}

/// Canonical expected DATA, not source/byte/SQL authority. Construction is also
/// usable by DestinationReady, so its durable claim uses exactly this digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct ExactProtection {
    id: String,
    payload: Vec<u8>,
    incarnation: String,
    owner: WorktreeId,
    database_identity: String,
    operation: String,
    manifests: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ProtectionPresence {
    Absent,
    Present,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ProtectionEffect {
    Inserted,
    AlreadyPresent,
}

impl ExactProtection {
    pub(in crate::projection) fn new(
        incarnation: &str,
        owner: &WorktreeId,
        database_identity: &str,
        transaction_id: &str,
        required: &[FileManifest],
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        if [incarnation, database_identity]
            .iter()
            .any(|v| v.is_empty() || v.len() > 512 || v.contains('\0'))
            || owner.as_str().len() > 512
            || transaction_id.len() != 32
            || !transaction_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || required.len() > MAX_MANIFESTS
        {
            return Err(invalid("exact protection binding exceeds bound"));
        }
        let operation = format!("provider-import:{transaction_id}");
        let mut ordered = BTreeMap::new();
        let mut lengths = BTreeMap::new();
        // Refuse before constructing a potentially enormous aggregate payload.
        let mut frame = 65536usize;
        for manifest in required {
            cancellation::check(cancel)?;
            let bytes = super::exact_content::canonical_manifest_bytes(manifest)?;
            frame = frame
                .checked_add(bytes.len())
                .and_then(|n| n.checked_add(1))
                .filter(|n| *n <= MAX_ROOT_BYTES)
                .ok_or_else(|| invalid("exact protection payload exceeds bound"))?;
            if ordered.insert(manifest.id().as_str(), manifest).is_some()
                || lengths
                    .insert(manifest.file_id(), manifest.byte_length())
                    .is_some_and(|old| old != manifest.byte_length())
            {
                return Err(invalid(
                    "exact protection union duplicates or contradicts representation",
                ));
            }
        }
        let ids = ordered.keys().map(|s| (*s).to_owned()).collect();
        let value = Protection {
            incarnation,
            owner,
            database_identity,
            operation: &operation,
            manifests: ordered.into_values().collect(),
        };
        let payload = canonical_json(&value).map_err(|e| invalid(e.to_string()))?;
        if payload.len() > MAX_ROOT_BYTES {
            return Err(invalid("exact protection payload exceeds bound"));
        }
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-partial-retention-v1\0");
        hash.update(&payload);
        cancellation::check(cancel)?;
        Ok(Self {
            id: hash.finalize().to_hex().to_string(),
            payload,
            incarnation: incarnation.into(),
            owner: owner.clone(),
            database_identity: database_identity.into(),
            operation,
            manifests: ids,
        })
    }

    pub(in crate::projection) fn id(&self) -> &str {
        &self.id
    }
    pub(in crate::projection) fn digest(&self) -> &str {
        &self.id
    }
    pub(in crate::projection) fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub(in crate::projection) fn operation(&self) -> &str {
        &self.operation
    }

    pub(in crate::projection) fn require_binding(
        &self,
        binding: &ExactBinding,
        required: &[FileManifest],
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        binding.validate()?;
        let expected = Self::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            required,
            cancel,
        )?;
        if *self != expected {
            return Err(invalid(
                "exact protection differs from managed binding or complete union",
            ));
        }
        Ok(())
    }

    /// A retained logical envelope, not a claim about physical SQLite bytes.
    /// Caller independently rederives metadata_floor from the immutable plan.
    /// No capacity is released or debited by protection in this protocol slice.
    pub(in crate::projection) fn require_envelope(
        &self,
        remaining: u64,
        metadata_floor: u64,
    ) -> Result<()> {
        if metadata_floor > 1024 * 1024 * 1024 {
            return Err(invalid("exact protection metadata envelope exceeds bound"));
        }
        let needed = (self.manifests.len() as u64)
            .checked_mul(256)
            .and_then(|n| n.checked_add(self.payload.len() as u64))
            .and_then(|n| n.checked_add(65536))
            .and_then(|n| n.checked_add(metadata_floor))
            .ok_or_else(|| invalid("exact protection envelope overflow"))?;
        if remaining < needed {
            return Err(invalid(
                "partial_store_quota: exact protection retained envelope insufficient",
            ));
        }
        Ok(())
    }

    /// Metadata-only inspection, including collisions owned by somebody else.
    /// Existing partial roots are refused; replay never repairs membership.
    pub(in crate::projection) fn inspect(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<ProtectionPresence> {
        self.inspect_mode(c, cancel, true)
    }

    /// Read only one retained import root. Additional ordinary roots, including
    /// roots of this same owner, do not change this immutable root's identity.
    /// The admitted family reader independently accounts for every import group.
    pub(in crate::projection) fn inspect_retained(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<ProtectionPresence> {
        self.inspect_mode(c, cancel, false)
    }

    fn inspect_mode(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
        unique_owner: bool,
    ) -> Result<ProtectionPresence> {
        if c.is_autocommit() {
            return Err(invalid("exact protection requires caller transaction"));
        }
        cancellation::check(cancel)?;
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM roots WHERE (?3 AND owner=?1) OR id=?2 LIMIT 2)",
            params![self.owner.as_str(), self.id, unique_owner],
            |r| r.get(0),
        )?;
        if count == 0 {
            return Ok(ProtectionPresence::Absent);
        }
        if count != 1 {
            return Err(invalid("exact protection owner has additional roots"));
        }
        let bad: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM roots WHERE ((?3 AND owner=?1) OR id=?2) AND (length(CAST(id AS BLOB))>128 OR length(CAST(owner AS BLOB))>512 OR length(CAST(database_identity AS BLOB))>512 OR length(CAST(operation AS BLOB))>128 OR length(CAST(digest AS BLOB))>128 OR length(payload)>16777216))",
            params![self.owner.as_str(),self.id,unique_owner], |r| r.get(0),
        )?;
        if bad {
            return Err(invalid("exact protection root metadata exceeds bound"));
        }
        let valid: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM roots WHERE id=?1 AND owner=?2 AND database_identity=?3 AND operation=?4 AND digest=?1 AND payload=?5)",
            params![self.id, self.owner.as_str(), self.database_identity, self.operation, self.payload], |r| r.get(0),
        )?;
        if !valid {
            return Err(invalid(
                "exact protection root collision or metadata differs",
            ));
        }
        if c.query_row("SELECT EXISTS(SELECT 1 FROM root_manifests WHERE root_id=?1 AND length(CAST(manifest_id AS BLOB))!=71)", [&self.id], |r| r.get::<_,bool>(0))? {
            return Err(invalid("exact protection membership key exceeds bound"));
        }
        let mut query = c.prepare(
            "SELECT manifest_id FROM root_manifests WHERE root_id=?1 ORDER BY manifest_id LIMIT ?2",
        )?;
        let mut rows = query.query(params![self.id, self.manifests.len() + 1])?;
        let mut ordinal = 0;
        while let Some(row) = rows.next()? {
            cancellation::check(cancel)?;
            // Bound the SQL TEXT before materializing an externally corrupt key.
            let value = row
                .get_ref(0)?
                .as_str()
                .map_err(|e| invalid(e.to_string()))?;
            if value.len() != 71 || self.manifests.get(ordinal).map(String::as_str) != Some(value) {
                return Err(invalid("exact protection membership differs"));
            }
            ordinal += 1;
        }
        if ordinal != self.manifests.len() {
            return Err(invalid("exact protection membership is incomplete"));
        }
        Ok(ProtectionPresence::Present)
    }

    /// Only called after typed complete Published validation and CAS proof by
    /// the managed adapter. Any post-mutation error requires caller rollback.
    pub(super) fn insert_or_replay(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<ProtectionEffect> {
        if self.inspect(c, cancel)? == ProtectionPresence::Present {
            return Ok(ProtectionEffect::AlreadyPresent);
        }
        if c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1 {
            return Err(invalid("exact protection writer requires FK ON"));
        }
        if c.execute("INSERT INTO roots(id,owner,database_identity,operation,digest,payload) VALUES(?1,?2,?3,?4,?1,?5)", params![self.id,self.owner.as_str(),self.database_identity,self.operation,self.payload])? != 1 {
            return Err(invalid("exact protection root insert differs"));
        }
        for id in &self.manifests {
            cancellation::check(cancel)?;
            if c.execute(
                "INSERT INTO root_manifests(root_id,manifest_id) VALUES(?1,?2)",
                params![self.id, id],
            )? != 1
            {
                return Err(invalid("exact protection membership insert differs"));
            }
        }
        if self.inspect(c, cancel)? != ProtectionPresence::Present {
            return Err(invalid("exact protection insert readback differs"));
        }
        Ok(ProtectionEffect::Inserted)
    }
}

#[cfg(test)]
#[path = "exact_protection/tests.rs"]
mod tests;
