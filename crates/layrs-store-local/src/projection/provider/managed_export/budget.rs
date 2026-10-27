//! Pure preallocation sizing DATA. This funds the complete future private9 and
//! family-retention envelope without constructing a provisional physical proof,
//! Ready record or receipt. Actual Staging/Ready readers must still rederive
//! their exact floors and complete plan under the real source/destination pins.
use crate::projection::{
    capture::CapturedTarget,
    provider::{
        bootstrap::{ImageDeletion, ImageFile, ManagedPlanData},
        managed_budget::{self, AdmissionBudget, IntentSize},
        records::{
            codec,
            import::{ImportSource, ProviderImportKind, ProviderTransitionReceiptRecord},
            transition::ProviderTransitionIntentRecord,
            Record,
        },
        recovery::error::{invalid, Result},
    },
    shared::exact_content::canonical_manifest_bytes,
};
use layrs_core::{
    projection::{PathSemantics, ProjectionSelection, ReservedPaths},
    scm::{FileManifest, TreeEntryTarget},
};
use layrs_vfs::Cancellation;
use std::collections::{BTreeMap, BTreeSet};

const MAX_FILES: usize = 100_000;
const MAX_DIRECTORIES: usize = 100_000;
const MAX_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIRECTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROOT_BYTES: u64 = 16 * 1024 * 1024;
// The same bounded root frame used by ExactProtection::new before appending
// canonical manifests. It covers all scalar fields and JSON escaping.
const ROOT_FRAME: u64 = 65_536;
const TRANSACTION_BYTES: u64 = 32;
const RESERVATION_BYTES: u64 = 84; // provider-transition: + 64 lowercase hex.
const DIGEST_BYTES: u64 = 71;

/// A logical funding calculation only. Retaining this does not retain a SQL
/// transaction or set ManagedPlanData::validated_metadata_floor.
pub(super) struct PlanningEnvelope {
    metadata: u64,
    base: u64,
    ready: u64,
    admission: AdmissionBudget,
    root_payload: u64,
}
impl PlanningEnvelope {
    pub(super) fn metadata_bytes(&self) -> u64 {
        self.metadata
    }
    pub(super) fn base_metadata_bound(&self) -> u64 {
        self.base
    }
    pub(super) fn ready_metadata_bound(&self) -> u64 {
        self.ready
    }
    pub(super) fn admission_budget(&self) -> &AdmissionBudget {
        &self.admission
    }
    pub(super) fn root_payload_bound(&self) -> u64 {
        self.root_payload
    }
}

/// Call with a genuine owner-rederived M=0 baseline, exact complete canonical
/// union, selected image and copied SQL bytes, BEFORE persisting SourcePlanned.
/// The caller derives the same complete plan again with the returned M. Codec
/// payload maxima make this independent of quota-digit serialization growth.
pub(super) fn planning_envelope(
    baseline: &ManagedPlanData,
    selected: &ProjectionSelection,
    required: &[FileManifest],
    files: &[ImageFile<'_>],
    deletions: &[ImageDeletion<'_>],
    copied_bytes: u64,
    cancel: &Cancellation,
) -> Result<PlanningEnvelope> {
    cancel.check()?;
    require_baseline(baseline, selected, required, files, deletions, cancel)?;
    let inventory = inventory(files, deletions, cancel)?;
    let payload = ProviderTransitionIntentRecord::MAX_BYTES as u64;
    // Source and destination Staging have fixed 32-byte IDs and 84-byte
    // reservation IDs. No receipt is present. Role lengths differ by five.
    let scalar = TRANSACTION_BYTES + 7 + 8 + RESERVATION_BYTES + DIGEST_BYTES;
    let base = managed_budget::base_metadata_floor(
        copied_bytes,
        &baseline.observation,
        &baseline.placement,
        &baseline.capture,
        &baseline.plan,
        selected,
        IntentSize {
            payload,
            scalars: scalar + 6,
        },
        IntentSize {
            payload,
            scalars: scalar + 11,
        },
    )?;
    let ready = complete_ready_bound(base, required, cancel)?;
    let admission =
        managed_budget::admission_budget(ready, inventory.files, inventory.directories)?;
    let root_payload = root_payload_bound(required, cancel)?;
    // Keep family root/relations/frame capacity in addition to the complete
    // private M. These are precisely AdmissionBudget::require_remaining and
    // ExactProtection::require_envelope's separate retained overheads. Funding
    // them in M is conservative; later actual debit/readers still decide usage.
    let margin = (required.len() as u64)
        .checked_mul(256)
        .and_then(|bytes| bytes.checked_add(ROOT_FRAME))
        .and_then(|bytes| bytes.checked_add(root_payload))
        .ok_or_else(|| invalid("ordinary family metadata margin overflow"))?;
    let metadata = managed_budget::add_metadata(admission.minimum_metadata(), margin)?;
    admission.require_metadata(metadata)?;
    cancel.check()?;
    Ok(PlanningEnvelope {
        metadata,
        base,
        ready,
        admission,
        root_payload,
    })
}

/// Upper bound on every addition in managed_budget::complete_metadata_floor.
/// ManagedOrdinary has no companion. Its one claim has a raw 64-byte root ID
/// and digest, literal kind and transaction owner. Only the Ready/receipt BLOB
/// lengths are unknown, so use their codec MAX_BYTES rather than fake records.
fn complete_ready_bound(
    base: u64,
    required: &[FileManifest],
    cancel: &Cancellation,
) -> Result<u64> {
    cancel.check()?;
    let additions = [
        ProviderTransitionReceiptRecord::MAX_BYTES as u64,
        DIGEST_BYTES, // Receipt digest SQL column.
        DIGEST_BYTES, // Receipt owner identity digest.
        TRANSACTION_BYTES,
        0,  // No companion ID.
        64, // Claim root ID.
        64, // Claim root digest.
        "provider-import".len() as u64,
        TRANSACTION_BYTES, // Claim owner.
        ProviderTransitionIntentRecord::MAX_BYTES as u64,
        TRANSACTION_BYTES,
        11, // Destination role.
        5,  // Ready state.
        8,  // Local generation.
        RESERVATION_BYTES,
        DIGEST_BYTES, // Receipt ID.
        DIGEST_BYTES, // Ready digest.
        managed_budget::content_mapping_metadata(required, Some(cancel))?,
    ];
    additions
        .into_iter()
        .try_fold(base, managed_budget::add_metadata)
}

fn root_payload_bound(required: &[FileManifest], cancel: &Cancellation) -> Result<u64> {
    cancel.check()?;
    let mut frame = ROOT_FRAME;
    for manifest in required {
        cancel.check()?;
        frame = frame
            .checked_add(canonical_manifest_bytes(manifest)?.len() as u64)
            .and_then(|bytes| bytes.checked_add(1))
            .filter(|bytes| *bytes <= MAX_ROOT_BYTES)
            .ok_or_else(|| invalid("ordinary protected union exceeds inline root bound"))?;
    }
    Ok(frame)
}

fn require_baseline(
    baseline: &ManagedPlanData,
    selected: &ProjectionSelection,
    required: &[FileManifest],
    files: &[ImageFile<'_>],
    deletions: &[ImageDeletion<'_>],
    cancel: &Cancellation,
) -> Result<()> {
    codec::transaction(&baseline.transaction_id)?;
    baseline.plan.digest()?;
    if !matches!(
        &baseline.plan.source_prefix.source,
        ImportSource::Ordinary {
            private_schema: 9,
            admission: Some(_),
            ..
        }
    ) || baseline.plan.kind != ProviderImportKind::ManagedOrdinary
        || !baseline.plan.source_prefix.supports_restore_history()
        || baseline.plan.companion_id.is_some()
        || baseline.plan.companion_files.count != 0
        || baseline.plan.companion_deletions.count != 0
        || required.len() > MAX_FILES
        || required.len() != baseline.required_manifests.len()
        || required.len() as u64 != baseline.plan.protected_manifests.count
    {
        return Err(invalid("ordinary planning baseline kind/union differs"));
    }
    selected
        .validate_snapshot(&baseline.placement)
        .map_err(|e| invalid(e.to_string()))?;
    let mut actual = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    for manifest in required {
        cancel.check()?;
        if actual
            .insert((manifest.file_id(), manifest.id()), manifest.byte_length())
            .is_some()
            || lengths
                .insert(manifest.file_id(), manifest.byte_length())
                .is_some_and(|prior| prior != manifest.byte_length())
        {
            return Err(invalid(
                "ordinary planning union repeats or contradicts representation",
            ));
        }
    }
    let expected: BTreeMap<_, _> = baseline
        .required_manifests
        .iter()
        .map(|row| ((&row.file_id, &row.manifest_id), row.bytes))
        .collect();
    if expected.len() != baseline.required_manifests.len() || actual != expected {
        return Err(invalid(
            "ordinary planning union differs from complete baseline",
        ));
    }
    // M must be zero in this baseline. Otherwise adding the new envelope could
    // silently fund a different quota or count an older metadata envelope twice.
    let q0 = managed_budget::quota(
        required,
        files.iter().map(|file| file.manifest.byte_length()),
        0,
    )?;
    if baseline.quota_bytes != q0 || baseline.plan.quota_bytes != q0 {
        return Err(invalid(
            "ordinary planning baseline already includes metadata",
        ));
    }
    require_complete_image(baseline, selected, &actual, files, deletions, cancel)
}

fn require_complete_image(
    baseline: &ManagedPlanData,
    selected: &ProjectionSelection,
    required: &BTreeMap<(&layrs_core::scm::FileId, &layrs_core::scm::FileManifestId), u64>,
    files: &[ImageFile<'_>],
    deletions: &[ImageDeletion<'_>],
    cancel: &Cancellation,
) -> Result<()> {
    let mut expected = BTreeMap::new();
    for entry in &baseline.placement.entries {
        cancel.check()?;
        if !selected.includes(&entry.path) {
            continue;
        }
        if let Some(TreeEntryTarget::File { file_id }) = &entry.target {
            expected.insert(
                entry.path.clone(),
                CapturedTarget {
                    file_id: file_id.clone(),
                    executable: entry.executable,
                },
            );
        }
    }
    let mut deleted = BTreeMap::new();
    for change in &baseline.capture.changes {
        cancel.check()?;
        if !selected.includes(&change.path) {
            return Err(invalid("ordinary planned image change escaped selection"));
        }
        if let Some(file) = &change.replacement {
            expected.insert(
                change.path.clone(),
                CapturedTarget {
                    file_id: file.file_id.clone(),
                    executable: file.executable,
                },
            );
        } else {
            expected.remove(&change.path);
            let before = change
                .expected
                .as_ref()
                .ok_or_else(|| invalid("ordinary planned deletion has no base"))?;
            if deleted.insert(change.path.clone(), before).is_some() {
                return Err(invalid("ordinary planned image repeats deletion"));
            }
        }
    }
    for file in files {
        cancel.check()?;
        let target = expected
            .remove(file.path)
            .ok_or_else(|| invalid("ordinary image file is extra or duplicated"))?;
        if target.file_id != *file.manifest.file_id()
            || target.executable != file.executable
            || required.get(&(file.manifest.file_id(), file.manifest.id()))
                != Some(&file.manifest.byte_length())
        {
            return Err(invalid(
                "ordinary image differs from complete planned result",
            ));
        }
    }
    for deletion in deletions {
        cancel.check()?;
        let before = deleted
            .remove(deletion.path)
            .ok_or_else(|| invalid("ordinary image deletion is extra or duplicated"))?;
        if &before.file_id != deletion.base || before.executable != deletion.executable {
            return Err(invalid("ordinary image deletion base differs"));
        }
    }
    if !expected.is_empty() || !deleted.is_empty() {
        return Err(invalid("ordinary image omits planned files or deletions"));
    }
    Ok(())
}

struct InventoryCounts {
    files: u64,
    directories: u64,
}

/// Same namespace/count rules as checkout_journal::inventory::expected_parts.
/// Only file parents create directories; deletion-only parents do not. This
/// projection sizes archives before a physical destination proof exists.
fn inventory(
    files: &[ImageFile<'_>],
    deletions: &[ImageDeletion<'_>],
    cancel: &Cancellation,
) -> Result<InventoryCounts> {
    cancel.check()?;
    if files.len() > MAX_FILES
        || deletions.len() > MAX_FILES
        || files
            .len()
            .checked_add(deletions.len())
            .is_none_or(|count| count > MAX_FILES)
    {
        return Err(invalid("ordinary checkout inventory count exceeds bound"));
    }
    let reserved = ReservedPaths::new(std::iter::empty::<&str>(), PathSemantics::PortableWindows)
        .map_err(|e| invalid(e.to_string()))?;
    let mut paths = 0usize;
    let mut total = 0u64;
    let mut leaves = BTreeSet::new();
    for file in files {
        cancel.check()?;
        if file.executable || file.manifest.byte_length() > 1073741824 {
            return Err(invalid("ordinary checkout cannot represent file mode/size"));
        }
        total = total
            .checked_add(file.manifest.byte_length())
            .filter(|bytes| *bytes <= 4 * 1073741824)
            .ok_or_else(|| invalid("ordinary checkout image exceeds 4GiB"))?;
    }
    for path in files
        .iter()
        .map(|file| file.path)
        .chain(deletions.iter().map(|file| file.path))
    {
        cancel.check()?;
        if path.as_str().len() > 4096 {
            return Err(invalid("ordinary checkout path exceeds bound"));
        }
        reserved
            .check_path(path.as_str())
            .map_err(|e| invalid(e.to_string()))?;
        paths = paths
            .checked_add(path.as_str().len())
            .filter(|bytes| *bytes <= MAX_PATH_BYTES)
            .ok_or_else(|| invalid("ordinary checkout aggregate paths exceed8MiB"))?;
        if !leaves.insert(path.as_str().to_uppercase()) {
            return Err(invalid("ordinary checkout has duplicate or aliased leaves"));
        }
    }
    for leaf in &leaves {
        cancel.check()?;
        for (end, _) in leaf.match_indices('/') {
            if leaves.contains(&leaf[..end]) {
                return Err(invalid("ordinary checkout leaf contains another leaf"));
            }
        }
    }
    let mut dirs = BTreeMap::new();
    let mut bytes = 0usize;
    for file in files {
        cancel.check()?;
        for (end, _) in file.path.as_str().match_indices('/') {
            let original = &file.path.as_str()[..end];
            let key = original.to_uppercase();
            if let Some(prior) = dirs.get(&key) {
                if *prior != original {
                    return Err(invalid(
                        "ordinary checkout aliases parent directory spelling",
                    ));
                }
                continue;
            }
            if dirs.len() >= MAX_DIRECTORIES {
                return Err(invalid("ordinary checkout directory count exceeds bound"));
            }
            bytes = bytes
                .checked_add(original.len())
                .filter(|bytes| *bytes <= MAX_DIRECTORY_BYTES)
                .ok_or_else(|| invalid("ordinary checkout directory metadata exceeds64MiB"))?;
            dirs.insert(key, original);
        }
    }
    Ok(InventoryCounts {
        files: files.len() as u64,
        directories: dirs.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{Chunk, ChunkingDescriptor, FileId, RepoPath};

    fn manifest(bytes: &[u8]) -> FileManifest {
        FileManifest::from_chunks(
            ChunkingDescriptor::single(),
            &[Chunk::new(bytes.to_vec()).unwrap()],
        )
        .unwrap()
    }

    fn file<'a>(path: &'a RepoPath, manifest: &'a FileManifest) -> ImageFile<'a> {
        ImageFile {
            path,
            manifest,
            executable: false,
            expected: None,
        }
    }

    #[test]
    fn admission_counts_shared_file_parents_but_not_deletion_parents() {
        let cancel = Cancellation::default();
        let bytes = manifest(b"image");
        let a = RepoPath::parse("dir/nested/a.txt").unwrap();
        let b = RepoPath::parse("dir/nested/b.txt").unwrap();
        let gone = RepoPath::parse("deleted/only/file.txt").unwrap();
        let base = FileId::from_bytes(b"removed");
        let counts = inventory(
            &[file(&a, &bytes), file(&b, &bytes)],
            &[ImageDeletion {
                path: &gone,
                base: &base,
                executable: false,
            }],
            &cancel,
        )
        .unwrap();
        assert_eq!((counts.files, counts.directories), (2, 2));
        let admission =
            managed_budget::admission_budget(0, counts.files, counts.directories).unwrap();
        assert_eq!(admission.archive_rows(), 6);
    }

    #[test]
    fn checkout_funding_refuses_windows_leaf_parent_and_reserved_aliases() {
        let cancel = Cancellation::default();
        let bytes = manifest(b"image");
        for (first, second) in [
            ("dir/a.txt", "DIR/A.txt"),
            ("dir", "dir/a.txt"),
            ("Dir/a.txt", "dir/b.txt"),
        ] {
            let a = RepoPath::parse(first).unwrap();
            let b = RepoPath::parse(second).unwrap();
            assert!(inventory(&[file(&a, &bytes), file(&b, &bytes)], &[], &cancel).is_err());
        }
        let reserved = RepoPath::parse(".layrs/metadata").unwrap();
        assert!(inventory(&[file(&reserved, &bytes)], &[], &cancel).is_err());
    }

    #[test]
    fn complete_ready_bound_counts_one_mapping_for_distinct_representations() {
        let cancel = Cancellation::default();
        let one = manifest(b"same file");
        let two = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(1048576).unwrap(),
            &[Chunk::new(b"same file".to_vec()).unwrap()],
        )
        .unwrap();
        assert_eq!(one.file_id(), two.file_id());
        assert_ne!(one.id(), two.id());
        let single = complete_ready_bound(0, &[one.clone()], &cancel).unwrap();
        let distinct = complete_ready_bound(0, &[one.clone(), two.clone()], &cancel).unwrap();
        assert_eq!(
            distinct, single,
            "Ready writes one private mapping per FileId"
        );
        let root = root_payload_bound(&[one.clone(), two.clone()], &cancel).unwrap();
        assert_eq!(
            root,
            ROOT_FRAME
                + canonical_manifest_bytes(&one).unwrap().len() as u64
                + canonical_manifest_bytes(&two).unwrap().len() as u64
                + 2
        );
        assert!(
            root > root_payload_bound(&[one], &cancel).unwrap(),
            "family retention keeps every exact representation"
        );
    }

    #[test]
    fn full_ready_envelope_refuses_overflow_and_preserves_cancellation() {
        let cancel = Cancellation::default();
        assert!(complete_ready_bound(1073741824, &[], &cancel).is_err());
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(complete_ready_bound(0, &[], &cancel).is_err());
        assert!(root_payload_bound(&[manifest(b"image")], &cancel).is_err());
    }
}
