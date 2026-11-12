//! Store format 6 CAS (ADR 0028, `docs/plans/store-format-6.md`): content
//! in packs v2 registered in SQLite (`cas_packs`), records over 1 MiB as
//! loose files registered in `loose_objects`, single-chunk Files as file
//! records with ChunkId and manifest aliases.
//!
//! Committed installation is SQLite's word. A reader only ever looks into
//! the packs and loose objects of its `RegisteredCatalog`, loaded from one
//! SQLite snapshot: the Store's immutable read connection, or the batch
//! transaction. A pack file without a `cas_packs` row is never present; a
//! registered pack whose metadata digest differs from the registered one is
//! corruption, never absence: a lookup that does not find an object while a
//! registered pack is unreadable fails instead of answering "absent".
use super::pack2::{self, FILE_KIND, FileIdentity, Location, Pack2Builder, Pack2Index, Recipe};
use super::*;
use layrs_core::scm::FileId;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

/// Decompressed blocks a catalog keeps for its next reads (a checkout reads
/// many small records of the same block in a row). Every record is still
/// verified against its id at each read, and the cache lives only as long
/// as the catalog (one SQLite snapshot): a later operation rereads the disk,
/// as format 5 rereads every object, and sees any damage done since.
const RECENT_BLOCKS: usize = 64;
/// Packs whose file records derived data may name by position
/// (`file_record_ref`): large ones, which consolidation rarely replaces.
const REFERENCED_PACK_RECORDS: usize = 4096;
const RECENT_BLOCK_MAX: usize = 64 * 1024;
/// Most pack files a held catalog keeps open (`hold_pack_files`): two
/// descriptors each (the file and its pinned directory).
const HELD_PACKS_MAX: usize = 16;
/// Most pack files every held catalog of this process keeps open together
/// (council s9 r3): concurrent staging sessions share it. Past it, a read
/// opens its pack for that read only.
const HELD_PACKS_PROCESS_MAX: usize = 32;

/// Pack files kept open by the held catalogs of this process.
fn held_packs() -> &'static std::sync::atomic::AtomicUsize {
    static HELD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    &HELD
}

/// One pack v2 loaded by this process: its validated metadata. A published
/// pack is immutable; the cache is keyed by
/// Store root identity and pack name, and every catalog compares the loaded
/// digest with the one SQLite registered. No handle stays open between
/// reads (ADR 0026): a block read opens the pack through the pinned CAS
/// directories, as a loose read does, so a Store directory is never held
/// by a process that merely read from it. The one exception is a bulk read
/// that holds its catalog's pack files (`RegisteredCatalog::hold_pack_files`)
/// until it ends.
pub(crate) struct LoadedPack {
    name: OsString,
    path: PathBuf,
    index: Pack2Index,
    /// The whole pack when it is registered inline in SQLite (council s7
    /// Q1); `None` for a pack file.
    image: Option<Arc<[u8]>>,
    /// Raw id of each file record's recipe manifest, computed on first use
    /// from the record's metadata (never from its bytes).
    manifest_digests: Vec<OnceLock<[u8; 32]>>,
}

impl fmt::Debug for LoadedPack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedPack")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

type PackCache = Mutex<HashMap<(String, OsString), Arc<LoadedPack>>>;

fn pack_cache() -> &'static PackCache {
    static CACHE: OnceLock<PackCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn alias_corruption(path: &Path, error: impl fmt::Display) -> CasError {
    pack2::pack2_corruption(
        path,
        format!("file record does not rebuild its manifest: {error}"),
    )
}

impl LoadedPack {
    /// The pack file, opened through `cas`'s pinned directories; it must
    /// still have the length its metadata was validated against (blocks are
    /// then checked against their checksums and records against their ids).
    fn open(&self, cas: &NativeCas) -> CasResult<PinnedFile> {
        let directory = cas.objects_dir.join(PACKS_DIR);
        let packs = cas
            .pinned_objects_for_read()?
            .open_child(OsStr::new(PACKS_DIR))
            .map_err(|error| missing_or_io("pin CAS packs", &directory, error))?;
        // For reading only: no delete access, which an opener that does
        // not share deletion would otherwise conflict with.
        #[cfg(windows)]
        let file = packs.open_readonly_file(&self.name);
        #[cfg(not(windows))]
        let file = packs.open_file(&self.name);
        let file = file.map_err(|error| missing_or_io("open CAS pack", &self.path, error))?;
        let length = file
            .file()
            .metadata()
            .map_err(|error| io_error("stat CAS pack", &self.path, error))?
            .len();
        if length != self.index.file_len {
            return Err(pack2::pack2_corruption(
                &self.path,
                "pack length changed since its metadata was read",
            ));
        }
        Ok(file)
    }

    fn read_block(&self, cas: &NativeCas, block: u32) -> CasResult<Vec<u8>> {
        if let Some(image) = &self.image {
            return pack2::read_block_from(
                pack2::PackSource::Image(image),
                &self.path,
                &self.index,
                block,
            );
        }
        let file = self.open(cas)?;
        pack2::read_block(file.file(), &self.path, &self.index, block)
    }

    /// Whether this pack is registered inline (it has no file).
    pub(crate) fn is_inline(&self) -> bool {
        self.image.is_some()
    }

    fn alias_ids(&self, file: usize) -> CasResult<(FileId, ChunkId)> {
        let record = &self.index.files[file];
        let file_id = FileId::parse(pack2::digest_text(&record.file_id))
            .map_err(|error| alias_corruption(&self.path, error))?;
        let chunk_id = ChunkId::parse(pack2::digest_text(&record.chunk_id))
            .map_err(|error| alias_corruption(&self.path, error))?;
        Ok((file_id, chunk_id))
    }

    /// The canonical one-chunk manifest file record `file` stands for.
    fn alias_manifest(&self, file: usize) -> CasResult<FileManifest> {
        let record = &self.index.files[file];
        let (file_id, chunk_id) = self.alias_ids(file)?;
        pack2::synthesize_manifest(
            &file_id,
            &chunk_id,
            u64::from(record.location.length),
            record.recipe,
        )
        .map_err(|error| alias_corruption(&self.path, error))
    }

    fn alias_manifest_digest(&self, file: usize) -> CasResult<[u8; 32]> {
        if let Some(digest) = self.manifest_digests[file].get() {
            return Ok(*digest);
        }
        let manifest = self.alias_manifest(file)?;
        let digest = pack2::raw_digest(&StoreObjectId::FileManifest(manifest.id().clone()))?;
        if digest[..4] != self.index.files[file].manifest_prefix {
            return Err(pack2::pack2_corruption(
                &self.path,
                "file record manifest prefix differs from its rebuilt manifest",
            ));
        }
        let _ = self.manifest_digests[file].set(digest);
        Ok(digest)
    }
}

impl NativeCas {
    /// A pack of version 2 (`whole` records) belongs only in a Store of
    /// format 7: never read as anything else (council s13 Q5).
    fn check_pack_version(&self, index: &Pack2Index, path: &Path) -> CasResult<()> {
        if index.version != 1 && !self.whole_files {
            return Err(pack2::pack2_corruption(
                path,
                "a pack of Store format 7 in an older Store",
            ));
        }
        Ok(())
    }

    fn compact_key(&self) -> String {
        format!("{}|{}", self.root_identity, self.root.display())
    }

    /// Where an inline pack's bytes are, for messages: the database row.
    fn inline_label(&self, name: &OsStr) -> PathBuf {
        self.root
            .join(crate::scm_v1::layout::METADATA_DIR_NAME)
            .join(format!(
                "{}#{}",
                crate::scm_v1::layout::SQLITE_FILE_NAME,
                name.to_string_lossy()
            ))
    }

    /// A pack registered inline, from the process cache or validated now
    /// from its image (as a pack file's metadata is). `fresh` as for
    /// `loaded_pack`.
    fn inline_pack(
        &self,
        name: &OsStr,
        image: Arc<[u8]>,
        fresh: bool,
    ) -> CasResult<Arc<LoadedPack>> {
        let key = (self.compact_key(), name.to_os_string());
        if !fresh {
            if let Some(pack) = pack_cache()
                .lock()
                .ok()
                .and_then(|cache| cache.get(&key).cloned())
                .filter(|pack| pack.image.as_deref() == Some(&*image))
            {
                self.check_pack_version(&pack.index, &pack.path)?;
                return Ok(pack);
            }
        }
        let path = self.inline_label(name);
        let index = pack2::load_from(pack2::PackSource::Image(&image), &path)?;
        self.check_pack_version(&index, &path)?;
        let manifest_digests = (0..index.files.len()).map(|_| OnceLock::new()).collect();
        let pack = Arc::new(LoadedPack {
            name: name.to_os_string(),
            path,
            index,
            image: Some(image),
            manifest_digests,
        });
        if fresh {
            return Ok(pack);
        }
        if let Ok(mut cache) = pack_cache().lock() {
            cache.insert(key, Arc::clone(&pack));
        }
        Ok(pack)
    }

    /// A registered pack file, from the process cache or loaded and
    /// validated now, opening the packs directory at most once for all the
    /// packs of one catalog (`packs`). Load failures are not cached. With
    /// `fresh` (audits), the metadata on disk now is loaded and checked, and
    /// the cache is neither read nor filled: damage done since the cache was
    /// filled is seen.
    fn loaded_pack_in(
        &self,
        packs: &std::cell::OnceCell<PinnedDirectory>,
        name: &OsStr,
        fresh: bool,
    ) -> CasResult<Arc<LoadedPack>> {
        let key = (self.compact_key(), name.to_os_string());
        if fresh {
            return self.load_pack_from_disk(packs, name).map(Arc::new);
        }
        // Only a cached pack file: a pack is served only in the form its
        // registration declares (an inline pack is never its file).
        if let Some(pack) = pack_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key).cloned())
            .filter(|pack| pack.image.is_none())
        {
            self.check_pack_version(&pack.index, &pack.path)?;
            return Ok(pack);
        }
        let pack = Arc::new(self.load_pack_from_disk(packs, name)?);
        if let Ok(mut cache) = pack_cache().lock() {
            cache.insert(key, Arc::clone(&pack));
        }
        Ok(pack)
    }

    fn load_pack_from_disk(
        &self,
        packs: &std::cell::OnceCell<PinnedDirectory>,
        name: &OsStr,
    ) -> CasResult<LoadedPack> {
        let _span = crate::perf::span("cas.pack_load");
        let directory = self.objects_dir.join(PACKS_DIR);
        let path = directory.join(name);
        let packs = match packs.get() {
            Some(packs) => packs,
            None => {
                let opened = self
                    .pinned_objects_for_read()?
                    .open_child(OsStr::new(PACKS_DIR))
                    .map_err(|error| missing_or_io("pin CAS packs", &directory, error))?;
                packs.get_or_init(|| opened)
            }
        };
        let file = packs
            .open_file(name)
            .map_err(|error| missing_or_io("open CAS pack", &path, error))?;
        let parse = crate::perf::span("cas.pack_index");
        let index = pack2::load(file.file(), &path)?;
        drop(parse);
        self.check_pack_version(&index, &path)?;
        drop(file);
        let manifest_digests = (0..index.files.len()).map(|_| OnceLock::new()).collect();
        Ok(LoadedPack {
            name: name.to_os_string(),
            path,
            index,
            image: None,
            manifest_digests,
        })
    }

    /// Format 6, under the exclusive Store lock: removes every pack file
    /// SQLite does not register. A pack is only registered by the exclusive
    /// session that published it, so one unregistered when a session starts
    /// (a batch that failed after publication) can never become registered.
    /// Best effort: garbage that cannot be removed now is removed later.
    pub(super) fn collect_orphan_packs(&self, catalog: &RegisteredCatalog) -> usize {
        let directory = self.objects_dir.join(PACKS_DIR);
        let Ok(entries) = fs::read_dir(&directory) else {
            return 0;
        };
        // Every registered name is kept, whatever form its row declares: a
        // damaged form (a file pack read as inline) must never cost the
        // file (council s8 r3). Reads alone refuse a file for an inline pack.
        let registered: HashSet<&OsStr> = catalog
            .rows
            .iter()
            .map(|row| row.name.as_os_str())
            .collect();
        let orphans: Vec<OsString> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .filter(|name| pack2::valid_pack2_name(name) && !registered.contains(name.as_os_str()))
            .collect();
        if orphans.is_empty() {
            return 0;
        }
        let Ok(packs) = self.pinned_objects().and_then(|objects| {
            objects
                .open_child(OsStr::new(PACKS_DIR))
                .map_err(|error| io_error("pin CAS packs", &directory, error))
        }) else {
            return 0;
        };
        let mut removed = 0;
        for name in orphans {
            if packs
                .open_file(&name)
                .and_then(|file| file.unlink())
                .is_ok()
            {
                removed += 1;
            }
        }
        crate::perf::count("cas.orphan_packs_removed", removed as u64);
        removed
    }
}

/// A logical object's identity: kind code and raw id.
pub(crate) type Identity = (u8, [u8; 32]);

/// One logical object of a pack by its form and identity: what a
/// consolidated pack's ordinal is found by for every old ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum LogicalKey {
    /// An explicit record: kind code and raw id.
    Explicit(u8, [u8; 32]),
    /// The chunk a file record (raw FileId) stands for.
    ChunkAlias([u8; 32]),
    /// The recipe manifest a file record (raw FileId) stands for.
    ManifestAlias([u8; 32]),
}

/// One registered pack written into a consolidated one.
#[derive(Debug)]
pub(crate) struct ConsolidatedPack {
    pub(crate) seq: i64,
    pub(crate) name: OsString,
    /// Whether it is registered inline (its image row goes with it).
    pub(crate) inline: bool,
    /// Its registered metadata digest.
    pub(crate) digest: [u8; 32],
    /// The new ordinal of each of its logical ordinals.
    pub(crate) ordinals: Vec<u64>,
    /// The identity of each of its logical ordinals, from its own index.
    pub(crate) identities: Vec<Identity>,
}

/// Registered packs written into one new pack, published durably but not
/// registered yet (ADR 0028, section 6).
#[derive(Debug)]
pub(crate) struct Consolidation {
    /// The new pack: name, metadata digest, logical entries.
    pub(crate) pack: (OsString, [u8; 32], u64),
    /// Its keys row (council s17, B1b).
    pub(crate) keys: super::keys::PackKeys,
    /// Its image when it is registered inline.
    pub(crate) inline: Option<Vec<u8>>,
    pub(crate) packs: Vec<ConsolidatedPack>,
    /// The identity of each logical ordinal of the new pack, from its own
    /// index (independent of the remap).
    pub(crate) identities: Vec<Identity>,
}

/// A consolidation candidate: its registration (sequence, name, metadata
/// digest) and whether it is an inline image.
pub(crate) type CandidatePack = (i64, OsString, [u8; 32], bool);

/// Name prefix of a maintenance pack being built (`.maint-…tmp`).
const MAINTENANCE_PREFIX: &str = "maint-";

/// A maintenance consolidation built outside the exclusive Store lock
/// (`NativeCas::build_maintenance`), not yet published.
pub(crate) struct BuiltPack {
    open: OpenPack,
    sealed: pack2::SealedPack2,
    packs: Vec<CandidatePack>,
    keys_of: CandidateKeys,
}

/// Each candidate's logical keys with their identities, for the remap.
type CandidateKeys = Vec<Vec<(LogicalKey, Identity)>>;

/// Which side of a record copy failed: reading or verifying the candidate's
/// record (the candidate is at fault), or writing the new pack (it is not).
enum CopyFailure {
    Source(CasError),
    Target(CasError),
}

impl From<CasError> for CopyFailure {
    fn from(error: CasError) -> Self {
        Self::Source(error)
    }
}

/// Why a consolidation stopped: the candidate (position in the catalog)
/// whose record could not be copied, when one did.
#[derive(Debug)]
pub(crate) struct ConsolidationFailure {
    pub(crate) pack: Option<usize>,
    pub(crate) error: CasError,
}

/// Registered packs that trigger a consolidation (ADR 0028, section 6):
/// above this count, every lookup and every catalog load pays for each.
const CONSOLIDATE_AT_PACKS: usize = 6;
/// Inline packs that are merged together (council s8 r1).
const CONSOLIDATE_INLINE_AT: usize = 8;
/// Bytes of inline packs that are merged together, whatever their count
/// (council s13 Q1): a batch keeps up to 256 KiB inline, so fewer, larger
/// inline packs are merged before the database holds much of them.
const CONSOLIDATE_INLINE_BYTES: u64 = 1024 * 1024;
/// A consolidation's output stays inline up to this size only (council s13
/// Q1): a larger merged pack becomes a file, so merged history does not
/// accumulate in the database.
const MERGED_INLINE_MAX: usize = 64 * 1024;
/// The most bytes of packs one consolidation rewrites inline, under the
/// exclusive lock of the write that triggered it (council s6 r2): readers
/// and writers wait seconds at most. Larger tiers are left as they are (an
/// explicit maintenance pass is future work); git's automatic gc runs
/// detached for the same reason.
const INLINE_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
/// The most decompressed bytes one inline consolidation reads, verifies and
/// recompresses (council s6 r3): compressible content is bounded by the work
/// it costs, not by its size on disk.
const INLINE_RAW_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

/// What one consolidation may write: its new pack's size on disk and its
/// decompressed bytes (half the reader's limits always apply too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    bytes: u64,
    raw: u64,
}

impl Budget {
    /// Inside a batch's exclusive session (council s6).
    pub(crate) const IN_BATCH: Self = Self {
        bytes: INLINE_BUDGET_BYTES,
        raw: INLINE_RAW_BUDGET_BYTES,
    };
    /// A maintenance pass, built outside the lock (council s17, B2b): only
    /// the reader's limits.
    pub(crate) const MAINTENANCE: Self = Self {
        bytes: u64::MAX,
        raw: u64::MAX,
    };
}

impl RegisteredCatalog {
    /// Whether a consolidation would find a group once `added` (a pack the
    /// batch registered: inline or not, and its sizes) joins this catalog,
    /// decided without reopening anything. Inline packs do not count toward
    /// the file trigger: they are merged among themselves at
    /// `CONSOLIDATE_INLINE_AT` packs (council s8 r1), so small batches do not
    /// consolidate every few, or at `CONSOLIDATE_INLINE_BYTES` (council s13
    /// Q1). At `CONSOLIDATE_AT_PACKS` pack files, only when a group fits the
    /// budgets: larger tiers wait for maintenance, and a batch no longer
    /// reopens the CAS to find nothing (council s17, B2a).
    pub(crate) fn consolidation_due(
        &self,
        added: Option<(bool, PackStats)>,
        budget: Budget,
    ) -> bool {
        // Counts and inline sizes first (no block is walked): the common
        // batch decides here (council s18 r1).
        let (mut inline_count, mut inline_bytes, mut file_count) = (0_usize, 0_u64, 0_usize);
        for pack in &self.packs {
            if pack.pack.is_inline() {
                inline_count += 1;
                inline_bytes = inline_bytes.saturating_add(pack.pack.index.file_len);
            } else {
                file_count += 1;
            }
        }
        match added {
            Some((true, (size, ..))) => {
                inline_count += 1;
                inline_bytes = inline_bytes.saturating_add(size);
            }
            Some((false, _)) => file_count += 1,
            None => {}
        }
        if inline_count < CONSOLIDATE_INLINE_AT
            && inline_bytes < CONSOLIDATE_INLINE_BYTES
            && file_count < CONSOLIDATE_AT_PACKS
        {
            return false;
        }
        let mut inline = self.by_size(&[], true);
        let mut files = self.by_size(&[], false);
        if let Some((is_inline, (size, records, blocks, raw))) = added {
            let list = if is_inline { &mut inline } else { &mut files };
            list.push((size, records, blocks, raw, usize::MAX));
            list.sort_unstable();
        }
        let file_count = files.len();
        candidates_among_within(&inline, &files, file_count, budget).len() >= 2
    }

    /// The packs to consolidate now, positions in increasing order, leaving
    /// out `excluded`; empty when none is due. At `CONSOLIDATE_INLINE_AT`
    /// inline packs, as many of them as fit the budgets, smallest first
    /// (council s8 r2, r3). Else, at `CONSOLIDATE_AT_PACKS` pack files, a
    /// geometric group of files, as git's: the smallest while each next one
    /// is at most twice their total, so a large pack is rewritten only once
    /// the small ones have grown to its size. Only sound packs: a damaged one
    /// keeps its row and bitmaps as they are, and does not stop the others
    /// (council s6 r1).
    pub(crate) fn consolidation_candidates(
        &self,
        excluded: &[usize],
        budget: Budget,
    ) -> Vec<usize> {
        let files = self.packs.len()
            - self
                .packs
                .iter()
                .filter(|pack| pack.pack.is_inline())
                .count();
        candidates_among_within(
            &self.by_size(excluded, true),
            &self.by_size(excluded, false),
            files,
            budget,
        )
    }

    /// Whether there are at least `CONSOLIDATE_AT_PACKS` pack files (a
    /// count, no block walked).
    pub(crate) fn many_pack_files(&self) -> bool {
        self.packs
            .iter()
            .filter(|pack| !pack.pack.is_inline())
            .count()
            >= CONSOLIDATE_AT_PACKS
    }

    /// Whether, leaving out `excluded`, at least `CONSOLIDATE_AT_PACKS`
    /// pack files remain among which a maintenance group exists (council
    /// s17, B2c): what a batch's own consolidation could not write.
    pub(crate) fn maintenance_due(&self, excluded: &[usize]) -> bool {
        let files = self.by_size(excluded, false);
        files.len() >= CONSOLIDATE_AT_PACKS
            && consolidation_group_within(&files, Budget::MAINTENANCE).len() >= 2
    }

    /// The pack files a maintenance pass consolidates (council s17, B2b):
    /// a geometric group of at least two, within half the reader's limits
    /// only, leaving out `excluded`; empty when none.
    pub(crate) fn maintenance_candidates(&self, excluded: &[usize]) -> Vec<usize> {
        consolidation_group_within(&self.by_size(excluded, false), Budget::MAINTENANCE)
    }

    /// (size, records, blocks, decompressed bytes, position) of the inline
    /// packs, or of the pack files, leaving out `excluded`; smallest first.
    fn by_size(&self, excluded: &[usize], inline: bool) -> Vec<(u64, u64, u64, u64, usize)> {
        let mut by_size: Vec<(u64, u64, u64, u64, usize)> = self
            .packs
            .iter()
            .enumerate()
            .filter(|(position, pack)| {
                pack.pack.is_inline() == inline && !excluded.contains(position)
            })
            .map(|(position, registered)| {
                let (size, records, blocks, raw) = pack_stats(&registered.pack.index);
                (size, records, blocks, raw, position)
            })
            .collect();
        by_size.sort_unstable();
        by_size
    }
}

/// The keys a question about object `id` asks of a pack's keys row: its
/// explicit record, and for a chunk or a manifest the file record aliases
/// that may stand for it (a whole chunk is named by its FileId).
fn object_keys(id: &StoreObjectId) -> CasResult<Vec<super::keys::Key>> {
    use super::keys::{self, key};
    let raw = pack2::raw_digest(id)?;
    let mut asked = vec![keys::explicit_key(pack2::kind_code(id.kind()), &raw)];
    match id.kind() {
        StoreObjectKind::ChunkRaw => {
            asked.push(key(keys::CHUNK_ALIAS_TAG, &raw));
            asked.push(key(keys::FILE_RECORD_TAG, &raw));
        }
        StoreObjectKind::FileManifest => asked.push(key(keys::MANIFEST_ALIAS_TAG, &raw)),
        _ => {}
    }
    Ok(asked)
}

/// The keys a question about File `file_id` (its manifests, its length)
/// asks: the FileId section and the file records.
fn file_keys(file_id: &FileId) -> Vec<super::keys::Key> {
    use super::keys::{self, key};
    let raw = file_id.digest_bytes();
    vec![
        key(keys::FILEID_SECTION_TAG, &raw),
        key(keys::FILE_RECORD_TAG, &raw),
    ]
}

/// The key filters of the `damaged` packs whose keys row is sound and bound
/// to their registration (`rows`); read only when a pack is unreadable.
fn damaged_filters(
    connection: &Connection,
    damaged: &[(i64, CasError)],
    rows: &[RegisteredRow],
) -> HashMap<i64, super::keys::KeyFilter> {
    let mut filters = HashMap::new();
    if damaged.is_empty() {
        return filters;
    }
    let Ok(mut statement) = connection.prepare(
        "SELECT layout_digest,keys_digest,key_count,rice_bits,group_digests,filter,checksum          FROM cas_pack_keys WHERE pack_seq=?1",
    ) else {
        return filters;
    };
    for (seq, _) in damaged {
        let Some(row) = rows.iter().find(|row| row.seq == *seq) else {
            continue;
        };
        let stored = statement.query_row([seq], |stored| {
            Ok((
                stored.get::<_, Vec<u8>>(0)?,
                stored.get::<_, Vec<u8>>(1)?,
                stored.get::<_, i64>(2)?,
                stored.get::<_, i64>(3)?,
                stored.get::<_, Vec<u8>>(4)?,
                stored.get::<_, Vec<u8>>(5)?,
                stored.get::<_, Vec<u8>>(6)?,
            ))
        });
        let Ok((layout, keys_digest, count, bits, groups, filter, checksum)) = stored else {
            continue;
        };
        if let Some(filter) = super::keys::PackKeys::filter_of_row(
            &row.digest,
            &layout,
            &keys_digest,
            count,
            bits,
            groups,
            filter,
            &checksum,
        ) {
            filters.insert(*seq, filter);
        }
    }
    filters
}

/// A pack's size on disk, records, blocks and decompressed bytes: what the
/// consolidation budgets count.
pub(crate) type PackStats = (u64, u64, u64, u64);

pub(super) fn pack_stats(index: &pack2::Pack2Index) -> PackStats {
    (
        index.file_len,
        (index.explicit_count() + index.files.len()) as u64,
        index.blocks.len() as u64,
        index
            .blocks
            .iter()
            .map(|block| u64::from(block.raw_len))
            .sum(),
    )
}

/// The packs to consolidate among the inline packs and the pack files (each
/// smallest first, as `by_size` lists them; `file_count` pack files are
/// registered): at `CONSOLIDATE_INLINE_AT` inline packs or
/// `CONSOLIDATE_INLINE_BYTES`, as many of them as fit the budgets (council
/// s8 r2, r3); else, at `CONSOLIDATE_AT_PACKS` pack files, a geometric group
/// of files. Inline packs never join pack files (they wait for their own
/// merge). Empty when none is due.
#[cfg(test)]
fn candidates_among(
    inline: &[(u64, u64, u64, u64, usize)],
    files: &[(u64, u64, u64, u64, usize)],
    file_count: usize,
) -> Vec<usize> {
    candidates_among_within(inline, files, file_count, Budget::IN_BATCH)
}

/// `candidates_among` within `budget`.
fn candidates_among_within(
    inline: &[(u64, u64, u64, u64, usize)],
    files: &[(u64, u64, u64, u64, usize)],
    file_count: usize,
    budget: Budget,
) -> Vec<usize> {
    let inline_bytes = inline
        .iter()
        .fold(0_u64, |total, pack| total.saturating_add(pack.0));
    if inline.len() >= CONSOLIDATE_INLINE_AT || inline_bytes >= CONSOLIDATE_INLINE_BYTES {
        let group = bounded_group_within(inline, false, budget);
        if group.len() >= 2 {
            return group;
        }
    }
    if file_count < CONSOLIDATE_AT_PACKS {
        return Vec::new();
    }
    consolidation_group_within(files, budget)
}

/// The packs to consolidate among `by_size` (size, records, blocks,
/// decompressed bytes, position; smallest first): from each size tier in
/// turn (a lone small pack must not block the tiers above it, council s6
/// r1), the first group of at least two, each next pack at most twice the
/// group's total, within half the reader's limits for one pack and the
/// inline budgets (on disk and decompressed). Positions in increasing order.
#[cfg(test)]
fn consolidation_group(by_size: &[(u64, u64, u64, u64, usize)]) -> Vec<usize> {
    consolidation_group_within(by_size, Budget::IN_BATCH)
}

/// `consolidation_group` within `budget`.
fn consolidation_group_within(
    by_size: &[(u64, u64, u64, u64, usize)],
    budget: Budget,
) -> Vec<usize> {
    for start in 0..by_size.len() {
        let chosen = bounded_group_within(&by_size[start..], true, budget);
        if chosen.len() >= 2 {
            return chosen;
        }
    }
    Vec::new()
}

/// The packs taken from `by_size` (smallest first) while the group stays
/// within the inline budgets (on disk and decompressed) and half the
/// reader's limits for one pack, and, `geometric`, while each next pack is
/// at most twice the group's total. Without the geometric rule a pack that
/// does not fit is skipped, so one oversized pack never blocks the others
/// (council s8 r3). Positions in increasing order.
#[cfg(test)]
fn bounded_group(by_size: &[(u64, u64, u64, u64, usize)], geometric: bool) -> Vec<usize> {
    bounded_group_within(by_size, geometric, Budget::IN_BATCH)
}

/// `bounded_group` within `budget`.
fn bounded_group_within(
    by_size: &[(u64, u64, u64, u64, usize)],
    geometric: bool,
    budget: Budget,
) -> Vec<usize> {
    let mut chosen = Vec::new();
    let (mut total, mut records, mut blocks, mut raw) = (0_u64, 0_u64, 0_u64, 0_u64);
    for &(size, pack_records, pack_blocks, pack_raw, position) in by_size {
        if geometric && !chosen.is_empty() && size > total.saturating_mul(2) {
            break;
        }
        // Within the inline budgets, and half the reader's limits for one
        // pack so the new pack always loads.
        let fits = total.saturating_add(size) <= budget.bytes
            && raw.saturating_add(pack_raw) <= budget.raw
            && records + pack_records <= pack2::MAX_PACK_ENTRIES / 2
            && blocks + pack_blocks <= pack2::MAX_PACK_BLOCKS / 2;
        if !fits {
            if geometric {
                break;
            }
            continue;
        }
        total = total.saturating_add(size);
        raw = raw.saturating_add(pack_raw);
        records += pack_records;
        blocks += pack_blocks;
        chosen.push(position);
    }
    chosen.sort_unstable();
    chosen
}

impl LoadedPack {
    /// The keys row this pack's index gives: what its builder wrote when it
    /// sealed it (`Pack2Builder::keys`), rebuilt from the index (each file
    /// record's alias manifest rebuilt and hashed).
    fn rebuild_keys(&self, rice_bits: u8) -> CasResult<super::keys::PackKeys> {
        use super::keys::{self, Ordinal, SectionEntry};
        let index = &self.index;
        let vanished = || pack2::pack2_corruption(&self.path, "pack record vanished");
        let explicit: Vec<(u8, [u8; 32])> = (0..index.explicit_count())
            .map(|ordinal| {
                index
                    .explicit(ordinal)
                    .map(|(code, id, _)| (code, *id))
                    .ok_or_else(vanished)
            })
            .collect::<CasResult<_>>()?;
        let manifests: Vec<[u8; 32]> = (0..index.files.len())
            .map(|file| self.alias_manifest_digest(file))
            .collect::<CasResult<_>>()?;
        let mut ordinals = Vec::with_capacity(explicit.len() + 2 * index.files.len());
        let mut tagged = Vec::with_capacity(explicit.len() + 3 * index.files.len());
        for (code, id) in &explicit {
            ordinals.push(Ordinal::Explicit {
                kind_code: *code,
                id,
            });
            tagged.push(keys::explicit_key(*code, id));
        }
        for (record, manifest_id) in index.files.iter().zip(&manifests) {
            ordinals.push(Ordinal::ChunkAlias {
                file_id: &record.file_id,
                chunk_id: &record.chunk_id,
                recipe: record.recipe.code(),
                length: u64::from(record.location.length),
            });
            ordinals.push(Ordinal::ManifestAlias {
                file_id: &record.file_id,
                manifest_id,
            });
            tagged.push(keys::key(keys::FILE_RECORD_TAG, &record.file_id));
            if record.recipe != pack2::Recipe::Whole {
                tagged.push(keys::key(keys::CHUNK_ALIAS_TAG, &record.chunk_id));
            }
            tagged.push(keys::key(keys::MANIFEST_ALIAS_TAG, manifest_id));
        }
        let section: Vec<SectionEntry<'_>> = index
            .fileid_section
            .iter()
            .map(|entry| SectionEntry {
                file_id: &entry.file_id,
                manifest_position: entry.manifest,
                length: entry.byte_length,
            })
            .collect();
        for entry in &index.fileid_section {
            tagged.push(keys::key(keys::FILEID_SECTION_TAG, &entry.file_id));
        }
        keys::PackKeys::build(&ordinals, &section, tagged, rice_bits)
            .map_err(|error| pack2::pack2_corruption(&self.path, error.to_string()))
    }

    /// The identity and form of each logical ordinal of this pack.
    fn logical_keys(&self) -> CasResult<Vec<(LogicalKey, Identity)>> {
        let index = &self.index;
        let chunk = pack2::kind_code(StoreObjectKind::ChunkRaw);
        let manifest = pack2::kind_code(StoreObjectKind::FileManifest);
        let mut keys = Vec::with_capacity(usize::try_from(index.logical_count()).unwrap_or(0));
        for ordinal in 0..index.explicit_count() {
            let (code, id, _) = index
                .explicit(ordinal)
                .ok_or_else(|| pack2::pack2_corruption(&self.path, "pack record vanished"))?;
            keys.push((LogicalKey::Explicit(code, *id), (code, *id)));
        }
        for file in 0..index.files.len() {
            let record = index.files[file];
            keys.push((
                LogicalKey::ChunkAlias(record.file_id),
                (chunk, record.chunk_id),
            ));
            keys.push((
                LogicalKey::ManifestAlias(record.file_id),
                (manifest, self.alias_manifest_digest(file)?),
            ));
        }
        Ok(keys)
    }
}

impl NativeCas {
    /// Format 6, under the exclusive Store lock: writes the registered packs
    /// at `candidates` (positions in `catalog`) into one new pack and
    /// publishes it durably. Each block of each candidate is read once (its
    /// checksum checked), and every record copied is first verified against
    /// its id; a record held by several candidates is written once. The
    /// caller registers the new pack, remaps every admission bitmap and
    /// unregisters the old packs in one transaction. A failure names the
    /// candidate it came from, when one did, so it can be left out.
    pub(crate) fn consolidate_packs(
        &self,
        catalog: &RegisteredCatalog,
        candidates: &[usize],
    ) -> Result<Consolidation, ConsolidationFailure> {
        let _span = crate::perf::span("cas.consolidate");
        let general = |error| ConsolidationFailure { pack: None, error };
        let mut open = self
            .open_new_pack(self.inline_pack_limit().min(MERGED_INLINE_MAX))
            .map_err(general)?;
        let (packs, keys_of) = self.copy_candidates(catalog, candidates, &mut open)?;
        let (pack, inline, _, pack_keys) = self.publish_pack2(open).map_err(general)?;
        self.finish_consolidation(pack, inline, pack_keys, packs, keys_of)
    }

    /// Copies every record of the `candidates` (positions in `catalog`)
    /// into `open`, each block read once and each record verified against
    /// its id; a record held by several candidates is written once. Returns
    /// each candidate's registration and logical keys, for the remap.
    fn copy_candidates(
        &self,
        catalog: &RegisteredCatalog,
        candidates: &[usize],
        open: &mut OpenPack,
    ) -> Result<(Vec<CandidatePack>, CandidateKeys), ConsolidationFailure> {
        let general = |error| ConsolidationFailure { pack: None, error };
        let mut explicit_written: HashSet<(u8, [u8; 32])> = HashSet::new();
        let mut files_written: HashMap<[u8; 32], FileIdentity> = HashMap::new();
        // A File kept whole in one candidate and as a v1 file record in
        // another (received, or captured before format 7): the whole record
        // keeps the file record, the v1 chunk and manifest are copied as
        // explicit records (council s16 r1). Both identities stay.
        let whole_files: HashSet<[u8; 32]> = candidates
            .iter()
            .filter_map(|position| catalog.packs.get(*position))
            .flat_map(|registered| registered.pack.index.files.iter())
            .filter(|record| record.recipe == Recipe::Whole)
            .map(|record| record.file_id)
            .collect();
        let mut packs = Vec::with_capacity(candidates.len());
        let mut keys_of = Vec::with_capacity(candidates.len());
        for &position in candidates {
            // Only what the candidate's own bytes prove (corruption, a wrong
            // id, a registered file missing) names it; a plain I/O failure
            // (a scanner holding the file, a flaky volume) is general and
            // retried later (council s6 r3).
            let failed = |error: CasError| match error {
                error @ CasError::Io { .. } => ConsolidationFailure { pack: None, error },
                error => ConsolidationFailure {
                    pack: Some(position),
                    error,
                },
            };
            let registered = catalog.packs.get(position).ok_or_else(|| {
                failed(io_error(
                    "consolidate CAS packs",
                    &self.objects_dir,
                    io::Error::other("candidate pack is not registered"),
                ))
            })?;
            let loaded = &registered.pack;
            let index = &loaded.index;
            let keys = loaded.logical_keys().map_err(failed)?;
            for (block, records) in index.records_by_block().into_iter().enumerate() {
                if records.is_empty() {
                    continue;
                }
                let raw = catalog
                    .block(self, position, block as u32)
                    .map_err(failed)?;
                for record in records {
                    self.copy_record(
                        open,
                        loaded,
                        &raw,
                        record,
                        &whole_files,
                        &mut explicit_written,
                        &mut files_written,
                    )
                    .map_err(|failure| match failure {
                        CopyFailure::Source(error) => failed(error),
                        // A write error (disk full) blames no candidate.
                        CopyFailure::Target(error) => general(error),
                    })?;
                    self.spill_if_over(open).map_err(general)?;
                }
            }
            keys_of.push(keys);
            packs.push((
                registered.seq,
                loaded.name.clone(),
                index.digest,
                loaded.is_inline(),
            ));
        }
        Ok((packs, keys_of))
    }

    /// The consolidation of the published `pack` (with its `inline` image
    /// and keys row): each candidate's admitted ordinals remapped by
    /// identity into the new pack, for `consolidation::register`.
    fn finish_consolidation(
        &self,
        pack: PackRegistration,
        inline: Option<Vec<u8>>,
        pack_keys: super::keys::PackKeys,
        packs: Vec<CandidatePack>,
        keys_of: CandidateKeys,
    ) -> Result<Consolidation, ConsolidationFailure> {
        let general = |error| ConsolidationFailure { pack: None, error };
        let new = match &inline {
            Some(image) => self
                .inline_pack(&pack.0, Arc::from(image.as_slice()), true)
                .map_err(general)?,
            None => Arc::new(
                self.load_pack_from_disk(&std::cell::OnceCell::new(), &pack.0)
                    .map_err(general)?,
            ),
        };
        let index = &new.index;
        let missing = || {
            general(pack2::pack2_corruption(
                &new.path,
                "consolidated pack lacks a record",
            ))
        };
        let identities = new
            .logical_keys()
            .map_err(general)?
            .into_iter()
            .map(|(_, identity)| identity)
            .collect();
        let packs = packs
            .into_iter()
            .zip(keys_of)
            .map(|((seq, name, digest, inline), keys)| {
                let mut ordinals = Vec::with_capacity(keys.len());
                let mut old_identities = Vec::with_capacity(keys.len());
                for (key, identity) in keys {
                    // By identity: an alias only when the new file record
                    // stands for exactly this object, else the explicit
                    // record it was copied as (council s16 r1).
                    let explicit = || index.find(identity.0, &identity.1).map(|o| o as u64);
                    let ordinal = match key {
                        LogicalKey::Explicit(code, id) => {
                            index.find(code, &id).map(|ordinal| ordinal as u64)
                        }
                        LogicalKey::ChunkAlias(file_id) => index
                            .find_file(&file_id)
                            .filter(|file| index.files[*file].chunk_id == identity.1)
                            .map(|file| index.chunk_alias_ordinal(file))
                            .or_else(explicit),
                        LogicalKey::ManifestAlias(file_id) => index
                            .find_file(&file_id)
                            .filter(|file| {
                                new.alias_manifest_digest(*file)
                                    .is_ok_and(|digest| digest == identity.1)
                            })
                            .map(|file| index.manifest_alias_ordinal(file))
                            .or_else(explicit),
                    }
                    .ok_or_else(missing)?;
                    ordinals.push(ordinal);
                    old_identities.push(identity);
                }
                Ok(ConsolidatedPack {
                    seq,
                    name,
                    inline,
                    digest,
                    ordinals,
                    identities: old_identities,
                })
            })
            .collect::<Result<Vec<_>, ConsolidationFailure>>()?;
        Ok(Consolidation {
            keys: pack_keys,
            pack,
            inline,
            packs,
            identities,
        })
    }

    /// Builds, outside the exclusive Store lock (council s17, B2b), the
    /// consolidation of `candidates`: a sealed, synced and verified pack
    /// file under a temporary name in the packs directory, which orphan
    /// collection ignores (not a pack name) and which is unlinked if it is
    /// dropped unpublished. `publish_maintenance` publishes it under the
    /// exclusive lock.
    pub(crate) fn build_maintenance(
        &self,
        catalog: &RegisteredCatalog,
        candidates: &[usize],
    ) -> Result<BuiltPack, ConsolidationFailure> {
        let _span = crate::perf::span("cas.maintenance_build");
        let general = |error| ConsolidationFailure { pack: None, error };
        let mut open = self.open_maintenance_pack().map_err(general)?;
        let (packs, keys_of) = self.copy_candidates(catalog, candidates, &mut open)?;
        let builder = std::mem::replace(&mut open.builder, empty_builder());
        let sealed = builder
            .finish(&mut open.sink)
            .map_err(|error| general(io_error("write CAS pack index", &open.path, error)))?;
        open.file()
            .map_err(general)?
            .file()
            .sync_all()
            .map_err(|error| general(io_error("sync CAS pack", &open.path, error)))?;
        pack2::verify_written(open.file().map_err(general)?.file(), &open.path, &sealed)
            .map_err(general)?;
        Ok(BuiltPack {
            open,
            sealed,
            packs,
            keys_of,
        })
    }

    /// Under the exclusive Store lock: publishes `built` under a pack name
    /// (no clobber) and returns what `consolidation::register` registers.
    /// The caller checked first that every candidate is still registered as
    /// it was when the pack was built.
    pub(crate) fn publish_maintenance(
        &self,
        built: BuiltPack,
    ) -> Result<Consolidation, ConsolidationFailure> {
        let general = |error| ConsolidationFailure { pack: None, error };
        let BuiltPack {
            mut open,
            sealed,
            packs,
            keys_of,
        } = built;
        let path = open.path.clone();
        let objects = self.pinned_objects().map_err(general)?;
        let packs_path = self.objects_dir.join(PACKS_DIR);
        let packs_dir = objects
            .ensure_child_unsynced(OsStr::new(PACKS_DIR))
            .map_err(|error| general(io_error("open CAS packs directory", &packs_path, error)))?;
        let name = pack2::new_pack2_name();
        let target = packs_path.join(&name);
        if publish_no_clobber(open.file().map_err(general)?, &packs_dir, &name).map_err(general)? {
            return Err(general(io_error(
                "publish CAS pack",
                &target,
                io::Error::new(io::ErrorKind::AlreadyExists, "pack name collision"),
            )));
        }
        // From here the file is the published pack: never unlink it.
        open.published = true;
        let installed = packs_dir
            .open_file(&name)
            .map_err(|error| general(io_error("reread published CAS pack", &target, error)))?;
        if !open.file().map_err(general)?.same_file(&installed) {
            return Err(general(pack2::pack2_corruption(
                &target,
                "published pack is not the file written",
            )));
        }
        sync_directory_compatible(packs_dir.sync())
            .map_err(|error| general(io_error("sync CAS packs directory", &packs_path, error)))?;
        self.cleanup_published_staging(open.file().map_err(general)?, &path, false)
            .map_err(general)?;
        crate::perf::count("cas.maintenance_published", sealed.logical);
        self.finish_consolidation(
            (name, sealed.digest, sealed.logical),
            None,
            sealed.keys,
            packs,
            keys_of,
        )
    }

    /// A new pack file for maintenance, in the packs directory under a
    /// temporary name (`.maint-<id>.tmp`), with no staging lease: it is
    /// never staging, and orphan collection keeps only pack names.
    fn open_maintenance_pack(&self) -> CasResult<OpenPack> {
        let objects = self.pinned_objects()?;
        let packs_path = self.objects_dir.join(PACKS_DIR);
        let packs = objects
            .ensure_child_unsynced(OsStr::new(PACKS_DIR))
            .map_err(|error| io_error("open CAS packs directory", &packs_path, error))?;
        let mut last_error = None;
        for _ in 0..64 {
            let name = format!(".{MAINTENANCE_PREFIX}{}.tmp", staging_name());
            match packs.create_file(OsStr::new(&name)) {
                Ok(file) => {
                    let path = packs_path.join(&name);
                    let mut sink = PackSink::File { file, _lease: None };
                    let builder = Pack2Builder::start(&mut sink, self.whole_files)
                        .map_err(|error| io_error("write CAS pack", &path, error))?;
                    return Ok(OpenPack {
                        sink,
                        path,
                        builder,
                        published: false,
                        inline_limit: 0,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                }
                Err(error) => return Err(io_error("create CAS pack", &packs_path, error)),
            }
        }
        Err(io_error(
            "allocate CAS pack",
            &packs_path,
            last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "collision")),
        ))
    }

    /// Removes the temporary maintenance packs a crashed pass left behind.
    /// Only under the maintenance lock: no pass is building one.
    pub(crate) fn remove_stale_maintenance_packs(&self) -> CasResult<usize> {
        let packs_path = self.objects_dir.join(PACKS_DIR);
        let entries = match std::fs::read_dir(&packs_path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(io_error("list CAS packs directory", &packs_path, error)),
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name
                .to_str()
                .is_some_and(|name| name.starts_with(&format!(".{MAINTENANCE_PREFIX}")))
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Copies one record of `loaded` from its decompressed block `raw` into
    /// `open`, verified against its id, unless an equal one was copied.
    #[allow(clippy::too_many_arguments)]
    fn copy_record(
        &self,
        open: &mut OpenPack,
        loaded: &LoadedPack,
        raw: &[u8],
        record: pack2::RecordRef,
        whole_files: &HashSet<[u8; 32]>,
        explicit_written: &mut HashSet<(u8, [u8; 32])>,
        files_written: &mut HashMap<[u8; 32], FileIdentity>,
    ) -> Result<(), CopyFailure> {
        let index = &loaded.index;
        let path = open.path.clone();
        let write = |error| CopyFailure::Target(io_error("write CAS pack", &path, error));
        let sink = &mut open.sink;
        match record {
            pack2::RecordRef::Explicit(ordinal) => {
                let (code, id, location) = index
                    .explicit(ordinal)
                    .ok_or_else(|| pack2::pack2_corruption(&loaded.path, "pack record vanished"))?;
                let id = *id;
                if explicit_written.contains(&(code, id)) {
                    return Ok(());
                }
                let kind = pack2::kind_of_code(code)
                    .ok_or_else(|| pack2::pack2_corruption(&loaded.path, "unknown record kind"))?;
                let object_id = pack2::object_id_of(kind, &id)?;
                let body = pack2::record_body(raw, location, code, &loaded.path)?;
                let object = validate_stored_bytes(&object_id, body, self.whole_files)?;
                open.builder
                    .add_record(sink, kind, id, body, None)
                    .map_err(write)?;
                if let CanonicalObject::FileManifest(manifest) = &object {
                    let file_raw = pack2::hex_digest(
                        manifest
                            .file_id()
                            .as_str()
                            .strip_prefix("blake3:")
                            .unwrap_or_default(),
                    )
                    .ok_or_else(|| corruption(&object_id, "invalid FileId"))?;
                    open.builder
                        .add_manifest_file(file_raw, id, manifest.byte_length());
                }
                explicit_written.insert((code, id));
            }
            pack2::RecordRef::File(file) => {
                let record = index.files[file];
                let identity = FileIdentity {
                    file_id: record.file_id,
                    chunk_id: record.chunk_id,
                    recipe: record.recipe,
                    manifest_id: loaded.alias_manifest_digest(file)?,
                };
                // A v1 record of a File also kept whole is copied as explicit
                // records whichever comes first (council s16 r2): only the
                // whole record is a file record.
                let explicit =
                    record.recipe != Recipe::Whole && whole_files.contains(&record.file_id);
                if !explicit {
                    match files_written.get(&record.file_id) {
                        Some(written) if *written == identity => return Ok(()),
                        Some(_) => {
                            return Err(CopyFailure::Source(pack2::pack2_corruption(
                                &loaded.path,
                                "one FileId has two different file records",
                            )));
                        }
                        None => {}
                    }
                }
                let (_, chunk_id) = loaded.alias_ids(file)?;
                let body = pack2::record_body(raw, record.location, FILE_KIND, &loaded.path)?;
                validate_file_record_chunk(chunk_id, body, record.recipe)?;
                if explicit {
                    // Its File is kept whole too: this v1 chunk and manifest
                    // become explicit records.
                    let chunk_code = pack2::kind_code(StoreObjectKind::ChunkRaw);
                    if explicit_written.insert((chunk_code, record.chunk_id)) {
                        open.builder
                            .add_record(
                                sink,
                                StoreObjectKind::ChunkRaw,
                                record.chunk_id,
                                body,
                                None,
                            )
                            .map_err(write)?;
                    }
                    let manifest_code = pack2::kind_code(StoreObjectKind::FileManifest);
                    if explicit_written.insert((manifest_code, identity.manifest_id)) {
                        let manifest = loaded.alias_manifest(file)?;
                        let encoded = encode_object(&CanonicalObject::FileManifest(manifest))?;
                        open.builder
                            .add_record(
                                sink,
                                StoreObjectKind::FileManifest,
                                identity.manifest_id,
                                &encoded,
                                None,
                            )
                            .map_err(write)?;
                        open.builder.add_manifest_file(
                            record.file_id,
                            identity.manifest_id,
                            u64::from(record.location.length),
                        );
                    }
                    return Ok(());
                }
                open.builder
                    .add_file(sink, identity, body, None)
                    .map_err(write)?;
                files_written.insert(record.file_id, identity);
            }
        }
        Ok(())
    }

    /// Forgets the process cache of packs that are no longer registered.
    pub(crate) fn forget_packs(&self, names: &[OsString]) {
        if let Ok(mut cache) = pack_cache().lock() {
            for name in names {
                cache.remove(&(self.compact_key(), name.clone()));
            }
        }
    }
}

/// A registered pack missing on disk is corruption; other I/O stays I/O.
fn missing_or_io(operation: &'static str, path: &Path, error: io::Error) -> CasError {
    if error.kind() == io::ErrorKind::NotFound {
        pack2::pack2_corruption(path, "registered pack is missing")
    } else {
        io_error(operation, path, error)
    }
}

fn sql_error(error: rusqlite::Error) -> CasError {
    CasError::Io {
        operation: "read registered CAS catalog",
        path: PathBuf::from("metadata/layrs.sqlite3"),
        message: error.to_string(),
    }
}

/// A decompressed block a catalog keeps: ((pack, block), raw bytes).
type RecentBlock = ((usize, u32), Arc<Vec<u8>>);

/// One copy of an object in a registered catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Explicit record `ordinal` of registered pack `pack`.
    Record { pack: usize, ordinal: usize },
    /// The chunk a file record stands for.
    Chunk { pack: usize, file: usize },
    /// The recipe manifest a file record stands for.
    Manifest { pack: usize, file: usize },
    /// A loose file registered in `loose_objects`.
    Loose,
}

#[derive(Debug)]
pub(crate) struct RegisteredPack {
    pub(crate) seq: i64,
    pub(crate) entries: u64,
    pack: Arc<LoadedPack>,
}

impl RegisteredPack {
    /// Its registered metadata digest.
    pub(crate) fn digest(&self) -> [u8; 32] {
        self.pack.index.digest
    }

    /// Its file name in the packs directory.
    pub(crate) fn name(&self) -> &OsStr {
        &self.pack.name
    }
}

/// One `cas_packs` row, as registered (sound or not).
#[derive(Debug)]
struct RegisteredRow {
    seq: i64,
    name: OsString,
    digest: Vec<u8>,
    entries: i64,
    /// Its declared form (`stored_inline`: 1 inline, 0 file; anything else
    /// is an invalid row).
    form: i64,
    image: ImageRow,
}

/// Whether reading an inline image failed on what its stored bytes prove.
fn image_damage(error: &rusqlite::Error) -> bool {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) => matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
        ),
        rusqlite::Error::InvalidColumnType(..) | rusqlite::Error::FromSqlConversionFailure(..) => {
            true
        }
        _ => false,
    }
}

/// What `cas_pack_inline` holds for a registered pack.
#[derive(Debug)]
enum ImageRow {
    Absent,
    /// The image of a pack declared inline.
    Present(Arc<[u8]>),
    /// An image row that could not be read: damage to that pack only
    /// (council s8 r2).
    Unreadable(String),
    /// An image row beside a pack not declared inline (not read).
    Stray,
}

impl RegisteredRow {
    fn inline(&self) -> bool {
        self.form == 1
    }

    /// Where the pack's bytes are, for findings: its database row or its
    /// file, as its registration declares.
    fn location(&self, cas: &NativeCas) -> PathBuf {
        if self.inline() {
            cas.inline_label(&self.name)
        } else {
            cas.objects_dir.join(PACKS_DIR).join(&self.name)
        }
    }

    /// The pack in its declared form: an inline pack is read from its
    /// image (missing is corruption, never a file), a file pack from disk
    /// (an image beside it is corruption); an invalid row is never loaded.
    fn load(
        &self,
        cas: &NativeCas,
        packs: &std::cell::OnceCell<PinnedDirectory>,
        fresh: bool,
    ) -> CasResult<Arc<LoadedPack>> {
        let corrupt = |reason: &str| pack2::pack2_corruption(&self.location(cas), reason);
        if !pack2::valid_pack2_name(&self.name)
            || self.digest.len() != 32
            || self.entries < 0
            || !(0..=1).contains(&self.form)
        {
            return Err(corrupt("invalid cas_packs row"));
        }
        match (self.inline(), &self.image) {
            (true, ImageRow::Present(image)) => {
                cas.inline_pack(&self.name, Arc::clone(image), fresh)
            }
            (true, ImageRow::Unreadable(error)) => Err(corrupt(&format!(
                "inline pack image is unreadable: {error}"
            ))),
            (true, _) => Err(corrupt("inline pack image is missing")),
            (false, ImageRow::Absent) => cas.loaded_pack_in(packs, &self.name, fresh),
            (false, _) => Err(corrupt("pack file has an inline image")),
        }
    }
}

/// One physical copy an audit checked: a pack record or a loose object.
#[derive(Debug)]
pub(crate) struct AuditedCopy {
    pub(crate) path: PathBuf,
    /// The object id (for a file record: its chunk), or the pack itself.
    pub(crate) object: String,
    pub(crate) message_prefix: String,
    pub(crate) result: CasResult<()>,
    /// The whole registered pack is unreadable (its records unnamed).
    pub(crate) pack_level: bool,
}

/// An audit's next step: the bytes it is about to read and decode (so the
/// auditor authorizes them first, against its budgets), or a copy checked.
#[derive(Debug)]
pub(crate) enum AuditEvent {
    /// One pack block: its records' canonical bytes split into envelope
    /// (decoded) and chunk (hashed) bytes, from the validated index.
    Block {
        envelope_bytes: u64,
        chunk_bytes: u64,
    },
    /// One registered loose object of `bytes` canonical bytes.
    Loose {
        envelope: bool,
        bytes: u64,
    },
    Copy(AuditedCopy),
}

#[derive(Clone, Copy, Debug)]
struct LooseEntry {
    kind: u8,
    byte_length: u64,
}

/// Content admissions of one Space in one SQLite snapshot.
#[derive(Debug, Default)]
pub(crate) struct SpaceAdmissions {
    bitmaps: HashMap<i64, Vec<u8>>,
    loose: HashSet<[u8; 32]>,
}

/// A manifest of a File, as the catalog lists them.
#[derive(Clone, Debug)]
pub(crate) struct FileManifestEntry {
    pub(crate) id: FileManifestId,
    /// The manifest's file byte length (FileId section, file record or
    /// `loose_objects`), never read from the content.
    pub(crate) file_length: u64,
    pub(crate) placement: Placement,
}

/// The registered packs and loose objects of one SQLite snapshot, and the
/// content admissions of the Spaces read from it.
#[derive(Debug)]
pub(crate) struct RegisteredCatalog {
    packs: Vec<RegisteredPack>,
    /// Registered packs that cannot be read: (pack_seq, error).
    damaged: Vec<(i64, CasError)>,
    /// The key filters of the unreadable packs whose keys row is sound
    /// (council s17, B1b).
    damaged_filters: HashMap<i64, super::keys::KeyFilter>,
    /// Every `cas_packs` row of the snapshot, sound or damaged.
    rows: Vec<RegisteredRow>,
    /// Whether every pack was loaded from the metadata on disk now (an
    /// audit catalog) rather than from the process cache.
    fresh: bool,
    loose: BTreeMap<[u8; 32], LooseEntry>,
    /// FileId -> loose manifests (manifests over 1 MiB): raw manifest id and
    /// file length.
    loose_by_file: BTreeMap<[u8; 32], Vec<([u8; 32], u64)>>,
    admissions: Mutex<HashMap<String, Arc<SpaceAdmissions>>>,
    /// Last decompressed blocks: ((pack, block), raw bytes).
    recent: Mutex<VecDeque<RecentBlock>>,
    /// Pack files kept open while a bulk read holds this catalog
    /// (`hold_pack_files`), by pack position, at most `HELD_PACKS_MAX`;
    /// closed when the last hold ends (ADR 0026, council s9 r1). Opened for
    /// reading only, with every sharing mode, so a pack retired meanwhile can
    /// still be deleted; its bytes do not change (each block is checked
    /// against its checksum, each record against its id).
    open_packs: Mutex<HashMap<usize, Arc<PinnedFile>>>,
    /// Bulk reads holding this catalog's pack files.
    holds: std::sync::atomic::AtomicUsize,
}

/// A bulk read's hold on a catalog's pack files: while one is alive, the
/// catalog keeps the pack files it reads open instead of opening one per
/// block; they are closed when the last hold is dropped.
pub(crate) struct CatalogHold(Arc<RegisteredCatalog>);

impl CatalogHold {
    /// Whether this hold is on `catalog`.
    pub(crate) fn holds(&self, catalog: &Arc<RegisteredCatalog>) -> bool {
        Arc::ptr_eq(&self.0, catalog)
    }
}

impl Drop for CatalogHold {
    fn drop(&mut self) {
        let catalog = &self.0;
        // Under the lock a reader checks `holds` before keeping a file.
        let mut open = catalog
            .open_packs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if catalog
            .holds
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            held_packs().fetch_sub(open.len(), std::sync::atomic::Ordering::AcqRel);
            open.clear();
        }
    }
}

impl RegisteredCatalog {
    fn block(&self, cas: &NativeCas, pack: usize, block: u32) -> CasResult<Arc<Vec<u8>>> {
        if let Ok(recent) = self.recent.lock() {
            if let Some((_, bytes)) = recent.iter().find(|(key, _)| *key == (pack, block)) {
                return Ok(Arc::clone(bytes));
            }
        }
        let raw = Arc::new(self.read_pack_block(cas, pack, block)?);
        if raw.len() <= RECENT_BLOCK_MAX {
            if let Ok(mut recent) = self.recent.lock() {
                recent.push_front(((pack, block), Arc::clone(&raw)));
                recent.truncate(RECENT_BLOCKS);
            }
        }
        Ok(raw)
    }

    /// Test support: how many pack files this catalog keeps open now.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn held_pack_count(&self) -> usize {
        self.open_packs.lock().map_or(0, |open| open.len())
    }

    /// A hold on this catalog's pack files for a bulk read (see
    /// `CatalogHold`).
    pub(crate) fn hold_pack_files(self: &Arc<Self>) -> CatalogHold {
        let _open = self
            .open_packs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.holds.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        CatalogHold(Arc::clone(self))
    }

    /// Block `block` of pack `pack`. Without a hold the pack file is opened
    /// for this read only (ADR 0026); while one is held, through the file
    /// kept open (opened on first use, at most `HELD_PACKS_MAX`).
    fn read_pack_block(&self, cas: &NativeCas, pack: usize, block: u32) -> CasResult<Vec<u8>> {
        let loaded = &self.packs[pack].pack;
        let held = || self.holds.load(std::sync::atomic::Ordering::Acquire) > 0;
        if loaded.is_inline() || !held() {
            return loaded.read_block(cas, block);
        }
        let kept = self
            .open_packs
            .lock()
            .ok()
            .and_then(|open| open.get(&pack).cloned());
        let file = match kept {
            Some(file) => file,
            None => {
                let _span = crate::perf::span("cas.pack_open");
                let file = Arc::new(loaded.open(cas)?);
                if let Ok(mut open) = self.open_packs.lock() {
                    let reserved = || {
                        held_packs()
                            .fetch_update(
                                std::sync::atomic::Ordering::AcqRel,
                                std::sync::atomic::Ordering::Acquire,
                                |kept| (kept < HELD_PACKS_PROCESS_MAX).then_some(kept + 1),
                            )
                            .is_ok()
                    };
                    if held()
                        && !open.contains_key(&pack)
                        && open.len() < HELD_PACKS_MAX
                        && reserved()
                    {
                        open.insert(pack, Arc::clone(&file));
                    }
                }
                file
            }
        };
        pack2::read_block(file.file(), &loaded.path, &loaded.index, block)
    }

    fn body(
        &self,
        cas: &NativeCas,
        pack: usize,
        kind: u8,
        location: Location,
    ) -> CasResult<Vec<u8>> {
        let block = self.block(cas, pack, location.block)?;
        let path = &self.packs[pack].pack.path;
        Ok(pack2::record_body(&block, location, kind, path)?.to_vec())
    }

    /// Reads `cas_packs` and `loose_objects` from `connection` (one SQLite
    /// snapshot) and binds every registered pack to its validated index.
    pub(crate) fn load(connection: &Connection, cas: &NativeCas) -> CasResult<Self> {
        Self::load_with(connection, cas, false)
    }

    /// `load` from the metadata on disk now, never from the process cache:
    /// what audits (fsck) use.
    pub(crate) fn load_fresh(connection: &Connection, cas: &NativeCas) -> CasResult<Self> {
        Self::load_with(connection, cas, true)
    }

    fn load_with(connection: &Connection, cas: &NativeCas, fresh: bool) -> CasResult<Self> {
        let _span = crate::perf::span("cas.compact_catalog");
        let mut statement = connection
            .prepare(
                "SELECT pack_seq, name, index_digest, entries, stored_inline \
                 FROM cas_packs ORDER BY pack_seq",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_error)?;
        // Which packs have an image row (their sequence numbers only); each
        // image is then read alone, in this same snapshot, so one unreadable
        // image damages its pack only (council s8 r2).
        // With their lengths, so an image past the limit is damage to its
        // pack without being read (council s8 r3).
        let with_image: HashMap<i64, i64> = connection
            .prepare("SELECT pack_seq, length(image) FROM cas_pack_inline")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
                    .collect::<rusqlite::Result<HashMap<_, _>>>()
            })
            .map_err(sql_error)?;
        let mut read_image = connection
            .prepare("SELECT image FROM cas_pack_inline WHERE pack_seq=?1")
            .map_err(sql_error)?;
        let mut packs = Vec::with_capacity(rows.len());
        let mut damaged = Vec::new();
        let mut registered_rows = Vec::with_capacity(rows.len());
        let packs_directory = std::cell::OnceCell::new();
        // The process cache keeps only this Store's registered packs: one a
        // consolidation retired (here or in another process) is forgotten.
        if let Ok(mut cache) = pack_cache().lock() {
            let key = cas.compact_key();
            let registered: HashSet<&str> = rows.iter().map(|row| row.1.as_str()).collect();
            cache.retain(|(store, name), _| {
                store != &key || name.to_str().is_some_and(|name| registered.contains(name))
            });
        }
        for (seq, name, digest, entries, form) in rows {
            let image = match (with_image.get(&seq), form == 1) {
                (None, _) => ImageRow::Absent,
                (Some(_), false) => ImageRow::Stray,
                (Some(&length), true)
                    if usize::try_from(length)
                        .is_ok_and(|length| (1..=MAX_INLINE_IMAGE_BYTES).contains(&length)) =>
                {
                    match read_image.query_row([seq], |row| row.get::<_, Vec<u8>>(0)) {
                        Ok(image) => ImageRow::Present(Arc::from(image)),
                        // What the stored bytes prove (a damaged page, a
                        // value of another type) damages this pack only; any
                        // other error (I/O, busy, memory) fails the catalog,
                        // to be retried (council s8 r3).
                        Err(error) if image_damage(&error) => {
                            ImageRow::Unreadable(error.to_string())
                        }
                        Err(error) => return Err(sql_error(error)),
                    }
                }
                (Some(length), true) => {
                    ImageRow::Unreadable(format!("image of {length} bytes is out of bounds"))
                }
            };
            let row = RegisteredRow {
                seq,
                name: OsString::from(name),
                digest,
                entries,
                form,
                image,
            };
            let checked = row.load(cas, &packs_directory, fresh).and_then(|pack| {
                check_registration(&pack, &row.name, &row.digest, row.entries).map(|()| pack)
            });
            let entries = row.entries;
            registered_rows.push(row);
            match checked {
                Ok(pack) => packs.push(RegisteredPack {
                    seq,
                    entries: entries as u64,
                    pack,
                }),
                Err(error) => damaged.push((seq, error)),
            }
        }
        drop(read_image);
        drop(statement);
        let mut statement = connection
            .prepare(
                "SELECT object_id, object_kind, byte_length, file_id, file_byte_length \
                 FROM loose_objects",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })
            .map_err(sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_error)?;
        let mut loose = BTreeMap::new();
        let mut loose_by_file: BTreeMap<[u8; 32], Vec<([u8; 32], u64)>> = BTreeMap::new();
        let invalid = || pack2::pack2_corruption(&cas.objects_dir, "invalid loose_objects row");
        for (id, kind, byte_length, file_id, file_length) in rows {
            let id: [u8; 32] = id.try_into().map_err(|_| invalid())?;
            let kind = u8::try_from(kind).map_err(|_| invalid())?;
            let byte_length = u64::try_from(byte_length).map_err(|_| invalid())?;
            match (kind, file_id, file_length) {
                (1, None, None) => {}
                (2, Some(file_id), Some(file_length)) => {
                    let file_id: [u8; 32] = file_id.try_into().map_err(|_| invalid())?;
                    let file_length = u64::try_from(file_length).map_err(|_| invalid())?;
                    loose_by_file
                        .entry(file_id)
                        .or_default()
                        .push((id, file_length));
                }
                _ => return Err(invalid()),
            }
            loose.insert(id, LooseEntry { kind, byte_length });
        }
        let damaged_filters = damaged_filters(connection, &damaged, &registered_rows);
        Ok(Self {
            packs,
            damaged,
            damaged_filters,
            fresh,
            rows: registered_rows,
            loose,
            loose_by_file,
            admissions: Mutex::new(HashMap::new()),
            recent: Mutex::new(VecDeque::with_capacity(RECENT_BLOCKS)),
            open_packs: Mutex::new(HashMap::new()),
            holds: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub(crate) fn packs(&self) -> &[RegisteredPack] {
        &self.packs
    }

    /// Whether this catalog was read from the database now (an audit).
    pub(crate) fn is_fresh(&self) -> bool {
        self.fresh
    }

    /// An explicit record `id` from one of the `limit` most recently
    /// registered packs, without loading the others: its stored length and
    /// the object, verified against `id`. `None` whenever this cannot
    /// answer (not in those packs, one of them unreadable, any SQLite
    /// error): the caller then reads through the whole catalog, which keeps
    /// every rule of a complete answer. A positive answer needs no other
    /// pack: the copy is registered and verified against its content address.
    /// Most reads of a command are of its newest objects (a View, a Step),
    /// so the large old packs are not even loaded for them.
    pub(crate) fn read_recent(
        connection: &Connection,
        cas: &NativeCas,
        id: &StoreObjectId,
        limit: usize,
    ) -> Option<(u64, CanonicalObject)> {
        let _span = crate::perf::span("cas.read_recent");
        let raw = pack2::raw_digest(id).ok()?;
        let code = pack2::kind_code(id.kind());
        let mut rows = connection
            .prepare_cached(
                "SELECT pack_seq, name, index_digest, entries, stored_inline FROM cas_packs \
                 ORDER BY pack_seq DESC LIMIT ?1",
            )
            .ok()?;
        let rows: Vec<(i64, String, Vec<u8>, i64, i64)> = rows
            .query_map([i64::try_from(limit).ok()?], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .ok()?
            .collect::<rusqlite::Result<_>>()
            .ok()?;
        let packs = std::cell::OnceCell::new();
        for (seq, name, digest, entries, form) in rows {
            let image = if form == 1 {
                let image: Vec<u8> = connection
                    .query_row(
                        "SELECT image FROM cas_pack_inline WHERE pack_seq=?1",
                        [seq],
                        |row| row.get(0),
                    )
                    .ok()?;
                ImageRow::Present(Arc::from(image))
            } else {
                ImageRow::Absent
            };
            let row = RegisteredRow {
                seq,
                name: OsString::from(name),
                digest,
                entries,
                form,
                image,
            };
            let loaded = row.load(cas, &packs, false).ok()?;
            check_registration(&loaded, &row.name, &row.digest, row.entries).ok()?;
            let Some(ordinal) = loaded.index.find(code, &raw) else {
                continue;
            };
            let (kind, _, location) = loaded.index.explicit(ordinal)?;
            let block = loaded.read_block(cas, location.block).ok()?;
            let body = pack2::record_body(&block, location, kind, &loaded.path).ok()?;
            let object = validate_stored_bytes(id, body, cas.whole_files).ok()?;
            crate::perf::count("cas.read_recent_hit", 1);
            return Some((u64::from(location.length), object));
        }
        None
    }

    /// The first unreadable registered pack, if any.
    pub(crate) fn damage(&self) -> Option<&CasError> {
        self.damaged.first().map(|(_, error)| error)
    }

    /// `Ok(value)` when the catalog answers exactly about `keys` (the
    /// keys the question asks): every unreadable registered pack's keys row
    /// excludes them all (council s17, B1b). Otherwise a negative or
    /// complete answer is refused with that pack's error: never absent,
    /// never present.
    fn complete<T>(
        &self,
        keys: impl FnOnce() -> CasResult<Vec<super::keys::Key>>,
        value: T,
    ) -> CasResult<T> {
        match self.unknown_to(keys)? {
            Some(error) => Err(error.clone()),
            None => Ok(value),
        }
    }

    /// The error of the first unreadable pack that may hold one of `keys`
    /// (built only when a pack is unreadable).
    pub(crate) fn unknown_to(
        &self,
        keys: impl FnOnce() -> CasResult<Vec<super::keys::Key>>,
    ) -> CasResult<Option<&CasError>> {
        if self.damaged.is_empty() {
            return Ok(None);
        }
        let keys = keys()?;
        Ok(self
            .damaged
            .iter()
            .find(|(seq, _)| {
                self.damaged_filters
                    .get(seq)
                    .is_none_or(|filter| keys.iter().any(|key| filter.may_contain(key)))
            })
            .map(|(_, error)| error))
    }

    /// `complete`, or for a `positive` question (a writer's: council s17,
    /// B1a) the answer of the readable packs alone: what they prove, where
    /// "not found" means "not proven", never "absent". A writer then keeps
    /// or writes its own copy, and a closure only they could prove fails.
    fn settle<T>(
        &self,
        positive: bool,
        keys: impl FnOnce() -> CasResult<Vec<super::keys::Key>>,
        value: T,
    ) -> CasResult<T> {
        if positive {
            Ok(value)
        } else {
            self.complete(keys, value)
        }
    }

    /// Every copy of `id` in this catalog, packs in registration order then
    /// a loose file.
    pub(crate) fn placements(&self, id: &StoreObjectId) -> CasResult<Vec<Placement>> {
        let digest = pack2::raw_digest(id)?;
        let code = pack2::kind_code(id.kind());
        let mut found = Vec::new();
        for (pack, registered) in self.packs.iter().enumerate() {
            let loaded = &registered.pack;
            if let Some(ordinal) = loaded.index.find(code, &digest) {
                found.push(Placement::Record { pack, ordinal });
            }
            match id.kind() {
                StoreObjectKind::ChunkRaw => {
                    if let Some(file) = loaded.index.find_chunk_alias(&digest) {
                        found.push(Placement::Chunk { pack, file });
                    }
                }
                StoreObjectKind::FileManifest => {
                    for file in loaded.index.manifest_alias_candidates(&digest) {
                        if loaded.alias_manifest_digest(file)? == digest {
                            found.push(Placement::Manifest { pack, file });
                        }
                    }
                }
                _ => {}
            }
        }
        if self
            .loose
            .get(&digest)
            .is_some_and(|entry| entry.kind == code)
        {
            found.push(Placement::Loose);
        }
        Ok(found)
    }

    /// Whether some copy of `id` is registered (never content proof).
    pub(crate) fn contains(&self, id: &StoreObjectId) -> CasResult<bool> {
        self.contains_with(id, false)
    }

    /// `contains`, `positive` as in `settle`.
    pub(crate) fn contains_with(&self, id: &StoreObjectId, positive: bool) -> CasResult<bool> {
        if self.placements(id)?.is_empty() {
            return self.settle(positive, || object_keys(id), false);
        }
        Ok(true)
    }

    /// Canonical byte length of `id`'s first copy, from metadata only.
    pub(crate) fn stored_length(&self, id: &StoreObjectId) -> CasResult<Option<u64>> {
        let Some(placement) = self.placements(id)?.into_iter().next() else {
            return self.complete(|| object_keys(id), None);
        };
        self.placement_length(id, placement).map(Some)
    }

    fn placement_length(&self, id: &StoreObjectId, placement: Placement) -> CasResult<u64> {
        Ok(match placement {
            Placement::Record { pack, ordinal } => {
                let (_, _, location) = self.packs[pack]
                    .pack
                    .index
                    .explicit(ordinal)
                    .ok_or_else(|| corruption(id, "pack record vanished"))?;
                u64::from(location.length)
            }
            Placement::Chunk { pack, file } => {
                u64::from(self.packs[pack].pack.index.files[file].location.length)
            }
            Placement::Manifest { pack, file } => encode_object(&CanonicalObject::FileManifest(
                self.packs[pack].pack.alias_manifest(file)?,
            ))?
            .len() as u64,
            Placement::Loose => self
                .loose
                .get(&pack2::raw_digest(id)?)
                .map(|entry| entry.byte_length)
                .ok_or_else(|| corruption(id, "loose object vanished"))?,
        })
    }

    /// The stored bytes of a pack placement (an explicit record or the chunk
    /// a file record stands for), unverified; `None` for other placements.
    fn stored_bytes(&self, cas: &NativeCas, placement: Placement) -> CasResult<Option<Vec<u8>>> {
        match placement {
            Placement::Record { pack, ordinal } => {
                let Some((kind, _, location)) = self.packs[pack].pack.index.explicit(ordinal)
                else {
                    return Ok(None);
                };
                self.body(cas, pack, kind, location).map(Some)
            }
            Placement::Chunk { pack, file } => {
                let location = self.packs[pack].pack.index.files[file].location;
                self.body(cas, pack, FILE_KIND, location).map(Some)
            }
            Placement::Manifest { .. } | Placement::Loose => Ok(None),
        }
    }

    fn read_placement(
        &self,
        cas: &NativeCas,
        id: &StoreObjectId,
        placement: Placement,
    ) -> CasResult<CanonicalObject> {
        match placement {
            Placement::Record { pack, ordinal } => {
                let loaded = &self.packs[pack].pack;
                let (kind, _, location) = loaded
                    .index
                    .explicit(ordinal)
                    .ok_or_else(|| corruption(id, "pack record vanished"))?;
                validate_stored_bytes(id, &self.body(cas, pack, kind, location)?, cas.whole_files)
            }
            Placement::Chunk { pack, file } => {
                let loaded = &self.packs[pack].pack;
                let record = loaded.index.files[file];
                let (_, chunk_id) = loaded.alias_ids(file)?;
                let object = validate_file_record_chunk(
                    chunk_id,
                    &self.body(cas, pack, FILE_KIND, record.location)?,
                    record.recipe,
                )?;
                if &object.object_id() != id {
                    return Err(CasError::IdMismatch {
                        expected: id.clone(),
                        actual: object.object_id().to_string(),
                    });
                }
                Ok(object)
            }
            Placement::Manifest { pack, file } => {
                let manifest = self.packs[pack].pack.alias_manifest(file)?;
                let actual = StoreObjectId::FileManifest(manifest.id().clone());
                if &actual != id {
                    return Err(CasError::IdMismatch {
                        expected: id.clone(),
                        actual: actual.to_string(),
                    });
                }
                Ok(CanonicalObject::FileManifest(manifest))
            }
            Placement::Loose => {
                let entry = self
                    .loose
                    .get(&pack2::raw_digest(id)?)
                    .copied()
                    .ok_or_else(|| corruption(id, "loose object vanished"))?;
                let object = cas.read_loose(id).map_err(|error| match error {
                    CasError::NotFound { .. } => {
                        corruption(id, "registered loose object is missing")
                    }
                    other => other,
                })?;
                if encode_object(&object)?.len() as u64 != entry.byte_length {
                    return Err(corruption(id, "loose object length differs from its row"));
                }
                Ok(object)
            }
        }
    }

    /// The first copy of `id` that verifies against `id`. `Ok(None)` when no
    /// copy is registered (and the catalog is complete); when every copy
    /// fails, the first error.
    pub(crate) fn read(
        &self,
        cas: &NativeCas,
        id: &StoreObjectId,
    ) -> CasResult<Option<CanonicalObject>> {
        self.read_with(cas, id, false)
    }

    /// `read`, `positive` as in `settle`.
    pub(crate) fn read_with(
        &self,
        cas: &NativeCas,
        id: &StoreObjectId,
        positive: bool,
    ) -> CasResult<Option<CanonicalObject>> {
        let placements = self.placements(id)?;
        if placements.is_empty() {
            return self.settle(positive, || object_keys(id), None);
        }
        let mut first_error = None;
        for placement in placements {
            match self.read_placement(cas, id, placement) {
                Ok(object) => return Ok(Some(object)),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(first_error.unwrap_or_else(|| corruption(id, "no readable copy")))
    }

    /// Streams a verified chunk: a pack copy is verified before `consume`
    /// sees it; a loose file is streamed and verified as it is read.
    pub(crate) fn stream_chunk(
        &self,
        cas: &NativeCas,
        chunk_id: &ChunkId,
        consume: &mut impl FnMut(&[u8]),
    ) -> CasResult<u64> {
        let id = StoreObjectId::ChunkRaw(chunk_id.clone());
        let placements = self.placements(&id)?;
        if placements.is_empty() {
            self.complete(|| object_keys(&id), ())?;
            return Err(CasError::NotFound {
                kind: StoreObjectKind::ChunkRaw,
                id: chunk_id.to_string(),
            });
        }
        let mut first_error = None;
        for placement in placements {
            if placement == Placement::Loose {
                return cas.stream_loose_chunk(chunk_id, &id, consume);
            }
            match self.read_placement(cas, &id, placement) {
                Ok(CanonicalObject::ChunkRaw(chunk)) => {
                    consume(chunk.bytes());
                    return Ok(chunk.byte_length());
                }
                Ok(_) => {
                    first_error.get_or_insert(corruption(&id, "pack copy is not a chunk"));
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(first_error.unwrap_or_else(|| corruption(&id, "no readable copy")))
    }

    /// The smallest length recorded for `file_id` among its manifests (what
    /// `file_manifests` lists), without building each manifest's id.
    /// `positive` as in `settle`.
    pub(crate) fn file_length_with(
        &self,
        file_id: &FileId,
        positive: bool,
    ) -> CasResult<Option<u64>> {
        let raw = file_id.digest_bytes();
        let mut length: Option<u64> = None;
        let mut keep = |value: u64| {
            length = Some(length.map_or(value, |current: u64| current.min(value)));
        };
        for registered in &self.packs {
            let loaded = &registered.pack;
            for entry in loaded.index.fileid_manifests(&raw) {
                // The FileId-section checks of `file_manifests`. A file
                // record's length is its record's: its alias manifest is not
                // rebuilt here (that is what this lookup saves), so a record
                // whose recipe is damaged answers a length, and its manifest
                // lookup the corruption.
                let ordinal = loaded
                    .index
                    .manifest_ordinal(entry.manifest)
                    .ok_or_else(|| {
                        pack2::pack2_corruption(&loaded.path, "FileId entry names no manifest")
                    })?;
                loaded.index.explicit(ordinal).ok_or_else(|| {
                    pack2::pack2_corruption(&loaded.path, "FileId entry names no manifest")
                })?;
                keep(entry.byte_length);
            }
            if let Some(file) = loaded.index.find_file(&raw) {
                keep(u64::from(loaded.index.files[file].location.length));
            }
        }
        for (_, file_length) in self.loose_by_file.get(&raw).into_iter().flatten() {
            keep(*file_length);
        }
        // A File's length is set by its id: one a readable pack records is
        // the answer whatever an unreadable one holds (council s17, B1a).
        if length.is_some() {
            return Ok(length);
        }
        self.settle(positive, || Ok(file_keys(file_id)), length)
    }

    /// Every registered manifest of `file_id`, sorted by id, with its file
    /// length: FileId sections, file records and loose manifests. Index
    /// answers only; no content is read.
    pub(crate) fn file_manifests(&self, file_id: &FileId) -> CasResult<Vec<FileManifestEntry>> {
        self.file_manifests_with(file_id, false)
    }

    /// `file_manifests`, `positive` as in `settle`: the readable packs'.
    pub(crate) fn file_manifests_with(
        &self,
        file_id: &FileId,
        positive: bool,
    ) -> CasResult<Vec<FileManifestEntry>> {
        let raw = file_id.digest_bytes();
        let mut found: BTreeMap<FileManifestId, FileManifestEntry> = BTreeMap::new();
        let parse = |path: &Path, digest: &[u8; 32]| {
            FileManifestId::parse(pack2::digest_text(digest))
                .map_err(|error| pack2::pack2_corruption(path, error.to_string()))
        };
        for (pack, registered) in self.packs.iter().enumerate() {
            let loaded = &registered.pack;
            for entry in loaded.index.fileid_manifests(&raw) {
                let ordinal = loaded
                    .index
                    .manifest_ordinal(entry.manifest)
                    .ok_or_else(|| {
                        pack2::pack2_corruption(&loaded.path, "FileId entry names no manifest")
                    })?;
                let (_, digest, _) = loaded.index.explicit(ordinal).ok_or_else(|| {
                    pack2::pack2_corruption(&loaded.path, "FileId entry names no manifest")
                })?;
                let id = parse(&loaded.path, digest)?;
                found.entry(id.clone()).or_insert(FileManifestEntry {
                    id,
                    file_length: entry.byte_length,
                    placement: Placement::Record { pack, ordinal },
                });
            }
            if let Some(file) = loaded.index.find_file(&raw) {
                let id = parse(&loaded.path, &loaded.alias_manifest_digest(file)?)?;
                found.entry(id.clone()).or_insert(FileManifestEntry {
                    id,
                    file_length: u64::from(loaded.index.files[file].location.length),
                    placement: Placement::Manifest { pack, file },
                });
            }
        }
        for (digest, file_length) in self.loose_by_file.get(&raw).into_iter().flatten() {
            let id = parse(&self_path(), digest)?;
            found.entry(id.clone()).or_insert(FileManifestEntry {
                id,
                file_length: *file_length,
                placement: Placement::Loose,
            });
        }
        self.settle(
            positive,
            || Ok(file_keys(file_id)),
            found.into_values().collect(),
        )
    }

    /// Whether a registered whole file record is named `chunk_id` (its
    /// chunk is then in the file domain).
    pub(crate) fn whole_record(&self, chunk_id: &ChunkId) -> bool {
        let raw = chunk_id.digest_bytes();
        self.packs.iter().any(|registered| {
            registered
                .pack
                .index
                .find_file(&raw)
                .is_some_and(|file| registered.pack.index.files[file].recipe == Recipe::Whole)
        })
    }

    /// The chunk the file record `file` of the registered pack at `pack`
    /// stands for.
    pub(crate) fn alias_chunk(&self, pack: usize, file: usize) -> CasResult<ChunkId> {
        let registered = self.packs.get(pack).ok_or_else(|| {
            pack2::pack2_corruption(&self_path(), "file record of an unknown pack")
        })?;
        if file >= registered.pack.index.files.len() {
            return Err(pack2::pack2_corruption(
                &registered.pack.path,
                "unknown file record",
            ));
        }
        registered.pack.alias_ids(file).map(|(_, chunk)| chunk)
    }

    /// An exact position naming `file_id`'s file record, for derived data
    /// (the projection cache) to hold instead of its 32-byte id: the digest
    /// of a large registered pack and the record's ordinal in it. Only packs
    /// of at least `REFERENCED_PACK_RECORDS` records are named, which
    /// consolidation rarely replaces; `None` otherwise.
    pub(crate) fn file_record_ref(&self, file_id: &FileId) -> Option<([u8; 32], u32)> {
        let raw = file_id.digest_bytes();
        self.packs
            .iter()
            .filter(|registered| registered.pack.index.files.len() >= REFERENCED_PACK_RECORDS)
            .find_map(|registered| {
                let file = registered.pack.index.find_file(&raw)?;
                Some((registered.pack.index.digest, u32::try_from(file).ok()?))
            })
    }

    /// The FileId and length `file_record_ref` named, while that pack is
    /// registered; `None` once it is not (or for a position it never had).
    pub(crate) fn resolve_file_record(
        &self,
        pack: &[u8; 32],
        ordinal: u32,
    ) -> Option<(FileId, u64)> {
        let registered = self
            .packs
            .iter()
            .find(|registered| &registered.pack.index.digest == pack)?;
        let record = registered
            .pack
            .index
            .files
            .get(usize::try_from(ordinal).ok()?)?;
        Some((
            FileId::from_digest_bytes(&record.file_id),
            u64::from(record.location.length),
        ))
    }

    /// Every registered content object: explicit content records, the chunk
    /// and the manifest each file record stands for, and loose content.
    /// Records whose identity cannot be rebuilt are returned as errors.
    pub(crate) fn content_objects(&self) -> (Vec<StoreObjectId>, Vec<CasError>) {
        let mut ids = Vec::new();
        let mut errors = Vec::new();
        for registered in &self.packs {
            let loaded = &registered.pack;
            for kind in [StoreObjectKind::ChunkRaw, StoreObjectKind::FileManifest] {
                for ordinal in loaded.index.kind_range(pack2::kind_code(kind)) {
                    if let Some((_, digest, _)) = loaded.index.explicit(ordinal) {
                        match pack2::object_id_of(kind, digest) {
                            Ok(id) => ids.push(id),
                            Err(error) => errors.push(error),
                        }
                    }
                }
            }
            for file in 0..loaded.index.files.len() {
                match loaded.alias_ids(file) {
                    Ok((_, chunk)) => ids.push(StoreObjectId::ChunkRaw(chunk)),
                    Err(error) => errors.push(error),
                }
                match loaded
                    .alias_manifest_digest(file)
                    .and_then(|digest| pack2::object_id_of(StoreObjectKind::FileManifest, &digest))
                {
                    Ok(id) => ids.push(id),
                    Err(error) => errors.push(error),
                }
            }
        }
        for (digest, entry) in &self.loose {
            match pack2::kind_of_code(entry.kind)
                .ok_or_else(|| pack2::pack2_corruption(&self_path(), "loose object kind"))
                .and_then(|kind| pack2::object_id_of(kind, digest))
            {
                Ok(id) => ids.push(id),
                Err(error) => errors.push(error),
            }
        }
        (ids, errors)
    }

    /// Logical admission ordinals of `id`'s pack copies: (pack_seq, ordinal).
    /// `loose` tells whether a registered loose copy exists.
    pub(crate) fn logical_ordinals(
        &self,
        id: &StoreObjectId,
    ) -> CasResult<(Vec<(i64, u64)>, bool)> {
        let mut ordinals = Vec::new();
        let mut loose = false;
        for placement in self.placements(id)? {
            match self.logical(placement) {
                Some(ordinal) => ordinals.push(ordinal),
                None => loose = true,
            }
        }
        Ok((ordinals, loose))
    }

    fn logical(&self, placement: Placement) -> Option<(i64, u64)> {
        let (pack, ordinal) = match placement {
            Placement::Record { pack, ordinal } => (pack, ordinal as u64),
            Placement::Chunk { pack, file } => {
                (pack, self.packs[pack].pack.index.chunk_alias_ordinal(file))
            }
            Placement::Manifest { pack, file } => (
                pack,
                self.packs[pack].pack.index.manifest_alias_ordinal(file),
            ),
            Placement::Loose => return None,
        };
        Some((self.packs[pack].seq, ordinal))
    }

    /// Content admissions of `space` in this snapshot, read once.
    pub(crate) fn space_admissions(
        &self,
        connection: &Connection,
        space: &str,
    ) -> CasResult<Arc<SpaceAdmissions>> {
        if let Some(found) = self
            .admissions
            .lock()
            .ok()
            .and_then(|cache| cache.get(space).cloned())
        {
            return Ok(found);
        }
        let mut admissions = SpaceAdmissions::default();
        let mut statement = connection
            .prepare_cached(
                "SELECT pack_seq, bitmap FROM space_pack_admissions WHERE space_id = ?1",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([space], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql_error)?;
        for row in rows {
            let (seq, bitmap) = row.map_err(sql_error)?;
            if let Some(registered) = self.packs.iter().find(|pack| pack.seq == seq) {
                check_bitmap(&bitmap, registered.entries)
                    .map_err(|reason| pack2::pack2_corruption(&registered.pack.path, reason))?;
            } else if !self.damaged.iter().any(|(damaged, _)| *damaged == seq) {
                return Err(pack2::pack2_corruption(
                    &self_path(),
                    "an admission bitmap names an unregistered pack",
                ));
            }
            admissions.bitmaps.insert(seq, bitmap);
        }
        drop(statement);
        let mut statement = connection
            .prepare_cached("SELECT object_id FROM space_loose_admissions WHERE space_id = ?1")
            .map_err(sql_error)?;
        let rows = statement
            .query_map([space], |row| row.get::<_, Vec<u8>>(0))
            .map_err(sql_error)?;
        for row in rows {
            let id: [u8; 32] = row
                .map_err(sql_error)?
                .try_into()
                .map_err(|_| pack2::pack2_corruption(&self_path(), "invalid loose admission"))?;
            admissions.loose.insert(id);
        }
        let admissions = Arc::new(admissions);
        if let Ok(mut cache) = self.admissions.lock() {
            cache.insert(space.to_owned(), Arc::clone(&admissions));
        }
        Ok(admissions)
    }

    pub(crate) fn placement_admitted(
        &self,
        admissions: &SpaceAdmissions,
        id: &StoreObjectId,
        placement: Placement,
    ) -> CasResult<bool> {
        Ok(match self.logical(placement) {
            Some((seq, ordinal)) => admissions
                .bitmaps
                .get(&seq)
                .is_some_and(|bitmap| bit(bitmap, ordinal)),
            None => admissions.loose.contains(&pack2::raw_digest(id)?),
        })
    }

    /// Whether some registered manifest of `file_id` is admitted to the
    /// Space whose admissions these are, decided from its placements' bits
    /// alone: no manifest id is built (council s13 Q2). Every copy of a
    /// manifest is reachable from its FileId (FileId sections, file records,
    /// loose manifests). Positive answers only: `false` also when the File
    /// has more placements than `max_manifests` (the exact path then
    /// reports it); a damaged catalog answers false itself (the exact path then
    /// reports it, council s14 r1).
    ///
    /// A file record's manifest bit is never set unless its alias rebuilt
    /// to that manifest's id: admission sets it only through `placements`
    /// (rebuilt digest compared), consolidation remaps it by identity and
    /// reads it back, and the pack is immutable under its registered digest.
    /// So the bit alone proves what the rebuilt alias would (council s14
    /// r1: the rebuild, the cost removed here, is what fsck still checks).
    pub(crate) fn file_manifest_admitted(
        &self,
        admissions: &SpaceAdmissions,
        file_id: &FileId,
        max_manifests: usize,
    ) -> CasResult<bool> {
        // An unreadable pack that may hold a manifest of the File leaves its
        // manifests unknown: the exact path reports it (council s14 r2).
        // One whose keys exclude the File changes nothing (council s17,
        // B1b).
        if self.unknown_to(|| Ok(file_keys(file_id)))?.is_some() {
            return Ok(false);
        }
        // With a pack unreadable, an explicit manifest's bit proves it was
        // admitted, not that its chunks are still readable (they may have
        // been in that pack): only a file record, whose chunk is in its own
        // readable pack, proves the File closed (council s18 r1).
        let explicit_counts = self.damaged.is_empty();
        let raw = file_id.digest_bytes();
        let set = |placement: Placement| {
            self.logical(placement).is_some_and(|(seq, ordinal)| {
                admissions
                    .bitmaps
                    .get(&seq)
                    .is_some_and(|bitmap| bit(bitmap, ordinal))
            })
        };
        let mut placements = 0_usize;
        let mut admitted = false;
        for (pack, registered) in self.packs.iter().enumerate() {
            let loaded = &registered.pack;
            for entry in loaded.index.fileid_manifests(&raw) {
                let ordinal = loaded
                    .index
                    .manifest_ordinal(entry.manifest)
                    .ok_or_else(|| {
                        pack2::pack2_corruption(&loaded.path, "FileId entry names no manifest")
                    })?;
                loaded.index.explicit(ordinal).ok_or_else(|| {
                    pack2::pack2_corruption(&loaded.path, "FileId entry names no manifest")
                })?;
                placements += 1;
                admitted |= explicit_counts && set(Placement::Record { pack, ordinal });
            }
            if let Some(file) = loaded.index.find_file(&raw) {
                placements += 1;
                admitted |= set(Placement::Manifest { pack, file });
            }
        }
        for (digest, _) in self.loose_by_file.get(&raw).into_iter().flatten() {
            placements += 1;
            admitted |= explicit_counts && admissions.loose.contains(digest);
        }
        Ok(admitted && placements <= max_manifests)
    }

    /// Whether some registered copy of `id` is admitted to the Space whose
    /// admissions these are.
    pub(crate) fn admitted(
        &self,
        admissions: &SpaceAdmissions,
        id: &StoreObjectId,
    ) -> CasResult<bool> {
        self.admitted_with(admissions, id, false)
    }

    /// `admitted`, `positive` as in `settle`: a bit of a readable pack.
    pub(crate) fn admitted_with(
        &self,
        admissions: &SpaceAdmissions,
        id: &StoreObjectId,
        positive: bool,
    ) -> CasResult<bool> {
        for placement in self.placements(id)? {
            if self.placement_admitted(admissions, id, placement)? {
                return Ok(true);
            }
        }
        self.settle(positive, || object_keys(id), false)
    }

    /// Where `id`'s first copy is (`CasLocation`), from metadata.
    pub(crate) fn locate(
        &self,
        cas: &NativeCas,
        id: &StoreObjectId,
    ) -> CasResult<Option<CasLocation>> {
        let Some(placement) = self.placements(id)?.into_iter().next() else {
            return self.complete(|| object_keys(id), None);
        };
        let length = self.placement_length(id, placement)?;
        let packed = |pack: usize, location: Location| {
            let registered = &self.packs[pack];
            let loaded = &registered.pack;
            // A pack v2 record has no bytes of its own on disk: its location
            // is the stored (compressed) bytes of the block that holds it.
            let offset = loaded
                .index
                .blocks
                .get(location.block as usize)
                .map_or(0, |block| block.offset + 5);
            if loaded.is_inline() {
                return CasLocation::Inline {
                    pack: loaded.path.clone(),
                    database: cas
                        .root
                        .join(crate::scm_v1::layout::METADATA_DIR_NAME)
                        .join(crate::scm_v1::layout::SQLITE_FILE_NAME),
                    pack_seq: registered.seq,
                    offset,
                    length,
                };
            }
            CasLocation::Packed {
                pack: loaded.path.clone(),
                offset,
                length,
            }
        };
        Ok(Some(match placement {
            Placement::Record { pack, ordinal } => {
                let (_, _, location) = self.packs[pack]
                    .pack
                    .index
                    .explicit(ordinal)
                    .ok_or_else(|| corruption(id, "pack record vanished"))?;
                packed(pack, location)
            }
            Placement::Chunk { pack, file } | Placement::Manifest { pack, file } => {
                packed(pack, self.packs[pack].pack.index.files[file].location)
            }
            Placement::Loose => CasLocation::Loose {
                path: cas.object_path(id)?,
                length,
            },
        }))
    }
}

fn self_path() -> PathBuf {
    PathBuf::from("metadata/layrs.sqlite3")
}

/// A bitmap of a pack with `entries` logical objects: exactly
/// `ceil(entries / 8)` bytes and no bit at or past `entries`.
pub(crate) fn check_bitmap(bitmap: &[u8], entries: u64) -> Result<(), &'static str> {
    if bitmap.len() as u64 != entries.div_ceil(8) {
        return Err("admission bitmap length differs from the pack's entries");
    }
    if entries % 8 != 0 {
        let last = bitmap[bitmap.len() - 1];
        if last >> (entries % 8) != 0 {
            return Err("admission bitmap sets a bit past the pack's entries");
        }
    }
    Ok(())
}

pub(crate) fn bit(bitmap: &[u8], ordinal: u64) -> bool {
    bitmap
        .get((ordinal / 8) as usize)
        .is_some_and(|byte| byte & (1 << (ordinal % 8)) != 0)
}

pub(crate) fn set_bit(bitmap: &mut [u8], ordinal: u64) {
    if let Some(byte) = bitmap.get_mut((ordinal / 8) as usize) {
        *byte |= 1 << (ordinal % 8);
    }
}

/// A File the batch stores as one file record: a single-chunk manifest whose
/// chunk and manifest are both new in the batch (phase 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    identity: FileIdentity,
    byte_length: u64,
}

/// Which new chunks become file records and which new manifests they cover.
#[derive(Debug, Default)]
pub(crate) struct FileRecordPlan {
    by_chunk: HashMap<[u8; 32], PlannedFile>,
    covered_manifests: HashSet<[u8; 32]>,
}

impl FileRecordPlan {
    /// Plans a file record for each single-chunk manifest of a recipe file
    /// records carry whose chunk is supplied by the batch, when neither is
    /// already registered in `catalog`. One record per chunk (and so per
    /// File); other manifests of the same File stay explicit.
    pub(crate) fn new<'m>(
        catalog: &RegisteredCatalog,
        manifests: impl IntoIterator<Item = &'m FileManifest>,
        supplied_chunks: &HashSet<ChunkId>,
        whole_files: &HashSet<[u8; 32]>,
    ) -> CasResult<Self> {
        let mut plan = Self::default();
        // Whole manifests first: their chunk can only be a file record, and
        // a batch keeps one record per FileId (council s13 Q5).
        let mut manifests: Vec<&FileManifest> = manifests.into_iter().collect();
        manifests.sort_by_key(|manifest| !manifest.chunks_are_whole());
        let mut planned_files: HashSet<[u8; 32]> = HashSet::new();
        for manifest in manifests {
            let Some(recipe) = Recipe::of(manifest) else {
                continue;
            };
            let file = manifest.file_id().digest_bytes();
            // A File whose whole chunk the batch supplies keeps its v1 form
            // explicit: its file record is the whole one.
            if planned_files.contains(&file)
                || (!manifest.chunks_are_whole() && whole_files.contains(&file))
            {
                continue;
            }
            let chunk = &manifest.chunks()[0];
            if !supplied_chunks.contains(chunk.chunk_id()) {
                continue;
            }
            let chunk_object = StoreObjectId::ChunkRaw(chunk.chunk_id().clone());
            let manifest_object = StoreObjectId::FileManifest(manifest.id().clone());
            let chunk_raw = pack2::raw_digest(&chunk_object)?;
            if plan.by_chunk.contains_key(&chunk_raw)
                || !catalog.placements(&chunk_object)?.is_empty()
                || !catalog.placements(&manifest_object)?.is_empty()
            {
                continue;
            }
            let file_raw = pack2::hex_digest(
                manifest
                    .file_id()
                    .as_str()
                    .strip_prefix("blake3:")
                    .unwrap_or_default(),
            )
            .ok_or_else(|| CasError::InvalidObjectId {
                kind: StoreObjectKind::FileManifest,
                id: manifest.file_id().to_string(),
            })?;
            let manifest_raw = pack2::raw_digest(&manifest_object)?;
            planned_files.insert(file_raw);
            plan.covered_manifests.insert(manifest_raw);
            plan.by_chunk.insert(
                chunk_raw,
                PlannedFile {
                    identity: FileIdentity {
                        file_id: file_raw,
                        chunk_id: chunk_raw,
                        recipe,
                        manifest_id: manifest_raw,
                    },
                    byte_length: manifest.byte_length(),
                },
            );
        }
        Ok(plan)
    }
}

/// A pack's registration: name, metadata digest, logical entries.
type PackRegistration = (OsString, [u8; 32], u64);

/// Where a pack being written goes: memory while it may still be registered
/// inline (council s7 Q1), else its staging file.
enum PackSink {
    Memory(Vec<u8>),
    File {
        file: PinnedFile,
        /// A staging pack's shared staging lease; none for a maintenance
        /// pack built in the packs directory.
        _lease: Option<StagingLease>,
    },
}

impl io::Write for PackSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Memory(image) => {
                image.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            Self::File { file, .. } => file.file_mut().write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Memory(_) => Ok(()),
            Self::File { file, .. } => file.file_mut().flush(),
        }
    }
}

/// The pack v2 being written by one `put_ordered` batch or consolidation.
struct OpenPack {
    sink: PackSink,
    path: PathBuf,
    builder: Pack2Builder,
    published: bool,
    /// The largest image registered inline (0: a file from the start).
    inline_limit: usize,
}

impl OpenPack {
    fn file(&self) -> CasResult<&PinnedFile> {
        match &self.sink {
            PackSink::File { file, .. } => Ok(file),
            PackSink::Memory(_) => Err(io_error(
                "write CAS pack",
                &self.path,
                io::Error::other("pack is in memory"),
            )),
        }
    }
}

impl Drop for OpenPack {
    fn drop(&mut self) {
        // An unpublished staging pack is never referenced. After a Windows
        // rename the handle names the published pack: never unlink it.
        #[cfg(windows)]
        if self.published {
            return;
        }
        if let PackSink::File { file, .. } = &self.sink {
            let _ = file.unlink();
        }
    }
}

/// The largest image `cas_pack_inline` accepts (its CHECK).
const MAX_INLINE_IMAGE_BYTES: usize = 1024 * 1024;

/// Bytes up to which a batch's pack is kept in memory and registered
/// inline, by default: `LAYRS_CAS_INLINE_MAX` (bytes; 0 never inlines; at
/// most `MAX_INLINE_IMAGE_BYTES`), else 256 KiB (council s13 Q1: a step
/// that rewrites a few large directories stays one database write, and a
/// fresh pack file costs its first open on a machine with real-time
/// antivirus scanning).
fn default_inline_pack_limit() -> usize {
    static FROM_ENV: OnceLock<usize> = OnceLock::new();
    *FROM_ENV.get_or_init(|| {
        std::env::var("LAYRS_CAS_INLINE_MAX")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(256 * 1024)
            .min(MAX_INLINE_IMAGE_BYTES)
    })
}

/// Test support: per Store root, the inline limits tests force. Consulted
/// only once one was set.
fn inline_limit_overrides() -> &'static Mutex<HashMap<PathBuf, usize>> {
    static OVERRIDES: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    OVERRIDES.get_or_init(Default::default)
}

static INLINE_LIMIT_FORCED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Test support: per Store root, the in-batch consolidation budget tests
/// force (on disk and decompressed). Consulted only once one was set.
fn budget_overrides() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static OVERRIDES: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    OVERRIDES.get_or_init(Default::default)
}

static BUDGET_FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Test support: forces the in-batch consolidation budget of the Store at
/// `root` in this process to `bytes` (on disk and decompressed), so packs
/// past it wait for maintenance; `None` restores the default.
#[cfg(any(test, feature = "test-support"))]
pub fn set_consolidation_budget(root: &Path, bytes: Option<u64>) {
    BUDGET_FORCED.store(true, std::sync::atomic::Ordering::Release);
    if let Ok(mut overrides) = budget_overrides().lock() {
        match bytes {
            Some(bytes) => {
                overrides.insert(root.to_path_buf(), bytes);
            }
            None => {
                overrides.remove(root);
            }
        }
    }
}

impl NativeCas {
    /// This Store's in-batch consolidation budget (`Budget::IN_BATCH`,
    /// unless a test forced one).
    pub(crate) fn consolidation_budget(&self) -> Budget {
        if BUDGET_FORCED.load(std::sync::atomic::Ordering::Acquire) {
            if let Some(bytes) = budget_overrides()
                .lock()
                .ok()
                .and_then(|overrides| overrides.get(&self.root).copied())
            {
                return Budget { bytes, raw: bytes };
            }
        }
        Budget::IN_BATCH
    }
}

/// Test support: forgets every pack this process loaded, as a new process
/// would start (a pack file removed meanwhile is then found unreadable).
#[cfg(any(test, feature = "test-support"))]
pub fn forget_loaded_packs() {
    if let Ok(mut cache) = pack_cache().lock() {
        cache.clear();
    }
}

/// Test support: forces the inline pack limit of the Store at `root` in
/// this process (`None` restores the default).
#[cfg(any(test, feature = "test-support"))]
pub fn set_inline_pack_limit(root: &Path, limit: Option<usize>) {
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    if let Ok(mut overrides) = inline_limit_overrides().lock() {
        match limit {
            Some(limit) => {
                overrides.insert(root, limit);
            }
            None => {
                overrides.remove(&root);
            }
        }
        INLINE_LIMIT_FORCED.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// A loose content object a batch installed: SQLite registers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LooseInstall {
    pub(crate) object_id: StoreObjectId,
    pub(crate) byte_length: u64,
    /// For a manifest: its FileId and file length.
    pub(crate) file: Option<(FileId, u64)>,
}

/// What a format 6 batch published, for its SQLite registration.
#[derive(Debug, Default)]
pub(crate) struct CompactPublication {
    /// The batch's pack: name, metadata digest, logical entries.
    pub(crate) pack: Option<(OsString, [u8; 32], u64)>,
    /// Its image when it is registered inline (council s7 Q1).
    pub(crate) inline: Option<Vec<u8>>,
    /// Its sizes, as the consolidation budgets count them.
    pub(crate) stats: Option<PackStats>,
    /// Its keys row (council s17, B1b).
    pub(crate) keys: Option<super::keys::PackKeys>,
    pub(crate) loose: Vec<LooseInstall>,
}

/// Format 6 state of one `put_ordered` batch.
#[derive(Default)]
pub(super) struct CompactBatch {
    plan: FileRecordPlan,
    pack: Option<OpenPack>,
    members: HashSet<(u8, [u8; 32])>,
    files: HashSet<[u8; 32]>,
    loose: Vec<LooseInstall>,
    publication: Option<CompactPublication>,
}

impl NativeCas {
    /// Format 6: plans the file records of the next `put_ordered` batch.
    pub(crate) fn plan_file_records(&self, plan: FileRecordPlan) -> CasResult<()> {
        let mut batch = self
            .compact_batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))?;
        *batch = Some(CompactBatch {
            plan,
            ..CompactBatch::default()
        });
        Ok(())
    }

    /// Format 6: what the last `put_ordered` batch published.
    pub(crate) fn take_compact_publication(&self) -> CasResult<CompactPublication> {
        Ok(self
            .compact_batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))?
            .take()
            .and_then(|mut batch| batch.publication.take())
            .unwrap_or_default())
    }

    /// One `put_ordered` item of a format 6 batch. A registered copy that
    /// verifies byte for byte is reused (registered packs were durable
    /// before their registration committed); a record over 1 MiB is a loose
    /// file; anything else goes into the batch's pack.
    pub(super) fn put_compact(
        &self,
        catalog: &RegisteredCatalog,
        object: &CanonicalObject,
    ) -> CasResult<Slot> {
        let object_id = object.object_id();
        let bytes = encode_object(object)?;
        let bytes_len = bytes.len() as u64;
        let mut first_error = None;
        for placement in catalog.placements(&object_id)? {
            // A copy stored byte for byte as this object's canonical encoding
            // (which hashes to its id) is that object: no decoding needed.
            if let Ok(Some(stored)) = catalog.stored_bytes(self, placement) {
                if stored == bytes {
                    crate::perf::count("cas.compact_reuse", 1);
                    return Ok(Slot::Done(InstallOutcome {
                        path: self.object_path(&object_id)?,
                        object_id,
                        existed: true,
                        bytes_len,
                    }));
                }
            }
            match catalog.read_placement(self, &object_id, placement) {
                Ok(existing) if encode_object(&existing)? == bytes => {
                    crate::perf::count("cas.compact_reuse", 1);
                    return Ok(Slot::Done(InstallOutcome {
                        path: self.object_path(&object_id)?,
                        object_id,
                        existed: true,
                        bytes_len,
                    }));
                }
                Ok(_) => {
                    first_error.get_or_insert(CasError::ExistingObjectMismatch {
                        kind: object_id.kind(),
                        id: object_id.to_string(),
                    });
                }
                // A damaged registered copy never blocks a write: a sound
                // copy is written again (reads try every copy).
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error @ CasError::ExistingObjectMismatch { .. }) = first_error {
            return Err(error);
        }
        if bytes.len() > pack2::MAX_RECORD_BYTES {
            let outcome = match self.verified_existing(object_id.clone(), &bytes)? {
                Some(outcome) => outcome,
                None => {
                    let prepared = self.stage_encoded(object_id.clone(), bytes)?;
                    self.install(prepared)?
                }
            };
            if matches!(
                object_id.kind(),
                StoreObjectKind::ChunkRaw | StoreObjectKind::FileManifest
            ) {
                let file = match object {
                    CanonicalObject::FileManifest(manifest) => {
                        Some((manifest.file_id().clone(), manifest.byte_length()))
                    }
                    _ => None,
                };
                self.compact_state(|batch| {
                    batch.loose.push(LooseInstall {
                        object_id: object_id.clone(),
                        byte_length: bytes_len,
                        file,
                    });
                    Ok(())
                })?;
            }
            // A loose content object is new to SQLite even when its file
            // already existed (an orphan of a failed batch, verified above).
            return Ok(Slot::Done(InstallOutcome {
                existed: false,
                ..outcome
            }));
        }
        // A whole file's chunk is verified in the file domain and stored only
        // as its file record (council s13 Q5).
        let whole_chunk = match object {
            CanonicalObject::ChunkRaw(chunk) if chunk.is_whole() => Some(chunk.id().clone()),
            _ => None,
        };
        match &whole_chunk {
            Some(chunk_id) => {
                validate_file_record_chunk(chunk_id.clone(), &bytes, Recipe::Whole)?;
            }
            None => {
                validate_stored_bytes(&object_id, &bytes, self.whole_files)?;
            }
        }
        let raw = pack2::raw_digest(&object_id)?;
        let code = pack2::kind_code(object_id.kind());
        // A large record is compressed before taking the batch lock, so
        // workers compress in parallel.
        let mut planned = match object {
            CanonicalObject::ChunkRaw(_) => {
                self.compact_state(|batch| Ok(batch.plan.by_chunk.get(&raw).copied()))?
            }
            _ => None,
        };
        if let (Some(chunk_id), None) = (&whole_chunk, planned) {
            // A whole chunk is stored only as its file's record: one not
            // planned (its manifest is not in the batch, or a registered
            // copy proved damaged above) gets its record now, which stands
            // for its manifest too (council s16 r1). One record per FileId
            // in a batch.
            let file_id = layrs_core::scm::FileId::from_digest_bytes(&chunk_id.digest_bytes());
            let manifest = layrs_core::scm::FileManifest::whole(file_id, bytes_len)
                .map_err(|error| corruption(&object_id, error.to_string()))?;
            let identity = FileIdentity {
                file_id: raw,
                chunk_id: raw,
                recipe: Recipe::Whole,
                manifest_id: pack2::raw_digest(&StoreObjectId::FileManifest(
                    manifest.id().clone(),
                ))?,
            };
            let file = PlannedFile {
                identity,
                byte_length: bytes_len,
            };
            planned = self.compact_state(|batch| {
                if batch
                    .plan
                    .by_chunk
                    .values()
                    .any(|planned| planned.identity.file_id == raw)
                {
                    return Err(corruption(
                        &object_id,
                        "a batch keeps one file record per FileId",
                    ));
                }
                batch.plan.by_chunk.insert(raw, file);
                Ok(Some(file))
            })?;
        }
        let covered = matches!(object, CanonicalObject::FileManifest(_))
            && self.compact_state(|batch| Ok(batch.plan.covered_manifests.contains(&raw)))?;
        if covered {
            // Written as the alias of its chunk's file record.
            let new = self.compact_state(|batch| Ok(batch.members.insert((code, raw))))?;
            return Ok(Slot::Packed {
                object_id,
                bytes_len,
                new,
            });
        }
        let record_kind = if planned.is_some() { FILE_KIND } else { code };
        let encoded = if pack2::is_large(bytes.len()) {
            Some(
                pack2::EncodedBlock::single_record(record_kind, &bytes).map_err(|error| {
                    io_error("compress CAS pack block", &self.staging_dir, error)
                })?,
            )
        } else {
            None
        };
        let new = self.compact_state(|batch| {
            if !batch.members.insert((code, raw)) {
                return Ok(false);
            }
            if batch.pack.is_none() {
                batch.pack = Some(self.open_new_pack(self.inline_pack_limit())?);
            }
            let Some(open) = batch.pack.as_mut() else {
                return Err(io_error(
                    "open CAS pack",
                    &self.staging_dir,
                    io::Error::other("no pack"),
                ));
            };
            let path = open.path.clone();
            let write = |error| io_error("write CAS pack", &path, error);
            let sink = &mut open.sink;
            match (planned, object) {
                (Some(planned), CanonicalObject::ChunkRaw(_)) => {
                    if planned.byte_length != bytes_len
                        || !batch.files.insert(planned.identity.file_id)
                    {
                        return Err(corruption(
                            &object_id,
                            "file record plan differs from its chunk",
                        ));
                    }
                    open.builder
                        .add_file(sink, planned.identity, &bytes, encoded)
                        .map_err(write)?;
                }
                (_, CanonicalObject::FileManifest(manifest)) => {
                    open.builder
                        .add_record(sink, object_id.kind(), raw, &bytes, encoded)
                        .map_err(write)?;
                    let file_raw = pack2::hex_digest(
                        manifest
                            .file_id()
                            .as_str()
                            .strip_prefix("blake3:")
                            .unwrap_or_default(),
                    )
                    .ok_or_else(|| corruption(&object_id, "invalid FileId"))?;
                    open.builder
                        .add_manifest_file(file_raw, raw, manifest.byte_length());
                }
                _ => {
                    open.builder
                        .add_record(sink, object_id.kind(), raw, &bytes, encoded)
                        .map_err(write)?;
                }
            }
            self.spill_if_over(open)?;
            Ok(true)
        })?;
        Ok(Slot::Packed {
            object_id,
            bytes_len,
            new,
        })
    }

    fn compact_state<T>(&self, f: impl FnOnce(&mut CompactBatch) -> CasResult<T>) -> CasResult<T> {
        let mut batch = self
            .compact_batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))?;
        let batch = batch.get_or_insert_with(CompactBatch::default);
        f(batch)
    }

    /// Seals and publishes the batch's pack (when it holds anything), after
    /// every worker finished cleanly and before the batch's durability step,
    /// and records what SQLite must register.
    pub(super) fn finish_compact_batch(&self) -> CasResult<()> {
        let (open, loose) =
            self.compact_state(|batch| Ok((batch.pack.take(), std::mem::take(&mut batch.loose))))?;
        let mut publication = CompactPublication {
            pack: None,
            inline: None,
            stats: None,
            keys: None,
            loose,
        };
        if let Some(open) = open {
            let (pack, inline, stats, keys) = self.publish_pack2(open)?;
            publication.pack = Some(pack);
            publication.inline = inline;
            publication.stats = Some(stats);
            publication.keys = Some(keys);
        }
        self.compact_state(|batch| {
            batch.publication = Some(publication);
            Ok(())
        })
    }

    /// Abandons the batch's pack (its staging file is removed).
    pub(super) fn abandon_compact_batch(&self) {
        if let Ok(mut batch) = self.compact_batch.lock() {
            if let Some(batch) = batch.as_mut() {
                drop(batch.pack.take());
                batch.loose.clear();
            }
        }
    }

    /// This Store's inline pack limit (`default_inline_pack_limit`).
    fn inline_pack_limit(&self) -> usize {
        if INLINE_LIMIT_FORCED.load(std::sync::atomic::Ordering::Acquire) {
            if let Some(limit) = inline_limit_overrides()
                .lock()
                .ok()
                .and_then(|overrides| overrides.get(&self.root).copied())
            {
                return limit.min(MAX_INLINE_IMAGE_BYTES);
            }
        }
        default_inline_pack_limit()
    }

    /// A new pack: in memory while it may be registered inline (up to
    /// `inline_limit` bytes), else a staging file under a shared staging
    /// lease.
    fn open_new_pack(&self, inline_limit: usize) -> CasResult<OpenPack> {
        if inline_limit > 0 {
            let mut sink = PackSink::Memory(Vec::new());
            let path = self.staging_dir.join("inline-pack");
            let builder = Pack2Builder::start(&mut sink, self.whole_files)
                .map_err(|error| io_error("write CAS pack", &path, error))?;
            return Ok(OpenPack {
                sink,
                path,
                builder,
                published: false,
                inline_limit,
            });
        }
        let lease = StagingLease::shared(&self.staging_lease_path)?;
        let staging = self.pinned_staging()?;
        open_pack(&staging, &self.staging_dir, lease, self.whole_files)
    }

    /// Moves a pack kept in memory to a staging file once it outgrows the
    /// inline limit.
    fn spill_if_over(&self, open: &mut OpenPack) -> CasResult<()> {
        match &open.sink {
            PackSink::Memory(image) if image.len() > open.inline_limit => self.spill(open),
            _ => Ok(()),
        }
    }

    /// Moves a pack kept in memory to a staging file.
    fn spill(&self, open: &mut OpenPack) -> CasResult<()> {
        let PackSink::Memory(image) = &open.sink else {
            return Ok(());
        };
        let _span = crate::perf::span("cas.pack_spill");
        let lease = StagingLease::shared(&self.staging_lease_path)?;
        let staging = self.pinned_staging()?;
        let (mut file, path) = create_staging_pack(&staging, &self.staging_dir)?;
        let written = file.file_mut().write_all(image);
        let spilled = PackSink::File {
            file,
            _lease: Some(lease),
        };
        // From here the staging file is the pack (removed if it fails).
        let PackSink::Memory(_) = std::mem::replace(&mut open.sink, spilled) else {
            unreachable!("checked above");
        };
        open.path = path;
        written.map_err(|error| io_error("write CAS pack", &open.path, error))
    }

    /// Seals and publishes `open`: inline when it fits (its image validated
    /// as untrusted input, as a reread file is), else as a pack file. Returns
    /// its registration and, inline, its image.
    #[allow(clippy::type_complexity)]
    fn publish_pack2(
        &self,
        mut open: OpenPack,
    ) -> CasResult<(
        PackRegistration,
        Option<Vec<u8>>,
        PackStats,
        super::keys::PackKeys,
    )> {
        let builder = std::mem::replace(&mut open.builder, empty_builder());
        let sealed = builder
            .finish(&mut open.sink)
            .map_err(|error| io_error("write CAS pack index", &open.path, error))?;
        if let PackSink::Memory(image) = &open.sink {
            if image.len() <= open.inline_limit {
                let _span = crate::perf::span("cas.pack2_inline");
                let image = std::mem::take(match &mut open.sink {
                    PackSink::Memory(image) => image,
                    PackSink::File { .. } => unreachable!("checked above"),
                });
                let name = pack2::new_pack2_name();
                let path = self.inline_label(&name);
                let index =
                    pack2::verify_written_from(pack2::PackSource::Image(&image), &path, &sealed)?;
                self.hit(CasFaultPoint::BeforePackPublication)?;
                self.hit(CasFaultPoint::AfterPackPublication)?;
                crate::perf::count("cas.pack2_objects", sealed.logical);
                return Ok((
                    (name, sealed.digest, sealed.logical),
                    Some(image),
                    pack_stats(&index),
                    sealed.keys,
                ));
            }
        }
        // Not inline (decided once): the pack is a file.
        self.spill(&mut open)?;
        let (registration, stats) = self.publish_pack_file(open, &sealed)?;
        Ok((registration, None, stats, sealed.keys))
    }

    fn publish_pack_file(
        &self,
        mut open: OpenPack,
        sealed: &pack2::SealedPack2,
    ) -> CasResult<(PackRegistration, PackStats)> {
        let _span = crate::perf::span("cas.pack2_publish");
        let path = open.path.clone();
        {
            let _span = crate::perf::span("cas.pack_sync");
            let file = open.file()?;
            match self.batch_mode()? {
                Some(BatchDurability::JournalOrdered) => write_through(file.file()),
                _ => file.file().sync_all(),
            }
            .map_err(|error| io_error("sync CAS pack", &path, error))?;
        }
        let stats = {
            let _span = crate::perf::span("cas.pack_verify");
            pack_stats(&pack2::verify_written(open.file()?.file(), &path, sealed)?)
        };
        self.hit(CasFaultPoint::BeforePackPublication)?;
        let objects = self.pinned_objects()?;
        let packs_path = self.objects_dir.join(PACKS_DIR);
        let packs = objects
            .ensure_child_unsynced(OsStr::new(PACKS_DIR))
            .map_err(|error| io_error("create CAS packs directory", &packs_path, error))?;
        let objects_dir = self.objects_dir.clone();
        let deferred = self.defer_directory(&objects_dir, &objects)?
            && self.defer_directory(&packs_path, &packs)?;
        let name = pack2::new_pack2_name();
        let target = packs_path.join(&name);
        if publish_no_clobber(open.file()?, &packs, &name)? {
            return Err(io_error(
                "publish CAS pack",
                &target,
                io::Error::new(io::ErrorKind::AlreadyExists, "pack name collision"),
            ));
        }
        // From here the file is the published pack (a Windows rename moved
        // the handle): never unlink it.
        open.published = true;
        let installed = packs
            .open_file(&name)
            .map_err(|error| io_error("reread published CAS pack", &target, error))?;
        if !open.file()?.same_file(&installed) {
            return Err(pack2::pack2_corruption(
                &target,
                "published pack is not the file written",
            ));
        }
        self.hit(CasFaultPoint::AfterPackPublication)?;
        if !deferred {
            for directory in [&packs, &objects] {
                sync_directory_compatible(directory.sync())
                    .map_err(|error| io_error("sync CAS packs directory", &packs_path, error))?;
            }
        }
        self.cleanup_published_staging(open.file()?, &path, false)?;
        crate::perf::count("cas.pack2_objects", sealed.logical);
        Ok(((name, sealed.digest, sealed.logical), stats))
    }

    /// Scrub of the registered packs (`audit_registered`; loose files are
    /// walked by `scrub` itself).
    pub(super) fn scrub_registered(
        &self,
        catalog: &RegisteredCatalog,
        report: &mut CasScrubReport,
    ) -> CasResult<()> {
        catalog.audit_registered(self, false, |event| {
            let AuditEvent::Copy(copy) = event else {
                return true;
            };
            report.checked_objects += 1;
            if let Err(error) = copy.result {
                report.issues.push(CasScrubIssue {
                    path: copy.path,
                    code: error.code(),
                    message: format!("{}{error}", copy.message_prefix),
                });
            }
            true
        });
        Ok(())
    }
}

impl RegisteredCatalog {
    /// Verifies every registered physical copy at its own place, never
    /// through the lookup by id (a sound copy elsewhere cannot hide a
    /// damaged one), from the metadata on disk now (never the process
    /// cache): each registered pack's metadata against its registered digest
    /// and entry count, then every block decompressed and every record
    /// verified; with `loose`, every registered loose object. A damaged
    /// block names every record whose index entry points into it; an
    /// unreadable pack is one copy (its records are then named by
    /// reachability, as `git fsck` does). Unregistered pack files are not
    /// present and are not checked. `visit` returns `false` to stop.
    pub(crate) fn audit_registered(
        &self,
        cas: &NativeCas,
        loose: bool,
        mut event: impl FnMut(AuditEvent) -> bool,
    ) {
        let unreadable = |path: PathBuf, name: &OsStr, error: CasError| {
            AuditEvent::Copy(AuditedCopy {
                path,
                object: name.to_string_lossy().into_owned(),
                message_prefix: String::new(),
                result: Err(error),
                pack_level: true,
            })
        };
        for row in &self.rows {
            let path = row.location(cas);
            // An audit catalog already holds this pack's fresh metadata (or
            // why it is unreadable); any other catalog is reread now.
            let loaded = if self.fresh {
                match self.packs.iter().find(|pack| pack.seq == row.seq) {
                    Some(pack) => Ok(Arc::clone(&pack.pack)),
                    None => Err(self
                        .damaged
                        .iter()
                        .find(|(seq, _)| *seq == row.seq)
                        .map(|(_, error)| error.clone())
                        .unwrap_or_else(|| {
                            pack2::pack2_corruption(&path, "registered pack is unreadable")
                        })),
                }
            } else {
                row.load(cas, &std::cell::OnceCell::new(), true)
                    .and_then(|pack| {
                        check_registration(&pack, &row.name, &row.digest, row.entries)
                            .map(|()| pack)
                    })
            };
            let loaded = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    if !event(unreadable(path, &row.name, error)) {
                        return;
                    }
                    continue;
                }
            };
            // One handle for the whole pack, closed when its audit ends (an
            // inline pack is read from its image).
            let file = match &loaded.image {
                Some(_) => None,
                None => Some(loaded.open(cas)),
            };
            for (block, records) in loaded.index.records_by_block().into_iter().enumerate() {
                let (mut envelope_bytes, mut chunk_bytes) = (0_u64, 0_u64);
                for record in &records {
                    match record {
                        pack2::RecordRef::Explicit(ordinal) => {
                            if let Some((kind, _, location)) = loaded.index.explicit(*ordinal) {
                                let length = u64::from(location.length);
                                if kind == pack2::kind_code(StoreObjectKind::ChunkRaw) {
                                    chunk_bytes += length;
                                } else {
                                    envelope_bytes += length;
                                }
                            }
                        }
                        pack2::RecordRef::File(file) => {
                            chunk_bytes += u64::from(loaded.index.files[*file].location.length);
                        }
                    }
                }
                if !event(AuditEvent::Block {
                    envelope_bytes,
                    chunk_bytes,
                }) {
                    return;
                }
                let raw = match (&loaded.image, &file) {
                    (Some(image), _) => pack2::read_block_from(
                        pack2::PackSource::Image(image),
                        &loaded.path,
                        &loaded.index,
                        block as u32,
                    ),
                    (None, Some(file)) => file.as_ref().map_err(Clone::clone).and_then(|file| {
                        pack2::read_block(file.file(), &loaded.path, &loaded.index, block as u32)
                    }),
                    (None, None) => Err(pack2::pack2_corruption(&loaded.path, "pack not opened")),
                };
                for record in records {
                    let (object, name, result) =
                        scrub_record(&loaded, record, raw.as_ref(), cas.whole_files);
                    let copy = AuditedCopy {
                        path: loaded.path.clone(),
                        object,
                        message_prefix: format!("{name}: "),
                        result,
                        pack_level: false,
                    };
                    if !event(AuditEvent::Copy(copy)) {
                        return;
                    }
                }
            }
        }
        if !loose {
            return;
        }
        for (digest, entry) in &self.loose {
            let chunk = entry.kind == pack2::kind_code(StoreObjectKind::ChunkRaw);
            if !event(AuditEvent::Loose {
                envelope: !chunk,
                bytes: entry.byte_length,
            }) {
                return;
            }
            let audited = pack2::kind_of_code(entry.kind)
                .ok_or_else(|| pack2::pack2_corruption(&self_path(), "loose object kind"))
                .and_then(|kind| pack2::object_id_of(kind, digest))
                .and_then(|id| {
                    let path = cas.object_path(&id)?;
                    let missing = |error| match error {
                        CasError::NotFound { .. } => {
                            corruption(&id, "registered loose object is missing")
                        }
                        other => other,
                    };
                    // A chunk is streamed and hashed, never held whole.
                    let length = match &id {
                        StoreObjectId::ChunkRaw(chunk_id) => cas
                            .stream_loose_chunk(chunk_id, &id, &mut |_| {})
                            .map_err(missing),
                        _ => cas
                            .read_loose(&id)
                            .map_err(missing)
                            .and_then(|object| Ok(encode_object(&object)?.len() as u64)),
                    };
                    let result = length.and_then(|length| {
                        if length == entry.byte_length {
                            Ok(())
                        } else {
                            Err(corruption(&id, "loose object length differs from its row"))
                        }
                    });
                    Ok((id.to_string(), path, result))
                });
            let copy = match audited {
                Ok((object, path, result)) => AuditedCopy {
                    path,
                    message_prefix: format!("{object}: "),
                    object,
                    result,
                    pack_level: false,
                },
                Err(error) => AuditedCopy {
                    path: self_path(),
                    object: pack2::digest_text(digest),
                    message_prefix: String::new(),
                    result: Err(error),
                    pack_level: false,
                },
            };
            if !event(AuditEvent::Copy(copy)) {
                return;
            }
        }
    }

    /// Each readable registered pack whose keys row is missing, bound to
    /// another index, fails its checksum, or differs from what its index
    /// gives (council s17, B1c), with why. The rows of unreadable packs
    /// cannot be checked against anything.
    pub(crate) fn keys_row_problems(
        &self,
        connection: &Connection,
        cas: &NativeCas,
    ) -> CasResult<Vec<(PathBuf, String)>> {
        let mut statement = connection
            .prepare(
                "SELECT layout_digest,keys_digest,key_count,rice_bits,group_digests,filter,checksum \
                 FROM cas_pack_keys WHERE pack_seq=?1",
            )
            .map_err(sql_error)?;
        let mut problems = Vec::new();
        for registered in &self.packs {
            let path = registered.pack.path.clone();
            let Some(row) = self.rows.iter().find(|row| row.seq == registered.seq) else {
                continue;
            };
            let stored = statement
                .query_row([registered.seq], |stored| {
                    Ok((
                        stored.get::<_, Vec<u8>>(0)?,
                        stored.get::<_, Vec<u8>>(1)?,
                        stored.get::<_, i64>(2)?,
                        stored.get::<_, i64>(3)?,
                        stored.get::<_, Vec<u8>>(4)?,
                        stored.get::<_, Vec<u8>>(5)?,
                        stored.get::<_, Vec<u8>>(6)?,
                    ))
                })
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    error => Err(error),
                })
                .map_err(sql_error)?;
            let Some((layout, keys_digest, count, bits, groups, filter, checksum)) = stored else {
                problems.push((path, "registered pack has no keys row".to_owned()));
                continue;
            };
            if layout != row.digest {
                problems.push((path, "keys row is bound to another index".to_owned()));
                continue;
            }
            let (Ok(keys_digest), Ok(key_count), Ok(rice_bits)) = (
                <[u8; 32]>::try_from(keys_digest.as_slice()),
                u64::try_from(count),
                u8::try_from(bits),
            ) else {
                problems.push((path, "keys row is malformed".to_owned()));
                continue;
            };
            let stored = super::keys::PackKeys {
                key_count,
                rice_bits,
                keys_digest,
                group_digests: groups,
                filter,
            };
            let layout: [u8; 32] = match layout.as_slice().try_into() {
                Ok(layout) => layout,
                Err(_) => {
                    problems.push((path, "keys row is malformed".to_owned()));
                    continue;
                }
            };
            if stored.checksum(&layout)[..] != checksum[..] {
                problems.push((path, "keys row checksum differs".to_owned()));
                continue;
            }
            match registered.pack.rebuild_keys(rice_bits) {
                Ok(rebuilt) if rebuilt == stored => {}
                Ok(_) => problems.push((path, "keys row differs from the pack's index".to_owned())),
                Err(error) => problems.push((path, error.to_string())),
            }
        }
        // An unreadable pack's row cannot be checked against its index, but
        // it must exist and check: without it, every negative answer about
        // the Store is unknown (council s18 r1).
        for (seq, _) in &self.damaged {
            if self.damaged_filters.contains_key(seq) {
                continue;
            }
            let path = self
                .rows
                .iter()
                .find(|row| row.seq == *seq)
                .map_or_else(|| cas.objects_dir.join(PACKS_DIR), |row| row.location(cas));
            problems.push((
                path,
                "unreadable pack has no usable keys row: no absence can be proven beside it"
                    .to_owned(),
            ));
        }
        Ok(problems)
    }

    /// Whether an unreadable registered pack may hold a manifest of `file`
    /// (its keys row does not exclude it): `false` on a sound catalog.
    pub(crate) fn file_may_be_unreadable(&self, file: &FileId) -> bool {
        self.unknown_to(|| Ok(file_keys(file)))
            .map_or(true, |unknown| unknown.is_some())
    }

    /// Whether an unreadable registered pack may hold `id` (its keys row
    /// does not exclude it): `false` on a sound catalog.
    pub(crate) fn may_hold_unreadably(&self, id: &StoreObjectId) -> CasResult<bool> {
        Ok(self.unknown_to(|| object_keys(id))?.is_some())
    }

    /// The registered packs that cannot be read, with their paths.
    pub(crate) fn unreadable_packs(&self, cas: &NativeCas) -> Vec<(PathBuf, CasError)> {
        self.damaged
            .iter()
            .map(|(seq, error)| {
                let path = self
                    .rows
                    .iter()
                    .find(|row| row.seq == *seq)
                    .map_or_else(|| cas.objects_dir.join(PACKS_DIR), |row| row.location(cas));
                (path, error.clone())
            })
            .collect()
    }
}

/// A loaded pack must be the one SQLite registered: its metadata digest
/// and logical entry count.
fn check_registration(
    pack: &LoadedPack,
    name: &OsStr,
    digest: &[u8],
    entries: i64,
) -> CasResult<()> {
    if !pack2::valid_pack2_name(name) || digest.len() != 32 || entries < 0 {
        return Err(pack2::pack2_corruption(&pack.path, "invalid cas_packs row"));
    }
    if pack.index.digest[..] != digest[..] {
        return Err(pack2::pack2_corruption(
            &pack.path,
            "pack metadata digest differs from the registered one",
        ));
    }
    if pack.index.logical_count() != entries as u64 {
        return Err(pack2::pack2_corruption(
            &pack.path,
            "pack entry count differs from the registered one",
        ));
    }
    Ok(())
}

/// (object id, display name, verification) of one pack record.
fn scrub_record(
    loaded: &LoadedPack,
    record: pack2::RecordRef,
    raw: Result<&Vec<u8>, &CasError>,
    whole_files: bool,
) -> (String, String, CasResult<()>) {
    match record {
        pack2::RecordRef::Explicit(ordinal) => {
            let Some((kind, digest, location)) = loaded.index.explicit(ordinal) else {
                return (
                    "pack entry".into(),
                    "pack entry".into(),
                    Err(pack2::pack2_corruption(&loaded.path, "entry vanished")),
                );
            };
            let Some(object_kind) = pack2::kind_of_code(kind) else {
                return (
                    "pack entry".into(),
                    "pack entry".into(),
                    Err(pack2::pack2_corruption(&loaded.path, "unknown kind")),
                );
            };
            let id = match pack2::object_id_of(object_kind, digest) {
                Ok(id) => id,
                Err(error) => return ("pack entry".into(), "pack entry".into(), Err(error)),
            };
            let checked = raw.map_err(Clone::clone).and_then(|block| {
                let body = pack2::record_body(block, location, kind, &loaded.path)?;
                validate_stored_bytes(&id, body, whole_files).map(|_| ())
            });
            (id.to_string(), id.to_string(), checked)
        }
        pack2::RecordRef::File(file) => {
            let file_record = loaded.index.files[file];
            let name = pack2::digest_text(&file_record.file_id);
            let chunk = StoreObjectId::ChunkRaw(
                ChunkId::parse(pack2::digest_text(&file_record.chunk_id))
                    .unwrap_or_else(|_| unreachable!("a 32-byte digest is a ChunkId")),
            )
            .to_string();
            let checked = (|| {
                let block = raw.map_err(Clone::clone)?;
                let body =
                    pack2::record_body(block, file_record.location, FILE_KIND, &loaded.path)?;
                let (file_id, chunk_id) = loaded.alias_ids(file)?;
                validate_file_record_chunk(chunk_id, body, file_record.recipe)?;
                if FileId::from_bytes(body) != file_id {
                    return Err(pack2::pack2_corruption(
                        &loaded.path,
                        "file record bytes differ from its FileId",
                    ));
                }
                loaded.alias_manifest_digest(file).map(|_| ())
            })();
            (chunk, format!("file {name}"), checked)
        }
    }
}

// A whole file is one record of its pack: its largest size is a record's.
const _: () = assert!(layrs_core::scm::WHOLE_MAX_BYTES as usize <= pack2::MAX_RECORD_BYTES);

/// A file record's body as its chunk: named in the file domain for a
/// `whole` record (council s13 Q5), else as any raw chunk.
fn validate_file_record_chunk(
    chunk_id: ChunkId,
    body: &[u8],
    recipe: Recipe,
) -> CasResult<CanonicalObject> {
    let id = StoreObjectId::ChunkRaw(chunk_id);
    if recipe != Recipe::Whole {
        return validate_stored_bytes(&id, body, false);
    }
    match Chunk::whole(body.to_vec()) {
        Ok(chunk) if StoreObjectId::ChunkRaw(chunk.id().clone()) == id => {
            Ok(CanonicalObject::ChunkRaw(chunk))
        }
        Ok(chunk) => Err(CasError::IdMismatch {
            expected: id,
            actual: chunk.id().to_string(),
        }),
        Err(error) => Err(CasError::Corruption {
            kind: StoreObjectKind::ChunkRaw,
            id: id.to_string(),
            reason: format!("invalid whole file chunk: {error}"),
        }),
    }
}

fn empty_builder() -> Pack2Builder {
    // Only replaces a builder being finished; never written.
    Pack2Builder::start(&mut io::sink(), false).unwrap_or_else(|_| unreachable!("sink never fails"))
}

fn open_pack(
    staging: &PinnedDirectory,
    staging_dir: &Path,
    lease: StagingLease,
    whole_files: bool,
) -> CasResult<OpenPack> {
    let (file, path) = create_staging_pack(staging, staging_dir)?;
    let mut sink = PackSink::File {
        file,
        _lease: Some(lease),
    };
    let builder = Pack2Builder::start(&mut sink, whole_files)
        .map_err(|error| io_error("write CAS pack", &path, error))?;
    Ok(OpenPack {
        sink,
        path,
        builder,
        published: false,
        inline_limit: 0,
    })
}

/// A new staging file for a pack.
fn create_staging_pack(
    staging: &PinnedDirectory,
    staging_dir: &Path,
) -> CasResult<(PinnedFile, PathBuf)> {
    let mut last_error = None;
    for _ in 0..64 {
        let name = staging_name();
        match staging.create_file(OsStr::new(&name)) {
            Ok(file) => return Ok((file, staging_dir.join(&name))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => return Err(io_error("create CAS pack", staging_dir, error)),
        }
    }
    Err(io_error(
        "allocate CAS pack",
        staging_dir,
        last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "collision")),
    ))
}

#[cfg(test)]
mod consolidation_tests {
    use super::{bounded_group, candidates_among, consolidation_group};

    fn packs(sizes: &[u64]) -> Vec<(u64, u64, u64, u64, usize)> {
        let mut packs: Vec<_> = sizes
            .iter()
            .enumerate()
            .map(|(position, size)| (*size, 1, 1, *size, position))
            .collect();
        packs.sort_unstable();
        packs
    }

    #[test]
    fn small_packs_merge_geometrically_and_a_lone_tiny_one_blocks_nothing() {
        // The tiny ones; 44 is more than twice their 12: it waits.
        assert_eq!(
            consolidation_group(&packs(&[1220, 88, 44, 4, 4, 4, 44])),
            vec![3, 4, 5]
        );
        // A lone 1 KiB pack below five of 100 KiB: those five merge.
        assert_eq!(
            consolidation_group(&packs(&[1, 100, 100, 100, 100, 100])),
            vec![1, 2, 3, 4, 5]
        );
        // Sizes too far apart: nothing.
        assert!(consolidation_group(&packs(&[1, 10, 100, 1000])).is_empty());
    }

    #[test]
    fn a_group_stays_within_half_the_reader_limits() {
        let half = crate::scm_v1::cas::pack2::MAX_PACK_ENTRIES / 2;
        let packs = vec![
            (10, half - 1, 1, 10, 0),
            (10, 2, 1, 10, 1),
            (11, 1, 1, 11, 2),
        ];
        // The first two would exceed the entries limit: the second and
        // third form the group.
        assert_eq!(consolidation_group(&packs), vec![1, 2]);
    }

    #[test]
    fn an_inline_group_stays_within_the_byte_budget() {
        let mib = 1024 * 1024;
        // 40 + 40 MiB exceeds 64 MiB: the large tier waits, the small one
        // above it still merges.
        assert!(consolidation_group(&packs(&[40 * mib, 40 * mib])).is_empty());
        assert_eq!(
            consolidation_group(&packs(&[40 * mib, 40 * mib, mib, mib, mib])),
            vec![2, 3, 4]
        );
        // The group stops at the budget: 30 + 30 fits, the third does not.
        assert_eq!(
            consolidation_group(&packs(&[30 * mib, 30 * mib, 30 * mib])),
            vec![0, 1]
        );
        // Compressible content is bounded by what it decompresses to:
        // three 1 MiB packs of 100 MiB each exceed 256 MiB at the third.
        let compressible: Vec<_> = (0..3).map(|p| (mib, 1, 1, 100 * mib, p)).collect();
        assert_eq!(consolidation_group(&compressible), vec![0, 1]);
    }

    #[test]
    fn an_inline_merge_stays_within_the_same_budgets() {
        let (kib, mib) = (1024, 1024 * 1024);
        // Eight inline packs of 64 KiB, each 64 MiB decompressed: four fit
        // the 256 MiB decompressed budget (council s8 r2).
        let inline: Vec<_> = (0..8).map(|p| (64 * kib, 1, 1, 64 * mib, p)).collect();
        assert_eq!(bounded_group(&inline, false), vec![0, 1, 2, 3]);
        // The smallest pack alone over the decompressed budget is skipped:
        // the eight after it still merge (council s8 r3).
        let mut blocked = vec![(kib, 1, 1, 300 * mib, 0)];
        blocked.extend((1..9).map(|p| (2 * kib, 1, 1, kib, p)));
        assert_eq!(bounded_group(&blocked, false), (1..9).collect::<Vec<_>>());
        // Without the geometric rule, sizes far apart still merge.
        let mixed = vec![(1, 1, 1, 1, 0), (10, 1, 1, 10, 1), (1000, 1, 1, 1000, 2)];
        assert_eq!(bounded_group(&mixed, false), vec![0, 1, 2]);
        assert_eq!(bounded_group(&mixed, true), vec![0]);
    }

    /// The trigger asks for a group that fits, not a count of packs: six
    /// pack files above the budgets wait for maintenance instead of making
    /// every batch reopen the CAS to find nothing (council s17, B2a).
    #[test]
    fn a_consolidation_is_due_only_when_a_group_fits() {
        let mib = 1024 * 1024;
        let files = |sizes: &[u64]| -> Vec<(u64, u64, u64, u64, usize)> {
            sizes
                .iter()
                .enumerate()
                .map(|(position, size)| (*size, 1, 1, *size, position))
                .collect()
        };
        let none: Vec<(u64, u64, u64, u64, usize)> = Vec::new();
        // Six tiers of 40 MiB: no two fit 64 MiB.
        let large = files(&[40 * mib; 6]);
        assert!(candidates_among(&none, &large, 6).is_empty());
        // Six small pack files: a group.
        let small = files(&[mib; 6]);
        assert_eq!(candidates_among(&none, &small, 6).len(), 6);
        // Five are not enough, whatever their sizes.
        assert!(candidates_among(&none, &files(&[mib; 5]), 5).is_empty());
        // Eight inline packs merge whatever the pack files.
        let inline: Vec<_> = (0..8).map(|p| (1024, 1, 1, 1024, p)).collect();
        assert_eq!(candidates_among(&inline, &large, 6).len(), 8);
    }
}
