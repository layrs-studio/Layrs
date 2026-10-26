//! Read-only product boundary for an already Active partial provider.
//!
//! A ticket is an expected binding, not an authority to activate, connect, or
//! recover a provider. The Store verifies every durable and native reference.
use layrs_core::scm::WorktreeId;
use layrs_store_local::projection::{
    read_provider_ticket_document, PartialStoreFamily, ProviderActivationFailureKind,
    ProviderActiveOpenFailure, ProviderBootstrapBinding, ProviderDatabaseScope,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
};

const TICKET_MAX_BYTES: usize = 128 * 1024;
const FAMILY_ROOT_MAX_BYTES: usize = 4096;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderStatusTicket {
    pub schema_version: String,
    pub family_root: PathBuf,
    pub worktree_id: WorktreeId,
    pub transaction_id: String,
    pub expected_binding: ProviderBootstrapBinding,
}

impl ProviderStatusTicket {
    /// Canonical ticket DATA for an external, audited issuer to persist.
    pub fn encode(&self) -> Result<Vec<u8>, ProviderStatusError> {
        self.validate()?;
        let bytes = serde_jcs::to_vec(self).map_err(|_| ProviderStatusError::TicketEncoding)?;
        if bytes.len() > TICKET_MAX_BYTES {
            return Err(ProviderStatusError::TicketTooLarge);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProviderStatusError> {
        if bytes.is_empty() || bytes.len() > TICKET_MAX_BYTES {
            return Err(ProviderStatusError::TicketTooLarge);
        }
        let ticket: Self =
            serde_json::from_slice(bytes).map_err(|_| ProviderStatusError::TicketEncoding)?;
        ticket.validate()?;
        if ticket.encode()? != bytes {
            return Err(ProviderStatusError::TicketEncoding);
        }
        Ok(ticket)
    }

    fn validate(&self) -> Result<(), ProviderStatusError> {
        if self.schema_version != "layrs.partial-provider-status-ticket.v1" {
            return Err(ProviderStatusError::TicketSchema);
        }
        let root = self
            .family_root
            .to_str()
            .ok_or(ProviderStatusError::TicketLocator)?;
        if !self.family_root.is_absolute() || root.is_empty() || root.len() > FAMILY_ROOT_MAX_BYTES
        {
            return Err(ProviderStatusError::TicketLocator);
        }
        if self.transaction_id != self.expected_binding.transaction_id
            || self.worktree_id != self.expected_binding.recovery.identity.worktree_id
        {
            return Err(ProviderStatusError::TicketBinding);
        }
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, ProviderStatusError> {
        let bytes = read_provider_ticket_document(path).map_err(ProviderStatusError::Io)?;
        Self::decode(&bytes)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub state: &'static str,
    pub root: PathBuf,
    pub catalog_id: String,
    pub generation: u64,
    pub digest: String,
}

#[derive(Debug)]
pub enum ProviderStatusError {
    Io(std::io::Error),
    TicketFile,
    TicketTooLarge,
    TicketEncoding,
    TicketSchema,
    TicketLocator,
    TicketBinding,
    Store(ProviderActiveOpenFailure),
}

impl ProviderStatusError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "ticket_io",
            Self::TicketFile => "ticket_file_invalid",
            Self::TicketTooLarge => "ticket_size_invalid",
            Self::TicketEncoding => "ticket_encoding_invalid",
            Self::TicketSchema => "ticket_schema_unsupported",
            Self::TicketLocator => "ticket_locator_invalid",
            Self::TicketBinding => "ticket_binding_inconsistent",
            Self::Store(failure) => match failure.kind {
                ProviderActivationFailureKind::ProcessProfileUnavailable => {
                    "process_profile_unavailable"
                }
                ProviderActivationFailureKind::ReadonlyRollback => "readonly_rollback",
                ProviderActivationFailureKind::ObservableCloseFailure => "observable_close_failure",
                ProviderActivationFailureKind::DatabaseBusy => "database_busy",
                ProviderActivationFailureKind::UnsupportedJournal => "unsupported_journal",
                ProviderActivationFailureKind::ControlInUse => "control_in_use",
                ProviderActivationFailureKind::NativeUnavailable => "native_unavailable",
                ProviderActivationFailureKind::NeedsRepair => "needs_repair",
            },
        }
    }

    pub fn database(&self) -> Option<ProviderDatabaseScope> {
        match self {
            Self::Store(failure) => failure.database,
            _ => None,
        }
    }
}

impl fmt::Display for ProviderStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{}: {error}", self.code()),
            Self::Store(error) => write!(
                f,
                "{}: database={:?}; operation={}; cause={}; readonly_rollback={}",
                self.code(),
                error.database,
                error.operation,
                error.cause,
                error.readonly_rollback.is_some()
            ),
            _ => write!(f, "{}", self.code()),
        }
    }
}
impl std::error::Error for ProviderStatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

/// Does not declare the process-wide SQLite profile. An unaudited host receives
/// `process_profile_unavailable` from the Store before provider SQL is read.
pub fn provider_status(ticket_path: &Path) -> Result<ProviderStatus, ProviderStatusError> {
    let ticket = ProviderStatusTicket::read(ticket_path)?;
    let holder = PartialStoreFamily::open_provider_active(
        &ticket.family_root,
        &ticket.worktree_id,
        &ticket.transaction_id,
        &ticket.expected_binding,
    )
    .map_err(ProviderStatusError::Store)?;
    holder.revalidate().map_err(ProviderStatusError::Store)?;
    Ok(ProviderStatus {
        state: "active",
        root: holder.root().to_path_buf(),
        catalog_id: holder.catalog_id().to_owned(),
        generation: holder.generation(),
        digest: holder.digest().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ticket() -> ProviderStatusTicket {
        serde_json::from_value(json!({
            "schemaVersion":"layrs.partial-provider-status-ticket.v1",
            "familyRoot":"C:\\family",
            "worktreeId":"worktree:fixture",
            "transactionId":"00000000000000000000000000000001",
            "expectedBinding":{
                "schemaVersion":"layrs.projection-provider-bootstrap-binding.v1",
                "recovery":{
                    "identity":{
                        "storeId":"store:fixture", "worktreeId":"worktree:fixture",
                        "spaceId":"space:fixture", "origin":"https://example.test", "principalId":"principal:fixture"
                    },
                    "checkoutIdentity":"opaque", "markerDigest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "databaseIdentity":"opaque", "familyMarkerDigest":"blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                },
                "transactionId":"00000000000000000000000000000001",
                "allocation":{"fileIdentity":"opaque","byteLength":1,"digest":"blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},
                "prepared":null,"abandoned":null
            }
        })).unwrap()
    }

    #[test]
    fn ticket_rejects_size_schema_and_noncanonical_bytes_before_store_access() {
        let bytes = ticket().encode().unwrap();
        let decoded = ProviderStatusTicket::decode(&bytes).unwrap();
        assert_eq!(decoded.encode().unwrap(), bytes);
        let mut changed = bytes.clone();
        changed.push(b'\n');
        assert!(matches!(
            ProviderStatusTicket::decode(&changed),
            Err(ProviderStatusError::TicketEncoding)
        ));
        let mut inconsistent = ticket();
        inconsistent.transaction_id.push('0');
        assert!(matches!(
            inconsistent.encode(),
            Err(ProviderStatusError::TicketBinding)
        ));
        let mut wrong_schema = ticket();
        wrong_schema.schema_version = "layrs.partial-provider-status-ticket.v2".into();
        assert!(matches!(
            wrong_schema.encode(),
            Err(ProviderStatusError::TicketSchema)
        ));
        assert!(matches!(
            ProviderStatusTicket::decode(&vec![b' '; TICKET_MAX_BYTES + 1]),
            Err(ProviderStatusError::TicketTooLarge)
        ));
        assert!(matches!(
            ProviderStatusTicket::decode(br#"{"schemaVersion":"wrong"}"#),
            Err(ProviderStatusError::TicketEncoding)
        ));
        assert!(matches!(
            ProviderStatusTicket::decode(b"{}"),
            Err(ProviderStatusError::TicketEncoding)
        ));
    }
}
