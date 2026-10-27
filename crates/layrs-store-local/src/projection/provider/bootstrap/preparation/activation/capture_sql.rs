//! Private capture transitions under the retained owner and native root proof.
//! The driver owns checkpoint and family installation; these calls own no
//! callback and never recover a rollback journal.
use super::*;
use crate::projection::provider::{
    database_io::WriterCloseObserver,
    records::{
        capture::{
            ProviderCaptureReceiptRecord, ProviderCaptureState, ProviderCaptureTransactionRecord,
        },
        Record,
    },
    validation::{CaptureContentProof, PrivateCaptureMutationValidation, PrivateCaptureProof},
};
use crate::projection::shared::provider_access::ActivationRegistration;
use layrs_core::scm::canonical_json;
use layrs_vfs::Checkpoint;
use rusqlite::{params, OptionalExtension};

fn valid_read<T>(state: database_io::ReadState<T>) -> CauseResult<T> {
    match state {
        database_io::ReadState::Valid(value) => Ok(value),
        database_io::ReadState::Uninitialized => Err(invalid("capture database uninitialized")),
        database_io::ReadState::Hot(hot) => Err(Cause::ReadonlyRollback(hot.trigger)),
        database_io::ReadState::UnboundRollback { trigger, cause } => Err(Cause::Cleanup {
            primary: Box::new(Cause::ReadonlyRollback(trigger)),
            close: Box::new(cause),
        }),
        database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::ReadButUnavailable(cause)
        | database_io::ReadState::Unavailable(cause) => Err(cause),
    }
}

fn private(
    c: &Connection,
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
) -> CauseResult<PrivateCaptureMutationValidation> {
    let prepared = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("capture prepared receipt absent"))?;
    let value = validation::private_active_capture_mutation_with_source(
        c,
        &held.artifacts.allocation,
        prepared,
        &held.binding,
        Some(root.registration().root_id()),
        None,
    )?;
    if value.active().mount != *mount || value.active().activation != *activation {
        return Err(invalid("capture retained active map changed"));
    }
    Ok(value)
}

fn family(held: &outcome::PublicationPins, proof: &PrivateCaptureProof) -> CauseResult<()> {
    let state = database_io::read(&held.family.catalog, |c| {
        held.family
            .validate_capture_catalog(c, &held.artifacts.allocation, proof, None)
    })?;
    if valid_read(state)? != ActivationRegistration::Ready {
        return Err(invalid("capture family owner is not ready"));
    }
    Ok(())
}

/// Caller retains the returned holder and native root. Each invocation holds
/// lifecycle, exclusive owner, and catalog locks through both complete reads.
pub(super) fn observe(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
) -> CauseResult<PrivateCaptureMutationValidation> {
    let _locks = held
        .family
        .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
    observe_locked(held, root, mount, activation)
}

fn observe_locked(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
) -> CauseResult<PrivateCaptureMutationValidation> {
    boundary(held, root)?;
    let state = database_io::read(&held.artifacts.database, |c| {
        private(c, held, root, mount, activation)
    })?;
    let value = valid_read(state)?;
    family(held, value.capture())?;
    boundary(held, root)?;
    Ok(value)
}

fn exact<'a>(
    proof: &'a PrivateCaptureProof,
    record: &ProviderCaptureTransactionRecord,
) -> CauseResult<&'a ProviderCaptureTransactionRecord> {
    proof
        .transactions
        .iter()
        .find(|row| row.transaction_id == record.transaction_id)
        .filter(|row| *row == record)
        .ok_or_else(|| invalid("capture exact transaction predecessor changed"))
}

fn checkpoint_boundary(
    checkpoint: &Checkpoint<'_>,
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
) -> CauseResult<()> {
    let proof = checkpoint
        .root_guard()
        .ok_or_else(|| invalid("capture checkpoint lacks retained root proof"))?;
    if proof.root() != root.root()
        || proof.registration().root_id() != root.registration().root_id()
        || proof.control_directory().identity() != held.guard.identity()
    {
        return Err(invalid(
            "capture checkpoint belongs to another provider root",
        ));
    }
    checkpoint.revalidate_namespace()?;
    boundary(held, root)
}

fn write(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    checkpoint: Option<&Checkpoint<'_>>,
    observed: &mut impl WriterCloseObserver,
    mutate: impl FnOnce(&Connection, &PrivateCaptureProof) -> CauseResult<()>,
    verify: impl Fn(&PrivateCaptureProof) -> CauseResult<()>,
) -> CauseResult<()> {
    let _locks = held
        .family
        .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
    if let Some(checkpoint) = checkpoint {
        checkpoint_boundary(checkpoint, held, root)?;
    }
    let predecessor = observe_locked(held, root, mount, activation)?;
    let database = &held.artifacts.database;
    let admission = database_io::clean_admission(database, false, |c| {
        private(c, held, root, mount, activation)?.require_same_state(&predecessor)
    })?;
    boundary(held, root)?;
    let bare = admission.open(database, observed)?;
    let result = (|| -> CauseResult<()> {
        admission.require_geometry(&bare)?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        let current = private(c, held, root, mount, activation)?;
        // The exclusive owner lock prevents a lawful concurrent writer; the
        // complete predecessor is still reinterpreted after writable open.
        current.require_same_state(&predecessor)?;
        mutate(c, current.capture())?;
        let successor = private(c, held, root, mount, activation)?;
        successor.require_same_publications(&predecessor)?;
        verify(successor.capture())?;
        // This prospective family check also proves an installed mapping or
        // already durable protection before the private COMMIT is attempted.
        family(held, successor.capture())?;
        if let Some(checkpoint) = checkpoint {
            checkpoint_boundary(checkpoint, held, root)?;
        }
        boundary(held, root)?;
        c.execute_batch("COMMIT")?;
        Ok(())
    })();
    bare.finish_writer(result, observed)?;
    if let Some(checkpoint) = checkpoint {
        checkpoint_boundary(checkpoint, held, root)?;
    }
    let durable = observe_locked(held, root, mount, activation)?;
    durable.require_same_publications(&predecessor)?;
    verify(durable.capture())
}

/// Inserts one Hashing intent at the exact durable capture head.
pub(super) fn begin(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    transaction: &ProviderCaptureTransactionRecord,
    observed: &mut impl WriterCloseObserver,
) -> CauseResult<()> {
    transaction.require_references(&held.artifacts.allocation, mount, activation)?;
    if !matches!(transaction.state, ProviderCaptureState::Hashing) {
        return Err(invalid("capture begin requires Hashing intent"));
    }
    write(
        held,
        root,
        mount,
        activation,
        None,
        observed,
        |c, prior| {
            if prior.transactions.iter().any(|row| {
                matches!(
                    row.state,
                    ProviderCaptureState::Hashing | ProviderCaptureState::Staging { .. }
                )
            }) || transaction.expected_capture_sequence != prior.patches.len() as u64
                || transaction.expected_capture_digest.as_deref()
                    != prior.patches.last().map(|patch| patch.digest.as_str())
            {
                return Err(invalid("capture begin head or active intent differs"));
            }
            let changed = c.execute(
                "INSERT INTO projection_provider_capture_transactions(id,mount_id,capture_generation,digest,expected_capture_sequence,expected_capture_digest,state,reservation,payload,record_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![transaction.transaction_id, transaction.mount_id, transaction.capture_generation,
                    transaction.capture_digest, transaction.expected_capture_sequence,
                    transaction.expected_capture_digest, transaction.sql_state(), transaction.reservation(),
                    transaction.encode()?, transaction.digest()?],
            )?;
            if changed != 1 {
                return Err(invalid("capture Hashing insert changed no row"));
            }
            Ok(())
        },
        |proof| exact(proof, transaction).map(|_| ()),
    )
}

/// Exact CAS for Hashing→Staging/Cancelled or Staging→Cancelled.
pub(super) fn transition(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    previous: &ProviderCaptureTransactionRecord,
    next: &ProviderCaptureTransactionRecord,
    checkpoint: &Checkpoint<'_>,
    observed: &mut impl WriterCloseObserver,
) -> CauseResult<()> {
    next.require_transition(previous)?;
    next.require_references(&held.artifacts.allocation, mount, activation)?;
    if matches!(next.state, ProviderCaptureState::Accepted { .. }) {
        return Err(invalid(
            "capture Accepted requires protected atomic publication",
        ));
    }
    write(
        held,
        root,
        mount,
        activation,
        Some(checkpoint),
        observed,
        |c, proof| {
            exact(proof, previous)?;
            update(c, previous, next)
        },
        |proof| exact(proof, next).map(|_| ()),
    )
}

fn update(
    c: &Connection,
    previous: &ProviderCaptureTransactionRecord,
    next: &ProviderCaptureTransactionRecord,
) -> CauseResult<()> {
    let changed = c.execute(
        "UPDATE projection_provider_capture_transactions SET state=?1,reservation=?2,payload=?3,record_digest=?4 WHERE id=?5 AND mount_id=?6 AND capture_generation=?7 AND digest=?8 AND expected_capture_sequence=?9 AND expected_capture_digest IS ?10 AND state=?11 AND reservation IS ?12 AND payload=?13 AND record_digest=?14",
        params![next.sql_state(), next.reservation(), next.encode()?, next.digest()?,
            previous.transaction_id, previous.mount_id, previous.capture_generation,
            previous.capture_digest, previous.expected_capture_sequence, previous.expected_capture_digest,
            previous.sql_state(), previous.reservation(), previous.encode()?, previous.digest()?],
    )?;
    if changed != 1 {
        return Err(invalid("capture exact state CAS changed no row"));
    }
    Ok(())
}

/// Publishes only family installed representations for the retained Staging
/// intent. The complete family validator authenticates every new mapping.
pub(super) fn publish_content(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    staged: &ProviderCaptureTransactionRecord,
    rows: &[CaptureContentProof],
    checkpoint: &Checkpoint<'_>,
    observed: &mut impl WriterCloseObserver,
) -> CauseResult<()> {
    if !matches!(staged.state, ProviderCaptureState::Staging { .. }) {
        return Err(invalid("capture content requires Staging"));
    }
    write(
        held,
        root,
        mount,
        activation,
        Some(checkpoint),
        observed,
        |c, proof| {
            exact(proof, staged)?;
            let ProviderCaptureState::Staging { stage } = &staged.state else {
                unreachable!("checked above")
            };
            for row in rows {
                if !stage.changes.iter().any(|change| {
                    change.replacement.as_ref().is_some_and(|file| {
                        file.file_id.as_str() == row.file_id && file.bytes == row.bytes
                    })
                }) {
                    return Err(invalid("capture content is not in the staged diff"));
                }
                let old: Option<(String, u64)> = c
                    .query_row(
                        "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
                        [&row.file_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((manifest, bytes)) = old {
                    if manifest != row.manifest_id || bytes != row.bytes {
                        return Err(invalid("capture content FileId mapping changed"));
                    }
                    continue;
                }
                let changed = c.execute(
                    "INSERT INTO projection_content(file_id,manifest_id,bytes) VALUES(?1,?2,?3)",
                    params![row.file_id, row.manifest_id, row.bytes],
                )?;
                if changed != 1 {
                    return Err(invalid("capture content insert changed no row"));
                }
            }
            Ok(())
        },
        |proof| {
            exact(proof, staged)?;
            for row in rows {
                if !proof.content.iter().any(|value| {
                    value.file_id == row.file_id
                        && value.manifest_id == row.manifest_id
                        && value.bytes == row.bytes
                }) {
                    return Err(invalid("capture content readback differs"));
                }
            }
            Ok(())
        },
    )
}

/// Final private atom: exact patch, protection claim, Accepted CAS and receipt.
/// Family protection must already be durable and is checked prospectively.
pub(super) fn accept(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    staged: &ProviderCaptureTransactionRecord,
    accepted: &ProviderCaptureTransactionRecord,
    receipt: &ProviderCaptureReceiptRecord,
    root_id: &str,
    checkpoint: &Checkpoint<'_>,
    observed: &mut impl WriterCloseObserver,
) -> CauseResult<()> {
    accepted.require_transition(staged)?;
    accepted.require_references(&held.artifacts.allocation, mount, activation)?;
    receipt.require_references(accepted)?;
    let ProviderCaptureState::Accepted { patch, .. } = &accepted.state else {
        return Err(invalid("capture accept requires Accepted record"));
    };
    if root_id.len() != 64 {
        return Err(invalid("capture protection root id differs"));
    }
    write(
        held,
        root,
        mount,
        activation,
        Some(checkpoint),
        observed,
        |c, proof| {
            exact(proof, staged)?;
            if patch.sequence == proof.patches.len() as u64 + 1 {
                let payload = canonical_json(patch).map_err(|e| invalid(e.to_string()))?;
                let changed = c.execute(
                    "INSERT INTO projection_captures(sequence,digest,projection_generation,payload) VALUES(?1,?2,?3,?4)",
                    params![patch.sequence, patch.digest, patch.projection_generation, payload],
                )?;
                if changed != 1 {
                    return Err(invalid("capture patch insert changed no row"));
                }
            } else if proof.patches.get(patch.sequence as usize - 1) != Some(patch) {
                return Err(invalid("capture dedup patch differs from durable history"));
            }
            let changed = c.execute(
                "INSERT INTO projection_claims(root_id,digest,kind,owner) VALUES(?1,?1,'provider-capture',?2)",
                params![root_id, accepted.transaction_id],
            )?;
            if changed != 1 {
                return Err(invalid("capture claim insert changed no row"));
            }
            update(c, staged, accepted)?;
            let changed = c.execute(
                "INSERT INTO projection_provider_capture_receipts(transaction_id,capture_sequence,capture_digest,payload,digest) VALUES(?1,?2,?3,?4,?5)",
                params![receipt.transaction_id, receipt.capture_sequence, receipt.capture_digest,
                    receipt.encode()?, receipt.digest()?],
            )?;
            if changed != 1 {
                return Err(invalid("capture receipt insert changed no row"));
            }
            Ok(())
        },
        |proof| {
            exact(proof, accepted)?;
            if proof.patches.get(patch.sequence as usize - 1) != Some(patch)
                || !proof.receipts.iter().any(|row| row == receipt)
                || !proof.claims.iter().any(|claim| {
                    claim.root_id == root_id
                        && claim.digest == root_id
                        && claim.owner == accepted.transaction_id
                        && claim.kind == "provider-capture"
                })
            {
                return Err(invalid("capture accepted readback differs"));
            }
            Ok(())
        },
    )
}

#[cfg(test)]
#[path = "capture_sql/tests.rs"]
mod tests;
