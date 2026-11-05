//! Exact V3 Product identity at the native CAS boundary.
use super::{Result, StoreError};
use layrs_core::scm::ObjectId;
use layrs_sync::v3::{ProductEnvelope, ValidateV3};
use serde::{Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProduct {
    envelope: Box<ProductEnvelope>,
    bytes: Vec<u8>,
}
impl CanonicalProduct {
    pub fn new(id: ObjectId, bytes: impl AsRef<[u8]>) -> Result<Self> {
        let bytes = bytes.as_ref();
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(invalid("Product exceeds 16 MiB"));
        }
        let envelope: ProductEnvelope = serde_json::from_slice(bytes).map_err(invalid)?;
        envelope.verify_canonical_wire(bytes).map_err(invalid)?;
        envelope.validate_v3().map_err(invalid)?;
        if let ProductEnvelope::Weave { record, .. } = &envelope {
            validate_receipt_ids(record)?;
        }
        if envelope.object_id() != &id {
            return Err(invalid("Product ID disagrees with envelope"));
        }
        Ok(Self {
            envelope: Box::new(envelope),
            bytes: bytes.to_vec(),
        })
    }
    pub fn from_envelope(envelope: ProductEnvelope) -> Result<Self> {
        Self::new(
            envelope.object_id().clone(),
            envelope.canonical_bytes().map_err(invalid)?,
        )
    }
    pub fn id(&self) -> &ObjectId {
        self.envelope.object_id()
    }
    pub fn envelope(&self) -> &ProductEnvelope {
        &self.envelope
    }
    pub fn canonical_json(&self) -> &[u8] {
        &self.bytes
    }
    pub fn direct_dependencies(&self) -> Vec<super::StoreObjectId> {
        let mut result = self
            .envelope
            .dependencies()
            .0
            .iter()
            .cloned()
            .map(super::StoreObjectId::Step)
            .collect::<Vec<_>>();
        if let ProductEnvelope::Weave { record, .. } = self.envelope.as_ref() {
            result.extend(
                record
                    .conflict_ids
                    .iter()
                    .cloned()
                    .map(super::StoreObjectId::Conflict),
            );
        }
        result.sort();
        result.dedup();
        result
    }
    /// Initial native projection deliberately refuses unimplemented logical or
    /// versioned Product dependency resolution instead of dropping dependencies.
    pub fn validate_native_subset(&self) -> Result<()> {
        if !self.envelope.dependencies().1.is_empty() {
            return Err(invalid("native Product dependencies not yet supported"));
        }
        match self.envelope.as_ref() {
            ProductEnvelope::Weave { record, .. } if record.gate_snapshot_id.is_none() => Ok(()),
            ProductEnvelope::Layer {
                previous_product_object_id,
                ..
            } if previous_product_object_id.is_none() => Ok(()),
            ProductEnvelope::Marker { .. } | ProductEnvelope::Release { .. } => Ok(()),
            _ => Err(invalid(
                "native Product kind or version dependency not yet supported",
            )),
        }
    }
}
impl Serialize for CanonicalProduct {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.envelope.serialize(serializer)
    }
}
fn invalid(error: impl std::fmt::Display) -> StoreError {
    StoreError::Corruption {
        message: format!("invalid Product: {error}"),
    }
}

fn verify_id<T: serde::Serialize>(value: &T, field: &str, domain: &str) -> Result<()> {
    let mut json = serde_json::to_value(value).map_err(invalid)?;
    let id = json
        .as_object_mut()
        .ok_or_else(|| invalid("identity object"))?
        .remove(field)
        .and_then(|id| id.as_str().map(str::to_owned))
        .ok_or_else(|| invalid("identity field"))?;
    let bytes = layrs_core::scm::canonical_json(&json).map_err(invalid)?;
    let mut hash = blake3::Hasher::new();
    hash.update(domain.as_bytes());
    hash.update(&[0]);
    hash.update(&bytes);
    if id != format!("blake3:{}", hash.finalize().to_hex()) {
        return Err(invalid(format!("{field} digest mismatch")));
    }
    Ok(())
}
fn validate_receipt_ids(record: &layrs_sync::v3::WeaveReceiptRecord) -> Result<()> {
    verify_id(record, "weaveId", "layrs-weave-receipt-v1")?;
    verify_id(
        &record.determinism_profile,
        "profileId",
        "layrs-determinism-profile-v1",
    )?;
    for execution in &record.lens_executions {
        verify_id(execution, "executionId", "layrs-lens-execution-v1")?;
        if execution.determinism_profile_id != record.determinism_profile.profile_id {
            return Err(invalid("execution profile differs from receipt"));
        }
        if let Some(id) = execution.conflict_id.as_ref() {
            if record.conflict_ids.binary_search(id).is_err() {
                return Err(invalid("execution Conflict absent from receipt"));
            }
        }
    }
    for base in &record.merge_bases {
        if let layrs_sync::v3::MergeBase::Step { base_id, step_id } = base {
            if base_id.as_str() != step_id.as_str() {
                return Err(invalid("real base ID differs from Step"));
            }
        }
    }
    Ok(())
}
