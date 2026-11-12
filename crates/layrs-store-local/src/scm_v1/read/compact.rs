//! Format 6 answers (ADR 0028): content metadata comes from the registered
//! catalog (packs v2 indexes, FileId sections, file records, loose objects)
//! and content admission from per-Space bitmaps, with the semantics, errors
//! and budgets of the format 5 SQL answers they replace
//! (`docs/plans/store-format-6.md`, reader replacements).
use super::*;
use crate::scm_v1::cas::{FileManifestEntry, Placement, RegisteredCatalog, SpaceAdmissions};
use std::sync::Arc;

const MAX_FILE_MANIFESTS: usize = 1024;
/// Packs a read of a recent object tries before the whole catalog.
const RECENT_PACKS: usize = 4;

/// Exact positions of file records in this reader's large packs, for the
/// projection cache to hold instead of 32-byte FileIds
/// (`projection_cache::FileRefs`). A snapshot of the catalog's metadata:
/// usable after the session ends, and never reads a pack.
pub struct FileRecordRefs(Arc<RegisteredCatalog>);

impl crate::scm_v1::projection_cache::FileRefs for FileRecordRefs {
    fn reference(&self, file_id: &FileId, length: u64) -> Option<([u8; 32], u32)> {
        let (pack, ordinal) = self.0.file_record_ref(file_id)?;
        // A record's length is its File's: an entry claiming another length
        // is not named by it.
        let (_, recorded) = self.0.resolve_file_record(&pack, ordinal)?;
        (recorded == length).then_some((pack, ordinal))
    }

    fn resolve(&self, pack: &[u8; 32], ordinal: u32) -> Option<(FileId, u64)> {
        self.0.resolve_file_record(pack, ordinal)
    }
}

impl NativeScmReader<'_> {
    /// Whether this reader reads a format 6 Store.
    pub(super) fn compact(&self) -> bool {
        self.cas.is_compact()
    }

    /// The file record positions of this reader's catalog (`None` outside
    /// format 6).
    pub fn file_record_refs(&self) -> Result<Option<FileRecordRefs>> {
        if !self.compact() {
            return Ok(None);
        }
        Ok(Some(FileRecordRefs(self.catalog()?)))
    }

    /// The registered catalog of this reader's snapshot: its CAS view's, or
    /// for a transactional reader the transaction's, loaded on first use
    /// (after the batch registered its pack).
    pub(super) fn catalog(&self) -> Result<Arc<RegisteredCatalog>> {
        match self.source {
            SqlReadSource::Ready(store) => {
                // The session's identity guard runs before any cached answer.
                drop(self.connection()?);
                match self.cas.compact_catalog() {
                    Some(catalog) => Ok(Arc::clone(catalog)),
                    None => self.cas.store_catalog(store).map_err(cas_read_error),
                }
            }
            SqlReadSource::Transaction(_) => {
                if let Some(catalog) = self.transaction_catalog.get() {
                    // Still refuse reads after the transaction ended.
                    drop(self.connection()?);
                    return Ok(Arc::clone(catalog));
                }
                let connection = self.connection()?;
                let loaded = Arc::new(
                    RegisteredCatalog::load(&connection, &self.cas).map_err(cas_read_error)?,
                );
                drop(connection);
                Ok(Arc::clone(self.transaction_catalog.get_or_init(|| loaded)))
            }
        }
    }

    fn admissions(
        &self,
        catalog: &RegisteredCatalog,
        space: &SpaceId,
    ) -> Result<Arc<SpaceAdmissions>> {
        let connection = self.connection()?;
        catalog
            .space_admissions(&connection, space.as_str())
            .map_err(cas_read_error)
    }

    pub(super) fn read_object_compact(
        &self,
        id: &StoreObjectId,
    ) -> Result<Option<CanonicalObject>> {
        if !crate::scm_v1::cas::is_content(id.kind()) {
            if let Some(object) = self.read_recent_compact(id)? {
                return Ok(Some(object));
            }
        }
        let catalog = self.catalog()?;
        if crate::scm_v1::cas::is_content(id.kind()) {
            return catalog.read(&self.cas, id).map_err(cas_read_error);
        }
        let connection = self.connection()?;
        let row: Option<(String, i64, i64)> = connection
            .prepare_cached(
                "SELECT object_kind, byte_length, verified FROM objects WHERE object_id = ?1",
            )
            .map_err(sqlite_backend)?
            .query_row([id.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(sqlite_backend)?;
        drop(connection);
        let Some((kind, byte_length, verified)) = row else {
            return Ok(None);
        };
        if kind != kind_name(id.kind()) || verified != 1 || byte_length <= 0 {
            return Err(corruption(format!(
                "objects row for {id} disagrees with requested kind or verification state"
            )));
        }
        // A registered pack record, or the loose file the row authorizes;
        // either is verified against its content address by the read below.
        let length = match catalog.stored_length(id).map_err(cas_read_error)? {
            Some(length) => Some(length),
            None => self.cas.loose_length(id).map_err(cas_read_error)?,
        };
        let length =
            length.ok_or_else(|| corruption(format!("verified CAS object {id} is missing")))?;
        if length != u64::try_from(byte_length).unwrap_or_default() {
            return Err(corruption(format!(
                "objects.byte_length for {id} does not match stored bytes"
            )));
        }
        self.cas
            .read_registered(&catalog, id)
            .map(Some)
            .map_err(cas_read_error)
    }

    /// A non-content object read from the most recent packs while no
    /// catalog is loaded (`RegisteredCatalog::read_recent`): the same row
    /// checks as `read_object_compact`, and `None` to let the whole catalog
    /// answer in every other case.
    fn read_recent_compact(&self, id: &StoreObjectId) -> Result<Option<CanonicalObject>> {
        let SqlReadSource::Ready(store) = self.source else {
            return Ok(None);
        };
        if self.cas.compact_catalog().is_some() || store.registered_catalog().get().is_some() {
            return Ok(None);
        }
        let connection = self.connection()?;
        let row: Option<(String, i64, i64)> = connection
            .prepare_cached(
                "SELECT object_kind, byte_length, verified FROM objects WHERE object_id = ?1",
            )
            .map_err(sqlite_backend)?
            .query_row([id.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(sqlite_backend)?;
        let Some((kind, byte_length, verified)) = row else {
            return Ok(None);
        };
        if kind != kind_name(id.kind()) || verified != 1 || byte_length <= 0 {
            return Ok(None);
        }
        Ok(
            RegisteredCatalog::read_recent(&connection, &self.cas, id, RECENT_PACKS)
                .filter(|(length, _)| i64::try_from(*length).is_ok_and(|l| l == byte_length))
                .map(|(_, object)| object),
        )
    }

    pub(super) fn preflight_chunk_bytes_compact(&self, id: &ChunkId) -> Result<usize> {
        let object_id = StoreObjectId::ChunkRaw(id.clone());
        let length = self
            .catalog()?
            .stored_length(&object_id)
            .map_err(cas_read_error)?
            .ok_or_else(|| {
                corruption(format!(
                    "FileManifest references Chunk {id} without a registered copy"
                ))
            })?;
        positive_usize(i64::try_from(length).unwrap_or(-1), "Chunk", id)
    }

    pub(super) fn chunk_present_compact(&self, id: &ChunkId) -> Result<bool> {
        self.catalog()?
            .contains(&StoreObjectId::ChunkRaw(id.clone()))
            .map_err(cas_read_error)
    }

    /// Whether a registered copy of content `object_id` has its bit (or
    /// loose admission row) in `space`.
    pub(super) fn content_admitted_compact(
        &self,
        object_id: &StoreObjectId,
        space: &SpaceId,
    ) -> Result<bool> {
        let catalog = self.catalog()?;
        let admissions = self.admissions(&catalog, space)?;
        catalog
            .admitted(&admissions, object_id)
            .map_err(cas_read_error)
    }

    fn file_manifest_entries(&self, id: &FileId) -> Result<Vec<FileManifestEntry>> {
        self.catalog()?.file_manifests(id).map_err(cas_read_error)
    }

    pub(super) fn file_length_metadata_compact(&self, id: &FileId) -> Result<Option<u64>> {
        // Every manifest of a File records the same validated length; the
        // smallest, as the format 5 query answered.
        Ok(self
            .file_manifest_entries(id)?
            .iter()
            .map(|entry| entry.file_length)
            .min())
    }

    pub(super) fn file_lengths_metadata_compact<'f>(
        &self,
        files: impl IntoIterator<Item = &'f FileId>,
    ) -> Result<std::collections::BTreeMap<FileId, u64>> {
        let catalog = self.catalog()?;
        let mut lengths = std::collections::BTreeMap::new();
        for file in files.into_iter().collect::<BTreeSet<_>>() {
            if let Some(length) = catalog.file_length(file).map_err(cas_read_error)? {
                lengths.insert(file.clone(), length);
            }
        }
        Ok(lengths)
    }

    pub(super) fn file_manifest_ids_metadata_compact(
        &self,
        id: &FileId,
    ) -> Result<Vec<FileManifestId>> {
        let entries = self.file_manifest_entries(id)?;
        if entries.len() > MAX_FILE_MANIFESTS {
            return Err(corruption(format!(
                "present file {id} exceeds the 1024 verified manifest bound"
            )));
        }
        Ok(entries.into_iter().map(|entry| entry.id).collect())
    }

    pub(super) fn admitted_manifests_metadata_compact<'f>(
        &self,
        files: impl IntoIterator<Item = &'f FileId>,
        space: &SpaceId,
    ) -> Result<std::collections::BTreeMap<FileId, FileManifestId>> {
        let catalog = self.catalog()?;
        let admissions = self.admissions(&catalog, space)?;
        let mut admitted = std::collections::BTreeMap::new();
        for file in files.into_iter().collect::<BTreeSet<_>>() {
            let entries = catalog.file_manifests(file).map_err(cas_read_error)?;
            if entries.is_empty() || entries.len() > MAX_FILE_MANIFESTS {
                continue;
            }
            // Sorted by id: the first admitted one is the smallest.
            for entry in entries {
                let object = StoreObjectId::FileManifest(entry.id.clone());
                if catalog
                    .admitted(&admissions, &object)
                    .map_err(cas_read_error)?
                {
                    admitted.insert(file.clone(), entry.id);
                    break;
                }
            }
        }
        Ok(admitted)
    }

    /// The Files of `files` closed in `space` by a durably admitted
    /// manifest, from the admission bitmaps (council s13 Q2).
    pub(super) fn files_durably_closed_compact<'f>(
        &self,
        files: impl IntoIterator<Item = &'f FileId>,
        space: &SpaceId,
    ) -> Result<BTreeSet<FileId>> {
        let catalog = self.catalog()?;
        // A damaged catalog proves nothing here: the exact path reports its
        // damage (council s14 r1).
        if catalog.damage().is_some() {
            return Ok(BTreeSet::new());
        }
        let admissions = self.admissions(&catalog, space)?;
        let mut closed = BTreeSet::new();
        for file in files.into_iter().collect::<BTreeSet<_>>() {
            if catalog
                .file_manifest_admitted(&admissions, file, MAX_FILE_MANIFESTS)
                .map_err(cas_read_error)?
            {
                closed.insert(file.clone());
            }
        }
        Ok(closed)
    }

    pub(super) fn admitted_chunk_ids_of_files_compact<'f>(
        &self,
        space: &SpaceId,
        files: impl IntoIterator<Item = &'f FileId>,
        limit: usize,
    ) -> Result<Option<BTreeSet<ChunkId>>> {
        let catalog = self.catalog()?;
        let admissions = self.admissions(&catalog, space)?;
        let mut ids = BTreeSet::new();
        for file in files.into_iter().collect::<BTreeSet<_>>() {
            for entry in catalog.file_manifests(file).map_err(cas_read_error)? {
                // A file record's chunk is in its index entry; an explicit
                // manifest is decoded (no projection rows in format 6).
                let chunks: Vec<ChunkId> = match entry.placement {
                    // That record's own chunk: a File may have a whole and
                    // a v1 record in different packs (council s16 r1).
                    Placement::Manifest { pack, file: record } => {
                        vec![catalog.alias_chunk(pack, record).map_err(cas_read_error)?]
                    }
                    // A manifest the catalog lists must read: absence here
                    // is corruption, never an empty chunk list.
                    _ => self
                        .get_file_manifest(&entry.id)?
                        .ok_or_else(|| {
                            corruption(format!(
                                "file {file} lists manifest {} that cannot be read",
                                entry.id
                            ))
                        })?
                        .chunks()
                        .iter()
                        .map(|chunk| chunk.chunk_id().clone())
                        .collect(),
                };
                for chunk in chunks {
                    if ids.contains(&chunk) {
                        continue;
                    }
                    let object = StoreObjectId::ChunkRaw(chunk.clone());
                    if catalog
                        .admitted(&admissions, &object)
                        .map_err(cas_read_error)?
                    {
                        ids.insert(chunk);
                        if ids.len() > limit {
                            return Ok(None);
                        }
                    }
                }
            }
        }
        Ok(Some(ids))
    }

    /// The manifest rows `file_availability_with_budget` charges and then
    /// reads, from the index: `None` when the File is absent.
    pub(super) fn file_availability_rows_compact(
        &self,
        id: &FileId,
        limits: FileAvailabilityLimits,
        consumed: &mut FileAvailabilityConsumption,
    ) -> Result<Option<Vec<FileManifestId>>> {
        let catalog = self.catalog()?;
        let entries = catalog.file_manifests(id).map_err(cas_read_error)?;
        if entries.is_empty() {
            return Ok(None);
        }
        let mut rows = Vec::with_capacity(entries.len());
        for entry in entries {
            consumed.charge_rows(1, limits.rows)?;
            consumed.charge_manifests(1, limits.manifests)?;
            let object = StoreObjectId::FileManifest(entry.id.clone());
            let stored = catalog
                .stored_length(&object)
                .map_err(cas_read_error)?
                .ok_or_else(|| {
                    corruption(format!(
                        "file {id} references manifest {} without a registered copy",
                        entry.id
                    ))
                })?;
            consumed.charge_bytes(
                positive_usize(i64::try_from(stored).unwrap_or(-1), "manifest", &entry.id)?,
                limits.bytes,
            )?;
            rows.push(entry.id);
        }
        Ok(Some(rows))
    }
}
