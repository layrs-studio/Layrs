# Layrs SCM V1 Target Architecture

This document is normative for the new source-control core. It supersedes the
source-control model described by the development V1 overview where the two
conflict. It does not describe a compatibility layer.

## Product model

Layrs continuously protects working files as anonymous immutable Steps.
Users do not create commits. They may independently create Markers or Releases
when a point deserves a name, explanation, proof or publication lifecycle.

The core model is:

```text
Store
  CAS: Chunk -> FileManifest -> Tree -> Conflict/Symlink/Tombstone
  Index: logical FileId -> {installed FileManifestId}
  SQLite: graph, views, operations, worktrees, markers, sync and recovery

Space
  Layer definitions
  globally reusable Step DAG

RepositoryView (immutable, local)
  Layer -> {zero or more Step heads}
  visible Marker/Release ids
  Worktree states

Operation (immutable, local DAG)
  parent Operation ids
  resulting RepositoryView
  optional Step rewrite map

Worktree (local checkout)
  Store and Space association
  validated checkout path

Marker / Release
  immutable named reference to one Step
```

## Identity layers

### Content-addressed identities

Every canonical object id is the lowercase BLAKE3 digest of
`domain ASCII || NUL || canonical payload without the claimed id`. Domains are
versioned and unique per object type: `layrs-step-v1`, `layrs-tree-v1`,
`layrs-conflict-v1`, `layrs-symlink-v1`, `layrs-tombstone-v1`,
`layrs-repository-view-v1`, `layrs-chunk-v1`, `layrs-file-v1` and
`layrs-file-manifest-v1`. Canonical collections are sorted and duplicate-free.
Equal ids mean equal canonical payload in the same domain; cross-type payloads
cannot collide by construction.

The Step payload uses the specialized encoding fixed by ADR 0006. Tree,
ConflictObject, SymlinkObject, TombstoneObject, RepositoryView and FileManifest
payloads use RFC 8785 JSON Canonicalization Scheme after removing only the
claimed id property. The `schemaVersion` property remains encoded. Arrays that
represent sets must already satisfy their normative sort order; canonical JSON
does not reorder arrays. Chunk and logical File payloads are their decoded raw
bytes, prefixed by their respective domains and the mandatory NUL separator.

`common.schema.json` is the single core authority for `ObjectId` and
repository-relative path syntax. `file.schema.json` and `chunk.schema.json`
define the canonical FileManifest and Chunk contracts. ConflictObject structure
and identity are defined only by `conflict.schema.json`; the core Tree contract
contains a typed reference to that external authority and does not duplicate
it.

RepoPath and TreeEntry component strings must arrive in Unicode NFC and are
rejected otherwise. Layrs never relies on RFC 8785 to normalize strings because
that standard deliberately preserves their code points.

### Stable opaque identities

Layer, Worktree, Marker, Release, Operation, observation, actor, device, Space
and Store ids are validated opaque ids. Their prefixes identify their type.
They are never inferred from display names or paths.

### Observation metadata

Times, actors, devices and capture reasons are observations. They never alter
Step identity. Several observations can point to the same Step.

## Canonical object graph

Trees are hierarchical and contain only immediate component names. A TreeEntry
has one target kind and matching target id. File entries target logical
`FileId`, never a particular chunk layout. A ConflictObject is a valid entry
target, so unresolved conflict state is captured by the root Tree and Step id.

`FileId = BLAKE3("layrs-file-v1" || NUL || raw file bytes)`. A CAS
`FileManifest` separately identifies one storage representation as
`BLAKE3("layrs-file-manifest-v1" || NUL || RFC8785(manifest without
manifestId))`; its payload contains `fileId`, byte length, chunking descriptor
and ordered chunk slices. Multiple manifests may resolve one FileId. The local
store and sync catalog maintain `FileId -> {FileManifestId}` mappings and may
select only a fully installed manifest whose chunks reconstruct and verify the
FileId. Thus every CAS id maps to exactly one canonical byte sequence.

Every accepted content-addressed object must satisfy both conditions:

1. its claimed id matches its canonical bytes;
2. all referenced child objects exist or are installed in the same prepared
   object batch before metadata acceptance.

Object publication happens before the SQLite acceptance transaction. A crash
may leave unreachable CAS objects; it must never leave an accepted Step whose
Merkle closure is absent. Orphans are safe and collected only after a grace
period.

## Repository state

Layers contain no heads. The only authoritative Layer head sets are inside
RepositoryViews. A multi-head Layer is normal divergent state, not corruption.
Each head set is an antichain: it contains only graph-maximal Steps, never both
an ancestor and its descendant.

A RepositoryView also records Worktree state needed for undo, but not checkout
paths. The Worktree definition stores local paths separately. Each Worktree has
exactly one mutable `activeOperationId`; the referenced Operation selects that
Worktree's unique current View. The repository may simultaneously have several
global Operation heads. Query accelerators may project a selected View but can
never become an independent authority.

All RepositoryView entries are sorted by typed id; all Step head sets are
sorted and duplicate-free.

## Operation model

Every successful local state mutation follows this pipeline:

1. read the current Operation head set and View;
2. scan/read stable filesystem inputs without an exclusive metadata lock;
3. build, hash, durably install and verify CAS objects;
4. compute the candidate Step and RepositoryView;
5. acquire a short exclusive metadata session;
6. compare the expected Operation head set;
7. atomically insert missing indexes, Step metadata, observations, View and
   Operation, then advance Operation heads;
8. acknowledge success only after SQLite commit and required durability
   barriers.

If step 6 detects concurrent change, Layrs never overwrites it. It either
recomputes against the new View or creates an explicit divergent/integration
Operation preserving every head.

Integration always creates a new multi-parent Operation. The new Operation and
View, global head replacement, and all affected Worktree
`activeOperationId` updates commit in one transaction. A Worktree never has an
ambiguous current View.

Operations are the user-visible undo history. Recovery journals are mutable
technical state machines for incomplete side effects. Server event logs are
shared append-only audit. They remain separate even when cross-referenced.

## Automatic capture

Capture occurs:

- before Layer or Worktree materialization changes;
- before Weave, resolution, rewrite, receive or compaction operations that may
  affect the checkout;
- at the start of commands that require a current snapshot;
- after a configured filesystem quiescence interval;
- when explicitly requested.

A stable scanner validates file metadata before and after reading and retries a
file changed during capture. No empty state transition is recorded. Background
capture is coalesced per Worktree, but the underlying accepted Steps remain
individually inspectable.

Advancing a Layer from a Worktree uses compare-and-swap against the Worktree's
base View. Concurrent advancement produces multiple heads rather than loss.

## Materialization

Before modifying user-visible paths, Layrs:

1. captures outstanding changes;
2. validates every relative path and rejects links/reparse escapes;
3. creates and fsyncs a byte-exact backup plus recovery journal record;
4. writes each replacement through temporary files and durable renames;
5. verifies the resulting Tree;
6. records the new Worktree state in a RepositoryView Operation;
7. finalizes and later retires the technical backup.

A crash before finalization restores the previous bytes or reports
`needs_repair`; it never guesses that a side effect completed.

## Conflicts and Weaves

A Weave always produces a Step when it has a material result. If reconciliation
cannot resolve a path, the result Tree references a ConflictObject. The Weave
is not held in a mandatory `continue/abort` session.

The canonical conflict structure is defined exclusively by
`docs/contracts/scm-v1/conflict.schema.json`. Resolving all or part of a
conflict creates a descendant Step. The original conflict Step remains
reachable through history. Conflict Steps can be marked, synchronized,
rewritten and used as parents.

Temporary resolution UI state is a cache and never the conflict authority.

## Graph and rewrite engine

`layrs-graph` provides validated DAG traversal, topological ordering, ancestor
and descendant queries, common ancestors, selectors and immutable rewrites.

Every rewrite returns:

- newly created Steps;
- a many-to-many rewrite map;
- the proposed RepositoryView changes;
- any propagated ConflictObjects.

Layers following rewritten Steps move according to explicit rewrite rules.
Immutable Markers and Releases never move automatically.

## Native synchronization

Synchronization sends native Layrs objects in dependency order:

1. chunks;
2. files, subtrees and conflict-related objects;
3. Steps in topological order;
4. Markers, Releases, Proofs and Gates;
5. generation-checked reference operations.

The server validates object identities, closure, authorization and expected
reference generations. A reference race preserves all uploaded objects and
returns divergent heads. The client integrates them into a local Operation.
There is no Git backend and no last-write-wins fallback.

## Crate boundaries

### `layrs-core`

Owns typed ids, canonical domain structures, enum semantics and canonical
encoding contracts. It performs no filesystem, SQLite, HTTP or UI work.

### `layrs-graph`

Depends on `layrs-core`. Owns Step DAG validation, queries, selectors and
rewrite planning. The generic business-relation graph is a separate module.

### `layrs-store-local`

Depends on core/graph. Owns the only local CAS writer, SQLite baseline,
transaction repositories, locks, materialization recovery, fsck, repair,
retention and GC. It exposes typed APIs and no document-filesystem emulation.

### `layrs-client-core`

Orchestrates capture, switching, Weaves, conflict resolution, rewrites and sync
through typed store/graph APIs. It does not issue SQL and does not implement a
second CAS.

### `layrs-sync`

Depends on `layrs-core`. Owns wire envelopes, batch ordering, validation at
trust boundaries, idempotency and acknowledgements. It does not define Step,
Tree or Layer identity.

### Surfaces

CLI, Desktop, Web and SDK are projections and command surfaces. They do not
implement source-control rules independently.

## Store baseline

The new store has a new format version and one checksummed schema baseline. Its
logical table groups are:

```text
store_meta, spaces
objects, object_edges
steps, step_parents, step_observations
layers, layer_relationships
repository_views, view_layer_heads, view_markers, view_worktrees
operations, operation_parents, operation_rewrites, operation_heads
worktrees
markers, releases, release_publications, marker_proofs
recovery_operations, recovery_paths, materialization_leases
pending_objects, pending_steps, pending_ref_updates, sync_cursors
object_pins, gc_roots
```

The exact SQL belongs to the Store implementation phase, but it must enforce
the invariants in `docs/contracts/scm-v1/invariants.md`.

Old development stores are detected and refused. Layrs does not migrate,
delete or rewrite them.

## Cross-Space admission

Content-addressed objects may be physically deduplicated across Spaces, but
knowledge of an ObjectId is not authorization. A Space reference is accepted
only if the caller may attach the complete Step-parent and Merkle closure.
Every Step reachable from a Layer, Marker, Release or View in a Space has its
complete ancestor and object closure admitted to that same Space.

Layer, View, Operation, Worktree, Marker, Release and ReleasePublication Space
ids agree across every relation. Cross-Space reuse is an explicit, auditable
admission/import Operation; it never creates a direct mutable reference between
Spaces and never exposes objects merely because the CAS deduplicated bytes.

## Garbage collection

GC uses an explicit root inventory and full graph marking. Initially the whole
Operation DAG is retained, providing complete undo at the cost of storage.
Future pruning is an auditable operation that changes retention roots; it is
never an incidental side effect of capture or sync.

GC and repair are mutually exclusive with acceptance transactions and honor
active readers, leases, pins, pending sync and a grace period for CAS orphans.

## Required implementation order

1. normative contracts and golden encodings;
2. canonical core types;
3. Step DAG primitives;
4. fresh durable Store baseline;
5. automatic capture, Layer switch and undo vertical slice;
6. Markers and Releases;
7. persistent conflicts and non-modal Weaves;
8. selectors and rewrites;
9. native sync/server;
10. multi-Worktree support;
11. product surfaces;
12. hardening, pruning and GC.

No later phase may compensate in UI, sync or server code for a missing core
invariant.
