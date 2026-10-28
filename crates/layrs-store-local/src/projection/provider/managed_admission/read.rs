//! Private-database-only admitted provenance. No source/family opener and no
//! call to database::validate, format::access or a shared catalog policy.
use super::super::{
    destination_ready::{read_frozen_ready_rows, FrozenReadyRows},
    import_validation::{read_admitted_destination_prefix, VerifiedDestinationPrefix},
};
use super::*;

/// Frozen ingress evidence plus exact representation references. This proves
/// neither a current family root nor canonical current manifests/CAS bytes.
pub(in crate::projection) struct VerifiedAdmittedImport<'a> {
    connection: &'a Connection,
    marker: format::Marker,
    record: ManagedAdmissionRecord,
    published: ProviderTransitionIntentRecord,
    prefix: VerifiedDestinationPrefix<'a>,
    frozen: FrozenReadyRows,
    outgoing: super::super::ordinary_transition::OrdinaryOutgoing,
    admission_column_bytes: u64,
}
impl VerifiedAdmittedImport<'_> {
    pub(in crate::projection::provider) fn record(&self) -> &ManagedAdmissionRecord {
        &self.record
    }
    pub(in crate::projection) fn outgoing(&self) -> &[ProviderTransitionIntentRecord] {
        self.outgoing.intents()
    }
    pub(in crate::projection::provider) fn outgoing_receipt(
        &self,
        id: &str,
    ) -> Result<&ProviderTransitionReceiptRecord> {
        Ok(self.outgoing.receipt(id)?)
    }
    pub(in crate::projection::provider) fn source_published(
        &self,
    ) -> &ProviderTransitionIntentRecord {
        &self.published
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        self.prefix.ready()
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        self.prefix.receipt()
    }
    pub(in crate::projection::provider) fn prefix(&self) -> &ProviderImportPrefixRecord {
        self.prefix.prefix()
    }
    pub(in crate::projection::provider) fn export_segments(
        &self,
    ) -> Result<Vec<super::super::records::import::ImportedSegment>> {
        self.prefix.export_segments()
    }
    pub(in crate::projection::provider) fn required(&self) -> &[RequiredRepresentation] {
        &self.frozen.required
    }
    pub(in crate::projection::provider) fn expected(
        &self,
    ) -> &super::super::checkout_journal::ExpectedCheckout {
        &self.frozen.expected
    }
    pub(in crate::projection::provider) fn archive(
        &self,
    ) -> &super::super::checkout_journal::CheckoutJournal {
        &self.frozen.journal
    }
    pub(in crate::projection::provider) fn admission_column_bytes(&self) -> u64 {
        self.admission_column_bytes
    }
    pub(in crate::projection::provider) fn archive_column_bytes(&self) -> u64 {
        self.frozen.archive.bytes
    }

    /// Every validating reuse reinterprets the frozen domain. Current ordinary
    /// tails may advance; they are deliberately not part of this commitment.
    pub(in crate::projection) fn require(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if c.is_autocommit() || !std::ptr::eq(c, self.connection) {
            return Err(invalid("admitted provenance transaction differs or ended"));
        }
        let fresh = read_admitted_import(c, &self.marker, cancel)?;
        if self.record != fresh.record
            || self.published != fresh.published
            || self.ready() != fresh.ready()
            || self.receipt() != fresh.receipt()
            || self.frozen.required != fresh.frozen.required
            || self.frozen.archive != fresh.frozen.archive
            || self.outgoing != fresh.outgoing
            || self.admission_column_bytes != fresh.admission_column_bytes
        {
            return Err(invalid("admitted immutable provenance changed"));
        }
        Ok(())
    }

    /// Origin DATA for a selected persisted capture, independent of the old
    /// source. A later ordinary validator still checks its actual row/body.
    pub(in crate::projection::provider) fn capture_origin<'s>(
        &'s self,
        c: &Connection,
        sequence: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<(&'s crate::projection::ProjectionIdentity, u64, u64)> {
        self.require(c, cancel)?;
        let boundaries = &self.record.boundaries;
        if sequence <= boundaries.imported_capture_count {
            return self
                .prefix
                .capture_origin(sequence, boundaries.imported_generation);
        }
        let (head,captures):(u64,u64)=c.query_row("SELECT (SELECT generation FROM projection_head WHERE singleton=1),coalesce((SELECT max(sequence) FROM projection_captures),0)",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if sequence < boundaries.ready_capture_sequence
            || sequence > captures
            || head < boundaries.ready_generation
        {
            return Err(invalid("admitted local capture escaped current boundary"));
        }
        Ok((&self.marker.identity, boundaries.ready_generation, head))
    }

    pub(in crate::projection::provider) fn restore_origin(
        &self,
        c: &Connection,
        operation_id: &str,
        sequence: u64,
        payload: &[u8],
        cancel: Option<&Cancellation>,
    ) -> Result<super::super::import_validation::RestoreHistoryOrigin> {
        self.require(c, cancel)?;
        if sequence == 0 || sequence > self.record.boundaries.imported_capture_count {
            return Err(invalid("restore history escaped admitted import boundary"));
        }
        self.prefix
            .restore_origin(c, operation_id, sequence, payload, cancel)
    }
}

pub(in crate::projection::provider) fn read_admitted_import<'a>(
    c: &'a Connection,
    marker: &format::Marker,
    cancel: Option<&Cancellation>,
) -> Result<VerifiedAdmittedImport<'a>> {
    let permit = schema::require_schema(c, marker, cancel)?;
    let (record, published, admission_column_bytes) = read_record(c, cancel)?;
    let prefix = read_admitted_destination_prefix(c, marker, &permit, &record, cancel)?;
    record.require_links(
        marker,
        prefix.prefix(),
        prefix.ready(),
        prefix.receipt(),
        &published,
    )?;
    let outgoing =
        super::super::ordinary_transition::read_outgoing(c, marker, &record, &prefix, cancel)?;
    let frozen = read_frozen_ready_rows(c, marker, &prefix, &record, &permit, &outgoing, cancel)?;
    cancellation::check(cancel)?;
    Ok(VerifiedAdmittedImport {
        connection: c,
        marker: marker.clone(),
        record,
        published,
        prefix,
        frozen,
        outgoing,
        admission_column_bytes,
    })
}

/// Bounded canonical admission metadata only. This neither follows incoming
/// prefix/frozen rows nor validates outgoing transitions, so it cannot recurse
/// through destination-prefix revalidation or issue an admitted-import proof.
/// The caller must first require the genuine schema9/FORMAT permit.
pub(in crate::projection::provider) fn read_record(
    c: &Connection,
    cancel: Option<&Cancellation>,
) -> Result<(ManagedAdmissionRecord, ProviderTransitionIntentRecord, u64)> {
    cancellation::check(cancel)?;
    let (count,oversized):(u64,bool)=c.query_row("SELECT count(*),coalesce(max(length(CAST(digest AS BLOB))!=71 OR length(CAST(source_published_digest AS BLOB))!=71 OR length(source_published)=0 OR length(source_published)>262144 OR length(CAST(receipt_id AS BLOB))!=71 OR length(CAST(archive_digest AS BLOB))!=71 OR length(payload)=0 OR length(payload)>65536),0) FROM projection_managed_admission",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count != 1 || oversized {
        return Err(invalid(
            "admitted record missing or exceeds SQL byte bounds",
        ));
    }
    let (singleton,digest,source_digest,source_body,receipt,generation,capture,count,archive_digest,payload):(u64,String,String,Vec<u8>,String,u64,u64,u64,String,Vec<u8>)=c.query_row("SELECT singleton,digest,source_published_digest,source_published,receipt_id,ready_generation,ready_capture_sequence,archive_count,archive_digest,payload FROM projection_managed_admission",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?)))?;
    let record = ManagedAdmissionRecord::decode(&payload)?;
    if singleton != 1
        || digest != record.digest()?
        || source_digest != record.source_published_digest
        || receipt != record.receipt_id
        || generation != record.boundaries.ready_generation
        || capture != record.boundaries.ready_capture_sequence
        || count != record.archive.count
        || archive_digest != record.archive.digest
    {
        return Err(invalid("admitted record SQL projection differs"));
    }
    let published = ProviderTransitionIntentRecord::decode(&source_body)?;
    if published.role != TransitionRole::Source
        || published.state != TransitionState::Published
        || published.digest()? != source_digest
    {
        return Err(invalid(
            "admitted stored body is not its exact SourcePublished",
        ));
    }
    let bytes = 32u64
        .checked_add(digest.len() as u64)
        .and_then(|n| n.checked_add(source_digest.len() as u64))
        .and_then(|n| n.checked_add(source_body.len() as u64))
        .and_then(|n| n.checked_add(receipt.len() as u64))
        .and_then(|n| n.checked_add(archive_digest.len() as u64))
        .and_then(|n| n.checked_add(payload.len() as u64))
        .ok_or_else(|| invalid("admitted normalized columns overflow"))?;
    if bytes > schema::MAX_ADMISSION_COLUMN_BYTES {
        return Err(invalid("admitted normalized columns exceed bound"));
    }
    Ok((record, published, bytes))
}
