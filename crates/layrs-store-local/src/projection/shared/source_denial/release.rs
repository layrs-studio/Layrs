//! Exact family-row release, without a reservation/funding conversion. The
//! caller retains authenticated receipt/native A/P/source ownership, exclusive
//! lifecycle and catalog gates, and a complete strict source reader proving
//! both the transaction and body absent across this family transaction. Never
//! use a receipt reference or namespace-only probe as that absence proof.
use super::*;
use crate::projection::provider::records::denial_start::PendingDeniedCoverageReleaseRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct SourceDeniedCoverageRelease {
    binding: SourceDeniedCoverageBinding,
    operation: String,
    reservation_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum SourceDeniedCoverageReleaseEffect {
    Released,
    AlreadyAbsent,
}

impl SourceDeniedCoverageRelease {
    /// Validated release-only DATA. Native receipt authentication, complete
    /// source absence and all gates remain mandatory caller preconditions.
    pub(in crate::projection) fn new(
        a: &AllocationRecord,
        p: &PreparedRecord,
        record: &PendingDeniedCoverageReleaseRecord,
    ) -> Result<Self> {
        record.require_references(a, p)?;
        Ok(Self {
            binding: record.binding().clone(),
            operation: record.operation().into(),
            reservation_bytes: record.reservation_bytes(),
        })
    }

    pub(in crate::projection) fn binding(&self) -> &SourceDeniedCoverageBinding {
        &self.binding
    }
    pub(in crate::projection) fn operation(&self) -> &str {
        &self.operation
    }
    pub(in crate::projection) fn reservation_bytes(&self) -> u64 {
        self.reservation_bytes
    }
    pub(in crate::projection) fn read_presence(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<SourceDeniedCoveragePresence> {
        super::read_presence(
            &self.binding,
            &self.operation,
            self.reservation_bytes,
            c,
            cancel,
        )
    }

    /// Delete only the exact fixed row, with no installation, under main Write
    /// and FK ON/query_only OFF. Replay is read-only. Global capacity is not a
    /// condition for freeing durable funding after a quota reduction.
    ///
    /// Caller owns commit, positive close, strict post-close readback and only
    /// then deletion of the pinned receipt. Any error stops the chain and the
    /// caller rolls back without further probes.
    pub(in crate::projection) fn release(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<SourceDeniedCoverageReleaseEffect> {
        cancellation::check(cancel)?;
        SourceDeniedCoverageReservation::transaction(c, true)?;
        if self.read_presence(c, cancel)? == SourceDeniedCoveragePresence::Absent {
            return Ok(SourceDeniedCoverageReleaseEffect::AlreadyAbsent);
        }
        cancellation::check(cancel)?;
        let deleted = c.execute(
            "DELETE FROM reservations WHERE id=?1 AND operation=?1 AND owner=?2 AND typeof(remaining)='integer' AND remaining=?3 AND NOT EXISTS(SELECT 1 FROM installations WHERE reservation=?1) AND NOT EXISTS(SELECT 1 FROM exact_installations WHERE reservation=?1)",
            params![self.operation, self.binding.source_owner.as_str(), self.reservation_bytes],
        )?;
        if deleted != 1 {
            return Err(invalid(
                "source denial exact release affected row count differs",
            ));
        }
        if self.read_presence(c, cancel)? != SourceDeniedCoveragePresence::Absent {
            return Err(invalid("source denial exact release readback differs"));
        }
        Ok(SourceDeniedCoverageReleaseEffect::Released)
    }
}
