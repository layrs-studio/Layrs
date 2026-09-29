# ADR 0011: Sync V3 canonical transactions and RepositoryView CAS

Date: 2026-07-19

Status: accepted

## Context

Layrs synchronizes its native content-addressed graph without Git. V2 combines
Layer-owned Steps, independently mutable Layer heads and loose acknowledgements.
That permits last-write-wins and creates a second head authority outside the
immutable RepositoryView selected by local Operations.

## Decision

Layrs adopts incompatible `layrs.sync.v3`. Sync reuses exact SCM V1 canonical
records. Step observations are separate; the local Operation DAG is never
synchronized.

### Objects and files

Send uses exact upload partition, verified streaming upload, then one final
transaction. Receive uses paginated, resumable root/RepoPath-authorized GET
capabilities. The catalog includes Steps, RepositoryViews, Chunks,
FileManifests, Trees, Conflicts, Symlinks, Tombstones, products, PolicySets and
server-generated AuthorizationSnapshots.

Tree file targets are raw-byte FileIds. Chunk layout lives in separate canonical
FileManifests; multiple manifests may reconstruct one FileId. Manifest/chunks
must reconstruct and hash to FileId before a Step closure is accepted. Fetch by
FileId lets the server select a verified manifest and returns that choice plus
chunk capabilities.

### Authoritative repository state

Layer heads are authoritative only inside a complete immutable RepositoryView.
The server has one mutable pointer:

`(currentRepositoryViewGeneration, currentRepositoryViewId)`.

Head tables are disposable, rebuildable projections. A Sync transaction proposes
a complete RepositoryView inline or by verified ID, the expected current View ID
and generation, and one mode for each Layer head-set diff. Published Views have
no local Worktree state; Operations/undo never cross the wire.

- `advance` requires exact current View/generation and descendant coverage.
- `integrate` may use retained historical expected View/generation and produces
  `antichain(current ∪ proposed)` for each changed Layer.
- `choose` requires exact current View/generation plus an authorized matching
  Decision product and is the only discard operation.

The server constructs and hashes the deterministic full result View. No-op keeps
pointer generation and emits no View event. A real change atomically changes
View ID and increments pointer generation once, with one typed event containing
old/proposed/new View IDs and all per-Layer results.

### Products, policies and authorization

Product roots are a closed union correlating kind, typed logical ID and object
ID. The server decodes CAS bytes and compares all three plus dependencies to the
record. A Policy product is inert until a separate PolicyRoot CAS.

PolicySet is a domain-separated canonical sorted set of PolicyVersion IDs,
product objects and dependencies. `PolicyRootMutation` atomically compares
`expectedPolicySetId` and installs `proposedPolicySetId`; equal is a no-op, while
a real change increments the authorization epoch and invalidates old cursors and
capabilities.

Authorization uses the PolicySet at transaction start. The server installs a
domain-separated canonical AuthorizationSnapshot containing Space,
principal/subject, starting PolicySet, epoch, request binding and exact
action/scope decisions. The receipt references it; only audit/admin capability
may fetch it.

### Atomicity and idempotency

`requestDigest` is domain-separated BLAKE3 over JCS of the entire semantic
envelope with only itself omitted. Idempotency scope is
`(spaceId, clientId, idempotencyKey)`. Steps, observations, products, Views,
PolicySets, AuthorizationSnapshot, both root CAS results, events and receipt
commit in one PostgreSQL transaction under a locked pre-request authorization
snapshot.

## Consequences

- No Layer-head authority exists outside RepositoryView.
- Concurrent integration preserves unrelated heads without synchronizing local
  Operations or Worktrees.
- File identity survives chunker changes.
- Policy admission and policy activation are separate auditable facts.
- Authorization decisions are immutable retained evidence.
- V1/V2 clients/stores fail explicitly; Git is neither storage nor transport.
