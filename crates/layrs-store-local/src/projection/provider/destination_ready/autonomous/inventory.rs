use super::super::super::checkout_journal::CheckoutFile;
use super::*;
use crate::projection::provider::{
    import_validation::OrdinaryFrozenBasis, managed_admission::RequiredRepresentation,
};
use serde::Serialize;

/// Full committed ordinary union. Exact historical/checkout mappings are
/// subsets; selected bases require FileId coverage, never an invented old B
/// ManifestId. Every representation of one FileId must have one length.
pub(in crate::projection::provider::destination_ready) fn require_ordinary_union(
    source: &OrdinaryFrozenBasis,
    copied: &BTreeMap<FileId, (FileManifestId, u64)>,
    expected: &ExpectedCheckout,
    inherited: &[RequiredRepresentation],
    required: &[RequiredRepresentation],
    cancel: Option<&Cancellation>,
) -> Result<BTreeMap<FileId, u64>> {
    if required.len() > 100_000 {
        return Err(invalid("ordinary frozen required union exceeds bound"));
    }
    let mut allowed = BTreeSet::new();
    for entry in &source.snapshot().entries {
        cancellation::check(cancel)?;
        if source.selection().includes(&entry.path) {
            if let Some(layrs_core::scm::TreeEntryTarget::File { file_id }) = &entry.target {
                allowed.insert(file_id.clone());
            }
        }
    }
    let selected = allowed.clone();
    allowed.extend(copied.keys().cloned());
    allowed.extend(inherited.iter().map(|row| row.file_id.clone()));
    allowed.extend(expected.files().iter().map(|file| file.file_id.clone()));
    let mut exact = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut prior = None;
    for row in required {
        cancellation::check(cancel)?;
        let key = (&row.file_id, &row.manifest_id);
        if row.bytes > 1073741824
            || !allowed.contains(&row.file_id)
            || prior.is_some_and(|old| old >= key)
            || lengths
                .insert(row.file_id.clone(), row.bytes)
                .is_some_and(|old| old != row.bytes)
            || exact.insert(key, row.bytes).is_some()
        {
            return Err(invalid(
                "ordinary frozen union order, coverage or length differs",
            ));
        }
        prior = Some(key);
    }
    if selected.iter().any(|file| !lengths.contains_key(file)) {
        return Err(invalid("ordinary frozen selected base FileId absent"));
    }
    for (file, (manifest, bytes)) in copied {
        cancellation::check(cancel)?;
        if exact.get(&(file, manifest)) != Some(bytes) {
            return Err(invalid(
                "ordinary frozen historical exact representation absent",
            ));
        }
    }
    for row in inherited {
        cancellation::check(cancel)?;
        if exact.get(&(&row.file_id, &row.manifest_id)) != Some(&row.bytes) {
            return Err(invalid(
                "ordinary frozen inherited companion representation absent",
            ));
        }
    }
    for file in expected.files() {
        cancellation::check(cancel)?;
        if exact.get(&(&file.file_id, &file.manifest_id)) != Some(&file.bytes) {
            return Err(invalid(
                "ordinary frozen checkout exact representation absent",
            ));
        }
    }
    Ok(lengths)
}

pub(super) fn require_union(
    source: &ProviderBootstrapSource,
    copied: &BTreeMap<FileId, (FileManifestId, u64)>,
    expected: &ExpectedCheckout,
    inherited: &[RequiredRepresentation],
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let mut union = BTreeMap::new();
    for descriptor in source.descriptors() {
        cancellation::check(cancel)?;
        union.insert(
            (descriptor.file_id.clone(), descriptor.manifest_id.clone()),
            descriptor.bytes,
        );
    }
    for (file, (manifest, bytes)) in copied {
        cancellation::check(cancel)?;
        if union
            .insert((file.clone(), manifest.clone()), *bytes)
            .is_some_and(|old| old != *bytes)
        {
            return Err(invalid("autonomous Ready historical length differs"));
        }
    }
    for file in expected.files() {
        cancellation::check(cancel)?;
        if union
            .insert((file.file_id.clone(), file.manifest_id.clone()), file.bytes)
            .is_some_and(|old| old != file.bytes)
        {
            return Err(invalid("autonomous Ready current length differs"));
        }
    }
    for row in inherited {
        cancellation::check(cancel)?;
        if union
            .insert((row.file_id.clone(), row.manifest_id.clone()), row.bytes)
            .is_some_and(|old| old != row.bytes)
        {
            return Err(invalid(
                "autonomous Ready inherited companion length differs",
            ));
        }
    }
    if required.len() != union.len() {
        return Err(invalid("autonomous Ready exact union coverage differs"));
    }
    let mut found = BTreeSet::new();
    for manifest in required {
        cancellation::check(cancel)?;
        let key = (manifest.file_id().clone(), manifest.id().clone());
        if !found.insert(key.clone()) || union.get(&key) != Some(&manifest.byte_length()) {
            return Err(invalid("autonomous Ready exact union differs"));
        }
    }
    Ok(())
}

pub(super) fn overlay(
    capture: &CapturedPatch,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<Vec<ManagedReadyOverlay>> {
    let mut result = Vec::with_capacity(capture.changes.len());
    for change in &capture.changes {
        cancellation::check(cancel)?;
        let manifest = match &change.replacement {
            Some(replacement) => {
                let index = expected
                    .files()
                    .binary_search_by(|file| file.path.cmp(&change.path))
                    .map_err(|_| invalid("autonomous Ready overlay path absent"))?;
                let file = &expected.files()[index];
                if file.file_id != replacement.file_id
                    || file.bytes != replacement.bytes
                    || file.executable != replacement.executable
                {
                    return Err(invalid("autonomous Ready overlay file differs"));
                }
                Some(file.manifest_id.clone())
            }
            None => None,
        };
        result.push(ManagedReadyOverlay {
            path: change.path.clone(),
            manifest_id: manifest,
        });
    }
    Ok(result)
}

/// Same canonical recovery image commitment as the producer (class and exact
/// chunking are intentionally excluded by that format). Journal/overlay above
/// separately retain the current exact representations.
pub(in crate::projection::provider::destination_ready) fn require_image(
    capture: &CapturedPatch,
    expected: &ExpectedCheckout,
    image: &str,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    if image_digest(capture, expected.files(), expected.deletions(), cancel)? != image {
        return Err(invalid("autonomous Ready image commitment differs"));
    }
    Ok(())
}

pub(super) fn image_digest(
    capture: &CapturedPatch,
    files: &[CheckoutFile],
    deletions: &[RepoPath],
    cancel: Option<&Cancellation>,
) -> Result<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    enum Item<'a> {
        File {
            file_id: &'a FileId,
            length: u64,
            executable: Option<bool>,
        },
        Deleted {
            base: &'a FileId,
        },
    }
    let mut map = BTreeMap::new();
    for CheckoutFile {
        path,
        file_id,
        bytes,
        executable,
        ..
    } in files
    {
        cancellation::check(cancel)?;
        map.insert(
            path,
            Item::File {
                file_id,
                length: *bytes,
                executable: Some(*executable),
            },
        );
    }
    for path in deletions {
        cancellation::check(cancel)?;
        let change = capture
            .changes
            .binary_search_by(|change| change.path.cmp(path))
            .ok()
            .and_then(|n| capture.changes.get(n))
            .ok_or_else(|| invalid("autonomous Ready deletion absent"))?;
        let base = &change
            .expected
            .as_ref()
            .ok_or_else(|| invalid("autonomous Ready deletion preimage absent"))?
            .file_id;
        if map.insert(path, Item::Deleted { base }).is_some() {
            return Err(invalid("autonomous Ready image path repeated"));
        }
    }
    let payload = canonical_json(&map).map_err(|e| invalid(e.to_string()))?;
    if payload.len() > 32 * 1024 * 1024 {
        return Err(invalid("autonomous Ready image document exceeds bound"));
    }
    let token =
        super::super::super::records::codec::hash("layrs-provider-recovery-image-v1", &payload);
    Ok(token
        .strip_prefix("blake3:")
        .ok_or_else(|| invalid("autonomous Ready image digest shape differs"))?
        .to_owned())
}
