//! Caller-owned SQL transaction primitive for a source transition intent.
//! This commits nothing and confers no filesystem, family, native, or image
//! authority. On any error the caller must roll back its transaction.
use super::{
    cancellation, denied_coverage_storage,
    import_validation::{self, ImportDatabaseKind},
    records::{
        AllocationRecord, PreparedRecord, ProviderBootstrapBinding, Record,
        denied_coverage::DerivedDeniedCoverage,
        import::ImportSourceReferences,
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    recovery::error::{Result, invalid},
    validation::{self, PrivateActiveCaptureValidation},
};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, DatabaseName, TransactionState, params};

mod publication;
pub(super) use publication::{apply_source_publication, apply_source_publication_with_companion};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceIntentEffect {
    Inserted,
    Updated,
    AlreadyPresent,
}

/// The caller retains the transaction and all external gates. A typed read
/// precedes every decision and follows every write. `image_digest` is compared
/// as declared DATA by the import reader; it is not a checkpoint witness here.
pub(super) fn apply_source_intent(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    root_id: [u8; 16],
    previous: Option<&ProviderTransitionIntentRecord>,
    next: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<SourceIntentEffect> {
    apply_source_intent_with_coverage(
        c, allocation, prepared, binding, root_id, previous, next, None, cancel,
    )
}

/// A fresh v2 Planned row and its observation-derived document are inserted in
/// the same caller-owned main write transaction. Existing v2 rows must already
/// own their complete stored body: supplied DATA never repairs replay/history.
/// No live caller, destination transport or capacity admission is provided here.
pub(super) fn apply_source_intent_with_coverage(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    root_id: [u8; 16],
    previous: Option<&ProviderTransitionIntentRecord>,
    next: &ProviderTransitionIntentRecord,
    coverage: Option<&DerivedDeniedCoverage>,
    cancel: Option<&Cancellation>,
) -> Result<SourceIntentEffect> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "source transition requires a caller-owned transaction",
        ));
    }
    if next.plan.denial_coverage.is_some() || coverage.is_some() {
        if c.transaction_state(Some(DatabaseName::Main))? != TransactionState::Write {
            return Err(invalid(
                "source denied coverage requires an active main write transaction",
            ));
        }
        let (foreign_keys, query_only): (i64, i64) = c.query_row(
            "SELECT (SELECT foreign_keys FROM pragma_foreign_keys), (SELECT query_only FROM pragma_query_only)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if foreign_keys != 1 || query_only != 0 {
            return Err(invalid(
                "source denied coverage requires writable foreign-key enforcement",
            ));
        }
    }
    let next_payload = next.encode()?;
    let next_digest = next.digest()?;
    if next.role != TransitionRole::Source
        || matches!(
            next.state,
            TransitionState::Ready | TransitionState::Published
        )
        || (previous.is_none()
            && (next.state != TransitionState::Planned
                || next.destination.is_some()
                || next.reservation.is_some()
                || next.receipt_id.is_some()))
    {
        return Err(invalid("source transition begin or phase differs"));
    }
    let before = validation::private_active_transition_with_source(
        c,
        allocation,
        prepared,
        binding,
        Some(root_id),
        cancel,
    )?;
    let current = before
        .transitions
        .iter()
        .find(|row| row.transaction_id == next.transaction_id);
    if next
        .plan
        .source_prefix
        .closure
        .denial_observations
        .is_some()
    {
        return Err(invalid(
            "source inherited denied coverage prefix scope is unsupported",
        ));
    }
    // Complete before-reader above must reject missing/substituted/extra SQL
    // bodies before considering even a genuine supplied normalized document.
    if let Some(coverage) = coverage {
        coverage.record().require_intent(next, cancel)?;
    }
    if current.is_none() && next.plan.denial_coverage.is_some() && coverage.is_none() {
        return Err(invalid(
            "new source denied coverage intent requires derived body",
        ));
    }
    let exact_replay = current == Some(next);
    if exact_replay {
        if let Some(previous) = previous {
            next.require_transition(previous)?;
        }
        if next.state == TransitionState::Cancelled {
            // Historical cancellation grants no new effects; a later capture
            // may legitimately have moved beyond its old source prefix.
            cancellation::check(cancel)?;
            return Ok(SourceIntentEffect::AlreadyPresent);
        }
    } else {
        match (previous, current) {
            (None, None) => {
                if before
                    .transitions
                    .iter()
                    .any(|row| row.state != TransitionState::Cancelled)
                {
                    return Err(invalid("source transition already has a live intent"));
                }
            }
            (Some(expected), Some(observed)) if expected == observed => {
                next.require_transition(expected)?;
            }
            _ => return Err(invalid("source transition predecessor differs")),
        }
    }
    // The complete SQL import closure is checked under this same retained
    // connection. The caller independently authenticates the image digest,
    // source locks, family and native state before any external effects.
    let proof = PrivateActiveCaptureValidation {
        active: before.active,
        capture: before.capture,
    };
    import_validation::verify_source(
        c,
        &allocation.format,
        ImportDatabaseKind::Provider,
        &next.plan.source_prefix,
        ImportSourceReferences::Provider {
            allocation,
            prepared,
            activation: &proof.active.activation,
        },
        Some(&proof),
        match &next.plan.source_prefix.source {
            super::records::import::ImportSource::Provider { version, .. } => {
                &version.effective_image_digest
            }
            _ => return Err(invalid("source transition requires provider prefix")),
        },
        cancel,
    )?;
    cancellation::check(cancel)?;
    if exact_replay {
        return Ok(SourceIntentEffect::AlreadyPresent);
    }
    if previous.is_none() {
        if let Some(reference) = &next.plan.denial_coverage {
            let coverage = coverage.ok_or_else(|| {
                invalid("new source denied coverage intent requires derived body")
            })?;
            denied_coverage_storage::insert_exact(c, coverage, reference, cancel)?;
        }
    }
    let changed = match previous {
        None => c.execute(
            "INSERT INTO projection_provider_transitions(id,role,state,local_generation,reservation,receipt_id,payload,digest) \
             SELECT ?1,?2,?3,?4,?5,?6,?7,?8 WHERE NOT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE id=?1)",
            params![next.transaction_id, "source", "planned", next.local_generation,
                next.reservation, next.receipt_id, next_payload, next_digest],
        )?,
        Some(previous) => c.execute(
            "UPDATE projection_provider_transitions SET role=?1,state=?2,local_generation=?3,reservation=?4,receipt_id=?5,payload=?6,digest=?7 \
             WHERE id=?8 AND role=?9 AND state=?10 AND local_generation=?11 AND reservation IS ?12 \
             AND receipt_id IS ?13 AND payload=?14 AND digest=?15",
            params!["source", next_state(next.state), next.local_generation, next.reservation,
                next.receipt_id, next_payload, next_digest, previous.transaction_id,
                "source", next_state(previous.state), previous.local_generation,
                previous.reservation, previous.receipt_id, previous.encode()?, previous.digest()?],
        )?,
    };
    if changed != 1 {
        return Err(invalid(
            "source transition exact SQL compare-and-swap failed",
        ));
    }
    let after = validation::private_active_transition_with_source(
        c,
        allocation,
        prepared,
        binding,
        Some(root_id),
        cancel,
    )?;
    if after
        .transitions
        .iter()
        .find(|row| row.transaction_id == next.transaction_id)
        != Some(next)
    {
        return Err(invalid(
            "source transition post-write interpretation differs",
        ));
    }
    cancellation::check(cancel)?;
    Ok(if previous.is_some() {
        SourceIntentEffect::Updated
    } else {
        SourceIntentEffect::Inserted
    })
}

fn next_state(state: TransitionState) -> &'static str {
    match state {
        TransitionState::Planned => "planned",
        TransitionState::Copying => "copying",
        TransitionState::Staging => "staging",
        TransitionState::Ready => "ready",
        TransitionState::Published => "published",
        TransitionState::Cancelled => "cancelled",
    }
}

#[cfg(test)]
#[path = "transition_sql/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "transition_sql/coverage_tests.rs"]
pub(in crate::projection::provider) mod coverage_tests;
