//! Read-only CAS verification after a separately validated complete Published
//! SQL snapshot. No staging adoption, journal mutation, creation or SQL here.
use super::*;
use crate::projection::shared::exact_protection::ExactProtection;
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
};

/// Ephemeral proof of the exact bytes read in this retained physical family.
/// It is not proof of Published SQL, protection, checkout or ordinary admission.
pub(in crate::projection) struct PublishedCasProof {
    binding: ExactBinding,
    union_digest: String,
    storage: ExactStorage,
    // The first key component distinguishes manifests from chunks. Each
    // ordinary Windows handle denies write/delete sharing until proof drop.
    // This is not exclusion of pre-existing writable memory mappings or
    // privileged raw-volume mutation, neither of which this protocol admits.
    objects: BTreeMap<(bool, String), PinnedFile>,
}
impl PublishedCasProof {
    pub(in crate::projection) fn verify_identity(&self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.storage.verify()?;
        if self.storage.root.identity_token() != self.binding.family_root_identity {
            return Err(invalid("Published CAS proof physical family differs"));
        }
        for ((manifest, name), pin) in &self.objects {
            cancel.check()?;
            let parent = if *manifest {
                &self.storage.manifests
            } else {
                &self.storage.chunks
            };
            require_same_file(parent, name, pin)?;
        }
        cancel.check()?;
        Ok(())
    }

    /// No SQL and no byte read. Identity checks retain the lifecycle and pins;
    /// callers must not cache this proof across unrelated filesystem effects.
    pub(in crate::projection) fn require(
        &self,
        binding: &ExactBinding,
        required: &[FileManifest],
        cancel: &Cancellation,
    ) -> Result<()> {
        self.verify_identity(cancel)?;
        let expected = protection(binding, required, cancel)?;
        if self.binding != *binding || self.union_digest != expected.digest() {
            return Err(invalid(
                "Published CAS proof binding or complete union differs",
            ));
        }
        Ok(())
    }
}

fn protection(
    binding: &ExactBinding,
    required: &[FileManifest],
    cancel: &Cancellation,
) -> Result<ExactProtection> {
    binding.validate()?;
    ExactProtection::new(
        &binding.family_incarnation,
        &binding.owner,
        &binding.database_identity,
        &binding.transaction_id,
        required,
        Some(cancel),
    )
}

/// The caller supplies manifests authenticated by its complete Published SQL
/// snapshot. Every exact representation is verified independently, even when
/// another representation has the same FileId or a chunk occurs twice.
pub(in crate::projection) fn verify_published_cas(
    storage: ExactStorage,
    binding: &ExactBinding,
    required: &[FileManifest],
    cancel: &Cancellation,
) -> Result<PublishedCasProof> {
    let expected = protection(binding, required, cancel)?;
    let mut proof = PublishedCasProof {
        binding: binding.clone(),
        union_digest: expected.digest().into(),
        storage,
        objects: BTreeMap::new(),
    };
    proof.verify_identity(cancel)?;
    for manifest in required {
        cancel.check()?;
        verify_manifest(&proof.storage, &mut proof.objects, manifest, cancel)?;
    }
    proof.verify_identity(cancel)?;
    Ok(proof)
}

fn require_same_file(parent: &PinnedDirectory, name: &str, pin: &PinnedFile) -> Result<()> {
    pin.verify_parent()?;
    if !pin.same_file(&parent.open_readonly_file(OsStr::new(name))?) {
        return Err(invalid("Published CAS object path identity changed"));
    }
    Ok(())
}

fn pin_object<'a>(
    parent: &PinnedDirectory,
    objects: &'a mut BTreeMap<(bool, String), PinnedFile>,
    manifest: bool,
    name: &str,
) -> Result<&'a PinnedFile> {
    let entry = objects.entry((manifest, name.to_owned()));
    Ok(match entry {
        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(parent.open_immutable_file(OsStr::new(name))?)
        }
    })
}

fn verify_manifest(
    storage: &ExactStorage,
    objects: &mut BTreeMap<(bool, String), PinnedFile>,
    manifest: &FileManifest,
    cancel: &Cancellation,
) -> Result<()> {
    let canonical = journal::manifest_bytes(manifest)?;
    let name = object_name(manifest.id().as_str())?;
    let pin = pin_object(&storage.manifests, objects, true, name)?;
    if pin.file().metadata()?.len() != canonical.len() as u64 {
        return Err(invalid("Published CAS manifest length differs"));
    }
    let mut scratch = [0u8; 64 * 1024];
    let mut reader = pin.file();
    reader.seek(SeekFrom::Start(0))?;
    for expected in canonical.chunks(scratch.len()) {
        cancel.check()?;
        reader.read_exact(&mut scratch[..expected.len()])?;
        if &scratch[..expected.len()] != expected {
            return Err(invalid("Published CAS canonical manifest bytes differ"));
        }
    }
    if reader.read(&mut [0u8; 1])? != 0 {
        return Err(invalid("Published CAS manifest grew during verification"));
    }
    require_same_file(&storage.manifests, name, &pin)?;
    let mut full = blake3::Hasher::new();
    full.update(b"layrs-file-v1\0");
    let mut offset = 0u64;
    for part in manifest.chunks() {
        cancel.check()?;
        if part.offset() != offset {
            return Err(invalid("Published CAS chunk offset differs"));
        }
        let name = object_name(part.chunk_id().as_str())?;
        let pin = pin_object(&storage.chunks, objects, false, name)?;
        if pin.file().metadata()?.len() != part.byte_length() {
            return Err(invalid("Published CAS chunk length differs"));
        }
        let mut reader = pin.file();
        reader.seek(SeekFrom::Start(0))?;
        let mut chunk = blake3::Hasher::new();
        chunk.update(b"layrs-chunk-v1\0");
        let mut remaining = part.byte_length();
        while remaining != 0 {
            cancel.check()?;
            let count = remaining.min(scratch.len() as u64) as usize;
            reader.read_exact(&mut scratch[..count])?;
            chunk.update(&scratch[..count]);
            full.update(&scratch[..count]);
            remaining -= count as u64;
            offset = offset
                .checked_add(count as u64)
                .ok_or_else(|| invalid("Published CAS length overflow"))?;
        }
        if reader.read(&mut [0u8; 1])? != 0
            || format!("blake3:{}", chunk.finalize()) != part.chunk_id().as_str()
        {
            return Err(invalid("Published CAS chunk bytes differ"));
        }
        require_same_file(&storage.chunks, name, &pin)?;
    }
    if offset != manifest.byte_length()
        || format!("blake3:{}", full.finalize()) != manifest.file_id().as_str()
    {
        return Err(invalid("Published CAS full FileId differs"));
    }
    cancel.check()?;
    Ok(())
}

#[cfg(test)]
#[path = "published_cas_tests.rs"]
mod tests;
