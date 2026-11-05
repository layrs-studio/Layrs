# ADR 0012: Typed Space events, cursors and object authorization

Date: 2026-07-19

Status: accepted

## Context

Incremental sync needs a total order stable under concurrency, pagination and
authorization filtering. Timestamps and random event IDs are insufficient.
Layrs may physically deduplicate CAS bytes across a deployment while access is
defined by Workspace, Space, Layer, root and path policies.

Filtering an immutable Tree while retaining its digest creates a false
content-addressed object. Download-by-known-digest bypasses root reachability and
path authorization. The local Operation DAG is also not the server audit log: it
contains undoable local Views and operations that may never be published.

## Decision

### Typed event log and deterministic replay

Each Space has an append-only `SpaceEventLog` with a positive, strictly
increasing server sequence. Event kinds and payloads are the closed,
discriminated union in `event-page.schema.json`: accepted Steps/observations,
accepted products, Layer definition changes, RepositoryView pointer changes and
PolicyRoot changes. New
kinds require a schema/protocol revision; clients do not interpret free-form
payloads.

Events reference their transaction but never transport a local Operation. They
commit in the same PostgreSQL transaction as state and receipt. Replay is
idempotent and preconditioned: RepositoryView replacement checks old View ID and
pointer generation, and PolicyRoot replacement checks old PolicySet ID. A
mismatch forces rebootstrap, never last-write-wins.

### Authorization-stable cursors

A cursor is opaque and integrity-protected. It is bound to principal, Space,
authorization epoch and the last **scanned** Space sequence. The server scans in
sequence order and returns the subset visible under that fixed authorization
epoch. Advancing by scanned rather than delivered sequence prevents loops when a
page contains invisible events.

Any principal or authorization-epoch mismatch returns
`authorization_epoch_changed` and requires a new bootstrap. A bootstrap is a
single database-consistent authorized projection at a watermark; incremental
application starts strictly after its scanned sequence. Time is never the
primary incremental order.

The local Operation DAG remains entirely local. Server events cannot rewrite
local operation history and local undo cannot rewrite server audit history.
Workspace-only security/administration events remain in a separate Workspace
audit stream.

### Visible paths without false digests

In the first complete model, a Layer reader may observe its visible Step graph,
Tree structure and logical paths. File/content bytes remain path-policy
controlled. Strict path-name confidentiality requires a future encrypted or
opaque-subtree protocol. Layrs never returns a filtered Tree under the source
Tree ID. Bootstrap projections wrap exact canonical records plus their verified
object IDs instead of claiming a digest for reduced data.

### Capability-based fetch

There is no direct `GET object-by-digest` endpoint. A fetch claim supplies an
object, one visible root and the normalized RepoPath through which access is
requested. The server independently traverses the verified closure and evaluates
path policy. Each short-lived GET capability binds principal, authorization
epoch, Space, client, request, plan, exact claim, descriptor, method, expiry and
resume token. Revocation or epoch change invalidates it.

Fetch planning is paginated and resumable. Range transfer is transport-only:
full decoded length, kind and canonical digest are verified before admission.

Upload and download capabilities always target the generic paths
`/v3/transfers/uploads` and `/v3/transfers/downloads`. The URL contains no
digest, claim, principal, Space, plan or capability secret. The opaque token is
carried only in an authorization header; it binds all those values server-side.

Physical CAS values may be deployment-global. Logical availability, roots,
reachability, quotas and authorization are Space-scoped. Upload and fetch plans,
denials and normal response shapes do not reveal whether identical bytes exist
only in another Space or Workspace.

AuthorizationSnapshots are retained CAS audit evidence, not ordinary repository
roots. Only an explicit audit/administration fetch claim can obtain them.
Bootstrap and event pages expose the current PolicySet ID so replay can verify
PolicyRoot changes exactly.

### Product and Evidence object identity

Sync Products are exact canonical objects, not trusted metadata around another
payload. Their ID is BLAKE3 over `layrs-product-v1`, a NUL byte, and RFC 8785 JCS
of the complete Product envelope with only `productObjectId` omitted. Admission
recomputes that claim and correlates the discriminant, logical ID, record ID,
Space and exact typed dependency projections. The stored Product bytes must be
the full envelope in JCS form, so another JSON serialization is not an alias.

Evidence is deliberately opaque: its ID is BLAKE3 over `layrs-evidence-v1`, a
NUL byte and the exact raw bytes, with a 64 MiB decoded-object limit. Opacity is
not authority. Only a versioned typed Policy or Product can interpret an exact
Evidence ID; uploading, possessing or referencing bytes cannot authorize an
action or activate policy.

### Exact decisions, policy expectations and rejected replay

A `choose` intent names `decisionProductObjectId`, not a logical Decision ID.
Admission decodes that exact Product and verifies its decision discriminant,
typed logical/record identity, Space, Layer and chosen/discarded head sets.

RepositoryView mutation authorization names the exact sorted set of Policy
Product object IDs in the PolicySet locked at transaction start. Any difference
is stale authorization (`authorization_epoch_changed`), never a subset match.

Semantic rejection is a finalized auditable outcome. The server atomically
stores a rejected receipt, its AuthorizationSnapshot and the idempotency row,
while admitting no semantic objects, changing no pointer/root and emitting no
Space event. Replay returns the byte-identical receipt. Backend unavailability,
corruption and failed commit are operational errors and do not manufacture a
receipt.

## Consequences

- Receive pagination cannot miss same-time events or loop on invisible events.
- Event replay has closed, versioned semantics.
- An authorization change deliberately trades incremental continuity for a fresh
  trustworthy bootstrap.
- Local undo/redo remains local and server audit stays append-only.
- Tree/name visibility and byte visibility are explicit separate policies.
- Direct unauthorised digest GETs and false Tree projections are forbidden.
- Strict path-name confidentiality still requires a separate ADR.
