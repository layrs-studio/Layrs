# ADR 0008: Worktrees, Markers and the new native store baseline

Date: 2026-07-19

Status: accepted

## Context

Layrs is still under development and does not need to preserve its development
store formats. Building the new source-control model on the current JSON
document projection would retain Layer-owned Steps, modal conflicts and a
single-checkout assumption.

Layrs also needs named important points without making every automatic
snapshot a commit, and it must eventually support several checkouts and agents
sharing one object store.

## Decision

### Worktrees

A `Worktree` is a local checkout with its own stable id and lifecycle. It is not
a Space, Layer, store or organization Workspace.

A Worktree explicitly references:

- the local Store that owns its metadata and CAS;
- the Space whose graph it materializes;
- a checkout root path and filesystem identity.

Its current active Layer, base Step, materialized Tree and state live in the
current RepositoryView. They are not fields on the Layer and are not inferred
from the path. Two Worktrees may use the same Store and Space while
materializing different Steps or the same Layer concurrently.

The Worktree record also owns exactly one `activeOperationId` after
initialization. The referenced Operation's resulting RepositoryView is the
unique current View for that Worktree. Moving this pointer and updating global
Operation heads is part of the same atomic transaction that accepts a new
Operation.

The checkout root is local metadata and is never part of a content-addressed
RepositoryView or synchronized Step. Secondary Worktrees locate their shared
Store through an explicit validated locator; directory ancestry and coincident
names are not authority.

### Markers and Releases

A `Marker` is an immutable named reference to exactly one Step. Supported core
kinds are `checkpoint`, `milestone`, `decision`, `baseline` and `custom`.
Marker notes, title, creator and proofs are metadata on or around the Marker,
never on the Step.

A `Release` is an immutable manifest and named reference to exactly one Step.
It contains the version, title, notes and proof bindings, but no mutable
publication state or publication timestamp. A correction creates a new Release
manifest.

Publication is represented by a separate immutable `ReleasePublication`
object/event referring to exactly one Release. It records publication actor,
device, time and optional originating Operation. Publishing never mutates the
Release manifest. Revocation is a separate auditable event and never mutates or
deletes either the Release or its publication record.

Marker and Release visibility is stored in RepositoryViews. Removing one from
the current View does not delete the object or its target Step. Visible Markers
and Releases, and retained historical Views that reference them, are garbage
collection roots.

### Merkle trees

Trees are strictly hierarchical. A TreeEntry name is exactly one normalized
path component, never an absolute path, `.`/`..`, an empty name or a string
containing a path separator or NUL.

Entries are unique by name and sorted by their UTF-8 byte representation before
canonical encoding. Each entry has exactly one typed target:

- logical FileId;
- child Tree;
- ConflictObject;
- SymlinkObject;
- TombstoneObject.

A Step points to one root Tree. ConflictObjects therefore participate in the
same Merkle closure and durability rules as files and subtrees.

Every canonical object id is domain-separated as
`BLAKE3(domain ASCII || NUL || canonical payload without the claimed id)`.
The V1 domains are `layrs-step-v1`, `layrs-tree-v1`, `layrs-conflict-v1`,
`layrs-symlink-v1`, `layrs-tombstone-v1`, `layrs-repository-view-v1`,
`layrs-chunk-v1`, `layrs-file-v1` and `layrs-file-manifest-v1`. Tree,
ConflictObject, SymlinkObject, TombstoneObject, RepositoryView and FileManifest
payloads use RFC 8785 canonical JSON. Their schema version remains in the
payload and only the claimed id property is removed. Set-like arrays must be
sorted before encoding; RFC 8785 object-key sorting does not replace this rule.

`FileId` is the logical byte identity
`BLAKE3("layrs-file-v1" || NUL || raw file bytes)` and is the id stored in a
Tree entry. A CAS `FileManifest` has a separate `manifestId`, computed over its
canonical manifest payload in domain `layrs-file-manifest-v1`; it contains the
`fileId`, byte length, chunking descriptor and ordered chunk slices. Several
manifests may represent the same FileId. Local store and sync metadata index
the `FileId -> {FileManifestId}` relation and choose only a completely
installed, verified manifest. A CAS key never denotes two different byte
encodings.

### New store baseline

Layrs introduces a new SQLite and CAS baseline for this model. It is not a
migration from the development JSON store or the previous SQLite projection.

Initialization rules are normative:

- a new Store is initialized only in an empty or explicitly prepared Layrs
  store location;
- a recognized previous format is refused with a precise unsupported-format
  error;
- old metadata and object bytes are never automatically deleted, moved,
  imported or rewritten;
- the previous `client_documents` source of truth and its projections do not
  exist in the new baseline;
- the baseline has a new format version and its first schema is installed as a
  single checksummed definition.

SQLite is the authority for identities, graph edges, Views, Operations,
Worktrees, Markers, recovery records, sync state and GC roots. The CAS is the
authority for immutable object bytes. An accepted Step/View/Operation
transaction may reference only CAS objects that have already been durably
installed and verified.

The old client object store and the durable store CAS must not coexist as two
write paths. The durable CAS in `layrs-store-local` becomes the only local
object installation path.

### Ownership

- `layrs-core` owns canonical domain types and identity rules.
- `layrs-graph` owns DAG traversal, selectors and rewrites.
- `layrs-store-local` owns CAS, SQLite, transactions, recovery, fsck and GC.
- `layrs-client-core` orchestrates workflows through typed store APIs and does
  not issue SQL directly.
- `layrs-sync` owns transport, batches and acknowledgements only.

### Garbage collection and retention

GC is conservative and mark-and-sweep. Roots include:

- current Operation heads and their resulting Views;
- retained historical Operations and Views;
- all Layer heads in rooted Views;
- visible Markers and Releases, and retained ReleasePublications;
- Worktree base and materialized states;
- pending sync objects and unacknowledged reference updates;
- Proof/Gate references;
- recovery journal records, leases and manual pins.

All reachable Step parents and full Merkle closures are marked. Newly installed
orphans receive a grace period. No object is swept while a shared session,
materialization lease, pending upload or fsck snapshot can still reference it.

Operation retention is explicit. Initially all Operations are retained. A
future prune operation may remove retention roots only after creating an audit
record, respecting Releases, pins and configured retention windows. Pruning an
Operation does not directly delete bytes; only a later successful GC may do so.

## Consequences

- Important points remain explicit without requiring commit-like Step names.
- Multi-Worktree support does not require copying the CAS.
- Existing development stores are intentionally incompatible and remain
  untouched for manual inspection or deletion.
- Sync cannot become the owner of object semantics.
- Retained undo history has an explicit storage cost.
- The previous JSON-to-SQLite migration work is not part of the target system.

## Rejected alternatives

- **A Marker field on Step:** would change or overload immutable snapshot
  identity.
- **One `.layrs` store per Worktree:** duplicates objects and obstructs parallel
  agents.
- **Automatic legacy import:** adds permanent compatibility constraints before
  the model is stable.
- **Flat full-path Trees:** weaken subtree sharing and create ambiguous path
  normalization.
- **Unbounded implicit GC roots:** make storage behavior impossible to explain
  or test.
