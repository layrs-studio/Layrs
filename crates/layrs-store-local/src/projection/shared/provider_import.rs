//! Family SQL DATA for a fresh managed ordinary destination. The caller owns
//! the catalog lock, source and destination pins, SQL transaction and close.
//! These methods neither create a filesystem member nor grant Ready/admission.
use super::*;
use crate::projection::provider::{
    cancellation,
    records::{
        activation::ProviderActivationRecord,
        import::{ImportSource, ImportSourceReferences, ProviderImportKind},
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionRole, TransitionState,
        },
        AllocationRecord, PreparedRecord, Record,
    },
    recovery::error::{invalid, Result},
    validation::PrivateCaptureProof,
};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ImportRegistration {
    Absent,
    Initializing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ImportRegistrationEffect {
    Inserted,
    AlreadyPresent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnreservedPhase {
    Copying,
    Staging,
}

/// Require the source's Ready family row and all of its capture protections,
/// then inspect only the destination's exact registry row and owner claims.
/// The source Copying intent must also have been interpreted from the pinned
/// private SQL transaction by the caller; this catalog cannot read that DB.
#[allow(clippy::too_many_arguments)]
fn validate_import_initializing(
    family: &FamilyAccess,
    c: &Connection,
    source_a: &AllocationRecord,
    source_p: &PreparedRecord,
    source_activation: &ProviderActivationRecord,
    source_capture: &PrivateCaptureProof,
    source_copying: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<ImportRegistration> {
    validate_unreserved(
        family,
        c,
        source_a,
        source_p,
        source_activation,
        source_capture,
        source_copying,
        UnreservedPhase::Copying,
        cancel,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_unreserved(
    family: &FamilyAccess,
    c: &Connection,
    source_a: &AllocationRecord,
    source_p: &PreparedRecord,
    source_activation: &ProviderActivationRecord,
    source_capture: &PrivateCaptureProof,
    source_copying: &ProviderTransitionIntentRecord,
    phase: UnreservedPhase,
    cancel: Option<&Cancellation>,
) -> Result<ImportRegistration> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid("import family read requires retained transaction"));
    }
    family.require_allocation(source_a)?;
    if family.validate_capture_catalog(c, source_a, source_capture, cancel)?
        != ActivationRegistration::Ready
    {
        return Err(invalid("import source family is not Ready"));
    }
    source_copying.validate_fields()?;
    let Some(destination) = source_copying.destination.as_ref() else {
        return Err(invalid("import Copying has no destination proof"));
    };
    if source_copying.role != TransitionRole::Source
        || source_copying.state
            != match phase {
                UnreservedPhase::Copying => TransitionState::Copying,
                UnreservedPhase::Staging => TransitionState::Staging,
            }
        || source_copying.reservation.is_some() != (phase == UnreservedPhase::Staging)
        || source_copying.receipt_id.is_some()
        || source_copying.plan.kind != ProviderImportKind::ManagedOrdinary
        || destination.backend != TransitionBackendKind::Ordinary
        || destination.native_control_identity_hex.is_some()
        || destination.native_root_identity.is_some()
    {
        return Err(invalid(
            "import requires exact ordinary source phase without family effects",
        ));
    }
    source_copying
        .plan
        .source_prefix
        .require_references(ImportSourceReferences::Provider {
            allocation: source_a,
            prepared: source_p,
            activation: source_activation,
        })?;
    let ImportSource::Provider { version, .. } = &source_copying.plan.source_prefix.source else {
        return Err(invalid("import family source prefix is not provider"));
    };
    if version.capture_count != source_capture.patches.len() as u64
        || version.capture_head.as_deref()
            != source_capture
                .patches
                .last()
                .map(|patch| patch.digest.as_str())
    {
        return Err(invalid(
            "import source capture head differs from family proof",
        ));
    }
    let marker = &destination.format;
    let dest_worktree = &marker.identity.worktree_id;
    let shared = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("import destination is not shared"))?;
    let expected_identity = family.writer_identity(dest_worktree);
    let member = family.member(dest_worktree);
    let member_path = exact_path(&member)?;
    let marker_bytes = marker.bytes()?;
    if marker.identity != expected_identity
        || marker.identity == source_a.format.identity
        || shared.root != family.root()
        || shared.incarnation != source_a.family.incarnation
        || shared.root_identity != source_a.family.root_identity
        || shared.metadata_path != member
        || shared.database_identity != destination.private_database_identity
        || destination.family_marker_digest != family.marker_digest()
        || marker_bytes.len() > 65536
    {
        return Err(invalid("import destination family/FORMAT binding differs"));
    }
    cancellation::check(cancel)?;
    let owner = dest_worktree.as_str();
    // Schema, marker, source protections and global FK were fenced above.
    // Test all target lengths before allocating TEXT/BLOB from a corrupt row.
    let bad: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND \
         (length(CAST(metadata_path AS BLOB))>4096 OR \
          length(CAST(database_identity AS BLOB))>512 OR \
          length(CAST(state AS BLOB))>12 OR length(marker)>65536))",
        [owner],
        |row| row.get(0),
    )?;
    if bad {
        return Err(invalid("import target registry row exceeds bound"));
    }
    let target: Option<(String, String, Vec<u8>, String)> = c
        .query_row(
            "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
            [owner],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let registration = match target {
        None => ImportRegistration::Absent,
        Some((path, id, bytes, state))
            if path == member_path
                && id == destination.private_database_identity
                && bytes == marker_bytes
                && state == "initializing" =>
        {
            ImportRegistration::Initializing
        }
        Some(_) => {
            return Err(invalid(
                "import target registry row differs from Initializing",
            ))
        }
    };
    if phase == UnreservedPhase::Staging && registration != ImportRegistration::Initializing {
        return Err(invalid(
            "import Staging requires existing Initializing registration",
        ));
    }
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
    ] {
        cancellation::check(cancel)?;
        let present: bool = c.query_row(sql, [owner], |row| row.get(0))?;
        if present {
            return Err(invalid("import destination has a root/reservation/installation"));
        }
    }
    family.verify()?;
    cancellation::check(cancel)?;
    Ok(registration)
}

/// Insert the exact Initializing row once, or read back an identical replay.
/// The caller commits or rolls back and observably closes the family writer.
#[allow(clippy::too_many_arguments)]
fn register_import_initializing(
    family: &FamilyAccess,
    c: &Connection,
    source_a: &AllocationRecord,
    source_p: &PreparedRecord,
    source_activation: &ProviderActivationRecord,
    source_capture: &PrivateCaptureProof,
    source_copying: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<ImportRegistrationEffect> {
    cancellation::check(cancel)?;
    if c.is_autocommit() || c.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u64>(0))? != 1
    {
        return Err(invalid(
            "import family writer requires FK-enabled caller transaction",
        ));
    }
    let before = validate_import_initializing(
        family,
        c,
        source_a,
        source_p,
        source_activation,
        source_capture,
        source_copying,
        cancel,
    )?;
    if before == ImportRegistration::Initializing {
        return Ok(ImportRegistrationEffect::AlreadyPresent);
    }
    let destination = source_copying
        .destination
        .as_ref()
        .ok_or_else(|| invalid("import destination absent"))?;
    let owner = destination.format.identity.worktree_id.as_str();
    let member = family.member(&destination.format.identity.worktree_id);
    let inserted = c.execute(
        "INSERT INTO worktrees(id,metadata_path,database_identity,marker,state) VALUES(?1,?2,?3,?4,'initializing')",
        params![owner, exact_path(&member)?, destination.private_database_identity, destination.format.bytes()?],
    )?;
    if inserted != 1
        || validate_import_initializing(
            family,
            c,
            source_a,
            source_p,
            source_activation,
            source_capture,
            source_copying,
            cancel,
        )? != ImportRegistration::Initializing
    {
        return Err(invalid(
            "import family Initializing insert/readback differs",
        ));
    }
    Ok(ImportRegistrationEffect::Inserted)
}

impl FamilyAccess {
    /// The private Staging intent names its future reservation, but no family
    /// reservation, installation or protection may exist yet. Caller proves
    /// this exact Staging row from the retained source transaction.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection) fn validate_import_staging_unreserved(
        &self,
        c: &Connection,
        source_a: &AllocationRecord,
        source_p: &PreparedRecord,
        source_activation: &ProviderActivationRecord,
        source_capture: &PrivateCaptureProof,
        source_staging: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        validate_unreserved(
            self,
            c,
            source_a,
            source_p,
            source_activation,
            source_capture,
            source_staging,
            UnreservedPhase::Staging,
            cancel,
        )?;
        Ok(())
    }

    /// Full source Ready and destination Initializing/Absent SQL interpretation
    /// under the caller's held family catalog gate and transaction.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection) fn validate_import_initializing(
        &self,
        c: &Connection,
        source_a: &AllocationRecord,
        source_p: &PreparedRecord,
        source_activation: &ProviderActivationRecord,
        source_capture: &PrivateCaptureProof,
        source_copying: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportRegistration> {
        validate_import_initializing(
            self,
            c,
            source_a,
            source_p,
            source_activation,
            source_capture,
            source_copying,
            cancel,
        )
    }

    /// Exact insert or no-write replay; commit and close remain caller-owned.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection) fn register_import_initializing(
        &self,
        c: &Connection,
        source_a: &AllocationRecord,
        source_p: &PreparedRecord,
        source_activation: &ProviderActivationRecord,
        source_capture: &PrivateCaptureProof,
        source_copying: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportRegistrationEffect> {
        register_import_initializing(
            self,
            c,
            source_a,
            source_p,
            source_activation,
            source_capture,
            source_copying,
            cancel,
        )
    }
}
