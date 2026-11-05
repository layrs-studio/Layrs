# Imported restore history and multi-segment publications

27 October 2026. V2 codec, bounded SQL DATA and fresh restore frontier qualified
on r23; **physical imported inverse and schema9 re-export still outstanding**.
This remains part of the Worktrees / partial VFS programme, alongside re-export
of admitted schema9 destinations. A local inverse on schema9 is already a
different, qualified path; it does not prove an imported ancestor inverse.

Latest source milestone: r27 qualifies the genuine ordinary owner, immutable
current image, independent selected bases, retained history streams and detached
plans. All 17 released tests pass; source transfer itself does not allocate or
admit C. See the [owner contract](worktrees-partial-vfs-ordinary-export-owner.md)
and [remaining physical driver integration](worktrees-partial-vfs-ordinary-export-driver.md).

## Transport gap identified before r23

`restore::targets(Inverse)` loads `engine::history_with_scope` and takes the
historical preview's `before`. If the associated capture belongs to another
identity, history previously used `conversion::provenance` to recover the source
Marker and compare the receipt's checkout identity. That legacy interpreter
still refuses schema9; the new typed route is described below.

Replacing only that call would not establish legitimate imported history:
`provider/import_validation/source.rs::require_quiescent_history` previously
forbade `projection_restore_history`. The twelve v1 committed rowsets did not
represent or copy restore receipts. V2 adds this DATA transport; the genuine
ordinary-source physical driver needed to deliver an ancestor receipt remains
outstanding.

Provider origins carry `ImportSource::Provider.binding.recovery`, including the
original identity and checkout identity, with activation bindings checked by
the scoped readers. Ordinary v1 origins carry identity and `format_digest` but no
recoverable Marker or checkout identity. R23 adds both the receipt rowset and the
ordinary checkout commitment with authenticated source-reference comparison,
rather than destination SQL injection or a lookup by matching identity alone.

## Required proof chain

1. Resolve the genuine **incoming** admitted prefix and receipt through
   `read_admitted_import`; an arbitrary outgoing transition is not authority.
2. Use the associated capture sequence to select its provenance segment and
   enforce the segment's lower and upper generation bounds.
3. Authenticate the canonical restore row (`operation_id`, capture sequence and
   payload) as part of that segment's committed historical closure.
4. Match receipt destination identity and checkout identity to the authenticated
   origin of that segment. Verify an ordinary origin's new checkout commitment
   against its actual source Marker when exporting it.
5. Preserve capture/snapshot/digest/permission and `after` consistency checks.
   Exact mappings and protections must cover **both `before` and `after`**;
   restore-only preimages may be absent from transported captures/companions.
6. Build a new inverse preview bound to the current checkout. Preserve the
   ancestor receipt unchanged and continue refusing its direct replay there.

## Implementation ownership and dependencies

- **Versioned transport:** `provider/records/import.rs` and
  `import_validation/{rows,source,scoped,ancestry,copy}` need a committed restore
  rowset and origin checkout evidence. Retain decoding/validation of old
  prefixes, which cannot authorize history they never committed. Copy exact
  canonical rows with collision checks and validate capture/snapshot references
  and every ancestor closure.
- **Content closure:** source mapping derivation and the protected union must
  include all exact restore preimage/result representations. Preserve identities
  when the same FileId has more than one representation.
- **Typed resolution:** extend `ProvenanceIndex` and the admitted-origin adapter
  with detached `RestoreHistoryOrigin` evidence identifying the interval,
  source identity/checkout and the committing segment/receipt. Use the existing
  snapshot discipline and admission reader; no fallback to legacy conversion.
- **Consumption:** `restore/engine.rs` separates receipt interpretation from
  origin/content validation and routes schema9 through that typed evidence.
  Preserve legacy inverse behavior and destination validation at application.
- **Re-export9:** accepting a schema number alone is insufficient. Source
  references, schema checks, ancestry, copy and incoming-admission anchoring all
  need the real schema9 contract. Do not relabel it as schema8.

## Independent publication qualification

The positive multi-segment publication reader can be qualified separately from
restore transport. Existing flow is `conversion::imported_publication_record`
through `admitted_ordinary::imported_publication`, segment resolution and
`completed_record_for_origin`. Generation lower bounds are already checked.
Current admitted tests mainly cover missing records/refusals; `two_imports.rs`
does not establish nonempty inherited publications in several segments.

Build authentic nonempty publications in at least two committed segments and
read them with their original identities after admission and reopening.
Imported captures remain non-publishable through `begin_publication`; ready
destination captures and the later local suffix use their ordinary path.

## Required acceptance evidence

- Authentic multi-segment chain with sources unavailable: inverse of an older
  and a newer imported receipt, restore-only preimage retained, new receipt
  bound to the current destination, ancestor replay refused.
- Positive publications from several segments and correct origin after reopen;
  imported captures cannot be republished, local suffix remains usable.
- Refusals without effects for a receipt outside its committed closure, correct
  identity with wrong checkout, wrong segment or interval, substituted capture,
  absent preimage/mapping, operation ID collision, unrelated ancestor or outgoing
  receipt used as incoming authority.
- Preserve old prefix codecs/closures, legacy inverses and local schema9
  inverses. Keep unsupported re-export paths explicitly refused until integrated.

## Current implementation and append stability

The working tree adds prefix v2 with a distinct digest domain, an optional-on-wire
restore-history rowset that is mandatory in v2, and an ordinary-origin checkout
identity compared against the source Marker. V1 omits both extensions and keeps
its original bytes/domain. V1 never authorizes restore rows, even when a provider
binding already carries a checkout identity. Source schema9 remains refused until
the separate genuine re-export contract is implemented.

V2 normalizes `operation_id,capture_sequence,payload` in binary operation-ID order,
scoped by the prefix's capture frontier. The source, historical ancestor, copied
destination and admitted-origin readers must all check that same scope. Preimages
and results contribute their FileId/length requirements before deriving exact
content mappings; each ancestor uses only its own historical frontier.

A newly completed restore now requires a fresh capture sequence, even when its
changes equal the previous capture. Ordinary capture deduplication and content
deduplication remain unchanged. Existing-operation replay returns its old receipt.
This is necessary: without a fresh sequence, a later restore could add a row below
an already committed frontier. Historical receipts that already share a sequence
remain valid; restore-row count must not be constrained by capture count.

This append-stability guarantee is a current-writer contract. Concurrent use of
older writers against a schema6/7 source is not qualified: those writers could
still reuse a capture sequence. Supporting such downgrade coexistence would need
explicit historical membership or a durable ordinal, not an inferred SQL rowid or
lexicographic operation timestamp.

All-targets checks and the final test build pass. Independent review closed the
local/imported dispatch finding: schema9 uses its admitted capture boundary,
not identity inequality, since an ancestor can share the current logical identity.
All 11 released r23 recipes pass: seven codecs, three SQL-memory metadata tests,
and the real admitted fresh-sequence/replay/reopen/local-inverse recipe (260.31 s
child). The SQL tests use detached synthetic metadata and mappings, not physical
content authority. No successful inherited inverse, physical re-export9 or complete
multi-segment publication is claimed. Exact evidence is in the checkpoint and
`imported-history-evidence-20261027-r23.tar.gz` portable supplement.

## Next physical integration: genuine admitted ordinary export

The current source adapter and producer are implemented: the ordinary source9
binding commits the actual `ManagedAdmissionRecord::digest()`, incoming prefix,
incoming receipt and private database identity. `plan_admitted_import` requires
the retained source transaction and independent effective-image digest, derives
the exact closure, then returns only the result of complete source validation.
The distinct historical anchor preserves the difference between the incoming
admission and the outgoing prefix. Historical schema9 in a copied database is
interpreted from committed ancestry DATA, without reopening it as a live source9.
This producer alone is not a physical export owner or a destination admission.

Independent review found a transport omission in the previous restore-history
increment: the v2 rowset reader had a thirteenth RestoreHistory spec, but the
copy insertion order visited only the original twelve. The exact final check
therefore refused a populated restore import after partial transaction writes.
The corrected copy visits optional RestoreHistory immediately after Captures,
while retaining collision checks for every rowset before any insert. Its SQL
regression exercises normal source verification and complete copying, exact
retry, a late restore collision while earlier rows need insertion, savepoint
and transaction rollback, and the legacy twelve-rowset route. Runtime results
are recorded in the main checkpoint; this source description does not infer
physical B-to-C delivery from SQL copying.

Source investigation confirms that the current managed planner/allocation/staging
drivers are reached by native fixtures, not an orchestrated public export API.
`ProviderActiveSession` is not an appropriate owner for an admitted ordinary
source. Add an ordinary managed-export holder entered from `ProjectionStore`,
owning its genuine `format::Access`, rather than synthesizing provider activation.

Required dependency order:

1. Add a source9 DATA adapter using `read_admitted_import`, anchored on its actual
   incoming receipt. Outgoing transfers must not become incoming ancestry. Update
   source/schema/ancestry/copy interpretation without labelling a source9 as 8 or
   interpreting the destination8 using the source9 physical schema.
2. Capture an ordinary export image under `Access::with_content`: current snapshot,
   selection and scan, pinned checkout files, verified manifests/chunks, repeated
   image validation before effects and during copying. Retain exact historical
   private mappings through scoped manifest/stream readers. No `ContentScope`
   escapes its callback; the holder keeps Access and reconstructs scopes.
3. Extract the common planner from the provider-only Active/initial/activation
   assumptions. Provider checks remain on its adapter; the ordinary adapter supplies
   its authenticated observation, placement, files/deletions and historical union.
4. Add ordinary source-intent validation and physical allocation through the same
   planned/copying/staging/ready/published protocol. Current SQL and parent/root
   ownership paths depend on A/P/activation and require genuine ordinary equivalents.
5. Extend family import authority, content production, Ready inputs and autonomous
   destination-prefix reconstruction. B's admitted lease authorizes B only; C's
   reservation/protection must still use dedicated import authority and exact C pins.
6. Extend publication handoff and C admission. Ordinary v2's checkout identity alone
   cannot replace the source Provider recovery binding: the handoff also requires
   the actual source database identity and an authenticated incoming-admission link.
   Add a dedicated admitted-source reference/variant instead of relaxing equality.

The existing real-admission fixture provides B9 after all A holders have closed.
Modify/capture/restore B through production ordinary APIs, then export B to C using
the new driver. Acceptance must reach C9 reopened with A and B unavailable, including
inherited history and unchanged B admission; stopping at copied C8 would miss the
remaining handoff and admission requirements. This is implementation work still
outstanding, not runtime evidence from the current metadata tests.

### Ordinary export owner and image lifetime

The next physical adapter should borrow `&ProjectionStore` and own
`Option<format::Access>` plus an owned image. Acquire Exclusive access so
cooperative Layrs writers cannot advance the source. Do not store a borrowed
`ContentScope`, a transaction, or `VerifiedImportClosure` inside the holder:
reconstruct them in `Access::with_content`, keep the SQL snapshot through the
operation callback, and return detached DATA only. Constructor failures after
Access acquisition and terminal outcomes must consume `Access::run`, preserving
both the primary error and an observable close failure. A silent Drop is not a
successful close or a transition acknowledgement.

The image retains the root and selected-file pins, canonical manifests, modes,
base preconditions, deletions and its effective-image digest. Use
`PinnedDirectory::open_immutable_file` for selected present files, validate
single-link identity/size/mode and hash via the retained handles. Compare the
complete image with the initial scoped scan and recheck the namespace and
absences before destination effects. Historical content uses exact scoped
manifest/stream readers under the retained owner/family access.

There is a concrete Windows integration constraint: the current capture scanner
uses `open_file`, requesting DELETE access. It cannot rescan while immutable
file pins deny write/delete sharing. Introduce a readonly scanner adapter using
`open_readonly_file`, or reuse a namespace-only check while hashing retained
handles. Do not remove immutable pins to make the old scanner succeed. These
pins block ordinary incompatible Windows opens, but do not freeze absent names
or the whole namespace; external writers must remain quiescent and rechecks
remain necessary.

Keep the Provider `derive`/`derive_publication` adapters and their activation
proof checks intact. Add a separate ordinary adapter and extract only the
common DATA derivation over observation, placement, selection, current image
and historical manifest union. Do not introduce an interchangeable image proof
that lets ordinary DATA satisfy Provider native authority. This is a design
decision from source inspection, not implementation or runtime qualification.
