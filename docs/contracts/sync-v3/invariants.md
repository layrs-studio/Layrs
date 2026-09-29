# Layrs Sync V3 normative invariants

This document is normative for `layrs.sync.v3`. JSON Schemas constrain wire
shape; these invariants constrain canonical encoding, graph admission,
authorization, compare-and-swap behavior and replay.

## 1. Canonical identities and validation

1. Sync references the exact SCM V1 Step, StepObservation, RepositoryView,
   Layer, Marker, Release, Tree, FileId and FileManifest definitions. Parallel
   wire identity DTOs are forbidden.
2. Content IDs use `blake3:` plus 64 lowercase hexadecimal characters and equal
   the digest of the type's versioned canonical identity bytes.
3. Step observations remain separate from Step identity. Conflicts are reached
   through Trees; supersession remains an Operation relation.
4. The local Operation DAG is never synchronized. A StepObservation may carry
   its opaque origin Operation ID, but no Operation record, parent edge,
   RepositoryView transition or undo state is uploaded because of it.
5. Unknown schema/encoder versions fail closed. RFC 8785 JCS is used only where
   this contract explicitly requires it; SCM records keep their SCM encoders.
6. JSON Schema `format` is asserted. `date-time` is RFC 3339 with explicit offset
   and canonical UTC serialization; transfer URLs are absolute allow-listed
   URIs.
7. IDs are type-prefixed through Sync common or SCM common/core definitions.
   Product roots are a closed discriminated union: each kind fixes the legal
   logical-ID type/prefix and cannot be paired with another kind's ID.
8. Set arrays are duplicate-free and canonically sorted. Steps and products use
   stable dependency-topological order with canonical-ID tie-breaks.
9. A request cannot repeat Step, Observation, product logical/object, mutation,
   PolicyVersion or Layer mutation identities. Duplicate identity rejects the
   complete request.
10. Existing content IDs are accepted only when kind and canonical bytes match.
    Redaction/projection creates its own bytes and never reuses a source digest.
10a. A Product object is the UTF-8 JSON bytes of one `ProductEnvelope` in exact
    RFC 8785 JCS form. Its identity preimage is
    `UTF8("layrs-product-v1\0") || JCS(ProductEnvelope without productObjectId)`.
    Exactly the claim property `productObjectId` is omitted: `kind`,
    `productId`, both dependency arrays, every kind-specific envelope property,
    the complete versioned `record`, nulls and empty arrays all remain. The
    uploaded bytes themselves MUST equal JCS of the complete envelope including
    the verified claim; semantically equivalent non-JCS JSON is rejected.
10b. Product admission independently recomputes the ID and correlates all typed
    fields. `kind` selects exactly one envelope variant; `productId` equals the
    record identity as follows: Layer/Marker/Release/Proof/GateDefinition/
    GateEvaluation/Policy/Decision use respectively `layerId`, `markerId`,
    `releaseId`, `proofId`, `gateDefinitionId`, `gateEvaluationId`, `policyId`
    and `decisionId`; Weave uses `"weave:" + record.weaveId`. `record.spaceId`
    equals the transaction Space. Any mismatch is
    `immutable_object_mismatch`, never a second interpretation of the bytes.
10c. Product dependencies are exact, not hints. `dependsOnStepIds` is the sorted,
    duplicate-free set of direct Step IDs named by the typed record: Marker and
    Release use `targetStepId`; Decision uses the union of discarded and chosen
    heads; Weave uses source heads, target heads before, merge-base Step IDs and
    result Step; a Step immutable subject contributes its Step ID. The analogous
    `dependsOnProductIds` is the sorted, duplicate-free set of direct logical
    product references: a Layer's parents and non-null
    `previousProductObjectId` of versioned Layer, Policy and GateDefinition
    products, Marker/Release proofs, Decision Layer, a
    `productRoot` subject, and each record object-ID reference whose verified
    object kind is Product, resolved to that Product's `productId`. References
    to Evidence, recipes, policy rule bytes or other non-Product CAS objects do
    not become product dependencies. Both projections are recomputed after
    resolving the full request/store overlay; omission, surplus, ambiguity or
    a kind mismatch rejects atomically.
10d. Evidence is an opaque raw-byte CAS object, not a JSON record and not a
    Product envelope. For exact decoded bytes `B`, including an empty byte
    string, `EvidenceObjectId = blake3:hex(BLAKE3(UTF8("layrs-evidence-v1\0") || B))`
    and `canonicalByteLen = len(B)`. The length is limited to 67,108,864 bytes
    (64 MiB); encoded transfer size is separately capability-bounded and bounded
    decompression MUST be used. Servers do not normalize, parse or infer a MIME
    type while verifying Evidence identity.
10e. Evidence opaqueness grants no authority, validity or truth. Only a typed,
    admitted Policy/Product may interpret Evidence bytes, and it does so by
    naming the exact EvidenceObjectId plus its own versioned recipe/policy
    semantics. Possession, upload, reachability or a Proof reference alone
    cannot authorize an action or activate a policy.
10f. Product version cardinality is closed. Layer, Policy and GateDefinition are
    versioned logical products and therefore require the nullable identity field
    `previousProductObjectId`; every non-null predecessor resolves to the same
    kind, logical ID and Space. Marker, Release, Proof, GateEvaluation, Weave and
    Decision are singleton logical products: their logical ID admits exactly one
    Product object and their envelopes forbid `previousProductObjectId`.

## 2. Request digest and idempotency

11. The digest preimage is
    `UTF8("layrs.sync.v3/request-digest\0") || JCS(semanticEnvelope)`, where
    `semanticEnvelope` is the full transaction with only `requestDigest` omitted.
12. It includes protocol/type, Space, client, idempotency key, nullable known
    cursor, Steps, observations, products, RepositoryView mutation and PolicyRoot
    mutation. Null and absent are distinct.
13. HTTP framing, credentials, cookies, TLS data, compression framing, trace
    IDs, receive time, remote address and server diagnostics are excluded. No
    other transaction field is excluded.
14. The idempotency scope is `(spaceId, clientId, idempotencyKey)`. Same key and
    digest returns the exact stored receipt bytes after replay authorization.
    Another digest returns closed `syncTransactionError` code
    `idempotency_digest_mismatch` with HTTP 409, creates no receipt/snapshot/event
    or success row, and leaves the original idempotency row and receipt unchanged.
14a. A claimed `requestDigest` unequal to the canonical request digest is the
    closed pre-semantic `syncTransactionError` code `request_digest_mismatch`
    with HTTP 409. It is never a semantic rejection and creates no receipt.
14b. `knownEventCursor`, when non-null, is a signed opaque cursor bound to the
    authenticated principal, request Space and authorization epoch. It is an
    event-consistency hint only: it never participates in RepositoryView or
    PolicyRoot compare-and-swap and cannot make an otherwise valid CAS stale.

## 3. CAS catalog and upload planning

15. The transport catalog is exactly Step, RepositoryView, Chunk, FileManifest,
    Tree, Conflict, Symlink, Tombstone, Evidence, Product, PolicySet and
    AuthorizationSnapshot. FileId identifies reconstructed raw bytes and is not
    a manifest/object-upload kind.
16. Upload plan `present` and `missing` are an exact, disjoint partition of the
    requested full descriptors. Present means verified and Space-available, not
    merely physically deduplicated elsewhere.
17. Exactly one capability exists for each missing descriptor and none for a
    present/unrequested descriptor. It binds principal, authorization epoch,
    Space, client, request, plan, descriptor/digest, compression, both length
    bounds, method and expiry.
17a. Transfer URLs contain no digest, object ID, claim ID, principal, Space,
    request, plan, cursor or capability secret. Every upload capability uses the
    generic absolute path `/v3/transfers/uploads`; every download capability
    uses `/v3/transfers/downloads`. The opaque capability token is supplied in
    an authorization header, never in the URL path or query. Deployments vary
    only the HTTPS origin. Any loopback HTTP exception belongs to a local
    development adapter and is not a valid Sync V3 capability document.
18. Upload verifies encoded length, bounded decompression, canonical length,
    kind and digest before CAS installation. Upload alone changes no current
    View, PolicySet, product visibility or byte-read authorization.
19. Clients cannot upload AuthorizationSnapshots. The server constructs and
    installs those objects from its locked authorization evaluation. PolicySets
    may be uploaded but are inactive until a successful PolicyRoot CAS.

## 4. FileId and FileManifest transport

20. A Tree file target is the raw-content FileId: BLAKE3 over the exact
    reconstructed file bytes, independent of chunking or compression.
21. A FileManifest is a separate canonical CAS object with FileManifestId. It
    declares one FileId, byte length, chunker/version/parameters and ordered
    Chunk IDs/ranges. Its canonical encoder and domain come from SCM V1.
22. Multiple verified manifests may map to one FileId. Neither Tree nor Step
    identity selects a preferred manifest, and a new chunking strategy never
    changes FileId.
23. Manifest admission verifies ordering, complete non-overlapping coverage,
    chunk lengths and reconstruction. Concatenated/ranged chunk bytes must hash
    to the declared FileId. A mismatch is quarantined and rejected.
24. Step closure requires every Tree FileId to have at least one verified,
    Space-available manifest whose chunks are complete. GC may remove redundant
    unpinned manifests but never the last verified manifest for a reachable
    FileId.
25. A file fetch claim names FileId, root and RepoPath. The server chooses one
    verified manifest, preferring a requested manifest only when valid, and
    returns its manifest/chunk descriptors and capabilities. The client verifies
    manifest ID, chunks and reconstructed FileId before admission.

## 5. Fetch, reachability and audit objects

26. CAS bytes and canonical records are downloaded only through a fetch plan and
    GET capability. Direct digest GET endpoints are forbidden.
27. Normal object/file claims identify a visible root and normalized RepoPath;
    the server independently traverses closure and evaluates exact path policy.
    File, Chunk, Manifest, Conflict, Symlink and Tombstone bytes require the
    authorized file path.
28. AuthorizationSnapshot is fetchable only through its dedicated claim and an
    explicit `audit` or `administration` authorization. It is not reachable to a
    normal repository reader merely because its ID appears in a receipt.
29. Every GET capability binds principal, epoch, Space, client, request, plan,
    claim, descriptor/digest, method, expiry and resume token. Epoch change or
    revocation invalidates upload, fetch and cursor capabilities.
30. Pagination cursors bind the same principal/epoch/request/plan and last
    scanned claim. Resume verifies offsets and admits only a complete decoded
    object with correct kind, length and digest.
31. Planning shapes, denials and normal timing classes do not reveal physical
    existence in another Space or Workspace.

## 6. Steps, products and correlated roots

32. Steps are parent-before-child. Every parent exists in the Space or earlier in
    the request. Root Tree and full Merkle/FileManifest closure verify before
    Step acceptance.
33. Missing parent, Tree, conflict side, FileManifest or Chunk rejects the whole
    transaction. A conflicted Step remains valid and usable as a parent.
34. Observations reference existing/earlier Steps and never change Step ID.
    The native authenticated-client profile further requires the exact current
    Space and authenticated Actor, no Operation/Worktree/Device, empty metadata,
    a deterministic assertion ID, and separate client-claimed/server-received
    timestamps; see [client-observations](client-observations.md). Optional SCM
    fields allowed by the generic record schema do not bypass this profile.
35. Products are dependency-topological after Steps. Every declared Step/product
    dependency exists or was accepted earlier; cycles reject atomically.
36. The server decodes product CAS bytes and requires exact equality among
    envelope kind, typed logical ID, object ID, Space, immutable subject,
    dependencies and record fields. Independent `kind`/`logicalId` trust is
    forbidden.
37. Layer, Policy and GateDefinition definition changes use versioned Products
    with exact previous-object CAS. A Policy product's admission does not
    activate it. Marker, Release, Proof, GateEvaluation, Weave and Decision IDs
    are singleton identities and cannot be versioned or rebound.
38. Published Releases are immutable; Markers/Releases remain on their Step.
    Proofs/Gates name exact recipe, evidence, policy and dependency versions.

## 7. Authoritative RepositoryView pointer

39. Layer heads are authoritative only inside an immutable canonical
    RepositoryView. The server's sole mutable repository reference is
    `(currentRepositoryViewGeneration, currentRepositoryViewId)`.
40. Any Layer-head tables are disposable projections of the selected current
    View. They must be rebuildable and fsck-equivalent; they are never an
    independent CAS authority or last-write-wins source.
41. A transaction proposes one complete canonical RepositoryView inline or by
    the ID of an already verified Space-available View, plus
    `expectedCurrentViewId`, expected generation and one intent for every Layer
    whose head set differs between expected and proposed Views.
41a. An empty Layer-intent set explicitly requests a reference-only mutation
    (ADR 0015). Its proposed View is inline and differs from the expected View;
    every Layer head set stays identical. Exact current View ID and generation
    CAS is mandatory, including when only Marker/Release IDs are added. Stored
    proposals, hidden head changes, implicit reference removals and stale
    reference-only mutations reject. The immutable Product admissions, Release
    gates, View update and receipt remain one transaction.
42. Published/server RepositoryViews have `worktrees=[]`; local Worktree state
    and local Operations are not synchronized. Non-empty Worktree state is
    rejected at inline transaction validation, bootstrap validation, stored-View
    semantic admission and server CAS installation. The local SCM RepositoryView
    model still permits Worktree state. Proposed marker/release additions must
    correspond to accepted/existing products. V3 does not remove those IDs
    implicitly.
43. The server retains immutable `(generation, RepositoryViewId)` history needed
    by accepted stale integration. Expected View must equal history at expected
    generation. Every proposed View and deterministic result is fully verified
    and installed before pointer CAS.
44. Let `heads(V,L)` be Layer L's head antichain in View V. For every listed
    Layer, observed heads are derived from expected View and proposed heads from
    proposed View; clients do not submit duplicate head arrays. Undeclared Layer
    differences reject.
45. `advance` requires expected View/generation equal current, and every current
    head is ancestor-or-equal to a proposed head. Result for that Layer is
    `antichain(proposed)`.
46. `integrate` may be stale when expected View/generation matches history and
    each proposed head descends from an observed head. Result is
    `antichain(heads(current,L) ∪ heads(proposed,L))`; unrelated concurrent heads
    survive.
47. `choose` requires expected View/generation equal current, discard
    authorization and `decisionProductObjectId`, the exact verified Product
    object ID whose envelope kind is `decision`, whose typed logical ID equals
    its record `decisionId`, and whose record names the same Space and Layer.
    A logical Decision ID is insufficient and the former `decisionProductId`
    field is invalid. Let `C = heads(current,L)`, `H = heads(proposed,L)` and
    `ancestorOrEqual(a,b)` mean Step `a` is an ancestor of or equal to Step `b`.
    The Decision must satisfy `chosenHeadStepIds = H` and
    `discardedHeadStepIds = { c in C | no h in H satisfies
    ancestorOrEqual(c,h) }`. Both are canonical sets. Thus a current head that
    is retained directly or through a chosen descendant is never falsely
    recorded as discarded, while every actually discarded current head is
    recorded. Result is `antichain(H)` and may be empty. Only choose discards
    unrelated heads.
48. If any Layer intent uses `advance` or `choose`, the transaction's expected
    View must be current. A stale transaction must contain at least one intent,
    and every intent must be `integrate`.
49. The deterministic result View starts from current, applies each Layer result,
    unions valid marker/release additions, retains all unrelated current state and
    keeps `worktrees=[]`. It may differ from the proposed View under stale
    integration and receives its own canonical RepositoryViewId.
50. If result View ID equals current View ID, outcome is `no_op`: pointer
    generation is unchanged and no View event is emitted. Otherwise the server
    atomically updates both pointer ID and generation (`new=old+1`) and emits one
    `repository_view.changed` event listing per-Layer results and old/proposed/new
    View IDs.
50a. `expectedPolicyVersionObjectIds` is the canonical sorted, duplicate-free,
    exact set of `policyProductObjectId` values in the current PolicySet captured
    and locked at transaction start. Omission, surplus, substitution or stale
    ordering is not a subset match: it fails as stale authorization with
    `authorization_epoch_changed` before any semantic admission.
50b. For every LayerMutationResult, `divergenceCreated` is true if and only if
    `len(previousHeads) <= 1` and `len(resultingHeads) > 1`. It does not mean
    merely that divergence already existed, nor that one new head was added.

## 8. PolicySet and PolicyRoot CAS

51. PolicySet is canonical `layrs.policy-set.v1`. Its identity preimage is
    `UTF8("layrs.sync.v3/policy-set\0") || JCS(record without policySetId)`.
52. It contains Space ID and a canonically sorted, duplicate-free list of exact
    PolicyVersion IDs, Policy product object IDs and sorted dependency object
    IDs. Each logical `policyId` occurs exactly once: a PolicySet selects one
    version of a logical Policy, never several. All referenced
    products/dependencies verify before admission.
53. The sole active policy root is `currentPolicySetId`. A PolicyRootMutation is
    compare-and-swap from the pair `(expectedPolicySetId,
    expectedAuthorizationEpoch)` to a complete inline or existing
    `proposedPolicySetId` in the same final transaction. Both expected values
    are mandatory and must equal the values locked at transaction start.
54. Accepting a Policy/PolicySet alone never activates it. Equal old/proposed is
    a no-op with no event or epoch change. A real CAS increments the authorization
    epoch exactly once and emits `policy.root_changed` with old/new PolicySet and
    epoch; receipt, bootstrap and event page expose the resulting values.
55. Transaction authorization always uses the current PolicySet captured and
    locked at transaction start. A Policy or PolicyRoot mutation in that request
    cannot authorize itself or any sibling action.

## 9. AuthorizationSnapshot

56. Every finalized semantic transaction has a canonical
    `layrs.authorization-snapshot.v1` installed by the server. Its identity is
    `UTF8("layrs.sync.v3/authorization-snapshot\0") || JCS(record without authorizationSnapshotId)`.
57. The complete preimage contains Space, principal or canonical subject digest,
    start PolicySet ID, authorization epoch, request binding (client,
    idempotency key and request digest), sorted action/scope decisions, exact
    PolicyVersion IDs, allow/deny result/reason and evaluation timestamp.
58. The receipt references this snapshot ID. The snapshot is immutable,
    auditable, part of receipt/event retention and backup/fsck closure, but does
    not grant repository read access.
    Receipt `authorizationEpoch` is the evaluation/start epoch;
    `resultingAuthorizationEpoch` and `currentPolicySetId` are the values after
    any accepted PolicyRoot CAS.

## 10. Atomicity, events and replay

59. The final transaction locks identity/revocation, membership, authorization
    epoch, current PolicySet, policy dependencies, current View pointer/history,
    Layer definitions, idempotency and required CAS availability in canonical
    order.
60. Steps, observations, products, Views, PolicySets, AuthorizationSnapshot,
    pointer CAS results, policy decisions, typed Space events and receipt commit
    in one PostgreSQL transaction. Rejection exposes no partial Space state;
    prior unreachable uploads remain leased orphans.
60a. A well-formed request that reaches semantic evaluation always finalizes a
    stored receipt and server-created AuthorizationSnapshot, including policy,
    permission, gate, stale-CAS and other semantic rejections. The rejected
    receipt, snapshot and idempotency `(Space, client, key, requestDigest)` row
    commit atomically and byte-identical replay returns that stored receipt.
    Rejection admits no Step, observation, Product, View or PolicySet, changes no
    View/Policy pointer, and emits no Space event. Transport/framing failures,
    unavailable/corrupt backends, invariant corruption and failed database/CAS
    commits remain operational errors and produce neither a synthetic rejected
    receipt nor an idempotency success row.
60b. Idempotent replay first authenticates and then requires exact equality of
    authenticated principal, `clientId` and `spaceId` with the stored receipt.
    Current revocation is a hard refusal. If those checks pass, a changed current
    authorization epoch or PolicySet alone does not re-evaluate or rewrite the
    historical result: replay returns the byte-identical stored receipt.
60c. Receipt storage integrity uses exactly
    `BLAKE3(UTF8("layrs.sync.v3/receipt\0") || JCS(receipt))`. The JCS bytes are
    the complete closed `syncTransactionReceipt` including `storedAt`; no field
    is omitted. This digest is storage metadata only, is not a receipt identity,
    is not added to the wire receipt and grants no authorization.
61. Event kinds are the closed union in `event-page.schema.json`. Replay checks
    exact old/new View IDs and generation for `repository_view.changed`, and exact
    old/new PolicySet IDs for `policy.root_changed`. Mismatch forces rebootstrap.
62. Space sequences are positive, strictly increasing and unique. Every event
    cursor is authenticated/signed and binds principal, Space, authorization
    epoch and last scanned sequence. Signature or binding mismatch fails closed.
    Visibility filtering advances by scanned sequence, not last delivered event.
63. Epoch/principal mismatch returns `authorization_epoch_changed` and requires
    bootstrap. Bootstrap is one consistent projection containing exact current
    RepositoryView ID/record/generation and a literal `currentPolicySetId` equal
    to the embedded current PolicySet record ID.
64. First/last receipt sequences are both null iff no event. Otherwise all listed
    events belong to its transaction. View and PolicyRoot no-ops emit no
    corresponding event.

## 11. Visibility, hosting and backup

65. Layer readers may observe authorized graph/Tree/path structure; content bytes
    remain path-controlled. Strict name confidentiality requires a future opaque
    subtree protocol; filtered Trees never retain source IDs.
66. CAS may deduplicate deployment-wide while Space availability, manifests,
    roots, leases, quotas and authorization remain Space-scoped.
66a. One authoritative Sync V3 database references exactly one configured CAS
    namespace through a single immutable `store_id`. Startup and restore fail
    closed if the configured CAS `store_id` differs. Multi-CAS routing, implicit
    fallback and per-row store selection are outside V3.
67. V3 rejects V1/V2 and pre-baseline stores. Git is not a canonical identity,
    storage or required service.
68. Filesystem and S3 backends implement identical streaming, verification,
    lease, quota and fsck semantics. PostgreSQL `BYTEA` is not production chunks.
69. Backup uses CAS-before-reference admission, a transaction-consistent DB/event
    watermark, a CAS snapshot at or after it, and a manifest covering Steps,
    RepositoryViews, FileManifests/Chunks, products, PolicySets,
    AuthorizationSnapshots, receipts and every reachable object.
70. Restore verifies baseline, closure, kind, length, FileId reconstruction and
    digest before serving. Missing bytes fail closed; repair never invents them.
