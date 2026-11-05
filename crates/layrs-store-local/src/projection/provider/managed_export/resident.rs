//! Exact retained CAS checks for an export-scoped ordinary owner. SQL indexing
//! and physical bytes are deliberately separate operations. Neither authenticates
//! B's private mappings, C's Staging or the complete retained/active catalog.
//! The driver must perform those checks before/after effects and positively
//! close the catalog snapshot BEFORE calling RetainedCas::verify.
use crate::{
    lock::StoreLock,
    projection::{
        provider::recovery::error::{Result as CauseResult, invalid},
        shared::{exact_content::canonical_manifest_bytes, provider_access::FamilyAccess},
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_core::scm::{ChunkId, FileId, FileManifest};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    io::{Read, Seek, SeekFrom, Write},
};

const MAX_FILES: usize = 100_000;
const MAX_MANIFEST_BYTES: usize = 32 * 1024 * 1024;
const MAX_CHUNKS: usize = 1_000_000;
const MAX_CHUNKS_PER_FILE: usize = 1024;
const MAX_CHUNK_BYTES: u64 = 1024 * 1024;
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const READ_BYTES: usize = 64 * 1024;

struct BoundedExpected<'a> {
    manifests: Vec<(&'a FileManifest, Vec<u8>)>,
    chunks: BTreeMap<ChunkId, u64>,
}

fn bounded_expected<'a>(
    expected: &'a BTreeMap<FileId, FileManifest>,
    cancel: &Cancellation,
) -> CauseResult<BoundedExpected<'a>> {
    cancel.check()?;
    if expected.len() > MAX_FILES {
        return Err(invalid("retained CAS manifest count exceeds bound"));
    }
    let mut manifests = Vec::with_capacity(expected.len());
    let mut ids = BTreeSet::new();
    let mut chunks = BTreeMap::new();
    let mut metadata = 0usize;
    for (file, manifest) in expected {
        cancel.check()?;
        if file != manifest.file_id()
            || !ids.insert(manifest.id())
            || manifest.byte_length() > MAX_FILE_BYTES
            || manifest.chunks().len() > MAX_CHUNKS_PER_FILE
        {
            return Err(invalid(
                "retained CAS expected file/manifest binding differs",
            ));
        }
        let canonical = canonical_manifest_bytes(manifest)?;
        metadata = metadata
            .checked_add(canonical.len())
            .filter(|bytes| *bytes <= MAX_MANIFEST_BYTES)
            .ok_or_else(|| invalid("retained CAS canonical manifests exceed32MiB"))?;
        for chunk in manifest.chunks() {
            cancel.check()?;
            if chunk.byte_length() == 0 || chunk.byte_length() > MAX_CHUNK_BYTES {
                return Err(invalid("retained CAS chunk exceeds index/reader bounds"));
            }
            if chunks.len() >= MAX_CHUNKS && !chunks.contains_key(chunk.chunk_id()) {
                return Err(invalid("retained CAS unique chunk count exceeds bound"));
            }
            if chunks
                .insert(chunk.chunk_id().clone(), chunk.byte_length())
                .is_some_and(|prior| prior != chunk.byte_length())
            {
                return Err(invalid("retained CAS ChunkId has conflicting lengths"));
            }
        }
        manifests.push((manifest, canonical));
    }
    Ok(BoundedExpected { manifests, chunks })
}

/// Exact SQL DATA only, under the driver's retained family transaction and
/// complete schema2/export policy. No reference opener, byte read or fallback
/// by FileId is used. Other exact representations may coexist in the family.
pub(super) fn require_indexed(
    c: &Connection,
    expected: &BTreeMap<FileId, FileManifest>,
    cancel: &Cancellation,
) -> CauseResult<()> {
    cancel.check()?;
    if c.is_autocommit() {
        return Err(invalid(
            "retained CAS index check requires a retained transaction",
        ));
    }
    // Complete canonical/count preflight precedes any catalog payload reads.
    let bounded = bounded_expected(expected, cancel)?;
    for (manifest, canonical) in &bounded.manifests {
        cancel.check()?;
        let id = manifest.id().as_str();
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sweep WHERE kind='manifests' AND id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("retained CAS manifest has a pending sweep"));
        }
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM manifests WHERE id=?1 LIMIT 2)",
            [id],
            |r| r.get(0),
        )?;
        if count != 1 {
            return Err(invalid("retained CAS exact manifest is absent or repeated"));
        }
        let (file_bytes, bytes, payload_bytes): (u64, u64, u64) = c.query_row(
            "SELECT length(CAST(file_id AS BLOB)),bytes,length(payload) FROM manifests WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if file_bytes != 71
            || bytes != manifest.byte_length()
            || payload_bytes != canonical.len() as u64
        {
            return Err(invalid("retained CAS indexed manifest bounds differ"));
        }
        let (file, payload): (String, Vec<u8>) = c.query_row(
            "SELECT file_id,payload FROM manifests WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if file != manifest.file_id().as_str() || payload != *canonical {
            return Err(invalid("retained CAS exact canonical manifest differs"));
        }
        let length: Option<u64> = c
            .query_row(
                "SELECT bytes FROM files WHERE file_id=?1",
                [manifest.file_id().as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if length != Some(manifest.byte_length()) {
            return Err(invalid(
                "retained CAS FileId length index is absent or differs",
            ));
        }
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM manifest_chunks WHERE manifest_id=?1 LIMIT 1025)",
            [id],
            |r| r.get(0),
        )?;
        if count != manifest.chunks().len() as u64 {
            return Err(invalid(
                "retained CAS manifest index has missing or extra chunks",
            ));
        }
        for (ordinal, chunk) in manifest.chunks().iter().enumerate() {
            cancel.check()?;
            if !c.query_row("SELECT EXISTS(SELECT 1 FROM manifest_chunks WHERE manifest_id=?1 AND ordinal=?2 AND chunk_id=?3)",
                params![id, ordinal as u64, chunk.chunk_id().as_str()], |r|r.get::<_,bool>(0))? {
                return Err(invalid("retained CAS manifest ordinal/ChunkId index differs"));
            }
        }
    }
    // A chunk shared by several retained representations is checked once.
    for (id, expected_bytes) in &bounded.chunks {
        cancel.check()?;
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sweep WHERE kind='chunks' AND id=?1)",
            [id.as_str()],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("retained CAS chunk has a pending sweep"));
        }
        let bytes: Option<u64> = c
            .query_row("SELECT bytes FROM chunks WHERE id=?1", [id.as_str()], |r| {
                r.get(0)
            })
            .optional()?;
        if bytes != Some(*expected_bytes) {
            return Err(invalid(
                "retained CAS chunk index length is absent or differs",
            ));
        }
    }
    cancel.check()?;
    Ok(())
}

struct RetainedObject {
    pin: PinnedFile,
    bytes: u64,
}

/// Physical bytes and retained identities only, never SQL/provenance or content
/// authorization. Each immutable Windows handle denies ordinary write/delete
/// sharing while held. Pre-existing writable mappings and privileged raw-volume
/// mutation remain outside this protocol, as in the existing CAS verifier.
pub(super) struct RetainedCas {
    root: PinnedDirectory,
    objects: PinnedDirectory,
    manifests: PinnedDirectory,
    chunks: PinnedDirectory,
    family_marker: RetainedObject,
    root_identity: String,
    // true = manifest, false = chunk. Unique across all representations.
    pins: BTreeMap<(bool, String), RetainedObject>,
    _lifecycle: StoreLock,
}

impl RetainedCas {
    /// Byte transfer only from a representation whose canonical manifest and
    /// chunk are still held immutably. The caller supplies export authority;
    /// this method neither opens SQL nor falls back to another representation.
    pub(super) fn copy_chunk(
        &self,
        manifest: &FileManifest,
        ordinal: usize,
        sink: &mut dyn Write,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.verify_identity(cancel)?;
        let manifest_name = object_name(manifest.id().as_str())?;
        let manifest_pin = self
            .pins
            .get(&(true, manifest_name.to_owned()))
            .ok_or_else(|| invalid("ordinary install historical manifest is not retained"))?;
        compare_bytes(
            &manifest_pin.pin,
            &canonical_manifest_bytes(manifest)?,
            cancel,
        )?;
        let chunk = manifest
            .chunks()
            .get(ordinal)
            .ok_or_else(|| invalid("ordinary install historical chunk ordinal exceeds manifest"))?;
        let name = object_name(chunk.chunk_id().as_str())?;
        let object = self
            .pins
            .get(&(false, name.to_owned()))
            .ok_or_else(|| invalid("ordinary install historical chunk is not retained"))?;
        if object.bytes != chunk.byte_length() {
            return Err(invalid("ordinary install historical chunk length differs"));
        }
        let mut input = object.pin.file();
        let copied = crate::projection::provider::bootstrap::copy_managed_chunk(
            manifest,
            ordinal,
            sink,
            cancel,
            |offset, block| {
                let local = offset
                    .checked_sub(chunk.offset())
                    .ok_or_else(|| invalid("ordinary install historical chunk offset underflow"))?;
                input.seek(SeekFrom::Start(local))?;
                input.read_exact(block)?;
                Ok(())
            },
        );
        let after = self.verify_identity(cancel);
        match (copied, after) {
            (Ok(()), value) => value,
            (Err(error), Ok(())) => Err(error.into()),
            (Err(primary), Err(close)) => Err(
                crate::projection::provider::recovery::error::ProviderBootstrapCause::Cleanup {
                    primary: Box::new(primary.into()),
                    close: Box::new(close),
                },
            ),
        }
    }

    /// Call after positive catalog close, with the same expected map previously
    /// authenticated by actual private mappings and the full export catalog.
    /// This function performs filesystem reads only and acquires its own shared
    /// lifecycle guard. The caller must revalidate actual B/C/catalog before
    /// and after any later effect; this object supplies none of that authority.
    pub(super) fn verify(
        family: &FamilyAccess,
        expected: &BTreeMap<FileId, FileManifest>,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        cancel.check()?;
        let bounded = bounded_expected(expected, cancel)?;
        family.verify()?;
        let lifecycle = family.lifecycle_lease()?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct FamilyIdentity {
            root_identity: String,
        }
        // FamilyAccess already binds these complete canonical bytes to its real
        // marker/root/catalog pins. This projection extracts identity only.
        let marker_bytes = family.marker_bytes();
        if marker_bytes.len() > 65536 {
            return Err(invalid("retained CAS family marker exceeds bound"));
        }
        let marker: FamilyIdentity =
            serde_json::from_slice(marker_bytes).map_err(|error| invalid(error.to_string()))?;
        let root = PinnedDirectory::open(family.root())?;
        if root.identity_token() != marker.root_identity {
            return Err(invalid(
                "retained CAS root differs from actual family marker",
            ));
        }
        let marker_pin = root.open_immutable_file(OsStr::new("FORMAT"))?;
        compare_bytes(&marker_pin, marker_bytes, cancel)?;
        require_same_file(&root, "FORMAT", &marker_pin)?;
        let objects = root.open_child_for_traversal(OsStr::new("objects"))?;
        let manifests = objects.open_child_for_traversal(OsStr::new("manifests"))?;
        let chunks = objects.open_child_for_traversal(OsStr::new("chunks"))?;
        let mut proof = Self {
            root,
            objects,
            manifests,
            chunks,
            family_marker: RetainedObject {
                pin: marker_pin,
                bytes: marker_bytes.len() as u64,
            },
            root_identity: marker.root_identity,
            pins: BTreeMap::new(),
            _lifecycle: lifecycle,
        };
        proof.verify_identity(cancel)?;
        for (manifest, canonical) in bounded.manifests {
            cancel.check()?;
            proof.verify_manifest(manifest, &canonical, cancel)?;
        }
        proof.verify_identity(cancel)?;
        family.verify()?;
        cancel.check()?;
        Ok(proof)
    }

    /// Physical path/inode/length checks only; no SQL, reauthorization or byte
    /// rehash. Immutable handles retain the already verified object bytes.
    pub(super) fn verify_identity(&self, cancel: &Cancellation) -> CauseResult<()> {
        cancel.check()?;
        for directory in [&self.root, &self.objects, &self.manifests, &self.chunks] {
            cancel.check()?;
            directory.verify_path_identity()?;
        }
        if self.root.identity_token() != self.root_identity {
            return Err(invalid("retained CAS physical root identity changed"));
        }
        require_object(&self.root, "FORMAT", &self.family_marker)?;
        for ((manifest, name), object) in &self.pins {
            cancel.check()?;
            let parent = if *manifest {
                &self.manifests
            } else {
                &self.chunks
            };
            require_object(parent, name, object)?;
        }
        cancel.check()?;
        Ok(())
    }

    fn verify_manifest(
        &mut self,
        manifest: &FileManifest,
        canonical: &[u8],
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let name = object_name(manifest.id().as_str())?;
        let object = pin_object(
            &self.manifests,
            &mut self.pins,
            true,
            name,
            canonical.len() as u64,
        )?;
        compare_bytes(&object.pin, canonical, cancel)?;
        require_object(&self.manifests, name, object)?;
        let mut verifier = manifest.verifier();
        for part in manifest.chunks() {
            cancel.check()?;
            let name = object_name(part.chunk_id().as_str())?;
            let object = pin_object(
                &self.chunks,
                &mut self.pins,
                false,
                name,
                part.byte_length(),
            )?;
            let mut input = object.pin.file();
            input.seek(SeekFrom::Start(0))?;
            let count = usize::try_from(part.byte_length())
                .map_err(|_| invalid("retained CAS chunk allocation length overflow"))?;
            let mut buffer = vec![0u8; count];
            for block in buffer.chunks_mut(READ_BYTES) {
                cancel.check()?;
                input.read_exact(block)?;
            }
            cancel.check()?;
            if input.read(&mut [0u8; 1])? != 0 {
                return Err(invalid("retained CAS chunk grew during verification"));
            }
            // The common verifier checks each chunk digest, offset/length and
            // the complete FileId. One bounded <=1MiB chunk is supplied at once.
            verifier
                .push(&buffer)
                .map_err(|error| invalid(error.to_string()))?;
            cancel.check()?;
            require_object(&self.chunks, name, object)?;
        }
        verifier
            .finish()
            .map_err(|error| invalid(error.to_string()))?;
        cancel.check()?;
        Ok(())
    }
}

fn object_name(id: &str) -> CauseResult<&str> {
    id.strip_prefix("blake3:")
        .filter(|name| {
            name.len() == 64
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| invalid("retained CAS object key is not an exact digest"))
}

fn require_same_file(parent: &PinnedDirectory, name: &str, pin: &PinnedFile) -> CauseResult<()> {
    pin.verify_parent()?;
    if !pin.same_file(&parent.open_readonly_file(OsStr::new(name))?) {
        return Err(invalid("retained CAS object path identity changed"));
    }
    Ok(())
}

fn require_object(
    parent: &PinnedDirectory,
    name: &str,
    object: &RetainedObject,
) -> CauseResult<()> {
    require_same_file(parent, name, &object.pin)?;
    if object.pin.file().metadata()?.len() != object.bytes {
        return Err(invalid("retained CAS object length changed"));
    }
    Ok(())
}

fn pin_object<'a>(
    parent: &PinnedDirectory,
    pins: &'a mut BTreeMap<(bool, String), RetainedObject>,
    manifest: bool,
    name: &str,
    bytes: u64,
) -> CauseResult<&'a RetainedObject> {
    use std::collections::btree_map::Entry;
    let object = match pins.entry((manifest, name.to_owned())) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            let pin = parent.open_immutable_file(OsStr::new(name))?;
            entry.insert(RetainedObject { pin, bytes })
        }
    };
    if object.bytes != bytes {
        return Err(invalid("retained CAS object has conflicting lengths"));
    }
    require_object(parent, name, object)?;
    Ok(object)
}

fn compare_bytes(pin: &PinnedFile, expected: &[u8], cancel: &Cancellation) -> CauseResult<()> {
    cancel.check()?;
    if pin.file().metadata()?.len() != expected.len() as u64 {
        return Err(invalid("retained CAS canonical object length differs"));
    }
    let mut input = pin.file();
    input.seek(SeekFrom::Start(0))?;
    let mut scratch = [0u8; READ_BYTES];
    for block in expected.chunks(READ_BYTES) {
        cancel.check()?;
        input.read_exact(&mut scratch[..block.len()])?;
        if &scratch[..block.len()] != block {
            return Err(invalid("retained CAS canonical object bytes differ"));
        }
    }
    cancel.check()?;
    if input.read(&mut [0u8; 1])? != 0 {
        return Err(invalid(
            "retained CAS canonical object grew during verification",
        ));
    }
    cancel.check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{Chunk, ChunkingDescriptor};

    fn manifest(bytes: &[u8]) -> FileManifest {
        FileManifest::from_chunks(
            ChunkingDescriptor::single(),
            &[Chunk::new(bytes.to_vec()).unwrap()],
        )
        .unwrap()
    }

    #[test]
    fn retained_preflight_refuses_a_file_key_bound_to_another_exact_manifest() {
        let manifest = manifest(b"exact retained bytes");
        let expected = BTreeMap::from([(FileId::from_bytes(b"different file"), manifest)]);
        assert!(bounded_expected(&expected, &Cancellation::default()).is_err());
    }

    #[test]
    fn retained_preflight_deduplicates_repeated_chunk_handles_without_losing_ordinals() {
        let chunk = Chunk::new(b"same chunk".to_vec()).unwrap();
        let manifest = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(chunk.bytes().len() as u64).unwrap(),
            &[chunk.clone(), chunk],
        )
        .unwrap();
        let expected = BTreeMap::from([(manifest.file_id().clone(), manifest)]);
        let bounded = bounded_expected(&expected, &Cancellation::default()).unwrap();
        assert_eq!(bounded.manifests[0].0.chunks().len(), 2);
        assert_eq!(bounded.chunks.len(), 1);
    }

    #[test]
    fn retained_preflight_observes_precancel_before_catalog_or_file_reads() {
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(bounded_expected(&BTreeMap::new(), &cancel).is_err());
    }
}
