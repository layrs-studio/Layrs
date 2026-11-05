# ADR 0007: Repository Views own Layer heads and Operations form a local DAG

Date: 2026-07-19

Status: accepted

## Context

Layrs needs automatic capture, undo and redo, concurrent Worktrees, offline
devices, graph rewrites and divergences that never lose work. A single mutable
head per Layer and a technical recovery journal cannot provide those
properties.

The previous schema stored a Step's Layer, one mutable ref per Layer, one
active timeline, and at most one non-terminal Weave per Space. Those
constraints serialize unrelated work and make concurrent updates susceptible
to rejection or overwrite.

## Decision

### Layers

A Layer is a stable named object containing identity, Space membership,
presentation state, settings and optional workflow relationships. It contains
no Step and no head field.

Every Layer head is stored only as part of an immutable `RepositoryView`. A
Layer maps to a sorted set of zero or more Step ids:

- zero heads means the Layer has no captured snapshot;
- one head is the ordinary case;
- more than one head is an explicit divergence.

Every Layer head set is an antichain: no head may be an ancestor of another
head in the same set. Ancestor heads are redundant and must be removed while
constructing the candidate View, before hashing and acceptance. This
normalization is deterministic and is not conflict resolution because the
descendant already preserves the ancestor's history.

No operation may silently reduce a multi-head set to one head. Resolving a
divergence requires an explicit graph operation, Weave or user choice recorded
as an Operation.

Layer parent/child relationships describe workflow lineage only. They do not
imply Step ownership, Step copying, automatic inheritance or first-parent
semantics.

### Repository Views

A `RepositoryView` is an immutable snapshot of all mutable local repository
references relevant to undo:

- the head set for every visible Layer;
- visible Marker and Release ids;
- the active Layer, base Step, materialized Tree and status of each Worktree.

Entries are canonically sorted. A View id is the BLAKE3 digest of its versioned
canonical encoding. A View never contains Worktree paths, server credentials,
network cursors or mutable caches.

Each Worktree record contains exactly one `activeOperationId` after repository
initialization. That Operation selects exactly one resulting RepositoryView,
which is the Worktree's current View. The pointer is local mutable metadata and
is not embedded in the content-addressed View, avoiding an identity cycle.

There is no second authoritative `layer_heads` table. SQL tables optimized for
current-head queries are projections of the current View and must be
rebuildable and fsck-verifiable.

### Operations

Every accepted local mutation that changes a RepositoryView creates one
immutable `Operation` with:

- one or more parent Operation ids, or none for repository initialization;
- exactly one resulting RepositoryView id;
- a typed operation kind and human-readable summary;
- an optional rewrite map from old Step ids to replacement Step ids;
- local actor/device/time observations.

Operations form a DAG. The local repository maintains a non-empty set of
global current Operation heads after initialization. Different Worktrees may
have different active Operations, and concurrent histories therefore create
several global heads. Operation parent ordering has no semantic meaning and is
canonicalized.

Integration is never a pointer shortcut. It creates one new Operation whose
parents are all integrated Operation heads and whose resulting View contains
the integrated state. In one atomic transaction Layrs inserts that Operation
and View, advances the affected Worktree `activeOperationId` pointers, and
replaces the integrated global heads with the new head.

`undo`, `redo`, `restore` and `revert` never delete or mutate prior Operations:

- `undo` creates a new Operation whose resulting View restores a selected
  prior View;
- `redo` creates a new Operation restoring the View selected from an undone
  Operation;
- `restore` selects an explicitly named historical View;
- `revert` computes and records an inverse repository mutation against the
  current View.

An Operation may reuse an existing RepositoryView. This is expected for undo
and restore.

### Three distinct logs

The following logs are separate types, tables and APIs:

1. the **local Operation DAG**, which is user-visible and enables undo;
2. the **recovery journal**, which records incomplete filesystem, metadata and
   network side effects;
3. the **server Space event log**, which is an append-only shared audit of
   accepted server actions.

A recovery record may refer to an Operation id, and a server event may record
an originating Operation id, but neither log is the Operation DAG and neither
is used as its storage.

## Concurrency

Filesystem scanning, hashing, Lens evaluation, CAS installation and network IO
must not hold a long-lived exclusive metadata lock. They prepare immutable
inputs first. A short transaction then compares the expected current Operation
head set and either:

- atomically installs the new Step/View/Operation and advances the head set;
- or preserves both histories and requests/inserts an explicit integration
  Operation.

Last-write-wins is forbidden for Layer heads, Markers and Worktree state.

## Consequences

- Layer switching, capture, Weave, resolution and rewrite are fully undoable.
- Multiple Worktrees and devices may expose multiple Layer heads without data
  loss.
- Every Worktree has one unambiguous current View even while the repository has
  several global Operation heads.
- A Step can be reached by any number of Layers and Views.
- Current state is derived from Operation heads, not from timestamps.
- Technical crash recovery remains independent of user-visible history.
- Sync transports Steps and reference mutations; it does not overwrite a
  generation mismatch.

## Rejected alternatives

- **One mutable head per Layer:** cannot faithfully represent divergence.
- **A Layer-owned timeline:** duplicates the repository graph and prevents
  cross-Layer reuse.
- **Reusing `operation_journal`:** technical states such as `applying` and
  `needs_repair` are not user history.
- **In-place undo:** destroys auditability and invalidates concurrent readers.
- **Holding one exclusive session for an API call:** blocks background capture,
  parallel Worktrees and slow-network safety.
