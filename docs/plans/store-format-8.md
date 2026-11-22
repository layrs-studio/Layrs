# Store format 8: history out of SQLite (council s24, s25)

Goal: a Store no larger than git's on long histories, weakening nothing beyond
what git itself does. Measured on 1,000 Steps of a 100-file folder (release,
format 7, automatic maintenance): Store 11.1 MiB against git 12.8 MiB as left
and 0.8 MiB after `git gc`; SQLite 10.3 MiB of it (about 10.3 KiB per Step),
objects 717 KiB (after the clustered consolidation of s24 phase 1).

## Council s25 (design, round 1, 2026-11-17)

Astra (GPT-6 Astra, xhigh), Sol (GPT-6.1 Sol, max) and Opus (Claude Opus 5.5,
xhigh) agree on:

- Format 8 = schema v4 + packs v4. Opt-in (`LAYRS_STORE_FORMAT=8`) until
  conversion, fault tests, consolidation and measurements pass; then `init`
  creates format 8 (format 7 is never made the default).
- Operations (record kind 11) and StepObservations (kind 12) become records of
  the batch's pack, with a stable history sequence (a clock in SQLite, bumped
  in the batch transaction) replacing `rowid`, and pack-local sections:
  observations by Step, Operations and observations by Space, actor/device
  postings. Their ids are not content addresses: every read checks the
  decoded id, and a pack's `keys_digest` commits to each entity's payload
  digest and sequence. Consolidation copies sequences and compares duplicate
  bytes; an unreadable pack that may hold an id blocks a different payload
  under it.
- The derived projections go: `objects`, `object_edges`, `steps`,
  `step_parents`, `repository_views` and its child tables,
  `space_object_admissions`, `operations` and its child tables,
  `step_observations`, the unused `pending_*` tables. Canonical records are
  the only source; edges come from one typed dependency decoder shared by
  admission, fsck and GC. Every dropped foreign key becomes an explicit
  writer check (recovery writers included) and an fsck check.
- Admission bitmaps for every packed object kind (Trees, Steps, Views,
  Symlinks, Tombstones, Conflicts, Products); loose catalog for every kind.
  History records are never admitted.
- GC: ordinary writes never scan history (council s23). Roots from mutable
  state stay incremental; roots of history (every Operation's View, rewrite
  sources and targets, observed Steps, markers, releases, Products) are
  derived when fsck or GC runs. An incomplete inventory never authorizes a
  deletion.
- Conversion (`layrs store convert`) is an offline, journaled,
  identity-preserving copy of the source's records, verified (clean fsck of
  the target and equal logical digests), never a replay of history through
  the resolver: replay could refuse legitimate old histories (the
  `OperationDag::from_history` integration order). This amends ADR 0028 §5
  ("rejouera l'historique en lots validés"). The Store root and `metadata`
  directory keep their identities (Worktree locators bind them).
- Receipts keep every field today's replay and VFS recovery compare.

Risk raised by Opus, to review first: the durable horizon. Format 7 registers
a batch's pack before validating it; non-content objects only become visible
when their `objects` rows are inserted after validation. Without those rows,
a validating reader would find the batch's own Steps and Operations in its
pack and treat them as durable (bounded admission would skip them). Format 8
reads non-content objects and history records, during validation, only from
packs registered before the batch. Test: a batch carrying a new Step whose
parent is missing is refused.

Chair's decisions where the members differ:

- Receipts stay in SQLite, compact (Sol): `applied_transactions(transaction_key
  BLOB PRIMARY KEY, receipt BLOB) WITHOUT ROWID`, a reversible key and a
  binary receipt (codec, request digest, four counts, time). In packs they
  would cost about as much (98 B per receipt measured, below) and replay would
  depend on pack readability (Opus); blocks (Astra) add rewriting for little.
- `gc_roots` keeps only roots maintained from mutable state (Astra, Sol).

## Phase 2.0 measurements (prototype `scratchpad/s24proto`)

History records dumped from a real 1,000-Step Store, packed by kind with a
40-byte index entry each: Operation 479 B raw, 109-119 B packed; observation
497 B raw, 117-126 B; receipt 294 B raw, 98-104 B. History in packs: about
0.33 KiB per Step (gate: 0.45). Projected format 8 total: objects 0.72 +
history 0.33 + SQLite about 0.1 = about 1.1 KiB per Step, against 0.82 for
`git gc`: phase 3 (binary Tree codec: objects 0.72 to about 0.34) is needed to
pass git.

## Landing order (one council review each)

1. 2a: format 8 opt-in, schema v4 draft, packs v3. Drop the object
   projections and `space_object_admissions`; all-kind bitmaps; the durable
   horizon; fsck from decoded objects. Gate: suites in formats 8, 7 and 5;
   lost-pack tests in 8; no latency regression at 10k and 99k files.
2. 2b: packs v4 with Operation and observation records, sequence clock,
   initializers, identities; client history paging; drop their tables.
3. 2c: compact receipts; freeze schema v4 and packs v4 (earlier format-8
   Stores refused).
4. 2d: `layrs store convert` from 5, 6 and 7.
5. 2e: default 8.

## Phase 2a progress (2026-11-17)

- Draft 1 (`c56d89e`): every object kind admitted by the pack bitmaps; schema
  v4 draft 1 drops `space_object_admissions`; fsck rebuilds it into a
  temporary table from the bitmaps. `684adee`: the v4 fixture is kept LF
  (its pinned checksum broke every Store creation on CRLF checkouts).
- Draft 2 (schema v4 draft 2, `native_scm_v4_history_out_draft2`): drops
  `objects`, `object_edges`, `steps`, `step_parents`, `repository_views` and
  its four child tables, and the four `pending_*` tables.
  - Writer: no projection rows (a Product keeps its `products` row). The
    roots of a Worktree follow its active Operation alone (Operations and
    Views never change), so format 8 replaces them only for the Worktrees a
    batch moves, resolved in Rust while the validation reader exists
    (Operation, then View, from the batch or the reader). The debug
    comparison of `gc_roots` with the full projection is skipped in format
    8: fsck checks it.
  - Readers: records that are not content are read through the catalog, as
    content is; projection cross-checks have nothing to compare.
  - Durable horizon (Opus, s25): the batch's pack is registered before
    validation, and without `objects` rows its own Steps would look durable
    (`new_batch_steps` skips a Step `get_step` finds). The validating reader
    of a format 8 batch reads records that are not content only from the
    packs and loose objects registered before the batch
    (`DurableHorizon`). Test: `a_new_step_is_never_read_back_from_its_own_batch`
    (formats 7 and 8; without the horizon format 8 refuses through a later
    check, `AdmissionRejected`, instead of `not_found`).
  - fsck: every registered record is read (`history_objects`), then its
    projection rows are written into temporary tables by the writer's own
    code (`insert_history_projection`); every check runs unchanged.
  - Metadata compaction skips the `pending_*` tables a schema lacks.

## Council s26 r1 (code review of phase 2a, 2026-11-17)

| Member | Verdict | Score |
| --- | --- | --- |
| GPT-6 Astra (xhigh) | ship after fixes | 6/10 |
| GPT-6.1 Sol (max) | ship after fixes | 7/10 |
| Claude Opus 5.5 (xhigh) | ship after fixes | 7/10 |

- Confirmed, fixed (all three): format 8 lost the foreign keys of the
  projection rows. `references.rs` lists, typed, every reference they
  guarded (records: Tree entries, Step root and parents, View heads, base
  Steps and materialized Trees, Product dependencies; rows: Spaces, Layers,
  Markers, Releases, Worktrees; entities: an Operation's View and rewrites,
  observations, Marker and Release targets, the current View). The writer
  checks a batch's at the point formats 5 to 7 check deferred keys; fsck
  checks the whole Store's, an id an unreadable pack may hold being unknown.
  Regressions: a carried Weave Product naming a missing Conflict is refused
  (client-core, every format; accepted by format 8 without the check); fsck
  reports a View naming a deleted Layer (formats 7 and 8; clean in 8
  without the check).
- Confirmed, fixed (Opus P3): beside an unreadable pack, format 8's derived
  tables miss what it holds; fsck no longer reports those as missing
  (`history_incomplete`, `history_unknowable`). Test: the only finding beside
  a lost pack is the pack (a GC root finding without the fix).
- Confirmed, kept (Opus P3): a record whose only copy is in an unreadable
  pack is not durable to a format 8 writer, and a reader answers unknown
  (error) instead of present: fail closed, where format 7 answers from its
  `objects` row. Pinned by `beside_a_lost_pack_format_8_does_not_answer_from_rows`;
  the Worktree root resolution now reports corruption, not a backend error.
- Fixed (Opus P3): the horizon test also runs a carried, not admitted Step;
  the v4 draft checksum is checked only for a Store that uses it (a draft
  can no longer stop formats 2 to 7); misplaced and stale comments. The
  loose variant of the horizon test (a record over 1 MiB) is not written.
- Lost-pack and consolidation tests of `scm_format6.rs` now also run in
  format 8 (keys rows, durable Tree, content-only pack, maintenance).
- Later (performance, not 2a): `products_in_space` scans every Product in
  format 8; `project_bitmap_admissions` decodes every bitmap before fsck's
  work budget.

## Council s26 r2 (2026-11-17)

| Member | Round 1 | Round 2 |
| --- | --- | --- |
| GPT-6 Astra (xhigh) | 6/10 | 7/10 |
| GPT-6.1 Sol (max) | 7/10 | 8/10 |
| Claude Opus 5.5 (xhigh) | 7/10 | 7/10 |

All three: ship after fixes; the round 1 reference fixes confirmed
complete (Opus mapped every dropped key).

- Fixed (P2, all three): beside an unreadable pack, uncertainty is now
  decided per record, never for the whole Store. A record this run read,
  and its admission bits, are known; another is unknown when an unreadable
  pack may hold it (or no audit catalog loaded). This reaches Step parents,
  the admission closure, the cross-Space admission checks (by the record
  each names), GC roots (a Worktree's by its View, a Product's by its
  bits), the current View, observation reachability (a walk meeting an
  unknown Step) and retained View dependencies. Test: Steps chained across
  two packs, either pack lost, fsck names only the pack
  (`beside_a_lost_pack_format_8_reports_only_the_pack`; StepParentMissing,
  AdmissionClosureBroken and GcRootInvalid without the fix).
- Fixed (P2, Sol, contesting deferral 9): fsck's admission projection is
  charged to the row budget as it is expanded, and stops there.
- Fixed (P3, Opus): metadata compaction lets only the `pending_*` tables be
  absent, and only from v4; the v4 baseline is named draft 3 (its fixture
  changed: Stores of earlier drafts are refused as such); the Product
  regression names the refusal of each format.
- Later (2b): the writer-level refusal tests of the row and entity
  references (format 8 shares their checks with the Product one, untested
  alone); fsck reports a dangling subtree or current View twice; the loose
  horizon variant.

## Council s26 r3 (2026-11-17, last round)

| Member | Round 1 | Round 2 | Round 3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 6/10 | 7/10 | 7/10 |
| GPT-6.1 Sol (max) | 7/10 | 8/10 | 8/10 |
| Claude Opus 5.5 (xhigh) | 7/10 | 7/10 | 8/10 (ship) |

- Fixed (P2 Astra, Sol; P3 Opus): reading a record does not prove its
  admissions. Bits are kept per copy; a record whose only copy was lost and
  that another Space carries again has a readable copy without the first
  Space's bit. Admission checks (closure, cross-Space "not admitted", a
  Product's GC root) now ask `admission_unknowable`, without the "was read"
  shortcut; presence checks keep it. Test: two Spaces, the first pack lost,
  the second Space carries the same Step again: only the pack is reported
  (AdmissionClosureBroken without the fix).
- Fixed (P2 Astra, Sol): the admission projection streams its rows and
  charges every bitmap row (one unit and one per 8 bytes) and loose row to
  fsck's CPU budget, stopping when it is spent. Test: a budget of one
  finding stops fsck, truncated, with only WorkBudgetExceeded.
- Later (2b): in format 8 a corrupt block inside a readable pack also
  yields parent, current View and GC root findings for the records it
  holds (formats 6 and 7 keep their rows); noise, not wrong data.

Council review of phase 2a is closed after three rounds (the chair's rule).

## Phase 2b design draft (chair, 2026-11-18, for council s27)

Goal: Operations and StepObservations leave SQLite (1,000 Steps: their rows
1.6 MB of 4.7 MB, plus most of the 2.4 MB of indexes; `gc_roots` 0.28 MB of
history roots). Measured target from s24proto: about 0.24 KiB per Step in
packs. What follows is the chair's draft; each point is open to council.

1. Records. Packs v4 (header version 4, format 8 only) add explicit record
   kinds 11 (Operation) and 12 (StepObservation). Their 32-byte index key is
   `blake3_derive("layrs history key v1", kind || id)`; the body is today's
   canonical payload JSON (a binary codec can come later). Every read
   decodes the body and checks the decoded id against the requested one.
   Bitmaps never admit them.
2. Sequence clock. Schema v4 draft 4 adds `history_clock(singleton,
   next_sequence)`. The pack is sealed before `BEGIN IMMEDIATE`, under the
   exclusive Store lock: the writer reads the clock there, gives the batch's
   new history records consecutive sequences (Operations, then
   observations, in batch order), and in the transaction moves the clock by
   compare-and-set from the value it read. A record the Store already holds
   with equal bytes is not new and gets no sequence (the CAS reuses it); a
   different payload under an existing id is refused as today.
3. Pack-local history section, after the FileId section, inside the index
   (so the digest and the keys row commit to it): per history record, its
   ordinal, sequence, Space (pack-local string table), and for an
   observation its Step key, Operation key, actor and device (string
   tables); for an Operation its kind flags (initializer), actor and device.
   The keys digest adds each record's sequence and payload digest. The
   catalog builds in memory, per loaded pack: by Space in sequence order, by
   Step, and actor/device postings.
4. Readers. `get_operation` and `get_step_observation` read the record
   through the catalog (the durable horizon covers kinds 11 and 12);
   `initializer_in_space`, `actor_exists`, `device_exists` and
   `observation_projection` use the section maps; the observation snapshot's
   watermark becomes the sequence (its digest hashes sequences, not rowids;
   metadata compaction no longer invalidates it). Projection cross-checks
   have nothing to compare.
5. Writer. The batch's new Operations and observations go through
   `put_ordered` into the batch's pack (or loose over 1 MiB). No SQL rows.
   Retained-Operation and observation GC roots are no longer stored: fsck
   and GC derive them (s25). Operation-head roots are computed in Rust from
   the batch or the reader, as Worktree roots are in 2a.
6. Schema v4 draft 4 drops `operations`, `operation_parents`,
   `operation_rewrites`, `operation_rewrite_targets`, `step_observations`
   and the foreign keys into them (`worktrees.active_operation_id`,
   `operation_heads.operation_id`, `release_publications.operation_id`,
   `recovery_operations.operation_id`). `references.rs` gains history
   references (to an Operation: from a Worktree, a head, a publication, a
   recovery, an observation, a parent Operation; recovery writers included),
   checked by the writer and by fsck.
7. fsck rebuilds the dropped tables as temporary tables from the records it
   reads, with the writer's own row code, as in 2a; every check runs
   unchanged; the dropped keys become explicit checks; history GC roots are
   derived.
8. Consolidation copies kinds 11 and 12 with their sections and sequences;
   a duplicate key compares bytes (corruption if they differ); an
   unreadable pack that may hold a key blocks a different payload under it.
9. Tests: suites in 8, 7 and 5; lost-pack tests holding history records;
   the horizon with a batch's own Operation; paging across consolidation.

Open questions for council s27: (a) is the pre-`BEGIN` sequence assignment
sound under the exclusive lock, including replay and a failed batch (the
clock moves only on commit; a sealed but unregistered pack is an orphan)?
(b) section in the index versus a separate per-pack history index; (c)
whether actor/device postings belong in packs or in a small SQLite table;
(d) anything in the client (paging, cursors) that this breaks.

## Council s27 r1 (design of 2b, 2026-11-18) and chair's decisions

All three: the direction holds (records in the batch's pack, an SQLite
clock, the section in the index); the draft needed the changes below before
code. Opus scored the draft 6/10.

Decided (all three agree):

- History keys use complete semantics everywhere (admission, immutables,
  `put_compact`): an equal registered copy is reused with its sequence; a
  different payload is refused; a key an unreadable pack (or a damaged
  block) may hold is refused as unknown. Invariant: one registered copy per
  history key (fsck checks it). History ids are never repaired as content.
- The durable horizon covers every history route: point reads, Space
  enumeration, and the identity and initializer answers (served by tables
  written after validation, below).
- The sequence is assigned before `BEGIN`, under the exclusive lock, on the
  writer's connection, to the batch's new history records in batch order
  (Operations, then observations); the clock moves by compare-and-set
  (exactly one row) in the transaction that registers the pack; ROLLBACK
  leaves it. Sequences are dense: 1 to `next_sequence - 1`. An unregistered
  pack's sequences may be reused, so no path may ever register a pack it did
  not publish in the same transaction (tested with `AfterAllCas`).
  Consolidation and maintenance copy sequences, never read or move the
  clock.
- Replay: when `precheck_new` finds the receipt, no history record is placed;
  replay decides, independent of history pack readability.
- The section lives in the index, sorted by (Space, sequence), trimmed to
  sequence and Space (pack-local string table). The keys digest commits to
  each record's sequence and payload digest, computed from the bodies when
  the pack is built; fsck, which decodes every history record, rebuilds it
  from them. The filter also gets one tagged key per Space with history in
  the pack, so a Space query beside an unreadable pack fails closed, never
  partial.
- No loose history: packs v4 hold kinds 11 and 12 up to the batch bound, a
  large one in a block of its own.
- Consolidation proves the logical history inventory (kind, key, sequence,
  payload digest, Space) of the new pack equals the candidates' before
  retiring them; a duplicate with other bytes or sequence is source
  corruption. History ordinals never carry an admission bit.
- GC: reachability starts from the verified mutable roots plus the roots
  derived from every retained history record (Views, rewrite endpoints,
  observed Steps), and from Markers, Releases and admitted Products; a
  missing pack, damaged block or spent budget forbids any deletion. Stored
  `gc_roots` keep only mutable roots. Operation-head roots are replaced only
  for the Spaces whose heads a batch changes, resolved in Rust.
- References (writer and fsck, typed history references): Operation to its
  Space; observation to its Space, Worktree and Operation; Worktree record,
  `WorktreeActiveOperation` and `OperationHeads` new ids, release
  publications and recovery plans to their Operation (recovery's
  `insert_plan` checks it itself, complete semantics).
- Packs v4 if and only if format 8 draft 4; kinds 11 and 12 only in v4.
- Observation snapshots: the watermark is the sequence (0: none); the
  digest domain becomes v2 and hashes sequence and canonical payload of the
  deduplicated logical records; a format 8 metadata compaction keeps cursors
  valid (`restart_history_traversal = false`). Operation paging still walks
  the DAG.
- fsck adds: index key equals the decoded id's key; section fields equal the
  body; sequences unique, dense, below the clock; one copy per key; no bit
  on a history ordinal; one Initialize per Space; the two tables below equal
  the records; the dropped keys as explicit checks with per-record
  uncertainty (history ids included); history work charged to the budget as
  read.

Chair's decisions where members differ:

- Initializers and identities go to two small SQLite tables (Opus), not to
  pack postings (Astra, Sol): `space_initializers(space_id PRIMARY KEY,
  operation_id UNIQUE)` and `history_identities(kind, id, PRIMARY KEY(kind,
  id)) WITHOUT ROWID`, written after validation in the batch transaction.
  They grow with Spaces and identities, not with history; they give back the
  uniqueness constraint, the horizon for free and complete answers beside a
  lost pack. fsck checks that they equal the records. The existing identity
  registry and the Marker, Release and publication witnesses stay.
- No per-record payload digest stored in the index (Opus), the keys digest
  being rebuilt from bodies by fsck.

Implementation order (each step compiles, keeps formats 5 to 7 unchanged,
and runs the format 8 suites):

1. Packs v4: kinds 11 and 12, the history section, Space tags in the
   filter, version gating; builder, decoder, keys; unit tests.
2. Schema v4 draft 4: drop the five history tables and their keys; add
   `history_clock`, `space_initializers`, `history_identities`.
3. Writer: a typed history path into the batch's publication; sequence
   assignment and clock; the two tables; references; replay rule.
4. Readers: point reads, initializer and identity answers, observation
   projection and snapshot v2.
5. Complete semantics in admission, immutables and `put_compact`.
6. GC roots: heads in Rust, history roots derived.
7. fsck: temporary history tables and the new checks.
8. Consolidation: history copy and inventory proof.
9. Client and compaction cursor flags; tests listed by the council
   (self-witnessed identities, passive Operation cycles, duplicate
   initializer beside a lost pack, replay after a lost response, conflicting
   duplicate sequences, history-only packs, corrupt blocks, budgets, paging
   across append, consolidation and compaction); then council review.

## Phase 2b progress (2026-11-18)

- Step 1 (`182409a`): packs v4, history section, Space keys, version gate.
- Steps 2 to 4, 7 and 8 (in progress, uncommitted at this note): schema v4
  draft 4 (the five history tables dropped; `history_clock`,
  `space_initializers`, `history_identities`); the writer plans the batch's
  new history records before `BEGIN` (complete lookup through the session
  catalog, clock read on the writer, sequences in batch order), the pack
  carries them, `register_compact` moves the clock by compare-and-set, a
  replay places none; Initialize and identity rows; no stored history GC
  roots; Operation-head roots in Rust for the Spaces a batch moves; readers
  (point reads through the catalog with the horizon, initializer and
  identity tables, observation projection by sequence, snapshot digest v2);
  consolidation copies sequences and refuses differing copies; fsck reads
  every history record (key and canonical bytes checked), rebuilds the five
  tables as temporary tables with the writer's row code, derives history
  roots for reachability, treats a history id an unreadable pack may hold
  as unknown.
- Still to do in 2b: history references (writer and fsck: Worktree, heads,
  publications, recovery plans, observations to Operations, Operation
  parents); fsck checks of sequence density and the clock, of the two
  tables against the records, of one Initialize per Space; consolidation's
  explicit inventory proof (today by construction: sequences come from the
  sources, and the builder refuses a differing duplicate); the client and
  compaction cursor flag; the council's test list; measurement; council
  review.

Measured after phase 2b (`411fbe6`, release, 1,000 Steps of a 100-file
folder): Store 1.7 MiB (5.5 before 2b, 11.1 in format 7): objects and
history packs 919 KiB, SQLite 752 KiB, of which `applied_transactions`
421 KB of rows (plus its index) and inline pack images 63 KB. git: 12.5 MiB
as left, 0.78 MiB after `git gc`. Next: 2c (compact receipts, about 100 B
each by the s25 decision) and phase 3 (binary Tree codec; history records
could take a binary codec too).

## Council s28 r1 (code review of 2b, 2026-11-22)

| Member | Verdict | Score |
| --- | --- | --- |
| GPT-6 Astra (xhigh) | ship after fixes | 6/10 |
| GPT-6.1 Sol (max) | ship after fixes | 6/10 |
| Claude Opus 5.5 (xhigh) | ship after fixes | 7/10 |

Opus found the sequences and clock, replay, horizon and formats 5 to 7
sound. Confirmed and fixed:

- Recovery journals (P1 Sol, P2 Astra, Opus): `create` and
  `create_and_claim` check the plan's Operation through the catalog in
  format 8 (complete lookup), before the plan is inserted.
- A point lookup is complete even when a readable copy is found: an
  unreadable pack that may hold the key refuses (Astra, Sol).
- fsck reads history through a streamed visit, each read authorized
  against its budgets first; checks every record's key, canonical bytes
  and section Space against its body; reports a key registered in two
  packs, identical or not; a body that cannot be read makes its record
  unknown (per-record uncertainty, density not judged) instead of
  missing (all three).
- Admission bits on history ordinals: fsck reports them, consolidation
  refuses to remap them (all three).
- Consolidation decodes every history body it copies (key, canonical
  bytes, Space) before retiring the sources, and compares Space with
  sequence and bytes for a duplicate (Opus, Astra, Sol).
- The observation projection is bounded before any body is read (count
  and stored bytes at or below the watermark, from the section and the
  locations) and checks each decoded id against its key (all three).
- A per-pack table finds a history record's section entry (Opus).
- An abandoned batch drops its history plan (Opus).
- Tests: a replay beside its lost history pack takes no sequence; fsck
  reports an admitted history record; the CLI compaction test expects
  format 8 cursors to survive.

Kept (P3, Opus): the compaction journal accepts either cursor flag (the
layout does not know the format; the flag is advisory). Still missing
from the s27 test list: sequence reuse after an unregistered pack
(`AfterAllCas`), the horizon with a batch's own Operation, a damaged
history block, paging across append and consolidation, the large
observation population in format 8.

## Council s28 r2 (2026-11-22)

| Member | Round 1 | Round 2 |
| --- | --- | --- |
| GPT-6 Astra (xhigh) | 6/10 | 6/10 |
| GPT-6.1 Sol (max) | 6/10 | 7/10 |
| Claude Opus 5.5 (xhigh) | 7/10 | 8/10 |

Opus: the round 1 fixes are complete and correct as written. Fixed:

- History payload integrity (P0 Astra, P2 Sol): the chair's deviation was
  wrong. A history id is not a content address, and the registered index
  digest covered block geometry, not bytes: a block replaced by a valid one
  of the same geometry went unseen. Each history section entry now carries
  its body's blake3 (32 bytes), committed by the registered index digest and
  by the keys digest; every read (point lookup, Space query, fsck visit,
  consolidation copy) checks it. Schema v4 draft 5 (packs v4 changed).
- A Worktree root whose active Operation is unknown is unknown (Astra,
  Sol).
- The recovery check decodes the record and requires that Operation in
  canonical bytes (Astra).
- A Space query refuses a key registered at two sequences (Astra, Opus).
- fsck stops when its budget runs out while history is read, before
  judging references against partial tables (Opus P2).
- A history record that does not decode is unknown, not missing (Opus).
- Tests: each section entry commits to its record's bytes (unit); a
  recovery journal names a stored Operation, an unknown one is refused.

## Council s28 r3 (2026-11-23, last round)

| Member | Round 1 | Round 2 | Round 3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 6/10 | 6/10 | 9/10 (ship) |
| GPT-6.1 Sol (max) | 6/10 | 7/10 | 8/10 (ship) |
| Claude Opus 5.5 (xhigh) | 7/10 | 8/10 | 8/10 (ship) |

No P0, P1 or P2. Opus's two P3 (a misplaced doc comment, the schema
comment's draft number) are fixed. Phase 2b is reviewed; the council's
remaining test list (a replaced block at file level, `AfterAllCas` sequence
reuse, the horizon with a batch's own Operation, a damaged history block,
paging across consolidation, a large observation population in format 8)
and the explicit consolidation inventory proof stay open for before the
default flip (2e).
