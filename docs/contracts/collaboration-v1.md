# Native collaboration API v1

Implementation in progress (T5/T6). This document defines the server-side contract;
it is not evidence that every endpoint/UI is already delivered.

Reviews use immutable canonical **Evidence** documents, referenced by rebuildable
PostgreSQL projections. They do not introduce an undeclared variant into the closed
Sync V3 Product union. CI results are Evidence plus existing Proof Products;
definitions/evaluations use existing GateDefinition/GateEvaluation Products.

## Binding and authority

A review version binds `spaceId`, `layerId`, `proposedStepId`,
`baseRepositoryViewId`, `policySetId` and `authorizationEpoch`. Its canonical
Evidence ObjectId is the proposal digest. Approvals and comments name that exact
version. A new version retains old comments/decisions and makes them obsolete;
no text anchor moves automatically. A changed target View or policy invalidates
the previous integration approval. States shown to users derive from immutable
decisions and the latest version.

All reads/writes recheck current membership and explicit policy permission.
The owner grants another Principal an explicit Space policy; service accounts
inherit no human permissions. Revoked Principals/members cannot supply a counted
approval or proof. Notifications are filtered by current Space access.

## Protection

Layer protection binds a versioned Evidence document and optional GateDefinition.
It specifies distinct human approval count, self-approval policy, unresolved
conflict policy, exact recipe and trusted runner Principals. The default permits
no self-approval. The final Sync V3 transaction rechecks these rules under its
authority locks, including direct API/CLI writes. A candidate Step must match
the approved proposal and proof exactly. No implicit administrator bypass exists.

## CI proof

A run binds one admitted Step, one recipe Evidence ObjectId, the authenticated
service Principal, captured policy, idempotency key and expiry. Pending/running/
completed/expired states differ from success/failure. A completion replay with
identical request bytes returns the original immutable result; contradictory
completion is rejected. The server derives the producer identity and permitted
trust from authentication/policy; a caller cannot choose its own trust level.

The server stores bounded logs as Evidence and generates a Proof Product naming
the exact Step, recipe, Evidence and policy versions. Gate evaluation checks the
subject, trusted runner, success, recipe, policy and freshness, and creates a
GateEvaluation Product. Repository code executes only in an external runner.

## API families

All request objects use camelCase and reject unknown fields. IDs/digests are
canonical native IDs; mutation idempotency keys must be 16–128 ASCII characters.

- `/v3/spaces/:space/members`: owner grants/revokes explicit per-Space access.
- `/v3/spaces/:space/reviews`: list/create proposals; version, comment and decision
  routes retain the immutable version context.
- `/v3/spaces/:space/layers/:layer/protection`: GET/PUT Layer protection;
  POST `/layers/:layer/gates/evaluate {stepId}` creates a GateEvaluation Product.
  Releases conservatively evaluate every protected Layer in the Space because
  a Release has no Layer identity.
- `/v3/spaces/:space/runs`: start exact-Step runs and submit bounded completion.
- `/v3/spaces/:space/notifications`: app-local, ordered, deduplicated notifications
  with a per-Principal read watermark.

Endpoint implementation and real integration proofs are tracked in journals T5/T6.

`GET /runs/:run` returns runId, stepId, recipeObjectId, runnerPrincipalId,
proofObjectId, outcome, expiresAt, completedAt, native product and evidence
(including the bounded log). Pending outcome is null; expiry never means success.
`POST /runs/:run/complete` accepts `{outcome: "passed" | "failed", log}`.

`POST /runs/:run/cancel` accepts `{idempotencyKey, reason}` (16–128 safe ASCII
key, nonempty reason at most 4096 UTF-8 bytes). The exact producer needs its
current `proof.run` capability; a current `policy.admin` holder may cancel any
run in that Space. No other automation or read-only/member role inherits this
permission. Identity, Space policy and run state are checked under the same
locks used by completion. Pending runs, including expired attempts without a
verdict, can be cancelled; completed attempts cannot.

The result is `{runId,status:"cancelled",cancellation:{evidenceObjectId,record}}`.
An immutable `layrs.run-cancellation.v1` Evidence records exact Step/recipe/run,
producer, actor, reason, timestamp and initial/current policy bindings. Migration
0011 provides an append-only run/Space projection; it does not rewrite an
existing proof or fabricate a failed outcome. Current run list/detail responses
include `status: running | expired | completed | cancelled` and nullable
`cancellation`. A cancelled run retains null `outcome`, `proofObjectId`, product
and `completedAt`.

Identical cancellation retries by the same currently authorized actor return
the same Evidence; a changed request fails. Completion and cancellation are
mutually exclusive under their shared transaction locks. After cancellation,
all result callbacks fail and cannot publish a Proof. This is a publication
stop, not remote process termination: an already-running external Docker
container remains governed by its runner timeout. Starting again requires a
new attempt/idempotency key. A cancellation emits one `proof.cancelled`
notification; it is distinct from `proof.completed`.

Notifications use `GET ?after=SEQUENCE` with nextAfter, hasMore and
lastReadSequence; `POST /notifications/read {lastSequence}` advances it monotonically.
Each notification also returns `reviewContext: {reviewId, version} | null`.
For `review.proposed`, `review.action`, `review.comment_corrected` and `review.integrated`, the version is
resolved from that notification's exact immutable Evidence object and matching
subject/Space, never from the review's latest-version projection. Other events
have no implicit review destination. Reading notifications and opening the
referenced review each require current Space access; a link is not a capability.
Clients opening a notification select that exact proposal version. Missing or
ambiguous historical data is an error, with no fallback to latest. Historical
proposals are read-only in the shared Web/Desktop surface, retain their original
Step/path anchors, and require an explicit action to open the current version.

## Audited comment corrections

`POST /reviews/:review/actions/:comment/corrections` accepts
`{idempotencyKey,version,expectedRevision,body,reason}`. Revision zero means the
original comment; a correction appends revision one and so forth. Only the
original author with current `review.write` permission may correct their text.
An administrator cannot edit somebody else's comment. Approval/request-change
decisions are excluded. A correction only replaces the displayed text: it
cannot change the original action, proposal version, reply, author, timestamp
or path/line anchor, and does not affect an integration receipt or gate decision.

The response is `{evidenceObjectId,record}` where the immutable
`layrs.review-comment-correction.v1` record includes the exact original action,
proposal, preceding correction Evidence, review/version, revision, author,
correctedAt, reason, body and current authorization context. Migration0012
stores an append-only projection; existing review actions are never overwritten.
Corrections of historical or integrated comments are permitted with current
rights and exact original version; that does not reopen or reapprove the review.
The historical notification view remains read-only: the current review view
can explicitly correct the author's older comment while preserving its version.

`expectedRevision` is compared under the Space lock. Two different corrections
against one revision cannot both apply. An identical retry by the same currently
authorized author returns the same Evidence, even when newer revisions exist;
changing the request while reusing its key is rejected. Text and reason must
be nonblank, without NUL, at most64Kio and4Kio UTF-8 respectively. Canonical
correction documents are bounded96Kio including JSON escaping. Limits are100
revisions per comment and1000 corrections/4Mio canonical documents per review;
read preflight enforces those same bounds before loading text.

`GET /reviews/:review` adds `commentCorrections:[{evidenceObjectId,record}]`;
`actions` still contains the original unmodified records. The shared UI displays
the newest text and an expandable audit history containing original text,
every revision, authors, timestamps and reasons. Broken rendering chains are
shown as unavailable rather than silently selecting a guessed current text.
Correction editor contents are held only in memory; existing local draft
persistence applies to new comments/replies, not this separate audit editor.
One `review.comment_corrected` notification is emitted per accepted revision;
its navigation context remains the original exact review version.
Webhooks live under `/v3/spaces/:space/webhooks`; see the
[signature, retry and receiver contract](../../examples/webhooks/README.md).
The [external runner](../../examples/ci/README.md) verifies an exact Step, executes
a pinned preloaded container without network or credentials, and saves a durable
completion for retry. Its initial recipe explicitly refuses conflicts and symlinks.

## Review pagination and decision projection (T5 completion, 20 September)

`GET /reviews?limit=1..100&cursor=TOKEN` returns
`{reviews:[{lifecycle,proposal,decisionState}],nextCursor,hasMore}`.
The cursor is opaque base64url, bounded and scoped to Space and authorization
 epoch. It anchors the immutable first-proposal notification sequence: reviews
created between pages are excluded, and the descending order cannot shift.
Proposal versions and lifecycle remain current when each page is read; this is
not a snapshot of historical review content. An epoch change returns409 and
requires restarting. Cursors select a range and confer no authorization; every
page rechecks membership and current policy. The previous `ListReviews` command
returns the first page; `ListReviewsPage {cursor,limit}` exposes continuation.

`decisionState` on list and detail is
`{state,approvals,blockingDecisions,requiredApprovals,stale,allowsIntegration:false}`.
States are pending, approved, changes_requested or stale. Only the latest human
decision per Principal for this exact version counts, with active membership,
self-approval rules and exact View/policy/epoch binding. At least one approval is
required for the descriptive approved label, even for an unprotected Layer.
This is an informational projection; it does not grant integration authority or
replace the final gate re-evaluation. Detail refuses histories exceeding10000
versions/actions instead of silently truncating them.

## Actual integration evidence

Migration0008 adds an append-only `review_integrations` table with foreign keys
to the exact review version and native Sync transaction. The runtime role has
SELECT/INSERT only. A generated canonical Evidence document with schemaVersion
`layrs.review-integration.v1` binds the proposal ObjectId, exact Step/Layer/Space,
transactionId, receiptDigest, previous/resulting RepositoryViews, server principal
and receipt timestamp. It is installed and referenced in the same PostgreSQL
transaction as the Applied receipt and head change, after authorization and
all required gates. Rejected transactions, receipt replay, reference-only writes,
unchanged heads, divergent outcomes, stale review bindings and matching a head
which was already present do not create integration events.

The hook recognizes an open latest review whose proposal becomes the new single
head of its own Layer and whose old View/policy/epoch exactly match that accepted
transaction. Equivalent matching proposals may all be recognized: this records
which content was integrated, not that each proposal independently supplied the
approval used by the gate. A maximum of1024 matching reviews bounds one change.

Detail returns `integration:null|{evidenceObjectId,record}`. The lifecycle projection
becomes integrated atomically and cannot be closed/reopened by a later action;
comments remain tied to their immutable version. New proposal versions require
an open review, so further work uses a new review. A `review.integrated`
notification is deduplicated by Evidence identity. There is no caller-controlled
"mark integrated" endpoint and no implicit policy bypass.

`GET /runs` also returns `createdAt`, `expiresAt`, and nullable `completedAt` in
RFC3339 UTC. A missing outcome after expiry remains expired, never a successful
Proof. Runs pagination and pagination of very long review details are separate
remaining work.

## Review lifecycle, local drafts and reconstruction (T5.C3)

The lifecycle and decision aggregate are separate. A proposal draft lives only in
bounded local storage, scoped to HTTPS origin, authenticated principal, Space and
exact new-review/existing-version target. Restoration and submission are explicit;
a draft never grants approval or exists as a published server review.

| State/transition | Authority and behavior |
|---|---|
| Draft → open | Explicit create publishes version1; exact retry returns that version. |
| Open → changes requested / approved | Immutable decisions for the exact version; latest decision per active human is counted against current protection. These are derived decision states. |
| Open → closed → open | Original author with current review.write; immutable close/reopen actions. Reopening preserves the same proposal and decisions, which still require current View/policy/epoch to count. |
| Open → next version | Author and exact expectedVersion CAS; old comments/decisions remain on their original version. Closed/integrated reviews refuse new versions. |
| Open → integrated | Exact successful native transaction records immutable integration Evidence atomically; matching a head alone does not integrate a review. |
| Integrated → closed/reopened | Refused. Comments and author corrections can remain append-only on their original version. |

Reconstruction uses the ordered immutable versions/actions/integration ledger and
its verified canonical Evidence. latest_version is the maximum recorded version;
lifecycle is initially open, then the latest ordered close/reopen, or terminal
integrated when its transactional Evidence exists. Decision aggregates are computed
at read time from exact-version decisions and current membership/protection/bindings.
The PostgreSQL maintenance drill damages only derived lifecycle/latest_version,
rebuilds them from that ledger, and checks identical full API detail for closed,
reopened and integrated reviews. This is not a public repair command and does not
claim reconstruction from an unordered CAS after losing the authoritative ledger.

When the target advances, the old proposal becomes stale. Recompute explicitly with
the client merge planner, publish the exact resulting Step, then propose a new
version. New approval and Proof for that result are required; an older approval or
Proof is never a universal permission. The composed PG drill executes the actual
structural merge planner, preserves a v1 line comment when the target deletes that
file, and integrates v2 only after its new exact approval and Proof. Automatic
recomputation in the review form and a semantic-conflict-specific review journey
are not claimed by this drill.
## Published history

Published history accepts an optional exact `path` (256 UTF-8 bytes, 128
components). A Step matches when that entry differs from at least one parent;
a root matches when the entry exists. The cursor binds the path as well as the
Space, identity, epoch and immutable anchor. Missing paths return no matches;
an inaccessible path/object rejects the page. Tree and Step reads are authorized
and bounded to 64 MiB combined; file bytes are not downloaded. This query does
not infer rename continuity or invent capture observations, dates or authors.

History additionally returns first-admission observations separately from Step
identity. `provenance=server_admission`, the authenticated publisher and the
server reception time describe publication, not authorship of the file contents.
Optional `filter.actorId`, `filter.since` and `filter.until` match the same
observation; date boundaries are inclusive RFC3339 instants. Steps predating this
feature remain visible without a filter and have no invented attribution.
Cursor v2 includes the exact filter and a pinned, validated observation snapshot;
its 4096-byte bound replaces the previous 1024-byte bound. Restart v1 cursors.
New admissions cannot alter that snapshot; corruption or disappearance rejects
the page. See [ADR0020](../decisions/0020-server-admission-observations.md).

## Run presentation and limits

The UI separates terminal outcome from lifecycle. A completed result requires both
passed/failed outcome and a Proof reference. Missing outcome never means failure.
An explicit server running status means an execution was declared and its result
is awaited; it is not remote-process supervision. A missing status with no result
is displayed as awaiting result, never an invented queued job. Unknown or
contradictory status/verdict/Proof fields require refresh rather than optimistic
running/success. Server expiry and the client's local deadline estimate are
labelled separately; cancelled does not assert that an external container stopped.
POST /runs/pending registers an attempt awaiting preparation (queued). Its
authorized producer calls POST /runs/:run/begin after fetching, verifying and
materializing the exact Step, before starting the container. The first start
timestamp is durable and repeated starts are idempotent. Cancellation, expiry,
completed attempts and stale policy epochs reject a new start. A pending attempt
cannot publish a Proof before begin. The legacy POST /runs keeps its immediate
running declaration without inventing a historical start timestamp. This is an
explicit producer lifecycle, not a hosted scheduler or remote-process supervision.
## Project-local teams

Teams are scoped to one Space, never implicitly to all projects in a Workspace.
Creation/replacement/archival require both policy.administer and an active Workspace
owner/administrator role. Members must be active human principals already holding
an active Workspace membership; Teams never admit or reactivate Workspace members.
Account/service management and invitations remain separate explicit operations.

GET/POST /v3/spaces/:space/teams list/create teams; GET /eligible-members returns
eligible principalId/displayName choices. PUT /:team replaces name, role and exact
membership with expectedVersion. POST /:team/archive takes expectedVersion and
reason. Every mutation requires an idempotencyKey. CREATE uses version0; existing
versions use compare-and-swap, conflicts require refresh. Roles are exactly member,
auditor and release_manager. Archived teams cannot be modified or reactivated.
GET /:team/history uses after,limit,anchor, returning immutable events with a fixed
anchorVersion and nextAfter/hasMore. Permissions are revalidated on every page.

Each version is canonical Evidence layrs.space-team.v1 with actor, timestamps,
previous event and PolicySet bindings. Migration0013 grants runtime INSERT/SELECT
only on that ledger; projections are reconstructible from its ordered versions.
Team grants expand into exact principals in an ordinary Policy Product; the current
PolicySet remains the authorization authority. Policy roots, epoch, audit event and
projections change in one SQL transaction. Empty/archived teams contribute no grant;
independent individual/team grants remain, and an explicit individual deny wins.
Previously issued capabilities and event cursors are invalidated by epoch changes.
team.changed notifications carry no review context. No token enters team records.

Bounds:32 teams per Space including archives;64 members/team;1000 versions/team;
128 UTF-8 bytes/name;32 total policy entries including existing individual grants;
100 events/page and1MiB aggregate history documents. Eligible choices are bounded
at256; oversize requests/results fail explicitly rather than silently truncating.
CAS bytes produced before a rolled-back transaction may remain as unadmitted
orphans; they convey no access. Exact retries return the original event and epoch.
Concurrent epoch changes can return409 and require a retry with refreshed state.

## Administrative head selection (choose)

The existing Sync V3 choose mode is implemented with exact View/generation and
captured-policy expectations. First supported authorization scope is deliberately
administrative: policy.administer AND active Workspace owner/administrator role.
Ordinary members and release managers receive no implicit discard permission.
Both transaction.apply and policy.administer appear in authorization evidence;
identity, membership role and captured roots are rechecked at commit.

The intent names decisionProductObjectId, an exact canonical Decision Product
(either included or previously admitted and verified through scoped CAS). Its
Space/Layer and chosen heads must match the intent. discardedHeadStepIds are
exactly those current heads not retained directly or through a chosen descendant.
The author must be the authenticated Actor; a nonempty reason up to4096bytes is
required. Existing decision objects are limited to1MiB each/8MiB aggregate reads.
All normal Layer protections apply, including refusal of empty protected heads.
A successful receipt carries outcome chosen and its exact Decision object ID.
Steps, contents and old Views remain retained and fetchable after selection.

NativeRemote.preview_choose returns a serializable NativeChoosePreview containing
origin/principal/actor, canonical expected View/generation, previous/chosen/discarded
heads and the complete immutable request. Callers persist it before sending and
retry that exact object after response loss; constructing a new preview is a new
user decision. apply_choose verifies account/origin, preview impact and receipt
bindings, and never silently refreshes stale expectations. Local Operations and
Worktree files remain untouched; receive is a separate explicit action.
The authenticated whoami response adds public actor_id alongside principal_id.

## Cooperative receive cancellation

`CloneCancellable` and `ReceiveCancellable` take a caller-generated `transferId`; `CancelTransfer` returns `cancelRequested`, `alreadyRequested`, `tooLate`, or `notFound`. The process registry is bounded to16 active entries, removed on scope exit and scoped to canonical server origin, canonical checkout root and a non-reversible session fingerprint. Cancellation is independent of the UI busy state. No bearer is stored in the registry.

Cancellation and the start of local commit compete through one atomic state transition. An accepted cancellation leaves the View, Operation and checkout unchanged and removes private download spools; once commit starts, cancellation returns `tooLate`. Cancellable receive requires a clean checkout before and after HTTP and validates the original binding/Operation/View again; it never captures pending edits implicitly. No native writer capability or publication journal lock is held during download. Checks occur between bounded reads; an outstanding HTTP read has a30-second timeout, so cancellation is cooperative rather than immediate socket termination. Legacy receive retains its documented capture behavior. Publication and CLI Ctrl+C are outside this contract; a publication whose response was lost must retain its durable retry intent.

## Publishing a rewritten history

`PublishCandidates` admits immutable local Steps and their closure using the ordinary durable publication journal, without a View or PolicyRoot mutation. A pending reference publication must be retried first. Candidate admission alone never substitutes a published head. After admission, an administrator previews and explicitly applies Choose against an exact remote View; the saved preview is the retry intent. Ordinary member admission rights do not grant administrative Choose. Old Steps, Markers and Releases remain immutable and retrievable. Existing clients retain their divergent local heads on receive; a fresh clone starts from the selected remote head.

## PolicyRoot administration and managed policies

GET `/v3/spaces/:space/policy-management` requires policy administration and an active Workspace owner/administrator role. It returns `{mode:"managed"|"custom",currentPolicySetId,managedPolicySetId}`. Authorized Teams list responses also include this object as `policyManagement`.

Custom roots use the existing whole-Space v1 rules, with actual enforcement at object transfer and collaboration boundaries. Unknown schemas/fields/actions and path scopes are refused. The root mutation requires the old authority, an exact expected root/epoch, and atomic installation with its receipt/event. Submitting an inactive Policy does not grant access. See [ADR 0022](../decisions/0022-policy-root-v1-and-managed-policy-provenance.md).

Managed membership, Teams and invitation grant APIs cannot replace a custom root: HTTP409 `custom_policy_root_active`. Returning to the exact `managedPolicySetId` through an authorized Sync transaction re-enables these APIs; equivalent-looking rules do not. No member list is inferred from arbitrary Policies.

Native client `preview_stored_policy(workspace, policySetId)` returns `NativePolicyPreview` with schema `layrs.policy-root-preview.v1`, origin, principalId, spaceId, worktreeId, storeRoot, expectedPolicySetId, expectedAuthorizationEpoch, proposedPolicySetId and the full digest-bound request. It never writes pending state. `apply_policy_preview(workspace, preview)` persists the scoped exact request before HTTP; uncertain outcomes remain pending, and recovery reuses the same preview without refreshing its epoch. Other accounts, Stores, Worktrees, origins or pending targets cannot adopt it. The response is a verified SyncTransactionReceipt, including explicit Rejected outcomes. Worktree files and local Operations remain unchanged. Receiving is separate.

CLI `remote policy-preview POLICY_SET_ID --output NEW_FILE` and `remote policy-apply PREVIEW_FILE` expose this explicit two-step flow; `remote policy-management` reads the current state. Preview files contain public identities/paths and canonical intent, never credentials.
