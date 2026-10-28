//! Closed ordinary publication source SQL DATA. Original Staging is calculation
//! DATA only; every use authenticates the actual B Staging/Ready/Published row.
//! This retained-owner reader requires the original current frontier. It is not
//! the historical Published handoff reader after B advances, nor physical proof.
use super::{
    cancellation,
    import_validation::VerifiedImportClosure,
    managed_admission::read_admitted_import,
    ordinary_transition::read_publication_state,
    records::{
        import::{
            AdmittedSourceBinding, ImportSource, ImportSourceReferences, ProviderImportKind,
            ProviderTransitionReceiptRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionRole, TransitionState,
        },
        Record,
    },
    recovery::error::{invalid, Result},
};
use crate::projection::{format, selection};
use layrs_vfs::Cancellation;
use rusqlite::Connection;

pub(in crate::projection) struct OrdinaryPublicationSource<'a> {
    connection: &'a Connection,
    marker: &'a format::Marker,
    closure: &'a VerifiedImportClosure,
    staging: &'a ProviderTransitionIntentRecord,
    destination_ready: &'a ProviderTransitionIntentRecord,
    receipt: &'a ProviderTransitionReceiptRecord,
}

impl<'a> OrdinaryPublicationSource<'a> {
    pub(in crate::projection) fn new(
        connection: &'a Connection,
        marker: &'a format::Marker,
        closure: &'a VerifiedImportClosure,
        original_staging: &'a ProviderTransitionIntentRecord,
        expected_c_ready: &'a ProviderTransitionIntentRecord,
        expected_c_receipt: &'a ProviderTransitionReceiptRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        let source = Self {
            connection,
            marker,
            closure,
            staging: original_staging,
            destination_ready: expected_c_ready,
            receipt: expected_c_receipt,
        };
        source.revalidate(cancel)?;
        Ok(source)
    }

    pub(in crate::projection) fn connection(&self) -> &Connection {
        self.connection
    }
    pub(in crate::projection) fn marker(&self) -> &format::Marker {
        self.marker
    }
    pub(in crate::projection) fn closure(&self) -> &VerifiedImportClosure {
        self.closure
    }
    pub(in crate::projection) fn staging(&self) -> &ProviderTransitionIntentRecord {
        self.staging
    }
    pub(in crate::projection) fn destination_ready(&self) -> &ProviderTransitionIntentRecord {
        self.destination_ready
    }
    pub(in crate::projection) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        self.receipt
    }

    pub(in crate::projection) fn revalidate(
        &self,
        cancel: Option<&Cancellation>,
    ) -> Result<ProviderTransitionIntentRecord> {
        cancellation::check(cancel)?;
        self.closure.require_source_connection(self.connection)?;
        self.staging.validate_fields()?;
        let prefix = &self.staging.plan.source_prefix;
        let ImportSource::Ordinary {
            private_schema: 9 | 10,
            admission: Some(_),
            version,
            ..
        } = &prefix.source
        else {
            return Err(invalid("ordinary publication requires admitted B9 source"));
        };
        if self.staging.role != TransitionRole::Source
            || self.staging.state != TransitionState::Staging
            || self.staging.local_generation != version.generation
            || self.staging.plan.kind != ProviderImportKind::ManagedOrdinary
            || !prefix.supports_restore_history()
            || self.staging.reservation.is_none()
            || self.staging.receipt_id.is_some()
            || self.closure.prefix_id() != prefix.digest()?
            || self.closure.closure() != &prefix.closure
            || self.destination_ready.role != TransitionRole::Destination
            || self.destination_ready.state != TransitionState::Ready
        {
            return Err(invalid(
                "ordinary publication source calculation DATA differs",
            ));
        }
        let mut destination_staging =
            ProviderTransitionIntentRecord::decode(&self.staging.encode()?)?;
        destination_staging.role = TransitionRole::Destination;
        destination_staging.local_generation = 0;
        self.destination_ready
            .require_transition(&destination_staging)?;
        self.destination_ready.require_receipt(self.receipt)?;
        let mut ready = ProviderTransitionIntentRecord::decode(&self.destination_ready.encode()?)?;
        ready.role = TransitionRole::Source;
        ready.local_generation = version.generation;
        ready.require_transition(self.staging)?;
        ready.require_receipt(self.receipt)?;
        let mut published = ProviderTransitionIntentRecord::decode(&ready.encode()?)?;
        published.state = TransitionState::Published;
        published.require_transition(&ready)?;
        published.require_receipt(self.receipt)?;
        let actual = read_publication_state(
            self.connection,
            self.marker,
            &self.staging.transaction_id,
            cancel,
        )?;
        let expected = match actual.intent.state {
            TransitionState::Staging => self.staging,
            TransitionState::Ready => &ready,
            TransitionState::Published => &published,
            _ => return Err(invalid("ordinary publication actual source phase differs")),
        };
        if &actual.intent != expected
            || match actual.intent.state {
                TransitionState::Staging => actual.receipt.is_some(),
                _ => actual.receipt.as_ref() != Some(self.receipt),
            }
        {
            return Err(invalid(
                "ordinary publication actual source row/receipt differs",
            ));
        }
        // Fresh proof is intentional: outgoing provenance changes at each B CAS.
        let admitted = read_admitted_import(self.connection, self.marker, cancel)?;
        let shared = self
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary publication source family absent"))?;
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
        let (generation, captures): (u64, u64) = self.connection.query_row(
            "SELECT (SELECT generation FROM projection_head WHERE singleton=1),(SELECT count(*) FROM (SELECT 1 FROM projection_captures LIMIT 1000001))",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if generation != version.generation || captures != version.capture_count {
            return Err(invalid(
                "ordinary publication retained current frontier changed",
            ));
        }
        cancellation::check(cancel)?;
        Ok(actual.intent)
    }

    /// Authenticate actual source phase and exact C FORMAT before reading C.
    /// The returned Staging pair is only calculation DATA, never actual B/C.
    pub(in crate::projection::provider) fn authenticate_destination(
        &self,
        destination: &Connection,
        marker: &format::Marker,
        cancel: Option<&Cancellation>,
    ) -> Result<ProviderTransitionIntentRecord> {
        if destination.is_autocommit() || std::ptr::eq(self.connection, destination) {
            return Err(invalid("ordinary publication requires distinct retained C"));
        }
        self.revalidate(cancel)?;
        let admitted = read_admitted_import(self.connection, self.marker, cancel)?;
        let prefix = &self.staging.plan.source_prefix;
        let proof = self
            .staging
            .destination
            .as_ref()
            .ok_or_else(|| invalid("ordinary publication destination proof absent"))?;
        marker.validate()?;
        let target = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary publication destination family absent"))?;
        let shared = self
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary publication source family absent"))?;
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
                "ordinary publication destination FORMAT/source binding differs",
            ));
        }
        marker.validate_initial_selection(&selection::at(self.connection, 0)?)?;
        let mut staging = ProviderTransitionIntentRecord::decode(&self.staging.encode()?)?;
        staging.role = TransitionRole::Destination;
        staging.local_generation = 0;
        staging.validate_fields()?;
        Ok(staging)
    }
}
