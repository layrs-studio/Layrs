//! Bounded release-only DATA published before source denial funding. Native
//! receipt authentication, complete source absence, locks and SQL ownership are
//! separate caller obligations. Decoding this record never constructs funding.
use super::{
    codec, invalid, AllocationRecord, PreparedRecord, Receipt, ReceiptRef, Record, Result,
};
use crate::projection::shared::source_denial::{
    charges_for, operation_for, SourceDeniedCoverageBinding, SourceDeniedCoverageReservation,
};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "layrs.projection-pending-denied-coverage-release.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct PendingDeniedCoverageReleaseRecord {
    schema_version: String,
    receipt_file_identity: String,
    allocation: ReceiptRef,
    prepared: ReceiptRef,
    binding: SourceDeniedCoverageBinding,
    operation: String,
    document_bytes: u64,
    intent_bytes: u64,
    reservation_bytes: u64,
}

fn reference(value: &ReceiptRef, max: usize) -> Result<()> {
    codec::file_identity(&value.file_identity)?;
    codec::digest_token(&value.digest)?;
    if value.byte_length == 0 || value.byte_length > max as u64 {
        return Err(invalid(
            "pending denial receipt reference byte count invalid",
        ));
    }
    Ok(())
}

impl Record for PendingDeniedCoverageReleaseRecord {
    const MAX_BYTES: usize = 64 * 1024;

    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, SCHEMA)?;
        codec::file_identity(&self.receipt_file_identity)?;
        reference(&self.allocation, AllocationRecord::MAX_BYTES)?;
        reference(&self.prepared, PreparedRecord::MAX_BYTES)?;
        let b = &self.binding;
        if b.family_incarnation.is_empty() || b.family_incarnation.len() > 128 {
            return Err(invalid("pending denial family incarnation length invalid"));
        }
        codec::directory_identity(&b.family_root_identity)?;
        codec::file_identity(&b.family_catalog_identity)?;
        codec::digest_token(&b.family_marker_digest)?;
        layrs_core::scm::WorktreeId::parse(b.source_owner.as_str()).map_err(invalid)?;
        codec::file_identity(&b.source_database)?;
        codec::transaction(&b.transaction_id)?;
        codec::digest_token(&b.plan_digest)?;
        codec::digest_token(&b.prefix_id)?;
        b.document.validate()?;
        let (intent_bytes, reservation_bytes) =
            charges_for(b, self.document_bytes).map_err(|e| invalid(e.to_string()))?;
        if self.operation != operation_for(b).map_err(|e| invalid(e.to_string()))?
            || self.intent_bytes != intent_bytes
            || self.reservation_bytes != reservation_bytes
        {
            return Err(invalid(
                "pending denial operation or deterministic charge differs",
            ));
        }
        Ok(())
    }
}

impl Receipt for PendingDeniedCoverageReleaseRecord {
    const DOMAIN: &'static str = "layrs-projection-pending-denied-coverage-release-v1";
    fn file_identity(&self) -> &str {
        &self.receipt_file_identity
    }
}

impl PendingDeniedCoverageReleaseRecord {
    /// Only the full-body-derived reservation can create a new record. A/P are
    /// compared with the exact references retained by that reservation.
    pub(in crate::projection) fn new(
        a: &AllocationRecord,
        p: &PreparedRecord,
        reservation: &SourceDeniedCoverageReservation,
        receipt_file_identity: String,
    ) -> Result<Self> {
        reservation
            .require_references(a, p)
            .map_err(|e| invalid(e.to_string()))?;
        let value = Self {
            schema_version: SCHEMA.into(),
            receipt_file_identity,
            allocation: a.data_reference()?,
            prepared: p.data_reference()?,
            binding: reservation.binding().clone(),
            operation: reservation.operation().into(),
            document_bytes: reservation.document_bytes(),
            intent_bytes: reservation.intent_bytes(),
            reservation_bytes: reservation.reservation_bytes(),
        };
        value.require_references(a, p)?;
        value.encode()?;
        Ok(value)
    }

    pub(in crate::projection) fn with_file_identity(&self, identity: String) -> Result<Self> {
        let mut value = self.clone();
        value.receipt_file_identity = identity;
        value.encode()?;
        Ok(value)
    }

    /// DATA comparison only. The caller must authenticate the native A/P and
    /// release receipt, including the latter's own pinned file identity.
    pub(in crate::projection) fn require_references(
        &self,
        a: &AllocationRecord,
        p: &PreparedRecord,
    ) -> Result<()> {
        self.validate_fields()?;
        a.validate_fields()?;
        p.require_allocation(a)?;
        let b = &self.binding;
        if self.allocation != a.data_reference()?
            || self.prepared != p.data_reference()?
            || b.family_incarnation != a.family.incarnation
            || b.family_root_identity != a.family.root_identity
            || b.family_catalog_identity != a.family.catalog_file_identity
            || b.family_marker_digest != a.family.marker_digest
            || b.source_owner != a.format.identity.worktree_id
            || b.source_database != a.private.database_file_identity
        {
            return Err(invalid(
                "pending denial allocation/prepared binding differs",
            ));
        }
        Ok(())
    }

    pub(in crate::projection) fn require_reservation(
        &self,
        reservation: &SourceDeniedCoverageReservation,
    ) -> Result<()> {
        self.validate_fields()?;
        if &self.allocation != reservation.allocation()
            || &self.prepared != reservation.prepared()
            || &self.binding != reservation.binding()
            || self.operation != reservation.operation()
            || self.document_bytes != reservation.document_bytes()
            || self.intent_bytes != reservation.intent_bytes()
            || self.reservation_bytes != reservation.reservation_bytes()
        {
            return Err(invalid("pending denial reservation differs"));
        }
        Ok(())
    }

    pub(in crate::projection) fn binding(&self) -> &SourceDeniedCoverageBinding {
        &self.binding
    }
    pub(in crate::projection) fn operation(&self) -> &str {
        &self.operation
    }
    pub(in crate::projection) fn allocation(&self) -> &ReceiptRef {
        &self.allocation
    }
    pub(in crate::projection) fn prepared(&self) -> &ReceiptRef {
        &self.prepared
    }
    pub(in crate::projection) fn document_bytes(&self) -> u64 {
        self.document_bytes
    }
    pub(in crate::projection) fn intent_bytes(&self) -> u64 {
        self.intent_bytes
    }
    pub(in crate::projection) fn reservation_bytes(&self) -> u64 {
        self.reservation_bytes
    }
}
