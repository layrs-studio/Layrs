//! Windows Stage1b pure records, codecs and source budgets.
//!
//! This module uses the existing Windows VFS data codecs and never creates a guard,
//! touches a filesystem, opens SQL, or establishes receipt authenticity. A
//! structurally valid record is not evidence that its described object exists.
//! R4 is immutable; see l5-store-stage1b-r4-disposition.md for later gates.

#[path = "records/codec.rs"]
pub(super) mod codec;
#[path = "records/receipts.rs"]
mod receipts;
#[path = "records/rows.rs"]
mod rows;
#[path = "records/source.rs"]
mod source;

use crate::projection::{Result, invalid};
use layrs_core::scm::canonical_json;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(in crate::projection) use receipts::*;
pub use receipts::{PreparedBinding, ProviderBootstrapBinding, ReceiptRef};
pub(in crate::projection) use rows::*;
pub(in crate::projection) use source::*;
pub use source::{ProviderBootstrapSource, ProviderSourceDescriptor};

/// Canonical data checks only. No method on this trait authenticates a receipt
/// or validates a SQL schema/history/backend, native identity or lifecycle.
pub(in crate::projection) trait Record: Serialize + DeserializeOwned + PartialEq {
    const MAX_BYTES: usize;
    fn validate_fields(&self) -> Result<()>;

    fn encode(&self) -> Result<Vec<u8>> {
        codec::preflight(self, Self::MAX_BYTES)?;
        self.validate_fields()?;
        codec::encode(self, Self::MAX_BYTES)
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Self = codec::decode(bytes, Self::MAX_BYTES)?;
        value.validate_fields()?;
        Ok(value)
    }
}

#[cfg(test)]
#[path = "records/tests.rs"]
mod tests;
