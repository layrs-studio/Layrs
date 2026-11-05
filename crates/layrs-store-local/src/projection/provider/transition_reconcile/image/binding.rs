use super::*;
use crate::projection::provider::{
    bootstrap::{
        recovery_image_digest, ProviderRecoveryExportClass, ProviderRecoveryExportDeletion,
        ProviderRecoveryExportFile,
    },
    records::{codec, import::ImportSource},
};

/// Pure comparison of metadata obtained from the retained native root. Calling
/// this helper with arbitrary DATA does not construct a checkpoint or a proof.
pub(super) fn require_owner(
    source: &ImportSource,
    control_identity_hex: &str,
    filesystem_identity: &str,
    format_bytes: &[u8],
) -> Result<()> {
    let ImportSource::Provider {
        identity,
        format_digest,
        binding,
        ..
    } = source
    else {
        return Err(invalid(
            "reconcile native checkpoint requires provider source",
        ));
    };
    let prepared = binding
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("reconcile provider owner has no prepared binding"))?;
    if format_bytes.len() > layrs_vfs::CONTROL_DIRECTORY_MAX_FORMAT_BYTES
        || prepared.control_identity_hex != control_identity_hex
        || binding.recovery.checkout_identity != filesystem_identity
        || codec::hash("layrs-projection-provider-import-format-v1", format_bytes) != *format_digest
        || codec::hash("layrs-projection-recovery-marker-v1", format_bytes)
            != binding.recovery.marker_digest
    {
        return Err(invalid(
            "reconcile checkpoint owner differs from source prefix",
        ));
    }
    let marker = crate::projection::format::parse_marker_bytes(format_bytes)?;
    if &marker.identity != identity || marker.checkout_identity() != filesystem_identity {
        return Err(invalid("reconcile checkpoint FORMAT identity differs"));
    }
    Ok(())
}

/// Uses the existing producer's image encoder. Recovery classification and
/// chunking are deliberately outside that format; exact manifests remain in
/// entries and the reconciliation result. No synthetic absence belongs here.
pub(super) fn require_image(
    expected: &str,
    entries: &BTreeMap<RepoPath, ImageEntryData<'_>>,
    deleted_bases: &BTreeMap<RepoPath, &FileId>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    path_budget(entries.keys().map(|p| p.as_str()))?;
    let mut files = Vec::new();
    let mut deletions = Vec::new();
    for (path, entry) in entries {
        cancellation::check(cancel)?;
        match &entry.file {
            Some(file) => {
                if deleted_bases.contains_key(path) {
                    return Err(invalid("reconcile image file is also deleted"));
                }
                files.push(ProviderRecoveryExportFile {
                    path: path.clone(),
                    file_id: file.manifest.file_id().clone(),
                    length: file.manifest.byte_length(),
                    executable: Some(file.executable),
                    // Class is not serialized by the image commitment encoder.
                    class: ProviderRecoveryExportClass::ManagedCandidate,
                });
            }
            None => {
                let base = deleted_bases
                    .get(path)
                    .ok_or_else(|| invalid("reconcile observed deletion has no catalogue base"))?;
                deletions.push(ProviderRecoveryExportDeletion {
                    path: path.clone(),
                    base: (*base).clone(),
                    expected_executable: None,
                    class: ProviderRecoveryExportClass::ManagedCandidate,
                });
            }
        }
    }
    if deletions.len() != deleted_bases.len() {
        return Err(invalid("reconcile image has an extra deletion base"));
    }
    let actual = recovery_image_digest(&files, &deletions, &[])?;
    cancellation::check(cancel)?;
    if actual.strip_prefix("blake3:") != Some(expected) {
        return Err(invalid(
            "reconcile checkpoint image differs from source prefix",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
