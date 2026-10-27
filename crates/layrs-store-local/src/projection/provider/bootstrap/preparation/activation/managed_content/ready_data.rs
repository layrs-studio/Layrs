//! Deterministic owned Ready DATA from a source-SQL-rederived plan. Construction
//! performs no SQL, filesystem/native query, byte read or fresh authorization.
//! The physical driver retains and revalidates those independent witnesses.
use super::super::{
    managed_destination,
    managed_image::{ManagedDeletion, ManagedFile, VerifiedManagedImage},
    managed_plan::{self, ManagedPlanData},
};
use crate::projection::{
    capture::CapturedPatch,
    database,
    provider::{
        destination_ready::{ManagedReadyClaim, ManagedReadyData, ManagedReadyOverlay},
        records::{
            import::{
                ImportRowset, ImportSource, ProviderImportKind, ProviderImportPrefixRecord,
                ProviderTransitionReceiptRecord,
            },
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
            Record,
        },
        recovery::error::{invalid, Result},
    },
    shared::exact_protection::ExactProtection,
};
use layrs_core::scm::{canonical_json, FileManifest, FileManifestId};
use layrs_vfs::Cancellation;
use serde::Serialize;
use std::collections::BTreeMap;

pub(super) struct OwnedReadyData {
    derived: ManagedPlanData,
    destination_staging: ProviderTransitionIntentRecord,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    overlay: Vec<ManagedReadyOverlay>,
    claims: [ManagedReadyClaim; 1],
    metadata_floor: u64,
}
impl OwnedReadyData {
    pub(super) fn new(
        derived: ManagedPlanData,
        source_staging: &ProviderTransitionIntentRecord,
        image: &VerifiedManagedImage<'_, '_>,
        required: &[FileManifest],
        protection: &ExactProtection,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        if derived.capture.changes.len() > 1024
            || derived.required_manifests.len() > 100_000
            || required.len() > 100_000
            || derived.capture.changes.iter().any(|c| {
                c.path.as_str().len() > 4096
                    || c.replacement.as_ref().is_some_and(|f| f.bytes > 1073741824)
            })
        {
            return Err(invalid(
                "Ready data capture or manifest inventory exceeds bound",
            ));
        }
        source_staging.validate_fields()?;
        let floor = derived
            .validated_metadata_floor
            .filter(|floor| *floor <= 1073741824 && *floor <= derived.quota_bytes)
            .ok_or_else(|| invalid("Ready data requires source-rederived metadata floor"))?;
        if source_staging.role != TransitionRole::Source
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
        {
            return Err(invalid(
                "Ready data differs from exact managed source Staging",
            ));
        }
        managed_plan::require_manifest_union(&derived.plan, required)?;
        require_derived_union(&derived, required, cancel)?;
        let prefix = &derived.plan.source_prefix;
        let ImportSource::Provider {
            identity, version, ..
        } = &prefix.source
        else {
            return Err(invalid("Ready data requires provider prefix"));
        };
        if image.digest() != version.effective_image_digest
            || version.generation.checked_add(1) != Some(derived.plan.observation.generation)
            || derived.plan.observation.generation.checked_add(1)
                != Some(derived.plan.placement_generation)
            || version.capture_count.checked_add(1) != Some(derived.plan.capture_sequence)
            || derived.observation_digest != derived.plan.observation.snapshot_digest
            || derived.placement_digest != derived.plan.placement_digest
        {
            return Err(invalid("Ready data image or local boundaries differ"));
        }
        let observation = derived
            .observation
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?;
        let placement = derived
            .placement
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?;
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
            return Err(invalid("Ready data snapshot or capture commitments differ"));
        }
        let overlay = build_overlay(
            &derived.capture,
            image.files(),
            image.deletions(),
            required,
            cancel,
        )?;
        require_overlay(&derived, &overlay, cancel)?;
        let destination_staging = managed_destination::destination_intent(source_staging)?;
        let destination = destination_staging
            .destination
            .as_ref()
            .ok_or_else(|| invalid("Ready data destination proof absent"))?;
        let family = destination
            .format
            .shared
            .as_ref()
            .ok_or_else(|| invalid("Ready data family binding absent"))?;
        let expected = ExactProtection::new(
            &family.incarnation,
            &prefix.destination_identity.worktree_id,
            &family.database_identity,
            &source_staging.transaction_id,
            required,
            Some(cancel),
        )?;
        if protection != &expected {
            return Err(invalid(
                "Ready data protection differs from exact complete union",
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
            companion_id: plan.companion_id.clone(),
            companion_files: ImportRowset {
                count: plan.companion_files.count,
                digest: plan.companion_files.digest.clone(),
            },
            companion_deletions: ImportRowset {
                count: plan.companion_deletions.count,
                digest: plan.companion_deletions.digest.clone(),
            },
        };
        receipt.validate_fields()?;
        receipt.require_prefix(prefix)?;
        let mut ready = ProviderTransitionIntentRecord::decode(&destination_staging.encode()?)?;
        ready.state = TransitionState::Ready;
        ready.receipt_id = Some(receipt.digest()?);
        ready.require_transition(&destination_staging)?;
        ready.require_receipt(&receipt)?;
        let claims = [ManagedReadyClaim {
            root_id: protection.id().into(),
            digest: protection.digest().into(),
            kind: "provider-import".into(),
            owner: source_staging.transaction_id.clone(),
        }];
        let metadata_floor =
            complete_metadata_floor(floor, &ready, &receipt, &claims[0], required, cancel)?;
        if metadata_floor > derived.quota_bytes {
            return Err(invalid(
                "Ready data retained metadata envelope insufficient",
            ));
        }
        cancel.check()?;
        Ok(Self {
            derived,
            destination_staging,
            ready,
            receipt,
            overlay,
            claims,
            metadata_floor,
        })
    }

    /// Attach only the same complete canonical union. These borrowed inputs
    /// remain DATA; DestinationReady independently revalidates their SQL scope.
    pub(super) fn as_data<'a>(
        &'a self,
        required: &'a [FileManifest],
    ) -> Result<ManagedReadyData<'a>> {
        managed_plan::require_manifest_union(&self.derived.plan, required)?;
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
    pub(super) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
    pub(super) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(super) fn destination_staging(&self) -> &ProviderTransitionIntentRecord {
        &self.destination_staging
    }
    pub(super) fn staging(&self) -> &ProviderTransitionIntentRecord {
        &self.destination_staging
    }
    pub(super) fn metadata_floor(&self) -> u64 {
        self.metadata_floor
    }
}

// Preserve the live constructor's exact accounting and test-facing helpers.
fn complete_metadata_floor(
    base: u64,
    ready: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    claim: &ManagedReadyClaim,
    required: &[FileManifest],
    cancel: &Cancellation,
) -> Result<u64> {
    crate::projection::provider::managed_budget::complete_metadata_floor(
        base,
        ready,
        receipt,
        claim,
        required,
        Some(cancel),
    )
}
#[cfg(test)]
fn add_metadata(sum: u64, bytes: u64) -> Result<u64> {
    crate::projection::provider::managed_budget::add_metadata(sum, bytes)
}
#[cfg(test)]
fn content_mapping_metadata(required: &[FileManifest], cancel: &Cancellation) -> Result<u64> {
    crate::projection::provider::managed_budget::content_mapping_metadata(required, Some(cancel))
}
fn require_derived_union(
    derived: &ManagedPlanData,
    required: &[FileManifest],
    cancel: &Cancellation,
) -> Result<()> {
    let ordered: BTreeMap<_, _> = required
        .iter()
        .map(|m| ((m.file_id(), m.id()), m.byte_length()))
        .collect();
    if ordered.len() != derived.required_manifests.len() {
        return Err(invalid("Ready data rederived manifest inventory differs"));
    }
    for (((file, manifest), bytes), row) in ordered.into_iter().zip(&derived.required_manifests) {
        cancel.check()?;
        if file != &row.file_id || manifest != &row.manifest_id || bytes != row.bytes {
            return Err(invalid("Ready data rederived manifest inventory differs"));
        }
    }
    Ok(())
}

fn build_overlay(
    capture: &CapturedPatch,
    files: &[ManagedFile<'_>],
    deletions: &[ManagedDeletion],
    required: &[FileManifest],
    cancel: &Cancellation,
) -> Result<Vec<ManagedReadyOverlay>> {
    if capture.changes.len() > 1024
        || files.len() > 100_000
        || deletions.len() > 100_000
        || required.len() > 100_000
        || files.windows(2).any(|p| p[0].path >= p[1].path)
        || deletions.windows(2).any(|p| p[0].path >= p[1].path)
    {
        return Err(invalid("Ready overlay input count or order differs"));
    }
    let by_id: BTreeMap<_, _> = required.iter().map(|m| (m.id(), m)).collect();
    let mut output = Vec::with_capacity(capture.changes.len());
    let mut previous = None;
    for change in &capture.changes {
        cancel.check()?;
        if previous.is_some_and(|path: &layrs_core::scm::RepoPath| path >= &change.path) {
            return Err(invalid("Ready overlay paths are not sorted and unique"));
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
                let manifest = file.manifest.manifest();
                if manifest.file_id() != &replacement.file_id
                    || manifest.byte_length() != replacement.bytes
                    || file.executable != replacement.executable
                    || file.expected.as_ref().map(|(id, mode)| (id, *mode))
                        != change.expected.as_ref().map(|e| (&e.file_id, e.executable))
                    || by_id.get(manifest.id()).copied() != Some(manifest)
                {
                    return Err(invalid(
                        "Ready overlay image representation or preimage differs",
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
                    "Ready overlay exact image path absent or conflicting",
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
    change: &'a crate::projection::capture::CapturedChange,
    manifest_id: Option<&'a FileManifestId>,
}
fn require_overlay(
    derived: &ManagedPlanData,
    overlay: &[ManagedReadyOverlay],
    cancel: &Cancellation,
) -> Result<()> {
    if overlay.len() != derived.capture.changes.len()
        || overlay.len() as u64 != derived.plan.working_overlay.count
    {
        return Err(invalid("Ready overlay cardinality differs from plan"));
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-overlay-v1\0");
    hash.update(&(overlay.len() as u64).to_be_bytes());
    for (change, row) in derived.capture.changes.iter().zip(overlay) {
        cancel.check()?;
        if change.path != row.path {
            return Err(invalid("Ready overlay path differs from capture"));
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
        return Err(invalid("Ready overlay commitment differs from plan"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{
        capture::{CapturedChange, CapturedFile, CapturedTarget},
        provider::{
            bootstrap::preparation::activation::managed_image::ManagedManifest,
            validation::tests::fixture::Fixture,
        },
    };
    use layrs_core::scm::{Chunk, ChunkingDescriptor, FileId, RepoPath};

    fn manifest(chunking: ChunkingDescriptor) -> FileManifest {
        FileManifest::from_chunks(chunking, &[Chunk::new(b"replacement".to_vec()).unwrap()])
            .unwrap()
    }
    fn patch() -> CapturedPatch {
        let current = manifest(ChunkingDescriptor::deterministic_fixed(1048576).unwrap());
        CapturedPatch {
            schema_version: "layrs.projection-capture.v1".into(),
            sequence: 1,
            previous_patch_digest: None,
            projection_generation: 3,
            projection_digest: "a".repeat(64),
            identity: Fixture::new("G1", false).allocation.format.identity,
            changes: vec![CapturedChange {
                path: RepoPath::parse("a").unwrap(),
                expected: Some(CapturedTarget {
                    file_id: FileId::from_bytes(b"base"),
                    executable: false,
                }),
                replacement: Some(CapturedFile {
                    file_id: current.file_id().clone(),
                    executable: false,
                    bytes: current.byte_length(),
                }),
            }],
            digest: String::new(),
        }
    }
    fn native(path: &str, manifest: FileManifest) -> ManagedFile<'static> {
        ManagedFile {
            path: RepoPath::parse(path).unwrap(),
            manifest: ManagedManifest::Native(manifest),
            executable: false,
            expected: Some((FileId::from_bytes(b"base"), false)),
        }
    }
    #[test]
    fn overlay_requires_exact_path_and_current_representation_among_same_file_id() {
        let current = manifest(ChunkingDescriptor::deterministic_fixed(1048576).unwrap());
        let historical = manifest(ChunkingDescriptor::single());
        assert_eq!(current.file_id(), historical.file_id());
        assert_ne!(current.id(), historical.id());
        let capture = patch();
        let cancel = Cancellation::default();
        let required = [historical.clone(), current.clone()];
        let other = [native("b", current.clone())];
        let error = build_overlay(&capture, &other, &[], &required, &cancel)
            .err()
            .unwrap();
        assert!(error.to_string().contains("exact image path absent"));
        let files = [native("a", current.clone())];
        assert!(build_overlay(&capture, &files, &[], &[historical], &cancel).is_err());
        let overlay = build_overlay(&capture, &files, &[], &required, &cancel).unwrap();
        assert_eq!(overlay.len(), 1);
        assert_eq!(overlay[0].manifest_id.as_ref(), Some(current.id()));
        assert_eq!(overlay[0].path, capture.changes[0].path);
    }
    #[test]
    fn deletion_overlay_requires_exact_image_preimage_and_unique_paths() {
        let mut capture = patch();
        capture.changes[0].replacement = None;
        let cancel = Cancellation::default();
        assert!(build_overlay(&capture, &[], &[], &[], &cancel).is_err());
        let mut deletion = ManagedDeletion {
            path: capture.changes[0].path.clone(),
            base: FileId::from_bytes(b"base"),
            executable: true,
        };
        assert!(build_overlay(&capture, &[], &[deletion], &[], &cancel).is_err());
        deletion = ManagedDeletion {
            path: capture.changes[0].path.clone(),
            base: FileId::from_bytes(b"base"),
            executable: false,
        };
        let overlay = build_overlay(&capture, &[], &[deletion], &[], &cancel).unwrap();
        assert!(overlay[0].manifest_id.is_none());
        capture.changes.push(capture.changes[0].clone());
        let deletion = ManagedDeletion {
            path: capture.changes[0].path.clone(),
            base: FileId::from_bytes(b"base"),
            executable: false,
        };
        let error = build_overlay(&capture, &[], &[deletion], &[], &cancel)
            .err()
            .unwrap();
        assert!(error.to_string().contains("not sorted and unique"));
    }

    #[test]
    fn absent_selected_paths_still_require_two_large_selection_rows() {
        use layrs_core::projection::{EntryState, ProjectionEntry, ProjectionSelection};
        let mut snapshot = Fixture::new("G0", false).source.initial().clone();
        let paths: Vec<_> = (0..4096)
            .map(|n| RepoPath::parse(format!("absent/{n:04}-{}", "a".repeat(180))).unwrap())
            .collect();
        snapshot.entries = paths
            .iter()
            .map(|path| ProjectionEntry {
                path: path.clone(),
                target: None,
                executable: false,
                writable: true,
                state: EntryState::Absent,
            })
            .collect();
        let selection =
            ProjectionSelection::selected(paths, vec![], snapshot.path_semantics).unwrap();
        let source = crate::projection::provider::records::ProviderBootstrapSource::validate(
            snapshot,
            selection,
            vec![],
        )
        .unwrap();
        assert!(
            source.descriptors().is_empty(),
            "this large selection has no file/image manifest charge"
        );
        assert_eq!(
            content_mapping_metadata(&[], &Cancellation::default()).unwrap(),
            0
        );
        let projected = managed_plan::selection_rows_metadata(source.selection()).unwrap();
        assert!(
            projected > 1024 * 1024,
            "an empty image cannot substitute for both selection payloads"
        );
        assert!(
            projected > 512 * 1024,
            "the old small metadata headroom cannot cover this transition"
        );
    }

    #[test]
    fn content_projection_counts_unique_file_ids_and_enforces_metadata_cap() {
        let current = manifest(ChunkingDescriptor::deterministic_fixed(1048576).unwrap());
        let historical = manifest(ChunkingDescriptor::single());
        let another = FileManifest::from_chunks(
            ChunkingDescriptor::single(),
            &[Chunk::new(b"another file".to_vec()).unwrap()],
        )
        .unwrap();
        let cancel = Cancellation::default();
        let one = content_mapping_metadata(&[current.clone()], &cancel).unwrap();
        let shared = content_mapping_metadata(&[current, historical], &cancel).unwrap();
        assert_eq!(
            one, shared,
            "multiple representations share one private FileId PK row"
        );
        let other = content_mapping_metadata(&[another], &cancel).unwrap();
        assert!(other > 0);
        assert!(add_metadata(1073741824 - shared, shared).is_ok());
        assert!(add_metadata(1073741824 - shared + 1, shared).is_err());
        assert!(add_metadata(u64::MAX, other).is_err());
    }
}
