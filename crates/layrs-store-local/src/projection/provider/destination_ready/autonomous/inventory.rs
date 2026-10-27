use super::super::super::checkout_journal::CheckoutFile;
use super::*;
use serde::Serialize;

pub(super) fn require_union(
    source: &ProviderBootstrapSource,
    copied: &BTreeMap<FileId, (FileManifestId, u64)>,
    expected: &ExpectedCheckout,
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
pub(super) fn require_image(
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
