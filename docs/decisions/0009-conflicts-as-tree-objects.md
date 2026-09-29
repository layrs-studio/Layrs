# ADR 0009: Conflicts are immutable tree objects

Date: 2026-07-19

Status: accepted

## Context

The development implementation stores conflicts as mutable records owned by an
active Weave session. A conflicted Weave pauses before advancing the target
Layer, writes side payloads under a session directory, and requires an explicit
`continue` or `abort`. This makes a conflict transient workflow state instead
of source-control history. It also prevents independent Worktrees and devices
from carrying, publishing, or resolving the same conflict concurrently.

Layrs needs conflicts to survive process restarts, synchronization, graph
rewrites, Layer movement, and server round trips without depending on an open
resolution session.

## Decision

### A conflict is a content-addressed object

`ConflictObject` is an immutable CAS object. Its identifier is computed from a
versioned canonical serialization of all authoritative fields. The normative
wire and identity contract is
`docs/contracts/scm-v1/conflict.schema.json`; no Rust, SQL, sync, server, or UI
type may define a competing ConflictObject shape. It records:

- the conflict kind;
- every affected placement, including all paths involved in a rename;
- zero or more merge-base terms, explicitly tagged `base`;
- at least two conflicting side terms, explicitly tagged `side`;
- file, tree, symlink, or `Absent` values for every placement;
- the exact Lens pin and implementation digest when a Lens produced semantic
  reconciliation data;
- optional rendered content, conflict blocks, ordered segments, and already
  selected partial resolutions;
- the Steps from which bases and sides originated.

`Absent` is a distinct value. It must never be encoded as an empty file. File
objects of zero bytes remain present file values.

Timestamps, actors, devices, Layer names, UI labels, and localized messages are
not part of a ConflictObject's authoritative identity. They belong to the
Operation, Weave receipt, or presentation layer.

Terms have canonical `termId` values. Every selection or resolution records the
exact input `termId` values it used. Labels such as source, target, existing,
incoming, ours, and theirs are execution-local roles recorded in the Weave
receipt. They never determine canonical side order or conflict identity.

### Canonical identity and ordering

Conflict JSON is canonicalized with RFC 8785 JSON Canonicalization Scheme. The
`conflictId` is lowercase BLAKE3 over the ASCII domain separator
`layrs-conflict-v1`, one NUL byte, and the RFC 8785 bytes of the complete object
with `conflictId` omitted.

Nested identifiers use the same construction with an identity projection and
their own domain separator:

- `placementId`: `layrs-conflict-placement-v1`, scoped to its normalized
  `RepoPath` and placement role;
- `termId`: `layrs-conflict-term-v1`, scoped to term kind, provenance, and its
  complete placement-value set;
- `blockId`: `layrs-conflict-block-v1`, scoped to the exact Lens pin,
  implementation digest, base/side values, canonical occurrence anchor, and
  supported methods.

The digest representation is `blake3:` plus 64 lowercase hexadecimal digits.
These projections never include their derived identifier, a parent
ConflictObject ID, display labels, timestamps, or collection ordinals.

Collections with set semantics are duplicate-free and sorted by the ASCII
bytes of their canonical identifier: placements, base terms, side terms,
values within a term, placement resolutions, blocks, term IDs in a resolution,
and origin Step IDs. Ordered conflict segments are sequences because their
order reconstructs content; sequence order is included in the conflict hash.

`supportedMethods` is also a set: duplicate method names reject admission, and
accepted names are sorted lexicographically by their ASCII bytes. The complete
sorted list participates in `blockId`; a method cannot be added, removed, or
substituted without changing the block identity. An unsorted encoding is
non-canonical and rejects admission rather than defining another identity.

Every block has a stable, discriminated occurrence anchor that also
participates in `blockId`:

- a text anchor contains its `placementId` and a duplicate-free set of
  `{ termId, startByte, endByte }` spans, sorted by `termId`; offsets address
  bytes in the immutable term objects, satisfy `startByte <= endByte`, and are
  within the addressed object;
- a structural anchor contains its `placementId`, canonical semantic
  `nodePath`, and duplicate-free `termIds` sorted by ASCII bytes.

An occurrence anchor never contains an insertion ordinal, rendered-marker
offset, database row number, or traversal index. Consequently, inserting an
unrelated earlier conflict does not renumber existing blocks, while two
otherwise identical conflicts at different occurrences receive different
anchors and therefore different `blockId` values.

### Trees may contain conflicts

A tree entry may target a file object, child tree, symlink object, or
ConflictObject. A Step whose root tree transitively contains one or more
ConflictObjects is a valid conflicted Step.

A conflicted Step:

- is durable;
- may be the head of a Layer;
- may be used as the parent of another Step;
- may be published and synchronized;
- may be marked, inspected, diffed, or used in another Weave;
- must retain every base and side object through GC.

`clean` and `conflicted` are derived properties of a Step's tree. A database may
cache them, but the cache is not authoritative.

### Structural conflicts may span several paths

Placements are separate from terms. Each term supplies a value for each
placement. This permits one ConflictObject to represent rename/modify,
rename/rename, directory moves, and other operations involving several paths.
The canonical model must not reduce these cases to an unrelated collection of
single-path content conflicts.

Rename detection is derived analysis. A false rename inference must not change
the identity of the captured source trees. An implementation that has no
reliable rename relation may conservatively represent the operation as delete
plus add.

Every placement path is a `RepoPath` as defined by
`common.schema.json#/$defs/RepoPath`. Conflict processing never defines a
second path grammar. Invalid, non-normalized, absolute, parent-traversing, NUL,
or otherwise non-portable paths reject admission before any object or Step is
accepted.

### Base provenance is explicit

A base term records either:

- a real source Step; or
- a virtual base with its duplicate-free, ASCII-sorted source Step IDs, exact
  base algorithm ID and version, and resulting Tree or CAS object ID.

A virtual base is immutable derived evidence, not a hidden synthetic Step. The
same source set and algorithm version must derive the same virtual-base result.
Side terms always identify their real source Step.

### Resolution creates history

ConflictObjects are never updated in place. Resolving one file, placement, or
block creates, atomically:

1. a new ConflictObject containing the selected partial resolutions, or a
   regular tree entry if that conflict is fully resolved;
2. a new tree;
3. a new Step whose parent is the previously selected conflicted Step;
4. a new Repository View and Operation;
5. a conditional Layer-head update.

If unresolved placements or blocks remain, the new Step remains conflicted. If
two Worktrees resolve the same Step differently, both descendants remain in
the graph and a shared Layer may acquire both heads.

Undo restores repository references through a new Operation and View. It does
not delete the resolved or conflicted Steps.

### Structural resolution is always available

Lenses may add semantic merge and block-level methods, but they may not make a
persisted conflict impossible to resolve. The core always supports applicable
structural choices:

- select an existing side or base value;
- select `Absent` when deletion is a represented value;
- supply a manual file, tree, or symlink value;
- assign explicit destinations for a rename resolution.

Canonical resolution methods are expressed in terms of selected term IDs, not
execution-local words such as existing or incoming. A manual or combined
resolution also lists every input term it considered. Selecting deletion means
selecting a term whose value is `Absent`, or producing `Absent` through an
explicit manual structural resolution.

`both` is only valid when a Lens or the core kind defines an unambiguous order
and output type. Unsupported, unavailable, timed-out, or rejected Lens
execution falls back to a structural ConflictObject with no silent raw merge.

### Materialization is a projection

Materializing a conflicted Step may render text markers or choose a safe preview
for a binary value, but rendered bytes are only a Worktree projection. The tree
continues to reference the ConflictObject. The Worktree records which conflict
and rendered-object digest produced each projected path.

An edited projection becomes a manual resolution only through the conflict
capture protocol. Ordinary automatic capture must not silently replace an
unresolved ConflictObject with the rendered file.

### Synchronization and garbage collection

Sync transfers ConflictObjects like every other CAS object. All referenced
values, semantic payloads, and rendered objects are uploaded before the Step
that reaches them. The server validates object availability and canonical IDs;
it does not replace the submitted conflict by rerunning a different Lens.

GC traversal follows tree-to-conflict and conflict-to-value edges. Any Step,
Layer head, Marker, Operation View, pending sync item, or lease that reaches a
ConflictObject also retains all of its transitive objects.

## Invariants

- A tree never references a missing ConflictObject.
- A ConflictObject never references a missing Step or CAS value.
- Every side has a value for every declared placement.
- Base and side terms are explicitly distinguished; a term occurs in exactly
  one of those collections.
- Every selection and resolution names at least one existing term ID.
- Execution-local source/target/existing/incoming roles never affect canonical
  collection order.
- Every value kind agrees with its referenced object kind.
- `Absent` has no object identifier, length, digest, or `executable` value.
- An empty file has a file object identifier and is not `Absent`.
- A present conflict value carries `executable: boolean | null`; `null` means
  the bit is not applicable to that value kind and is distinct from `false`.
- Conflict IDs, block IDs, placement IDs, and term IDs are stable within their
  documented scopes.
- A fully resolved conflict is not retained as a conflict entry in the result
  tree.
- Partial resolution never mutates the parent Step or its ConflictObject.
- Lens-derived payloads identify the exact executed Lens pin and implementation
  digest.
- Set-like collections are duplicate-free and canonically sorted; only segment
  sequences use array position as semantic identity.

## Consequences

- The session-owned conflict tables and side-blob directories are not part of
  the new storage format.
- `weave continue` and `weave abort` are not graph primitives. A UI may offer a
  temporary resolution workspace, but it is never the source of truth.
- Tree, sync, server, fsck, and GC schemas must add ConflictObject edges.
- Resolution commands become ordinary Step-producing operations.
- File/directory, executable-bit, symlink, and rename conflicts require first-class tree
  entry kinds instead of a flat file-only snapshot.

## Rejected alternatives

- **Mutable conflict rows attached to a Weave:** cannot be carried by history
  and makes concurrent resolution destructive.
- **Conflict markers as authoritative file contents:** loses binary, deletion,
  structural, and provenance information.
- **Treat deletion as an empty file:** is not byte- or filesystem-correct.
- **Require all blocks to be resolved before creating history:** recreates a
  modal session and loses useful intermediate work.
- **Let each server rerun its preferred Lens:** makes results depend on where
  they are synchronized.
