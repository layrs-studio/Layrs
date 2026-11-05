//! Current transition coverage transport, separate from inherited prefix rows.
//! DATA/SQL only. The caller retains source/family authority, authenticates the
//! destination schema and pins, and owns rollback, COMMIT and observed close.
//! The source Copying proof is not evidence that family B is funded.
use super::{
    cancellation, denied_coverage_storage,
    records::{
        AllocationRecord, PreparedRecord, ProviderBootstrapBinding, Record,
        denied_coverage::TransitionDeniedCoverageRecord,
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    recovery::error::{Result, invalid},
    validation,
};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, DatabaseName, TransactionState, params};

/// An actual current body read from one retained source connection. This is
/// neither a normalized-observation constructor nor a destination capability.
/// Private fields prevent decoded/literal bodies from manufacturing transport.
pub(in crate::projection::provider) struct SourceCurrentDeniedCoverage<'s> {
    source: &'s Connection,
    allocation: &'s AllocationRecord,
    prepared: &'s PreparedRecord,
    binding: &'s ProviderBootstrapBinding,
    root_id: [u8; 16],
    intent: ProviderTransitionIntentRecord,
    body: TransitionDeniedCoverageRecord,
}

/// Read the full source interpretation before accepting the exact expected
/// intent and its actual canonical body. Legacy callers have no current body
/// and branch before invoking this constructor.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn read_source<'s>(
    source: &'s Connection,
    allocation: &'s AllocationRecord,
    prepared: &'s PreparedRecord,
    binding: &'s ProviderBootstrapBinding,
    root_id: [u8; 16],
    expected: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<SourceCurrentDeniedCoverage<'s>> {
    let (intent, body) = read_actual_source(
        source, allocation, prepared, binding, root_id, expected, cancel,
    )?;
    Ok(SourceCurrentDeniedCoverage {
        source,
        allocation,
        prepared,
        binding,
        root_id,
        intent,
        body,
    })
}

#[allow(clippy::too_many_arguments)]
fn read_actual_source(
    source: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    root_id: [u8; 16],
    expected: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<(
    ProviderTransitionIntentRecord,
    TransitionDeniedCoverageRecord,
)> {
    cancellation::check(cancel)?;
    if source.is_autocommit() {
        return Err(invalid(
            "current coverage requires a retained source transaction",
        ));
    }
    expected.validate_fields()?;
    if expected.role != TransitionRole::Source
        || expected.state != TransitionState::Copying
        || expected.destination.is_none()
        || expected.reservation.is_some()
        || expected.receipt_id.is_some()
        || expected.plan.denial_coverage.is_none()
    {
        return Err(invalid(
            "current coverage requires unreserved source Copying with destination proof",
        ));
    }
    let full = validation::private_active_transition_with_source(
        source,
        allocation,
        prepared,
        binding,
        Some(root_id),
        cancel,
    )?;
    let intent = full
        .transitions
        .into_iter()
        .find(|row| row.transaction_id == expected.transaction_id)
        .filter(|row| row == expected)
        .ok_or_else(|| invalid("current coverage exact source intent is absent or differs"))?;
    let reference = intent
        .plan
        .denial_coverage
        .as_ref()
        .ok_or_else(|| invalid("current coverage source reference is absent"))?;
    let body = denied_coverage_storage::read_required(source, reference, cancel)?;
    body.require_intent(&intent, cancel)?;
    cancellation::check(cancel)?;
    Ok((intent, body))
}

impl SourceCurrentDeniedCoverage<'_> {
    pub(in crate::projection::provider) fn record(&self) -> &TransitionDeniedCoverageRecord {
        &self.body
    }

    /// Require actual Source Copying with the exact C proof before any fresh
    /// destination DDL, and re-read the complete source/body in this snapshot.
    /// Physical ownership and full family B authority remain enclosing checks.
    pub(in crate::projection::provider) fn require_destination(
        &self,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        if self.source.is_autocommit() {
            return Err(invalid("current coverage source transaction ended"));
        }
        intent.validate_fields()?;
        if intent.role != TransitionRole::Destination
            || intent.transaction_id != self.intent.transaction_id
            || intent.plan != self.intent.plan
        {
            return Err(invalid(
                "current coverage destination transaction or plan differs",
            ));
        }
        if intent.state != TransitionState::Copying
            || intent.reservation.is_some()
            || intent.receipt_id.is_some()
            || intent.destination != self.intent.destination
        {
            return Err(invalid(
                "current coverage requires unreserved destination Copying with matching destination proof",
            ));
        }
        self.body.require_intent(intent, cancel)?;
        // A borrowed Connection does not freeze its transaction's application
        // rows. This check also runs before an initializer's first DDL.
        let (_, actual) = read_actual_source(
            self.source,
            self.allocation,
            self.prepared,
            self.binding,
            self.root_id,
            &self.intent,
            cancel,
        )?;
        if actual != self.body {
            return Err(invalid("current coverage retained source body changed"));
        }
        Ok(())
    }

    /// Fresh initialization only: an existing ID is refused even if identical.
    /// Replay readers use read_current and never call this insertion to repair
    /// a missing body. Any error requires the caller's destination rollback.
    pub(in crate::projection::provider) fn copy_fresh_into(
        &self,
        destination: &Connection,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        if std::ptr::eq(self.source, destination) {
            return Err(invalid(
                "current coverage destination must be a distinct connection",
            ));
        }
        if destination.is_autocommit()
            || destination.transaction_state(Some(DatabaseName::Main))? != TransactionState::Write
        {
            return Err(invalid(
                "current coverage destination requires an active main write transaction",
            ));
        }
        let foreign_keys: i64 =
            destination.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        let query_only: i64 = destination.query_row("PRAGMA query_only", [], |row| row.get(0))?;
        if foreign_keys != 1 || query_only != 0 {
            return Err(invalid(
                "current coverage destination requires writable FK enforcement",
            ));
        }
        self.require_destination(intent, cancel)?;
        let reference = self.body.reference()?;
        let present: bool = destination.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_observations WHERE digest=?1)",
            [&reference.id],
            |row| row.get(0),
        )?;
        if present {
            return Err(invalid(
                "fresh current coverage destination ID already exists",
            ));
        }
        let payload = self.body.encode()?;
        cancellation::check(cancel)?;
        if destination.execute(
            "INSERT INTO projection_observations(digest,payload) VALUES(?1,?2)",
            params![reference.id, payload],
        )? != 1
        {
            return Err(invalid(
                "fresh current coverage insertion row count differs",
            ));
        }
        let stored = read_current(destination, intent, cancel)?
            .ok_or_else(|| invalid("fresh current coverage readback is absent"))?;
        if stored != self.body {
            return Err(invalid("fresh current coverage readback differs"));
        }
        cancellation::check(cancel)
    }
}

/// Authenticate only the current body required by actual destination intent
/// DATA. The caller validates its owning SQL row, complete inventory and schema.
/// Inherited documents are interpreted through their distinct prefix scope.
pub(in crate::projection::provider) fn read_current(
    c: &Connection,
    intent: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<Option<TransitionDeniedCoverageRecord>> {
    cancellation::check(cancel)?;
    intent.validate_fields()?;
    if intent.role != TransitionRole::Destination {
        return Err(invalid(
            "current coverage reader requires destination intent",
        ));
    }
    let Some(reference) = &intent.plan.denial_coverage else {
        return Ok(None);
    };
    let body = denied_coverage_storage::read_required(c, reference, cancel)?;
    body.require_intent(intent, cancel)?;
    Ok(Some(body))
}

#[cfg(test)]
#[path = "destination_coverage/tests.rs"]
mod tests;
