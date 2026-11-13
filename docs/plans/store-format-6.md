# Store format 6 — implementation spec (ADR 0028 phases 2 and 3)

Status: implemented on 6 November 2026 behind `LAYRS_STORE_FORMAT=6` (opt-in);
see Deviations and Measurements at the end. Format 5 stays
the default until phase 6 of ADR 0028. Read ADR 0028 first
(`docs/decisions/0028-store-compact.md`), then ADR 0026 (packs v1) and ADR
0012 (Space admission).

Goal measured by `scratchpad`-style script: a 10,000-file project (10 dirs x
1,000 files, `"line i\n" + "\n" * (i % 40)`), `layrs init`, then
`du -sk .layrs`. Today 43,233 KiB (SQLite 33,152, pack 7,644, trees 1,484,
change index 528, projection cache 412). git: 1,655 KiB. Target for this
work: SQLite under 400 KiB and pack + trees under 1,000 KiB, with
`init`, `status`, `step` no slower than format 5.

## Invariants that must hold exactly as today

1. **CAS before reference.** Pack bytes are published and made durable
   before the SQLite transaction that registers them (`repository.rs`
   ~L298-410 today: CAS install before `BEGIN IMMEDIATE`).
2. **Committed installation is SQLite's word.** A pack file that is not
   registered in `cas_packs` (orphan of a failed batch) is never present to
   any reader. A registered pack whose index blake3 differs from the
   registered digest is corruption, never absence.
3. **Exact per-Space admission, monotonic.** Admission is per object and per
   Space, never inferred from Trees (chunks and manifests are admitted
   without Trees; a FileId can have several manifests). Admissions are
   never removed (the closure induction in `admission.rs` ~L657-679 relies
   on it).
4. **Every read verifies content against its id** (canonical decode and
   re-encode for envelope kinds, chunk hash for chunks), as today.
5. **Reader API semantics unchanged** (`crates/layrs-store/src/scm/repository.rs`
   trait, `NativeScmReader` in `scm_v1/read/*`): same answers, same
   errors for corrupt metadata, same budgets and limits
   (`file_manifest_ids_metadata` sorted by id, 1,024 bound;
   `admitted_chunk_ids_of_files` intersects chunk admission, not manifest
   admission; `file_availability_with_budget` charges before reading).
6. Transactional readers (`read/transactional.rs`) still refuse reads
   after their transaction ends, including index-only answers.
7. Crash safety: every crash point (pack rename, durability, commit) leaves
   either the old or the new committed state; failed batches leave only
   unregistered files.

## SQLite schema v3 (new baseline, do not edit v2)

New `crates/layrs-store-local/tests/fixtures/scm-v3.sql`, `SCHEMA_VERSION`
3 for format-6 Stores with its own checksum and fingerprint
(`scm_v1/schema.rs`); format-5 Stores keep v2. Differences from v2:

- Content kinds (`chunk_raw`, `file_manifest`) never have `objects` rows.
  `objects` keeps the other kinds; drop `storage_path` (derived from the id)
  and use `WITHOUT ROWID` where the key is the object id.
- Drop `file_availability`, `file_manifests`, `file_manifest_chunks`.
- `object_edges`: no `manifest_chunk` rows (manifests decode their chunks).
- `space_object_admissions`: non-content objects only.
- New:
  - `cas_packs(pack_seq INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE,
    index_digest BLOB NOT NULL CHECK(length(index_digest)=32),
    entries INTEGER NOT NULL, created_at_ms INTEGER NOT NULL) STRICT`
    — one row per registered pack, inserted in the batch transaction.
  - `space_pack_admissions(space_id TEXT NOT NULL, pack_seq INTEGER NOT NULL
    REFERENCES cas_packs, bitmap BLOB NOT NULL, PRIMARY KEY(space_id,
    pack_seq)) WITHOUT ROWID, STRICT` — bit i = index ordinal i of that pack
    admitted to that Space. Updates only OR bits in.
  - `loose_objects(object_id BLOB PRIMARY KEY CHECK(length=32), kind INTEGER,
    byte_length INTEGER) WITHOUT ROWID, STRICT` for content records over
    1 MiB (large chunks), plus `space_loose_admissions(space_id, object_id)`.
  - Foreign keys that pointed content objects at `objects` (pins, gc roots,
    recovery paths, pending tables) must accept content ids by validation
    in code inside the transaction, with matching fsck checks.
- Use `PRAGMA page_size = 1024` for new format-6 databases if `init`,
  `status` and `step` timings do not regress (measure; else 4096).

## Pack v2 (`objects/packs/pack-<32 hex>.lp2`)

- Header: magic `LYRSPAK2`, u32 version 1.
- Records grouped into **blocks** of at most 16 KiB uncompressed, each
  block a zstd frame (level 3; workspace already has `zstd = 0.13`, add it
  to `layrs-store-local`) prefixed by its compressed length and followed by
  an 8-byte checksum (blake3 of the compressed bytes, truncated). A record
  never spans blocks except a single record larger than 16 KiB, which gets
  a block of its own (raw if zstd does not shrink it, flag in the block
  header).
- Record: `kind u8, codec u8, varint length, body`. No id in the record:
  the id lives in the index (ADR 0028). Codec 0 = canonical bytes as v1
  stores them; phase 4 adds binary Tree/manifest codecs.
- Index (after the blocks): entries sorted by `(kind, id)`, each `kind u8,
  id [32], block u32, offset-in-block u32, length u32`; a 256-entry fanout
  on the first id byte per kind.
- **FileId section**: sorted `(FileId [32], manifest ordinal u32)` for every
  FileManifest in the pack, so FileId -> manifests is an index lookup.
- Footer: offsets/sizes of index and FileId section, entry counts, blake3
  of header + index + FileId section (this is the digest registered in
  `cas_packs`), end magic.
- Every content or envelope record of at most 1 MiB in a batch goes into
  that batch's single pack (no 64-object threshold, no 64 KiB limit); only
  records over 1 MiB stay loose files (registered in `loose_objects`).
- Reading: index validated as untrusted input (bounds, order, uniqueness,
  ranges within blocks), then compared with the registered digest; each
  read decompresses one block with a bounded output size and verifies the
  record's content against its id. Per-process index cache keyed by Store
  identity and pack name (as v1).
- Scrub: decompress every block, verify every indexed record; a damaged
  block reports every record whose index entry points into it.

## Phase 3 in the same format: small files stored once

- A file of exactly one chunk and at most 1 MiB is stored as **one record
  of kind `file`** keyed by FileId whose body is the raw bytes, instead of
  a chunk record plus a manifest record.
- Its ChunkId and its manifest recipe (`deterministic_fixed` with the
  standard size, or `single`) are in an **alias section** of the index:
  `(ChunkId [32] -> file ordinal)` and `(FileManifestId [32] -> file
  ordinal, recipe u8)`. Lookups by ChunkId or FileManifestId stay exact
  and never hash the content to answer metadata questions.
- `get_chunk` / `get_file_manifest` on an alias synthesize the canonical
  object (raw chunk bytes; the canonical JCS manifest) and verify it
  against the requested id. The wire format is unchanged.
- Admission bits apply to the logical objects: the file record's ordinal
  bit admits the chunk and the recipe manifest it carries, and only if the
  batch admitted both (otherwise store them separately as today).

## Reader replacements (council r1 table)

| Today (SQL) | Format 6 |
| --- | --- |
| `objects` row for chunk/manifest | registered pack index or `loose_objects` |
| `space_object_admissions` for content | bitmap bit of the object's ordinal, or `space_loose_admissions` |
| `file_availability`, `file_manifests` | FileId section + file records |
| `file_manifest_chunks` | decode the manifest record (or the alias recipe) |
| `manifest_chunk` edges, `read/projections.rs` cross-checks | decode; the cross-check disappears with the projection |
| `vfs_recovery.rs` `file_manifests` query | catalog lookup |
| fsck content checks | walk registered packs and admitted roots |

Keep a single in-memory catalog per open Store snapshot: registered packs
(from one SQLite read), their validated indexes, and the admission bitmaps
of the Space being read, so metadata answers cost no SQL per file.

## Tests (at least)

- Format 6 init/capture/status/step/merge/restore/room end to end with
  `LAYRS_STORE_FORMAT=6` (run the existing client-core and CLI suites with
  the variable set; they must pass unchanged).
- Pack v2: round trip, mixed kinds, blocks boundary, record over 16 KiB,
  over 1 MiB (loose), corrupt block (only its records fail, scrub names
  them), corrupt index, digest mismatch with SQLite = corruption, orphan
  pack never present, crash at rename / durability / commit then reopen.
- Admission: two Spaces sharing bytes with different admissions; chunk
  admitted alone; manifest alternative for the same FileId; bitmaps only
  grow; differential test of reader answers format 5 vs format 6 on the
  same batch sequence.
- File records: lengths 0, 1, 1 MiB, 1 MiB + 1; lookups by FileId,
  ChunkId and manifest id; the same chunk referenced by a large file's
  manifest; a corrupt file record.
- Size: the 10,000-file measurement above.

## Out of scope here

Binary Tree/manifest codecs (phase 4), consolidation (phase 5), making 6
the default and `layrs store convert` (phase 6), deltas (phase 7).

## Deviations (implementation, 6 November 2026)

Each one keeps every invariant above; the reason is given.

1. **Index layout is compact, not fixed-width.** Entries are grouped per kind
   (no kind byte per entry) and their `block, offset, length` are LEB128
   varints; the 256-entry fanout is rebuilt in memory when the index is
   loaded instead of being stored (1 KiB per kind and per pack: a small step
   pack would pay 4 KiB of fanouts). The metadata is still decoded entirely
   and validated as untrusted input (bounds, strict order, uniqueness,
   minimal varints, permutations, ranges within blocks) before any use, and
   its blake3 is the registered digest.
2. **Manifest aliases store a 4-byte prefix of the FileManifestId**, not the
   32-byte id. A lookup by manifest id narrows to the file records with that
   prefix and recomputes each candidate's full id from the record's index
   metadata (FileId, ChunkId, length, recipe: the canonical JCS manifest),
   so the answer stays exact and never hashes content. The computed id is
   cached per loaded pack and checked against the stored prefix. Saves 28
   bytes per small file.
3. **Two admission bits per file record** (its chunk at `explicit + 2f`, its
   recipe manifest at `explicit + 2f + 1`) instead of one bit that admits
   both. Admission stays exact per logical object, and a Space that admits
   only the chunk of an existing file record (or only its manifest) gets
   exactly that, without storing the bytes a second time. A file record is
   therefore formed whenever a single-chunk manifest of a supported recipe
   and its chunk are both new in the batch, whatever the batch admits.
4. **The FileId section also records each explicit manifest's file
   length**, so `file_length_metadata` and status never decompress a
   manifest. A decoded manifest must agree with it (fsck checks it).
5. **`loose_objects` has two nullable columns `file_id`,
   `file_byte_length`** for loose manifests (over 1 MiB, Files of more than
   about 7,000 chunks): without them a loose manifest would be invisible to
   FileId lookups. The whole table is read into the catalog (it only holds
   records over 1 MiB).
6. **A registered pack that cannot be read** (missing file, metadata digest
   or entry count different from its row) makes every negative or complete
   answer of the catalog an error: absence, the manifest list of a File,
   "not admitted". Positive answers found in sound packs are still served.
   This is invariant 2 ("corruption, never absence") applied to lookups
   whose answer that pack could change; it also means a Store with such a
   pack refuses new batches until repaired, as a corrupt SQLite does.
7. **Orphan packs are never reused but collected** (format 5 verified and
   reused orphan pack members). A failed batch's pack stays an unregistered
   file, never present; every exclusive writer session, when it opens its
   CAS, removes the `pack-*.lp2` files SQLite does not register (a pack is
   only registered by the session that published it, so one unregistered at
   session start never will be). A loose file left by a failed batch is
   still verified byte for byte, made durable and registered by the retry,
   as in format 5. Single-object `NativeCasWriter::put/stage/install` are
   refused in format 6: they would report success for an object that is
   never registered.
8. **Registration happens before validation.** The batch's `cas_packs` and
   `loose_objects` rows are inserted in the batch transaction before the
   admission resolver runs (format 5 does the same for the object rows of
   streamed chunks); admission bits, `objects` rows and projections come
   after validation as before. `ensure_immutables` skips the objects the
   batch itself just wrote (`existed == false`): they had no registered copy
   to disagree with.
9. **`objects` keeps `byte_length`, `verified` and `created_at_ms`** for the
   non-content kinds, and is `WITHOUT ROWID`; `object_edges`,
   `space_object_admissions` and `gc_roots` are `WITHOUT ROWID` too.
   `recovery_paths.expected_object_id`, `object_pins.object_id` and
   `gc_roots.object_id` lose their foreign key to `objects`; fsck checks that
   each resolves to an `objects` row or a registered content object
   (production writes no pins and no expected recovery object today).
10. **Phases 2 and 3 land together**: the batch writer plans file records
    before `put_ordered` from the same registered snapshot it deduplicates
    against, so a planned chunk is never found already stored.
11. **No pack handle stays open, and decompressed blocks are cached per
    snapshot.** The process cache keeps only each registered pack's
    validated metadata; a block read opens the pack through the pinned CAS
    directories and checks its length (as ADR 0026 asked: an open handle
    kept the Store directory from being renamed on Windows). The last eight
    decompressed blocks are kept by the catalog of one SQLite snapshot, so
    a later operation rereads the disk as format 5 does. A reader loads its
    catalog on first content access: a clean `status` loads none.
12. **`NativeCas::locate` of a packed v2 record** returns the offset of the
    stored bytes of the block that holds it (a record has no bytes of its
    own on disk).
13. **Audits never trust the process cache, and check every copy.** fsck
    and scrub load each registered pack's metadata from disk and check it
    against its registered digest and entry count; ordinary reads keep the
    per-process metadata cache (the policy of packs v1). fsck verifies every
    registered physical copy at its own place (each record of each block,
    each loose object), so a damaged copy is reported even when a sound
    duplicate serves its reads; logical reads only feed closure and
    reachability. One audit catalog serves the whole run. With an
    unreadable registered pack, fsck reports it once and does not judge
    Tree closure, FileId sections or content admission on a catalog that
    cannot answer absence.

## Measurements (6 November 2026)

10,000 files as above, release build copied under `%TEMP%`, median of three
runs; `status` on a clean tree (twice), `status` after appending a line to
one file, then `step` and `status` again. The 4 KiB-page column comes from an
earlier build of the same code (before the lazy catalog load).

| | format 5 | format 6 | format 6, 4 KiB pages | git |
| --- | ---: | ---: | ---: | ---: |
| `.layrs` total | 43,819 KiB | **2,833 KiB** | 3,205 KiB | 1,655 KiB |
| SQLite | 33,156 KiB | **204 KiB** | 572 KiB | none |
| packs (content + Trees) | 9,294 KiB | **1,264 KiB** | 1,268 KiB | 805 KiB |
| change index | 528 KiB | 528 KiB | 528 KiB | 784 KiB |
| projection cache | 828 KiB | 828 KiB | 828 KiB | none |
| `init` | 4,149 ms | **1,841 ms** | 1,967 ms | |
| `status` / again | 107 / 97 ms | 112 / 100 ms | 103 / 95 ms | |
| `status` dirty | 130 ms | 130 ms | 121 ms | |
| `step` | 276 ms | **223 ms** | 221 ms | |
| `status` after `step` | 101 ms | 93 ms | 95 ms | |

The `init` pack is 1,249 KB: 479 KB of compressed blocks (the content and
the 11 Trees, about 44 bytes per Tree entry of JCS) and 770 KB of metadata,
about 77 bytes per file record (FileId and ChunkId, 32 bytes each, plus
location, recipe, manifest prefix and the two alias permutations). All
10,000 files are file records. 1 KiB pages keep the same timings and save
368 KiB, so format 6 keeps them. The clean `status` wall times differ by
less than their run-to-run spread (±10 ms); its `LAYRS_PERF` profile is the
same in both formats (88.1 ms in format 6, 88.5 ms in format 5, of which
1.7 ms load the catalog to read the Step and its Trees, now in a pack).

Not reached in this phase: pack + Trees stay above the 1,000 KiB target
(1,264 KiB), and the whole `.layrs` (2,833 KiB) above git (1,655 KiB). The
two ids per File are the floor of the metadata without hashing content; the
Trees need the binary codec of phase 4; the change index and projection
cache (1,356 KiB together, unchanged by this work) are now the largest
part.

## Blockers before format 6 becomes the default (phase 6)

**Council s17 (2026-11-12, design round, Astra, Sol, Opus; brief
`council-s17-design.md`).** Format 7 stays opt-in until both blockers are
removed and the measurement gate passes; only then does `init` create
format 7 (new Stores only: existing Stores keep their format; conversion
and `layrs store repair` come after). Agreed by all three, in this order:

1. B1a, no schema change: writers act on positive evidence only. An
   uncertain answer (a registered pack unreadable) means "not proven",
   never "absent": deduplication writes its own copy, `new_objects` and
   the capture's known-chunk hints keep what they cannot prove, admission
   closure is satisfied by what the batch carries, and an unprovable
   closure fails that batch only, naming `step --full`. Public reader
   answers stay complete (an uncertain negative is still an error).
   Found while writing its test: a full capture whose folder equals its
   base still has to carry what the Store can no longer prove (a recovery
   batch of content and admissions, without a new Step); today it returns
   "unchanged".
   Done (`7212ac4`).
2. B1b (done, ADR 0028 section 8), the last change to schema v3 (existing opt-in Stores are then
   refused, as for inline packs): one `cas_pack_keys` row per registered
   pack (inline ones too), written in the registering transaction
   (`register_compact`, `consolidation::register`), bound to the
   registered index digest and checksummed, read only for packs found
   unreadable. Keys are tagged by form (explicit kind and id, FileId
   section, file record FileId, ChunkId alias, full FileManifestId alias).
   Filter: a Golomb-Rice coded set written in-house (deterministic, a
   stable encoding fsck recomputes exactly), its parameter stored per row
   (Opus: 32-bit fingerprints, a false "uncertain" per key of 2^-32;
   Astra and Sol preferred a binary-fuse 8 dependency at ~9 bits per key;
   the chair chose GCS with a per-row parameter, set from measured size).
   Also a layout-independent `keys_digest` of the ordered logical keys and
   a digest per 256 ordinals, for exact repair later (they can only be
   computed at registration). Reserved `cas_packs.lost_at_ms`.
3. B1c: fsck checks every readable pack's keys row against its index and
   judges what stays provable around an unreadable pack.
4. B2a (done, `b915318`): consolidation is due only when a group fits the
   budgets (a batch no longer reopens the CAS to find nothing).
5. B2b: one maintenance engine, foreground command and detached pass:
   choose a group under a short shared lock, build and verify it with no
   Store lock into a temporary name orphan collection ignores, then under
   the exclusive lock check every candidate is still registered as chosen,
   publish, and register with the existing exact remap (reading the
   current bitmaps). No grace period: the pack is published by the session
   that registers it.
   Done: `layrs maintenance` (`scm_v1::maintain`). One pass at a time
   (`metadata/maintenance.lock`, else `Busy`); the group is a geometric
   one within half the reader's limits only (`Budget::MAINTENANCE`),
   leaving out the packs consolidation skipped; the pack is built as
   `objects/packs/.maint-<id>.tmp` (synced and verified), which a crashed
   pass leaves and the next removes; a candidate retired meanwhile (or its
   file collected) abandons the pass and unlinks the file (`Abandoned`).
   Tests: `scm_format6` `maintenance_*` (consolidation, admissions made
   while it builds, moved candidates, busy and crash cleanup), CLI
   `native_maintenance`.
6. B2c: the batch path marks maintenance due (from its catalog); the CLI
   starts it detached after releasing its lock; `LAYRS_AUTO_MAINTENANCE=0`
   disables it.
   Done: a batch that did not consolidate notes a pass due when at least
   `CONSOLIDATE_AT_PACKS` pack files remain (skipped ones left out) among
   which a maintenance group exists (`consolidation::maintenance_due`,
   decided from the batch's catalog; the skipped list is read only past
   that count). The note is per process (`take_maintenance_due`); the
   CLI, once its output is written, starts `layrs __layrs-maintenance
   <store root>` detached (no window, own process group, null standard
   handles, working directory outside the checkout) unless a pass holds
   the maintenance lock. The detached run makes passes while they
   consolidate, at most 8. A maintenance candidate with a malformed
   bitmap or unreadable records is recorded as skipped, as by a batch.
   Hosts other than the CLI (Desktop, Studio Web) do not start it yet.
   Council s19 r1 (Astra 6, Sol 7, Opus 7): the Store sessions of a pass
   wait as a batch's do (shared 5 s, exclusive 60 s, keeping the built
   pack) and a wait that runs out is `Busy`, not a failure; general
   failures are recorded (`maintenance-failed`) and pause automatic
   passes as consolidation pauses (2 in a row, 1 hour; an explicit pass
   still runs and clears them); a batch decides "due" after its own
   consolidation (none due, paused, failed or nothing found) and counts
   the pack it adds; the remap is computed from the verified temporary
   file before the exclusive session, which only publishes and registers;
   the candidates' files stay open while copied; temporary packs are
   removed through the pinned packs directory, and by any exclusive
   session when no pass holds the maintenance lock; the lock file is
   opened without following links; cleanup after the commit is best
   effort.
   Council s19 r2 (Astra 6, Sol 8, Opus 8 ship): a pass keeps the
   maintenance lock until its outcome is recorded, and records only once
   it held it; a failure note's temporary file is removed only when this
   call created it; an absent lock file counts as idle for the cleanup of
   temporary packs (a crash may lose its unsynced entry); malformed-bitmap
   scans are bounded per pass; a busy Store while recording damaged packs
   is no failure; the temporary pack read back for the remap must carry
   the sealed digest; the CLI reports a later pass's failure as a warning
   and words `Busy` for both causes; on Windows the detached pass breaks
   away from a kill-on-close job when allowed.
7. Measurement gate: 10,000 and 100,000 files, history, the 1.5 GiB file,
   a Store past 64 MiB of packs with maintenance, a lost pack end to end,
   clone size; no command slower than format 5.
8. The flip: `creation_format_version` defaults to 7 with an explicit
   `"5"`; CI runs the suites unset and with `LAYRS_STORE_FORMAT=5`.

- **One unreadable registered pack refuses the whole Store** (Deviation 6,
  council r2, P2). Every absence, admission and manifest-list answer then
  errors, so capture, step and every new batch fail, including the batch
  that would bring the lost content back, and there is no repair path.
  Proposed: a per-pack negative filter (a Bloom filter or the sorted 4-byte
  id prefixes) stored with the pack and covered by its registered digest,
  so ids it excludes keep exact "absent" answers while the pack is
  unreadable; and `layrs store repair`, which republishes the lost objects
  (from the server or the working tree) as a new registered pack and keeps
  the old bitmaps.

  **Decided by council s2 r1 (2026-11-07, unanimous on the points below;
  this replaces the proposal above).**
  - The filter cannot live in or next to the pack (it is what gets lost):
    one row per registered pack in SQLite (`cas_pack_keys`), written in
    the transaction that registers the pack, bound to the registered
    digest, covering every key the pack answers for (explicit kind and id,
    FileId, ChunkId alias, FileManifestId alias). An xor or binary-fuse
    filter of about 9 bits per key (about 34 KiB per 10,000 small files)
    is loaded only when a pack is unreadable. A key it excludes is an
    exact "absent from that pack"; a key it includes, while the pack is
    unreadable, stays an error (never absence, never presence).
  - Writers never need "absent": when a lookup has no positive answer and
    a registered pack is unreadable, the batch writes its own copy
    (duplicates are legal). Admission answers stay positive-only, so a
    closure provable only through the lost pack fails that batch, unless
    the batch carries the objects.
  - Repair must keep the old admission bitmaps meaningful, so it restores
    the lost pack's exact keys and ordinals, never a reordered pack:
    either from an independently recoverable copy of its index (Astra,
    Sol), or by splitting the registered digest into a keys-and-ordinals
    digest and a layout digest, plus a digest per group of 256 ordinals,
    so a pack rebuilt from candidates (working trees, server, other packs)
    proves it holds the same keys in the same order (Opus). Choice left to
    the implementation; both are exact.
  - What cannot be restored stays registered as `lost`: its row, filter
    and bitmaps are kept for ever (admissions are never withdrawn), its
    bits are never moved to another pack, and objects found again are
    admitted only by new exact acts (capture, fetch).
  - Consolidation (phase 5) is required before the flip, as a size and
    speed requirement (every lookup probes every pack; git consolidates by
    default): geometric policy; replacement packs durable first, then one
    transaction registers them, remaps every bitmap exactly old to new
    (per Space: every admitted old ordinal maps by form and identity to a
    new ordinal naming the same identity, the new bitmap is read back by
    its key and compared byte for byte, all in the same transaction) and
    retires the old ones; retired files deleted later under the write
    lock; a `lost` pack is never consolidated.
  - Implemented (council s6): triggered at the end of a format 6 write
    batch, under the same exclusive lock, at 6 registered packs; groups by
    size tier, each next pack at most twice the group's total, within half
    the reader's limits and inline budgets of 64 MiB on disk and 256 MiB
    decompressed. A candidate whose own bytes fail (a damaged record, a
    registered file missing) or whose bitmap is malformed is left out and
    named in `metadata/consolidation-skipped`, so later sessions do not
    try it again; the sound packs still consolidate. A plain I/O failure
    blames no candidate. After two general failures in a row
    (`metadata/consolidation-failed`), consolidation pauses for an hour,
    as git pauses its automatic gc. Both notes are read without following
    links, bounded, and replaced by a new file renamed over them. The
    registration transaction remaps one Space at a time and checks that
    the admission rows it deletes are exactly those it remapped. Retired
    files are removed when the CAS is opened exclusively again; a reader
    holding an older catalog reloads it when a pack it names is gone.
  - Inline packs (council s7 Q1, decided with Opus's design: Astra and Sol
    judged it credible): a batch's pack whose whole image is at most
    256 KiB (council s13 Q1; `LAYRS_CAS_INLINE_MAX`, 0 disables) is not a
    file. Its exact pack v2
    image is a row of `cas_pack_inline` (pack_seq, image), inserted with
    its `cas_packs` row in the batch transaction: durable with the
    registration at the checked checkpoint, so the batch publishes no file
    and skips its durability barrier. A pack is written in memory and
    spills to a staging file once it outgrows the limit. The image is
    validated as untrusted input before registration (as a reread file),
    and read from SQLite with the catalog: bitmaps, the exact remap, audits
    and damage rules are those of every pack (a damaged block fails its
    records; fsck names `metadata/layrs.sqlite3#pack-...`). A pack's form
    is declared (`cas_packs.stored_inline`): a missing image is corruption
    of that row, never read as a file, and a file carrying an inline pack's
    name is never that pack. A scrub reads the images from the database
    now. Inline packs do not count toward the 6-pack trigger: at 8 of them
    or 1 MiB together (council s13 Q1) they merge, as many as fit the
    consolidation budgets (on disk, decompressed, reader limits; one that
    does not fit is skipped), inline while the result is at most 64 KiB,
    else into a pack file. An image is read only
    once its length is within the limit; only an error the stored bytes
    prove damages its pack, any other fails the catalog (retried). Orphan
    collection keeps every registered name, whatever form its row declares
    (council s8 r2, r3).
    The limit is clamped to the column's 1 MiB. Only format 6 batches skip
    the barrier (a format 5 batch may reuse a pack whose own barrier never
    ran). `cas_packs.pack_seq` is AUTOINCREMENT: a sequence number is never
    reused. The test helpers that damage copies or force the limit exist
    only with the `test-support` feature (council s8 r1). Schema v3 checksum and fingerprint changed: format 6
    Stores created before this are refused (format 6 is not the default).
    Measured: a Room capture's `cas.put_ordered` 18 -> 4 ms, no barrier.
  - Known limit, a blocker before phase 6 (council s6 r3): tiers above the
    inline budgets are never consolidated. A Store past about 64 MiB of
    packs keeps one pack per 32-64 MiB written, and a batch at 6 or more
    packs opens a consolidation that finds no group. An explicit
    maintenance pass (`layrs store gc` or a detached pass taking the lock
    itself, with a grace period for orphan collection) is required.
    Lifted: B2a (a batch opens a consolidation only when a group fits) and
    B2b (`layrs maintenance`, built outside the lock under a temporary
    name, so no grace period); B2c starts it detached.

Known limits before phase 5 (now lifted for inline tiers): every batch
added one pack, so lookups and the catalog load grew with the number of
packs, and the process cache kept the metadata of every pack it had read
(the cache now drops the names its Store no longer registers).

Test suites run with and without `LAYRS_STORE_FORMAT=6`: store-local (all,
including `scm_format6.rs`), client-core `native_*`, CLI `native_rooms`.
Tests that exercise format 5 internals (v2 SQL rows, loose and v1-packed
layout) create format 5 Stores explicitly (`init_with_format`). Under format
6, three client-core tests stay format 5 specific by construction:
`native_cas_preflight` (reconciles `objects.storage_path` with loose files),
`native_worktree_retention` (deletes a Tree's loose file) and
`native_capture_growth` (counts `objects` rows by kind; it also fails in
format 5, earlier, on a one-byte growth during idle polling that predates
this work).
