# Managed destination admission — implementation boundary

Date: 27 October 2026. Status: handshake design independently reviewed;
implementation in progress, not globally qualified. The baseline is local commit
`ddf31d3` and its bounded publication, capture and autonomous SQL evidence in
[the managed checkpoint](worktrees-partial-vfs-managed-source-checkpoint.md).

## Required outcome

A managed destination becomes an ordinary format-4 Worktree that can reopen
without its original source or transfer holders, preserve imported history and
provenance, and accept subsequent ordinary capture/restore operations. Passing
the exact initial Ready interpreter alone does not satisfy this outcome: that
interpreter deliberately fixes the initial head, capture count and claim set.

The implementation separates three boundaries:

1. Read-only physical reopening of initial Ready authenticates the actual
   destination FORMAT, private database, family, checkout identities and exact
   protected CAS representations. No current source connection or source member
   is needed. This is the first implementation in progress.
2. A durable publication handoff establishes that the matching source
   transaction really reached Published. The destination remains unmodifiable
   until this handoff is independently validated and durably recorded.
3. Explicit ordinary admission preserves immutable imported provenance while
   allowing an ordinary mutable suffix. Each future opener must distinguish
   those two domains rather than reassert initial Ready as the current state.

## Publication handoff

The current source publisher requires an exact unchanged destination Ready.
Allowing destination mutations while source publication is still pending could
make source completion impossible. A destination Ready receipt is therefore not
evidence that the source reached Published.

The proposed handoff reads the genuine canonical source Published intent and
receipt, compares their transaction, plan, prefix and destination bindings with
the complete destination Ready proof, then records a bounded acknowledgement
and admission in one destination transaction. Admission requires that
acknowledgement. Source, family and
destination owners and the necessary read snapshots span the destination write;
positive closes and a fresh interpretation precede acknowledgement to callers.
The driver must open its own independent read-only source connection through
the observed database IO path. A caller-supplied source writer transaction could
contain an uncommitted Published row and is not evidence of durability: it must
never be accepted as authority to COMMIT destination admission. The historical
SQL interpreter alone deliberately makes no durability claim.

Recovery must handle the gap after SourcePublished commits but before the
handoff commits. The source is already allowed to capture in this gap. Retry
must therefore validate the matching Published transition as immutable history,
not require its old capture head or image to remain the current source state.
It must never synthesize SourcePublished from destination Ready fields. If the
source cannot be inspected and no handoff exists, the initial destination can
be inspected read-only but cannot yet receive ordinary mutations. Independent
review accepted this tradeoff: autonomy follows the durable handoff. It
recommended preserving the current publisher rather than making its retained
physical Ready proof accept subsequent destination mutations.

## Durable provenance and ordinary suffix

The proposed private schema extension archives the exact checkout prejournal
as immutable import provenance and writes a compact admission record binding
the prefix, Ready receipt, publication handoff, protection root, exact binding,
rowset digest and initial Ready generation/capture boundaries. The operational
prejournal must no longer masquerade as an unfinished ordinary operation.
The archive move and admission record belong to one private SQLite transaction.
FORMAT remains version 4 with its existing import digest; schema versioning and
compatibility checks must explicitly describe the new extension.

The permanent admitted-import reader validates copied prefix and initial Ready
suffix at their historical boundaries, including archived exact representation
provenance. Ordinary history validators separately cover later heads, captures,
claims and reservations. Merely allowing nonempty provider evidence in the
existing ordinary/reclamation gate would be incorrect.

Family protection roots, exact installation journals and their reservation
charges remain retained until a separately specified cleanup protocol exists.
Ordinary content admission needs a typed policy for these retained imports;
global acceptance of arbitrary exact journals is not permitted. Cleanup and GC
must not silently become enabled as a side effect of ordinary admission.

## Required acceptance evidence

- Reopen after all original holders are dropped, with source absent, and reject
  substituted FORMAT, database, family, checkout or CAS objects.
- Refuse ordinary mutation when source completion has no durable handoff.
- Resume the handoff after source publication and a later genuine source
  capture, preserving both histories and all outgoing receipts.
- Resume each committed admission step without adopting unrelated artifacts,
  losing provenance, double-debiting or releasing protection early.
- Open, capture a new ordinary edit, close and reopen again; restore and read
  historical imported content while the initial Ready boundary remains intact.
- Validate coexistence of more than one imported member/reservation in a
  family, and refuse unrelated or corrupt retained journals.
- Keep cleanup/reclamation refused until its separate protocol and evidence
  prove that every retained dependency can be released safely.

Companions, selection expansion, public CLI/Desktop integration and complete
lifecycle qualification remain programme requirements beyond this boundary.

## Current bounded evidence

`OrdinaryRootGuard` is implemented and independently source-reviewed. It uses
the existing local-NTFS ancestor pins, held/fresh identity and CFAPI registration
checks and bounded stream inventory, without requiring an empty root. It does
not freeze descendants or authorize mutation. VFS all-target check r1 passes.

The historical `published_handoff` SQL interpreter is implemented and reviewed.
It retains the complete source publication interpretation, selects an actual
Published source row and compares its full target commitments. The strengthened
source-publication SQL-memory recipe passes on r8 (child120/outer180, fresh C:
parent): Ready refusal, actual Published, internally coherent different-plan
refusal, acceptance after another captured history entry, and refusal after
the source transaction ends. This is not a durable admission execution.

The physical `ManagedReadyInspection` and its native recipe are integrated;
all-target managed-reopen check r1 and no-run build r1 pass. Independent review
released the exact native recipe, which passes on r8 in 152.63 seconds,
child240/outer300 under a fresh C: parent. The fixture completes SourcePublished
and returns only the destination PathBuf after the original helper disconnects
and drops all source/transfer holders. New open/require/drop/reopen operations
retain exact CAS and working-file pins, refuse write access while pinned and
honour cancellation before open. Bounded checkout/family witnesses stay exact.
This proves read-only inspection without original holders or a source
connection; the source files themselves are retained. It does not qualify
ordinary edits, schema9 admission, crash recovery or source deletion.

The r8 artifact has 538 source inputs and CodeView
age 81, EXE SHA256
`525cc3ae45d60a6cf1321497380585745d12b4872b5084d29ba45c90964b69e4`.
Frozen source/evidence directory:
`.layrs-local/validation/provider-family-publication-20261027-r8`.
Unmodulated transition-observation drafts are included in its source inventory;
they were not compiled and receive no qualification from this artifact.

## 27 October continuation: private schema9 reader qualification

The two private historical-reader SQL-memory recipes passed on the audited r11
binary (`989dde773830ba69ab6ac880f54c6df9c3fd6d0a5b7bbd97469cac6c944f2ec0`).
The tests cover immutable imported provenance coexisting with later local
captures, publications, claims, head generations and operational prejournal
rows; ACK/archive/schema substitution refuses without writes. A separate SQL
recipe confirms the shared detached mapping commitment matches SQLite encoding
and rejects representation/type substitution. Runs use bounded owned children,
a fresh disposable parent and stop-first-failure execution; all three passed.

These are DATA reader results only. Schema9 remains explicitly fenced from
ordinary mutable access and reclamation. The migration writer, independent
read-only source handoff retained through destination commit/positive closes,
physical admission, budgets, family policy and ordinary dispatch remain open.
The new admission budget helper passed independent source review and all seven
pure r12 recipes. It accounts for `F_ready + 2J_bound + Amax`, refuses an
insufficient existing reservation, and preserves the separate family margin.
This is logical column accounting, not a measurement of SQLite pages or physical
space. The transactional writer is implemented and under review; the production
planner and physical admission driver are still open.

## Transactional admission writer qualified in r13

`managed_admission::admit_ready` takes real retained opaque Ready and historical
Published SQL proofs. It reinterprets them and reconstructs the metadata floor,
baseline Q0, fixed M and net remaining family capacity before effects. It copies
the exact checkout journal into its permanent archive, writes the actual
SourcePublished body and ACK record, removes the operational journal, sets
schema9 and invokes the complete private historical reader. The caller owns
COMMIT/rollback/positive closes; an error after effects requires rollback.
Already-admitted replay reads schema9 without touching source/family connections.

Independent review found no remaining blocker after fixing a representation
comparison: family protection orders by ManifestId, while provenance orders by
(FileId, ManifestId). The writer now compares sorted exact triples with unchanged
cardinality. Its pure regression and all four owned SQL-memory recipes pass on
r13 (EXE `16f86c9e9f577a296c193b42e05746d4383887e77d4a85e0bbbdda19b26ecac7`,
CodeView age 88). The recipes cover exact copy/replay, caller rollback to8,
changed journal or historical ACK refusal before effects, and inadequate legacy
budget without resizing. They are not physical or durable-source qualification.

## Remaining ordinary-access integration, mapped against current code

1. Private interpreter: recognize true schema9 and permanent provenance in short
   transaction scopes. `VerifiedAdmittedImport` must not outlive its transaction.
   Keep `require_ordinary` closed where no typed context exists, and keep the
   independent reclamation fence closed for9.
2. Physical access: lifecycle -> owner -> FORMAT/private DB pins -> typed RO
   private interpretation -> typed family gate -> requested private handle.
   `format::access/verify/open` and the `marker.import_digest` branches currently
   assume ordinary conversion receipts. Do not require immutable initial checkout
   files forever: legitimate user edits must remain openable after admission.
3. Family policy: a distinct admitted context must validate its imported root,
   exact union, journals and reservation alongside new ordinary roots/claims.
   Existing exact protection requires a unique root per owner; existing exact
   installation reader refuses other reservation groups. Multiple retained
   imports need group-aware validation, rejecting unknown/corrupt groups.
4. Context propagation: `retention`, `capture/storage` and `offline` recreate
   Ordinary leases through `shared::for_store`; `usage` and `registered_state`
   call the catalog directly. Carry the existing owner-scoped admitted context
   through these paths. Never acquire owner again beneath a catalog gate or
   recurse from private validation through family opening back to private DB.
5. Capture/publication provenance: dispatch conversion and admitted history
   separately. Imported captures use actual segment origins and both generation
   bounds; C_ready and later local captures use destination identity. Imported
   captures remain non-publishable; C_ready follows ordinary publication checks.
   The origin-specific low-level loaders must remain independent of dispatch.
6. Exact content mapping: recapture must prefer the existing private protected
   ManifestId. Choosing the first family manifest for a FileId can conflict with
   a different imported representation. Do not rewrite the imported mapping.

Required integrated scenarios include source-independent reopen9, edit/capture/
drop/reopen/restore; inherited origins over several segments; C_ready publication;
two representations of one FileId; ordinary claims alongside imported protection;
two retained imports; corruption/refusal without repair; owner/catalog contention,
identity replacement, cancellation and close failures. Ready8/provider/reclamation
and unsupported recovery must remain fenced until their own routes are qualified.

Future B->C export additionally needs explicit schema9 handling in
`records/import`, `import_validation::{source,ancestry,scoped,copy}`. Anchor on B's
admitted incoming receipt, never an arbitrary outgoing receipt A->D. Ordinary
restore/refresh/rebase history is still refused by source closure until explicitly
represented; accepting a version number alone must not bypass that limitation.
