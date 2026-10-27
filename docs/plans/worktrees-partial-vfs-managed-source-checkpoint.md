# Managed source checkpoint — 26 October 2026

**27 October continuation:** work resumed at the user's request. Exact content
installation/protection, physical checkout, private/family Ready and source
terminal publication are qualified below. Complete autonomous admission and
the later product stages remain open.
The [night handoff](worktrees-partial-vfs-night-handoff-2026-10-27.md) preserves
the earlier stop point; it is no longer the current working state.

The initial slice described next implemented the source side of a strict managed
import. Later chronological sections record destination work and its evidence.

`managed_image` retains the real ExportCheckpoint. It bounds path inventory
before cloning, rejects unknown/excluded/relocated/protected edits, and verifies
all bytes. Immutable files must match both each declared chunk and the whole
FileId; native replacements receive canonical manifests. Missing source bytes
remain an explicit failure. Revalidation hashes again and compares manifests,
deletions and namespace, rather than relying on file identity or length alone.

`managed_plan` derives observation G+1, placement G+2 and capture C+1, including
an empty capture when appropriate. The complete diff is relative to the fresh
observation. This first slice requires the fresh canonical observation and
selection to equal the original source; it rejects inherited companions.
Protected requirements retain source descriptors, all accepted capture
representations and current image manifests. Multiple representations of one
FileId are allowed only with equal length. Exact supplied manifest coverage and
checked metadata/content/staging budgets are required.

The import closure now retains the exact provider source/activation and scoped
content mappings. The planner accepts the complete Active proof and requires
the image to use the same source. This closes substitution of another manifest
representation with the same FileId and size. This is SQL DATA binding, not
proof of historical byte availability, remote permissions or family protection.

`managed_journal` records Planned and Cancelled under the retained allocation
attempt lock, exclusive owner/catalog gates, exact native boundary and typed
private/family reads. It hashes outside the exclusive gates, uses clean writer
admission, commits the exact SQL intent, observes close, and rereads durable
state. An uncertain result ends the attempt. Cancellation grants no deletion or
reservation-release permission. The read-only ExportCheckpoint root getter does
not convert export evidence into a capture capability.

## Validation

`provider-managed-journal-check-r3.log` passes `cargo check -p layrs-store-local
--all-targets`. The retained r1 binary passes **128 provider pure tests** with
301 ignored. Its source is unchanged in r2 except the corrected cold-file test.

`provider-managed-source-20261026-r2` retains 444 source/input files and EXE/PDB
CodeView age 45. EXE SHA256:
`fb76885cb7405f774cb77398c42402641b670ea16775cd780f3626e9115adda4`.
The independent source review and audited test-host startup/native inputs are
recorded with the artifact. Three exact recipes pass in fresh owned Windows
fixtures, serially, with 240-second owned child and 300-second outer bounds:

- A real writable replacement produces the exact manifest and stable digest;
  adding an unknown file invalidates the retained image without deleting bytes.
- An unchanged nonempty cold placeholder cannot produce an image without bytes.
- Planned commit followed by an observer refusing its close acknowledgement
  returns an error; a separate retry observes AlreadyPresent. Cancellation after
  namespace change preserves bytes, leaves the family database unchanged and
  permits Active reopen.

The exact SQL recipe `active_source_closure_is_exact_and_map_tamper_is_refused`
also passes on r2: the closure accepts its source proof and refuses a distinct
valid Active proof. This fixture has zero captures; substitution of a nonempty
historical mapping is reviewed statically, not directly exercised here.

The first r1 native queue stopped on a faulty cold test using an empty file;
success there was legitimate because no source bytes were needed. Its artifact,
fixture and failure log are retained. The test now requires a nonempty descriptor
and r2 uses a fresh parent. The first replacement recipe passed on both runs.

## Empty destination SQL ingress

`destination_sql::initialize_empty` creates the exact schema8 ordinary ingress
inside a caller-owned transaction. It requires a completely empty schema at
version zero and foreign keys enabled. The typed reader requires exactly seven
bootstrap rows (FORMAT, snapshot zero, head, selection, backend, import prefix,
Destination/Copying intent); every other table must be empty. It binds source
A/P/activation, prefix, family marker and destination DATA. String/blob bounds
are checked before reading payloads, including digest bytes after embedded NUL.
The caller remains responsible for actual distinct pins and the deterministic
destination member path, family state, COMMIT, rollback and observable close.

`require_exact_copied_rows` separately validates the twelve complete immutable
tables against a retained verified source closure. Cardinalities stop at the
first extra row, then the scoped semantic/hash reader validates each rowset.
This catches an out-of-scope tail as well as altered or missing rows. It does
not interpret the destination head, live state, ingress or Ready receipt.

`provider-destination-empty-check-r3.log` passes all targets of layrs-store-local.
`provider-destination-empty-20261026-r1` retains 448 source/input files,
CodeView age 46, EXE SHA256
`112257b42786f2258db75a522e22b986f20cef37725ec6c0a797978f906b6313`.
**128 pure tests pass, 304 ignored. Four exact owned SQLite recipes pass**:
fresh initialization/reinitialization refusal/rollback/reopen; marker, family,
backend and foreign-row refusal; hostile parent/mount TEXT and embedded-NUL
selection digest refusal; and exact copy/replay/collision/tail refusal.
Source review authorizes only these memory SQLite recipes, with 120-second
owned child / 180-second outer bounds. They create no real destination owner.

## Copied prefix and fresh physical allocation

`promote_or_replay_copied` now copies the verified immutable prefix and advances
head zero to the source generation in the same caller-owned transaction.
The exact CopiedPrefix reader also checks ingress, head and absence of live
destination state. A replay performs no writes. Dispatch uses the exact head;
reader errors, including uncertain reference-close errors, are never swallowed
as a reason to try another state. The source proof must belong to the supplied
connection with an active transaction; continuous transaction retention remains
the caller's responsibility.

Fresh allocation verifies the durable source Planned row and recomputes its
complete SQL closure before filesystem effects. The destination Worktree must
be absent from the family registry. It creates the root, private control/member
directories, lock, empty database and temporary FORMAT exclusively. Existing
entries are never adopted. Failures retain attempted/confirmed effects and
available pins; they perform no cleanup. Physical verification uses actual
parent/root/control/metadata/database/file identities.

The destination proof now requires `formatFileIdentity`, distinct from the DB
identity and included in the owner commitment. Expanded-provider DATA must also
agree with the FORMAT identity decoded from the native control record. Source
Planned without a destination is unchanged. The source Copying writer binds the
actual allocation before any FORMAT bytes or destination SQL are published.

`provider-managed-allocation-check-r2.log` passes all layrs-store-local targets.
The retained `provider-managed-allocation-20261026-r1` artifact has 450 inputs,
CodeView age 47 and EXE SHA256
`81d90479a038e81f87519f16457230cf54871fb5b963d626b200d7ed1f62def8`.
**130 pure tests pass, 307 ignored. Three exact recipes pass**: transactional
copy/promotion/replay with unchanged SQLite total_changes on replay and rollback;
fresh allocation followed by durable source Copying with FORMAT/DB still empty
and no family registration; and refusal of an occupied root with valuable bytes
and source Planned preserved. The SQL child uses 120 seconds; native children
240 seconds, outer wrapper 300 seconds, fresh C: parent, serial stop on failure.

Before a physical destination writer, the retained lock creator must be closed
and the destination owner lock acquired. Physical identity verification alone
does not establish empty content, registry state or SQL writer admission. A
crash before the source Copying receipt leaves unproven objects: retain and
refuse them; never infer ownership from the nonce. No restart recipe for that
window or for FORMAT publication has run yet.

## Pinned FORMAT publication

The retained allocation now closes its lock creator and acquires the destination
owner lock before publication. The durable source Copying proof and full source
closure are checked before effects. Only the recorded FORMAT inode is accepted,
at its temporary or final name. A temporary exact prefix of the canonical bytes
may be completed; any unrelated content is refused. Publication writes, syncs,
reads back and uses a no-replace rename. Replay requires that same final inode
and exact bytes. Errors retain the holder and effects; no cleanup is attempted.

`provider-managed-format-check-r1.log` passes all store-local targets. The
retained `provider-managed-format-20261026-r1` artifact has 451 source/input
files, CodeView age 48 and EXE SHA256
`6004a0648097ec7b4dc661fe51d6bf36c6d57180f5e63c3006217e094e71633a`.
**130 pure tests pass, 309 ignored. Two exact native recipes pass**: publication
with preserved inode and idempotent replay; and a foreign final FORMAT collision
preserving both original inodes and contents. They ran serially in a fresh C:
parent with 240-second owned child and 300-second outer bounds, after independent
source review. These tests cover same-process replay and preexisting collision,
not restart after crash, interrupted rename or power loss.

## Physical destination SQL and family Initializing

`managed_destination::register_and_copy` retains both physical owners and the
source attempt while registering the exact destination as Initializing. The
family writer must commit, close and pass a durable typed readback before any
destination SQL write. A retained source read transaction issues and keeps the
verified closure through destination initialization and immutable prefix copy.
The destination writer also commits and observably closes before the final
CopiedPrefix/family readbacks. Exact replay opens no writer. Foreign registry
rows, hot journals and all uncertain closes are refused without cleanup.

All store-local targets compile (`provider-managed-destination-check-r1.log`).
The r1 pure run passes 130 tests, 310 ignored. The retained native artifact
`provider-managed-destination-20261026-r2` has 453 source/input files, CodeView
age 50 and EXE SHA256
`77d459c5ecf9c2044e25abdbd146bbafc795e87a3131ef0f5e26d8a23d94e7b7`.
**Three exact native recipes pass**: actual family registration and physical
SQL copy plus no-write replay with source DB witness unchanged; refusal of an
acknowledgement after successful family COMMIT/close, leaving destination DB
zero bytes; and a foreign Initializing registration, refused before any writer.
The source-reviewed test file SHA256 is
`05c1f9535c61a45ec66b1ef4c0a3669e7ac94d44b0e24f4bd5481cd5f3e15711`.
Fresh C: parent, serial execution and owned 240/outer 300 second bounds apply.

Only fresh zero bytes, exact EmptyIngress or exact CopiedPrefix are admitted.
A nonzero SQLite header with version-zero empty schema after interrupted
initialization remains refused in this slice. These recipes do not qualify
restart recovery, hot-journal recovery or a genuine SQLite close failure.

## Exact byte transfer and SQL Staging

`VerifiedManagedImage::copy_exact_file` resolves only an exact path and manifest
from its retained current image. It transfers at most 64 KiB per read, checks
each chunk and the complete FileId, and revalidates the checkpoint namespace.
Native readers remain inside the checkpoint closure; immutable reads retain
the same SnapshotSource. A partial sink is possible on failure and must remain
unpublished. Historical bytes absent from the image are not supplied by this
API. Reader failures retain ExportRead path/backing provenance; checksum,
cancellation and output failures keep their original consumer cause.

`managed_plan::require_manifest_union` binds the exact supplied representations
to the planned ordered count/digest, rejecting duplicates, omissions and
substitutions while permitting distinct representations of one FileId.
This is metadata validation only, not proof of bytes or authorization.

Destination `stage_copied` now advances an exact CopiedPrefix Copying row to
Staging with its derived reservation identifier, inside caller-owned SQL.
It retains the source connection proof, compares all predecessor columns and
reads the complete result. Exact replay writes nothing. The original Empty
and Copied readers still require unreserved Copying; Staging has a separate
typed reader. No actual family reservation or content effect is performed.

All store-local targets compile (`provider-managed-staging-check-r1.log`).
The final retained `provider-managed-staging-20261026-r2` artifact has 456
source/input files, CodeView age 52 and EXE SHA256
`30bf8009bc858fb83e610469cb01117d974e54d364a10ddc45bcb8c4b17b4191`.
**134 pure tests pass, 314 ignored. Two exact recipes pass**: SQL Staging,
no-write replay, rollback, forged plan/extra-row refusal; and actual Native
80 KiB replacement copied under retained checkpoint, followed by namespace
change refusing a second copy before any output. Independent source reviews
released both recipes; SQL child 120 seconds, Native child 240 seconds,
outer 300 seconds, fresh owned C: parent and serial stop-on-failure execution.

## Physical Staging coordination

`managed_staging::stage` rehashes the retained image and binds the manifest union
before acquiring SQL gates. The first source write requires the exact copied
destination and family Initializing. Source Staging COMMIT, observable close and
readback precede the destination Staging write. A separate retry can complete
Source Staging/Destination Copying; fully Staging replay opens no writer. The
family reader for this phase requires Initializing with no actual reservation,
installation or root. A reservation identifier in private SQL is only intent.

All store-local targets compile (`provider-managed-staging-driver-check-r4.log`).
The retained r3 artifact has 460 source/input files, CodeView age 55 and EXE
SHA256 `4a956832fa59baeef51183bacd77d21528d384e1ed1f64f7ad8ba77b55e95c6a`.
The preceding r1 pure run passes 134 tests, 316 ignored. **Both final r3 native
recipes pass**: physical Staging and no-write replay; and an acknowledgement
refusal after successful source COMMIT/close, proving destination remains
Copying, followed by a distinct retry and no-write replay. Child 240 / outer
300 seconds, fresh owned C: parent, independent source release and serial
stop-on-failure rules apply. The source-reviewed test file SHA256 is
`e4146fe120494e321d52fe9de3e137085ca4d9ea192081b0597a0830d413a477`.

The first r1 native recipe correctly refused an old synthetic allocation plan
whose manifest union was empty. That failed fixture and log are retained; the
second recipe was not run. The corrected fixture derives its actual plan from
the complete SQL closure and retained image under scoped source/catalog gates.
No production validation was relaxed. These tests do not simulate a genuine
SQLite close failure, process crash or power loss.

## Family schema-2 SQL and exact representation readers

The isolated caller-transaction migration in `shared/schema_v2.rs` preserves
legacy rows and installation intents, removes FileId uniqueness from manifests,
adds a file/length consistency relation and introduces a separate exact
installation journal keyed by reservation/ManifestId. Its reference fingerprint
is constructed through the same DDL transformation. Schema, copied-key bounds,
foreign keys, cancellation and post-transform fingerprint are checked. COMMIT,
physical admission, lifecycle locking and observable writer close remain the
future caller's responsibility. Exact v2 replay performs no writes.

Existing readers now distinguish an exact private/trusted ManifestId from a
FileId-only candidate. Private mappings and capture protections resolve the
exact representation; a missing protected representation remains an error.
FileId-only selection is deterministic. Provider range reads prefer a present
trusted ID and refuse its corruption; a valid alternative still requires the
existing complete FileId hash before returning bytes.

All store-local targets compile (`provider-family-schema2-check-r1.log`). The
retained `provider-family-schema2-20261026-r1` artifact has 463 inputs, CodeView
age 56 and EXE SHA256
`7e27a091eff7733eb948e17ece6f6876bc8eab48ec84c98946e78c54f957ba0d`.
**134 provider pure tests and one exact retention pure test pass. Four owned
SQLite memory recipes pass**: populated migration, two representations and
no-write replay; rollback, foreign schema, cancellation and FK-valid 1 MiB key
refusal before DDL; exact shared reader/corruption refusal; and preferred trusted
representation/fallback metadata selection. Independent source reviews released
all four, child 120 / outer 180 seconds, serial stop on failure, fresh C: parent.

These four SQL recipes alone do not qualify physical family admission; the
subsequent integration is described below. No legacy GC fence was relaxed.

## Explicit physical upgrade and ordinary admission (2026-10-27)

`family_upgrade::upgrade_source_family` takes nonblocking exclusive lifecycle,
source-owner and catalog gates before SQLite. It retains the validated Active
source read transaction while the family passes clean admission, migrates in
one transaction, commits and closes observably. A fresh source/family readback
follows only positive closes. Exact v2 replay opens no writer. FORMAT, catalog
inode and source private data retain their identities. Sessions/checkpoints
must be dropped before migration. Bare Active holders do not permanently hold
the lifecycle gate, so dropping them remains a caller precondition.

Ordinary and provider catalog readers dispatch exact v1/v2 fingerprints.
Ordinary v2 access refuses any exact installation stage. Manifest writers
establish the v2 FileId/length relation before publishing filesystem objects;
reclamation removes that relation only after its last representation disappears.
Existing root foreign keys and provider reclamation fences remain in force.

Independent review found that the old ordinary RW opener could recover a hot
journal before examining exact installation stages. The corrected writer holds
the exclusive catalog gate through read-only schema/stage/marker admission,
positive read close and pin verification before RW. The ordinary hot-journal
retry is removed. This deliberately refuses hot family catalogs in both v1 and
v2: an unreadable catalog cannot establish which recovery contract applies.
Explicit unshared private recovery is unchanged. Shared recovery docs and the
old ordinary crash test now describe refusal rather than automatic rollback.

Independent source reviews release three SQL-memory recipes, three physical
upgrade recipes and one real interrupted-family-writer refusal recipe. The last
uses two fresh fixtures (v1 and v2 with a committed exact stage), kills/reaps only
its owned flushed writer, confirms `SQLITE_READONLY_ROLLBACK`, then checks that
ordinary usage/open/quota changes refuse with unchanged DB/journal/private/
FORMAT witnesses.

All store-local targets compile (`provider-family-upgrade-check-r3.log`;
PowerShell reports stderr warnings, while Cargo finishes successfully). Final
no-run build r2 exits zero. The retained `provider-family-upgrade-20261026-r2`
artifact has 470 source/input files, CodeView age 58 and EXE SHA256
`48362a93117e3a9c9038956cdb148ed8a3d81b3ee2c1834355bd058000eb57ac`.
**135 provider pure tests pass, 320 ignored; all seven released recipes pass.**
The native test file SHA256 is
`14208e6430e0dc1d9b26e8f339ef0814c0288b92c3baef5497ded4c97f50b22c`.
SQL children are bounded at 120 seconds, physical children at 240 seconds,
outer processes at 300 seconds, in a fresh C: parent with serial stop-on-failure.
The real interrupted writer test qualifies refusal with retained evidence,
not migration interrupted during DDL or automatic recovery. The unchanged
shared `recover_databases` entry is source-reviewed but not exercised by that
native recipe. The extra managed-reservation code retained in this artifact's
source snapshot is not qualified by these seven recipes.

## Retained managed reservation (2026-10-27)

`ManagedImportLease` borrows the source publication/root/attempt, destination
allocation, fresh observation and mutable image checkpoint. Its closed API
opens and reserves only the exact Staging plan. Image rehash occurs before
owner/catalog gates. Each short SQL window retains the source read transaction,
reconstructs the complete plan from source descriptors, capture history and
current image, and verifies destination Staging plus family Initializing. The
destination owner is already held by allocation and is never reacquired.

The metadata budget is a bounded envelope: subtract the fully reconstructed
zero-metadata charge from the durable quota, require at least the logical
twelve-rowset and new-record byte floor, cap it at 1 GiB, then compare the whole
rederived plan. It is not a physical SQLite allocation measurement. Manifest
count, file/chunk sizes and reader limits precede serialization; the complete
inline protection payload must fit its current 16 MiB envelope. Unsupported
representations refuse before reservation rather than silently reducing the
union. Physical capacity is checked separately at family and destination.

The family predicate preserves source Ready and exact destination Initializing,
requires matching private Staging intents and a complete canonical union, and
allows only the exact derived reservation with its full original remaining
charge. Roots, legacy destination installations and all exact installation rows
are still refused by this reserve-only phase. The lower reader accepts FK OFF;
the writer requires FK ON and a caller transaction. SQL quota insertion and
readback are atomic; no automatic budget growth or content side effect occurs.

The retained family-upgrade r2 binary also passes both independently released
reserve-only SQL memory recipes (child 120 / outer 180 seconds). They establish
exact insertion, no-write replay, rollback, quota and foreign-effect refusals;
they do not establish the physical lease. Their source SHA256 is
`3cfb0ab488906ff396326724bb6af4e2e1e4fb16466189c81867bdced191fc07`.
The integrated physical lease compiles for all store-local targets
(`provider-managed-reservation-check-r2.log`); final no-run build r3 exits zero.
The retained `provider-managed-reservation-20261027-r2` artifact (471 inputs,
CodeView age 60, EXE SHA256
`37e8f55eae5d3acaf0ab722e59e118dbcc1e4c0e0f144f1f7132b02ff1cf6851`)
passes **135 provider pure tests, 324 ignored**, and the two native recipes for
exact reservation/no-write replay and acknowledgement refusal/retry. Its third
recipe failed because its post-allocation forged plan was correctly rejected
by the allocation holder before reaching the intended complete-union check.
That fixture remains untouched; execution stopped before the fourth recipe.

The corrected negative fixture commits its incomplete but self-consistent plan
before SourcePlanned and allocation, and performs no later journal corruption.
The final retained r3 artifact (471 inputs, CodeView age 61, EXE SHA256
`5b4070957aeb4cf830c8994855b87cf07743cf0271e81886a78d418c5505e1d0`)
passes **both remaining native recipes**: true complete-union/metadata-envelope
refusals before reservation, and real source `sqlite3_close` BUSY after successful
family COMMIT/close. The latter verifies the exact causal error and filesystem
witnesses, catches assertion panics while holders remain borrowed outside the
closure, and exits its owned child with success/failure before any finalizer or
provider/SQLite re-entry. It qualifies stopping after uncertain source close,
not subsequent restart or recovery. Final native source SHA256 is
`1707f56e08cd8795ec7f6fe82c9ece85edc1551704f66d386ac654744923de9c`.
All native executions use independently released sources, child 240 / outer
300 seconds, fresh C: parents and serial stop-on-first-failure operation.

## Exact current-image chunk producer (2026-10-27)

`VerifiedManagedImage::copy_exact_chunk` binds the complete current-image
manifest and path before selecting an ordinal. It transfers at most 64 KiB
per read, checks cancellation and the selected ChunkId, and deliberately does
not assert a full FileId. `ExportCheckpoint::with_range_reader` reads a checked
range from the retained native handle with surrounding file/namespace proofs.
Range EOF is synthetic; the original full reader still requires physical EOF.
Reader and consumer failure provenance remain distinct.

All store-local targets compile, and both final store-local and VFS no-run
builds succeed. The retained `provider-managed-chunk-20261027-r1` store artifact
(475 source/input files, CodeView age 63, EXE SHA256
`d30d385e17775d7f78551a6147329c4309c0472ae7ed0a16b1d7d479e633f8b2`)
passes **137 provider pure tests and one released native recipe**. The separately
pinned VFS artifact (CodeView age 3, EXE SHA256
`fd08c3652172192dc8d9a02b12a5dec19474b84367fa00af26d19a95cea73c34`)
passes both exact memory range-reader recipes. Native test source SHA256 is
`2027763cf4875cb485f8fc90ae94a622c63952fcafcd80a36d2d32b23c860d25`.
The native fixture verifies the second chunk of a 1 MiB + 512 KiB + 17 byte
ordinary replacement, range EOF, invalid range/ordinal/representation refusal,
and namespace drift with zero output. It does not qualify nonzero CFAPI range
hydration of a partial/relocated placeholder or interruption of synchronous
hydration. Child 240 / outer 300 seconds, fresh C: parent, serial execution.
Unmodulated exact-installer drafts retained in the source snapshot are outside
this qualification.

## Resumed exact installer: SQL and pure qualification (2026-10-27)

Work resumed explicitly after the night checkpoint `62b9971`. The exact engine
and retained managed adapter compile. DestinationReady is now part of the module
tree; its first integration exposed two private enum SQL accessors, corrected
using the exact already-validated destination/Staging literals. All store-local
targets and the test binary build pass (`provider-ready-integration-check-r2.log`
and `provider-exact-install-build-r1.log`). DestinationReady is not yet qualified.

Independent source review releases two pure recipes and two owned SQL-memory
recipes. All four pass on the retained `provider-exact-install-20261027-r1`
artifact (486 source/input files, CodeView age 64, EXE SHA256
`ebd8bdbf3dc90998c6e72c098026219b98a894619b140bc3bd2e45d2f4dce828`).
The codec binds exact representations and state; the bounded sink refuses tail,
short writes and cancellation without retrying Interrupted indefinitely. SQL
recipes establish journal transitions, duplicate-chunk charging, both manifest
charges, ordinary/exact usage agreement, conservation of fixed quota, no-write
replay, rollback, insufficient charge and wrong union/remaining refusals.
The test named foreign_rows does not insert a competing foreign journal row;
its other-reservation assertion is only an absent lookup.

Reviewed test SHA256 values:
`405fe303366b5813a578ab5d3ba18ab5dfb8f8dd8997e9c29b9da99999c81034`
(`tests.rs`) and
`07d806416fed5fc04f019f2ab658326beceb1cd3cd7fb7d0577fbbee78db4f8b`
(`spool_tests.rs`). SQL children are bounded at 120 seconds, outer processes at
180 seconds, under a fresh C: parent, serial stop on first failure. These tests
do not prove physical inode ownership, filesystem publication, enclosing-close
faults, crash recovery or DestinationReady. Native installation fixtures are
being integrated and require their own review and execution.

## Physical exact installation and private Ready SQL (2026-10-27)

The retained `provider-exact-install-20261027-r2` artifact passes two independently
released physical recipes and all four independently released Ready SQL recipes.
All-target check r3 and no-run build r2 pass. EXE SHA256:
`48ab751a20396b05829e33920731237921dcc458d1dd16a84e01213a6c3df9e0`,
CodeView age 65, 488 source/input files. Runs are serialized with fresh C:
parents, child 240 / outer 300 seconds for physical recipes, 120 / 180 for SQL.

Physical tests install a retained ordinary replacement of 2 MiB + 17 bytes:
three ordinals, two distinct chunks, the repeated chunk debited once, fixed
journal charge and both manifest charges accounted, full CAS reconstruction
equal to the source. The destination remains Initializing without protection.
The second recipe drops and reopens the ManagedImportLease with the same
process, checkpoint and physical holders. Published replay opens no writer and
preserves database and CAS/staging file identity/length/hash witnesses. Directory
names are compared, not directory identities. This is not crash-restart evidence.
Test SHA256: `dc864c7dc7313fc9dbb2d95781ef83915291b8d98d1b50e13a54f1166d070686`;
helper SHA256: `591106d9a17a63c25c63ceecd86face663f970fd7fb7114173bc2943c3989a2c`.

Ready SQL appends exactly observation G+1, placement G+2, capture C+1, durable
provider-import claim and transition receipt. Its complete reader validates
the immutable prefix and exact suffix separately, preserving the historical
FileId mapping when the current image uses another ManifestId for the same
bytes. Replay runs query-only with FK OFF and zero changes; mutation requires
FK ON. Rollback, forged inputs, prefix/suffix/mapping/claim/tail corruption are
refused. Independent review found and corrected missing sweep exclusions and
unbounded relationship IDs before String allocation; tests include both fixes
and acceptance of unrelated sweep rows. Family state remains Initializing in
this private Ready phase. Sources: tests
`58299255cb3f39820a952a8d5b740cb4395e6b4cba87a164f78af7215949cbe8`, fixture
`f4cea9435b33a2af920ee739720a45cdb2772a155afbc080f96c0e43b4cde1cc`, source fixture
`75f937eeff11ee8fa75d94a08284c15c83b59599ad1ad491cd086efae38959fe`, family reader
`ca36adaaf31ab74b2ec34c62842cf20f13a5e93a989f990ad368688ac2afb721`.

These SQL recipes use three memory databases. They do not prove a physically
protected checkout, durable Ready publication, close-fault recovery or ordinary
admission. Active-piece recovery, real producer cancellation and installation
of two historical representations still require separate runtime evidence.

## Exact family protection and immutable CAS retention (2026-10-27)

The managed lease now derives the canonical ordinary protection root from the
complete exact manifest union. Publication requires every exact journal to be
Published, canonical indexes, no overlapping sweeps, and the rederived remaining
metadata envelope. It changes neither reservation remaining bytes nor journal
accounting. The destination remains Initializing and both private intents remain
Staging. Exact replay reads without opening a writer. Ready SQL now uses the same
canonical root constructor.

Before the family writer, the adapter checks all manifest bytes, chunk hashes,
full FileIds, lengths and EOF outside catalog gates. It retains one immutable
Windows handle per unique CAS object through COMMIT, all enclosing closes and
fresh SQL readback, then returns those handles in the protection proof. Independent
review found and fixed an initial gap where handles were dropped after hashing.
The final handles share READ only; ordinary concurrent write/delete opens are
refused. This does not claim protection from preexisting writable memory maps,
raw volume access or privileged writes. Resource exhaustion refuses before the
writer. No Ready or cleanup capability follows from this proof alone.

All-target checks r2/r3 and no-run build r2 pass. The retained artifact
`provider-exact-protection-20261027-r2` (494 source/input files, CodeView age 67,
EXE SHA256 `6451155f6cd1dd406e9251b512d356d99b82181174996f571b0fff68f374b1f2`)
passes eight kernel/regression recipes and two integrated native recipes.
The first group contains one pure codec test, two protection SQL tests, one
isolated CAS filesystem test and all four Ready SQL regressions. The CAS test
proves live write/remove sharing violations (Windows 32), rename refusal,
existing-writer refusal and hash refusal after deliberately corrupting a released
file. Four retained handles cover two manifests and two unique chunks.

The two native recipes install two different representations of the same FileId,
charge their shared chunk once, publish exact root membership, and reopen the
lease under retained same-process owners with zero replay writers. Database and
CAS witnesses and remaining quota are unchanged. The second injects an observer
acknowledgement refusal after a successful physical family close; retry reads the
committed root. This is not genuine BUSY, cancellation or crash restart. Their
source SHA256 is `6c867ba973f07e0d8e2d4f39572dc5f70bd75a2b2d49464a6a2365c6f2c41644`.
Kernel SQL and CAS children are bounded at 120/180 seconds, native children at
240/300 seconds, pure test at 60 seconds, with fresh C: parents and serial
stop-on-first-failure operation. Independent source release precedes execution.

Review also found legacy read-only source-capture opens requesting DELETE,
which conflicted with the retained immutable handles. Seven read/recheck sites
now use the read-only opener. All-target check r4 and no-run build r3 pass.
The retained r3 artifact (498 inputs, CodeView age 68, EXE SHA256
`1fdeaae4a603182b29776d4367ade2b1b9132fb2f69630e74b29000c344e5d7c`) passes
the independently released real Accepted-capture regression (child 240 / outer
300, fresh C: parent). The fixture has nonempty accepted content, demonstrates
the old DELETE opener's actual error 32, and validates the historical catalog in
a retained read transaction under live immutable manifest/chunk pins with
positive close and unchanged witnesses. Source SHA256:
`4200182700fc0de833f3ff30c2afb7a812d63ce4df7e1e63aafdce146d18ac87`.
It covers accepted catalog reads, not the entire managed import with accepted
history or the unfinished source-installation fallback paths. The unmodulated
checkout-journal drafts in the source snapshot are outside this qualification.

## Checkout journal and retained provenance (2026-10-27)

`checkout_journal` uses the existing schema-8 `projection_prejournal` table.
The exact header binds plan, prefix, source image, destination proof and native
identities. Complete initial declarations precede filesystem allocation.
Directories transition Declared -> Owned; files transition Declared -> Owned
-> Verified -> Publishing -> Published. Every row is canonical and bounded;
the reader checks the complete inventory, unique identities, reserved DB/FORMAT
identities and publication parents. Mutation requires FK ON and the caller's
transaction; exact replay is query-only compatible and performs no writes.
Each CAS currently rereads the complete journal: total work is quadratic in
row count, despite the explicit 100k leaf/directory and 64MiB logical bounds.
This is not a large-checkout performance qualification.

Independent review found an asymmetric directory/file identity-alias check.
Both header and relationship readers now normalize and reserve DB/FORMAT
identities in the directory namespace. The retained
`provider-checkout-journal-20261027-r2` artifact (505 inputs, CodeView age 70,
EXE SHA256 `af944ae6ee966a468c3c4950f43d5ea3e3c68e94d9edd75258f49c7b8e5b50f3`)
passes one pure and three memory SQL recipes. They cover canonical inventory,
whole-row CAS, parent/stage/identity refusals without writes, rollback,
complete progress, query-only replay and bounded corruptions. They do not
qualify complete private Staging admission, the physical adapter, close faults
or its runtime budget checks. Source test SHA256:
`09d276345182ed8ca118b8e221de3124e0964d293289ff456403ca9f6cde86c2`.

The integrated adapter preserves the strict Staging reader and introduces a
separate typed checkout route. Journal progress is acknowledged only after
destination COMMIT, positive destination/source closes and fresh readback.
Only an exact Owned temporary can restart from byte zero. A distinct read-only
`reopen_protection` reconstructs immutable CAS pins when a journal already
exists; the initial reservation/installation/protection writers stay strict.
Checkout bytes and maximum future journal size remain in the fixed reservation.

Physical copying uses exclusive creates, single-link checks, full FileId
verification, no-replace rename and a bounded exhaustive namespace observation.
Final files are retained with immutable READ-only sharing through the returned
proof. Directory listing retains the existing before/after pin checks, not an
atomic hostile namespace snapshot. Existing Windows directory-sync tolerances
do not establish power-loss durability. Executable files are refused before
checkout effects because this Windows backend cannot preserve that mode.

The initial r3 native recipe failed before destination allocation: a test helper
reconstructed a fixed one-file source and rejected the new nested source's
catalogue digest. Its fixture is preserved; the second recipe was not run.
The helper now uses the source returned by the actual private validator. The
nested creation has an explicit Absent+writable source entry before bootstrap,
and native bytes are written before the image/plan are derived.

All-target check r4 and no-run build r3 pass. The retained r4 artifact (505
inputs, CodeView age 72, EXE SHA256
`a7304f2e9538b86453453303c124177a3fa6d95b2e797e566b88f65b1b842df0`)
passes both independently released native recipes (child 240 / outer 300,
fresh C: parent, serial execution). They copy README and an explicitly
authorized nested file, retain immutable final handles, reopen under the same
process/checkpoint/allocation owners, replay without writers and refuse an
unexpected file without modifying that file or the database. Source and family
witnesses remain unchanged throughout checkout. The second recipe withholds
an acknowledgement after the fourth positive writer close: the exact temporary
inode is durably Owned but still zero bytes. A new lease and read-only protection
reconstruction finish the checkout. The destination stays Staging without a
claim; these tests do not establish Ready publication, crash restart, real BUSY,
partial-written temporary or rename-gap recovery, link/reparse attacks or
power-loss behavior. Test SHA256:
`e57a9c87a03acb9eb0cccfbe2e31b7d8ca8835471193ab5554ecdcd14a4d10bf`;
FS helper `9d5fe4517244cd32ca2119b0c182b3f56c91983a51f968c30e54c1f692232b39`;
staging fixture `a69795a6ec5c9442e250b3201cc193b7d87d9d29f6d3597a6075abea56ebb3f8`;
source-plan helper `cd4cd1e675e190a29a06d85702127fb98e46e627d200c88abc80160faa6b8296`.

Ready must retain exact `path -> ManifestId` provenance: the historical
FileId-primary-key mapping cannot reconstruct every current representation.
The new typed Ready SQL route therefore retains the complete Published checkout
journal byte-for-byte, while the old Ready reader still requires an empty
journal. There is no journal deletion, quota release or ordinary admission in
this tranche. Transfer to permanent import provenance requires a separate
protocol before generic cleanup or ordinary mutation paths can be admitted.

The same retained r4 artifact passes four independently released retained-
checkout Ready SQL recipes and all four legacy Ready SQL regressions (child
120 / outer 180, another fresh C: parent, serial stop on first failure).
The new recipes check byte-exact journal retention, query-only Ready replay
without writes, rollback to Staging with the complete journal restored, and
incomplete/foreign/aliased/unrelated inventory refusals. The rollback is after
a successful finalizer; no mid-write failure is injected. Their three memory
databases do not qualify the physical Ready adapter or enclosing-close faults.
Test SHA256 `54232a7b15d9fda7482fd22d7fdb9169073cfd27471f878b6682d751019b9cd2`;
finalizer `e918a366749d511357ba413f3d9f76a198bc5a711931349740af295b2b96678c`;
reader `732dbb161577c7579a666a2c2e1211a263ad0068f9fc218dd0683b47447ef1e4`.

## Private Ready adapter (2026-10-27)

The private Ready adapter now derives owned suffix DATA from the actual source
transaction and retained image, and borrows both exact CAS and physical checkout
proofs. It compares the complete SQL checkout journal with the retained physical
proof before admission, under the writer, after DML and on fresh readback.
Canonical but substituted inode DATA cannot replace the retained ownership.
Schema fingerprints precede phase-dispatch reads in the touched adapters.

The destination append is acknowledged only after destination COMMIT and positive
destination, family and source closes, followed by fresh complete validation.
Already-Ready replay uses no writer. Source Staging, family Initializing, quota,
exact installation journals and complete checkout provenance are retained.
The returned proof borrows its physical holders and exposes only receipt DATA;
it does not grant ordinary admission or family publication.

Independent review closed an omitted selection-budget charge: rederivation now
counts both actual selection payloads and scalar columns. The Ready floor adds
the complete receipt, claim, Ready intent and unique FileId content mappings.
Checked sums enforce the 1GiB metadata bound; protection and checkout-journal
envelopes are checked separately. These are normalized logical column bytes,
not a claim about SQLite page, index or rollback-journal geometry.

All-target check r2 and no-run build r1 pass. The retained
`provider-private-ready-20261027-r1` artifact (509 inputs, CodeView age 73,
EXE SHA256 `cb9fd4f845547f0a77bd7e4be798d25789ab94856af492dc73b6aaf34c025280`)
passes four independently released pure DATA recipes: exact overlay path and
representation, deletion preimages/uniqueness, 4096 selected absent paths with
no content mapping, and unique FileId accounting with cap/overflow refusals.
They do not exercise the complete constructor or SQL finalizer. Source SHA256:
`cd30214c1f7cfd06fd98b7d1674587d0ab9b2e52ef502c073338ecdcdc509e58`;
managed plan `b19bc518fa8595240685017f1a2104a12b46b576b3d8a2bc665b7efeb494ef24`.
Both separately released native Ready recipes also pass on this same artifact,
serially with a fresh C: parent and child 240 / outer 300 second bounds. The
normal recipe finalizes Ready then replays without a writer. The second refuses
the acknowledgement after a physically successful committed writer close;
the retry reads the durable Ready and returns its exact receipt with zero writer
opens/closes. Source/family and output-file witnesses remain unchanged, and
all five complete checkout journal rows remain byte-identical. Canonical inode
substitution is tested before Ready in the first recipe and after Ready in the
second: the SQL DATA reader accepts it, but the physical adapter refuses before
any writer. Only the owned fixture then restores the original columns by exact
CAS. This is not a product repair, real close failure, BUSY or crash restart.
Test SHA256 `78738d0e277119b014c52aa1f1b8570c0deaaca6a5bd0bfa16d35979a3f241b2`.

## Family and source publication SQL (2026-10-27)

Dedicated publication readers preserve the old Initializing/Staging APIs.
The family kernel validates complete retained accounting and changes only the
exact destination worktree row from Initializing to Ready. The destination
has a separate full Ready reader for that family phase. Source publication has
a distinct terminal reader with bounded transition/receipt preflights before
FK checks, a bijection between outgoing receipts and terminal intentions, and
full-row CAS for Staging -> Ready -> Published. Ready remains pending;
Published is historical. None of these kernels commits or grants admission.

All-target check r2 and no-run build r1 pass. Retained artifact
`provider-family-publication-20261027-r1` (516 inputs, CodeView age 74,
EXE SHA256 `a77a195c471b8cb51a1beeac4cb0abbb760c80ae9459ca1da94f9b8add9cd02d`)
passes six separately reviewed SQL-memory recipes, serially with fresh C:
parents and child 120 / outer 180 second bounds. Two family tests cover the
lower CAS/fingerprint helpers, not full FamilyAccess pins; a third checks the
complete distinct destination Ready reader and strict legacy-reader refusal.
Three source recipes cover both phase changes, rollback, query-only replay,
forged/orphan/competing receipt refusals, FK OFF and 4097-row preflight refusal.
An Accepted-capture fixture is added after Published and interpreted before
historical replay; it does not execute the physical capture API.

Test SHA256: family `65292dc236cb20b11a02edf38e00e66d81d0e06aef755744af27864b1b2970eb`,
destination `27e9ebe92a4f038b0003650cbe8a47f6f73a393eb220d8e11841698acee29a86`,
source `5994f5e14fcc2b438960ecba335b6cae5a999a6c9fd3d7f139cf7a8bff2da85d`.
The integrated family driver retains source and destination read transactions
through the family COMMIT, then closes positively and rereads. All-target check
r4 and no-run build r2 pass. The r2 artifact (519 inputs, CodeView age 75,
EXE SHA256 `76d2122837e441fcf0d8b64d4b0c148f35735d76f598542f71c5018664be6979`)
passes its separately released native recipe, child 240 / outer 300 with a
fresh C: parent. A single README fixture has two representations of one FileId.
The writer commits family Ready, closes physically, then its observer refuses
the acknowledgement. All thirteen family tables are compared: only the exact
destination worktree state changes. Source Staging, private Ready, receipt,
three checkout rows, output bytes, reservations and protection remain intact.
The retry validates freshly with zero writer opens/closes and unchanged witnesses.
Test SHA256 `77aacc781327d1f3c4baa195104e17fdedb47b9b1919ebe74c736dedfebd7445`.
This does not establish actual close failure, BUSY, process restart or admission.

The same r2 artifact passes a separately reviewed publication-aware import
closure SQL recipe (child 120 / outer 180, another fresh C: parent). Capture
and Publication proofs are mode-separated; the publication wrapper rereads the
full current SQL proof before import/FK interpretation, binds its intentions
and receipts and keeps the original strict current capture head. An added
Planned intent invalidates the old proof without changing incoming ancestry.
That recipe does not yet exercise a Ready receipt or changed capture head.
Test SHA256 `67a97344d439e3acbb28922ac4e672cff67fd12343ab1452a29ecec5261badfb`.
The strengthened source-publication recipe then passes on r3 (520 inputs,
CodeView age 76, EXE SHA256
`47ce70223c0142014c5abb3c9b3b76b17188ef6eb2b3252460953d65bbe1d173`),
after all-target source check r2 and no-run build r1. It now reads publication
closures at real Staging, Ready-with-receipt and Published, verifies the same
incoming closure while rejecting prior full-proof bindings, then adds the
Accepted capture and verifies rejection of both the old proof and the old
prefix under a fresh proof without writes. It uses the same separately reviewed
120/180-second SQL-memory recipe bounds and a fresh C: parent. Updated test
SHA256 `01d4eadba391c53d6183490981fa370451ae013c023616734db854c8901c9b33`.
The first physical source-publication recipe failed on r4 (523 inputs,
CodeView age 77, EXE SHA256
`7042773ed655c53070dbcd1381bb0f3dbb1697274bcddd88e777da752fa974be`),
before calling the source publisher. Its generic no-follow file witness tried
to traverse the live ProjFS checkout root to read source `.layrs/FORMAT`, and
correctly refused the reparse point. The failed C: fixture and log remain
preserved. The fixture now leaves this path out of generic witnesses; source
FORMAT remains checked by the existing native-owner boundary and retained pins.
Product filesystem checks were not weakened.

The corrected recipe passes on r5 (525 inputs, CodeView age 78, EXE SHA256
`65599817d34c65c3f8584b33ba1c9e52cfaf8f187eca833788907d3443f161ee`),
after all-target check r7 and no-run build r3. It runs in a fresh C: parent,
child 240 / outer 300 seconds, finishing in 138.66 seconds. Source Ready commits
and closes before an injected ACK refusal; retry writes Published exactly once,
then Published replay opens no writer. Source tables other than transitions and
receipts, all schemas, family and destination tables, checkout journals, quota
and output witnesses remain exact. Both outgoing receipt and normalized source
projections are checked after each phase. This remains retained-owner,
same-process/checkpoint qualification, not crash restart or actual close failure.
Test SHA256 `829caad150840641bd8d83f1ad84899e44cb965e732fb8c9ff3d89744056e3ce`.

The same recipe exercises LocalSource validation and real 21-byte README
callbacks after Ready/Published, including cancellation with untouched output.
Explicit live readers retain the full publication proof; export also compares
transitions and receipts. The old protocol readers and capture mutation still
refuse terminal states in this tranche. Review found four read-only CAS opens
requesting DELETE, incompatible with retained immutable READ-sharing pins;
they now use open_readonly_file while retaining identity/hash/length checks.
Helper SHA256 `5dedd2c1828610422791ba50ddf884f0df967f23208f25a8160f9bca9a94a4db`;
provider content `34ca331ef9e29251b6409666d0d0b9c674725f5e1ba47463dd90534cf179d487`.
No native reconnect or export recipe was run for these explicit live routes.

## Autonomous historical prefix (2026-10-27, qualification in progress)

The new prefix reader uses the actual destination FORMAT/SQL and a distinct
transaction-bound DATA type. Source and family connections are closed in its
recipes. It reconstructs copied source catalog/activation and validates incoming
history/scopes, without fabricating source FORMAT, A/P records or a live source
closure. It explicitly does not validate the local Ready suffix or admission.

Critical review found SQL TEXT digests with NUL tails could reach String
allocation before byte-length checks. A dedicated twelve-table preflight now
bounds all historical TEXT/BLOB fields in bytes before semantic loaders, with
bounded row scans and a 2GiB aggregate ceiling. Seventeen small NUL-tail probes
are source-released but not yet executed successfully.

On r5 the first autonomous prefix recipe passes. The second failed in fixture
setup while assigning a nonexistent receipt_id: immediate FK enforcement
rejected the deliberate corruption before the reader ran. The queue stopped;
the third recipe was not executed. The correction uses deferred FKs only inside
that owned savepoint, rolls the corruption back, then restores immediate FK
checking. A fresh artifact and parent are required; the failed log is retained.

## Post-publication capture and autonomous Ready continuation (2026-10-27)

All-target check `provider-post-publication-check-r2.log` and no-run libtest
build `provider-post-publication-build-r1.log` pass. The r6 artifact has 531
source inputs, CodeView age 79, EXE SHA256
`d7beea710c8145c93b9395aa256e3585687b99cfe273c4b27e78e4082032e478`.
Its source closure and startup/native-input audit are retained under
`.layrs-local/validation/provider-family-publication-20261027-r6`.

The strengthened source-publication SQL recipe passes on r6, child120/outer180
with a fresh C: parent. It additionally verifies that the opaque mutation gate
refuses pending Ready, admits historical Published, preserves outgoing
publications after a subsequent Accepted capture fixture, and rejects stale
full state. Test SHA256
`12cb71decf92ef82c2785f787e06804db4d5745b9458eba26a6063d87adad879`.
This SQL fixture alone is not a real capture API execution.

All three historical-prefix SQL recipes pass on r6, including the corrected
receipt-substitution fixture and all seventeen NUL-tail bounds probes. Tests
SHA256 `948c156fa17c5ed8117190297ed3f9b0b05565b431f052f4cbe9afc72b0a1427`.
The r5 failure remains preserved; it is not overwritten by this fresh run.

The complete autonomous Ready DATA reader is integrated and independently
reviewed. It uses actual destination and family transactions, reconstructs the
local suffix and exact manifest union, and validates retained checkout and
installation journals, protection, reservations and budgets without consulting
the current source member. Its first r6 recipe fails BEFORE reader execution:
the synthetic journal setup assumes every manifest has one chunk, but the
fixture includes an empty manifest. The second recipe was not run. Correcting
that fixture and rerunning on a fresh artifact remain pending. Neither code
review nor compilation qualifies the complete reader at runtime.

The real post-Published capture helper is integrated through an owned-result
fixture hook: all transfer proofs and the original attempt lock are dropped
before the mutable session callback, then the ordinary final disconnect runs.
The source-reviewed native recipe passes on frozen r6 in 221.31 seconds
(child240/outer300, fresh C: parent). After Ready/Published ACK/replay and live
read assertions, it makes a real 51-byte authorized README edit and calls
`session.capture`. The result reaches Accepted with a new patch and capture
receipt, retains its claim, releases its own reservation, and reads the exact
resident manifest bytes after positive catalog close. Historical source
Published intent and outgoing receipt remain byte-exact. Source revalidation
and final state/publication comparisons pass before disconnect.
Recipe SHA256 `f280888e4b2c0b031b9ab225959ff9fcb28794c4e3384a654faf147973de1eb4`;
capture helper `0f3375a50ba385f756010b17fccd4b617b4fa01593f967144e912c245e7d9874`;
fixture `3b0d6380da5ba372235889f4c574cf9c7da0b0e86cc23f5e4ccb4a235852a63b`.
This qualifies one real post-Published capture in the same connected source
session, not native reconnect, multiple successive imports or ordinary
destination admission.

The corrected complete autonomous Ready recipes both pass on r7, after
all-target check r3 and no-run build r2. Artifact EXE SHA256
`d4bce7b243233b0fd610aa3bf29415321663847725fc9bd4f93c3c82294b94ee`,
CodeView age 80, 531 source inputs; tests SHA256
`1d993376ddee3d0238f9537d7ac3b77179aa3777cd9126ba4ec3c9ed81af798d`.
The independently reviewed correction derives journal cursors and resident
bits from each actual manifest's chunk count, including zero; assertions and
production readers are unchanged. Both recipes use child120/outer180 and a
fresh C: parent. Positive coverage closes the source, removes only its fixture
family membership, reads query-only destination/family DATA and rejects a
proof after its transaction ends. Negative coverage rejects changed suffix
head, journal representation, family phase and reservation debit. Identities
remain synthetic SQL fixture values; no physical reopen or ordinary admission
is claimed. The existing exact-installation reader also still refuses other
reservation journals in the family, so multi-import admission remains open.

## Remaining qualification boundaries

The acknowledgement-refusal injections remain distinct from the separately
qualified reservation source BUSY/quarantine recipe. Neither establishes an
interrupted commit, restart after quarantine or power-loss guarantee.
The two physical recipes above qualify their exact installation scope only.
Family protection, physical checkout and private Ready have the bounded evidence
above, with one real capture after Published now qualified on r6. Complete
autonomous admission, companion conversion and selection expansion are not yet
qualified. Product routes and Desktop
integration remain separate. The existing strict GC fence remains.
