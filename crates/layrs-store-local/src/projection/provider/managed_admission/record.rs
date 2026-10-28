//! Bounded immutable admission DATA. The stored actual SourcePublished body is
//! separate SQL BLOB evidence, never synthesized from destination Ready here.
use super::super::records::codec;
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) struct ArchiveCommitment {
    pub count: u64,
    pub bytes: u64,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) struct Boundaries {
    pub imported_generation: u64,
    pub imported_capture_count: u64,
    pub observation_generation: u64,
    pub ready_generation: u64,
    pub ready_capture_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) struct ManagedAdmissionRecord {
    pub schema_version: String,
    pub format_digest: String,
    pub transaction_id: String,
    pub prefix_id: String,
    pub ready_intent_digest: String,
    pub receipt_id: String,
    pub source_published_digest: String,
    pub source_database_identity: String,
    pub source_format_digest: String,
    pub root_id: String,
    pub binding: ExactBinding,
    pub archive: ArchiveCommitment,
    pub boundaries: Boundaries,
    pub metadata_layout_version: u32,
}

fn token(value: &str) -> bool {
    value.strip_prefix("blake3:").is_some_and(|s| hex(s, 64))
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Record for ManagedAdmissionRecord {
    const MAX_BYTES: usize = 65536;
    fn validate_fields(&self) -> crate::projection::Result<()> {
        let bad = || crate::projection::invalid("managed admission record fields differ");
        if self.schema_version != "layrs.managed-admission.v1"
            || self.metadata_layout_version != 1
            || !hex(&self.transaction_id, 32)
            || !hex(&self.root_id, 64)
            || [
                self.format_digest.as_str(),
                &self.prefix_id,
                &self.ready_intent_digest,
                &self.receipt_id,
                &self.source_published_digest,
                &self.source_format_digest,
                &self.archive.digest,
            ]
            .iter()
            .any(|s| !token(s))
            || self.source_database_identity.is_empty()
            || self.source_database_identity.len() > 512
            || self.source_database_identity.contains('\0')
            || !(2..=200002).contains(&self.archive.count)
            || self.archive.bytes > super::schema::MAX_ARCHIVE_COLUMN_BYTES
        {
            return Err(bad());
        }
        self.binding.validate().map_err(|_| bad())?;
        let b = &self.boundaries;
        if b.imported_generation.checked_add(1) != Some(b.observation_generation)
            || b.observation_generation.checked_add(1) != Some(b.ready_generation)
            || b.imported_capture_count.checked_add(1) != Some(b.ready_capture_sequence)
            || b.ready_generation > i64::MAX as u64
            || b.ready_capture_sequence > i64::MAX as u64
        {
            return Err(bad());
        }
        Ok(())
    }
}

impl ManagedAdmissionRecord {
    pub(in crate::projection::provider) fn digest(&self) -> Result<String> {
        Ok(codec::hash("layrs-managed-admission-v1", &self.encode()?))
    }

    pub(super) fn require_links(
        &self,
        layout: schema::AdmittedLayout,
        marker: &format::Marker,
        prefix: &ProviderImportPrefixRecord,
        ready: &ProviderTransitionIntentRecord,
        receipt: &ProviderTransitionReceiptRecord,
        published: &ProviderTransitionIntentRecord,
    ) -> Result<()> {
        self.validate_fields()?;
        ready.require_prefix(prefix)?;
        ready.require_receipt(receipt)?;
        published.require_receipt(receipt)?;
        let (format_digest, source_database_identity, version) = match (&prefix.source, layout) {
            (
                ImportSource::Provider {
                    format_digest,
                    binding,
                    version,
                    ..
                },
                schema::AdmittedLayout::Provider9,
            ) => (format_digest, &binding.recovery.database_identity, version),
            (
                ImportSource::Ordinary {
                    format_digest,
                    admission: Some(admission),
                    version,
                    ..
                },
                schema::AdmittedLayout::Ordinary10,
            ) => (format_digest, &admission.database_identity, version),
            _ => return Err(invalid("admitted import source and private layout differ")),
        };
        let proof = ready
            .destination
            .as_ref()
            .ok_or_else(|| invalid("admitted import destination proof absent"))?;
        let shared = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("admitted import shared FORMAT absent"))?;
        let b = &self.binding;
        if ready.role != TransitionRole::Destination
            || ready.state != TransitionState::Ready
            || published.role != TransitionRole::Source
            || published.state != TransitionState::Published
            || published.plan != ready.plan
            || published.destination != ready.destination
            || published.reservation != ready.reservation
            || published.receipt_id != ready.receipt_id
            || self.transaction_id != ready.transaction_id
            || published.transaction_id != self.transaction_id
            || self.format_digest
                != codec::hash("layrs-managed-admission-format-v1", &marker.bytes()?)
            || proof.format != *marker
            || self.prefix_id != prefix.digest()?
            || marker.import_digest.as_deref() != Some(self.prefix_id.as_str())
            || self.ready_intent_digest != ready.digest()?
            || self.receipt_id != receipt.digest()?
            || self.source_published_digest != published.digest()?
            || self.source_format_digest != *format_digest
            || self.source_database_identity != *source_database_identity
            || self.boundaries.imported_generation != version.generation
            || self.boundaries.imported_capture_count != version.capture_count
            || self.boundaries.observation_generation != ready.plan.observation.generation
            || self.boundaries.ready_generation != ready.plan.placement_generation
            || self.boundaries.ready_capture_sequence != ready.plan.capture_sequence
            || b.family_incarnation != shared.incarnation
            || b.family_root_identity != shared.root_identity
            || b.family_marker_digest != proof.family_marker_digest
            || b.plan_digest != ready.plan.digest()?
            || b.prefix_id != self.prefix_id
            || b.destination_proof_digest != proof.digest(&ready.plan)?
            || b.transaction_id != self.transaction_id
            || b.owner != marker.identity.worktree_id
            || b.database_identity != shared.database_identity
            || b.database_identity != proof.private_database_identity
            || b.operation != ready.reservation_operation()?
            || ready.reservation.as_deref() != Some(b.reservation.as_str())
            || b.quota_bytes != ready.plan.quota_bytes
        {
            return Err(invalid("admitted import ACK or Ready commitments differ"));
        }
        Ok(())
    }
}
