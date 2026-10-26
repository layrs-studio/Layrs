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

## Remaining boundaries

The post-close injection is an acknowledgement refusal after successful close.
It is not a genuine sqlite3_close failure, quarantined connection, interrupted
commit or power-loss test. No content installation, claim, Ready receipt,
family Ready publication, companion
conversion or selection expansion is qualified by these recipes. Product routes
and Desktop integration remain separate. The existing strict GC fence remains.
