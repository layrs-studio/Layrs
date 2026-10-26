# Managed source checkpoint — 26 October 2026

This slice implements and qualifies the source side of a strict managed import.
It does not create or publish a managed destination.

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

## Remaining boundaries

The acknowledgement-refusal injections remain distinct from the separately
qualified reservation source BUSY/quarantine recipe. Neither establishes an
interrupted commit, restart after quarantine or power-loss guarantee.
No content installation, claim, Ready receipt,
family Ready publication, companion
conversion or selection expansion is qualified by these recipes. Product routes
and Desktop integration remain separate. The existing strict GC fence remains.
