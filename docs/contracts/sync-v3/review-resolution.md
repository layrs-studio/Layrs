# Phase 0 Sync V3 review resolution matrix

This matrix records the authoritative correction of the independent Sync V3
review. It is not a substitute for the normative invariants.

| # | Review finding | Resolution |
|---:|---|---|
| 1 | Sync Step contradicted canonical SCM Step | `transaction.schema.json` references the exact SCM Step and carries exact StepObservations separately. |
| 2 | `requestDigest` was self-referential | Invariants define domain-separated BLAKE3 over JCS of the complete semantic envelope with only `requestDigest` omitted and an explicit exclusion list. |
| 3 | Receive protocol was absent | `fetch-plan.schema.json` defines paginated GET planning, root/RepoPath proof, authorization binding and resume. |
| 4 | Merkle kinds could not all be transported | The catalog includes every SCM kind, RepositoryView, separate FileManifest, PolicySet and AuthorizationSnapshot. |
| 5 | Product payloads were untyped and unbound | `products.schema.json` defines discriminated versioned records, typed IDs, Step/product dependencies and CAS record binding. |
| 6 | `advance`/`integrate` semantics were ambiguous | ADR 0011 and invariants derive Layer diffs from expected/proposed RepositoryViews and define exact antichain results. |
| 7 | Layer create/metadata/lifecycle could not sync | Layer is a CAS-checked product with `previousProductObjectId`; typed definition events replay it. |
| 8 | Generic identifiers contradicted SCM typed IDs | Sync common definitions reference SCM typed IDs and add typed client/plan/event IDs. |
| 9 | Bootstrap Layer status contradicted SCM | Bootstrap wraps the exact SCM Layer record; no parallel `deleted` status exists. |
| 10 | Free-form events could not replay deterministically | `event-page.schema.json` is a closed discriminated union with state preconditions. |
| 11 | Cursor semantics under authorization filtering were undefined | Cursor binds principal/Space/authorization epoch/last scanned sequence; epoch change forces bootstrap. |
| 12 | A transaction could potentially self-authorize with a new Policy | Authorization is an immutable pre-product snapshot taken under transaction locks. |
| 13 | `unchanged` receipt contradicted generation rules | `no_op` has old=new and no ref event; every real mutation has new=old+1 and one event. |
| 14 | Duplicate identities/order were ambiguous | Normative uniqueness, topological order and canonical tie-break ordering are specified. |
| 15 | Upload partition/capability guarantees were incomplete | Upload plan is an exact partition with a capability bijection and strong principal/epoch/descriptor/size/compression binding. |
| 16 | Product roots were untyped | Product roots are `{kind, logicalId, objectId}` triples shared across bootstrap/events/fetch. |
| 17 | Bootstrap projections did not match canonical products | Layer/Marker/Release projections wrap exact canonical records plus their verified object IDs. |
| 18 | JSON Schema formats might be annotations only | Normative validation requires format assertion, RFC 3339 UTC normalization and absolute allow-listed URIs. |
| 19 | Self-host minimum lacked identity/admin bootstrap | ADR 0013 requires local auth, optional OIDC, one-shot admin secret, secret rotation and durable volumes. |
| 20 | DB/CAS backup consistency had no protocol | ADR 0013 defines CAS-before-ref, DB watermark, CAS snapshot at-or-after, reachability manifest and pre-serve restore fsck. |
| 21 | Wrong-case SCM common refs | All Sync aliases use canonical `$defs/ObjectId` and `$defs/RepoPath`; exhaustive resolution checks their JSON Pointers. |
| 22 | Layer heads had a second mutable authority | Sync CASes only the server current RepositoryView pointer/generation; head rows are rebuildable projections. |
| 23 | File identity was coupled to one manifest | Tree targets raw FileId; `file_manifest` is separate, selectable and many-to-one with mandatory reconstruction verification. |
| 24 | Policy admission implicitly activated policy | Domain-separated PolicySet plus explicit `expectedPolicySetId -> proposedPolicySetId` CAS separates admission from activation. |
| 25 | Authorization evidence was not canonical/auditable | Server installs a domain-separated AuthorizationSnapshot bound to subject, start PolicySet, epoch, request and action/scope decisions; audit/admin fetch only. |
| 26 | Product kind and logical ID could disagree | ProductRoot is a closed `oneOf` correlating every kind with its typed ID prefix; decoded CAS record must match all fields. |
| 27 | Product and Evidence CAS identities were under-specified | Product uses `layrs-product-v1` over JCS with only its claim omitted; Evidence uses `layrs-evidence-v1` over bounded opaque raw bytes. Golden vectors verify claims, kinds and exact wire canonicality. |
| 28 | `choose` named an ambiguous logical Decision | It now names the exact `decisionProductObjectId`; kind, typed identity, Space and Layer are independently verified. |
| 29 | Policy version expectations could be interpreted as a subset | The field is the exact sorted Policy Product object-ID set from the PolicySet locked at transaction start; any mismatch is stale authorization. |
| 30 | Rejected idempotent transactions and transport URLs were ambiguous | True semantic rejection persists a receipt, AuthorizationSnapshot and idempotency row without admissions/events. Request-digest and idempotency-digest mismatches are closed HTTP 409 `syncTransactionError` responses and never receipts; the original replay row remains unchanged. Transfer URLs use generic non-sensitive paths and header-only capability tokens. |

## Gate evidence

- Every Sync V3 schema and example parses as JSON.
- Every local and registered SCM `$ref` resolves to an existing definition.
- `verify-contracts.mjs` compiles all Sync/SCM dependencies with Ajv Draft
  2020-12 and validates every mapped fixture.
- Fixtures cover transaction/receipt, upload request/response, fetch
  request/response, bootstrap/event pagination and a canonical Layer product.
- `identity-vectors.json` covers empty/binary Evidence and a complete Product
  variant, plus wrong-claim, wrong-kind and non-JCS rejection cases.
- Changes are confined to ADR 0011–0013 and `docs/contracts/sync-v3`.
