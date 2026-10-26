//! Canonical post-bootstrap mount DATA. The writer, complete SQL interpreter,
//! native proof and handoff capability are separate later boundaries.
#![allow(dead_code)]

use super::{
    codec, invalid, AllocationRecord, MountPhase, MountRecord, PreparedRecord,
    ProviderBootstrapBinding, ProviderBootstrapSource, Record, SourceCommitment,
};
use crate::projection::Result;
use layrs_vfs::{ControlDirectoryIdentity, Registration};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "layrs.projection-provider-mount.v2";
const DOMAIN: &str = "layrs-projection-provider-mount-v2";
const TICKET_MAX_BYTES: usize = 186 + layrs_vfs::CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES;

/// A SQL-shaped row is comparison DATA, not evidence that it is the only row,
/// its foreign keys are valid, or that its native ticket is registered.
pub(in crate::projection) struct MountRowData<'a> {
    pub id: &'a str,
    pub catalog_id: &'a str,
    pub phase: &'a str,
    pub payload: &'a [u8],
    pub digest: &'a str,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::projection) enum PreparationState {
    RegistrationPending,
    ProviderPrepared { pending_digest: String },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderPreparationRecord {
    pub schema_version: String,
    pub bootstrap: MountRecord,
    pub binding: ProviderBootstrapBinding,
    pub source: SourceCommitment,
    pub root_id_hex: String,
    pub registration_ticket_hex: String,
    pub state: PreparationState,
}

impl Record for ProviderPreparationRecord {
    // The schema8 payload column permits 1 MiB; the successor contract is
    // intentionally tighter and includes the bounded bootstrap binding.
    const MAX_BYTES: usize = 128 * 1024;

    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, SCHEMA)?;
        self.bootstrap.validate_fields()?;
        if self.bootstrap.phase != MountPhase::ControlPublished {
            return Err(invalid("post-bootstrap predecessor must be published"));
        }
        self.binding.validate_fields()?;
        if self.binding.prepared.is_none() || self.binding.abandoned.is_some() {
            return Err(invalid("post-bootstrap binding requires P and excludes T"));
        }
        self.source.validate_fields()?;
        if self.bootstrap.transaction_id != self.binding.transaction_id
            || self.bootstrap.allocation != self.binding.allocation
            || self.bootstrap.prepared.as_ref()
                != self.binding.prepared.as_ref().map(|p| &p.receipt)
            || self.bootstrap.catalog_id != self.source.catalog_id
        {
            return Err(invalid("post-bootstrap predecessor/binding/source differ"));
        }
        let root_id = self.root_id()?;
        let ticket = self.registration()?;
        if ticket.root_id() != root_id || ticket.catalog_id() != &self.source.vfs_catalog_id {
            return Err(invalid("post-bootstrap ticket root/catalog differs"));
        }
        match &self.state {
            PreparationState::RegistrationPending => (),
            PreparationState::ProviderPrepared { pending_digest } => {
                codec::digest_token(pending_digest)?;
                if pending_digest != &self.reconstructed_pending_digest()? {
                    return Err(invalid("post-bootstrap prepared pending link differs"));
                }
            }
        }
        Ok(())
    }
}

impl ProviderPreparationRecord {
    fn root_id(&self) -> Result<[u8; 16]> {
        if !codec::lower_hex(&self.root_id_hex, 32) {
            return Err(invalid("post-bootstrap root ID is not canonical"));
        }
        let bytes = codec::decode_hex(&self.root_id_hex, 16)?;
        let id: [u8; 16] = bytes
            .try_into()
            .map_err(|_| invalid("post-bootstrap root ID has wrong length"))?;
        if id == [0; 16] {
            return Err(invalid("post-bootstrap root ID is zero"));
        }
        Ok(id)
    }

    fn registration(&self) -> Result<Registration> {
        let bytes = codec::decode_hex(&self.registration_ticket_hex, TICKET_MAX_BYTES)?;
        let ticket = Registration::decode_control_directory(&bytes).map_err(invalid)?;
        if ticket.encode() != bytes {
            return Err(invalid("post-bootstrap ticket is not canonical"));
        }
        let prepared = self
            .binding
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("missing P"))?;
        let control = codec::decode_hex(
            &prepared.control_identity_hex,
            layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES,
        )?;
        let control = ControlDirectoryIdentity::decode(&control).map_err(invalid)?;
        ticket.matches_control_identity(&control).map_err(invalid)?;
        Ok(ticket)
    }

    fn reconstructed_pending_digest(&self) -> Result<String> {
        // The bounded generic codec performs no Record::validate_fields call.
        // Decode the current canonical fields, change only the state, and hash
        // that exact pending payload; no recursive validation or SQL occurs.
        let bytes = codec::encode(self, Self::MAX_BYTES)?;
        let mut pending: Self = codec::decode(&bytes, Self::MAX_BYTES)?;
        pending.state = PreparationState::RegistrationPending;
        Ok(codec::hash(
            DOMAIN,
            &codec::encode(&pending, Self::MAX_BYTES)?,
        ))
    }

    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(DOMAIN, &self.encode()?))
    }

    pub fn predecessor_digest(&self) -> Result<String> {
        self.bootstrap.digest()
    }

    pub fn sql_phase(&self) -> &'static str {
        match self.state {
            PreparationState::RegistrationPending => "control_published",
            PreparationState::ProviderPrepared { .. } => "provider_prepared",
        }
    }

    /// Recheck the exact A/P/source DATA and caller-chosen root ID. The caller
    /// must separately authenticate receipts, SQL inventory and native proof.
    pub fn require_references(
        &self,
        allocation: &AllocationRecord,
        prepared: &PreparedRecord,
        source: &ProviderBootstrapSource,
        expected_root_id: [u8; 16],
    ) -> Result<()> {
        self.validate_fields()?;
        self.bootstrap
            .require_references(allocation, Some(prepared))?;
        self.binding
            .require_references(allocation, Some(prepared), None)?;
        source.require_commitment(&self.source)?;
        source.require_commitment(&allocation.source)?;
        if self.root_id()? != expected_root_id {
            return Err(invalid("post-bootstrap root ID changed"));
        }
        Ok(())
    }

    /// Compare an exact published-v1 predecessor SQL row for a future CAS. This
    /// does not acquire locks or authorize a transaction.
    pub fn require_predecessor_row(&self, row: MountRowData<'_>) -> Result<()> {
        self.validate_fields()?;
        if self.state != PreparationState::RegistrationPending {
            return Err(invalid("only pending may replace the published v1 row"));
        }
        if row.id != self.bootstrap.transaction_id
            || row.catalog_id != self.bootstrap.catalog_id
            || row.phase != "control_published"
            || row.payload != self.bootstrap.encode()?
            || row.digest != self.predecessor_digest()?
        {
            return Err(invalid("post-bootstrap predecessor row differs"));
        }
        Ok(())
    }

    /// Compare the complete SQL-shaped v2 projection; row-set cardinality,
    /// foreign keys, backend and native state require their own full validator.
    pub fn require_row(&self, row: MountRowData<'_>) -> Result<()> {
        self.validate_fields()?;
        if row.id != self.bootstrap.transaction_id
            || row.catalog_id != self.bootstrap.catalog_id
            || row.phase != self.sql_phase()
            || row.payload != self.encode()?
            || row.digest != self.digest()?
        {
            return Err(invalid("post-bootstrap mount row differs"));
        }
        Ok(())
    }

    /// Compare a prepared document with its exact pending predecessor DATA.
    pub fn require_pending(&self, pending: &Self) -> Result<()> {
        self.validate_fields()?;
        pending.validate_fields()?;
        let PreparationState::ProviderPrepared { pending_digest } = &self.state else {
            return Err(invalid("post-bootstrap document is not prepared"));
        };
        if pending.state != PreparationState::RegistrationPending
            || pending_digest != &pending.digest()?
        {
            return Err(invalid("post-bootstrap pending predecessor differs"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "preparation/tests.rs"]
mod tests;
