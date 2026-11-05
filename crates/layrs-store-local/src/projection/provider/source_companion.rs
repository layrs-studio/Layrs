//! Closed source publication DATA and its independently funded B SQL envelope.
//! Construction grants no authority. Managed retention supplies CAS/image pins
//! and both genuine C Ready scopes before this DATA reaches a source writer.
use super::{
    records::{
        denied_coverage::DerivedDeniedCoverage,
        import::{
            ImportRowset, ImportSource, ProviderImportPrefixRecord, ProviderTransitionReceiptRecord,
        },
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        AllocationRecord, Record,
    },
    recovery::error::{invalid, Result},
    transition_reconcile::companions::OwnedCompanionData,
};
use crate::projection::shared::{
    exact_content::ExactBinding,
    source_companion::{SourceCompanionBinding, SourceCompanionProtection},
};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection};

mod prospective;
pub(in crate::projection) use prospective::ProspectiveCompanionCost;

pub(in crate::projection::provider) struct SourceCompanionPublication<'a> {
    rows: &'a OwnedCompanionData,
    coverage: Option<&'a DerivedDeniedCoverage>,
    protection: SourceCompanionProtection,
    receipt: ProviderTransitionReceiptRecord,
    ready: ProviderTransitionIntentRecord,
    published: ProviderTransitionIntentRecord,
    metadata: u64,
}
impl<'a> SourceCompanionPublication<'a> {
    /// Must run before the first reservation/content/publication effect. Every
    /// count, encoded size and overflow is fixed here from the immutable plan.
    pub(in crate::projection::provider) fn derive(
        a: &AllocationRecord,
        staging: &ProviderTransitionIntentRecord,
        c: &ExactBinding,
        rows: &'a OwnedCompanionData,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        Self::derive_in_mode(a, staging, c, rows, None, cancel)
    }

    /// Closed denial-aware DATA derivation from the retained normalized body.
    /// The enclosing owner must separately authenticate this same body in the
    /// actual source SQL. This constructor supplies no read or write authority.
    pub(in crate::projection::provider) fn derive_with_coverage(
        a: &AllocationRecord,
        staging: &ProviderTransitionIntentRecord,
        c: &ExactBinding,
        rows: &'a OwnedCompanionData,
        coverage: &'a DerivedDeniedCoverage,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        Self::derive_in_mode(a, staging, c, rows, Some(coverage), cancel)
    }

    fn derive_in_mode(
        a: &AllocationRecord,
        staging: &ProviderTransitionIntentRecord,
        c: &ExactBinding,
        rows: &'a OwnedCompanionData,
        coverage: Option<&'a DerivedDeniedCoverage>,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        // Authenticate the mode and the complete retained body before deriving
        // a receipt, terminal intent or its independently funded SQL envelope.
        let receipt = planned_receipt_in_mode(staging, coverage, cancel)?;
        let header = rows
            .header()
            .ok_or_else(|| invalid("source companion closed header absent"))?;
        if staging.plan.companion_id.as_deref() != Some(header.digest()?.as_str())
            || staging.plan.companion_files.count != header.file_count
            || staging.plan.companion_deletions.count != header.deletion_count
            || staging.plan.companion_files.digest != header.files_digest
            || staging.plan.companion_deletions.digest != header.deletions_digest
        {
            return Err(invalid(
                "source companion closed rows differ from immutable plan",
            ));
        }
        header.require_prefix(&staging.plan.source_prefix)?;
        header.require_receipt(&receipt)?;
        let mut ready = ProviderTransitionIntentRecord::decode(&staging.encode()?)?;
        ready.state = TransitionState::Ready;
        ready.receipt_id = Some(receipt.digest()?);
        ready.require_transition(staging)?;
        ready.require_receipt(&receipt)?;
        let mut published = ProviderTransitionIntentRecord::decode(&ready.encode()?)?;
        published.state = TransitionState::Published;
        published.require_transition(&ready)?;
        published.require_receipt(&receipt)?;
        // The document itself is already charged by d. Only its references in
        // the actual v2 receipt/terminal intent encodings contribute to s here.
        let metadata = publication_bytes(rows.source_row_bytes()?, &ready, &published, &receipt)?;
        let binding = SourceCompanionBinding::new(a, staging, c)?;
        let protection =
            SourceCompanionProtection::new(binding, rows.manifests(), metadata, cancel)?;
        protection.require_plan(a, staging, cancel)?;
        Ok(Self {
            rows,
            coverage,
            protection,
            receipt,
            ready,
            published,
            metadata,
        })
    }
    pub(in crate::projection::provider) fn protection(&self) -> &SourceCompanionProtection {
        &self.protection
    }
    pub(in crate::projection::provider) fn rows(&self) -> &OwnedCompanionData {
        self.rows
    }
    pub(in crate::projection::provider) fn coverage(&self) -> Option<&DerivedDeniedCoverage> {
        self.coverage
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(in crate::projection::provider) fn require(
        &self,
        a: &AllocationRecord,
        staging: &ProviderTransitionIntentRecord,
        receipt: &ProviderTransitionReceiptRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        let actual = Self::derive_in_mode(
            a,
            staging,
            &self.protection.binding.destination,
            self.rows,
            self.coverage,
            cancel,
        )?;
        if actual.protection != self.protection
            || actual.metadata != self.metadata
            || actual.receipt != self.receipt
            || actual.receipt != *receipt
            || actual.ready != self.ready
            || actual.published != self.published
        {
            return Err(invalid(
                "source companion closed rows, envelope or receipt changed",
            ));
        }
        self.protection.require_envelope(actual.metadata)
    }
    /// Only absent rows may be created. A partial/foreign rowset or claim in a
    /// Staging predecessor is rejected by the complete before interpreter.
    pub(in crate::projection::provider) fn insert(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if c.is_autocommit() || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1
        {
            return Err(invalid(
                "source companion publication requires FK-enabled transaction",
            ));
        }
        self.protection.require_envelope(self.metadata)?;
        self.rows.insert_source_mappings_and_rows(c, cancel)?;
        let id = self.protection.id();
        if c.execute("INSERT INTO projection_claims(root_id,digest,kind,owner) VALUES(?1,?1,'provider-companion',?2)",params![id,self.protection.binding.destination.transaction_id])?!=1 {
            return Err(invalid("source companion claim insertion differs"));
        }
        self.require_rows(c, cancel)
    }
    pub(in crate::projection::provider) fn require_rows(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.rows.require_new_rows(c, cancel)?;
        let n:u64=c.query_row("SELECT count(*) FROM projection_claims WHERE root_id=?1 AND digest=?1 AND kind='provider-companion' AND owner=?2",params![self.protection.id(),self.protection.binding.destination.transaction_id],|r|r.get(0))?;
        if n != 1 {
            return Err(invalid("source companion complete private claim absent"));
        }
        self.protection.require_envelope(publication_bytes(
            self.rows.source_row_bytes()?,
            &self.ready,
            &self.published,
            &self.receipt,
        )?)
    }
}

/// Deterministic real planned receipt, not a fake sizing record. No C Ready is
/// inferred from it: every value comes from the already immutable plan/proof.
pub(in crate::projection::provider) fn planned_receipt(
    staging: &ProviderTransitionIntentRecord,
) -> Result<ProviderTransitionReceiptRecord> {
    planned_receipt_in_mode(staging, None, None)
}

/// Derive the exact v2 receipt while retaining the sealed body as input. The
/// caller still proves actual source/C SQL and physical ownership before use.
pub(in crate::projection::provider) fn planned_receipt_with_coverage(
    staging: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    cancel: Option<&Cancellation>,
) -> Result<ProviderTransitionReceiptRecord> {
    planned_receipt_in_mode(staging, Some(coverage), cancel)
}

fn planned_receipt_in_mode(
    staging: &ProviderTransitionIntentRecord,
    coverage: Option<&DerivedDeniedCoverage>,
    cancel: Option<&Cancellation>,
) -> Result<ProviderTransitionReceiptRecord> {
    super::cancellation::check(cancel)?;
    staging.validate_fields()?;
    if staging.role != TransitionRole::Source || staging.state != TransitionState::Staging {
        return Err(invalid(
            "source companion derivation requires original source Staging",
        ));
    }
    let denial_coverage = match (coverage, staging.plan.denial_coverage.as_ref()) {
        (None, None) => None,
        (Some(body), Some(reference)) => {
            body.record().require_intent(staging, cancel)?;
            let actual = body.record().reference()?;
            if actual != *reference {
                return Err(invalid(
                    "source companion denial coverage reference differs",
                ));
            }
            Some(actual)
        }
        _ => {
            return Err(invalid(
                "source companion denial coverage requires matching closed mode",
            ));
        }
    };
    let p = &staging.plan;
    let prefix = &p.source_prefix;
    let ImportSource::Provider {
        identity, version, ..
    } = &prefix.source
    else {
        return Err(invalid("source receipt requires provider prefix"));
    };
    let destination = staging
        .destination
        .as_ref()
        .ok_or_else(|| invalid("source receipt destination absent"))?;
    let row = |r: &super::records::transition::TransitionRowset| ImportRowset {
        count: r.count,
        digest: r.digest.clone(),
    };
    let receipt = ProviderTransitionReceiptRecord {
        schema_version: if denial_coverage.is_some() {
            "layrs.projection-provider-transition-receipt.v2"
        } else {
            "layrs.projection-provider-transition-receipt.v1"
        }
        .into(),
        denial_coverage,
        owner_identity_digest: destination.owner_digest(p)?,
        transaction_id: staging.transaction_id.clone(),
        transition_plan_digest: p.digest()?,
        destination_proof_digest: destination.digest(p)?,
        source_prefix_id: p.source_prefix_id.clone(),
        source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode()?)?,
        parent_receipt_id: prefix.ancestry_root_receipt.clone(),
        source_identity: identity.clone(),
        destination_identity: prefix.destination_identity.clone(),
        imported_max_generation: version.generation,
        destination_observation_generation: p.observation.generation,
        destination_observation_digest: p.observation.snapshot_digest.clone(),
        destination_placement_generation: p.placement_generation,
        destination_placement_digest: p.placement_digest.clone(),
        imported_capture_count: version.capture_count,
        destination_capture_sequence: p.capture_sequence,
        destination_capture_digest: p.capture_digest.clone(),
        protected_manifests: row(&p.protected_manifests),
        content_mappings: prefix.closure.content_mappings.clone(),
        companion_id: p.companion_id.clone(),
        companion_files: row(&p.companion_files),
        companion_deletions: row(&p.companion_deletions),
    };
    receipt.validate_fields()?;
    receipt.require_prefix(prefix)?;
    super::cancellation::check(cancel)?;
    Ok(receipt)
}
fn add(sum: u64, bytes: u64) -> Result<u64> {
    sum.checked_add(bytes)
        .filter(|n| *n <= 1024 * 1024 * 1024)
        .ok_or_else(|| invalid("source SQL envelope overflow or exceeds 1GiB"))
}
/// Exact normalized values, conservatively charging reused mappings and BOTH
/// terminal intent projections. INTEGER=8, NULL=0, UTF8/BLOB=actual length.
pub(in crate::projection::provider) fn publication_bytes(
    base: u64,
    ready: &ProviderTransitionIntentRecord,
    published: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) -> Result<u64> {
    let mut total = add(
        base,
        64 + 64 + "provider-companion".len() as u64 + receipt.transaction_id.len() as u64,
    )?;
    for n in [
        71,
        receipt.owner_identity_digest.len() as u64,
        receipt.transaction_id.len() as u64,
        receipt.companion_id.as_ref().map_or(0, |s| s.len() as u64),
        receipt.encode()?.len() as u64,
    ] {
        total = add(total, n)?;
    }
    for (intent, state) in [(ready, "ready"), (published, "published")] {
        intent.require_receipt(receipt)?;
        for n in [
            intent.transaction_id.len() as u64,
            6,
            state.len() as u64,
            8,
            intent.reservation.as_ref().map_or(0, |s| s.len() as u64),
            71,
            intent.encode()?.len() as u64,
            71,
        ] {
            total = add(total, n)?;
        }
    }
    Ok(total)
}
