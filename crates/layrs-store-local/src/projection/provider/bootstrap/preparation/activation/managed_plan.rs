//! Deterministic transition DATA. The caller retains source SQL, checkpoint,
//! native and shared-content proofs through the eventual destination writer.
use super::managed_image::VerifiedManagedImage;
use crate::projection::{
    capture::{
        CapturedChange, CapturedFile, CapturedPatch, CapturedTarget, MAX_CAPTURE_BYTES,
        MAX_CAPTURE_FILE_BYTES,
    },
    database, format,
    provider::{
        import_validation::VerifiedImportClosure,
        records::{
            activation::activation_snapshot,
            import::{ImportSource, ProviderImportKind, ProviderImportPrefixRecord},
            transition::{TransitionObservation, TransitionPlan, TransitionRowset},
            ProviderBootstrapSource, Record,
        },
        recovery::error::{invalid, Result},
        validation::PrivateActiveCaptureValidation,
    },
    ProjectionIdentity,
};
use layrs_core::{
    projection::{EntryState, ProjectionSnapshot},
    scm::{canonical_json, FileId, FileManifest, FileManifestId, RepoPath, TreeEntryTarget},
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
pub(super) struct RequiredManifest {
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
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

pub(super) struct ImageFile<'a> {
    pub path: &'a RepoPath,
    pub manifest: &'a FileManifest,
    pub executable: bool,
    pub expected: Option<(FileId, bool)>,
}
pub(super) struct ImageDeletion<'a> {
    pub path: &'a RepoPath,
    pub base: &'a FileId,
    pub executable: bool,
}

/// Factual upper budget from this exact image and manifest set. It is not a
/// reservation, capacity proof, or permission to mark a destination Ready.
pub(super) struct ManagedPlanData {
    pub observation: ProjectionSnapshot,
    pub observation_digest: String,
    pub placement: ProjectionSnapshot,
    pub placement_digest: String,
    pub capture: CapturedPatch,
    pub plan: TransitionPlan,
    pub required_manifests: Vec<RequiredManifest>,
    pub quota_bytes: u64,
    pub transaction_id: String,
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
    closure.require_provider_inputs(provider_proof)?;
    let (_, _, source, activated_version, captures) = provider_proof.import_parts();
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
pub(super) fn require_manifest_union(
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
        || fresh != source.initial()
        || activated != &activation_snapshot(source)?
        || version.effective_image_digest.len() != 64
        || prefix.closure.companions.count != 0
        || prefix.closure.companion_files.count != 0
        || prefix.closure.companion_deletions.count != 0
        || transaction_id.is_empty()
        || transaction_id.len() > 128
        || transaction_id.contains('\0')
    {
        return Err(invalid(
            "managed plan fresh observation or identity differs",
        ));
    }
    source
        .selection()
        .validate_snapshot(fresh)
        .map_err(|e| invalid(e.to_string()))?;
    if version.selection_digest != source.commitment().selection_digest
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
        if !source.selection().includes(&entry.path) {
            if file.is_some() || deletion.is_some() {
                return Err(invalid("managed image includes excluded path"));
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
    for descriptor in source.descriptors() {
        insert_required(
            &mut required,
            &mut lengths,
            &mut manifest_owners,
            RequiredManifest::from_manifest(&descriptor.manifest),
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
    let observation_record = TransitionObservation {
        step_id: observation.base_step_id.clone(),
        root_tree_id: observation.base_root_tree_id.clone(),
        policy_set_id: observation.observed_policy_set_id.clone(),
        authorization_epoch: observation.observed_authorization_epoch,
        generation: observation_generation,
        snapshot_digest: observation_digest.clone(),
        selection_digest: version.selection_digest.clone(),
    };
    let plan = TransitionPlan {
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
        companion_id: None,
        companion_files: empty.clone(),
        companion_deletions: empty,
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
    if metadata > MAX_METADATA_BYTES {
        return Err(invalid("managed metadata budget exceeds bound"));
    }
    let mut total = metadata
        .checked_add(65536)
        .ok_or_else(|| invalid("managed quota overflow"))?;
    for manifest in manifests {
        let payload = canonical_json(manifest)
            .map_err(|e| invalid(e.to_string()))?
            .len() as u64;
        let estimate = (manifest.chunks().len() as u64)
            .checked_mul(256)
            .and_then(|n| n.checked_add(2048))
            .ok_or_else(|| invalid("managed quota overflow"))?;
        let overhead = payload.max(estimate);
        total = total
            .checked_add(manifest.byte_length())
            .and_then(|n| n.checked_add(overhead))
            .ok_or_else(|| invalid("managed quota overflow"))?;
    }
    for file in image {
        total = total
            .checked_add(file.manifest.byte_length())
            .ok_or_else(|| invalid("managed quota overflow"))?;
    }
    if total > i64::MAX as u64 {
        return Err(invalid("managed quota exceeds SQLite range"));
    }
    Ok(total)
}

#[cfg(test)]
#[path = "managed_plan/tests.rs"]
mod tests;
