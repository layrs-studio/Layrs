//! Deterministic ordinary Ready DATA from the driver's rederived B9 plan.
//! No SQL, filesystem/native read, owner acquisition or source authentication
//! occurs here. In particular the supplied base floor remains DATA, and the
//! deliberately absent validated_metadata_floor is never populated. The driver
//! must authenticate actual B admission/prefix references, image and B/C Staging
//! before and after effects, and retain the physical CAS/checkout witnesses.
use super::{
    content,
    image::{OrdinaryExportImage, OrdinaryImageDeletion, OrdinaryImageFile},
};
use crate::projection::{
    capture::{CapturedChange, CapturedPatch},
    database,
    provider::{
        bootstrap::ManagedPlanData,
        cancellation,
        destination_ready::{ManagedReadyClaim, ManagedReadyData, ManagedReadyOverlay},
        managed_budget,
        records::{
            import::{
                ImportRowset, ImportSource, ProviderImportKind, ProviderImportPrefixRecord,
                ProviderTransitionReceiptRecord,
            },
            transition::{
                ProviderTransitionIntentRecord, TransitionBackendKind, TransitionRole,
                TransitionState,
            },
            Record,
        },
        recovery::error::{invalid, Result as CauseResult},
    },
    shared::{exact_content::canonical_manifest_bytes, exact_protection::ExactProtection},
};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{canonical_json, FileId, FileManifest, FileManifestId},
};
use layrs_vfs::Cancellation;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const MAX_FILES: usize = 100_000;
const MAX_CHANGES: usize = 1024;
const MAX_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 32 * 1024 * 1024;
const MAX_METADATA: u64 = 1073741824;

pub(super) struct OwnedOrdinaryReadyData {
    derived: ManagedPlanData,
    destination_staging: ProviderTransitionIntentRecord,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    overlay: Vec<ManagedReadyOverlay>,
    claims: [ManagedReadyClaim; 1],
    actual_base_metadata_floor: u64,
    metadata_floor: u64,
}

impl OwnedOrdinaryReadyData {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        derived: ManagedPlanData,
        source_staging: &ProviderTransitionIntentRecord,
        image: &OrdinaryExportImage,
        required: &[FileManifest],
        protection: &ExactProtection,
        actual_base_metadata_floor: u64,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        cancel.check()?;
        // Record encoding supplies its bounded preflight before cloning any
        // source/proof fields. Durability is a separate driver responsibility.
        source_staging.encode()?;
        if derived.validated_metadata_floor.is_some()
            || actual_base_metadata_floor == 0
            || actual_base_metadata_floor > MAX_METADATA
            || source_staging.role != TransitionRole::Source
            || source_staging.state != TransitionState::Staging
            || source_staging.receipt_id.is_some()
            || source_staging.reservation.as_deref()
                != Some(source_staging.reservation_operation()?.as_str())
            || derived.plan != source_staging.plan
            || derived.transaction_id != source_staging.transaction_id
            || derived.quota_bytes != derived.plan.quota_bytes
            || derived.plan.kind != ProviderImportKind::ManagedOrdinary
            || derived.plan.companion_id.is_some()
            || derived.plan.companion_files.count != 0
            || derived.plan.companion_deletions.count != 0
            || derived.capture.changes.len() > MAX_CHANGES
            || derived.capture.changes.iter().any(|change| {
                change.path.as_str().len() > 4096
                    || change
                        .replacement
                        .as_ref()
                        .is_some_and(|file| file.bytes > MAX_METADATA)
            })
        {
            return Err(invalid(
                "ordinary Ready data differs from rederived Staging/floor",
            ));
        }
        require_union(&derived, required, Some(cancel))?;
        require_snapshot_bound(&derived.observation, cancel)?;
        require_snapshot_bound(&derived.placement, cancel)?;
        let prefix = &derived.plan.source_prefix;
        let ImportSource::Ordinary {
            identity,
            private_schema: 9 | 10,
            admission: Some(admission),
            checkout_identity: Some(checkout_identity),
            version,
            ..
        } = &prefix.source
        else {
            return Err(invalid(
                "ordinary Ready requires admitted private9 source DATA",
            ));
        };
        let destination = source_staging
            .destination
            .as_ref()
            .ok_or_else(|| invalid("ordinary Ready destination proof absent"))?;
        let family = destination
            .format
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary Ready shared destination absent"))?;
        if !prefix.supports_restore_history()
            || source_staging.local_generation != version.generation
            || image.digest() != version.effective_image_digest
            || checkout_identity.is_empty()
            || identity.worktree_id == prefix.destination_identity.worktree_id
            || identity.store_id != prefix.destination_identity.store_id
            || identity.space_id != prefix.destination_identity.space_id
            || identity.origin != prefix.destination_identity.origin
            || identity.principal_id != prefix.destination_identity.principal_id
            || destination.backend != TransitionBackendKind::Ordinary
            || destination.native_control_identity_hex.is_some()
            || destination.native_root_identity.is_some()
            || destination.format.identity != prefix.destination_identity
            || destination.private_database_identity != family.database_identity
            || admission.database_identity == destination.private_database_identity
            || version.generation.checked_add(1) != Some(derived.plan.observation.generation)
            || derived.plan.observation.generation.checked_add(1)
                != Some(derived.plan.placement_generation)
            || version.capture_count.checked_add(1) != Some(derived.plan.capture_sequence)
            || derived.observation_digest != derived.plan.observation.snapshot_digest
            || derived.placement_digest != derived.plan.placement_digest
            // derive_ordinary_data passes actual B current for BOTH bodies;
            // their different generation/parent frames produce distinct digests.
            || derived.observation != derived.placement
        {
            return Err(invalid(
                "ordinary Ready identity/image/local boundaries differ",
            ));
        }
        let observation = derived
            .observation
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?;
        let placement = derived
            .placement
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?;
        if observation.len() > 64 * 1024 * 1024 || placement.len() > 64 * 1024 * 1024 {
            return Err(invalid("ordinary Ready snapshot payload exceeds bound"));
        }
        if database::digest(
            derived.plan.observation.generation,
            Some(&version.snapshot_digest),
            &observation,
        ) != derived.observation_digest
            || database::digest(
                derived.plan.placement_generation,
                Some(&derived.observation_digest),
                &placement,
            ) != derived.placement_digest
            || derived.observation.base_step_id != derived.plan.observation.step_id
            || derived.observation.base_root_tree_id != derived.plan.observation.root_tree_id
            || derived.observation.observed_policy_set_id != derived.plan.observation.policy_set_id
            || derived.observation.observed_authorization_epoch
                != derived.plan.observation.authorization_epoch
            || derived.plan.observation.selection_digest != version.selection_digest
            || derived.capture.schema_version != "layrs.projection-capture.v1"
            || derived.capture.identity != prefix.destination_identity
            || derived.capture.sequence != derived.plan.capture_sequence
            || derived.capture.previous_patch_digest != version.capture_head
            || derived.capture.projection_generation != derived.plan.placement_generation
            || derived.capture.projection_digest != derived.placement_digest
            || derived.capture.digest != derived.plan.capture_digest
            || derived.capture.computed_digest()? != derived.capture.digest
        {
            return Err(invalid(
                "ordinary Ready snapshot/capture commitments differ",
            ));
        }
        let overlay = build_overlay(
            &derived.capture,
            image.files(),
            image.deletions(),
            required,
            cancel,
        )?;
        require_overlay(&derived, &overlay, cancel)?;
        let mut destination_staging =
            ProviderTransitionIntentRecord::decode(&source_staging.encode()?)?;
        destination_staging.role = TransitionRole::Destination;
        destination_staging.local_generation = 0;
        destination_staging.encode()?;
        let expected_root = ExactProtection::new(
            &family.incarnation,
            &prefix.destination_identity.worktree_id,
            &family.database_identity,
            &source_staging.transaction_id,
            required,
            Some(cancel),
        )?;
        if protection != &expected_root {
            return Err(invalid(
                "ordinary Ready protection differs from complete exact union",
            ));
        }
        let plan = &derived.plan;
        let receipt = ProviderTransitionReceiptRecord {
            schema_version: "layrs.projection-provider-transition-receipt.v1".into(),
            owner_identity_digest: destination.owner_digest(plan)?,
            transaction_id: source_staging.transaction_id.clone(),
            transition_plan_digest: plan.digest()?,
            destination_proof_digest: destination.digest(plan)?,
            source_prefix_id: plan.source_prefix_id.clone(),
            source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode()?)?,
            parent_receipt_id: prefix.ancestry_root_receipt.clone(),
            source_identity: identity.clone(),
            destination_identity: prefix.destination_identity.clone(),
            imported_max_generation: version.generation,
            destination_observation_generation: plan.observation.generation,
            destination_observation_digest: derived.observation_digest.clone(),
            destination_placement_generation: plan.placement_generation,
            destination_placement_digest: derived.placement_digest.clone(),
            imported_capture_count: version.capture_count,
            destination_capture_sequence: plan.capture_sequence,
            destination_capture_digest: derived.capture.digest.clone(),
            protected_manifests: ImportRowset {
                count: plan.protected_manifests.count,
                digest: plan.protected_manifests.digest.clone(),
            },
            content_mappings: prefix.closure.content_mappings.clone(),
            companion_id: None,
            companion_files: ImportRowset {
                count: plan.companion_files.count,
                digest: plan.companion_files.digest.clone(),
            },
            companion_deletions: ImportRowset {
                count: plan.companion_deletions.count,
                digest: plan.companion_deletions.digest.clone(),
            },
        };
        receipt.encode()?;
        receipt.require_prefix(prefix)?;
        let mut ready = ProviderTransitionIntentRecord::decode(&destination_staging.encode()?)?;
        ready.state = TransitionState::Ready;
        ready.receipt_id = Some(receipt.digest()?);
        ready.require_transition(&destination_staging)?;
        ready.require_receipt(&receipt)?;
        ready.encode()?;
        let claims = [ManagedReadyClaim {
            root_id: protection.id().into(),
            digest: protection.digest().into(),
            kind: "provider-import".into(),
            owner: source_staging.transaction_id.clone(),
        }];
        let metadata_floor = managed_budget::complete_metadata_floor(
            actual_base_metadata_floor,
            &ready,
            &receipt,
            &claims[0],
            required,
            Some(cancel),
        )?;
        // Reuse the ordinary checkout DATA constructor for its complete bounded
        // inventory and Windows alias/leaf rules; no physical lookup occurs.
        let checkout = content::expected_checkout(source_staging, image, cancel)?;
        let baseline_quota = managed_budget::quota(
            required,
            image.files().iter().map(|f| f.manifest.byte_length()),
            0,
        )?;
        let admission = managed_budget::ordinary_admission_budget(
            metadata_floor,
            checkout.files().len() as u64,
            checkout.directories().len() as u64,
            required.len() as u64,
        )?;
        let metadata = admission.require_quota(derived.quota_bytes, baseline_quota)?;
        // This checks the durable fixed funding DATA, not remaining SQL quota.
        // The driver/SQL gate independently checks real debits and remaining.
        protection.require_envelope(metadata, admission.minimum_metadata())?;
        cancel.check()?;
        Ok(Self {
            derived,
            destination_staging,
            ready,
            receipt,
            overlay,
            claims,
            actual_base_metadata_floor,
            metadata_floor,
        })
    }

    pub(super) fn as_data<'a>(
        &'a self,
        required: &'a [FileManifest],
    ) -> CauseResult<ManagedReadyData<'a>> {
        // Large inventories cannot be substituted between construction and use.
        // This is still DATA; actual retained SQL and bytes stay driver-owned.
        require_union(&self.derived, required, None)?;
        Ok(ManagedReadyData {
            observation: &self.derived.observation,
            placement: &self.derived.placement,
            capture: &self.derived.capture,
            overlay: &self.overlay,
            required,
            receipt: &self.receipt,
            ready: &self.ready,
            claims: &self.claims,
        })
    }
    pub(super) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(super) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(super) fn destination_staging(&self) -> &ProviderTransitionIntentRecord {
        &self.destination_staging
    }
    pub(super) fn metadata_floor(&self) -> u64 {
        self.metadata_floor
    }
    pub(super) fn actual_base_metadata_floor(&self) -> u64 {
        self.actual_base_metadata_floor
    }
}

fn require_snapshot_bound(snapshot: &ProjectionSnapshot, cancel: &Cancellation) -> CauseResult<()> {
    if snapshot.entries.len() > MAX_FILES {
        return Err(invalid("ordinary Ready snapshot entry count exceeds bound"));
    }
    // Mirror ProjectionSnapshot::validate's conservative encoded-size bound.
    // The selected image has its own 8MiB path bound; unselected snapshot paths
    // must not accidentally inherit that narrower checkout-only restriction.
    let mut upper = 1024usize;
    for entry in &snapshot.entries {
        cancel.check()?;
        upper = entry
            .path
            .as_str()
            .len()
            .checked_mul(6)
            .and_then(|n| n.checked_add(512))
            .and_then(|n| upper.checked_add(n))
            .filter(|n| *n <= 64 * 1024 * 1024)
            .ok_or_else(|| invalid("ordinary Ready snapshot encoded size exceeds bound"))?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequiredRow<'a> {
    file_id: &'a FileId,
    manifest_id: &'a FileManifestId,
    bytes: u64,
}

fn require_union(
    derived: &ManagedPlanData,
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    cancellation::check(cancel)?;
    if required.len() > MAX_FILES
        || derived.required_manifests.len() != required.len()
        || derived.plan.protected_manifests.count != required.len() as u64
    {
        return Err(invalid("ordinary Ready complete union count differs"));
    }
    let mut ordered = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut lengths = BTreeMap::new();
    let mut bytes = 0usize;
    for manifest in required {
        cancellation::check(cancel)?;
        bytes = bytes
            .checked_add(canonical_manifest_bytes(manifest)?.len())
            .filter(|n| *n <= MAX_MANIFEST_BYTES)
            .ok_or_else(|| invalid("ordinary Ready canonical union exceeds 32MiB"))?;
        if ordered
            .insert((manifest.file_id(), manifest.id()), manifest)
            .is_some()
            || !ids.insert(manifest.id())
            || lengths
                .insert(manifest.file_id(), manifest.byte_length())
                .is_some_and(|old| old != manifest.byte_length())
        {
            return Err(invalid(
                "ordinary Ready union duplicates or contradicts representation",
            ));
        }
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
    hash.update(&(ordered.len() as u64).to_be_bytes());
    for (manifest, row) in ordered.into_values().zip(&derived.required_manifests) {
        cancellation::check(cancel)?;
        if manifest.file_id() != &row.file_id
            || manifest.id() != &row.manifest_id
            || manifest.byte_length() != row.bytes
        {
            return Err(invalid("ordinary Ready rederived exact union differs"));
        }
        let encoded = canonical_json(&RequiredRow {
            file_id: manifest.file_id(),
            manifest_id: manifest.id(),
            bytes: manifest.byte_length(),
        })
        .map_err(|e| invalid(e.to_string()))?;
        hash.update(&(encoded.len() as u64).to_be_bytes());
        hash.update(&encoded);
    }
    if format!("blake3:{}", hash.finalize()) != derived.plan.protected_manifests.digest {
        return Err(invalid("ordinary Ready complete union commitment differs"));
    }
    Ok(())
}

fn build_overlay(
    capture: &CapturedPatch,
    files: &[OrdinaryImageFile],
    deletions: &[OrdinaryImageDeletion],
    required: &[FileManifest],
    cancel: &Cancellation,
) -> CauseResult<Vec<ManagedReadyOverlay>> {
    cancel.check()?;
    if capture.changes.len() > MAX_CHANGES
        || files.len() > MAX_FILES
        || deletions.len() > MAX_FILES
        || files
            .len()
            .checked_add(deletions.len())
            .is_none_or(|n| n > MAX_FILES)
        || required.len() > MAX_FILES
        || files.windows(2).any(|p| p[0].path >= p[1].path)
        || deletions.windows(2).any(|p| p[0].path >= p[1].path)
    {
        return Err(invalid("ordinary Ready overlay count/order differs"));
    }
    let by_id: BTreeMap<_, _> = required.iter().map(|m| (m.id(), m)).collect();
    let mut path_bytes = 0usize;
    let mut image_bytes = 0u64;
    for file in files {
        cancel.check()?;
        if file.path.as_str().len() > 4096
            || file.manifest.byte_length() > MAX_METADATA
            || by_id.get(file.manifest.id()).copied() != Some(&file.manifest)
            || deletions
                .binary_search_by(|d| d.path.cmp(&file.path))
                .is_ok()
        {
            return Err(invalid(
                "ordinary Ready image path/union/representation differs",
            ));
        }
        path_bytes = path_bytes
            .checked_add(file.path.as_str().len())
            .filter(|n| *n <= MAX_PATH_BYTES)
            .ok_or_else(|| invalid("ordinary Ready image path bytes exceed bound"))?;
        image_bytes = image_bytes
            .checked_add(file.manifest.byte_length())
            .filter(|n| *n <= 4 * MAX_METADATA)
            .ok_or_else(|| invalid("ordinary Ready image exceeds 4GiB"))?;
    }
    for deletion in deletions {
        cancel.check()?;
        if deletion.path.as_str().len() > 4096 {
            return Err(invalid("ordinary Ready deletion path exceeds bound"));
        }
        path_bytes = path_bytes
            .checked_add(deletion.path.as_str().len())
            .filter(|n| *n <= MAX_PATH_BYTES)
            .ok_or_else(|| invalid("ordinary Ready deletion path bytes exceed bound"))?;
    }
    let mut output = Vec::with_capacity(capture.changes.len());
    let mut previous = None;
    for change in &capture.changes {
        cancel.check()?;
        if change.path.as_str().len() > 4096
            || previous.is_some_and(|path: &layrs_core::scm::RepoPath| path >= &change.path)
        {
            return Err(invalid(
                "ordinary Ready overlay paths are not bounded/sorted/unique",
            ));
        }
        previous = Some(&change.path);
        let file = files
            .binary_search_by(|f| f.path.cmp(&change.path))
            .ok()
            .map(|n| &files[n]);
        let deletion = deletions
            .binary_search_by(|d| d.path.cmp(&change.path))
            .ok()
            .map(|n| &deletions[n]);
        let manifest_id = match (&change.replacement, file, deletion) {
            (Some(replacement), Some(file), None) => {
                let manifest = &file.manifest;
                if manifest.file_id() != &replacement.file_id
                    || manifest.byte_length() != replacement.bytes
                    || file.executable != replacement.executable
                    || file.expected.as_ref().map(|(id, mode)| (id, *mode))
                        != change.expected.as_ref().map(|e| (&e.file_id, e.executable))
                {
                    return Err(invalid(
                        "ordinary Ready overlay exact representation/preimage differs",
                    ));
                }
                Some(manifest.id().clone())
            }
            (None, None, Some(deletion))
                if change.expected.as_ref().is_some_and(|e| {
                    e.file_id == deletion.base && e.executable == deletion.executable
                }) =>
            {
                None
            }
            _ => {
                return Err(invalid(
                    "ordinary Ready overlay exact image path absent or conflicting",
                ))
            }
        };
        output.push(ManagedReadyOverlay {
            path: change.path.clone(),
            manifest_id,
        });
    }
    Ok(output)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayRow<'a> {
    change: &'a CapturedChange,
    manifest_id: Option<&'a FileManifestId>,
}

fn require_overlay(
    derived: &ManagedPlanData,
    overlay: &[ManagedReadyOverlay],
    cancel: &Cancellation,
) -> CauseResult<()> {
    if overlay.len() != derived.capture.changes.len()
        || overlay.len() as u64 != derived.plan.working_overlay.count
    {
        return Err(invalid("ordinary Ready overlay count differs from plan"));
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-overlay-v1\0");
    hash.update(&(overlay.len() as u64).to_be_bytes());
    for (change, row) in derived.capture.changes.iter().zip(overlay) {
        cancel.check()?;
        if change.path != row.path {
            return Err(invalid("ordinary Ready overlay path differs from capture"));
        }
        let encoded = canonical_json(&OverlayRow {
            change,
            manifest_id: row.manifest_id.as_ref(),
        })
        .map_err(|e| invalid(e.to_string()))?;
        hash.update(&(encoded.len() as u64).to_be_bytes());
        hash.update(&encoded);
    }
    if format!("blake3:{}", hash.finalize()) != derived.plan.working_overlay.digest {
        return Err(invalid(
            "ordinary Ready overlay commitment differs from plan",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::capture::{CapturedFile, CapturedTarget};
    use layrs_core::scm::{Chunk, ChunkingDescriptor, RepoPath};
    use serde_json::json;

    fn manifest(chunking: ChunkingDescriptor) -> FileManifest {
        FileManifest::from_chunks(chunking, &[Chunk::new(b"replacement".to_vec()).unwrap()])
            .unwrap()
    }

    fn patch(current: &FileManifest) -> CapturedPatch {
        CapturedPatch {
            schema_version: "layrs.projection-capture.v1".into(),
            sequence: 2,
            previous_patch_digest: None,
            projection_generation: 5,
            projection_digest: "a".repeat(64),
            identity: serde_json::from_value(json!({
                "storeId": "store:test", "spaceId": "space:test", "worktreeId": "worktree:c",
                "origin": "https://example.invalid", "principalId": "principal:test"
            }))
            .unwrap(),
            changes: vec![CapturedChange {
                path: RepoPath::parse("a").unwrap(),
                expected: Some(CapturedTarget {
                    file_id: FileId::from_bytes(b"base"),
                    executable: false,
                }),
                replacement: Some(CapturedFile {
                    file_id: current.file_id().clone(),
                    bytes: current.byte_length(),
                    executable: false,
                }),
            }],
            digest: String::new(),
        }
    }

    #[test]
    fn ordinary_ready_overlay_keeps_current_exact_representation_and_preimage() {
        let current = manifest(ChunkingDescriptor::deterministic_fixed(1048576).unwrap());
        let historical = manifest(ChunkingDescriptor::single());
        assert_eq!(current.file_id(), historical.file_id());
        assert_ne!(current.id(), historical.id());
        let capture = patch(&current);
        let mut files = [OrdinaryImageFile {
            path: RepoPath::parse("a").unwrap(),
            manifest: current.clone(),
            executable: false,
            expected: Some((FileId::from_bytes(b"base"), false)),
        }];
        let cancel = Cancellation::default();
        assert!(build_overlay(&capture, &files, &[], &[historical.clone()], &cancel).is_err());
        let required = [historical, current.clone()];
        let overlay = build_overlay(&capture, &files, &[], &required, &cancel).unwrap();
        assert_eq!(overlay[0].manifest_id.as_ref(), Some(current.id()));
        files[0].expected = Some((FileId::from_bytes(b"wrong preimage"), false));
        assert!(build_overlay(&capture, &files, &[], &required, &cancel).is_err());
    }

    #[test]
    fn ordinary_ready_deletion_requires_exact_preimage_and_observes_precancel() {
        let current = manifest(ChunkingDescriptor::single());
        let mut capture = patch(&current);
        capture.changes[0].replacement = None;
        let mut deletions = [OrdinaryImageDeletion {
            path: RepoPath::parse("a").unwrap(),
            base: FileId::from_bytes(b"base"),
            executable: true,
        }];
        let cancel = Cancellation::default();
        assert!(build_overlay(&capture, &[], &deletions, &[], &cancel).is_err());
        deletions[0].executable = false;
        let overlay = build_overlay(&capture, &[], &deletions, &[], &cancel).unwrap();
        assert!(overlay[0].manifest_id.is_none());
        cancel.cancel();
        assert!(build_overlay(&capture, &[], &deletions, &[], &cancel).is_err());
    }
}
