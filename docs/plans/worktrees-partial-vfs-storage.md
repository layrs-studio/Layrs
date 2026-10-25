# Shared partial Store — implementation design

## L5 Stage1a schema foundation — implementation and qualification evidence

Root approved the complete private7-to-8 schema and Stage1a implementation.
`FORMAT4`, family catalog1, immutable projection history and existing recovery
bindings remain unchanged. New members still start at private7. Ordinary opening
never upgrades to8; `ProjectionStore::upgrade_provider_schema(expected_generation)`
is the explicit atomic operation. Its result is `Result<()>`.

The entire schema8 fingerprint is declared in `projection/provider/schema.rs`:

| Table | Key, indexed projections and reference contract |
| --- | --- |
| `projection_backend` | Singleton1; ordinary requires null mount, provider requires FK to mount. |
| `projection_provider_catalogs` | Digest ID; source generation FK, source snapshot digest, selection digest; payload ≤64MiB. |
| `projection_provider_descriptors` | PK(catalog,path), FK catalog; index(catalog,file_id); FileId, length, mode, exact manifest ID and canonical manifest ≤256KiB. |
| `projection_provider_mounts` | Transaction ID; catalog FK; phase; document ≤1MiB and digest. Phases: allocating, control_prepared, **control_published**, provider_prepared, active, retiring, retired, abandoned. |
| `projection_provider_activations` | Digest ID; UNIQUE(owner identity digest,mount ID); catalog FK; source and activated generation FKs and exact snapshot digests; document ≤1MiB. This is a scalar catalog/generation placement mapping, never a per-path map or history array. No live-mount FK: inherited immutable placement evidence survives independently. |
| `projection_provider_capture_transactions` | Transaction ID; mount and capture-generation FKs; generation digest; expected capture sequence/digest; state, reservation, document ≤16MiB and record digest. Constant unique partial index permits only one hashing/staging transaction. |
| `projection_provider_capture_receipts` | Transaction PK/FK; actual capture sequence FK and capture digest; document ≤1MiB and digest. Capture sequence is not unique because exact deduplication may return an existing capture. |
| `projection_provider_transitions` | Transaction ID; source/destination role; state; local-generation FK; reservation and optional receipt FK; document ≤16MiB and digest. Constant unique partial index permits only one planned/copying/staging transition. |
| `projection_provider_transition_receipts` | Digest ID; UNIQUE(owner identity digest,transaction ID); optional companion FK; compact document ≤64KiB. References immutable prefix/catalog/capture and companion IDs/counts/rowset digests, never inline namespace/history/path-map arrays. No live-transition FK: immutable import lineage remains copyable independently. |
| `projection_provider_companions` | Digest ID; file_count and deletion_count each0..100,000; files_digest and deletions_digest; compact canonical header ≤64KiB. Header binds original source identity/version and immutable prefix/catalog/capture references, counts and rowset digests. It contains no per-path arrays. |
| `projection_provider_companion_files` | PK(companion_id,path), FK companion header; FileId, length, mode, exact manifest ID, optional paired base FileId/mode, reason token and row digest. No inline manifest or file bytes. Existing private claims protect the exact shared representation; no cross-database FK. |
| `projection_provider_companion_deletions` | PK(companion_id,path), FK companion header; original base FileId/mode, optional exact base manifest ID, reason token and row digest. Preserves deletion intent without authorizing a destination deletion. |

All tables are STRICT. No later capability stage may add a table, column, index or
constraint under the same version. The declaration is complete; lifecycle writers
and typed document interpreters are deliberately unavailable in Stage1a. An exact8
ordinary singleton is accepted **only when every one of the eleven provider tables
is empty**, including imported catalogs, maps, companions and receipts. Any row
refuses ordinary access and owner reclamation before its opaque payload is read.
This preserves future inherited evidence instead of treating it as unreferenced.
The twelve-table declaration includes both normalized companion row tables before
any schema8 deployment. The earlier ten-table test draft is not a supported8
fingerprint; no same-version automatic extension is performed.

The later typed interpreters must check canonical JSON, exact `schemaVersion`,
bounded fields and byte-for-byte reserialization; every indexed SQL field must
equal the corresponding typed document projection before any authority is used.
Catalog validation must join the exact source snapshot and selection, validate
each descriptor path/target/mode and exact `FileManifest`/FileId/length, and reject
more than100,000 descriptors or more than256MiB total catalog payload plus descriptor
path/ID/manifest byte lengths (checked arithmetic). These aggregate limits are
exported constants and a mandatory acceptance/query contract, not an implemented
populated-catalog reader in Stage1a. Descriptors use the existing FileManifest
canonical contract; the other documents use table-specific `layrs.projection-provider-<kind>.v1`
schema versions. New document IDs/digests use `blake3:` plus64 lowercase hexadecimal
digits with `layrs-projection-provider-<kind>-v1\0` domains (catalog, mount,
activation, capture-transaction, capture-receipt, transition, transition-receipt,
companion, companion-file, companion-deletion). **Mount IDs, capture/transition
transaction IDs and reservation tokens are free bounded transaction identifiers,
not domain hashes**; their document/record digest columns are domain hashes.
Catalog, activation, companion and transition-receipt primary IDs are domain
hashes. Owner identity digests are domain hashes of immutable identity bindings.
Existing snapshot **and selection** digests remain their existing
raw64 lowercase hex encoding. Capture and content/manifest IDs retain their
existing prefixed encoding. No digest or row presence grants provider readiness.

Companion row contracts are fixed now; typed writers/readers remain future work.
All descriptor/companion paths use SQL `length(CAST(path AS BLOB))` in1..4096,
so the bound counts UTF-8 bytes, not Unicode characters. Future interpretation
also validates RepoPath/path semantics. Reason tokens use1..64 UTF-8 bytes and a
supported typed reason; unknown reason values grant no authority. File rows bind
replacement FileId/length/mode to an exact manifest ID; optional base FileId/mode
must be both present or both absent. Deletion rows require the original base
FileId/mode and may additionally retain an exact resident preimage manifest.

Each companion permits at most100,000 file rows and100,000 deletion rows (up
to200,000 combined for a complete replacement/deletion image). Its total metadata
budget is1GiB: checked sum of canonical header bytes and, for each row, canonical
row-document bytes plus its71-byte digest. Row documents carry their exact
schemaVersion/path/typed fields, excluding companion_id and their own digest;
this avoids a cycle between header ID and rowset roots. Hash row documents in
the companion-file/companion-deletion domains above. Each ordered rowset digest
hashes its domain (`layrs-projection-provider-companion-files-v1\0` or
`layrs-projection-provider-companion-deletions-v1\0`), big-endian u64 row count,
then each big-endian u64 path-byte length, UTF-8 path bytes and71-byte row digest,
ordered by SQLite BINARY path order. A future interpreter must stream bounded
rows, check every row digest/canonical representation, exact header counts and
rowset digests, the aggregate budget, and cross-kind unique paths before accepting
authority. Empty rowsets still have domain-separated digests. Row presence,
counts or a receipt alone never substitute for these checks or durable claims.
No persistent range table or range-state payload is permitted: read windows and
range buffers remain ephemeral under the retained reader lease. The activation
record maps scalar source/catalog/generation identities only.

Upgrade acquires the validated exclusive private access without migration or
claim settlement, checks expected generation, all filesystem/publication journals,
rebase preview, private budgets/prejournal, acquiring preparations and any shared
reservation owned by this member. It refuses staging entries. Empty plain staging
parent directories are harmless; an unowned child, even empty, is retained and
blocks upgrade. Existing successful journal engines can leave such empty children;
Stage1a does not infer ownership from their names or add an automatic cleanup.
Ready/released preparations and their existing claims survive. The complete DDL,
ordinary singleton and `user_version=8` commit in one transaction. An idempotent
retry rechecks all gates. Fault points are SchemaCreated, BeforeCommit, AfterCommit.

Ordinary access first acquires filesystem-only family bindings and the owner lock,
then retains no-follow root, `.layrs`, FORMAT, metadata and private DB identity pins.
Canonical FORMAT/native IDs are checked before SQL. Readonly exact schema/private
identity/backend validation precedes `CheckoutReader`, ordinary scanning, writer
opening and settlement. Existing Store/recovery handles repeat these checks on use.
Generic root pins may reject CFAPI roots: Stage1b's population-independent control
capability is still required; there is no ordinary-reader fallback. Bound explicit
database recovery retains the previously disclosed typed hot-rollback exception;
unsupported committed schema/backend can be refused after that rollback, without
migration. No new writable discovery fallback was added for ordinary open/resume/GC.
There is also a distinct **existing explicit family-maintenance metadata recovery
exception**: `reconcile_retention`/`evict_unretained` may perform the already
qualified typed SQLite rollback after validating family/registry and pinned
private metadata/DB identities, under the family lifecycle and exclusive owner
locks. This can restore committed unsupported-schema/provider evidence before
schema/backend refusal. It does not need a surviving checkout directory and does
not open a CheckoutReader or grant provider capability. Beyond restoring committed
SQLite pages, it makes no namespace, private-claim, shared-root, reservation,
installation or content-reclamation changes until every owner validates. The ordinary7 hot-owner/deleted-checkout
behavior remains unchanged. Readonly discovery still does not roll back anything;
provider-specific recovery effects await actual Stage1b control capabilities.
Existing all-owner reconciliation validates exact7 or exact8 ordinary owners and
finishes all owner inspections before reclaiming anything; unknown schemas or any
provider evidence abort the whole sweep. Conversion recognizes an already-copied
ordinary8 destination and never overwrites it with a schema7 backup. Interrupted
exact source-schema6 backup remains resumable before private import acceptance.

Public constants: `PROJECTION_PROVIDER_SCHEMA_VERSION`, `PROVIDER_CATALOG_MAX_BYTES`,
`PROVIDER_DESCRIPTOR_MAX_BYTES`, `PROVIDER_CATALOG_AGGREGATE_MAX_BYTES`,
`PROVIDER_CATALOG_MAX_DESCRIPTORS`, `PROVIDER_COMPANION_MAX_FILES`,
`PROVIDER_COMPANION_MAX_DELETIONS`, `PROVIDER_COMPANION_AGGREGATE_MAX_BYTES`.
Public fault seam:
`upgrade_provider_schema_with_faults(expected_generation, &dyn ProviderSchemaUpgradeFaults)`
and `ProviderSchemaUpgradePoint`. There is no provider mount/read/capture API yet.

Qualification: root ran the original nine Stage1a Rust tests successfully in
`l5-schema8-stage1a-r1.log` (30.35 seconds). Both initial independent reviews found
no P0/P1; their companion-capacity and test-adequacy corrections are incorporated
here. Those passes cover the earlier ten-table draft, not this revised fingerprint.
Root compiled client-core against the revised frozen production in31.72 seconds
and passed all three streaming export tests. Root then passed all15 focused
provider_schema Rust tests in `l5-schema8-stage1a-r2.log` (37.66 seconds). Both fresh
R2 independent reviews (critical Opus and spec Astra) report no remaining P0/P1/P2.
The serialized full projection run in `l5-projection-r2.log` passed **155/155**, zero
failures, in1007.10 seconds, against `l5-projection-r2-precompile.sha256`.
Subsequent focused root runs passed the strengthened acquiring-preparation test
in `l5-acquiring-gate-r2-fixed.log` (**1/1**, 3.41 seconds) and the singleton
diagnostic regression in `l5-singleton-diagnostic-r2.log` (**1/1**, 1.07 seconds;
compile 5.13 seconds). These used the same freshly compiled Store binary. The
diagnostic distinguishes a missing/invalid backend singleton (integrity failure)
from a valid provider backend (provider capability required); it changes no schema
or API. These later focused results do not relabel the earlier full-suite run.

Root also qualified the two test-only process recipes in
`projection/tests/shared/provider_schema/process.rs`:

- **A — killed upgrade:** `l5-upgrade-killed-process-r1.log`, compile 13.00 seconds,
  **1/1 passed in 20.23 seconds**. One test covers all three existing fault seams:
  SchemaCreated, BeforeCommit and AfterCommit. Each genuine schema7 fixture has
  accepted capture data and a ready pin. A synced nonce/phase acknowledgment
  precedes killing and reaping only the owned child. Readonly inspection is the
  first database operation after termination and preserves private DB/journal
  bytes; explicit bound rollback is permitted only for typed hot evidence. The
  test validates exact7 before commit or exact8 after commit, unchanged accepted
  table payloads, bindings/native identities and durable checkout/family file
  bytes, then successful and byte-stable idempotent retry. Across upgrade, private
  DB pages may change; original table payloads are compared instead. A surviving
  non-hot private rollback journal is retained and excluded only from the
  cross-upgrade accepted-data comparison, never deleted to satisfy the test.
- **B — retained schema7 reader:** `l5-retained-schema7-reader-r1.log`, **1/1 passed
  in 9.77 seconds**. This ignored Windows opt-in test used the explicitly supplied
  `benchmark-candidate-r5-12a33a8d81b448a4bc3adcc4958bed0b/layrs.exe` under
  `.layrs-local/validation`, with SHA-256
  `1c23a1d6d2a8806e6edae769de26651c142448000b286a8181979fd03d488ee9`
  checked before each launch. The old CLI successfully reads a genuine7 fixture;
  after explicit upgrade of that same fixture, its status and same-quota mutation
  both refuse with a projection schema mismatch. Private/family metadata,
  namespace, content bytes, native identities and SQLite sidecars remain unchanged
  by those commands. This proves **fresh-process old-reader refusal**, not an old
  cached handle surviving an upgrade.

Both recipes use bounded owned subprocesses and keep logs outside the checkout;
failed fixtures are retained. They add no production hooks. The process-kill
results do not establish forced-spill coverage or Windows power-loss durability.
The independent Stage1a R2 reviews remain the source-review evidence for the
foundation; the later focused execution results above are recorded separately.
The current source DDL separately passed SQLite3.45.3 execution checks for all
twelve tables, FK graph/fingerprint budgets and transactional version rollback;
100,000 file plus100,000 deletion rows, UTF-8 path-byte rejection, compact-header
rejection and an inherited companion receipt with no live transition also passed
in that SQLite check. This is not Rust test execution.
Rust tests cover ordinary7-to-8 behavior,
pre/postcommit interruption, idempotence, CAS, journal/staging/budget/acquiring
refusal, ready-pin preservation, specific stale-handle fence errors, all eleven
evidence fences, unsupported-owner zero reclamation and exact-fingerprint/legacy
refusal. New tests enable FKs for inherited activation without a live mount and
companion receipt without a live transition, check real open/state/capture refusal,
exercise private-budget-only/prejournal/publication gates and every staging slot,
UTF-8 byte limits and compact headers, and store100,000 rows of each companion
kind without a large header. A killed-writer populated8 regression checks that
readonly discovery leaves the hot DB unchanged, explicit family maintenance
restores exact committed companion/claim rows, then refuses reclamation with
shared catalog/reservations/object bytes unchanged. Previous L3 passes do not
qualify these changes. Windows power-loss remains unqualified;
provider activation, managed member transitions/companions and expansion are later
required stages, not completion claims for this foundation.

Stage1b remains read-only design with **no production or dependency grant**. The
corrected associated-static bootstrap discovery contract below received independent
SPEC R2 review with no P0/P1/P2; this is contract review, not implementation
qualification. Observed directory bytes/identity remain data only. Canonical
root/ancestor preflight pins remain held through publication, while expected-open
or a separately qualified publisher alone may yield a retained control guard.
A persistence error has unknown durability and requires rereading the external
receipt. Neither preliminary control evidence nor cloud-tag recognition grants
registration, complete population, source-read or activation authority.

Native feasibility remains a separate gate. Both prior retained-FORMAT recipes
failed directory rename with Win32 error5: the strict shareREAD pin in
`l5-control-directory-feasibility-r1-fixed.log`, and the temporary retained
shareREAD|DELETE/no-WRITE pin in `l5-control-retained-handoff-r1.log`. Both failure
logs and recipes remain preserved; no close-and-retry fallback was added to them.
Root's separate `l5-control-close-window-r1.log` passed **6/6 probe arms within one
Rust test** (0.22 seconds; compilation 2.45 seconds), against source manifest
`l5-control-close-window-r1-source.json`, all under `.layrs-local/validation`.
The clean arm reacquired the final strong pin and checked exact native IDs, bytes,
EOF, length, single link and closed layout; replacement, changed bytes, changed
length, extra child and missing file were refused. This is bounded close-window
feasibility, not atomic namespace, process-crash, power-loss, source/readiness or
Store integration qualification. Critical provider review still requires handle
interoperability, registration/window, third-party-reader, ADS, link, collision
and ACL test gates; those probes are being drafted separately. No production
publication protocol is approved by this result.

## L5 Store contract revision R2 — proposal only, 2026-10-24

**Read-only design; no implementation/API grant.** All L3 backend production stays
frozen through the current client/CLI qualification. Root reports full projection
140/140 passed in `l3-projection-r6.log` (1030.44 seconds); the strengthened genuine
schema-5 exact test is a separate run. Those results do not qualify L5. This section
revises the proposed Store seams for the five critical design corrections; the
parent owns [convergence](worktrees-partial-vfs-convergence.md) integration and any
schema-8/dependency grant. Windows power-loss durability remains unqualified.

### Capability and source phase

Keep `ProjectionProviderStore` separate from ordinary `ProjectionStore`. It retains
a cloned actual `ProviderRootGuard`, never a DTO asserting a complete namespace.
An immutable `ProviderSourceBinding` identifies family incarnation, member/private
database, mount transaction, registration/control identity, catalog digest, source
generation/digest, selection and observed Step/root/authority. An activation map
binds that exact source to its placement/capture generation. Historical imported
maps are distinct immutable evidence; absence below concerns this mount's map.

| Private phase and exact map, read under one private lock | Source reads |
| --- | --- |
| `ProviderPrepared`, no activation map for this mount | Allowed only with complete native population and exact root guard, ticket, catalog and source generation. |
| `Active`, one matching committed activation map | Allowed after validating both immutable generation digests and unchanged targets/Step/root/selection/rights across placement. |
| `ProviderPrepared` with a map; `Active` without a map or with a different map | Corruption; never infer a map from the current head or equal FileIds. |
| SQL validation pending, incomplete native population, or another lifecycle phase | No content authority; typed recovery/busy result. |

Proposed signatures (not callable APIs):

```text
ProjectionProviderStore::open(root, &ProviderRootGuard) -> Result<Self>
ProjectionProviderStore::source_binding() -> Result<ProviderSourceBinding>
ProjectionProviderStore::activate(&Checkpoint) -> Result<ProviderActivation>
ProjectionProviderStore::open_range_reader(&ProviderSourceBinding)
    -> Result<ProjectionRangeReader>
ProjectionRangeReader::read_exact_at(&FileId, offset, &mut [u8], &Cancellation)
    -> Result<(), ProviderReadError>
```

The adapter may expose a fixed catalog before population, but its source-read gate
stays closed until it can attach the complete root proof and Store-validated source
binding. It does not manufacture a guard to satisfy initial session construction.
An initializing registry member is readable only through its exact owned bootstrap
or transition binding. New-member user mutations remain fenced until transition
readiness; only that transition token may accept its required destination capture.
Each range call takes the family lifecycle lease before the private reader lock,
checks phase/map together, and resolves one exact installed manifest. Retain those
guards through intersecting chunk verification and the final buffer copy. Bound
output to 256 KiB; use bounded chunk buffers. Copy no partial success to the caller.
Captured/protected missing representations are corruption; an intentionally
unprepared immutable source can return `NeedsHydration`. Descriptor metadata alone
never enters the shared installed-content catalog or grants remote authorization.

### Hash first, whole-image capture, known-content stages

Keep the existing proposed `capture_availability`, `begin_capture`, `stage_capture`,
`accept_capture`, `capture_receipt` and cancellation seams. Their concrete ownership
contract is:

```text
capture_availability(&mut Checkpoint, &Cancellation) -> Result<ProviderCaptureAvailability>
begin_capture(&ProviderCaptureGuard, transaction_id) -> Result<ProviderCaptureToken>
stage_capture<'g, 's>(token, &'g mut Checkpoint<'s>, &Cancellation)
    -> Result<StagedProviderCapture<'g, 's>>
accept_capture(StagedProviderCapture) -> Result<ProviderCaptureReceipt>
capture_receipt(transaction_id) -> Result<Option<ProviderCaptureReceipt>>
```

Tokens are private operation ownership, not completeness evidence. `stage_capture`
borrows the real checkpoint until acceptance/ack. It first hashes **every**
`NeedsHash` entry through the checkpoint's retained file guard, with bounded memory,
checking exact declared length and EOF. Immutable cold references stay references;
deleted entries stay explicit deletions. Common reconciliation compares the entire
effective image against the activated base and checks write authority only on the
actual diff, including mode/deletion changes. Availability uses that same reconciler
without accepting its advisory result as later capture authority.

Only after the first pass establishes all replacement FileIds/lengths does staging
reserve whole-capture capacity and install known content. Reuse
[`Lease::install_file`](../../crates/layrs-store-local/src/projection/shared/content.rs:29)
and its durable per-reservation/FileId installation intent. A second
[`Checkpoint::with_reader`](../../crates/layrs-vfs/src/windows/checkpoint.rs:209)
call clones and rewinds the **same retained file guard**; installation verifies its
FileId and length again. Reuse, resume or reject an existing owned attempt; every
physical duplicate shared stage must already be charged before writing. There is
no new unknown-hash spool and no checkout-relative `.layrs` stage. Failed acceptance
retains explicit owned content/claims for exact resume or cancellation.

Extract internal whole-image reconciliation and content admission from
[`capture/storage.rs`](../../crates/layrs-store-local/src/projection/capture/storage.rs:43),
keeping ordinary scanning as its existing evidence adapter. Provider acceptance
uses verified installed manifests and the actual checkpoint, never ordinary live
path reopening or a fabricated `Scan`. Preserve one private transaction for patch,
claims, capture head and transaction-to-capture receipt. Revalidate namespace and
binding immediately before acceptance and through ack; post-commit interference
retains the receipt and data rather than pretending the capture never committed.

Hashing/staging holds no private acceptance write lock, so a native hash hydration
can call the fixed local source without deadlocking. Lock order remains family
lifecycle, owned attempt lock, private lock, short shared catalog lock; callback
readers do not acquire the attempt lock. Acceptance requires the stored source/map,
generation and capture-head witnesses. Cancellation requires the same exact attempt
ownership and cannot discard an already accepted capture or publication dependency.

### Provider recovery before complete population

**Proposal only; static discovery correction independently SPEC R2 reviewed, no
P0/P1/P2. Implementation and publication remain unqualified.**

`ControlRootPreflight::open(root)` must retain canonical root/ancestor pins and
positively establish an ordinary fresh empty root outside every sync root, before
FORMAT construction. `discover_control_directory` returns distinct
`ObservedControlDirectory` bytes/identity **data only**. It cannot be promoted to
a guard by feeding the just-observed identity into expected-open. The proposed
population-independent `ControlDirectoryGuard::open(root, expected)` instead uses
an independently durable expected identity and validates exact root, `.layrs` and
single-link plain FORMAT identities, bounded immutable bytes and closed FORMAT-only
layout. Expected cloud-tag recognition grants layout identity only, not exclusion,
registration, population, source-read, activation or transition readiness.
`ProviderRootGuard` remains the separate later complete-readiness proof. No ordinary
`CheckoutReader` fallback is permitted.

The bootstrap restart APIs are **associated static functions**, with no family
instance or public `PartialStoreFamily::open` prerequisite:

```rust
PartialStoreFamily::inspect_provider_bootstrap(
    family_root: &Path,
    worktree: &WorktreeId,
    transaction_id: &str,
) -> Result<ProviderBootstrapInspection>;

PartialStoreFamily::open_provider_bootstrap_recovery(
    family_root: &Path,
    worktree: &WorktreeId,
    transaction_id: &str,
    expected: &ProviderBootstrapBinding,
) -> Result<ProviderBootstrapRecovery>;

ProviderBootstrapRecovery::inspect(
    &self,
) -> Result<ProviderBootstrapInspection>;

ProviderBootstrapRecovery::recover_databases(
    &self,
) -> Result<ProviderBootstrapInspection>;
```

These proposed signatures do not expose control publication or resume. Inspection
and handle construction are nonmutating even when either database requires
rollback. Ordinary family opening keeps its existing behavior: public
[`PartialStoreFamily::open`](../../crates/layrs-store-local/src/projection/shared.rs:201)
calls `inspection_catalog(true)`, which may recover a hot catalog. The provider
static entries must internally use filesystem-only `read_bound`, retain/revalidate
family FORMAT/root/metadata/catalog-inode evidence, and never call public `open`,
`open_inner`, `for_store`, `catalog(false)` or `inspection_catalog(true)` as discovery
prerequisites.

Receipt lookup is deterministic and **SQL-free**:

```text
family/worktrees/<existing BLAKE3 Worktree key>/metadata/provider-control/
    allocation.json
    prepared.json
```

There is one bootstrap per fresh member; both receipts bind its exact transaction.
Do not scan arbitrary names or query a possibly hot registry to locate them.
Allocation is canonical and bounded to128KiB, including intended FORMAT at most64KiB;
prepared is bounded to64KiB. Allocation binds family incarnation/marker digest,
Worktree/transaction, canonical root, private metadata/DB identities, external stage
parent/path/directory and newly created FORMAT inode, intended FORMAT bytes/length/
digest, and source/selection/catalog commitments. Persist ownership before FORMAT
byte writes or private SQLite initialization. An allocation whose native identities
were never durably recorded remains retained unclassified data, not an adoptable
stage. The external stage must be on the checkout volume and outside the checkout
and sync roots; family storage need not be on that volume.

`ProviderBootstrapBinding` contains the existing `RecoveryBinding`, transaction ID,
allocation digest and allocation-file native identity, plus an optional prepared
receipt binding (digest, native file identity and encoded `ControlDirectoryIdentity`).
Prepared binds the allocation digest. Use camelCase/deny_unknown_fields, bounded
identity strings and domain-separated `blake3:` plus64-lowercase-hex receipt digests;
existing recovery digest domains and native identity encodings remain unchanged.
The allocation commitment covers all paths and physical identities above. When
FORMAT is missing, the nested marker digest identifies the allocation's **intended
canonical FORMAT**, not an assertion that FORMAT exists. Only the complete bootstrap
wrapper/handle accepts that evidence; do not pass its nested binding alone to ordinary
recovery. Inspection separately reports physical control presence and SQL state.

After filesystem-only family validation, acquire lifecycle then owner shared locks,
validate the exact deterministic allocation/prepared chain and retain receipt/private
DB pins before any SQL probe. Final-present control requires expected-open from the
durable prepared identity. Final-absent control requires fresh retained preflight
matching the recorded canonical root/native identity plus exact external stage
evidence. An observed identity, matching bytes or a filename prefix supplies neither
branch's missing proof. Absent/unreadable allocation, replaced native identities,
final-present control without a durable expected identity, or unavailable required
root/stage proof yields `NeedsRepair`, without a usable member rollback capability.

Only then probe catalog through `inspection_catalog(false)`/`catalog_once(false)` and
private metadata through a readonly connection and the proposed structural/bootstrap
validator. Independently bound private probing may classify private rollback while
the family registry is unreadable; it cannot establish membership or expose phase.
Report family/private typed rollback evidence independently. Overall
`SqlValidationPending` exposes **no ticket, phase, activation map, transition receipt
or readiness**. When both databases are readable, validate exact catalog1, registry
or a precisely classified pre-registration allocation, exact private8/identity and
bootstrap receipt projections before exposing phase. An exactly owned never-initialized
DB is a distinct uncommitted bootstrap result, not a fabricated schema8 owner.

| Restart evidence, requiring no pre-opened family | Readonly result |
| --- | --- |
| FORMAT missing; allocation/root/stage/private identities independently valid; family catalog hot | Bound inspection with `FamilyRollback`; registry and phase unavailable. |
| FORMAT missing; readable matching registry; private DB hot | Bound inspection with `WorktreeRollback`; private phase unavailable. |
| Both databases hot; filesystem receipt bindings independently valid | Both rollback reasons and `SqlValidationPending`; no SQL-derived membership/phase/readiness claim. |
| Required receipt or physical proof missing, unreadable or mismatched | `NeedsRepair`; retain all data and issue no member rollback capability. |

`open_provider_bootstrap_recovery` repeats this readonly route, checks the supplied
expected binding and retains exact filesystem evidence. `recover_databases` rechecks
that binding under lifecycle, exclusive owner and short exclusive catalog locks
**before writes**. The exclusive catalog path needs an internal non-lock-acquiring
opener/validator that consumes or borrows the already-held lock and pinned catalog
identity. Do not call `catalog_once` while holding that same catalog lock: it acquires
its own lock at [shared.rs:409](../../crates/layrs-store-local/src/projection/shared.rs:409).
Probe readonly through the locked helper, then allow one RW recovery per hot DB only
for exact `SQLITE_READONLY_ROLLBACK`; unsupported schema, corruption and generic
readonly errors are not retries. No nested catalog lock acquisition is permitted.

Recover family first, revalidate restored catalog/registry or the explicitly recorded
pre-registration state, then private metadata if its independent allocation evidence
still matches. Revalidate all pins/digests between stages and before return. If family
rollback succeeds but registry/private validation fails, report **partial metadata
recovery** and stop further effects. This is not an atomic two-database rollback;
do not hide the successful first recovery behind an unchanged-data claim. Discovery
never rolls back. Recovery performs no migration, file creation, stage repair, marker
publication, readiness transition or reclamation. Typed rollback may restore committed
unsupported metadata before refusal, consistent with the existing disclosed boundary.

Fresh bootstrap retains the current
[initial validation order](../../crates/layrs-store-local/src/projection/format/family.rs:56):
canonical snapshot, selection/platform validation and refusal of **any `Materialized`
entry before member, stage or receipt allocation or writes**. Do not normalize such
entries to cold during creation or resume. Required future tests cover restart-only
static discovery for each missing-FORMAT/hot combination, unchanged DB/journal/receipt
bytes during inspection and handle construction, expected-binding replacement
refusals, exclusive-lock recovery without reacquisition, partial recovery reporting,
unsupported-schema refusal and zero-write Materialized rejection.

This correction does not approve a publisher, Session, population, source/capture
API, provider dependency or persistent-format change. Control publication awaits
the separately reviewed native feasibility gates recorded above. Complete provider
reads/capture still require the later root proof and validated private lifecycle.

### Immutable transition prefix and readiness

Managed ordinary recovery and expanded provider mounts use fresh destination roots.
Copy the immutable source snapshot, selection, capture and accepted-receipt prefix
**verbatim**, with exact maximum generation/capture/head digests in the transition
receipt. Preserve existing import lineage and original capture identities/digests;
an already imported source must not collapse to one invented source identity.
Copy immutable activation-map evidence needed to interpret historical captures,
but exclude live provider registration/phase, attempts, locks and host endpoints.
Re-establish destination-owned shared claims; never copy source claims, reservations
or live journals as destination ownership.

**Required future interpreter boundary, accepted clarification (not implemented
shared-source conversion):** provider capture transactions and their idempotency
receipts are source-local journals, including accepted/deduplicated acknowledgements.
Their mount FK is intentional. Exclude both `projection_provider_capture_transactions`
and `projection_provider_capture_receipts` from imports. The inherited immutable
history is CapturedPatch ancestry and original identity/provenance, snapshot and
selection prefix, accepted publication receipts, immutable activation/catalog
evidence and immutable transition/companion receipts. Import an explicit immutable
closure, not a raw backup of live provider tables. Source-local acknowledgement
replay continues to use the source private DB; it is not destination readiness.

Snapshot/capture/catalog metadata alone does **not** bind every exact preimage
manifest. Counterexample: g0 observes `new.txt` Absent, so it has no file descriptor;
C1 creates bytes A; the live file later contains B and destination authority revokes
it. The companion must retain B with base A. C1 contains A's FileId/length/mode,
but no FileManifestId; the original catalog still has no A. The current source
resolves M_A from `projection_content` plus its accepted captured-content protection,
not from the catalog or activation map. See
[CapturedFile/CapturedPatch](../../crates/layrs-store-local/src/projection/capture.rs),
[captured-content acceptance](../../crates/layrs-store-local/src/projection/capture/storage.rs)
and [exact representation acceptance/lookup](../../crates/layrs-store-local/src/projection/retention.rs).

Therefore the imported immutable closure MUST preserve verified exact
`(FileId, FileManifestId, length)` mappings for every retained Capture and companion
preimage dependency in the existing `projection_content` table. The compact
transition receipt/header binds their ordered mapping count/digest to the exact
source identity, source version and immutable prefix. Resolve each mapping through
its accepted source protection under the source private/family bindings; a current
preferred-cache lookup or equal FileId is insufficient proof. Check canonical
manifest identity, FileId and length, preserve existing imported identity boundaries,
then establish destination-owned claims for those exact manifests before Ready.
The header's exact source version defines each file row's base; a prior captured
creation resolves through that effective Capture, not a guessed remote catalog base.
Missing/mismatched mappings or protection leave NeedsRepair with the source retained;
there is no partial-ready fallback. Transition quota preflight includes this entire
closure and refuses overflow before effects, without silently dropping history.
This obligation permits companion_files to omit a duplicate base_manifest_id; it
does not assert that the existing format3-only conversion already implements it.

Before future GC unfencing, typed schema8 interpreters must enumerate all provider
capture/transition reservations and exact installation/staging dependencies alongside
ordinary budgets/preparations. Hashing/staging captures and unfinished transitions
must retain their charges and content; accepted receipts may retire a reservation
only after validating durable destination claims/ownership transfer. Cancellation
cleans only exactly owned stages. Unknown states, missing receipts, invalid prefix
or representation commitments and unclassifiable dependencies stop the whole sweep.
The current all-eleven-evidence-tables fence stays until these interpreters and
fault/reclamation tests exist; source-local journal exclusion from import is never
permission for GC to ignore those journals in their owning source member.

Append destination observation and placement generations strictly **above** the
imported maximum. Labels `g_source`/`g_activated` are roles, not literal 0/1.
Expansion uses one fresh exact Step/root and authorization epoch over the entire
old-plus-added scope, including cache hits; it is not independent authority for only
new paths. The destination gets its own mount map. Imported maps grant no live
source authority and cannot satisfy that new map's absence/presence checks.

`prepare_member_transition(checkpoint, request, cancel)` returns an owned transition
token; `resume_member_transition(token, fresh_checkpoint, cancel)` revalidates source
prefix and current effective work before a ready receipt exists. After readiness,
`publish_member_transition(token)` publishes that **exact committed receipt** without
re-reading/re-executing source work. The existing proposed transition receipt must
bind destination observation/placement generations, effective overlay/deletions,
new destination capture, retained manifests, import-prefix digest and companion
receipt. Family member readiness follows this private durable receipt. Provider
mount `Active` alone never means transition `Ready`.

The recovery-only companion records both retained files and **deletion intents**.
A revoked deletion records source path, exact source/base version and target/mode,
plus its exclusion reason; it is not silently discarded and does not delete an
unauthorized destination path. File records retain exact verified manifests. The
companion digest/claims are accepted before transition readiness. Source files,
history and family remain intact throughout every stage and after failure.

### Cancellation, implementation order and remaining gates

The provider confirmed that existing sequential `with_reader` calls use the same
retained guard, but each synchronously calls `CfHydratePlaceholder` before entering
the consumer. Initial cancellation is therefore cooperative: check before the call,
immediately on callback entry, at each bounded read, between passes, and after native
return. Do not report that the token interrupts hydration or promise a wall-clock
bound while native I/O is blocked. Keep checkpoint, root/file guards, family lease
and attempt ownership until native I/O actually completes; no detached cancellation
worker, truncated success, or early pin release. Avoid mapping cancellation to
`Interrupted` where helpers would automatically retry it.

The provider's preferred later seam is an opaque borrowing
`Checkpoint::open_reader(path, cancel) -> CheckpointReader<'_>` with bounded Read and
Seek within `0..size`, no exposed File/path, and no clean Immutable hydration. It
hydrates once then permits hash/rewind/install. Truly interruptible native I/O needs
a separately implemented and tested overlapped completion/cancel/drain contract.
It is not claimed by the current synchronous API. Full L5 qualification must test
ownership through cancellation and outstanding native I/O, not just token polling.

After root grants schema 8 and the acyclic Windows Store-local→VFS dependency:

1. Provider implements control-directory/preliminary layout proof and complete
   root guard. Store adds explicit schema-8 upgrade, provider-private records and
   ordinary-mutator gates, then provider-specific bound recovery. Recovery tests
   must precede mount/read authority.
2. Implement catalog/descriptors, exact phase/map activation and retained local
   range reader; adapter connects the initially closed source gate. Qualify cold,
   prepared, missing-protected-content, restart and two-member isolation cases.
3. Extract common reconciliation and implement two-pass guarded capture with
   whole-operation quota, existing external stages and durable capture receipts.
   Qualify cancellation, failure boundaries and no scanner fallback.
4. Implement prefix-preserving managed ordinary/new-provider transitions, companion
   deletions and ready-receipt publication. Qualify source changes before readiness,
   exact publication after readiness and complete-scope expansion.

Decisive gates remaining: schema/dependency grant; actual provider control/root
capabilities; adapter's closed source gate before population; exact recoverable
bootstrap evidence at each crash boundary; inherited import-prefix/mapping schema;
and native-I/O ownership/cancellation qualification. They are dependencies and
review targets, not implicit implementation approval. No new raw spool, native
incomplete Store, ordinary provider scanner, or automatic format conversion is part
of this proposal.

## Bound-recovery R5 test and documentation follow-up, 2026-10-24

The root reports focused bound recovery 6/6 passed (`l3-bound-recovery-r6.log`)
and full projection 140 is running against the preceding test freeze. Independent
R5 review found no new P1/runtime P2, but its test-adequacy P2 was valid: changing
only `user_version` left a fingerprint that the ordinary migrator already refused.
The corrected exact regression drops `projection_selections`, restores the genuine
schema-5 fingerprint (including removing empty schema-7 tables in the unsupported
format-4 fixture), and checks raw bytes, version 5, and selection-table absence after
every recovery entry is exercised. A separate correctly created legacy checkout is
the positive control: ordinary open must successfully migrate that genuine fixture
to schema 6. Production is unchanged; the revised exact test awaits the root queue.

The unchanged-raw-bytes refusal guarantee here concerns databases without a hot
rollback journal. Explicit typed SQLite rollback recovery may first restore the
last committed unsupported-schema database and then refuse its schema. That is
journal recovery, not migration; it can change database bytes before returning an
error. Read-only inspection still performs no rollback or migration.

Compatibility entry points remain public: the five unbound static application
resumes and family bootstrap resume are retained. Product recovery routes use the
inspected binding and opaque handle; this follow-up introduces no deprecation or
API break. Existing explicit family/destination abandonment remains independent of
source inspection. A remaining P3 defense-in-depth limitation is that some internal
content/retention paths freshly call `shared::for_store`: noncooperating replacement
of a family during an operation can create a check/use race outside the retained
outer family binding. Cooperative writers are excluded by the locks; eliminating
those fresh opens requires a separately coordinated retained-family change, not a
production refactor during this test freeze.

## Offline-surface R3 bound recovery correction, 2026-10-24

The root reports the preceding freeze passed shared 47/47 in
`l3-shared-r5.log` (746.38 seconds, including the inert subprocess helper). The
following full-projection compile overlapped early entry-point wiring and failed
in `l3-projection-r5.log`; it does not establish a full 134-test pass. These new
bound-recovery changes require fresh compilation, runtime tests, and review.

`ProjectionInspection.binding` is a serializable, camelCase, unknown-field-denying
`RecoveryBinding`: full `identity`, opaque `checkoutIdentity` and `databaseIdentity`
(nonempty, at most 512 bytes), `markerDigest`, and nullable `familyMarkerDigest`.
Both digests encode `blake3:` plus 64 lowercase hexadecimal digits. They hash exact
canonical FORMAT bytes under `layrs-projection-recovery-marker-v1\0` and
`layrs-projection-recovery-family-v1\0` respectively. The separate database inode
and family digest are necessary because legacy FORMAT does not contain the private
database inode, and the Worktree locator does not contain the complete family
FORMAT/catalog identity. This is a request binding, not a persistent schema change.

`open_bound_recovery(root, &expected)` returns an opaque `ProjectionRecovery` with
the inspected marker and retained caller-root pin. Its database recovery, ordinary
initialization, five journal resumes, and reservation cancellation compare the
complete binding under the exclusive owner lock before writes. Application engines
receive that same validated `Access`; initialization derives its snapshot/selection
and registers readiness while retaining it. There is no check followed by an
unbound root recovery call. Binding verification remains in `Access::verify`.
The just-added static database/init APIs now require `&RecoveryBinding`; existing
five static application resumes remain source-compatible and use nonmigrating
recovery access. Readonly schema validation precedes their writer, including
cancellation; schema 5 cannot migrate just because a recovery button was pressed.
Typed hot-journal evidence may create a recovery handle but grants no content
authority. Explicit source-independent family/destination abandonment is unchanged.

New tests cover independently valid replacement Worktrees at the original path
(different IDs and identical logical IDs with new native identities), replacement
legacy databases with identical bytes, valid family/catalog identity drift followed
by a killed-writer hot journal, every handle/static recovery entry on schema 5 with
unchanged raw database bytes, strict binding encoding, and matching bound install
recovery. All older recovery tests now pass their inspected binding explicitly.

## Offline-surface R2 / backend R3 corrections, 2026-10-24

The root reported the preceding source passed shared 36/36 and projection 123/123
(`l3-shared-r4-fixed.log`, `l3-projection-r4.log`). The following changes require a
fresh root-run queue and independent review; they do not introduce a schema bump.

- **Restore destination:** new `RestorePreview.destination` contains the full
  projection identity and native checkout-directory identity, covered by the
  canonical reviewed digest. Apply validates it under exclusive access before
  receipt replay. Pending-journal recovery validates present bindings too. Missing
  bindings remain readable for old history, but caller-supplied unbound previews
  cannot apply/replay. An already durable old pending journal can finish only via
  the private FORMAT/inode-bound database and its recorded quarantine/file inode
  witnesses; its original digest/payload is preserved. Imported history retains
  and validates its source destination through the existing conversion receipt.
- **Read-only discovery:** `inspect_recovery` returns `ProjectionInspection` with
  typed family/private rollback, initialization, conversion, filesystem-operation,
  and publication reasons. All filesystem bindings precede SQL. Only SQLite's
  `SQLITE_READONLY_ROLLBACK` becomes typed recovery evidence; SQL schema/identity
  validation remains pending in that case. `recover_databases` explicitly permits
  the existing bounded exclusive rollback retry without migration, application
  journal replay, or claim settlement. `resume_initialization` reads the bound
  private generation-zero snapshot/selection and invokes the existing ordinary
  bootstrap engine; imports require source-preserving conversion recovery.
  Missing FORMAT or unreadable/unsupported private identities remain fail-closed.
  Caller-relative/case-alias discovery requires the same native root identity and
  the original bound path still naming that root. Pending filesystem/publication
  states report conversion ineligible with a specific reason.
- **Preparation history:** `preparation(id)` resolves one canonical private record
  independently of listing limits. New active intents are capped at 1024 before
  reserving capacity. Listing returns at most 1024 active and 64 released records.
  Release retains the most recently created 64 released records and prunes at most
  64 older records per call, after exact installation/reservation cleanup and an
  absence-of-claims check. Older oversized draft histories drain incrementally;
  they never block active exact lookup. Pruned released IDs return `None`. No active
  record, accepted capture, preparation claim, or user file is pruned as history.

R3 P3 limitations remain open: directory/file creation can precede durable private
stage tracking; private-member creation can fail before registry publication;
conversion-checkout claims have no dependency-aware release; the 64-KiB metadata
reservation can refuse a no-content preparation near quota; corrupted orphan
content has no repair workflow; several verification paths hash bytes redundantly.
Ambiguous or corrupt artifacts retain protection and require backup-assisted
recovery, not prefix-based deletion. Necessary follow-ups are exact bootstrap/stage
ownership before allocation, dependency-aware conversion-claim release, and a
separately reviewed repair protocol. These require coordinated design, not broad
cleanup in this correction. Windows power-loss durability remains unqualified.

## Backend self-audit follow-up, 2026-10-24

The root's latest shared test run passed 13 tests (`l3-shared-r2.log`). The following
implementation gaps have code and regression-test responses in the owned backend;
those passing tests do not qualify the corrections until the next root-run queue:

- **Installation staging:** random unowned attempt directories could outlive a
  reservation retry. The shared catalogue now stores a per-reservation/FileId
  installation intent before byte writes and its exact canonical manifest before
  any CAS rename. Retries reuse that attempt; unverified owned staging is cleared
  with a bounded whitelist, and verified stages finish from their stage/CAS pieces.
  Publication and charge acceptance share one transaction. Ambiguity blocks
  cleanup and keeps the charge. Eviction also retains exact active-intent objects.
- **Imported receipts:** subsequent provenance reads now recompute the imported
  publication prefix against the marker-bound digest, including missing rows.
- **Fsck counters:** count format-4 private capture replacements from their shared
  representations, deduplicated by FileId; legacy blob-table counts are insufficient.
- **Restore availability:** use the same whole-selected-capture capacity model as
  restore acceptance, including unrelated edits and preimages. Querying capacity
  does not reserve it; the engine still repeats the witnessed preflight on apply.

Already implemented: readmission atomically cancels an interrupted sweep for its
exact objects; ready conversion resume avoids another allocation. Capture/Restore preparation
receipts remain explicitly unsupported in this bounded stage; immutable `Read`
preparation feeds the working version-restore engine, whose own preflight protects
the full resulting capture. Windows power-loss durability remains unqualified.

### Independent backend review R1 responses

- **Stale capture budgets:** `cancel_shared_reservation` now safely cancels an
  interrupted capture budget after excluding all private journals/publication.
  It recovers only authenticated shared installation staging, preserves accepted
  claims and checkout edits, and permits a fresh capture after further edits.
  Other filesystem operations can own checkout-side pre-journal staging, so this
  cancellation API explicitly refuses them and requires their exact recovery.
- **Acquisition/orphan reservations:** `release_preparation` can now cancel an
  acquiring intent without network authorization. It completes verified local
  staging or removes only its unverified owned files before removing the private
  claim and releasing capacity. Existing content is pinned in the begin transaction.
  A fallback installation checks additional capacity before any staged byte write.
  Exclusive reconciliation first validates every registered owner, then classifies
  reservation references and installation intents; proven orphan/terminal charges
  are reclaimed. Unreadable owners or ambiguous staging retain their protection.
- **Historical bases:** `release_generation_retention` explicitly releases only
  a superseded generation's base claims, refusing active-base content, retained
  capture dependencies and explicit preparation references. Captured local work,
  preimages and operation-history claims remain retained. This stage has no history
  pruning: when those dependencies fill the configured limit, increase quota or
  start a separate family while preserving the old family. Eviction does not
  silently remove history and may legitimately reclaim zero objects.
- **Failed conversions:** `PartialStoreFamily::abandon_conversion` removes claims
  and registration only for a validated unready destination with no new accepted
  captures. Source and destination physical data remain in place. It works even
  when the source changed and exact resume is no longer possible; a ready or
  privately accepted destination must be resumed instead. An abandoned locator
  cannot resume or re-register itself. `resume_worktree` separately recovers an
  exact marker-bound bootstrap before registration, with no accepted history.
- **Availability cost:** root payload and exact membership checks use metadata;
  the dependency map is built once per availability query. Requested manifests
  and chunk sizes are checked without hashing unrelated retained content. Actual
  readers verify content hashes; missing protected target bytes remain corruption,
  never a remote cache miss. Full verification remains an explicit fsck/recovery job.
- **Conversion reader boundary:** `open_conversion_source` is read-only and refuses
  private schemas other than 6 before ordinary open can migrate them. Conversion
  methods repeat this check before exclusive access. Existing local unbound APIs
  remain usable for local engines/tests; remote adapters use the generation-bound
  variants and fresh acquisition authorization on each attempt.

New focused tests are in `projection/tests/shared/recovery.rs`: installation fault
boundaries, active-intent eviction protection, unknown-stage refusal, capture retry
after edits, acquiring cancellation, orphan reconciliation, two-Worktree cached
preparation pins, imported prefix tampering, counters/capacity, conversion abandon,
non-mutating old-reader refusal, bootstrap resume and base-release dependencies.

Status: implementation contract after independent preimplementation review, 2026-10-24.

### Independent backend review R2 responses (awaiting root-run validation)

These changes supersede the R1 cancellation/base-release restrictions above. The
root's 23 passing shared tests predate this R2 patch; no Cargo was run by the author.

- **Journal-free budgets:** namespace size/change limits run before reservation.
  Private schema 7 now includes exact prejournal ownership records (operation,
  pinned inode/directory identity, enumerated allowed files, canonical digest and
  resumable cleanup state). Cancel permits any operation only while no accepted
  journal/publication is pending, preserves accepted claims and user files, and
  removes only these authenticated stages. Repeated staging requires cancellation,
  preventing duplicate physical attempts under one charge. Earlier draft format-4
  schema fingerprints are explicitly rejected, with no automatic migration.
- **Hot rollback journals:** only typed `SQLITE_READONLY_ROLLBACK` triggers one
  validated exclusive catalog recovery/re-read. GC also holds the owner lock for
  typed private-database recovery. Other read-only, schema and corruption errors
  propagate. Tests kill a real child after cache-spilled uncommitted catalog/owner
  writes and verify the hot-journal condition before exercising recovery.
- **Retired generations:** release first accepts exact required manifest subsets
  for active and retained capture bases in the same private transaction, then
  removes the retired generation claims. Clean outgoing refresh bases remain
  owned by their old generation. Capture/preimage/history/preparation claims stay
  protected. Full accepted-history pruning remains outside this stage.
- **Missing bootstrap marker:** private identity plus initializing registry intent
  now precede checkout FORMAT. `abandon_initializing(&WorktreeId)` can classify an
  unready owner without the checkout, validate its private bootstrap/import proof,
  and remove registry/claims while preserving physical files. Ready owners and new
  captures refuse abandonment. Unreadable/replaced private metadata or an unproven
  schema-6 conversion-copy interruption remain fail-closed: restore the private
  metadata backup before recovery; never forget the accepted owner. Tests cover
  missing/deleted bootstrap checkouts and GC with a deleted ready checkout.
- **Acquisition capacity:** begin reserves metadata and pins existing exact content.
  Missing-file writes top up durable capacity by actual streamed length before
  writing, accounting for both spool and installation copies. Quota failures keep
  the acquiring intent resumable/releasable. Successful file progress is privately
  pinned before fetching the next file. Capacity checks include outstanding family
  reservations; unknown stage entries and unclassifiable physical remnants remain
  preserved rather than inferred from filename prefixes.
- **Capture availability:** untouched read-only paths no longer require write
  permission; reconciled scanner changes enforce modification/deletion eligibility.
- **Discovery/recovery:** `open_for_recovery(root)` validates canonical marker,
  pinned identities, family/ready registry, current head/selection, exact schema and
  import provenance using strictly read-only handles. It permits pending operation
  journals but does not replay them, migrate, settle retention or recover SQLite.
  Legacy schema 5 and unready conversions remain explicit errors.

Focused cases live in `projection/tests/shared/r2.rs`, with the old release test
updated to assert retained dependencies rather than all-or-nothing refusal. Root
owns compilation/runtime and fresh independent review. Windows power-loss safety
remains unqualified. No L5 work or actual user-data conversion was performed.

## Required review corrections

- Each shared protection has an immutable intent/root ID bound to Store incarnation,
  owner database identity, operation identity and exact manifest/chunk membership.
  The private transaction accepts its claim together with the journal/capture;
  completing a journal transfers the claim to durable resulting state in that
  same transaction. Shared finalization is idempotent bookkeeping, never necessary
  for retention safety. Exclusive lifecycle reconciliation reads durable claims,
  not only pending journals; ambiguity or unreadable owners prevents reclamation.
- Imported captures retain exact original payloads/digests, identity, sequence range,
  chain and bases under authenticated provenance. Their receipts remain historical
  under the original publication identity. Both Store and client refuse to publish
  an imported capture. The first destination capture MUST cross the identity boundary
  even when bytes/base/changes match; ordinary deduplication resumes afterward.
  Conversion refuses unresolved publication outcomes or local acceptance.
- Restore and namespace quota preflight reserves the WHOLE final selected capture,
  including unrelated dirty replacements, target staging, preimages and metadata.
  It binds this reservation to the verified checkout before any filesystem mutation.
- A version reader holds the shared lifecycle lease from representation resolution
  through its final chunk. Protection names exact manifests and chunks; a preferred
  FileId lookup is not a retention root. Local rechunking preserves FileId, never
  implicitly a remote manifest ID.
- Supported publication ordering is flushed content, no-replace publication and
  required filesystem flushes before the shared protective transaction, then the
  private receipt. Process interruption and sharing violations require tests.
  Power-loss durability on Windows is a distinct, unverified qualification gate;
  process fault injection alone must never be reported as that evidence.

The remainder records the source-based design from which this contract was reviewed.

Recommend **format 4 as a shared PARTIAL Store family**, retaining the existing projection engines and one private database per Worktree. Share only verified immutable content and its retention accounting. Do not route partial metadata through native SCM admission.

This is a source-based design recommendation for the current uncommitted worktree at `4601b16`. No files changed, Cargo commands run, or child agents created. RepoWise confirms no index.

**What exists today**

- **Base bytes are not generally retained.** Clone downloads into `GraphObjects`, then supplies a file-writing callback. Installation verifies staging bytes and **moves that file into the writable checkout**. It does not install a retained base copy. Capture persists replacement bytes only; unchanged paths and deleted-file preimages are not saved by ordinary capture. Editing a freshly cloned file can therefore remove the only durable local copy of its base. See [clone:144](../../crates/layrs-client-core/src/native_scm/remote/projection_clone.rs:144), [install:144](../../crates/layrs-store-local/src/projection/install.rs:144), [install:340](../../crates/layrs-store-local/src/projection/install.rs:340), [capture storage:66](../../crates/layrs-store-local/src/projection/capture/storage.rs:66).
- There are exceptions: restore preserves overwritten files in capture storage; rebase also persists old files; refresh retains displaced files in quarantine. These are recovery sources, not comprehensive version coverage. [Restore:143](../../crates/layrs-store-local/src/projection/restore/engine.rs:143), [rebase:307](../../crates/layrs-store-local/src/projection/rebase/engine.rs:307), [refresh:207](../../crates/layrs-store-local/src/projection/refresh/recovery.rs:207).
- Captures are patches against their recorded projection generation, **not incremental patches against the preceding capture**. The previous digest chains history. Reconstruct a capture by overlaying its changes on its own base snapshot.
- Local store capture already requires no network. Local restore currently supports captured **changed paths** and inverse restoration; the client wrapper performs remote authorization. It cannot currently restore arbitrary unchanged paths from a captured version. [Capture API:74](../../crates/layrs-store-local/src/projection/capture.rs:74), [restore targets:162](../../crates/layrs-store-local/src/projection/restore.rs:162), [remote restore:18](../../crates/layrs-client-core/src/native_scm/remote/projection_restore.rs:18).

**Format and ownership**

Use this concrete layout:

```text
<partial-store>/
  FORMAT                         formatVersion=4, minReaderVersion=4
  metadata/catalog.sqlite3       family identity, registry, content, retention, quota
  metadata/lifecycle.lock
  metadata/catalog.lock
  objects/chunks/<digest>
  objects/manifests/<digest>
  staging/<installation-id>/
  worktrees/<worktree-id>/metadata/
    projection.sqlite3           private projection schema 7
    layrs.lock

<checkout>/.layrs/
  FORMAT                         format-4 locator and checkout binding
  projection-staging/
  projection-quarantine/
```

Keep staging/quarantine beside each checkout so existing filesystem moves remain on its volume. Private histories remain in the Store when a Worktree closes or detaches.

The family marker binds `StoreId`, a storage-incarnation identifier, canonical origin, principal and Space. Initially share only within that exact identity context. Each locator additionally binds Worktree ID, checkout path/filesystem identity, Store path/filesystem identity and metadata identity. Reuse L1’s validation principles, not its native `WorktreeCreation` payload containing complete-store Operations.

Keep projection snapshots, selections, captures, pending publication requests, receipts and filesystem journals private. Their existing singleton tables need no `worktree_id` rewrite.

The shared catalogue needs:

- Registered Worktrees and lifecycle state.
- Verified chunks; canonical manifests; ordered manifest/chunk membership; `FileId → manifest` lookup.
- Durable retention roots and their object membership, with owner Worktree, operation/capture/preparation digest and root kind.
- Quota configuration, reservations and acquisition/reclamation intents.

Private schema 7 adds immutable version observations, preparation records, coverage membership and shared-root references. Store explicit absent-path observations alongside file targets; missing metadata never proves absence.

Only complete, verified file representations enter the shared content index. Remote Step/Tree identifiers remain descriptive projection metadata. Native format 2 retains its existing closure rules, as required by [ADR0008:126](../../docs/decisions/0008-worktrees-markers-store-baseline.md:126) and [ADR0024:27](../../docs/decisions/0024-editable-partial-worktrees.md:27).

**Availability must describe an operation**

Introduce typed version references: exact remote Step/root plus recorded observation; Worktree generation/digest; or Worktree capture sequence/digest with its anchored base. A moving Layer is resolved before preparation.

A coverage query takes version operand(s), normalized paths and operation. Return selection, observed access, checkout state, verified residency and retention separately, with precise blockers:

- **Read:** exact path/version binding and complete verified file content; an explicit absence is a valid absent result.
- **Edit:** supported materialized file plus the applicable complete writable observation. Preparing edit also retains its base before exposing writable bytes.
- **Capture:** stable selected namespace, applicable writable observations, stable current bytes and capacity to retain replacements. Base **bytes** are unnecessary for identifying changes; base metadata is necessary. Preserve that useful distinction for converted stores missing old bases. Existing `capture_patch` scans the selected working set: availability must disclose that effective scope rather than promise independent per-path capture.
- **Compare identities/modes:** both operands’ exact metadata; no content required.
- **Compare content:** bytes for differing present operands. Equal FileIds need no content read. Report missing left/right/base specifically.
- **Restore:** selected target content or explicit absence, current destination eligibility, generation/checkout witnesses, and capacity to preserve overwritten bytes. Preflight all paths before changing any.
- Acquisition, publication, remote reception and services remain network operations.

“Prepared” means the complete requested dependency set is durably retained. It does not guarantee future disk capacity for arbitrary edits. Legacy captures with unavailable unchanged bases can remain usable patches while reporting incomplete read/compare/restore coverage.

**Retain bytes before materialization**

Reuse `FileManifestBuilder`, canonical Chunk/File identities, bounded streaming verification and the existing capture readers. Extract content persistence behind a shared verified-content interface; preserve public `write_captured_file`, manifest and chunk-reading APIs.

At the store boundary, add retention **after staging verification and before the journal becomes ready or any checkout rename**:

- `install::install_selected`: retain staged base files before `projection_install` becomes durable.
- `refresh_projection`: retain incoming base files before `ready=true`; retain any required outgoing base before replacement.
- Restore/rebase: retain targets and recovery preimages before filesystem mutation.
- Capture: install replacement content and durable retention references before accepting its private capture row.

The existing path-only callbacks can remain compatibility adapters: the store already knows expected FileId and mode and can persist the verified staging file. This avoids pushing durability into transport.

However, the rebase callback requires an additional **base-content input**: it currently emits resolved checkout bytes, which can differ from the incoming remote base. Its prepared graph must separately offer exact incoming base descriptors/content for retention. Unchanged receive paths also bypass download callbacks; preparation must verify/import their existing checkout bytes or use retained content, never infer residency from `Materialized`. [Receive:98](../../crates/layrs-client-core/src/native_scm/remote/projection_receive.rs:98), [rebase staging:104](../../crates/layrs-store-local/src/projection/rebase/engine.rs:104).

Proposed new API contracts—not existing implementations—are `availability`, `prepare_offline`, `preparation_status/resume`, `release_preparation`, `read_version_file`, `compare_versions`, version-based restore preview/apply, and clean dematerialization preview/apply. Extend `RestoreSource` with an exact version source; reuse its existing preservation, preview and recovery engine.

**Preparation, pins and quota**

Preparation should persist a bounded intent and quota reservation, acquire/verify missing content, durably retain the complete dependency set, then atomically publish one private ready receipt. Cancellation or failure leaves a resumable/cancelled intent, never partial success.

Private and shared SQLite commits are separate. Use an explicit asymmetric protocol:

1. Shared content and protective root become durable.
2. Private capture/preparation/journal transaction accepts references.
3. Finalize shared ownership bookkeeping.

Crashes may leave excess retention, never accepted dangling references. Release in reverse dependency order: durably remove the private claim before releasing its shared root. Do not expire protection by elapsed time alone; reconcile against the owner database. An unreadable owner blocks reclamation.

Protect accepted capture replacements, prepared versions, active materialized bases, restore preimages, pending publication, recovery journals and active readers/installers. Retaining a capture’s complete selected state also retains its unchanged-base dependencies; otherwise report that capture’s limited coverage explicitly.

Quota counts unique physical content once, plus reserved installation space. Budget private staging/quarantine and database overhead separately, including their volumes. Evict only unrooted content. If protected usage exceeds quota, refuse new preparation/capture before destructive work and preserve checkout edits.

Initially dematerialize only verified-clean regular files without conflicts or pending operations. Journal the withdrawal, retain exact remote targets, and never emit Tombstones. Selection withdrawal and byte eviction are distinct operations. L2’s monotonic-selection fsck rule must recognize an audited withdrawal; excluded explicit-absence observations must remain historical evidence without violating the current selection validator. [Selection validation:181](../../crates/layrs-core/src/projection/selection.rs:181), [fsck:71](../../crates/layrs-store-local/src/projection/maintenance.rs:71).

**Authorization boundary**

A cache hit proves bytes, never permission. Acquisition and publication retain current exact-path server checks even when content is shared.

Offline operations act on previously observed, prepared local drafts. They do not mint new grants, roll authority backward with historical versions, or authorize an unobserved destination. Known reserved/denied destinations remain blocked; preserved history remains recoverable. Make this local-draft distinction explicit when supplementing ADR0024’s stale-authority wording.

Add local client entry points for prepared read/compare/restore rather than silently removing authorization from existing `NativeRemote` methods. Preserve current remote methods for server-coordinated workflows.

**Locks, Windows and recovery**

Use one ordering everywhere: family lifecycle lease → Worktree lock → short shared-catalogue lock. Normal sessions share the lifecycle lease; reclamation/conversion use it exclusively. Never hold catalogue write transactions during network requests. Pass existing connection/session handles into nested helpers.

The current L2 fix now passes the caller’s connection into scanning; preserve that approach instead of reopening the database during a write transaction. [Selection:82](../../crates/layrs-store-local/src/projection/selection.rs:82).

Reuse pinned-directory, no-follow, identity and no-clobber operations. Materialization streams into new private files: no writable hardlinks. Existing `NativeCas` construction requires native Store handles, so extract only the necessary immutable publication primitives rather than manufacturing a native Store. [CAS:350](../../crates/layrs-store-local/src/scm_v1/cas.rs:350).

Windows sharing violations leave journals pending and bytes protected. Do not treat directory sync alone as proven power-loss durability: the helper tolerates unsupported directory flushes, while pinned rename uses `NtSetInformationFile`. Qualify the actual publication sequence. [Directory sync:7](../../crates/layrs-store-local/src/layout.rs:7), [Windows rename:196](../../crates/layrs-store-local/src/secure_fs/windows.rs:196).

Extend fsck to verify shared manifests/chunks, private-reference ownership, preparation dependency closure, locator identities, pending intents and quota accounting. Distinguish intentionally unprepared content from missing protected content; the latter is corruption.

**Conversion and implementation boundaries**

Conversion must be explicit and side-by-side. Freeze the source with its existing lock, require filesystem/publication journals resolved, copy and verify immutable history/content and checkout state into an initializing destination, then publish readiness last. Source mutation or failed verification leaves the destination unready and the source intact. Import available bases only after hashing; unavailable historical bases remain reported gaps.

Allocate a fresh destination Worktree identity. Preserve imported capture payloads/digests under a bounded import-provenance record rather than rewriting their identity chain. Newly accepted captures use the destination identity; imported captures require a new local capture before publication. Do not transplant pending publication requests. The existing complete-to-partial migration is a remote clone with source retention, not a format-3 history converter. [Migration:18](../../crates/layrs-client-core/src/native_scm/remote/projection_migrate.rs:18).

Implement in these bounded stages:

1. **`layrs-core/projection`:** exact version/availability contracts. **`store-local/projection/format.rs`, `database.rs`, new shared-content module:** format-4 opening, private schema 7, content retention protocol. Keep format-3 compatibility isolated; no automatic family conversion on open.
2. **`capture/storage.rs`, install/refresh/restore/rebase engines:** replace byte persistence through the content interface and establish retention-before-mutation. Keep their existing journal algorithms.
3. **Client projection commands and remote acquisition adapters:** preparation, offline version reads/comparison/restoration, separate incoming-base retention, then clean withdrawal/quota reclamation.
4. **Conversion and qualification:** disposable copies only; old-reader refusal, crash boundaries, corrupt shared content, disk-full, duplicate concurrent installs, Windows open handles and two Worktrees retaining divergent captures after restart.

The decisive failure tests are interruption between shared retention and private acceptance, editing a hydrated file before first capture, missing unchanged capture bases, eviction racing readers, and cache reuse after authorization changes. No actual user-data conversion is authorized by this memo.
