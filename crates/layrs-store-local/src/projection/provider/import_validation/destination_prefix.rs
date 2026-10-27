//! Destination-local interpretation of the immutable incoming prefix only.
//! The caller pins the genuine destination FORMAT and retains its transaction.
//! No live source, native authority, Ready suffix or content bytes are proved.
use super::super::records::{
    activation::ProviderActivationRecord,
    transition::{
        ProviderTransitionIntentRecord, TransitionRole, TransitionRowData, TransitionState,
    },
    ProviderBootstrapSource, Record,
};
use super::*;
use layrs_core::scm::{FileId, FileManifestId};
mod bounds;

/// A destination transaction's historical DATA, deliberately distinct from a
/// live-source `VerifiedImportClosure`. Getters alone grant no authority.
pub(in crate::projection::provider) struct VerifiedDestinationPrefix<'a> {
    connection: &'a Connection,
    marker: format::Marker,
    prefix: ProviderImportPrefixRecord,
    receipt: ProviderTransitionReceiptRecord,
    ready: ProviderTransitionIntentRecord,
    metadata: SourceMetadata,
    source: ProviderBootstrapSource,
    activation: ProviderActivationRecord,
    schema: DestinationSchema,
}

#[derive(Clone, Copy)]
enum DestinationSchema {
    Ready8,
    Admitted9,
}

impl VerifiedDestinationPrefix<'_> {
    pub(in crate::projection::provider) fn prefix(&self) -> &ProviderImportPrefixRecord {
        &self.prefix
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection::provider) fn source(&self) -> &ProviderBootstrapSource {
        &self.source
    }
    pub(in crate::projection::provider) fn activation(&self) -> &ProviderActivationRecord {
        &self.activation
    }

    pub(in crate::projection::provider) fn require_destination_connection(
        &self,
        c: &Connection,
    ) -> CauseResult<()> {
        if c.is_autocommit() || !std::ptr::eq(c, self.connection) {
            return Err(invalid(
                "destination prefix connection differs or transaction ended",
            ));
        }
        Ok(())
    }

    /// Reinterpret the complete prefix, including its actual SQL entry links.
    /// A borrow does not prevent a caller ending/restarting a SQLite transaction.
    pub(in crate::projection::provider) fn require_prefix_scope(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> CauseResult<()> {
        self.require_destination_connection(c)?;
        let current = match self.schema {
            DestinationSchema::Ready8 => read_destination_prefix(c, &self.marker, cancel)?,
            DestinationSchema::Admitted9 => {
                let permit = super::super::managed_admission::schema::require_schema(
                    c,
                    &self.marker,
                    cancel,
                )?;
                read_admitted_destination_prefix(c, &self.marker, &permit, cancel)?
            }
        };
        if current.prefix != self.prefix
            || current.receipt != self.receipt
            || current.ready != self.ready
        {
            return Err(invalid("destination prefix entry links changed"));
        }
        Ok(())
    }

    /// Exact incoming mappings only; local Ready additions never join this map.
    pub(in crate::projection::provider) fn copied_content_mappings(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> CauseResult<BTreeMap<FileId, (FileManifestId, u64)>> {
        self.require_prefix_scope(c, cancel)?;
        scoped::with_scope(
            c,
            ImportDatabaseKind::Provider,
            &self.prefix,
            &scoped::ScopeView::from_metadata(&self.metadata),
            cancel,
            |specs, closure| {
                if closure != &self.prefix.closure {
                    return Err(invalid("destination prefix mapping scope differs"));
                }
                let spec = specs
                    .iter()
                    .find(|spec| spec.kind == rows::Kind::ContentMappings)
                    .ok_or_else(|| invalid("destination prefix mapping scope missing"))?;
                let mut stmt = c.prepare(spec.rows_sql)?;
                let mut rows = stmt.query(spec.params)?;
                let mut mappings = BTreeMap::new();
                while let Some(row) = rows.next()? {
                    super::super::cancellation::check(cancel)?;
                    let (file, manifest, bytes): (String, String, u64) =
                        (row.get(0)?, row.get(1)?, row.get(2)?);
                    if file.len() != 71
                        || manifest.len() != 71
                        || bytes > crate::projection::capture::MAX_CAPTURE_FILE_BYTES
                    {
                        return Err(invalid("destination prefix mapping exceeds bound"));
                    }
                    let file = FileId::parse(&file).map_err(|e| invalid(e.to_string()))?;
                    let manifest =
                        FileManifestId::parse(&manifest).map_err(|e| invalid(e.to_string()))?;
                    if mappings.insert(file, (manifest, bytes)).is_some() {
                        return Err(invalid("destination prefix mapping duplicated"));
                    }
                }
                Ok(mappings)
            },
        )
    }

    /// Normalized logical columns of exactly the engaged incoming rowsets.
    /// This is neither SQLite allocation geometry nor a capacity capability.
    pub(in crate::projection::provider) fn metadata_bytes(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> CauseResult<u64> {
        self.require_prefix_scope(c, cancel)?;
        scoped::with_scope(
            c,
            ImportDatabaseKind::Provider,
            &self.prefix,
            &scoped::ScopeView::from_metadata(&self.metadata),
            cancel,
            |specs, closure| {
                if closure != &self.prefix.closure {
                    return Err(invalid("destination prefix metadata scope differs"));
                }
                Ok(rows::preflight(c, specs, cancel)?.metadata_bytes())
            },
        )
    }

    /// Historical identity DATA only. Current destination captures are outside
    /// this immutable source prefix and need the admitted destination boundary.
    pub(in crate::projection::provider) fn capture_origin(
        &self,
        sequence: u64,
        maximum_generation: u64,
    ) -> CauseResult<(&crate::projection::ProjectionIdentity, u64, u64)> {
        self.metadata
            .lineage
            .capture_source(sequence, maximum_generation)
    }
}

/// Read links from the actual destination, never from a caller-selected prefix.
/// Schema and genuine FORMAT equality precede every application-table query.
pub(in crate::projection::provider) fn read_destination_prefix<'a>(
    c: &'a Connection,
    marker: &format::Marker,
    cancel: Option<&Cancellation>,
) -> CauseResult<VerifiedDestinationPrefix<'a>> {
    if c.is_autocommit() {
        return Err(invalid(
            "destination prefix requires a retained SQL snapshot",
        ));
    }
    require_schema(c, marker, ImportDatabaseKind::Provider, cancel)?;
    // Scope derivation itself reaches selection/capture/publication and copied
    // provider semantic loaders. Fence their SQL columns before any allocation.
    bounds::preflight(c, cancel)?;
    read_prefix_rows(c, marker, DestinationSchema::Ready8, cancel)
}

/// Actual schema9 only. The permit is issued by the distinct exact fingerprint
/// reader, never by changing SQLite's version or inventing a source marker.
pub(in crate::projection::provider) fn read_admitted_destination_prefix<'a>(
    c: &'a Connection,
    marker: &format::Marker,
    permit: &super::super::managed_admission::schema::Schema9Permit<'_>,
    cancel: Option<&Cancellation>,
) -> CauseResult<VerifiedDestinationPrefix<'a>> {
    permit.require(c, marker, cancel)?;
    read_prefix_rows(c, marker, DestinationSchema::Admitted9, cancel)
}

fn read_prefix_rows<'a>(
    c: &'a Connection,
    marker: &format::Marker,
    schema: DestinationSchema,
    cancel: Option<&Cancellation>,
) -> CauseResult<VerifiedDestinationPrefix<'a>> {
    let (count, bytes, digest_bytes): (u64,u64,u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(payload)),0),coalesce(max(length(CAST(digest AS BLOB))),0) FROM projection_imports", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count != 1 || bytes > ProviderImportPrefixRecord::MAX_BYTES as u64 || digest_bytes != 71 {
        return Err(invalid("destination incoming prefix missing or oversized"));
    }
    let (digest, payload): (String, Vec<u8>) = c.query_row(
        "SELECT digest,payload FROM projection_imports WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let prefix = ProviderImportPrefixRecord::decode(&payload)?;
    prefix.require_row(&digest, &payload)?;
    if marker.import_digest.as_deref() != Some(digest.as_str())
        || marker.identity != prefix.destination_identity
        || prefix.kind != super::super::records::import::ProviderImportKind::ManagedOrdinary
    {
        return Err(invalid("destination FORMAT incoming prefix differs"));
    }
    let (count, backend_bytes, mount_bytes): (u64,u64,u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(backend AS BLOB))),0),coalesce(max(length(CAST(mount_id AS BLOB))),0) FROM projection_backend", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count != 1 || backend_bytes > 16 || mount_bytes != 0 || !c.query_row("SELECT EXISTS(SELECT 1 FROM projection_backend WHERE singleton=1 AND backend='ordinary' AND mount_id IS NULL)", [], |r| r.get::<_,bool>(0))? {
        return Err(invalid("destination prefix backend differs"));
    }
    let ready = read_ready_link(c)?;
    ready.require_prefix(&prefix)?;
    if ready.role != TransitionRole::Destination
        || ready.state != TransitionState::Ready
        || ready
            .destination
            .as_ref()
            .is_none_or(|proof| &proof.format != marker)
    {
        return Err(invalid("destination prefix requires its exact Ready entry"));
    }
    let receipt = read_receipt_link(
        c,
        ready
            .receipt_id
            .as_deref()
            .ok_or_else(|| invalid("destination prefix receipt missing"))?,
    )?;
    ready.require_receipt(&receipt)?;
    if matches!(schema, DestinationSchema::Admitted9) {
        let ImportSource::Provider { version, .. } = &prefix.source else {
            return Err(invalid("admitted prefix requires provider source"));
        };
        if version.generation.checked_add(1) != Some(ready.plan.observation.generation)
            || ready.plan.observation.generation.checked_add(1)
                != Some(ready.plan.placement_generation)
            || version.capture_count.checked_add(1) != Some(ready.plan.capture_sequence)
        {
            return Err(invalid("admitted prefix Ready boundaries differ"));
        }
        bounds::preflight_admitted(
            c,
            version.generation,
            ready.plan.placement_generation,
            version.capture_count,
            ready.plan.capture_sequence,
            cancel,
        )?;
    }
    let anchor = source::HistoryAnchor::destination(marker, &prefix, &receipt)?;
    let lineage = ancestry::read_ancestry_rows(c, ImportDatabaseKind::Provider, &prefix, cancel)?;
    let metadata = source::read_historical_metadata(
        c,
        &anchor,
        ImportDatabaseKind::Provider,
        &prefix,
        lineage,
        None,
        cancel,
    )?;
    ancestry::verify_historical_closures(c, &prefix, &metadata, cancel)?;
    scoped::read_scoped_closure(c, ImportDatabaseKind::Provider, &prefix, &metadata, cancel)?;
    let ImportSource::Provider {
        activation_map_id, ..
    } = &prefix.source
    else {
        return Err(invalid("destination prefix requires a provider source"));
    };
    // The scoped interpreter just validated every scalar and binding of this
    // exact activation. Decode only its separately bounded canonical body.
    let activation_payload: Vec<u8> = c.query_row("SELECT payload FROM projection_provider_activations WHERE id=?1 AND length(payload)<=1048576", [activation_map_id], |r| r.get(0))?;
    let activation = ProviderActivationRecord::decode(&activation_payload)?;
    if activation.digest()? != *activation_map_id {
        return Err(invalid("destination prefix activation differs"));
    }
    let source = scoped::catalogs::reconstruct_catalog(c, &activation.catalog_id, cancel)?;
    if source.commitment() != &activation.source {
        return Err(invalid("destination prefix reconstructed source differs"));
    }
    require_bounded_foreign_keys(c, cancel)?;
    super::super::cancellation::check(cancel)?;
    Ok(VerifiedDestinationPrefix {
        connection: c,
        marker: marker.clone(),
        prefix,
        receipt,
        ready,
        metadata,
        source,
        activation,
        schema,
    })
}

fn read_ready_link(c: &Connection) -> CauseResult<ProviderTransitionIntentRecord> {
    let (count, oversized): (u64,bool) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(id AS BLOB))>128 OR length(CAST(role AS BLOB))>16 OR length(CAST(state AS BLOB))>16 OR length(CAST(digest AS BLOB))!=71 OR length(payload)>262144 OR length(CAST(reservation AS BLOB))>128 OR length(CAST(receipt_id AS BLOB))>71),0) FROM projection_provider_transitions", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if count != 1 || oversized {
        return Err(invalid(
            "destination prefix Ready entry missing or oversized",
        ));
    }
    let (id,role,state,generation,reservation,receipt,payload,digest): (String,String,String,u64,Option<String>,Option<String>,Vec<u8>,String) = c.query_row(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?;
    let ready = ProviderTransitionIntentRecord::decode(&payload)?;
    ready.require_row(TransitionRowData {
        id: &id,
        role: &role,
        state: &state,
        local_generation: generation,
        reservation: reservation.as_deref(),
        receipt_id: receipt.as_deref(),
        payload: &payload,
        digest: &digest,
    })?;
    Ok(ready)
}

fn read_receipt_link(c: &Connection, id: &str) -> CauseResult<ProviderTransitionReceiptRecord> {
    let (count, bytes, owner_bytes, tx_bytes, companion_bytes): (u64,u64,u64,u64,u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(payload)),0),coalesce(max(length(CAST(owner_identity_digest AS BLOB))),0),coalesce(max(length(CAST(transaction_id AS BLOB))),0),coalesce(max(length(CAST(companion_id AS BLOB))),0) FROM projection_provider_transition_receipts WHERE id=?1", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    if count != 1 || bytes > 65536 || owner_bytes != 71 || tx_bytes > 128 || companion_bytes > 71 {
        return Err(invalid(
            "destination prefix incoming receipt missing or oversized",
        ));
    }
    let (owner,tx,companion,payload): (String,String,Option<String>,Vec<u8>) = c.query_row("SELECT owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts WHERE id=?1", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let receipt = ProviderTransitionReceiptRecord::decode(&payload)?;
    receipt.require_row(TransitionReceiptRowData {
        id,
        owner_identity_digest: &owner,
        transaction_id: &tx,
        companion_id: companion.as_deref(),
        payload: &payload,
    })?;
    Ok(receipt)
}

/// FK interpretation scans the whole schema, including unclaimed suffix rows.
/// Bound that work without adopting those rows into the historical closure.
fn require_bounded_foreign_keys(c: &Connection, cancel: Option<&Cancellation>) -> CauseResult<()> {
    let mut statement = c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let mut tables = statement.query([])?;
    let mut total = 0u64;
    while let Some(row) = tables.next()? {
        super::super::cancellation::check(cancel)?;
        let table: String = row.get(0)?;
        // Exact schema inventory was fenced before this helper. Quote anyway.
        let quoted = table.replace('"', "\"\"");
        let count: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM \"{quoted}\" LIMIT 1000001)"),
            [],
            |r| r.get(0),
        )?;
        total = total
            .checked_add(count)
            .ok_or_else(|| invalid("destination prefix FK row bound overflow"))?;
        if count > 1_000_000 || total > 16_000_000 {
            return Err(invalid("destination prefix FK rowset exceeds bound"));
        }
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("destination prefix foreign key violation"));
    }
    Ok(())
}
