# ADR 0016 — Immutable Release publication lifecycle evidence

Date: 2026-09-20. Status: accepted; additive implementation of ADR 0008.

`Release` remains an immutable manifest and `ReleasePublication` retains its
existing schema. Publication and revocation are separate, ordered
`layrs.release-lifecycle-event.v1` records carried as immutable V3 Evidence.
Their object ID is BLAKE3(`layrs-evidence-v1` + NUL + canonical JCS bytes).

Every event binds Space, Release ID, exact Release Product object, target Step,
previous event object, authenticated Actor ID and server timestamp. A published
event embeds the existing ReleasePublication record; a revoked event identifies
the exact active publication and a required bounded reason. Actor/time bindings
are checked when decoding. The server additionally verifies the admitted Product,
Step, Evidence bytes and their PostgreSQL projections in the selected Space.

There is at most one active publication per Release. State is derived from the
append-only event chain: publish → revoke → optional explicit publish. A new
publication has a new identity. Revoking does not delete or rewrite the manifest,
Step, earlier publication or Evidence. Evidence dependency edges retain the
manifest and previous event. Local Operation IDs and device identities are not
invented by the server; their optional publication fields remain null.

Writes require a request ID and exact expected last event ID (null for an empty
chain). Replay is bound to principal, Space, Release, request ID and request
digest; it returns the original event even after later lifecycle changes.
Different requests against stale state fail with `stale_release_state` (409).
The Space row serializes publication and policy/protection changes. Current
identity, membership and authorization epoch are rechecked under locks.

`release.read`, `release.publish` and `release.revoke` are distinct policy actions.
New Space owners receive all three. Members, auditors and automation receive
read access only. The explicit `release_manager` Space role grants all three.
Existing policies do not gain privileges silently: an authorized administrator
can grant this role; an owner may explicitly grant it to themselves without
changing their Workspace owner membership. Existing deny rules still win.

Publication requires a currently visible manifest and rechecks all applicable
Space Layer protections against its exact Step, current policy and active
reviewers/runners. Missing/stale Proofs or reviews yield `release_gate_rejected`.
Revocation needs its own authorization and exact publication CAS, and remains
possible if publication proofs or review criteria subsequently stop passing.

Migration `0007_v3_release_publications.sql` adds a projection outside the frozen
native catalog; runtime receives SELECT/INSERT only. No baseline rewrite or
mutation of old Release schemas occurs. Lists are bounded to 1,024 Releases and
4 MiB; each Release history is bounded to 1,024 events of at most 16 KiB each.
History currently uses a complete bounded response, not pagination. Events are
available through the authenticated lifecycle API; this does not claim offline
replication into native local Operations or local ReleasePublication tables.

Routes: GET `/v3/spaces/{space}/releases`; GET/POST
`/v3/spaces/{space}/releases/{release}/publications`; POST
`.../publications/{publication}/revoke`. Read responses expose `events`,
`lastEventObjectId` and derived `activePublicationId`. Writes return one
`{eventObjectId,event}` entry. The client exposes corresponding typed
`CollaborationCommand` variants shared by CLI/Desktop.
