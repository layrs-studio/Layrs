//! Fixed source-B metadata funding before a v2 Planned intent is persisted.
//!
//! DATA/SQL only: the caller authenticates the actual allocation, complete
//! source observation/body and intent, catalog-v2 schema and family identity,
//! and holds source authority plus lifecycle/catalog locks across validation
//! and this transaction. A detached digest or this reservation grants none of
//! those capabilities. The caller owns opening, rollback, commit and close;
//! after any error it must stop SQL and roll back. No live reader/driver is
//! activated by this module. Funding remains while the document is retained,
//! including Cancelled and Published. The separate release-only token cannot
//! create funding; its caller must first prove complete source absence.
use crate::projection::{
    provider::{
        cancellation,
        records::{
            AllocationRecord, PreparedRecord, Receipt, ReceiptRef, Record,
            denied_coverage::{DeniedCoverageRef, TransitionDeniedCoverageRecord},
            import::ImportSource,
            transition::{ProviderTransitionIntentRecord, TransitionRole},
        },
        recovery::error::{Result, invalid},
    },
    shared::exact_protection::ProtectionEffect,
};
use layrs_core::scm::{WorktreeId, canonical_json};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, DatabaseName, TransactionState, params};
use serde::{Deserialize, Serialize};

#[path = "source_denial/release.rs"]
mod release;
pub(in crate::projection) use release::{
    SourceDeniedCoverageRelease, SourceDeniedCoverageReleaseEffect,
};

pub(in crate::projection) const NAMESPACE: &str = "provider-denial:";
const MAX_METADATA: u64 = 1024 * 1024 * 1024;

/// Immutable operation key. It deliberately contains neither phase nor C's
/// physical database/proof, so it can be derived before C exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct SourceDeniedCoverageBinding {
    pub(in crate::projection) family_incarnation: String,
    pub(in crate::projection) family_root_identity: String,
    pub(in crate::projection) family_catalog_identity: String,
    pub(in crate::projection) family_marker_digest: String,
    pub(in crate::projection) source_owner: WorktreeId,
    pub(in crate::projection) source_database: String,
    pub(in crate::projection) transaction_id: String,
    pub(in crate::projection) plan_digest: String,
    pub(in crate::projection) prefix_id: String,
    pub(in crate::projection) document: DeniedCoverageRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct SourceDeniedCoverageReservation {
    allocation: ReceiptRef,
    prepared: ReceiptRef,
    binding: SourceDeniedCoverageBinding,
    operation: String,
    document_bytes: u64,
    intent_bytes: u64,
    reservation_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum SourceDeniedCoveragePresence {
    Absent,
    Reserved,
}

fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("source denial metadata accounting overflow"))
}

fn digest(domain: &'static str, payload: &[u8]) -> String {
    blake3::hash(&layrs_core::scm::domain_separated_bytes(domain, payload))
        .to_hex()
        .to_string()
}

/// Shared deterministic DATA calculations. These do not construct reservation
/// authority from a compact preimage.
pub(in crate::projection) fn operation_for(
    binding: &SourceDeniedCoverageBinding,
) -> Result<String> {
    Ok(format!(
        "{NAMESPACE}{}",
        digest(
            "layrs-source-denied-coverage-reservation-v1",
            &canonical_json(binding).map_err(|e| invalid(e.to_string()))?
        )
    ))
}

pub(in crate::projection) fn charges_for(
    binding: &SourceDeniedCoverageBinding,
    document_bytes: u64,
) -> Result<(u64, u64)> {
    if document_bytes == 0 || document_bytes > MAX_METADATA {
        return Err(invalid(
            "source denial normalized document byte count invalid",
        ));
    }
    let operation = operation_for(binding)?;
    // The closed intent codec plus the largest normalized SQL scalars. Keep
    // this phase-independent envelope identical for funding and exact release.
    let intent_bytes = add(
        ProviderTransitionIntentRecord::MAX_BYTES as u64,
        add(
            binding.transaction_id.len() as u64,
            6 + 9 + 8 + 128 + 71 + 71,
        )?,
    )?;
    let reservation_scalars = add(
        add(operation.len() as u64, operation.len() as u64)?,
        add(binding.source_owner.as_str().len() as u64, 8)?,
    )?;
    let reservation_bytes = add(add(document_bytes, intent_bytes)?, reservation_scalars)?;
    if reservation_bytes > MAX_METADATA {
        return Err(invalid("source denial metadata exceeds 1GiB"));
    }
    Ok((intent_bytes, reservation_bytes))
}

impl SourceDeniedCoverageReservation {
    /// Complete body DATA is mandatory; a compact document reference cannot
    /// construct a reservation. The allocation and prefix's full provider
    /// binding are compared independently of any C destination proof.
    pub(in crate::projection) fn new(
        a: &AllocationRecord,
        intent: &ProviderTransitionIntentRecord,
        body: &TransitionDeniedCoverageRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        a.validate_fields()?;
        // Enforce the complete closed intent codec, including a later proof,
        // before using its fixed maximum as the phase-independent envelope.
        intent.encode()?;
        body.require_intent(intent, cancel)?;
        let ImportSource::Provider {
            identity,
            format_digest,
            private_schema,
            allocation,
            prepared,
            binding,
            ..
        } = &intent.plan.source_prefix.source
        else {
            return Err(invalid("source denial funding requires provider prefix"));
        };
        if intent.role != TransitionRole::Source
            || *private_schema != 8
            || identity != &a.format.identity
            || allocation != &a.data_reference()?
            || format_digest
                != &format!(
                    "blake3:{}",
                    digest(
                        "layrs-projection-provider-import-format-v1",
                        &a.format.bytes()?
                    )
                )
            || binding.transaction_id != a.transaction_id
            || binding.allocation != a.data_reference()?
            || binding.recovery.identity != a.format.identity
            || binding.recovery.checkout_identity != a.format.checkout_identity()
            || binding.recovery.database_identity != a.private.database_file_identity
            || binding.recovery.marker_digest
                != format!(
                    "blake3:{}",
                    digest("layrs-projection-recovery-marker-v1", &a.format.bytes()?)
                )
            || binding.recovery.family_marker_digest.as_ref() != Some(&a.family.marker_digest)
            || binding.prepared.is_none()
            || binding.abandoned.is_some()
        {
            return Err(invalid("source denial allocation/provider binding differs"));
        }
        let allocation = allocation.clone();
        let prepared = prepared.clone();
        let binding = SourceDeniedCoverageBinding {
            family_incarnation: a.family.incarnation.clone(),
            family_root_identity: a.family.root_identity.clone(),
            family_catalog_identity: a.family.catalog_file_identity.clone(),
            family_marker_digest: a.family.marker_digest.clone(),
            source_owner: identity.worktree_id.clone(),
            source_database: a.private.database_file_identity.clone(),
            transaction_id: intent.transaction_id.clone(),
            plan_digest: intent.plan.digest()?,
            prefix_id: intent.plan.source_prefix_id.clone(),
            document: body.reference()?,
        };
        let operation = operation_for(&binding)?;
        let document_bytes = body.normalized_column_bytes()?;
        let (intent_bytes, reservation_bytes) = charges_for(&binding, document_bytes)?;
        cancellation::check(cancel)?;
        Ok(Self {
            allocation,
            prepared,
            binding,
            operation,
            document_bytes,
            intent_bytes,
            reservation_bytes,
        })
    }

    pub(in crate::projection) fn binding(&self) -> &SourceDeniedCoverageBinding {
        &self.binding
    }
    pub(in crate::projection) fn allocation(&self) -> &ReceiptRef {
        &self.allocation
    }
    pub(in crate::projection) fn prepared(&self) -> &ReceiptRef {
        &self.prepared
    }
    pub(in crate::projection) fn operation(&self) -> &str {
        &self.operation
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

    pub(in crate::projection) fn require_references(
        &self,
        a: &AllocationRecord,
        p: &PreparedRecord,
    ) -> Result<()> {
        a.validate_fields()?;
        p.require_allocation(a)?;
        if self.allocation != a.data_reference()? || self.prepared != p.data_reference()? {
            return Err(invalid(
                "source denial allocation/prepared references differ",
            ));
        }
        Ok(())
    }

    fn transaction(c: &Connection, write: bool) -> Result<()> {
        if c.is_autocommit() {
            return Err(invalid("source denial requires caller-owned transaction"));
        }
        if write {
            if c.transaction_state(Some(DatabaseName::Main))? != TransactionState::Write {
                return Err(invalid(
                    "source denial requires active main write transaction",
                ));
            }
            let (foreign_keys, query_only): (i64, i64) = c.query_row(
                "SELECT (SELECT foreign_keys FROM pragma_foreign_keys), (SELECT query_only FROM pragma_query_only)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if foreign_keys != 1 || query_only != 0 {
                return Err(invalid(
                    "source denial requires writable connection and FK ON",
                ));
            }
        }
        if super::catalog_schema_version(c)? != super::CatalogSchema::V2 {
            return Err(invalid("source denial requires catalog v2"));
        }
        Ok(())
    }

    /// A complete caller validation must precede this local exact-row probe.
    /// Global ID/operation collisions are never hidden by an owner predicate.
    pub(in crate::projection) fn read_presence(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<SourceDeniedCoveragePresence> {
        read_presence(
            &self.binding,
            &self.operation,
            self.reservation_bytes,
            c,
            cancel,
        )
    }

    /// Validate complete global logical usage, including replay.
    fn require_capacity(
        &self,
        c: &Connection,
        presence: SourceDeniedCoveragePresence,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        let usage = super::usage(c).map_err(|e| invalid(e.to_string()))?;
        let mut total = add(usage.content_bytes, usage.reserved_bytes)?;
        if presence == SourceDeniedCoveragePresence::Absent {
            total = add(total, self.reservation_bytes)?;
        }
        if total > usage.limit_bytes {
            return Err(invalid(
                "partial_store_quota: source denial metadata unfunded",
            ));
        }
        cancellation::check(cancel)?;
        Ok(())
    }

    pub(in crate::projection) fn require_present(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.require_retained(c, cancel)?;
        self.require_capacity(c, SourceDeniedCoveragePresence::Reserved, cancel)
    }

    /// Read-only prospective envelope before funding the document. The caller
    /// first validates the complete family and authenticates this Planned
    /// template against the retained source/image. This is neither a C
    /// reservation nor a promise of capacity across later unlocked intervals.
    /// The returned physical envelope includes all existing reservations and
    /// the future B/C charges; content already on disk is counted only in the
    /// logical quota. The same B reservation is never added twice on retry.
    pub(in crate::projection) fn prospective_destination_capacity(
        &self,
        c: &Connection,
        planned: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<u64> {
        self.prospective_destination_capacity_with_companion(c, planned, None, cancel)
    }

    pub(in crate::projection) fn prospective_destination_capacity_with_companion(
        &self,
        c: &Connection,
        planned: &ProviderTransitionIntentRecord,
        companion: Option<&crate::projection::provider::source_companion::ProspectiveCompanionCost>,
        cancel: Option<&Cancellation>,
    ) -> Result<u64> {
        cancellation::check(cancel)?;
        planned.validate_fields()?;
        if planned.role != TransitionRole::Source
            || planned.state
                != crate::projection::provider::records::transition::TransitionState::Planned
            || planned.transaction_id != self.binding.transaction_id
            || planned.plan.digest()? != self.binding.plan_digest
        {
            return Err(invalid(
                "source denial prospective destination plan differs",
            ));
        }
        if planned.plan.companion_id.is_some() != companion.is_some() {
            return Err(invalid(
                "source denial prospective companion sizing is missing or unexpected",
            ));
        }
        if let Some(cost) = companion {
            cost.require_plan(planned)?;
        }
        let presence = self.read_presence(c, cancel)?;
        let usage = super::usage(c).map_err(|e| invalid(e.to_string()))?;
        let mut physical = usage.reserved_bytes;
        if presence == SourceDeniedCoveragePresence::Absent {
            physical = add(physical, self.reservation_bytes)?;
        }
        physical = add(physical, planned.plan.quota_bytes)?;
        if let Some(cost) = companion {
            physical = add(physical, cost.reservation_bytes())?;
        }
        if add(usage.content_bytes, physical)? > usage.limit_bytes {
            return Err(invalid(
                "partial_store_quota: denial source and future destination unfunded",
            ));
        }
        cancellation::check(cancel)?;
        Ok(physical)
    }

    /// Authenticate funding already bound to an actual private document.
    /// Lowering the quota must not invalidate that durable identity or prevent
    /// cancellation; new/prospective reservations still enforce capacity.
    pub(in crate::projection) fn require_retained(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        let presence = self.read_presence(c, cancel)?;
        if presence != SourceDeniedCoveragePresence::Reserved {
            return Err(invalid("source denial fixed reservation absent"));
        }
        Ok(())
    }

    /// Insert at most one exact reservation. Replay is read-only and still
    /// checks total capacity; insert readback belongs to the same transaction.
    pub(in crate::projection) fn reserve(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<ProtectionEffect> {
        cancellation::check(cancel)?;
        Self::transaction(c, true)?;
        let presence = self.read_presence(c, cancel)?;
        self.require_capacity(c, presence, cancel)?;
        if presence == SourceDeniedCoveragePresence::Reserved {
            return Ok(ProtectionEffect::AlreadyPresent);
        }
        cancellation::check(cancel)?;
        c.execute(
            "INSERT INTO reservations(id,owner,operation,remaining) VALUES(?1,?2,?1,?3)",
            params![
                self.operation,
                self.binding.source_owner.as_str(),
                self.reservation_bytes
            ],
        )?;
        self.require_present(c, cancel)?;
        Ok(ProtectionEffect::Inserted)
    }
}

fn read_presence(
    binding: &SourceDeniedCoverageBinding,
    operation: &str,
    reservation_bytes: u64,
    c: &Connection,
    cancel: Option<&Cancellation>,
) -> Result<SourceDeniedCoveragePresence> {
    cancellation::check(cancel)?;
    SourceDeniedCoverageReservation::transaction(c, false)?;
    if !c.query_row(
            "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND database_identity=?2 AND state='ready')",
            params![binding.source_owner.as_str(), binding.source_database],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("source denial source registration differs"));
        }
    let count: u64 = c.query_row(
        "SELECT count(*) FROM reservations WHERE id=?1 OR operation=?1",
        [operation],
        |r| r.get(0),
    )?;
    if count > 1 {
        return Err(invalid("source denial reservation ID/operation collision"));
    }
    if count == 1
            && !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 AND owner=?2 AND operation=?1 AND typeof(remaining)='integer' AND remaining=?3)",
                params![operation, binding.source_owner.as_str(), reservation_bytes],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(invalid("source denial fixed reservation differs"));
        }
    if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM installations WHERE reservation=?1 UNION ALL SELECT 1 FROM exact_installations WHERE reservation=?1)",
            [operation],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("source denial reservation cannot fund installations"));
        }
    cancellation::check(cancel)?;
    Ok(if count == 0 {
        SourceDeniedCoveragePresence::Absent
    } else {
        SourceDeniedCoveragePresence::Reserved
    })
}

#[cfg(test)]
#[path = "source_denial/tests.rs"]
mod tests;

#[cfg(test)]
pub(in crate::projection) use tests::release_recipes::release_fixture as release_test_data;
