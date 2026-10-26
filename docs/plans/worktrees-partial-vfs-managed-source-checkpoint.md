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

## Remaining boundaries

The post-close injection is an acknowledgement refusal after successful close.
It is not a genuine sqlite3_close failure, quarantined connection, interrupted
commit or power-loss test. No physical destination initializer, copied-prefix admission,
content installation, claim, Ready receipt, family Ready publication, companion
conversion or selection expansion is qualified by these recipes. Product routes
and Desktop integration remain separate. The existing strict GC fence remains.
