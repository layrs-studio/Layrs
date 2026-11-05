//! Bounded outgoing ordinary intent DATA. Incoming admission is authenticated
//! first by the caller; outgoing plans never become incoming history or grant
//! checkout, content, family, reservation or destination authority.
use super::{
    cancellation,
    managed_admission::ManagedAdmissionRecord,
    records::{
        Record,
        import::{
            AdmittedSourceBinding, ImportSource, ImportSourceReferences, ProviderImportKind,
            ProviderTransitionReceiptRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionRole, TransitionRowData, TransitionState,
        },
    },
    recovery::error::{Result as CauseResult, invalid},
};
use crate::projection::{Result, format};
use layrs_vfs::Cancellation;
use rusqlite::Connection;

mod allocation;
mod frontier;
mod publication;
mod receipts;
mod staging;
mod write;
pub(super) use allocation::{apply_copying, require_allocation_capacity};
pub(super) use publication::{
    PublicationCapacity, PublicationState, apply_published, apply_ready, read_publication_state,
};
pub(super) use receipts::preflight as preflight_receipts;
pub(super) use staging::{apply_staging, proposed as proposed_staging};
pub(super) use write::{apply_planned, cancel_planned};

const MAX_TRANSITIONS: u64 = 4096;
const MAX_TRANSITION_BYTES: u64 = 16 * 1024 * 1024;

/// An exact disjoint receipt partition, constructed only after authenticating
/// incoming ancestry. Receipt DATA is retained for proof revalidation as well
/// as inventory accounting; outgoing receipts never become ancestry roots.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OrdinaryOutgoing {
    intents: Vec<ProviderTransitionIntentRecord>,
    receipts: Vec<ProviderTransitionReceiptRecord>,
}
impl OrdinaryOutgoing {
    pub(super) fn intents(&self) -> &[ProviderTransitionIntentRecord] {
        &self.intents
    }
    pub(super) fn receipt_count(&self) -> u64 {
        self.receipts.len() as u64
    }
    pub(super) fn receipt(&self, id: &str) -> CauseResult<&ProviderTransitionReceiptRecord> {
        self.receipts
            .iter()
            .find(|receipt| receipt.digest().is_ok_and(|digest| digest == id))
            .ok_or_else(|| invalid("ordinary publication exact outgoing receipt absent"))
    }
}

/// Called only after genuine incoming links and ancestry authentication in
/// this retained snapshot. This reader deliberately cannot call that admission
/// reader, the planner or full source validator, which would recurse here.
pub(super) fn read_outgoing(
    c: &Connection,
    marker: &format::Marker,
    admission: &ManagedAdmissionRecord,
    incoming: &super::import_validation::VerifiedDestinationPrefix<'_>,
    cancel: Option<&Cancellation>,
) -> CauseResult<OrdinaryOutgoing> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "ordinary outgoing reader requires a retained snapshot",
        ));
    }
    let binding = actual_binding(marker, admission)?;
    let outgoing = read_rows(c, Some(&admission.transaction_id), cancel)?;
    let receipts = receipts::read_partition(c, incoming, &outgoing, cancel)?;
    if outgoing.is_empty() {
        return Ok(OrdinaryOutgoing {
            intents: outgoing,
            receipts,
        });
    }
    let current = frontier::Current::read(c, admission, cancel)?;
    let actual_schema: u32 = c.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let mut live = false;
    for record in &outgoing {
        cancellation::check(cancel)?;
        require_shape(record)?;
        if !matches!(&record.plan.source_prefix.source,
            ImportSource::Ordinary { private_schema, .. } if *private_schema == actual_schema)
        {
            return Err(invalid(
                "ordinary outgoing source schema differs from admitted layout",
            ));
        }
        record
            .plan
            .source_prefix
            .require_references(ImportSourceReferences::AdmittedOrdinary {
                marker,
                admission: &binding,
            })?;
        frontier::require_version(c, marker, admission, record, &current, cancel)?;
        if matches!(
            record.state,
            TransitionState::Copying
                | TransitionState::Staging
                | TransitionState::Ready
                | TransitionState::Published
        ) {
            allocation::require_destination(c, marker, admission, record)?;
        }
        if is_live(record.state) {
            if live {
                return Err(invalid(
                    "ordinary source has multiple live outgoing intents",
                ));
            }
            live = true;
        }
    }
    cancellation::check(cancel)?;
    Ok(OrdinaryOutgoing {
        intents: outgoing,
        receipts,
    })
}

/// An idle-route fence only. Genuine FORMAT/schema/admission validation remains
/// separate. Legacy and provider databases keep their existing behavior.
pub(in crate::projection) fn has_live(c: &Connection) -> Result<bool> {
    let version: u32 = c.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !matches!(version, 9 | 10) {
        return Ok(false);
    }
    let rows = read_rows(c, None, None)?;
    let mut incoming = 0u64;
    let mut live = 0u64;
    for record in &rows {
        if record.role == TransitionRole::Destination
            && record.state == TransitionState::Ready
            && record.plan.kind == ProviderImportKind::ManagedOrdinary
        {
            incoming += 1;
        } else {
            require_shape(record)?;
            live += u64::from(is_live(record.state));
        }
    }
    if incoming != 1 || live > 1 {
        return Err(crate::projection::invalid(
            "ordinary transition ingress or live count differs",
        ));
    }
    Ok(live == 1)
}

pub(in crate::projection) fn require_no_live(c: &Connection) -> Result<()> {
    if has_live(c)? {
        return Err(crate::projection::invalid(
            "ordinary source has a live managed export intent",
        ));
    }
    Ok(())
}

fn actual_binding(
    marker: &format::Marker,
    admission: &ManagedAdmissionRecord,
) -> CauseResult<AdmittedSourceBinding> {
    marker.validate()?;
    admission.validate_fields()?;
    let shared = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("ordinary outgoing source shared FORMAT absent"))?;
    if marker.import_digest.as_deref() != Some(admission.prefix_id.as_str())
        || admission.binding.database_identity != shared.database_identity
    {
        return Err(invalid("ordinary outgoing admission and FORMAT differ"));
    }
    Ok(AdmittedSourceBinding {
        admission_digest: admission.digest()?,
        incoming_prefix_id: admission.prefix_id.clone(),
        incoming_receipt_id: admission.receipt_id.clone(),
        database_identity: shared.database_identity.clone(),
    })
}

fn is_live(state: TransitionState) -> bool {
    matches!(
        state,
        TransitionState::Planned
            | TransitionState::Copying
            | TransitionState::Staging
            | TransitionState::Ready
    )
}

fn require_shape(record: &ProviderTransitionIntentRecord) -> CauseResult<()> {
    record.validate_fields()?;
    let prefix = &record.plan.source_prefix;
    if record.role != TransitionRole::Source
        || !matches!(
            record.state,
            TransitionState::Planned
                | TransitionState::Copying
                | TransitionState::Staging
                | TransitionState::Ready
                | TransitionState::Published
                | TransitionState::Cancelled
        )
        || record.destination.is_some()
            != matches!(
                record.state,
                TransitionState::Copying
                    | TransitionState::Staging
                    | TransitionState::Ready
                    | TransitionState::Published
            )
        || record.reservation.is_some()
            != matches!(
                record.state,
                TransitionState::Staging | TransitionState::Ready | TransitionState::Published
            )
        || record.receipt_id.is_some()
            != matches!(
                record.state,
                TransitionState::Ready | TransitionState::Published
            )
        || record.plan.kind != ProviderImportKind::ManagedOrdinary
        || !prefix.supports_restore_history()
        || !matches!(
            &prefix.source,
            ImportSource::Ordinary {
                private_schema: 9 | 10,
                admission: Some(_),
                ..
            }
        )
    {
        return Err(invalid(
            "ordinary outgoing intent phase or source kind unsupported",
        ));
    }
    Ok(())
}

pub(super) fn preflight_rows(
    c: &Connection,
    cancel: Option<&Cancellation>,
) -> CauseResult<(u64, u64)> {
    cancellation::check(cancel)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 4097)",
        [],
        |row| row.get(0),
    )?;
    if count == 0 || count > MAX_TRANSITIONS {
        return Err(invalid(
            "ordinary transition count missing or exceeds bound",
        ));
    }
    let (bytes, oversized): (u64, bool) = c.query_row(
        "SELECT coalesce(sum(8+length(CAST(id AS BLOB))+length(CAST(role AS BLOB))+length(CAST(state AS BLOB))+coalesce(length(CAST(reservation AS BLOB)),0)+coalesce(length(CAST(receipt_id AS BLOB)),0)+length(payload)+length(CAST(digest AS BLOB))),0),coalesce(max(length(CAST(id AS BLOB))=0 OR length(CAST(id AS BLOB))>128 OR length(CAST(role AS BLOB))>16 OR length(CAST(state AS BLOB))>16 OR coalesce(length(CAST(reservation AS BLOB)),0)>128 OR coalesce(length(CAST(receipt_id AS BLOB)),71)!=71 OR length(payload)=0 OR length(payload)>262144 OR length(CAST(digest AS BLOB))!=71),0) FROM projection_provider_transitions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if bytes > MAX_TRANSITION_BYTES || oversized {
        return Err(invalid("ordinary transition metadata exceeds SQL bounds"));
    }
    cancellation::check(cancel)?;
    Ok((count, bytes))
}

fn read_rows(
    c: &Connection,
    skip: Option<&str>,
    cancel: Option<&Cancellation>,
) -> CauseResult<Vec<ProviderTransitionIntentRecord>> {
    let (count, _) = preflight_rows(c, cancel)?;
    if let Some(skip) = skip {
        let incoming: u64 = c.query_row(
            "SELECT count(*) FROM projection_provider_transitions WHERE id=?1",
            [skip],
            |row| row.get(0),
        )?;
        if incoming != 1 {
            return Err(invalid("ordinary transition exact incoming row absent"));
        }
    }
    let mut statement = c.prepare(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions WHERE ?1 IS NULL OR id<>?1 ORDER BY id COLLATE BINARY",
    )?;
    let mut rows = statement.query([skip])?;
    let mut output = Vec::with_capacity(count as usize);
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let (id, role, state, generation, reservation, receipt, payload, digest): (
            String,
            String,
            String,
            u64,
            Option<String>,
            Option<String>,
            Vec<u8>,
            String,
        ) = (
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        );
        let record = ProviderTransitionIntentRecord::decode(&payload)?;
        if record.plan.denial_coverage.is_some() {
            return Err(invalid(
                "ordinary outgoing denial coverage reader is not integrated",
            ));
        }
        record.require_row(TransitionRowData {
            id: &id,
            role: &role,
            state: &state,
            local_generation: generation,
            reservation: reservation.as_deref(),
            receipt_id: receipt.as_deref(),
            payload: &payload,
            digest: &digest,
        })?;
        output.push(record);
        if output.len() as u64 > count {
            return Err(invalid("ordinary transition rowset grew after preflight"));
        }
    }
    if output.len() as u64 + u64::from(skip.is_some()) != count {
        return Err(invalid(
            "ordinary transition rowset changed after preflight",
        ));
    }
    cancellation::check(cancel)?;
    Ok(output)
}
