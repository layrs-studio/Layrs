//! Private-database-only admitted provenance. No source/family opener and no
//! call to database::validate, format::access or a shared catalog policy.
use super::super::{
    destination_ready::{FrozenReadyRows, read_frozen_ready_rows},
    import_validation::{VerifiedDestinationPrefix, read_admitted_destination_prefix},
};
use super::*;

mod c14_capture;
mod c14_restore;

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
    private_schema: u32,
    witness_column_bytes: u64,
}
impl VerifiedAdmittedImport<'_> {
    /// Closed terminal C14 DATA, with the actual archived checkout moved out
    /// only after exact private inventory, canonical family union and complete
    /// admission capacity checks. This grants no ordinary mutation authority.
    pub(in crate::projection::provider) fn into_denial_terminal(
        self,
        required: &[layrs_core::scm::FileManifest],
        installation: crate::projection::shared::exact_content::ExactInstallState,
        cancel: Option<&Cancellation>,
    ) -> Result<(
        super::super::admitted_ordinary::AdmittedOrdinaryData,
        super::super::checkout_journal::ExpectedCheckout,
        super::super::checkout_journal::CheckoutJournal,
    )> {
        use super::super::{destination_ready::ManagedReadyClaim, managed_budget};
        use crate::projection::shared::exact_protection::ExactProtection;

        cancellation::check(cancel)?;
        if self.private_schema != 9
            || self.prefix.origin()
                != super::super::import_validation::DestinationOriginKind::Provider
            || !matches!(
                self.prefix.prefix().source,
                ImportSource::Provider {
                    private_schema: 8,
                    ..
                }
            )
            || self.ready().plan.denial_coverage.is_none()
            || self.receipt().denial_coverage.is_none()
            || !self.outgoing.intents().is_empty()
            || self.outgoing.receipt_count() != 0
            || self.witness_column_bytes != 0
        {
            return Err(invalid(
                "terminal denial resolution requires unchanged Provider9 C14",
            ));
        }
        self.require_denial_terminal_inventory(cancel)?;
        let mut witness: Vec<_> = required
            .iter()
            .map(|m| RequiredRepresentation {
                file_id: m.file_id().clone(),
                manifest_id: m.id().clone(),
                bytes: m.byte_length(),
            })
            .collect();
        witness.sort_unstable();
        if witness != self.frozen.required {
            return Err(invalid("terminal denial canonical witness differs"));
        }
        let binding = &self.record.binding;
        let protection = ExactProtection::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            required,
            cancel,
        )?;
        if protection.id() != self.record.root_id {
            return Err(invalid("terminal denial protection differs"));
        }
        let ready = self.ready();
        let claim = ManagedReadyClaim {
            root_id: self.record.root_id.clone(),
            digest: self.record.root_id.clone(),
            kind: "provider-import".into(),
            owner: self.record.transaction_id.clone(),
        };
        // The historical reader authenticates the actual fresh observation,
        // placement, selection, current document and local companion columns.
        // Complete that basis with these canonical manifests, excluding Jmax.
        let floor = managed_budget::complete_metadata_floor(
            self.frozen
                .metadata_basis_floor
                .ok_or_else(|| invalid("terminal denial metadata basis absent"))?,
            ready,
            self.receipt(),
            &claim,
            required,
            cancel,
        )?;
        let budget = managed_budget::admission_budget(
            floor,
            self.frozen.expected.files().len() as u64,
            self.frozen.expected.directories().len() as u64,
        )?;
        let baseline = managed_budget::quota(
            required,
            self.frozen.expected.files().iter().map(|file| file.bytes),
            0,
        )?;
        let metadata = budget.require_quota(ready.plan.quota_bytes, baseline)?;
        let cost = managed_budget::admission_cost(
            self.published.encode()?.len() as u64,
            self.record.encode()?.len() as u64,
            self.frozen.archive.count,
            self.frozen.archive.bytes,
        )?;
        budget.require_actual(metadata, &cost)?;
        let remaining = installation
            .remaining
            .checked_sub(self.frozen.expected.header().file_bytes)
            .ok_or_else(|| invalid("terminal denial checkout envelope insufficient"))?;
        budget.require_remaining(
            remaining,
            protection.payload().len() as u64,
            required.len() as u64,
        )?;
        let data = super::super::admitted_ordinary::inspect(self.connection, &self.marker, cancel)?;
        cancellation::check(cancel)?;
        Ok((data, self.frozen.expected, self.frozen.journal))
    }

    fn require_denial_terminal_inventory(&self, cancel: Option<&Cancellation>) -> Result<()> {
        let c = self.connection;
        if c.is_autocommit() {
            return Err(invalid("terminal denial transaction ended"));
        }
        let add = |n: u64, extra: u64| {
            n.checked_add(extra)
                .filter(|n| *n <= 1_100_010)
                .ok_or_else(|| invalid("terminal denial row bound overflow"))
        };
        let counts = self.denial_terminal_counts()?;
        for (kind, name, _) in super::super::database_io::schema_inventory(c)? {
            cancellation::check(cancel)?;
            if kind != "table" {
                continue;
            }
            let expected = counts
                .iter()
                .find(|(table, _)| *table == name)
                .map_or(0, |(_, n)| *n);
            let count: u64 = c.query_row(
                &format!("SELECT count(*) FROM (SELECT 1 FROM \"{name}\" LIMIT ?1)"),
                [add(expected, 1)?],
                |r| r.get(0),
            )?;
            if count != expected {
                return Err(invalid("terminal denial table has missing or extra rows"));
            }
        }
        let (singleton, head, captures): (u64, u64, u64) = c.query_row(
            "SELECT singleton,generation,(SELECT coalesce(max(sequence),0) FROM projection_captures) FROM projection_head",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if (singleton, head, captures) != (1, self.record.boundaries.ready_generation, self.record.boundaries.ready_capture_sequence)
            || !c.query_row("SELECT backend='ordinary' AND mount_id IS NULL FROM projection_backend WHERE singleton=1", [], |r| r.get::<_, bool>(0))?
            || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1
            || c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)", [], |r| r.get::<_, bool>(0))?
        {
            return Err(invalid("terminal denial frontier/backend/foreign keys differ"));
        }
        cancellation::check(cancel)
    }

    fn denial_terminal_counts(&self) -> Result<Vec<(&'static str, u64)>> {
        let closure = &self.prefix.prefix().closure;
        let plan = &self.ready().plan;
        let add = |n: u64, extra: u64| {
            n.checked_add(extra)
                .filter(|n| *n <= 1_100_010)
                .ok_or_else(|| invalid("terminal denial row bound overflow"))
        };
        // Exact reference schema is already authenticated. Every table outside
        // this frozen inventory must be empty, including operational journals,
        // later ordinary effects and outgoing transitions/receipts.
        Ok(vec![
            (
                "projection_observations",
                add(
                    closure.denial_observations.as_ref().map_or(0, |r| r.count),
                    1,
                )?,
            ),
            ("projection_snapshots", add(closure.snapshots.count, 2)?),
            ("projection_selections", add(closure.selections.count, 2)?),
            ("projection_captures", add(closure.captures.count, 1)?),
            ("projection_publications", closure.publications.count),
            (
                "projection_restore_history",
                closure.restore_history.as_ref().map_or(0, |r| r.count),
            ),
            ("projection_provider_catalogs", closure.catalogs.count),
            ("projection_provider_descriptors", closure.descriptors.count),
            (
                "projection_provider_activations",
                closure.activation_maps.count,
            ),
            (
                "projection_provider_transition_receipts",
                add(closure.transition_receipts.count, 1)?,
            ),
            (
                "projection_provider_companions",
                add(
                    closure.companions.count,
                    u64::from(plan.companion_id.is_some()),
                )?,
            ),
            (
                "projection_provider_companion_files",
                add(closure.companion_files.count, plan.companion_files.count)?,
            ),
            (
                "projection_provider_companion_deletions",
                add(
                    closure.companion_deletions.count,
                    plan.companion_deletions.count,
                )?,
            ),
            (
                "projection_content",
                add(
                    closure.content_mappings.count,
                    self.frozen.new_content.len() as u64,
                )?,
            ),
            ("projection_meta", 1),
            ("projection_head", 1),
            ("projection_backend", 1),
            ("projection_imports", 1),
            ("projection_provider_transitions", 1),
            ("projection_claims", 1),
            ("projection_managed_admission", 1),
            ("projection_import_checkout", self.frozen.archive.count),
        ])
    }

    /// Detach the real authenticated archive, never a reconstruction from its
    /// summary. The generic admitted reader permits mutable ordinary tails;
    /// this closed C10 entry additionally requires the unchanged terminal
    /// inventory. Canonical manifests and installation DATA must come from
    /// the independently validated complete family snapshot in this scope.
    pub(in crate::projection::provider) fn into_ordinary_terminal(
        self,
        required: &[layrs_core::scm::FileManifest],
        installation: crate::projection::shared::exact_content::ExactInstallState,
        cancel: Option<&Cancellation>,
    ) -> Result<(
        super::super::admitted_ordinary::AdmittedOrdinaryData,
        super::super::checkout_journal::ExpectedCheckout,
        super::super::checkout_journal::CheckoutJournal,
    )> {
        use super::super::{destination_ready::ManagedReadyClaim, managed_budget};
        use crate::projection::shared::exact_protection::ExactProtection;

        cancellation::check(cancel)?;
        if self.private_schema != 10
            || self.prefix.origin()
                != super::super::import_validation::DestinationOriginKind::Ordinary
            || !self.outgoing.intents().is_empty()
            || self.outgoing.receipt_count() != 0
        {
            return Err(invalid("terminal resolution requires unchanged Ordinary10"));
        }
        self.require_ordinary_terminal_inventory(cancel)?;
        let mut witness: Vec<_> = required
            .iter()
            .map(|m| RequiredRepresentation {
                file_id: m.file_id().clone(),
                manifest_id: m.id().clone(),
                bytes: m.byte_length(),
            })
            .collect();
        witness.sort_unstable();
        if witness != self.frozen.required {
            return Err(invalid("terminal Ordinary10 canonical witness differs"));
        }
        let binding = &self.record.binding;
        let protection = ExactProtection::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            required,
            cancel,
        )?;
        if protection.id() != self.record.root_id {
            return Err(invalid("terminal Ordinary10 protection differs"));
        }
        // Recompute the original admission capacity with actual immutable
        // prefix metadata. A cheap read never shrinks its fixed peak budget.
        let basis = self.prefix.ordinary_frozen()?;
        let ready = self.ready();
        let claim = ManagedReadyClaim {
            root_id: self.record.root_id.clone(),
            digest: self.record.root_id.clone(),
            kind: "provider-import".into(),
            owner: self.record.transaction_id.clone(),
        };
        let (source_size, destination_size) = managed_budget::original_staging_sizes(ready)?;
        let base = managed_budget::base_metadata_floor(
            self.prefix.metadata_bytes(self.connection, cancel)?,
            basis.snapshot(),
            basis.snapshot(),
            &self.frozen.capture,
            &ready.plan,
            basis.selection(),
            source_size,
            destination_size,
        )?;
        let floor = managed_budget::complete_metadata_floor(
            base,
            ready,
            self.receipt(),
            &claim,
            required,
            cancel,
        )?;
        let budget = managed_budget::ordinary_admission_budget(
            floor,
            self.frozen.expected.files().len() as u64,
            self.frozen.expected.directories().len() as u64,
            required.len() as u64,
        )?;
        let baseline = managed_budget::quota(
            required,
            self.frozen.expected.files().iter().map(|f| f.bytes),
            0,
        )?;
        let metadata = budget.require_quota(ready.plan.quota_bytes, baseline)?;
        let cost = managed_budget::ordinary_admission_cost(
            self.published.encode()?.len() as u64,
            self.record.encode()?.len() as u64,
            self.frozen.archive.count,
            self.frozen.archive.bytes,
            self.frozen.required.len() as u64,
            self.witness_column_bytes,
        )?;
        budget.require_actual(metadata, &cost)?;
        let remaining = installation
            .remaining
            .checked_sub(self.frozen.expected.header().file_bytes)
            .ok_or_else(|| invalid("terminal Ordinary10 checkout envelope insufficient"))?;
        budget.require_remaining(
            remaining,
            protection.payload().len() as u64,
            required.len() as u64,
        )?;
        // This detached summary is itself obtained from the real interpreter
        // in the same retained transaction. Only the authenticated expected
        // inventory and journal below are moved; neither is synthesized.
        let data = super::super::admitted_ordinary::inspect(self.connection, &self.marker, cancel)?;
        cancellation::check(cancel)?;
        Ok((data, self.frozen.expected, self.frozen.journal))
    }

    fn require_ordinary_terminal_inventory(&self, cancel: Option<&Cancellation>) -> Result<()> {
        let c = self.connection;
        if c.is_autocommit() {
            return Err(invalid("terminal Ordinary10 transaction ended"));
        }
        let closure = &self.prefix.prefix().closure;
        let add = |n: u64, extra: u64| {
            n.checked_add(extra)
                .filter(|n| *n <= 1_100_010)
                .ok_or_else(|| invalid("terminal Ordinary10 row bound overflow"))
        };
        // Exact reference schema was already authenticated. Account for all
        // tables, including operational tables, defaulting every other table
        // to zero. Admission moves the complete prejournal into its archive.
        let counts = [
            ("projection_snapshots", add(closure.snapshots.count, 2)?),
            ("projection_selections", add(closure.selections.count, 2)?),
            ("projection_captures", add(closure.captures.count, 1)?),
            ("projection_publications", closure.publications.count),
            (
                "projection_restore_history",
                closure.restore_history.as_ref().map_or(0, |r| r.count),
            ),
            ("projection_provider_catalogs", closure.catalogs.count),
            ("projection_provider_descriptors", closure.descriptors.count),
            (
                "projection_provider_activations",
                closure.activation_maps.count,
            ),
            (
                "projection_provider_transition_receipts",
                add(closure.transition_receipts.count, 1)?,
            ),
            ("projection_provider_companions", closure.companions.count),
            (
                "projection_provider_companion_files",
                closure.companion_files.count,
            ),
            (
                "projection_provider_companion_deletions",
                closure.companion_deletions.count,
            ),
            (
                "projection_content",
                add(
                    closure.content_mappings.count,
                    self.frozen.new_content.len() as u64,
                )?,
            ),
            ("projection_meta", 1),
            ("projection_head", 1),
            ("projection_backend", 1),
            ("projection_imports", 1),
            ("projection_provider_transitions", 1),
            ("projection_claims", 1),
            ("projection_managed_admission", 1),
            ("projection_import_checkout", self.frozen.archive.count),
            (
                "projection_import_representations",
                self.frozen.required.len() as u64,
            ),
        ];
        for (kind, name, _) in super::super::database_io::schema_inventory(c)? {
            cancellation::check(cancel)?;
            if kind != "table" {
                continue;
            }
            let expected = counts
                .iter()
                .find(|(table, _)| *table == name)
                .map_or(0, |(_, n)| *n);
            let count: u64 = c.query_row(
                &format!("SELECT count(*) FROM (SELECT 1 FROM \"{name}\" LIMIT ?1)"),
                [add(expected, 1)?],
                |r| r.get(0),
            )?;
            if count != expected {
                return Err(invalid(
                    "terminal Ordinary10 table has missing or extra rows",
                ));
            }
        }
        let (singleton, head, captures): (u64, u64, u64) = c.query_row(
            "SELECT singleton,generation,(SELECT coalesce(max(sequence),0) FROM projection_captures) FROM projection_head",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if (singleton, head, captures) != (1, self.record.boundaries.ready_generation,
            self.record.boundaries.ready_capture_sequence)
            || !c.query_row(
                "SELECT backend='ordinary' AND mount_id IS NULL FROM projection_backend WHERE singleton=1",
                [], |r| r.get::<_, bool>(0),
            )?
            || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1
            || c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)", [], |r| r.get::<_, bool>(0))?
        {
            return Err(invalid("terminal Ordinary10 frontier/backend/foreign keys differ"));
        }
        // Prefix scope authenticates every imported row; frozen Ready checks
        // authenticate both local snapshots/selections, the capture, all new
        // mappings, incoming intent/receipt, claim, witness and exact archive.
        // The counts above exclude every coherent later suffix as well.
        cancellation::check(cancel)
    }

    pub(in crate::projection::provider) fn private_schema(&self) -> u32 {
        self.private_schema
    }
    pub(in crate::projection::provider) fn witness_column_bytes(&self) -> u64 {
        self.witness_column_bytes
    }
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
            || self.private_schema != fresh.private_schema
            || self.witness_column_bytes != fresh.witness_column_bytes
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
        permit.layout(),
        marker,
        prefix.prefix(),
        prefix.ready(),
        prefix.receipt(),
        &published,
    )?;
    let outgoing =
        super::super::ordinary_transition::read_outgoing(c, marker, &record, &prefix, cancel)?;
    let frozen = read_frozen_ready_rows(c, marker, &prefix, &record, &permit, &outgoing, cancel)?;
    let witness_column_bytes = match permit.layout() {
        schema::AdmittedLayout::Provider9 => 0,
        schema::AdmittedLayout::Ordinary10 => {
            representations::column_bytes(frozen.required.len() as u64)?
        }
    };
    cancellation::check(cancel)?;
    let verified = VerifiedAdmittedImport {
        connection: c,
        marker: marker.clone(),
        record,
        published,
        prefix,
        frozen,
        outgoing,
        admission_column_bytes,
        private_schema: permit.private_schema(),
        witness_column_bytes,
    };
    if verified.ready().plan.denial_coverage.is_some() {
        if permit.layout() != schema::AdmittedLayout::Provider9
            || !verified.outgoing.intents().is_empty()
            || verified.outgoing.receipt_count() != 0
        {
            return Err(invalid(
                "admitted denial requires Provider9 without outgoing transitions",
            ));
        }
        verified.require_local_capture_suffix(cancel)?;
    }
    Ok(verified)
}

/// Bounded canonical admission metadata only. This neither follows incoming
/// prefix/frozen rows nor validates outgoing transitions, so it cannot recurse
/// through destination-prefix revalidation or issue an admitted-import proof.
/// The caller must first require the genuine admitted-schema/FORMAT permit.
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
    if published.plan.denial_coverage.is_some()
        && !matches!(
            published.plan.source_prefix.source,
            ImportSource::Provider {
                private_schema: 8,
                ..
            }
        )
    {
        return Err(invalid(
            "admitted denial requires Provider8 source provenance",
        ));
    }
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
