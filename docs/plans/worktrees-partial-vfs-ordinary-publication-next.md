# Ordinary B SourceReady / SourcePublished and C9: next implementation design

> Historical investigation, superseded in part on 28 October 2026. Ordinary
> admission now targets private schema **10**, with the bounded immutable union
> witness described in [the autonomy decision](worktrees-partial-vfs-ordinary-autonomy-evidence-gap.md).
> Provider8→9 remains separate. The retained B publication and C8→10 admission
> paths have bounded runtime evidence (r51 and r57); the blockers listed below
> describe the earlier source state, not the current implementation. See the
> [current handoff](worktrees-partial-vfs-resume-2026-10-28.md) for qualification,
> failures and remaining fresh-owner/recovery work.

Read-only source investigation, 28 October 2026. Product root: `D:/Layrs/layrs`.
All paths below are relative to that root; `P` abbreviates
`crates/layrs-store-local/src/projection/provider/`, and `S` abbreviates
`crates/layrs-store-local/src/projection/shared/`. No build, Cargo, SQL, native
execution, test/fixture opening, Git mutation or product-file edit was performed.
This note makes no runtime or qualification claim. The family-publication worker
is changing `P/managed_export/ready.rs`; line references describe the source
observed during this investigation and integration must accommodate that work.

## Contract and current execution path

The required end state remains B Published, actual C9 admission, then autonomous
C with A/B unavailable. `docs/plans/worktrees-partial-vfs-ordinary-export-driver.md`
(`Acceptance sequence`, items 1-4) explicitly requires inherited history/inverse
and a new C capture, plus unchanged B admission. C8 Ready alone is insufficient.

Observed path:

1. `P/managed_export/ready.rs:163::with_ready_source` validates retained B image,
   calls `plan_admitted_import`, finds exact durable SourceStaging, derives the
   original ordinary plan/union, builds `OwnedOrdinaryReadyData`, and validates
   C/family with independent physical witnesses. Private/family outcomes are
   bounded DATA, not a transferable admission capability.
2. `P/destination_ready/ordinary.rs:111::prepare` calls
   `P/ordinary_destination.rs:75::authenticate_staging_source`. It requires actual
   B SourceStaging, not merely a reconstructed Staging proposal. C is validated
   separately as complete Ready. `S/provider_import/source.rs:231::require_export_target`
   also explicitly requires actual Staging.
3. `P/managed_admission/read.rs:134::read_admitted_import` authenticates schema9,
   actual incoming record/SourcePublished archive, copied incoming prefix,
   immutable Ready rows, then outgoing ordinary transitions. The incoming
   transition is selected by B's admission transaction, never by a caller's ID.
4. `P/ordinary_transition.rs:151::require_shape` currently rejects outgoing
   Ready/Published and every receipt ID. `:144::is_live` only includes
   Planned/Copying/Staging. `frontier.rs:57::require_version` requires a live
   prefix to equal the current generation/capture frontier and allows older
   bounded frontiers for terminal rows.
5. A second, independent blocker is
   `P/destination_ready/historical.rs:25::read_frozen_ready_rows`: its immutable
   inventory counts the WHOLE receipt table and requires incoming closure count
   + one incoming Ready receipt. Inserting B's outgoing receipt would break B's
   admission even after extending outgoing state shapes.

Existing Provider publication is a useful protocol example, not an ordinary
source proof: `P/transition_sql/publication.rs:8::apply_source_publication` and
`P/bootstrap/preparation/activation/managed_content/source_publication.rs`
require allocation/prepared/bootstrap binding, active mount, activation, native
root, and Provider-specific publication closure. None can be filled with B9 DATA.

## Close semantics: explicit requirements versus design choice

Explicit ordinary contract:

- `docs/plans/worktrees-partial-vfs-ordinary-source-intents.md`, `Actual writer`:
  "Commit is observed explicitly" and "The owner keeps its pins until positive
  close". It distinguishes transaction commit from owner close.
- `docs/plans/worktrees-partial-vfs-ordinary-export-driver.md`, `Concrete adapters`:
  "Terminal success still observes positive database closure."
- `P/managed_export.rs:707::finish_writer` observes COMMIT/rollback only;
  `:559::close` / `:563::finish` consumes Access and closes the source connection.
  `projection/format.rs:430::Access::run` and
  `P/admitted_private_io/handle.rs:66::finish` implement the actual finalization.

Provider-specific implementation:

- `managed_content/source_publication.rs:7-10` explicitly says SourceReady and
  Published have distinct commits and destination/family snapshots span each
  source COMMIT. `publish_source` closes the scoped source writer before fresh
  readback/next phase. This is stronger per-phase connection scoping in that
  implementation, not evidence that existing ordinary callbacks close SQLite.

Smallest ordinary design consistent with existing owner semantics: keep the
genuine B owner/connection through distinct Ready and Published transactions;
return phase *progress DATA*, with explicit documentation that COMMIT succeeded
but source native close is still pending. C/family RO snapshots must span each B
COMMIT and close positively before continuing. The outer operation must close
the B owner positively before reporting terminal success. C admission must still
open its own independent B RO connection; the held B writer connection is never
the publication durability witness. Do not invent a per-phase OS close claim.

If the integration instead promises a physical source-writer close ACK after
each phase, deliberately add scoped clean B writers while the existing B handle
is idle/autocommit (no B RO snapshot may block their COMMIT), or a narrow
close/reopen adapter retaining B's owner/lifecycle/image pins. Existing
AdmittedPrivateHandle hardcodes its internal close observer and exposes no such
per-phase API. That is an implementation choice, not a prerequisite inferred
from the ordinary source-intents contract.

For either choice, a failed/withheld enclosing close ACK stops progress: no next
phase, new SQL readback, admission, release or cleanup on that invocation.
Transaction rollback and mandatory owned-handle cleanup are distinct from
validation/reprobe. Audit ordinary outer error exits: `with_source_transaction`
does post-action image/SQL validation, and `AdmittedPrivateHandle::finish` also
calls verify on error; these must not turn a nested observable-close failure
into fresh SQL validation. A narrow failed-close state/no-validation cleanup
path may be needed. Preserve actual close failure/quarantine behavior.

## B row model and reader changes (first implementation unit)

No new transition state, receipt type, or SQL table is necessary. The existing
`P/schema.rs` transition table has `receipt_id` FK into
`projection_provider_transition_receipts`; `records/transition.rs::require_transition`
already supports Staging->Ready->Published and preserves immutable fields.
The correct receipt codec is `P/records/import.rs::ProviderTransitionReceiptRecord`
(not bootstrap receipt-file DATA from `records/receipts.rs`).

Extend ordinary shape deliberately:

| B phase | destination | reservation | receipt_id | live/current rule |
|---|---|---|---|---|
| Planned | none | none | none | current; fenced |
| Copying | exact ordinary C proof | none | none | current; fenced |
| Staging | same proof | exact operation | none | current; fenced |
| Ready | same proof | same operation | exact C receipt digest | current; fenced |
| Published | same proof | same operation | same digest | historical allowed |
| Cancelled | existing supported form only | existing rule | none | historical allowed |

In particular add Ready to `is_live` and `has_live`'s supported shape together.
Otherwise Ready could admit a new ordinary mutation/export before publication.
Keep one incoming DestinationReady and at most one live outgoing intent; do not
change `database.rs::require_idle` / restore recovery to bypass the fence.
`allocation::require_destination` can validate the same proof for Ready/Published;
it does not require a Copying phase internally. No new backend/native fields.

Receipt partition must be explicit and nonrecursive. Suggested internal return:
`OrdinaryOutgoing { intents, receipts }`, not an arbitrary all-receipts vector.
After genuine incoming prefix/receipt links are authenticated, use bounded
normalized SQL preflight for all receipt scalar/payload columns before decoding
or global FK scans. Decode/require_row each outgoing terminal receipt; require
its exact terminal source intent via require_receipt, unique owner/reference,
and no collision with the authenticated incoming ancestry or incoming receipt.
Every receipt must belong to exactly one of those disjoint domains. Reject an
orphan, duplicate owner, wrong transaction, wrong prefix or extra receipt.

`read_admitted_import` can read outgoing before frozen Ready and pass the
validated outgoing receipt partition into its inventory check; read_outgoing
uses incoming record + bounded frontiers and must never call admission/planner.
Alternatively split partition preflight from final outgoing interpretation.
Replace frozen Ready's whole-table receipt count with exact incoming scoped
count + verified terminal outgoing receipts, preserving all other frozen checks.
Keep new receipt count/aggregate byte limits explicit (e.g. outgoing count
bounded by the 4096-transition envelope and 16 MiB payload aggregate); account
incoming ancestry independently rather than adopting an outgoing receipt as a
new ancestry root. Fence totals before global FK interpretation.

Ready must match current B generation AND capture count. Published reads use
the declared immutable high-water within admission/current bounds, with the
existing snapshot parent/selection/capture-head checks. A historical Published
handoff must not invoke a current-prefix planner, compare its old image with
B's new checkout, or require old generation == current. The current ordinary
`read_source_metadata` intentionally has a current-head check; use its historical
kernel/explicit admitted historical anchor instead of weakening the live API.

## Exact B SQL primitives and durable mutations

Proposed new `P/ordinary_transition/publication.rs` exports narrow pure SQL APIs:

- `apply_ready(c, marker, previous_staging, destination_ready, receipt, cancel)`.
- `apply_published(c, marker, previous_ready, destination_ready, receipt, cancel)`.
- A distinct complete `read_publication_state` returns the actual Staging,
  Ready or Published row/receipt under a retained transaction for physical
  driver replay; it must not route terminal states through a Staging reader.

Each API requires a caller transaction/FK ON, genuine read_admitted_import,
actual stored predecessor (or exact already-next replay), and all canonical
row/prefix links. The SQL primitive proves DATA only; physical C/family/pins
are the outer driver's obligation. On actual promotion the writer must require
the appropriate current B frontier; Published read/replay used by handoff may
be historical. No silent changes to prefix, plan, quota, source generation,
destination, owner or reservation.

Construct SourceReady by decoding C's ACTUAL complete DestinationReady then
changing only role=Source and local_generation=source version.generation; leave
state Ready and receipt digest unchanged. Compare immutable plan/proof/reservation
with actual B Staging and use require_transition + require_receipt. Receipt bytes
must be C's decoded canonical receipt, compared exactly with retained expected
DATA; never create a new similar source receipt from caller fields.

Staging->Ready writes in ONE B transaction:

1. INSERT the exact C receipt into
   `projection_provider_transition_receipts(id,owner_identity_digest,transaction_id,companion_id,payload)`.
2. CAS B transition by every old normalized column/payload/digest, setting only
   state='ready', receipt_id, payload, digest. Role/source local generation,
   destination and reservation remain exact.
3. Complete post-read validates B incoming admission is byte-for-byte unchanged,
   source row exact, receipt exact, partition complete, capacity sufficient.

Ready->Published is a separate transaction: no INSERT/DELETE, CAS only
state='published', payload, digest while exact receipt FK/bytes remain unchanged.
Never mutate B's projection_managed_admission, its stored incoming SourcePublished,
incoming DestinationReady, import prefix, archive, head/captures/content/claims,
or C/family in either B primitive. Exact Ready/Published replay does no writes;
an already receipt without its source owner is corruption, not an adoptable
predecessor. Add source terminal receipt/row bounds and normalized capacity
before first mutation; validate the funded remaining envelope without changing
the immutable quota. Provider source_metadata_floor is sizing precedent only.

## Ordinary physical publication context (second unit)

New `P/managed_export/source_publication.rs`, using existing owner rather than
Provider `ManagedImportLease`. Suggested methods:
`publish_source_ready`, `publish_source_published`, and a sequential
`publish_source` wrapper, returning explicit progress/effect DATA. Bind arguments
to retained destination allocation, protection, checkout proof, C privateReady
and familyReady DATA; outcomes alone never authorize an effect.

Before first promotion while B really is Staging, fully rederive/freeze original
ordinary plan, snapshot/selection, copied mappings, full exact required union,
expected checkout/journal and metadata. Authenticate actual C8 Ready using its
closed reader and exact decoded receipt; authenticate family Ready separately
via full retained-family + C publication walk with member==Ready. Cache bounded
DATA only alongside retained physical capabilities. On replay from B Ready or
Published use a separately named publication reader/basis, never call
with_ready_source, OrdinaryCopyInputs, reserve_plan::derive with a pretend actual
Staging row, or OrdinarySourceInputs::require_export_target unchanged.

Factor common ordinary Ready suffix mechanics only behind closed phase inputs,
e.g. `OrdinaryPublicationInputs` authenticating the ACTUAL B outgoing phase plus
unchanged prefix/plan and `read_ordinary_ready_for_source_publication`. It can
construct expected predecessor DATA for calculations, but must not assert B is
still Staging. Provider variants remain strict.

Retain independently: B Exclusive owner/lifecycle/FORMAT/DB and original image
pins; C owner/root/control/FORMAT-inode/private DB/member pins from allocation;
family marker/catalog/lifecycle pins and catalog gate; immutable C CAS proof for
the entire base+history+current union; exact completed checkout journal plus
verified final-file pins. Recheck identities/namespace boundaries before effects,
before COMMIT and after positive closes. The external quiescence requirement
still applies to absent/deleted names; retained leaf handles alone do not freeze
the whole namespace. No content hash/scan under the family catalog gate.

For EACH B phase: begin retained B transaction; open distinct C RO and family RO
scopes; validate actual C privateReady + exact receipt and full familyReady/root/
Published exact journals/shared debit/remaining quota; run the pure B CAS;
post-interpret all three while snapshots remain held; COMMIT B; perform only
allowed retained physical checks after commit; close enclosing RO scopes
positively; then perform fresh complete publication interpretation. If any
boundary or close fails, stop at its actual possibly committed phase. Use
phase-specific source validation after Ready, without rehashing through the old
Staging content route or adding a new observation/image.

## B Published -> C9 handoff and autonomous C (required follow-on units)

Reuse existing physical pinning/checkout/CAS/clean-I/O mechanisms, not their
Provider-only source contracts. Required changes:

1. `P/import_validation/destination_prefix.rs:18,262`: replace mandatory
   ProviderBootstrapSource/activation fields with a closed historical source
   enum. Provider keeps current catalog/activation reconstruction; ordinary
   requires real source schema9 admission binding, copied immutable B snapshot,
   selection and ancestry/restore rowsets. Its destination anchor already avoids
   fabricating B FORMAT. Read C8 singleton Ready or C9 admission-selected incoming
   Ready as today. Reuse bounded historical/scoped closure kernels and origin
   resolution. Do not require access to live B/A in this destination-local path.
2. `P/destination_ready/autonomous.rs:122`, `historical.rs:25`, and
   `autonomous/inventory.rs`: add explicit ordinary frozen basis. Observation and
   placement bodies are BOTH copied B snapshot at prefix.version.generation,
   with two distinct generation/parent frames; selection comes from copied B
   selection and version.selection_digest. Required union includes selected
   bases, copied capture/restore representations and exact checkout reps, with
   protected triple-rowset/overlay/image commitments checked. Ordinary historical
   inventory must derive unchanged lengths from authenticated committed evidence,
   not Provider descriptors. Later C ordinary snapshots/captures/claims remain
   separate from the frozen import boundary.
3. `P/checkout_journal/inventory.rs:72::expected_ready` and
   `archive.rs:6::read_archived` still hardcode Provider. Add closed ordinary
   terminal constructors/readers retaining all alias/path/header/journal rules;
   do not pass an ordinary record through the Provider constructor.
4. Terminal family validation must use the complete multi-group interpreter in
   `S/admitted_catalog.rs:389::validate_catalog_parts` (or factored kernel).
   `read_managed_ready` and `managed_ready_reopen/admission.rs:273::require_family`
   currently call validate_protected_installations; its inner implementation
   `S/exact_content/sql.rs:485` explicitly rejects every foreign exact reservation.
   B's retained group plus C's group therefore needs the existing shared charged
   sets, complete root/reservation inventory and retained/exact classifiers.
   Reuse `validate_retained_installations`; do not skip B rows or clear them.
5. Add ordinary `PublishedForReady` variant or separately named
   `read_ordinary_published_for_ready(c,B marker,C ready,C receipt,cancel)` in
   `P/published_handoff.rs`. Require actual B schema9/admission/database/FORMAT,
   exact B SourcePublished row AND stored receipt, immutable equality with C's
   transaction/plan/prefix/destination/reservation/receipt, and revalidate on the
   same retained transaction. Historical B suffix may advance. No Provider root,
   native session or synthesized SourcePublished. An independent physical driver
   opens the actual pinned B DB RO and retains that snapshot through C COMMIT.
6. `P/managed_admission/write.rs:34` and `record.rs:99` currently reject Ordinary.
   Add a closed origin interpretation: ordinary source_database_identity comes
   from prefix.source.admission.database_identity; source_format_digest from
   the ordinary prefix; boundaries from its version. Preserve actual incoming
   admission/receipt binding as part of prefix ancestry. Existing v1 admission
   fields can represent these scalars without DDL changes if semantics are
   documented. Reuse exact journal archive, actual SourcePublished BLOB/digest,
   budget/preflight, prejournal deletion and user_version=9 in ONE transaction.
   C's incoming DestinationReady/receipt/prefix/FORMAT stay immutable.
7. Extend `P/managed_ready_reopen{,/admission,/pins}.rs` or create explicit ordinary
   equivalents. Preserve Exclusive C owner and independent B RO + family scopes
   through C9 COMMIT/positive closes, physical proof rechecks, fresh schema9
   readback, and destination-only `resolve` after an uncertain admission ACK.
   Current `ManagedReadyAdmission::commit` takes Provider PublicationSource and
   cannot admit an ordinary source as is. After admission, ordinary format/access
   uses read_admitted_import and multi-group admitted family policy; no live B
   or A may be opened to inspect history, restore or capture in C.

Material representation-evidence decision: the live ordinary basis retains exact
B selected-base FileId->ManifestId, but prefix closure includes history/restore
mappings and need not include every base. `managed_export::include_selected_bases`
adds them separately. `destination_ready/data.rs::prepare_suffix` selects first
required representation per previously unmapped FileId for new C mappings; that
need not be B's original base ManifestId when the current image has another
manifest of the same FileId. Autonomous C must not claim that row uniquely
recovers the original B mapping. Smallest full-goal-consistent resolution is to
validate the frozen committed exact protected union, copied exact mappings,
checkout exact representations, selected-base coverage and length consistency,
without claiming an unrecorded B mapping. If the intended contract instead
requires reconstructing that exact historical mapping on each reopen, persist
a bounded explicit ordinary-basis witness before publication/admission (and
include its schema/budget/codec commitment); this is a real additional contract
decision. Do not silently choose an arbitrary FileId representation as evidence.

## Validation cases and recovery distinction

These are proposed cases, not executed checks:

- Exact B Staging->Ready inserts precisely C's receipt and one source row change;
  unchanged B ingress/archives/head/captures/content and unchanged C/family.
  Ready->Published changes only source row phase/payload/digest. Already-phase
  replay opens no writer and neither reinserts receipt nor changes quota.
- Wrong C decoded receipt/body, changed plan/proof/owner/database/admission,
  outgoing-as-incoming receipt, orphan/duplicate/missing receipt, receipt scalar
  or aggregate overflow and extra transition all refuse before mutation.
- Ordinary operations and second export refuse at B Ready; after Published
  valid later B generation/capture is accepted by historical handoff. Regression:
  using old Staging callback after promotion must refuse, while the new
  publication reader succeeds against actual terminal state.
- C privateReady without exact familyReady refuses; wrong member phase,
  missing root/manifest/chunk/journal, altered checkout/deletion identity,
  unknown family group, double debit or insufficient remaining envelope refuses.
  Include unchanged and changed files with the same FileId but distinct manifests,
  selected bases absent from captured history, restore-only bytes and deletions.
- Same-owner deliberately withheld ACK AFTER a genuinely successful C/family
  close: retained B/C owners/pins survive, durable state is independently checked
  only on a later explicit retry, exact replay makes no duplicate write, and the
  failed call performs no subsequent SQL. This is ACK-loss coverage, not actual
  sqlite3_close/xClose failure. Under retained B connection semantics inject at
  enclosing C/family close or outer terminal close; do not label transaction
  rollback as a source physical close. Under scoped B writer semantics additionally
  inject at each real B writer close and prove no next phase/readback on failure.
- Genuine owner loss: destroy/restart all relevant holders, independently reacquire
  owner/lifecycle/FORMAT/root/database/family/CAS/checkout proofs, interpret actual
  durable phase and resume from original journal commitments. Existing allocation
  explicitly forbids adopting an orphan by pathname. A lost C holder while B is
  Ready needs a fresh ordinary Ready opener/publication recovery adapter; generic
  begin_managed_export cannot reopen a live intent through ordinary idle access.
  Metadata rollback/journal recovery, process-profile quarantine and cancellation
  remain explicit capabilities; no automatic cleanup follows ambiguous SQL.
- Published-before-admission recovery: B advances coherently, new independent B
  RO handoff still succeeds. If B is unavailable with no C9 handoff, C8 remains
  read-only; after C9 committed, destination-only resolve/reopen succeeds without
  B and never republishes source. Full acceptance closes all owners, makes A/B
  checkout/private databases unavailable while retaining the shared family needed
  by C, reopens C, reads inherited captures/publications/restore history, applies
  inherited inverse, and creates a new ordinary capture. Preserve B's original
  admission and family retention evidence; source unavailability is not GC.

Implementation order: receipt partition + ordinary Ready/Published shapes/fence
and pure writers; phase-aware ordinary physical publication; destination-local
ordinary historical/terminal Ready + checkout archive and full-family reading;
actual ordinary Published handoff + atomic C9 admission/physical resolution;
autonomous C end-to-end; then genuinely independent owner-loss recovery cases.
Keep the full goal pending until C9/autonomy passes, even if B publication is
delivered as a separately reviewable increment.
