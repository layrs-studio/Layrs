//! Detached cancellation DATA and the admitted, release-before-delete path.
use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection) struct RestoreCancellationData {
    generation: u64,
    digest: String,
    budget: Option<Budget>,
    staging: Option<staging::RestoreStage>,
}

#[derive(Debug, PartialEq, Eq)]
struct Budget {
    operation: String,
    reservation: String,
    bytes: u64,
    witness: String,
    committed: bool,
}

impl RestoreCancellationData {
    pub(in crate::projection) fn validate_family(
        &self,
        lease: &shared::Lease,
        store: &ProjectionStore,
    ) -> Result<()> {
        if !lease.is_admitted() {
            return Err(invalid(
                "restore cancellation requires admitted family context",
            ));
        }
        if let Some(budget) = &self.budget {
            lease.preflight_restore_cancellation(
                &store.marker,
                &budget.reservation,
                &budget.operation,
                budget.bytes,
                budget.committed,
            )?;
        }
        Ok(())
    }

    pub(in crate::projection) fn validate_staging(&self, store: &ProjectionStore) -> Result<()> {
        if let Some(stage) = &self.staging {
            stage.validate_physical(store)?;
        }
        Ok(())
    }
}

/// Requires the genuine RO admission snapshot or the admitted handle's own
/// validation snapshot. This function only reads bounded private DATA.
pub(in crate::projection) fn restore_cancellation_preflight(
    c: &Connection,
    _store: &ProjectionStore,
) -> Result<RestoreCancellationData> {
    if c.is_autocommit() {
        return Err(invalid(
            "restore cancellation DATA requires an active snapshot",
        ));
    }
    if !matches!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
        9 | 10
    ) {
        return Err(invalid(
            "admitted restore cancellation requires admitted private9 or private10",
        ));
    }
    database::require_mutable(c)?;
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_preparations WHERE status='acquiring')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("pending preparation blocks restore cancellation"));
    }
    // Bind fresh DATA to the same active base without materializing its payload.
    let (count, generation, latest, digest_bytes): (u64, u64, u64, u64) = c.query_row(
        "SELECT count(*),h.generation,(SELECT max(generation) FROM projection_snapshots),length(CAST(s.digest AS BLOB)) FROM projection_head h JOIN projection_snapshots s ON s.generation=h.generation",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    )?;
    if count != 1 || generation != latest || digest_bytes != 64 {
        return Err(invalid(
            "restore cancellation active base missing or invalid",
        ));
    }
    let digest: String = c.query_row(
        "SELECT digest FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| r.get(0),
    )?;
    if !valid_digest(&digest) {
        return Err(invalid("restore cancellation base digest invalid"));
    }
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_budgets LIMIT 2)",
        [],
        |r| r.get(0),
    )?;
    if count > 1 {
        return Err(invalid("restore cancellation budget is ambiguous"));
    }
    let budget = if count == 0 {
        None
    } else {
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_budgets WHERE singleton!=1 OR length(CAST(operation AS BLOB))=0 OR length(CAST(operation AS BLOB))>128 OR length(CAST(reservation AS BLOB))=0 OR length(CAST(reservation AS BLOB))>128 OR length(CAST(witness AS BLOB))!=64 OR bytes<65536 OR state NOT IN ('active','committed'))",
            [], |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("restore cancellation budget exceeds bounds"));
        }
        let (operation, reservation, bytes, witness, state): (String, String, u64, String, String) = c.query_row(
            "SELECT operation,reservation,bytes,witness,state FROM projection_budgets WHERE singleton=1",
            [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        )?;
        restore_id(&operation)?;
        if !valid_digest(&witness)
            || !reservation.is_ascii()
            || reservation.chars().any(char::is_control)
        {
            return Err(invalid("restore cancellation budget proof invalid"));
        }
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_preparations WHERE reservation=?1)",
            [&reservation],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid(
                "restore cancellation reservation belongs to preparation",
            ));
        }
        Some(Budget {
            operation,
            reservation,
            bytes,
            witness,
            committed: state == "committed",
        })
    };
    let staging = staging::read_restore(c, budget.as_ref().map(|b| b.operation.as_str()))?;
    if budget.as_ref().is_some_and(|b| b.committed) && staging.is_some() {
        return Err(invalid(
            "committed restore cancellation still owns checkout staging",
        ));
    }
    Ok(RestoreCancellationData {
        generation,
        digest,
        budget,
        staging,
    })
}

pub(super) fn restore_id(operation: &str) -> Result<&str> {
    let id = operation
        .strip_prefix("restore:")
        .ok_or_else(|| invalid("cancellation only accepts restore operations"))?;
    let (nanos, pid) = id
        .split_once('-')
        .ok_or_else(|| invalid("restore cancellation operation is not canonical"))?;
    if !canonical_number::<u128>(nanos) || !canonical_number::<u32>(pid) {
        return Err(invalid("restore cancellation operation is not canonical"));
    }
    Ok(id)
}

pub(super) fn canonical_number<T>(value: &str) -> bool
where
    T: std::str::FromStr + std::fmt::Display,
{
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<T>()
            .ok()
            .is_some_and(|n| n.to_string() == value)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(in crate::projection) fn cancel_admitted_with_access(
    store: &ProjectionStore,
    access: format::Access,
) -> Result<()> {
    cancel_with_faults(store, access, &|_| Ok(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum RestoreCancellationPoint {
    InstallationsCleared,
    StagingCleared,
    StagingMarkedCleaning,
    StagingFileRemoved,
    StagingDirectoryRemoved,
    BudgetCommitted,
    ReservationReleased,
}

#[cfg(test)]
pub(in crate::projection) fn cancel_admitted_with_faults(
    store: &ProjectionStore,
    access: format::Access,
    faults: &dyn Fn(RestoreCancellationPoint) -> Result<()>,
) -> Result<()> {
    cancel_with_faults(store, access, faults)
}

fn cancel_with_faults(
    store: &ProjectionStore,
    access: format::Access,
    faults: &dyn Fn(RestoreCancellationPoint) -> Result<()>,
) -> Result<()> {
    access.run(|access| {
        let data = match &access.connection {
            format::PrivateConnection::Admitted(handle) => handle.with_validation(|c| {
                restore_cancellation_preflight(c, store).map_err(Into::into)
            })?,
            _ => return Err(invalid("restore cancellation lost admitted private handle")),
        };
        let lease = access.family.as_ref()
            .ok_or_else(|| invalid("restore cancellation lost admitted family"))?;
        data.validate_family(lease, store)?;
        data.validate_staging(store)?;
        let Some(budget) = &data.budget else {
            // An already completed retry has no authority over orphan family
            // reservations. It performs no settlement or artifact mutation.
            return Ok(());
        };
        lease.cancel_restore_installations(
            &store.marker, &budget.reservation, &budget.operation, budget.bytes, budget.committed,
        )?;
        faults(RestoreCancellationPoint::InstallationsCleared)?;
        if let Some(stage) = &data.staging {
            stage.cancel(&access.connection, store, faults)?;
        }
        faults(RestoreCancellationPoint::StagingCleared)?;
        // Autocommit makes the terminal retry reference durable before release.
        // A failed release leaves it intact; a failed DELETE remains retryable.
        if access.connection.execute(
            "UPDATE projection_budgets SET state='committed' WHERE singleton=1 AND reservation=?1 AND operation=?2",
            params![budget.reservation, budget.operation],
        )? != 1 {
            return Err(invalid("restore cancellation budget changed before settlement"));
        }
        faults(RestoreCancellationPoint::BudgetCommitted)?;
        settle_with_lease_after_release(&access.connection, store, Some(lease), || {
            faults(RestoreCancellationPoint::ReservationReleased)
        })?;
        access.verify()
    })
}
