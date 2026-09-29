# UI navigation contracts: Space catalog, capabilities, whoami

Implemented in lot L0 of the UI redesign (`docs/design/redesign/03-delivery.md`).
The UI calls a Space a *project*. Examples use fake data only.

| Route | Purpose |
| --- | --- |
| `GET /v3/spaces?limit=&cursor=` | Spaces the caller may open, by name |
| `GET /v3/spaces/:space/capabilities` | Presentation hint: which actions to show for one Space |
| `GET /v3/auth/whoami` | Identity for the account page, plus `instance_administrator` |

Server: `crates/layrs-server/src/collaboration/catalog.rs`, store logic in
`crates/layrs-store-server/src/v3/collaboration/space_{catalog,capabilities}.rs`,
shared predicates in `crates/layrs-store-server/src/v3/space_access.rs`.
TypeScript wire types: `packages/ui/src/api/catalog-types.ts`.

## Authentication and caching

Both Space routes use the collaboration bearer convention: exactly one
`Authorization: Bearer <token>`; a missing, duplicated, expired or revoked token
answers `403 {"error":{"code":"invalid_capability"}}`. Every request is
evaluated against current database state; nothing is cached server-side and
responses carry `Cache-Control: no-store`. A revocation therefore applies to
the next request.

## Catalog: `GET /v3/spaces`

**Scope.** Candidates are the `ready` Spaces of Workspaces (same deployment)
where the caller holds an *active* Workspace membership. A candidate is listed
only if the injected policy authorizer allows `space.bootstrap`, the exact
decision behind `GET /v3/spaces/:space/bootstrap`. Access through a Team is a
policy grant like a direct grant, so it is covered by the same decision. A
Workspace membership alone never lists a Space; a revoked grant, an archived or
emptied Team, a suspended membership or principal hides it. Instance
administrators get no implicit listing of other owners' Workspaces.

**Query.** `limit` 1–100 (default 50); `cursor` from a previous page. Unknown
parameters are refused.

**Order.** `lower(name)` in the server database collation, then `spaceId`, so
homonyms have a stable order; display `workspace.name` to disambiguate.

```json
{
  "schemaVersion": "layrs.space-catalog.v1",
  "spaces": [
    {
      "spaceId": "space:4f1c…e2",
      "slug": "site-vitrine",
      "name": "Site vitrine",
      "workspace": { "workspaceId": "workspace:9a0b…17", "slug": "atelier-nord", "name": "Atelier Nord" },
      "workspaceRole": "member",
      "access": "full",
      "lastActivityAt": "2026-09-29T16:42:05Z"
    }
  ],
  "nextCursor": "q7VbX0…",
  "hasMore": true
}
```

- `access`: `full`, or `limited` when the authorizer marks the caller
  content-restricted (path-filtered projection; whole-Space collaboration
  endpoints refuse them).
- `workspaceRole`: the stored Workspace membership role (`owner`,
  `administrator`, `member`, `auditor`). Per-Space rights come from policy; use
  capabilities for actions. No finer "effective role" is stored server-side.
- `lastActivityAt`: `created_at` of the latest accepted repository event
  (sync transaction: published captures, Layer/View/policy-root changes), RFC 3339
  UTC, seconds precision. `null` when no event exists yet, and always `null` for
  `limited` access. Review, comment, run and membership activity is not included.

**Pagination.** `nextCursor` is present iff `hasMore`. Cursors are opaque and
sealed: the resume position may name a Space the caller cannot see, so it is
encrypted and authenticated (HMAC-SHA256 synthetic IV + keyed BLAKE3 keystream,
keys derived from `LAYRS_SIGNING_KEY`), bound to the caller's principal and to
this listing/order. A cursor used by another principal, altered, truncated or
longer than 2048 characters answers `400 invalid_cursor`. Cursors do not expire;
authorization is re-evaluated for every item on every page, so an old cursor
never reveals a Space that became hidden.

**Bounded work and short pages.** One request evaluates at most 256 candidates.
A page can therefore hold fewer than `limit` items, even zero, while
`hasMore=true`; keep following `nextCursor`. When `limit` items are found the
server confirms another *visible* item exists before answering `hasMore=true`,
so a full last page does not hint at hidden Spaces. No total or hidden count is
ever returned. The only residual signal is an empty/short page with
`hasMore=true`, which occurs only after 256 consecutive non-openable candidates
in the caller's Workspaces.

## Capabilities: `GET /v3/spaces/:space/capabilities`

A **hint for presentation only**. It can be stale as soon as it is returned, and
every mutation still performs its own server check. Each flag is computed with
the predicates the guarded handler uses (`authority`/`scoped_authority`
decisions, `space_access::{full_access, workspace_administrator,
human_principal, service_principal}`, `policy_management::managed`); those
handlers were refactored to call the same functions so the hint cannot drift.

```json
{
  "schemaVersion": "layrs.space-capabilities.v1",
  "spaceId": "space:4f1c…e2",
  "authorizationEpoch": 7,
  "access": "full",
  "capabilities": {
    "open": true, "browseFiles": true, "applyChanges": true,
    "readReviews": true, "proposeReviews": true, "decideReviews": true, "integrateReviews": true,
    "readScopedReviews": true, "proposeScopedReviews": true, "decideScopedReviews": true,
    "integrateScopedReviews": true,
    "manageMembers": false, "manageTeams": false, "manageInvitations": false,
    "managePolicy": false, "manageWebhooks": false, "configureProtection": false,
    "readReleases": true, "publishReleases": false, "revokeReleases": false,
    "readRuns": true, "runProofs": false
  }
}
```

"Full" below means the action is granted **and** the caller is not
content-restricted (the `authority()` guard of collaboration handlers);
"granted" means the Space-level decision allows it (restricted callers pass).

| Flag | Derived from | Guarded routes |
| --- | --- | --- |
| `open` | `space.bootstrap` granted (always `true` in a 200) | `GET …/bootstrap` |
| `browseFiles` | `ReadProjection` granted | `POST …/projection` |
| `applyChanges` | `transaction.apply` full | `POST …/transactions` |
| `readReviews` | `review.read` full | review list/detail, notifications, gate evaluation |
| `proposeReviews` | `review.write` full | propose, new version, comment |
| `decideReviews` | `review.approve` full and human principal | approve / request changes |
| `integrateReviews` | same as `applyChanges` | integration is a transaction |
| `readScopedReviews` | `review.read` granted | scoped review list/detail/status |
| `proposeScopedReviews` | `review.write` granted | scoped proposal/comment |
| `decideScopedReviews` | `review.approve` granted and human principal | scoped approve / request changes |
| `integrateScopedReviews` | `review.write` granted | `POST …/scoped-reviews/:review/integrate` |
| `manageMembers`, `manageTeams`, `manageInvitations` | `policy.administer` full, Workspace owner/administrator, managed policy root | members, Teams (+ eligible members), invitation create |
| `managePolicy` | `policy.administer` full, Workspace owner/administrator | policy-management, choose/policy transactions |
| `manageWebhooks`, `configureProtection` | `policy.administer` full | webhooks, Layer protection PUT |
| `readReleases` / `publishReleases` / `revokeReleases` | `release.read` / `release.publish` / `release.revoke` full | releases |
| `readRuns` | `space.bootstrap` full | run list/detail |
| `runProofs` | `proof.run` full and service principal | run register/begin/complete |

**Not represented** because they depend on the target object and cannot be
decided faithfully at Space level: per-path writes of `limited` callers (use
`POST /v3/spaces/:space/projection/access`, which returns `writable` per path),
paths of a scoped review candidate, closing/reopening a review or correcting a
comment (author only), review version freshness, Layer protection gates and
approval counts, trusted runner/recipe for runs, cancelling a run (any run for
policy administrators, own runs for runners), accepting an invitation
(recipient inbox), creating a project (`POST /v3/workspaces/provision`, owner of
the target Workspace). Instance administration is reported by `whoami`.

**Errors.** Unknown Space *and* Space the caller cannot open both answer
`404 {"error":{"code":"not_found"}}` (no existence oracle). A malformed Space id
answers `403 invalid_capability` (collaboration convention).

## whoami: `GET /v3/auth/whoami`

Unchanged snake_case response, one additive field:

```json
{ "principal_id": "principal:…", "actor_id": "actor:…", "display_name": "Camille Martin",
  "kind": "account", "instance_administrator": false }
```

`instance_administrator` uses `identity::is_instance_administrator`, the same
rule as every instance administration endpoint (`authenticated_in_tx(.., true)`:
accounts, service accounts, telemetry, storage budget); today it is
`kind == "bootstrap_admin"`. It is a UI hint; those endpoints recheck it under lock.

## Errors (both Space routes)

| Status | `error.code` | When |
| --- | --- | --- |
| 400 | `limit_exceeded` | `limit` outside 1–100 |
| 400 | `invalid_descriptor` | malformed query (non-integer `limit`, unknown parameter) |
| 400 | `invalid_cursor` | cursor altered, foreign to the caller, empty or too long |
| 403 | `invalid_capability` | missing/invalid bearer; malformed Space id |
| 404 | `not_found` | capabilities: Space missing or not openable |
| 409 | `authorization_epoch_changed` | policy changed during evaluation (catalog after one retry); retry |
| 503 | `backend_error` / `io_error` | database or CAS unavailable |

## Invalidation guidance for clients

- Refetch the catalog when the Projects page opens and after sign-in/out or a
  principal change; never treat recent local projects as proof of access.
- Refetch capabilities when entering a project, after any 403/404/409 from a
  mutation, and after membership/Team/policy edits (the epoch changes).
- Drop late responses that belong to a previous principal or project.

## Desktop transport

`native_remote` (Tauri) accepts two new `NativeRemoteAction`s, dispatched like
`whoami` and independent of the local checkout (`root` is ignored). They return
the JSON bodies above unchanged; failures are strings `"<code>: <detail>"`
(`remote_forbidden`, `not_found`, `invalid_cursor`, …).

```json
{ "origin": "https://layrs.example", "caPem": null,
  "action": { "action": "listSpaces", "limit": 50, "cursor": null } }
{ "origin": "https://layrs.example", "caPem": null,
  "action": { "action": "spaceCapabilities", "space": "space:4f1c…e2" } }
```

The client refuses `limit` outside 1–100 and cursors that are empty, longer than
2048 characters or not base64url before sending anything.

## Limits and versioning

- Catalog: ≤100 items and ≤256 authorizer evaluations per request; candidate
  queries fetch 64 rows at a time. Each evaluation reads the Space policy set.
- Capabilities: 14 authorizer decisions and a few indexed reads per request,
  plus the policy-management provenance read for Workspace administrators.
- `schemaVersion` changes on incompatible change. New fields and flags may be
  added; clients must ignore unknown fields and treat a missing flag as `false`.
