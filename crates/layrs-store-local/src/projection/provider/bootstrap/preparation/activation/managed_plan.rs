//! Deterministic transition DATA. The caller retains source SQL, checkpoint,
//! native and shared-content proofs through the eventual destination writer.
use super::managed_image::VerifiedManagedImage;
use crate::projection::{
    ProjectionIdentity,
    capture::{
        CapturedChange, CapturedFile, CapturedPatch, CapturedTarget, MAX_CAPTURE_BYTES,
        MAX_CAPTURE_FILE_BYTES,
    },
    database, format,
    provider::{
        import_validation::VerifiedImportClosure,
        records::{
            ProviderBootstrapSource, Record,
            activation::activation_snapshot,
            denied_coverage::DerivedDeniedCoverage,
            import::{ImportSource, ProviderImportKind, ProviderImportPrefixRecord},
            transition::{TransitionObservation, TransitionPlan, TransitionRowset},
        },
        recovery::error::{Result, invalid},
        validation::{PrivateActiveCaptureValidation, PrivateActivePublicationValidation},
    },
};
use layrs_core::{
    projection::{EntryState, ProjectionSelection, ProjectionSnapshot},
    scm::{FileId, FileManifest, FileManifestId, RepoPath, TreeEntryTarget, canonical_json},
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const OVERLAY_DOMAIN: &[u8] = b"layrs-provider-managed-overlay-v1\0";
const MANIFEST_DOMAIN: &[u8] = b"layrs-provider-managed-protected-manifests-v1\0";
const MAX_MANIFESTS: usize = 100_000;
const MAX_CHANGES: usize = 1024;
const MAX_METADATA_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::projection::provider) struct RequiredManifest {
    pub(in crate::projection::provider) file_id: FileId,
    pub(in crate::projection::provider) manifest_id: FileManifestId,
    pub(in crate::projection::provider) bytes: u64,
}
impl RequiredManifest {
    fn from_manifest(manifest: &FileManifest) -> Self {
        Self {
            file_id: manifest.file_id().clone(),
            manifest_id: manifest.id().clone(),
            bytes: manifest.byte_length(),
        }
    }
}

pub(in crate::projection::provider) struct ImageFile<'a> {
    pub(in crate::projection::provider) path: &'a RepoPath,
    pub(in crate::projection::provider) manifest: &'a FileManifest,
    pub(in crate::projection::provider) executable: bool,
    pub(in crate::projection::provider) expected: Option<(FileId, bool)>,
}
pub(in crate::projection::provider) struct ImageDeletion<'a> {
    pub(in crate::projection::provider) path: &'a RepoPath,
    pub(in crate::projection::provider) base: &'a FileId,
    pub(in crate::projection::provider) executable: bool,
}

/// Factual upper budget from this exact image and manifest set. It is not a
/// reservation, capacity proof, or permission to mark a destination Ready.
pub(in crate::projection::provider) struct ManagedPlanData {
    pub(in crate::projection::provider) observation: ProjectionSnapshot,
    pub(in crate::projection::provider) observation_digest: String,
    pub(in crate::projection::provider) placement: ProjectionSnapshot,
    pub(in crate::projection::provider) placement_digest: String,
    pub(in crate::projection::provider) capture: CapturedPatch,
    pub(in crate::projection::provider) plan: TransitionPlan,
    pub(in crate::projection::provider) required_manifests: Vec<RequiredManifest>,
    pub(in crate::projection::provider) quota_bytes: u64,
    pub(in crate::projection::provider) transaction_id: String,
    // Populated by candidate derivation or admission against retained source SQL.
    // This DATA floor alone does not grant allocation or publication authority.
    pub(in crate::projection::provider) validated_metadata_floor: Option<u64>,
}

#[derive(Clone, Copy)]
enum ManagedProviderProof<'a> {
    Capture(&'a PrivateActiveCaptureValidation),
    Publication(&'a PrivateActivePublicationValidation),
}
impl ManagedProviderProof<'_> {
    fn require(self, closure: &VerifiedImportClosure) -> Result<()> {
        match self {
            Self::Capture(proof) => closure.require_provider_inputs(proof),
            Self::Publication(proof) => closure.require_publication_inputs(proof),
        }
    }
    fn parts(
        &self,
    ) -> (
        &ProviderBootstrapSource,
        &crate::projection::VersionedProjection,
        &crate::projection::provider::validation::PrivateCaptureProof,
    ) {
        let (_, _, source, activated, captures) = match self {
            Self::Capture(proof) => proof.import_parts(),
            Self::Publication(proof) => proof.import_parts(),
        };
        (source, activated, captures)
    }
}

/// Adapter from retained witnesses. The caller supplies complete canonical
/// manifests matching the exact union, including historical capture mappings.
/// The caller independently proves historical bytes/protection and measures
/// metadata before effects. This function never opens SQL or shared content.
pub(super) fn derive(
    prefix: &ProviderImportPrefixRecord,
    closure: &VerifiedImportClosure,
    image: &VerifiedManagedImage<'_, '_>,
    provider_proof: &PrivateActiveCaptureValidation,
    fresh: &ProjectionSnapshot,
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    supplied: &[FileManifest],
    metadata_bytes: u64,
) -> Result<ManagedPlanData> {
    derive_parts(
        prefix,
        closure,
        image,
        ManagedProviderProof::Capture(provider_proof),
        fresh,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        supplied,
        metadata_bytes,
    )
}

/// Publication-aware counterpart retaining the entire proof and its distinct
/// SQL binding. Like `derive`, this requires the CURRENT retained image/head.
pub(super) fn derive_publication(
    prefix: &ProviderImportPrefixRecord,
    closure: &VerifiedImportClosure,
    image: &VerifiedManagedImage<'_, '_>,
    proof: &PrivateActivePublicationValidation,
    fresh: &ProjectionSnapshot,
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    supplied: &[FileManifest],
    metadata_bytes: u64,
) -> Result<ManagedPlanData> {
    derive_parts(
        prefix,
        closure,
        image,
        ManagedProviderProof::Publication(proof),
        fresh,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        supplied,
        metadata_bytes,
    )
}

fn derive_parts(
    prefix: &ProviderImportPrefixRecord,
    closure: &VerifiedImportClosure,
    image: &VerifiedManagedImage<'_, '_>,
    provider_proof: ManagedProviderProof<'_>,
    fresh: &ProjectionSnapshot,
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    supplied: &[FileManifest],
    metadata_bytes: u64,
) -> Result<ManagedPlanData> {
    let prefix_id = prefix.digest()?;
    if closure.prefix_id() != prefix_id || prefix.kind != ProviderImportKind::ManagedOrdinary {
        return Err(invalid("managed plan source prefix proof differs"));
    }
    let (source_identity, version) = match &prefix.source {
        ImportSource::Provider {
            identity, version, ..
        } => (identity, version),
        _ => return Err(invalid("managed plan requires an Active provider source")),
    };
    provider_proof.require(closure)?;
    let (source, activated_version, captures) = provider_proof.parts();
    let activated = &activated_version.snapshot;
    image.require_source(source, activated)?;
    if image.digest() != version.effective_image_digest
        || captures.patches.len() as u64 != version.capture_count
        || captures.patches.last().map(|p| p.digest.as_str()) != version.capture_head.as_deref()
        || prefix.closure.companions.count != 0
        || prefix.closure.companion_files.count != 0
        || prefix.closure.companion_deletions.count != 0
    {
        return Err(invalid(
            "managed plan image, capture, content or companion prefix differs",
        ));
    }
    let referenced: BTreeSet<_> = captures
        .patches
        .iter()
        .flat_map(|patch| patch.changes.iter())
        .filter_map(|change| {
            change
                .replacement
                .as_ref()
                .map(|file| (file.file_id.clone(), file.bytes))
        })
        .collect();
    let historical: Vec<_> = captures
        .content
        .iter()
        .map(|row| -> Result<_> {
            Ok(RequiredManifest {
                file_id: FileId::parse(row.file_id.clone()).map_err(|e| invalid(e.to_string()))?,
                manifest_id: FileManifestId::parse(row.manifest_id.clone())
                    .map_err(|e| invalid(e.to_string()))?,
                bytes: row.bytes,
            })
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|row| referenced.contains(&(row.file_id.clone(), row.bytes)))
        .collect();
    let represented: BTreeSet<_> = historical
        .iter()
        .map(|row| (row.file_id.clone(), row.bytes))
        .collect();
    if !referenced.is_subset(&represented)
        || historical.len() as u64 != prefix.closure.content_mappings.count
    {
        return Err(invalid(
            "managed historical capture lacks exact content mapping",
        ));
    }
    let files: Vec<_> = image
        .files()
        .iter()
        .map(|file| ImageFile {
            path: &file.path,
            manifest: file.manifest.manifest(),
            executable: file.executable,
            expected: file.expected.clone(),
        })
        .collect();
    let deletions: Vec<_> = image
        .deletions()
        .iter()
        .map(|deletion| ImageDeletion {
            path: &deletion.path,
            base: &deletion.base,
            executable: deletion.executable,
        })
        .collect();
    derive_data(
        prefix,
        source_identity,
        source,
        activated,
        fresh,
        &files,
        &deletions,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        &historical,
        supplied,
        metadata_bytes,
    )
}

/// Reconstruct the complete durable plan before reserving any family space.
/// Its metadata component is a bounded envelope, never a measurement inferred
/// from a mutable SQLite file length. The source transaction remains retained.
#[allow(clippy::too_many_arguments)]
pub(super) fn rederive_reservation(
    source_c: &rusqlite::Connection,
    closure: &VerifiedImportClosure,
    image: &VerifiedManagedImage<'_, '_>,
    proof: &PrivateActiveCaptureValidation,
    fresh: &ProjectionSnapshot,
    intent: &crate::projection::provider::records::transition::ProviderTransitionIntentRecord,
    manifests: &[FileManifest],
    cancel: &layrs_vfs::Cancellation,
) -> Result<ManagedPlanData> {
    rederive_parts(
        source_c,
        closure,
        image,
        ManagedProviderProof::Capture(proof),
        fresh,
        intent,
        manifests,
        cancel,
    )
}

/// Revalidate a managed plan using the explicit publication-aware SQL closure.
/// No capture-proof object or physical authority is manufactured from it.
pub(super) fn rederive_publication_reservation(
    source_c: &rusqlite::Connection,
    closure: &VerifiedImportClosure,
    image: &VerifiedManagedImage<'_, '_>,
    proof: &PrivateActivePublicationValidation,
    fresh: &ProjectionSnapshot,
    intent: &crate::projection::provider::records::transition::ProviderTransitionIntentRecord,
    manifests: &[FileManifest],
    cancel: &layrs_vfs::Cancellation,
) -> Result<ManagedPlanData> {
    rederive_parts(
        source_c,
        closure,
        image,
        ManagedProviderProof::Publication(proof),
        fresh,
        intent,
        manifests,
        cancel,
    )
}

fn rederive_parts(
    source_c: &rusqlite::Connection,
    closure: &VerifiedImportClosure,
    image: &VerifiedManagedImage<'_, '_>,
    proof: ManagedProviderProof<'_>,
    fresh: &ProjectionSnapshot,
    intent: &crate::projection::provider::records::transition::ProviderTransitionIntentRecord,
    manifests: &[FileManifest],
    cancel: &layrs_vfs::Cancellation,
) -> Result<ManagedPlanData> {
    require_catalog_representable(manifests, cancel)?;
    let plan = &intent.plan;
    let derive_with = |metadata| {
        derive_parts(
            &plan.source_prefix,
            closure,
            image,
            proof,
            fresh,
            plan.source_prefix.destination_identity.clone(),
            &plan.destination_parent,
            &plan.destination_parent_identity,
            &plan.destination_leaf_nonce,
            &intent.transaction_id,
            manifests,
            metadata,
        )
    };
    cancel.check()?;
    // This derives the union from source descriptors, capture history and the
    // retained image. A caller-supplied rehashed subset cannot pass this step.
    let baseline = derive_with(0)?;
    let metadata = plan
        .quota_bytes
        .checked_sub(baseline.quota_bytes)
        .filter(|bytes| *bytes <= MAX_METADATA_BYTES)
        .ok_or_else(|| invalid("managed reserved metadata envelope invalid"))?;
    let destination_intent = super::managed_destination::destination_intent(intent)?;
    let (source, _, _) = proof.parts();
    let minimum = crate::projection::provider::managed_budget::base_metadata_floor(
        closure.metadata_bytes(source_c, &plan.source_prefix, Some(cancel))?,
        &baseline.observation,
        &baseline.placement,
        &baseline.capture,
        plan,
        source.selection(),
        crate::projection::provider::managed_budget::intent_size(intent)?,
        crate::projection::provider::managed_budget::intent_size(&destination_intent)?,
    )?;
    if metadata < minimum {
        return Err(invalid(
            "managed reservation metadata envelope insufficient",
        ));
    }
    let mut derived = derive_with(metadata)?;
    if derived.plan != *plan {
        return Err(invalid(
            "managed durable plan differs from retained source and image",
        ));
    }
    derived.validated_metadata_floor = Some(minimum);
    Ok(derived)
}

/// Both destination local generations persist the actual immutable source
/// selection. Count their normalized SQL columns independently of image size:
/// two BLOB payloads, two 8-byte generations and two raw 64-byte digests.
/// This bounds logical values only, not SQLite page/index/journal geometry.
pub(super) fn selection_rows_metadata(
    selection: &layrs_core::projection::ProjectionSelection,
) -> Result<u64> {
    crate::projection::provider::managed_budget::selection_rows_metadata(selection)
}

pub(super) fn require_catalog_representable(
    manifests: &[FileManifest],
    cancel: &layrs_vfs::Cancellation,
) -> Result<()> {
    if manifests.len() > MAX_MANIFESTS {
        return Err(invalid("managed manifest count exceeds catalog bound"));
    }
    // Current roots encode the complete union inline. Reserve no plan that
    // this representation cannot protect; leave bounded framing room rather
    // than silently dropping representations to fit the catalog.
    let mut protection_bytes = 65536u64;
    for manifest in manifests {
        cancel.check()?;
        if manifest.byte_length() > MAX_CAPTURE_FILE_BYTES
            || manifest.chunks().len() > 1024
            || manifest
                .chunks()
                .iter()
                .any(|chunk| chunk.byte_length() == 0 || chunk.byte_length() > 1048576)
        {
            return Err(invalid(
                "managed exact representation exceeds catalog bounds",
            ));
        }
        let payload = canonical_json(manifest).map_err(|e| invalid(e.to_string()))?;
        if payload.len() > 262144 {
            return Err(invalid(
                "managed exact manifest payload exceeds catalog bound",
            ));
        }
        protection_bytes = protection_bytes
            .checked_add(payload.len() as u64 + 1)
            .filter(|bytes| *bytes <= 16 * 1024 * 1024)
            .ok_or_else(|| invalid("managed protected union exceeds inline root bound"))?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayRow<'a> {
    change: &'a CapturedChange,
    manifest_id: Option<&'a FileManifestId>,
}

fn rowset<T: Serialize>(domain: &[u8], rows: &[T]) -> Result<TransitionRowset> {
    let mut hash = blake3::Hasher::new();
    hash.update(domain);
    hash.update(&(rows.len() as u64).to_be_bytes());
    for row in rows {
        let bytes = canonical_json(row).map_err(|e| invalid(e.to_string()))?;
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(&bytes);
    }
    Ok(TransitionRowset {
        count: rows.len() as u64,
        digest: format!("blake3:{}", hash.finalize()),
    })
}

/// Bind the exact supplied representations to the immutable planned union.
/// This verifies metadata only, not bytes, family availability or write rights.
pub(in crate::projection::provider) fn require_manifest_union(
    plan: &TransitionPlan,
    supplied: &[FileManifest],
) -> Result<()> {
    plan.digest()?;
    if supplied.len() > MAX_MANIFESTS || supplied.len() as u64 != plan.protected_manifests.count {
        return Err(invalid("managed manifest union cardinality differs"));
    }
    let mut required = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut owners = BTreeMap::new();
    for manifest in supplied {
        let row = RequiredManifest::from_manifest(manifest);
        if required.contains_key(&(row.file_id.clone(), row.manifest_id.clone())) {
            return Err(invalid(
                "managed manifest union contains duplicate representation",
            ));
        }
        insert_required(&mut required, &mut lengths, &mut owners, row)?;
    }
    let rows: Vec<_> = required.into_values().collect();
    if rowset(MANIFEST_DOMAIN, &rows)? != plan.protected_manifests {
        return Err(invalid("managed manifest union differs from plan"));
    }
    Ok(())
}

/// Pure kernel for offline fixtures. No object in the output is an authority
/// proof: source history, manifest bytes and destination ownership stay external.
#[allow(clippy::too_many_arguments)]
pub(super) fn derive_data(
    prefix: &ProviderImportPrefixRecord,
    source_identity: &ProjectionIdentity,
    source: &ProviderBootstrapSource,
    activated: &ProjectionSnapshot,
    fresh: &ProjectionSnapshot,
    image_files: &[ImageFile<'_>],
    image_deletions: &[ImageDeletion<'_>],
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    historical: &[RequiredManifest],
    supplied: &[FileManifest],
    metadata_bytes: u64,
) -> Result<ManagedPlanData> {
    let version = match &prefix.source {
        ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => version,
    };
    if fresh != source.initial() || activated != &activation_snapshot(source)? {
        return Err(invalid(
            "managed plan fresh observation or identity differs",
        ));
    }
    if version.selection_digest != source.commitment().selection_digest {
        return Err(invalid(
            "managed plan selection or capture boundary differs",
        ));
    }
    let base_manifests: Vec<_> = source
        .descriptors()
        .iter()
        .map(|descriptor| &descriptor.manifest)
        .collect();
    derive_kernel(
        prefix,
        source_identity,
        fresh,
        activated,
        source.selection(),
        &source.commitment().selection_digest,
        &base_manifests,
        image_files,
        image_deletions,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        historical,
        supplied,
        metadata_bytes,
        None,
        None,
    )
}

/// Ordinary admission DATA only. The owner separately binds the current
/// snapshot, selection, manifests and image to its retained live SQL scope.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn derive_ordinary_data(
    prefix: &ProviderImportPrefixRecord,
    current: &ProjectionSnapshot,
    selection: &ProjectionSelection,
    selection_digest: &str,
    base_manifests: &[&FileManifest],
    image_files: &[ImageFile<'_>],
    image_deletions: &[ImageDeletion<'_>],
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    historical: &[RequiredManifest],
    supplied: &[FileManifest],
    metadata_bytes: u64,
) -> Result<ManagedPlanData> {
    prefix.validate_fields()?;
    let source_identity = match &prefix.source {
        ImportSource::Ordinary {
            identity,
            private_schema: 9 | 10,
            admission: Some(_),
            ..
        } if prefix.supports_restore_history() => identity,
        _ => {
            return Err(invalid(
                "managed ordinary plan requires admitted schema9 source",
            ));
        }
    };
    selection
        .validate_snapshot(current)
        .map_err(|e| invalid(e.to_string()))?;
    if base_manifests.len() > MAX_MANIFESTS || historical.len() > MAX_MANIFESTS {
        return Err(invalid(
            "managed ordinary base or history exceeds manifest bound",
        ));
    }
    let expected: BTreeSet<_> = current
        .entries
        .iter()
        .filter(|entry| selection.includes(&entry.path))
        .filter_map(|entry| match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some(file_id),
            _ => None,
        })
        .collect();
    let mut actual = BTreeSet::new();
    for manifest in base_manifests {
        if !expected.contains(manifest.file_id()) || !actual.insert(manifest.file_id()) {
            return Err(invalid(
                "managed ordinary base manifest is extra or duplicated",
            ));
        }
    }
    if actual != expected {
        return Err(invalid("managed ordinary base manifest coverage differs"));
    }
    let mut history = BTreeMap::new();
    for row in historical {
        if history
            .insert(row.file_id.clone(), (row.manifest_id.clone(), row.bytes))
            .is_some()
        {
            return Err(invalid("managed ordinary historical mapping is duplicated"));
        }
    }
    if crate::projection::provider::import_validation::content_mapping_rowset_data(&history, None)?
        != prefix.closure.content_mappings
    {
        return Err(invalid(
            "managed ordinary historical mapping commitment differs",
        ));
    }
    derive_kernel(
        prefix,
        source_identity,
        current,
        current,
        selection,
        selection_digest,
        base_manifests,
        image_files,
        image_deletions,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        historical,
        supplied,
        metadata_bytes,
        None,
        None,
    )
}

/// Companion-aware immutable plan DATA. Unlike the legacy adapter, its input
/// is the closed reconcile owner, not arbitrary recovery rows. The active
/// driver must retain source/native/authority proofs and rederive this before
/// reservation and every publication boundary; this result grants none.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn derive_reconciled_data(
    prefix: &ProviderImportPrefixRecord,
    owner: &crate::projection::provider::transition_reconcile::companions::OwnedCompanionData,
    observed: &crate::projection::provider::transition_observation::TransitionObservationData,
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
) -> Result<ManagedPlanData> {
    derive_reconciled_parts(
        prefix,
        owner,
        observed,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        None,
    )
}

/// Closed denial-aware counterpart. The complete observation and sealed body
/// are bound before any plan is returned; no positive absence is synthesized.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn derive_reconciled_denial_data(
    prefix: &ProviderImportPrefixRecord,
    owner: &crate::projection::provider::transition_reconcile::companions::OwnedCompanionData,
    observed: &crate::projection::provider::transition_observation::TransitionObservationData,
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    coverage: &DerivedDeniedCoverage,
) -> Result<ManagedPlanData> {
    coverage
        .record()
        .require_observation(prefix, transaction_id, observed, None)?;
    derive_reconciled_parts(
        prefix,
        owner,
        observed,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        Some(coverage),
    )
}

#[allow(clippy::too_many_arguments)]
fn derive_reconciled_parts(
    prefix: &ProviderImportPrefixRecord,
    owner: &crate::projection::provider::transition_reconcile::companions::OwnedCompanionData,
    observed: &crate::projection::provider::transition_observation::TransitionObservationData,
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    coverage: Option<&DerivedDeniedCoverage>,
) -> Result<ManagedPlanData> {
    if coverage.is_some() {
        owner.require_denial_plan_observation(prefix, observed)?;
    } else {
        owner.require_plan_observation(prefix, observed)?;
    }
    if prefix.kind != ProviderImportKind::ManagedOrdinary {
        return Err(invalid(
            "reconciled ordinary plan requires managed conversion",
        ));
    }
    let (identity, version) = match &prefix.source {
        ImportSource::Ordinary {
            identity, version, ..
        }
        | ImportSource::Provider {
            identity, version, ..
        } => (identity, version),
    };
    if version.generation.checked_add(1) != Some(observed.binding().generation) {
        return Err(invalid(
            "reconciled ordinary observation generation is not contiguous",
        ));
    }
    let fresh = observed.snapshot();
    let selection = observed.selection();
    let mut placement = fresh.clone();
    for entry in &mut placement.entries {
        if selection.includes(&entry.path)
            && entry.state != EntryState::Reserved
            && matches!(entry.target, Some(TreeEntryTarget::File { .. }))
        {
            entry.state = EntryState::Materialized;
        }
    }
    let entries: BTreeMap<_, _> = fresh.entries.iter().map(|e| (&e.path, e)).collect();
    let manifests: BTreeMap<_, _> = owner.manifests().iter().map(|m| (m.id(), m)).collect();
    let mut files = Vec::with_capacity(owner.authorized_files().len());
    for file in owner.authorized_files() {
        let entry = entries
            .get(&file.path)
            .ok_or_else(|| invalid("reconciled ordinary authorized path missing"))?;
        let manifest = manifests
            .get(&file.manifest_id)
            .copied()
            .ok_or_else(|| invalid("reconciled ordinary exact representation missing"))?;
        if manifest.file_id() != &file.file_id || manifest.byte_length() != file.bytes {
            return Err(invalid("reconciled ordinary authorized manifest differs"));
        }
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id.clone(), entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => return Err(invalid("reconciled ordinary expected base unsupported")),
        };
        files.push(ImageFile {
            path: &file.path,
            manifest,
            executable: file.executable,
            expected,
        });
    }
    let mut deletions = Vec::new();
    for change in owner.overlay().iter().filter(|c| c.replacement.is_none()) {
        let base = change
            .expected
            .as_ref()
            .ok_or_else(|| invalid("reconciled ordinary refuses None-to-None Capture"))?;
        deletions.push(ImageDeletion {
            path: &change.path,
            base: &base.file_id,
            executable: base.executable,
        });
    }
    let bases = observed
        .descriptors()
        .iter()
        .map(|d| &d.manifest)
        .collect::<Vec<_>>();
    let union = owner
        .representations()
        .iter()
        .map(|r| RequiredManifest {
            file_id: r.file_id.clone(),
            manifest_id: r.manifest_id.clone(),
            bytes: r.bytes,
        })
        .collect::<Vec<_>>();
    let data = derive_kernel(
        prefix,
        identity,
        fresh,
        &placement,
        selection,
        &version.selection_digest,
        &bases,
        &files,
        &deletions,
        destination,
        parent,
        parent_identity,
        leaf_nonce,
        transaction_id,
        &union,
        owner.manifests(),
        owner.metadata_bytes(),
        Some(owner),
        coverage.map(|coverage| (coverage, observed)),
    )?;
    if data.observation_digest != observed.binding().snapshot_digest
        || data.capture.changes != owner.overlay()
        || data.quota_bytes != owner.quota_bytes()
    {
        return Err(invalid(
            "reconciled ordinary plan observation/overlay/budget differs",
        ));
    }
    Ok(data)
}

#[allow(clippy::too_many_arguments)]
fn derive_kernel(
    prefix: &ProviderImportPrefixRecord,
    source_identity: &ProjectionIdentity,
    fresh: &ProjectionSnapshot,
    activated: &ProjectionSnapshot,
    selection: &ProjectionSelection,
    selection_digest: &str,
    base_manifests: &[&FileManifest],
    image_files: &[ImageFile<'_>],
    image_deletions: &[ImageDeletion<'_>],
    destination: ProjectionIdentity,
    parent: &str,
    parent_identity: &str,
    leaf_nonce: &str,
    transaction_id: &str,
    historical: &[RequiredManifest],
    supplied: &[FileManifest],
    metadata_bytes: u64,
    companions: Option<
        &crate::projection::provider::transition_reconcile::companions::OwnedCompanionData,
    >,
    denial: Option<(
        &DerivedDeniedCoverage,
        &crate::projection::provider::transition_observation::TransitionObservationData,
    )>,
) -> Result<ManagedPlanData> {
    if let Some((coverage, observed)) = denial {
        coverage
            .record()
            .require_observation(prefix, transaction_id, observed, None)?;
        if observed.snapshot() != fresh || observed.selection() != selection {
            return Err(invalid(
                "managed denial kernel observation/selection differs",
            ));
        }
    }
    format::validate_identity(source_identity)?;
    format::validate_identity(&destination)?;
    let source_prefix_id = prefix.digest()?;
    let version = match &prefix.source {
        ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => version,
    };
    prefix.validate_fields()?;
    let prefix_source_identity = match &prefix.source {
        ImportSource::Ordinary { identity, .. } | ImportSource::Provider { identity, .. } => {
            identity
        }
    };
    if source_identity != prefix_source_identity
        || destination != prefix.destination_identity
        || destination.worktree_id == source_identity.worktree_id
        || destination.store_id != source_identity.store_id
        || destination.space_id != source_identity.space_id
        || destination.origin != source_identity.origin
        || destination.principal_id != source_identity.principal_id
        || version.effective_image_digest.len() != 64
        || (companions.is_none()
            && (prefix.closure.companions.count != 0
                || prefix.closure.companion_files.count != 0
                || prefix.closure.companion_deletions.count != 0))
        || transaction_id.is_empty()
        || transaction_id.len() > 128
        || transaction_id.contains('\0')
    {
        return Err(invalid(
            "managed plan fresh observation or identity differs",
        ));
    }
    selection
        .validate_snapshot(fresh)
        .map_err(|e| invalid(e.to_string()))?;
    selection
        .validate_snapshot(activated)
        .map_err(|e| invalid(e.to_string()))?;
    if version.selection_digest != selection_digest
        || version.capture_head.is_some() != (version.capture_count > 0)
        || version.capture_count > 4096
    {
        return Err(invalid(
            "managed plan selection or capture boundary differs",
        ));
    }
    let observation_generation = version
        .generation
        .checked_add(1)
        .filter(|n| *n < i64::MAX as u64)
        .ok_or_else(|| invalid("managed observation generation exhausted"))?;
    let placement_generation = observation_generation + 1;
    let capture_sequence = version
        .capture_count
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("managed capture sequence exhausted"))?;
    let observation = fresh.clone();
    let observation_digest = database::digest(
        observation_generation,
        Some(&version.snapshot_digest),
        &observation
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?,
    );
    let placement = activated.clone();
    let placement_digest = database::digest(
        placement_generation,
        Some(&observation_digest),
        &placement
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?,
    );
    let mut files = BTreeMap::new();
    let mut deletions = BTreeMap::new();
    let mut image_bytes = 0u64;
    for file in image_files {
        image_bytes = image_bytes
            .checked_add(file.manifest.byte_length())
            .filter(|n| *n <= MAX_CAPTURE_BYTES)
            .ok_or_else(|| invalid("managed complete image exceeds 4GiB"))?;
        if files.insert(file.path, file).is_some()
            || file.manifest.byte_length() > MAX_CAPTURE_FILE_BYTES
        {
            return Err(invalid("managed image duplicate or oversized file"));
        }
    }
    for deletion in image_deletions {
        if deletions.insert(deletion.path, deletion).is_some() || files.contains_key(deletion.path)
        {
            return Err(invalid("managed image duplicate or conflicting deletion"));
        }
    }
    let mut changes = Vec::new();
    let mut overlay_manifests = BTreeMap::new();
    let mut changed_bytes = 0u64;
    for entry in &placement.entries {
        let file = files.remove(&entry.path);
        let deletion = deletions.remove(&entry.path);
        if !selection.includes(&entry.path) {
            if file.is_some() || deletion.is_some() {
                return Err(invalid("managed image includes excluded path"));
            }
            continue;
        }
        if entry.state == EntryState::Reserved {
            // Reconciliation preserves denied PRESENT bytes as companions,
            // never as a working file or an authorized deletion in C. Only
            // the typed reconciled owner can admit this reserved observation.
            if companions.is_none() || entry.writable || file.is_some() || deletion.is_some() {
                return Err(invalid(
                    "managed reserved path cannot carry checkout effects",
                ));
            }
            continue;
        }
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some(CapturedTarget {
                file_id: file_id.clone(),
                executable: entry.executable,
            }),
            None if entry.state == EntryState::Absent => None,
            _ => return Err(invalid("managed selected base is not file or absence")),
        };
        if let Some(file) = file {
            if file.expected.as_ref().map(|(id, mode)| (id, mode))
                != expected
                    .as_ref()
                    .map(|target| (&target.file_id, &target.executable))
            {
                return Err(invalid("managed file exact expected base differs"));
            }
            let actual = CapturedTarget {
                file_id: file.manifest.file_id().clone(),
                executable: file.executable,
            };
            if Some(&actual) != expected.as_ref() {
                if !entry.writable || changes.len() >= MAX_CHANGES {
                    return Err(invalid(
                        "managed edit lacks write grant or exceeds patch bound",
                    ));
                }
                changed_bytes = changed_bytes
                    .checked_add(file.manifest.byte_length())
                    .filter(|n| *n <= MAX_CAPTURE_BYTES)
                    .ok_or_else(|| invalid("managed diff exceeds capture bytes"))?;
                overlay_manifests.insert(entry.path.clone(), file.manifest.id().clone());
                changes.push(CapturedChange {
                    path: entry.path.clone(),
                    expected,
                    replacement: Some(CapturedFile {
                        file_id: actual.file_id,
                        executable: actual.executable,
                        bytes: file.manifest.byte_length(),
                    }),
                });
            }
        } else if let Some(deletion) = deletion {
            if expected
                .as_ref()
                .map(|target| (&target.file_id, target.executable))
                != Some((deletion.base, deletion.executable))
                || !entry.writable
                || changes.len() >= MAX_CHANGES
            {
                return Err(invalid("managed deletion lacks exact writable preimage"));
            }
            changes.push(CapturedChange {
                path: entry.path.clone(),
                expected,
                replacement: None,
            });
        } else if expected.is_some() {
            return Err(invalid("managed selected file missing from complete image"));
        }
    }
    if !files.is_empty() || !deletions.is_empty() {
        return Err(invalid("managed image path is outside fresh observation"));
    }
    let mut capture = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: capture_sequence,
        previous_patch_digest: version.capture_head.clone(),
        projection_generation: placement_generation,
        projection_digest: placement_digest.clone(),
        identity: destination.clone(),
        changes,
        digest: String::new(),
    };
    capture.digest = capture.computed_digest()?;
    let overlays: Vec<_> = capture
        .changes
        .iter()
        .map(|change| OverlayRow {
            change,
            manifest_id: overlay_manifests.get(&change.path),
        })
        .collect();
    let working_overlay = rowset(OVERLAY_DOMAIN, &overlays)?;

    let mut required = BTreeMap::<(FileId, FileManifestId), RequiredManifest>::new();
    let mut lengths = BTreeMap::<FileId, u64>::new();
    let mut manifest_owners = BTreeMap::<FileManifestId, FileId>::new();
    for manifest in base_manifests {
        insert_required(
            &mut required,
            &mut lengths,
            &mut manifest_owners,
            RequiredManifest::from_manifest(manifest),
        )?;
    }
    for historical in historical {
        insert_required(
            &mut required,
            &mut lengths,
            &mut manifest_owners,
            historical.clone(),
        )?;
    }
    for file in image_files {
        insert_required(
            &mut required,
            &mut lengths,
            &mut manifest_owners,
            RequiredManifest::from_manifest(file.manifest),
        )?;
    }
    if required.len() > MAX_MANIFESTS || supplied.len() != required.len() {
        return Err(invalid("managed protected manifest coverage differs"));
    }
    let mut supplied_by_id = BTreeMap::new();
    for manifest in supplied {
        let row = RequiredManifest::from_manifest(manifest);
        let key = (row.file_id.clone(), row.manifest_id.clone());
        if supplied_by_id.insert(key.clone(), row.clone()).is_some()
            || required.get(&key) != Some(&row)
        {
            return Err(invalid(
                "managed supplied manifest differs from exact requirement",
            ));
        }
    }
    let required_manifests: Vec<_> = required.into_values().collect();
    let protected_manifests = rowset(MANIFEST_DOMAIN, &required_manifests)?;
    let quota_bytes = quota(supplied, image_files, metadata_bytes)?;
    let empty = rowset(b"layrs-provider-managed-companion-empty-v1\0", &[] as &[u8])?;
    let (companion_id, companion_files, companion_deletions) =
        match companions.and_then(|c| c.header()) {
            Some(header) => (
                Some(header.digest()?),
                TransitionRowset {
                    count: header.file_count,
                    digest: header.files_digest.clone(),
                },
                TransitionRowset {
                    count: header.deletion_count,
                    digest: header.deletions_digest.clone(),
                },
            ),
            None => (None, empty.clone(), empty),
        };
    let observation_record = TransitionObservation {
        step_id: observation.base_step_id.clone(),
        root_tree_id: observation.base_root_tree_id.clone(),
        policy_set_id: observation.observed_policy_set_id.clone(),
        authorization_epoch: observation.observed_authorization_epoch,
        generation: observation_generation,
        snapshot_digest: observation_digest.clone(),
        selection_digest: denial.map_or_else(
            || version.selection_digest.clone(),
            |(_, observed)| observed.binding().selection_digest.clone(),
        ),
    };
    let plan = TransitionPlan {
        denial_coverage: denial
            .map(|(coverage, _)| coverage.record().reference())
            .transpose()?,
        kind: ProviderImportKind::ManagedOrdinary,
        source_prefix_id,
        source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode()?)?,
        destination_parent: parent.to_owned(),
        destination_parent_identity: parent_identity.to_owned(),
        destination_leaf_nonce: leaf_nonce.to_owned(),
        observation: observation_record,
        placement_generation,
        placement_digest: placement_digest.clone(),
        capture_sequence,
        capture_digest: capture.digest.clone(),
        working_overlay,
        companion_id,
        companion_files,
        companion_deletions,
        protected_manifests,
        quota_bytes,
    };
    plan.digest()?;
    Ok(ManagedPlanData {
        observation,
        observation_digest,
        placement,
        placement_digest,
        capture,
        plan,
        required_manifests,
        quota_bytes,
        transaction_id: transaction_id.to_owned(),
        validated_metadata_floor: None,
    })
}

fn insert_required(
    rows: &mut BTreeMap<(FileId, FileManifestId), RequiredManifest>,
    lengths: &mut BTreeMap<FileId, u64>,
    owners: &mut BTreeMap<FileManifestId, FileId>,
    row: RequiredManifest,
) -> Result<()> {
    if row.bytes > MAX_CAPTURE_FILE_BYTES {
        return Err(invalid("managed required manifest exceeds file bound"));
    }
    if lengths
        .insert(row.file_id.clone(), row.bytes)
        .is_some_and(|old| old != row.bytes)
        || owners
            .insert(row.manifest_id.clone(), row.file_id.clone())
            .is_some_and(|old| old != row.file_id)
        || rows
            .insert((row.file_id.clone(), row.manifest_id.clone()), row.clone())
            .is_some_and(|old| old != row)
    {
        return Err(invalid(
            "managed manifest FileId, ManifestId or length conflict",
        ));
    }
    Ok(())
}

fn quota(manifests: &[FileManifest], image: &[ImageFile<'_>], metadata: u64) -> Result<u64> {
    crate::projection::provider::managed_budget::quota(
        manifests,
        image.iter().map(|file| file.manifest.byte_length()),
        metadata,
    )
}

#[cfg(test)]
#[path = "managed_plan/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "managed_plan/ordinary_tests.rs"]
mod ordinary_tests;

#[cfg(test)]
#[path = "managed_plan/denial_tests.rs"]
mod denial_tests;
