# Provider capture checkpoint — 26 October 2026

The Active session now exposes `capture(transaction_id, cancellation)` and
`cancel_capture(transaction_id)`. This is a local Store/VFS capability, not yet
CLI/Desktop integration or completion of the Worktrees programme.

## Implemented boundary

- A complete retained checkpoint is reconciled against the fixed selected
  source and exact activation map. Unknown paths, protected changes and omitted
  source entries refuse capture. Selected writable absences may become files;
  ordinary replacements are fully hashed before comparison with the base.
- A nonblocking Windows byte-range lock on the authenticated allocation receipt
  serializes attempts across processes. It creates no new persistent artifact
  and leaves callback owner/catalog gates free while checkpoint readers run.
- Private Hashing and Staging records bind the head, owner, mount, image and
  reservation. Shared installation reuses the existing durable content engine.
  Private content mappings follow family installation; family protection follows
  those mappings; Accepted, patch, private claim and receipt commit atomically.
- Provider content scopes require clean restricted admission and observable
  close. They cannot enter ordinary automatic journal recovery. Their narrow
  methods authorize owner/reservation/files/protection before effects and reread
  private evidence under the owner lease. Typed primary/close causes survive.
- Reusing an Accepted transaction returns its durable acknowledgement. Reusing
  a Staging transaction requires the same whole image. A different transaction
  may deduplicate the last identical patch. Cancellation retains staged content,
  reservations and protection; it cannot erase an Accepted patch.
- Successful acceptance releases its remaining quota reservation only after
  fresh Accepted validation, retaining roots and private claims. Retry repeats
  this cleanup idempotently before acknowledging.
- Active reopen, session validation and range reads understand exact durable
  capture prefixes. Bootstrap, preparation and activation admission remain
  strict. Capture never changes the immutable callback catalogue or downloads.

## Retained evidence

Evidence is under ignored `.layrs-local/validation/`. Every native queue uses a
copied, SHA-256-pinned Rust libtest executable/PDB and a new owned C: fixture
parent; exact parent recipes run serially and stop at the first failure. The
source snapshots retain the exact Rust inputs for each executable.

| Snapshot | Passed evidence | Explicit failures / scope |
| --- | --- | --- |
| `provider-capture-validation-20261026-r1` | Two private capture SQL validators and one family-protection recipe | Family manifest-index fixture failed while injecting an invalid FK; fixed on a dedicated corruption connection, not by weakening production FKs. |
| `…-r2` | Corrected family-index recipe; real cross-process allocation lock; empty-image private SQL cycle; real reserve/install/protect content cycle | Four isolated recipes. Public session driver was not released in this queue. EXE `c16640c649f5f929d7c5d536fe216e7d86473e3d7199a0b16bf9b572dce76fe1`, PDB age 23. |
| `…-r4` | Cross-owner reservation refusal; preauthorized content engine; public empty capture, same-ID retry and new-ID deduplication with reservation release; four ordinary shared-content regressions | Cancellation fixture failed before its action: its ordinary bootstrap witness tried to cross the registered CFAPI checkout reparse point. The guard correctly refused. Queue stopped; remaining recipes were run freshly in r5. EXE `d197a83c8235f2298b764cdb26165943f0d9a0beca663f63ce3fe1dce623ae12`, PDB age 25. |
| `…-r5` | Cancellation before writing with both ordinary metadata DB witnesses unchanged; writable creation on selected absence with durable reopen; constructed Staging/reservation resume, cancellation and subsequent capture | Only the cancellation witness changed from r4. Three native recipes pass. EXE `7825d404e94cd044f5e9d9f8a238d61118d5bd60fc144ff0ca52ef70ccad922e`, PDB age 26. |
| `provider-capture-coverage-20261026-r1` | Replacement of a known writable nonempty placeholder followed by reopen; deletion preserving its expected base; protected mutation refused without Accepted history | Three fresh native recipes pass. The callback catalogue still resolves the original cold base and rejects the replacement FileId. EXE `af886e5ff08713756d83d316dc54789d7ea67021da8826c2a814e0a7c142bb0b`, PDB age 27. Two uncabled recovery-export draft files were retained in this source snapshot but were not compiled or qualified by this queue. |
| `provider-capture-crash-20261026-r1` | Real owned-process termination after durable Staging, and after durable Accepted before quota release/acknowledgement; each resumes in another process with the exact pre-crash A/P binding | Both recipes pass. Replay has one patch, receipt, claim and content mapping; reservation is released and checkout bytes remain intact. EXE `f98a53add97e1a5c87d558edf785a2b9ca4da1f770d79004f22834c7ad4b9edb`, PDB age 31, 426 source inputs retained. The compiled import reader is unfinished and not qualified by this queue. |

`provider-capture-pure-r8.log`: **97 passed**, 277 native recipes intentionally
ignored, 226 other tests filtered. `provider-capture-workspace-check-r2.log`:
`cargo check --locked --offline --workspace --all-targets` passes. The ordinary
regression queue covers two independent members sharing bytes, preparation and
claim ordering, quota preflight before restore, and preparation cancellation.

Independent critical source review identified and rechecked authorization before
effects, callback admission during capture, owner-scoped reservation IDs, typed
error propagation, private-proof continuity and Accepted reservation cleanup.
The parent performed executable/PDB/source binding; the reviewer did not claim
to audit or execute those binaries. Stock Rust/MSVC/Windows loader assumptions
and the earlier host-audit limits continue to apply.

## Remaining work

The earlier constructed Staging prefixes were not process-crash injection. The
new crash recipes terminate only their owned child handles after observable SQL
close and a complete private/family reread. Their barrier acknowledgement is
written and synced to a pending file, then renamed before the parent observes it.
The request and expected A/P binding are fixed before the crash; recovery does
not adopt a binding from post-crash state. `provider-capture-crash-pure-r1.log`
also records 111 passing provider pure tests, with 291 native recipes ignored.

These two cuts do not qualify an uncertain commit/close, the intermediate
installation/protection prefix, or cancellation during installation. Nor do they prove
power loss, arbitrary concurrent external namespace mutation, forced callback
disconnect, or a production host. The source caller must keep external namespace
mutation quiescent for the checkpoint/acknowledgement interval.

Recovery export, managed ordinary recovery, companion preservation, selection
additions, authorized content preparation, CLI/Desktop integration and the final
lifecycle/failure matrix remain open. Historical failure fixtures/logs remain
retained. This checkpoint is a continuation point, not a declaration of L5
completion.
