# SCM V1 conflict scenarios

These scenarios are normative acceptance cases for ConflictObjects, Weaves,
resolution, synchronization, and undo. Identifiers shown symbolically stand for
valid canonical identifiers.

## General assertions

Every scenario assumes the following invariants:

- accepted objects are immutable and content-addressed;
- a Layer references a set of Step heads and does not own Steps;
- a tree may reference a ConflictObject;
- `Absent` and a present zero-byte file are different values;
- a resolution creates a descendant Step and never mutates its parent;
- publish, receive, and GC traverse all conflict-to-side object edges;
- authorization, Gate, validation, and canonical-data errors are command
  rejections rather than content conflicts.
- `conflict.schema.json` is the sole authoritative ConflictObject shape;
- set-like collections are duplicate-free and sorted by canonical identifier
  bytes; only conflict segments and explicitly documented execution sequences
  use array order semantically.

## CF-000 — canonical IDs, term roles, and ordering

**Given** semantically identical ConflictObjects whose input maps and set-like
collections were constructed in different insertion orders, **when** they are
normalized, **then** placements, base terms, side terms, values, resolutions,
blocks, resolution term IDs, and origin Step IDs have identical canonical
order. RFC 8785 serialization and the `layrs-conflict-v1` BLAKE3 domain derive
the same `conflictId`.

Placement, term, and block IDs use their documented domain-separated identity
projections. Base terms have `termKind: base`; side terms have `termKind: side`.
Execution-local source/target/existing/incoming labels appear only as receipt
role mappings to explicit `termId` values and cannot change conflict identity.
Real merge-base IDs equal their Step IDs. Virtual-base, deterministic-profile,
Lens-execution, and receipt IDs use their documented RFC 8785 and
domain-separated BLAKE3 projections.

For each block, `supportedMethods` rejects duplicates, is sorted
lexicographically by ASCII method name, and participates in `blockId`.
Changing the accepted method set changes `blockId`.

## CF-000A — stable block occurrence anchors

**Given** a text block, **when** its identity is computed, **then** its
occurrence anchor contains the placement ID and one immutable-object byte span
per participating term, sorted by `termId`. Every span satisfies
`startByte <= endByte` and lies within its term object. Inserting an unrelated
conflict earlier in rendered content does not change this anchor or `blockId`.

**Given** a structural block, **then** its anchor contains the placement ID,
canonical semantic node path, and duplicate-free ASCII-sorted term IDs. No text
or structural anchor contains an insertion ordinal, traversal index, database
row number, or rendered-marker offset. Two otherwise identical blocks at
different occurrences have different anchors and therefore different
`blockId` values.

## CF-001 — clean text auto-merge

**Given** a base text file and two heads that modify disjoint line ranges,
**when** an accepted Weave reconciles those heads with an exact Text Lens pin,
**then** it creates one clean result Step with both modifications, a Weave
receipt with outcome `clean`, no ConflictObject, and one deterministic Lens
execution record.

Repeating the pure reconciliation with identical inputs and resource profile
must produce the same tree and Step IDs.

## CF-002 — overlapping text conflict

**Given** two heads that modify the same base line differently, **when** they
are Woven, **then** the accepted Weave creates a conflicted result Step. Its
tree references a content ConflictObject containing the exact base and side
file objects, exact Text Lens pin and implementation digest, conflict blocks,
segments, and an optional rendered marker object.

The rendered marker object is not the authoritative tree value.

## CF-003 — partial block resolution

**Given** a ConflictObject with two unresolved text blocks, **when** a user
resolves only the first block by recording the exact input `termId` set and
selected result, **then** Layrs creates a new ConflictObject, new
tree, descendant Step, Repository View, and Operation. The second block remains
unresolved, the new Step remains conflicted, and the original Step and conflict
remain inspectable.

Resolving the second block creates another descendant whose tree contains the
resolved file rather than a fully resolved ConflictObject.

## CF-004 — byte-exact binary conflict

**Given** base, existing, and incoming binary objects that all differ, **when**
Raw reconciliation cannot auto-resolve them, **then** Layrs creates a binary
ConflictObject without text markers. Choosing `incoming` produces a descendant
whose file bytes and object ID exactly equal the incoming side.

No UTF-8 conversion, newline normalization, image decoding, or lossy preview is
allowed in the authoritative result.

## CF-005 — delete/modify and empty-file distinction

**Given** a base file, one side with `Absent`, and another side with modified
content, **when** the heads are Woven, **then** the ConflictObject kind is
`delete_modify` and the deleted side is represented by `{ "kind": "absent" }`.

Choosing deletion records the `termId` of the side whose value is `Absent` and
removes the tree entry. Choosing a present zero-byte manual file records all
input term IDs considered and creates a file entry referring to a zero-byte
file object. These outcomes must have different trees and Step IDs.

## CF-006 — file/directory type conflict

**Given** one side with a file at `assets/theme` and another with a tree at that
path, **when** they are Woven, **then** Layrs creates a `directory_file`
ConflictObject whose sides retain the file and tree object IDs.

Selecting either side materializes the correct filesystem type atomically. A
failure during replacement restores the prior Worktree state and does not
acknowledge the new View.

## CF-007 — directory conflict

**Given** both sides changed different descendants of the same directory,
**when** child trees can be reconciled cleanly, **then** the result contains a
clean merged child tree. If a child path conflicts, the result child tree
contains that ConflictObject while non-conflicting siblings remain present.

Empty directories, when supported by the canonical tree format, remain distinct
from `Absent`.

## CF-008 — symlink conflict

**Given** one side changes a symlink target and another replaces it with a file,
**when** they are Woven, **then** the result is a `type` ConflictObject retaining
the symlink and file objects. Materialization must create a symlink only after
path-containment, platform-policy, and reparse-point validation succeeds.

If local policy forbids symlink creation, the Step remains valid and
synchronizable even though that Worktree cannot materialize the selected value.

## CF-009 — rename/modify conflict

**Given** one side renames `a.txt` to `b.txt` and the other modifies `a.txt`,
**when** a deterministic rename relation is available, **then** a single rename
ConflictObject declares placements for both paths and terms describing the
values at each placement.

A resolution explicitly chooses the destination and content. The system must
not emit two unrelated single-path conflicts that can independently create
both files by accident.

Without a reliable rename relation, representing delete plus add is permitted,
but the captured trees remain unchanged by the inference.

## CF-010 — rename/rename conflict

**Given** the same source path is renamed to two different destinations,
**when** the heads are Woven, **then** all source and destination paths are
placements of one ConflictObject. A manual resolution may select one
destination, retain both under explicit names, or delete the value. Each choice
creates a descendant Step.

## CF-011 — executable-bit-only conflict

**Given** identical file bytes but divergent executable bits, **when** they are
Woven, **then** the ConflictObject kind is `mode`; both present values use the
same file object ID and distinct boolean `executable` values. Selecting either
value creates a new tree entry with the same file object ID. `null` is reserved
for value kinds where the bit is not applicable and is distinct from `false`;
an integer filesystem mode is never admitted to this V1 contract.

## CF-012 — no merge base

**Given** unrelated source and target histories, **when** an accepted Weave is
configured to reconcile them, **then** the receipt has an empty `mergeBases`
array and conflicts use zero base terms or explicit absent bases according to
the canonical algorithm. No arbitrary head is treated as the base.

## CF-013 — multiple minimal merge bases

**Given** a criss-cross graph with several minimal common ancestors, **when** a
Weave is planned, **then** every selected base is recorded. The engine either
creates a deterministic virtual base or preserves all base terms. Database row
order, timestamp, actor, device, and Layer name must not select a winner.

A virtual base records its ASCII-sorted source Step IDs, exact algorithm ID and
version, and resulting Tree or CAS object. Repeating derivation with the same
profile produces the same virtual provenance and base term ID.

## CF-014 — unsupported or unavailable Lens

**Given** an artifact whose selected Lens lacks reconciliation support, is
missing, times out, exceeds quota, traps, or returns invalid output, **when** the
input objects themselves are valid, **then** Layrs preserves them in a
structurally resolvable ConflictObject. Applicable whole-value and manual
methods remain available.

Code and Image conflicts specifically retain their exact objects and expose
term-ID-based structural choices. They must not route bytes through Raw
silently and must not choose a side automatically.

## CF-015 — Lens determinism and quotas

**Given** an exact Lens pin and fixed resource profile, **when** reconciliation
is executed on two supported hosts, **then** canonical output is byte-identical.
The Lens has no ambient network, time, random, locale, filesystem, or
environment input.

If a resource limit is reached, no partial Lens output is accepted. Both input
sides remain reachable and the fallback behavior is CF-014.

The receipt records canonicalization/hash, merge and base algorithm versions,
host policy ID/version/digest, disabled ambient capabilities, and all memory,
time, input, output, fuel, block, segment, path, and recursion-depth limits. It
also records the complete resolution policy: ID/version/digest, duplicate-free
ASCII-sorted allowed methods, automatic-side-selection prohibition, `Absent`
semantics, combine ordering, manual-input and rename policies, and the action
for every unsupported/unavailable Lens, timeout, quota violation, trap, invalid
output, invalid signature, and digest mismatch.

Changing any resolution-policy field changes `profileId` and the Lens execution
identity. Reordering the same already-canonical method set does not; a duplicate
method rejects admission.

## CF-016 — pure preview

**Given** source and target heads, **when** a Weave preview is requested, **then**
it returns a proposed clean or conflicted model without creating a Step,
receipt, Operation, active session, pending-sync item, Worktree write, or Layer
head update.

Changing a head after preview does not corrupt apply; apply validates or
recomputes from explicit current Step IDs.

## CF-016A — no-op rejection before acceptance

**Given** selected heads for which reconciliation adds no parent relation,
tree state, head state, or other repository-visible result, **when** Weave is
requested, **then** Layrs returns `NoChanges` before acceptance. It creates no
Step, Operation, Repository View, Weave receipt, or pending-sync item.

If the same Tree joins a genuinely new parent set, it is not a no-op: the
accepted Weave creates or reuses exactly one result Step and creates one
Operation.

## CF-017 — Gate, authorization, and validation rejection

**Given** a failed Gate, denied actor, malformed selector, missing object, or
invalid canonical payload, **when** Weave is requested, **then** no result Step,
ConflictObject, receipt, View, pending-sync item, or head movement is created.
The error is not reported as a content conflict.

## CF-017A — invalid and non-canonical paths

**Given** a placement or Lens execution path that is absolute, contains a NUL,
contains parent traversal, is not normalized, or fails the shared `RepoPath`
contract, **when** an object, preview, or Weave is admitted, **then** admission
fails before hashing or reference mutation. No implementation may sanitize the
path into a different authoritative value or apply a private path grammar.

## CF-017B — typed receipt identifiers

**Given** a Weave receipt, **then** its Space, Step, Conflict, Operation, term,
timestamp, and content-addressed identifiers satisfy the corresponding
`core.schema.json`, `conflict.schema.json`, or `common.schema.json` definition.
A generic non-empty string that does not satisfy the business identifier type
rejects the receipt before hashing or reference mutation.

## CF-018 — concurrent resolutions

**Given** two Worktrees based on the same conflicted Step, **when** they resolve
the conflict differently while offline, **then** each creates a valid descendant
Step. Synchronization retains both descendants. If both update the same Layer,
the Layer acquires both heads instead of applying last-write-wins.

A later Weave can reconcile those resolutions like any other heads.

## CF-019 — publish and receive a conflicted Step

**Given** a conflicted Step, **when** it is published, **then** chunks, file/tree/
symlink objects, ConflictObjects, semantic payloads, rendered objects, and Steps
are uploaded in dependency order before the Layer-head update.

Another device can receive the Step, inspect every side, materialize a safe
projection, and create a resolution descendant without access to the creating
device's session files.

The published receipt outcome and `conflictIds` must exactly match the
transitive ConflictObject closure of the result Step. A clean receipt with any
conflict ID, a conflicted receipt with none, or a missing/extra ID is rejected.

## CF-020 — undo a Weave or resolution

**Given** a Weave or partial resolution moved a Layer head, **when** the user
executes undo, **then** Layrs creates a new Operation and Repository View that
restore the prior head set. It does not delete result Steps, ConflictObjects,
side objects, or receipts. Redo can restore the later View while its objects
remain available.

## CF-021 — crash during conflict creation

**Given** a fault at any point while installing conflict sides, ConflictObject,
tree, Step, View, receipt, or head update, **when** recovery runs, **then** the
repository exposes either the complete old View or the complete new View. It
never exposes a Step with missing conflict dependencies and never acknowledges
an uncommitted head movement.

## CF-022 — garbage collection

**Given** a reachable conflicted Step, **when** GC runs, **then** every base,
side, block payload, segment payload, rendered object, Lens bundle pin required
by policy, and child object remains available.

Once no Layer, Marker, Worktree, Operation View, pending sync item, receipt
retention policy, pin, or lease reaches those objects, conservative GC may
collect them according to repository policy.
