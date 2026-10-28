//! Ordinary B9 -> C8 copied metadata, under retained caller transactions.
//! No opener, physical pin, family authorization, COMMIT or ordinary admission
//! is granted here. Fresh schema creation and complete prefix copying are one
//! destination transaction; a partial ingress is never a resumable state.
use super::{
    cancellation,
    checkout_journal::{CheckoutJournal, ExpectedCheckout},
    database_io,
    import_validation::{self, ImportDatabaseKind, VerifiedImportClosure},
    managed_admission::read_admitted_import,
    records::{
        import::{
            AdmittedSourceBinding, ImportSource, ImportSourceReferences, ProviderImportKind,
            ProviderImportPrefixRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionRole,
            TransitionRowData, TransitionState,
        },
        Record,
    },
    recovery::error::{invalid, Result},
    schema,
};
use crate::projection::{database, format, retention, selection};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection};

mod checkout;
pub(super) mod staging;
pub(in crate::projection::provider) use checkout::read_checkout_staging;

/// Borrow only from the actual ordinary owner callback. The source closure
/// binds this exact live source connection, while its actual durable Copying
/// row names C. The caller independently retains source image and C pins.
pub(in crate::projection::provider) struct OrdinaryCopyInputs<'a> {
    connection: &'a Connection,
    marker: &'a format::Marker,
    copying: &'a ProviderTransitionIntentRecord,
    closure: &'a VerifiedImportClosure,
}

impl<'a> OrdinaryCopyInputs<'a> {
    pub(in crate::projection::provider) fn new(
        connection: &'a Connection,
        marker: &'a format::Marker,
        copying: &'a ProviderTransitionIntentRecord,
        closure: &'a VerifiedImportClosure,
    ) -> Self {
        Self {
            connection,
            marker,
            copying,
            closure,
        }
    }

    fn prefix(&self) -> &ProviderImportPrefixRecord {
        &self.copying.plan.source_prefix
    }

    fn require(
        &self,
        destination: &Connection,
        marker: &format::Marker,
        cancel: Option<&Cancellation>,
    ) -> Result<ProviderTransitionIntentRecord> {
        self.require_phase(destination, marker, TransitionState::Copying, cancel)
    }

    /// Authenticate actual B9 SourceStaging and its exact C FORMAT binding.
    /// The returned destination Staging row is expected DATA only: this helper
    /// never reads C tables and does not claim C is still Staging. Ready ingress
    /// must separately validate the complete actual C prefix/suffix or journal.
    pub(in crate::projection::provider) fn authenticate_staging_source(
        &self,
        destination: &Connection,
        marker: &format::Marker,
        cancel: Option<&Cancellation>,
    ) -> Result<ProviderTransitionIntentRecord> {
        self.require_phase(destination, marker, TransitionState::Staging, cancel)
    }

    // The expected phase must match the actual durable source row. A proposal
    // alone cannot authorize inspection of C, including during a phase gap.
    fn require_phase(
        &self,
        destination: &Connection,
        marker: &format::Marker,
        phase: TransitionState,
        cancel: Option<&Cancellation>,
    ) -> Result<ProviderTransitionIntentRecord> {
        cancellation::check(cancel)?;
        if self.connection.is_autocommit()
            || destination.is_autocommit()
            || std::ptr::eq(self.connection, destination)
        {
            return Err(invalid(
                "ordinary copy requires distinct retained transactions",
            ));
        }
        self.closure.require_source_connection(self.connection)?;
        self.copying.validate_fields()?;
        let prefix = self.prefix();
        let ImportSource::Ordinary {
            private_schema: 9 | 10,
            admission: Some(_),
            version,
            ..
        } = &prefix.source
        else {
            return Err(invalid(
                "ordinary destination requires admitted private9 source",
            ));
        };
        if prefix.kind != ProviderImportKind::ManagedOrdinary
            || !prefix.supports_restore_history()
            || version.generation == 0
            || self.copying.role != TransitionRole::Source
            || !matches!(phase, TransitionState::Copying | TransitionState::Staging)
            || self.copying.state != phase
            || self.copying.local_generation != version.generation
            || self.copying.reservation.is_some() != (phase == TransitionState::Staging)
            || self.copying.receipt_id.is_some()
            || self.closure.prefix_id() != prefix.digest()?
            || self.closure.closure() != &prefix.closure
        {
            return Err(invalid("ordinary destination source phase/closure differs"));
        }
        let admitted = read_admitted_import(self.connection, self.marker, cancel)?;
        if !admitted.outgoing().iter().any(|row| row == self.copying) {
            return Err(invalid("ordinary destination source phase is not durable"));
        }
        let shared = self
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary destination source family absent"))?;
        let binding = AdmittedSourceBinding {
            admission_digest: admitted.record().digest()?,
            incoming_prefix_id: admitted.record().prefix_id.clone(),
            incoming_receipt_id: admitted.record().receipt_id.clone(),
            database_identity: shared.database_identity.clone(),
        };
        prefix.require_references(ImportSourceReferences::AdmittedOrdinary {
            marker: self.marker,
            admission: &binding,
        })?;
        let proof = self
            .copying
            .destination
            .as_ref()
            .ok_or_else(|| invalid("ordinary destination Copying proof absent"))?;
        marker.validate()?;
        let target = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary destination family absent"))?;
        if proof.backend != TransitionBackendKind::Ordinary
            || proof.format != *marker
            || proof.private_database_identity != target.database_identity
            || proof.family_marker_digest != admitted.record().binding.family_marker_digest
            || proof.native_control_identity_hex.is_some()
            || proof.native_root_identity.is_some()
            || marker.identity != prefix.destination_identity
            || marker.identity == self.marker.identity
            || marker.identity.store_id != self.marker.identity.store_id
            || marker.identity.space_id != self.marker.identity.space_id
            || marker.identity.origin != self.marker.identity.origin
            || marker.identity.principal_id != self.marker.identity.principal_id
            || marker.import_digest.as_deref() != Some(prefix.digest()?.as_str())
            || marker.initial_snapshot_digest != self.marker.initial_snapshot_digest
            || target.root != shared.root
            || target.incarnation != shared.incarnation
            || target.root_identity != shared.root_identity
        {
            return Err(invalid(
                "ordinary destination FORMAT/source binding differs",
            ));
        }
        marker.validate_initial_selection(&selection::at(self.connection, 0)?)?;
        let mut incoming = ProviderTransitionIntentRecord::decode(&self.copying.encode()?)?;
        incoming.role = TransitionRole::Destination;
        incoming.local_generation = 0;
        incoming.validate_fields()?;
        cancellation::check(cancel)?;
        Ok(incoming)
    }
}

/// Result marker for this validation call, not retained SQL authority. Every
/// later operation must read again under its own actual transaction. This grants
/// neither Ready nor C9 admission; writers still require commit and positive close.
pub(in crate::projection::provider) struct CopiedPrefix {
    _private: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum CopyEffect {
    Copied,
    AlreadyPresent,
}

/// Accept only strict uninitialized storage or an exact complete Copied replay.
/// Every error requires caller rollback, including errors after creating DDL.
pub(in crate::projection::provider) fn initialize_or_replay_copied(
    destination: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<(CopiedPrefix, CopyEffect)> {
    let incoming = source.require(destination, marker, cancel)?;
    require_foreign_keys(destination)?;
    let version: u32 = destination.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 0 {
        return Ok((
            read_copied(destination, marker, source, cancel)?,
            CopyEffect::AlreadyPresent,
        ));
    }
    if !database_io::schema_inventory(destination)?.is_empty() {
        return Err(invalid(
            "ordinary destination requires strict fresh empty schema",
        ));
    }
    cancellation::check(cancel)?;
    destination.execute_batch(
        &[
            database::SQL,
            selection::SQL,
            retention::SQL,
            schema::SQL,
            "PRAGMA user_version=8;",
        ]
        .concat(),
    )?;
    destination.execute(
        "INSERT INTO projection_meta VALUES(1,?1)",
        [marker.bytes()?],
    )?;
    destination.execute(
        "INSERT INTO projection_backend VALUES(1,'ordinary',NULL)",
        [],
    )?;
    destination.execute(
        "INSERT INTO projection_imports VALUES(1,?1,?2)",
        params![source.prefix().digest()?, source.prefix().encode()?],
    )?;
    // The shared copier inserts generation zero and its selection from B's
    // authenticated prefix. Only then can the FK-bound destination intent and
    // head be inserted; foreign keys stay ON throughout the atomic operation.
    import_validation::copy_verified_prefix(
        source.connection,
        destination,
        marker,
        source.prefix(),
        source.closure,
        cancel,
    )?;
    destination.execute(
        "INSERT INTO projection_provider_transitions(id,role,state,local_generation,reservation,receipt_id,payload,digest) \
         VALUES(?1,'destination','copying',0,NULL,NULL,?2,?3)",
        params![incoming.transaction_id, incoming.encode()?, incoming.digest()?],
    )?;
    destination.execute(
        "INSERT INTO projection_head VALUES(1,?1)",
        [source.copying.local_generation],
    )?;
    let copied = read_copied(destination, marker, source, cancel)?;
    cancellation::check(cancel)?;
    Ok((copied, CopyEffect::Copied))
}

fn require_foreign_keys(c: &Connection) -> Result<()> {
    if c.is_autocommit() || c.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u64>(0))? != 1
    {
        return Err(invalid(
            "ordinary destination requires FK-enabled caller transaction",
        ));
    }
    Ok(())
}

/// Inspect all copied tables and all operational tables. Populated restore
/// history is an imported rowset; arbitrary pending work remains forbidden.
pub(in crate::projection::provider) fn read_copied(
    destination: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<CopiedPrefix> {
    let incoming = source.require(destination, marker, cancel)?;
    read_copied_parts(destination, marker, source, &incoming, cancel)
}

// Callers authenticate the actual source phase before supplying the expected
// destination row. This also validates C Copying after B commits Staging.
fn read_copied_parts(
    destination: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    incoming: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<CopiedPrefix> {
    read_copied_kernel(
        destination,
        marker,
        source,
        incoming,
        Prejournal::Empty,
        cancel,
    )?;
    Ok(CopiedPrefix { _private: () })
}

// Only the explicit checkout reader can permit projection_prejournal progress;
// every other operational table and the complete copied prefix remain strict.
enum Prejournal<'a> {
    Empty,
    Checkout(&'a ExpectedCheckout),
}
fn read_copied_kernel(
    destination: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    incoming: &ProviderTransitionIntentRecord,
    prejournal: Prejournal<'_>,
    cancel: Option<&Cancellation>,
) -> Result<Option<CheckoutJournal>> {
    import_validation::require_schema(destination, marker, ImportDatabaseKind::Provider, cancel)?;
    require_inventory_mode(destination, &prejournal, cancel)?;
    let head: (u64, u64) = destination.query_row(
        "SELECT singleton,generation FROM projection_head",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if head != (1, source.copying.local_generation) {
        return Err(invalid("ordinary copied destination head differs"));
    }
    if !destination.query_row(
        "SELECT backend='ordinary' AND mount_id IS NULL FROM projection_backend WHERE singleton=1",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(invalid("ordinary copied destination backend differs"));
    }
    let initial: String = destination.query_row(
        "SELECT digest FROM projection_snapshots WHERE generation=0 AND parent_digest IS NULL AND length(CAST(digest AS BLOB))=64",
        [], |row| row.get(0),
    )?;
    if initial != marker.initial_snapshot_digest {
        return Err(invalid(
            "ordinary copied initial snapshot differs from FORMAT",
        ));
    }
    marker.validate_initial_selection(&selection::at(destination, 0)?)?;
    require_prefix_row(destination, source.prefix())?;
    require_intent_row(destination, incoming)?;
    // Includes exact counts and closure hashes for all thirteen v2 rowsets,
    // including restore history, and refuses any tail outside the copied scope.
    import_validation::require_exact_copied_rows(
        destination,
        marker,
        source.prefix(),
        source.closure,
        cancel,
    )?;
    if destination.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(invalid("ordinary copied destination foreign key violation"));
    }
    let journal = match prejournal {
        Prejournal::Empty => None,
        Prejournal::Checkout(expected) => Some(super::checkout_journal::read(
            destination,
            expected,
            cancel,
        )?),
    };
    cancellation::check(cancel)?;
    Ok(journal)
}

fn require_inventory(c: &Connection, cancel: Option<&Cancellation>) -> Result<()> {
    require_inventory_mode(c, &Prejournal::Empty, cancel)
}
fn require_inventory_mode(
    c: &Connection,
    prejournal: &Prejournal<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    // These names are accepted only after the complete exact schema fingerprint.
    for (kind, name, _) in database_io::schema_inventory(c)? {
        cancellation::check(cancel)?;
        if kind != "table" {
            continue;
        }
        let expected = match name.as_str() {
            "projection_meta"
            | "projection_head"
            | "projection_backend"
            | "projection_imports"
            | "projection_provider_transitions" => Some(1),
            "projection_prejournal" if matches!(prejournal, Prejournal::Checkout(_)) => None,
            "projection_snapshots"
            | "projection_selections"
            | "projection_captures"
            | "projection_publications"
            | "projection_provider_catalogs"
            | "projection_provider_descriptors"
            | "projection_provider_activations"
            | "projection_provider_transition_receipts"
            | "projection_provider_companions"
            | "projection_provider_companion_files"
            | "projection_provider_companion_deletions"
            | "projection_content"
            | "projection_restore_history" => None,
            _ => Some(0),
        };
        if let Some(expected) = expected {
            let count: u64 = c.query_row(
                &format!("SELECT count(*) FROM (SELECT 1 FROM \"{name}\" LIMIT 2)"),
                [],
                |row| row.get(0),
            )?;
            if count != expected {
                return Err(invalid(
                    "ordinary copied operational table cardinality differs",
                ));
            }
        }
    }
    Ok(())
}

fn require_prefix_row(c: &Connection, expected: &ProviderImportPrefixRecord) -> Result<()> {
    let (digest_bytes, payload_bytes): (u64, u64) = c.query_row(
        "SELECT length(CAST(digest AS BLOB)),length(payload) FROM projection_imports WHERE singleton=1",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if digest_bytes != 71
        || payload_bytes == 0
        || payload_bytes > ProviderImportPrefixRecord::MAX_BYTES as u64
    {
        return Err(invalid("ordinary copied import row exceeds bound"));
    }
    let (digest, payload): (String, Vec<u8>) = c.query_row(
        "SELECT digest,payload FROM projection_imports WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    expected.require_row(&digest, &payload)?;
    Ok(())
}

fn require_intent_row(c: &Connection, expected: &ProviderTransitionIntentRecord) -> Result<()> {
    let bad: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE length(CAST(id AS BLOB))>128 \
         OR length(CAST(role AS BLOB))>16 OR length(CAST(state AS BLOB))>16 OR length(CAST(digest AS BLOB))!=71 \
         OR length(payload)=0 OR length(payload)>262144 OR length(CAST(reservation AS BLOB))>128 OR length(CAST(receipt_id AS BLOB))>71)",
        [], |row| row.get(0),
    )?;
    if bad {
        return Err(invalid("ordinary copied transition row exceeds bound"));
    }
    let (id, role, state, generation, reservation, receipt, payload, digest):
        (String, String, String, u64, Option<String>, Option<String>, Vec<u8>, String) = c.query_row(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
    )?;
    expected.require_row(TransitionRowData {
        id: &id,
        role: &role,
        state: &state,
        local_generation: generation,
        reservation: reservation.as_deref(),
        receipt_id: receipt.as_deref(),
        payload: &payload,
        digest: &digest,
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "ordinary_destination/tests.rs"]
pub(in crate::projection::provider) mod tests;
