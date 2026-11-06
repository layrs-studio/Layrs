# Store format 6 — implementation spec (ADR 0028 phases 2 and 3)

Status: to implement, behind `LAYRS_STORE_FORMAT=6` (opt-in). Format 5 stays
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
