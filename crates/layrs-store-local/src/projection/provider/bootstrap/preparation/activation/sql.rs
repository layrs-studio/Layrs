//! Exact private generation-one transaction, followed by family READY CAS.
//! Both writer opens require a completed clean read-only admission.
use super::*;
use rusqlite::params;

pub(super) fn private_transition(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    prepared: &ProviderPreparationRecord,
    activation: &ProviderActivationRecord,
    active: &ProviderActiveMountRecord,
    snapshot: &ProjectionSnapshot,
    context: &mut report::Context,
) -> std::result::Result<(), preparation_report::Failure> {
    boundary(held, root).map_err(preparation_report::Failure::private)?;
    let database = &held.artifacts.database;
    let validate = |c: &Connection| -> CauseResult<()> {
        let p = held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("activation P absent"))?;
        let value = validation::private_preparation_with_source(
            c,
            &held.artifacts.allocation,
            p,
            &held.binding,
            root.registration().root_id(),
        )?;
        if value.mount != *prepared || value.source.commitment() != &prepared.source {
            return Err(invalid("private activation predecessor changed"));
        }
        activation.require_references(
            &held.artifacts.allocation,
            p,
            prepared,
            &value.source,
            snapshot,
        )?;
        active.require_references(
            &held.artifacts.allocation,
            p,
            &value.source,
            activation,
            snapshot,
        )?;
        Ok(())
    };
    let admission = database_io::clean_admission(database, false, validate)
        .map_err(preparation_report::Failure::private)?;
    boundary(held, root).map_err(preparation_report::Failure::private)?;
    let phase = ProviderActivationSqlPhase::PrivateActivation;
    let open = context
        .arm(ProviderActivationWriteStep::SqlOpen(phase), database.path())
        .map_err(preparation_report::Failure::private)?;
    let bare = {
        let mut observed = report::CloseLedger::new(context, phase, database.path());
        admission
            .open(database, &mut observed)
            .map_err(preparation_report::Failure::private)?
    };
    let result = (|| -> CauseResult<()> {
        admission.require_geometry(&bare)?;
        context.confirm(open)?;
        let transaction = context.arm(
            ProviderActivationWriteStep::SqlTransaction(phase),
            database.path(),
        )?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        validate(c)?;
        let snapshot_bytes = snapshot
            .canonical_bytes()
            .map_err(|error| invalid(error.to_string()))?;
        let inserted = c.execute("INSERT INTO projection_snapshots(generation,digest,parent_digest,payload) VALUES(1,?1,?2,?3)",
            params![activation.activated_digest, activation.source_digest, snapshot_bytes])?;
        if inserted != 1 {
            return Err(invalid("activation generation-one insert changed no row"));
        }
        let changed = c.execute(
            "UPDATE projection_head SET generation=1 WHERE singleton=1 AND generation=0",
            [],
        )?;
        if changed != 1 {
            return Err(invalid("activation head CAS changed no row"));
        }
        let inserted = c.execute("INSERT INTO projection_provider_activations(id,owner_identity_digest,mount_id,catalog_id,source_generation,source_digest,activated_generation,activated_digest,payload) VALUES(?1,?2,?3,?4,0,?5,1,?6,?7)",
            params![activation.digest()?, activation.owner_identity_digest, activation.mount_id, activation.catalog_id,
                activation.source_digest, activation.activated_digest, activation.encode()?])?;
        if inserted != 1 {
            return Err(invalid("activation row insert changed no row"));
        }
        let changed = c.execute("UPDATE projection_provider_mounts SET phase='active',payload=?1,digest=?2 WHERE id=?3 AND catalog_id=?4 AND phase='provider_prepared' AND payload=?5 AND digest=?6",
            params![active.encode()?, active.digest()?, prepared.bootstrap.transaction_id, prepared.source.catalog_id,
                prepared.encode()?, prepared.digest()?])?;
        if changed != 1 {
            return Err(invalid("activation mount CAS changed no exact predecessor"));
        }
        let p = held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("activation P absent"))?;
        let readback = validation::private_active_with_source(
            c,
            &held.artifacts.allocation,
            p,
            &held.binding,
            Some(root.registration().root_id()),
            None,
        )?;
        if readback.mount != *active
            || readback.activation != *activation
            || readback.activated.snapshot != *snapshot
        {
            return Err(invalid("private activation readback changed"));
        }
        context.confirm(transaction)?;
        boundary(held, root)?;
        let commit = context.arm(
            ProviderActivationWriteStep::SqlCommit(phase),
            database.path(),
        )?;
        c.execute_batch("COMMIT")?;
        context.confirm(commit)?;
        Ok(())
    })();
    let mut observed = report::CloseLedger::new(context, phase, database.path());
    bare.finish_writer(result, &mut observed)
        .map_err(preparation_report::Failure::private)
}

pub(super) fn family_transition(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    prepared: &ProviderPreparationRecord,
    active: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    context: &mut report::Context,
) -> std::result::Result<(), preparation_report::Failure> {
    boundary(held, root).map_err(preparation_report::Failure::from)?;
    let PrivateState::Active(checked_mount, checked_activation) =
        private_state(held, root, prepared)?
    else {
        return Err(preparation_report::Failure::private(invalid(
            "family READY requires durable private activation",
        )));
    };
    if checked_mount != *active || checked_activation != *activation {
        return Err(preparation_report::Failure::private(invalid(
            "private activation changed before family READY",
        )));
    }
    let database = &held.family.catalog;
    let validate = |c: &Connection| -> CauseResult<()> {
        if held
            .family
            .validate_activation_catalog(c, &held.artifacts.allocation, None)?
            != ActivationRegistration::Initializing
        {
            return Err(invalid("family READY predecessor changed"));
        }
        Ok(())
    };
    let admission = database_io::clean_admission(database, false, validate).map_err(|error| {
        preparation_report::Failure::new(error, Some(ProviderDatabaseScope::FamilyCatalog))
    })?;
    boundary(held, root).map_err(preparation_report::Failure::from)?;
    let PrivateState::Active(mount, row) = private_state(held, root, prepared)? else {
        return Err(preparation_report::Failure::private(invalid(
            "private activation changed after family admission",
        )));
    };
    if mount != *active || row != *activation {
        return Err(preparation_report::Failure::private(invalid(
            "private activation differs after family admission",
        )));
    }
    let phase = ProviderActivationSqlPhase::FamilyReady;
    let family_error =
        |cause| preparation_report::Failure::new(cause, Some(ProviderDatabaseScope::FamilyCatalog));
    let open = context
        .arm(ProviderActivationWriteStep::SqlOpen(phase), database.path())
        .map_err(family_error)?;
    let bare = {
        let mut observed = report::CloseLedger::new(context, phase, database.path());
        admission
            .open(database, &mut observed)
            .map_err(family_error)?
    };
    let result = (|| -> CauseResult<()> {
        admission.require_geometry(&bare)?;
        context.confirm(open)?;
        let transaction = context.arm(
            ProviderActivationWriteStep::SqlTransaction(phase),
            database.path(),
        )?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        validate(c)?;
        let a = &held.artifacts.allocation;
        let changed = c.execute("UPDATE worktrees SET state='ready' WHERE id=?1 AND metadata_path=?2 AND database_identity=?3 AND marker=?4 AND state='initializing'",
            params![a.format.identity.worktree_id.as_str(), a.private.metadata_path, a.private.database_file_identity,
                a.format.bytes()?])?;
        if changed != 1 {
            return Err(invalid("family READY CAS changed no exact owner"));
        }
        if held.family.validate_activation_catalog(c, a, None)? != ActivationRegistration::Ready {
            return Err(invalid("family READY SQL readback differs"));
        }
        context.confirm(transaction)?;
        boundary(held, root)?;
        let commit = context.arm(
            ProviderActivationWriteStep::SqlCommit(phase),
            database.path(),
        )?;
        c.execute_batch("COMMIT")?;
        context.confirm(commit)?;
        Ok(())
    })();
    let mut observed = report::CloseLedger::new(context, phase, database.path());
    bare.finish_writer(result, &mut observed)
        .map_err(family_error)
}
