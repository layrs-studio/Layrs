//! Source publication SQL DATA only. The caller independently proves the
//! destination's private Ready, exact physical checkout and family protection.
//! Errors require rollback by the caller; this module neither commits nor
//! grants admission, performs cleanup, or opens/closes any connection.
use super::*;
use crate::projection::provider::records::import::ProviderTransitionReceiptRecord;

pub(in crate::projection::provider) fn apply_source_publication(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    root_id: [u8; 16],
    previous: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    cancel: Option<&Cancellation>,
) -> Result<SourceIntentEffect> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "source publication requires a caller-owned transaction",
        ));
    }
    next.require_transition(previous)?;
    next.require_receipt(receipt)?;
    if previous.role != TransitionRole::Source
        || next.role != TransitionRole::Source
        || !matches!(
            (previous.state, next.state),
            (TransitionState::Staging, TransitionState::Ready)
                | (TransitionState::Ready, TransitionState::Published)
        )
    {
        return Err(invalid("source publication phase or role differs"));
    }
    let next_payload = next.encode()?;
    let next_digest = next.digest()?;
    let receipt_id = receipt.digest()?;
    let receipt_payload = receipt.encode()?;
    let before = validation::private_active_publication_with_source(
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
    if current == Some(next) {
        if before
            .receipts
            .iter()
            .find(|r| r.transaction_id == next.transaction_id)
            != Some(receipt)
        {
            return Err(invalid("source publication replay receipt differs"));
        }
        cancellation::check(cancel)?;
        return Ok(SourceIntentEffect::AlreadyPresent);
    }
    if current != Some(previous) {
        return Err(invalid("source publication predecessor differs"));
    }
    if next.state == TransitionState::Ready {
        // The original scoped prefix is authenticated before adding its
        // outgoing receipt. Incoming ancestry and outgoing receipts remain
        // separate sets; later terminal reads expose the complete new set.
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
                crate::projection::provider::records::import::ImportSource::Provider {
                    version,
                    ..
                } => &version.effective_image_digest,
                _ => return Err(invalid("source publication requires provider prefix")),
            },
            cancel,
        )?;
    } else if before
        .receipts
        .iter()
        .find(|r| r.transaction_id == next.transaction_id)
        != Some(receipt)
    {
        return Err(invalid("source publication existing receipt differs"));
    }
    cancellation::check(cancel)?;
    let foreign_keys: bool = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    if !foreign_keys {
        return Err(invalid("source publication writer requires foreign keys"));
    }
    if next.state == TransitionState::Ready {
        // No orphan is adopted: the complete before-reader requires every
        // existing receipt to have its own terminal source intent.
        if c.execute(
            "INSERT INTO projection_provider_transition_receipts(id,owner_identity_digest,transaction_id,companion_id,payload) VALUES(?1,?2,?3,?4,?5)",
            params![receipt_id, receipt.owner_identity_digest, receipt.transaction_id, receipt.companion_id, receipt_payload],
        )? != 1 {
            return Err(invalid("source publication receipt insertion differs"));
        }
    }
    if c.execute(
        "UPDATE projection_provider_transitions SET role=?1,state=?2,local_generation=?3,reservation=?4,receipt_id=?5,payload=?6,digest=?7 \
         WHERE id=?8 AND role=?9 AND state=?10 AND local_generation=?11 AND reservation IS ?12 \
         AND receipt_id IS ?13 AND payload=?14 AND digest=?15",
        params!["source", next_state(next.state), next.local_generation, next.reservation,
            next.receipt_id, next_payload, next_digest, previous.transaction_id,
            "source", next_state(previous.state), previous.local_generation,
            previous.reservation, previous.receipt_id, previous.encode()?, previous.digest()?],
    )? != 1 {
        return Err(invalid("source publication exact SQL compare-and-swap failed"));
    }
    let after = validation::private_active_publication_with_source(
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
        || after
            .receipts
            .iter()
            .find(|row| row.transaction_id == next.transaction_id)
            != Some(receipt)
    {
        return Err(invalid(
            "source publication post-write interpretation differs",
        ));
    }
    cancellation::check(cancel)?;
    Ok(SourceIntentEffect::Updated)
}
