# L5 partial VFS convergence — proposed implementation contract

Date: 2026-10-24. Programme: [Worktrees, partial availability and VFS](worktrees-partial-vfs.md).

**Status: contract reviewed; schema foundation authorized, implementation pending qualification.** The programme direction is accepted: a
distinct format-4 `ProjectionVfsWorktree`, the existing availability/content
engine, and no incomplete native SCM Store. New types, APIs, database extensions,
state transitions and tests below are proposals, not descriptions of implemented
contracts. Existing L3/L4 code supplies building blocks; its qualification does
not qualify this integration.

The backend corrections and bound recovery now have fresh native qualification
and a narrow independent review without remaining P0/P1/P2 findings. Critical
contract review R3 found no remaining P1/P2 design blocker after the five
revisions in the [Store memo](worktrees-partial-vfs-storage.md#l5-store-contract-revision-r2--proposal-only-2026-10-24).
The parent authorized stage 1a: explicit private schema 7-to-8 upgrade,
compatibility validation and ordinary-backend gates. This grant does not enable
mounting, source reads, activation or capture before their actual provider
capabilities and tests exist. The Windows-only Store-local dependency on VFS is
reserved to the parent for that next integration boundary. The proposed APIs
below remain unimplemented until their respective source and qualification are
recorded. The parent owns scheduling, interface decisions and integration.

## Outcome and completion boundary

Two independent Windows CFAPI Worktrees expose selected metadata from exact
observed versions, share verified immutable content in one format-4 family, and
keep private writes, captures and journals. Known files remain visible when
their bytes are unavailable. Explicit authorized preparation makes their bytes
available for subsequent offline reads; filesystem callbacks never fetch them
from the server.

A first delivery may have a fixed selected namespace, resident-only reads,
provider-aware capture, durable reconnect and ordinary-file recovery export.
That delivery is a partial milestone, **not L5 completion**.

L5 completion also requires:

- Ordinary **managed** recovery into a usable ordinary format-4 Worktree,
  preserving effective work and history/provenance. An export artifact alone
  does not satisfy this requirement.
- Explicit selection additions that preserve existing edits and deletions. A
  safe remount or a fresh destination root is acceptable; in-place expansion is
  not required.
- Qualification of the supported Windows lifecycle and failure cases below,
  with independent review and recorded evidence.
- Conditional evaluation of Linux/FUSE only after Windows qualification and
  only where its prerequisites are available. macOS remains unqualified.

## Existing implementation seams

These observations describe the source inspected on the date above, not passing
L5 tests. The [L3 storage design](worktrees-partial-vfs-storage.md) and
[Windows provider handoff](worktrees-partial-vfs-provider.md) retain their own
implementation and qualification records.

| Existing seam | Reuse and required change |
| --- | --- |
| [`core/projection/availability.rs`](../../crates/layrs-core/src/projection/availability.rs), [`model.rs`](../../crates/layrs-core/src/projection/model.rs), [`selection.rs`](../../crates/layrs-core/src/projection/selection.rs) | Exact versions, selection, observed rights and availability facts. Keep namespace installation separate from content residency. |
| [`projection/offline.rs`](../../crates/layrs-store-local/src/projection/offline.rs): `availability`, `read_version_file`, preparation methods | Add selected-catalog/range APIs and provider-aware capture preflight. Current capture/restore preflight enters the ordinary scanner. |
| [`projection/retention.rs`](../../crates/layrs-store-local/src/projection/retention.rs): `root_metadata`, `lookup_with_roots`, `reserve`, `accept_claim` | Reuse private ownership, missing-protected-content checks, quota and protection ordering. |
| [`shared/content.rs`](../../crates/layrs-store-local/src/projection/shared/content.rs): `check_manifest_resident`, `stream`, `stream_chunk` | Reuse admitted manifests, pinned chunks and verification; add a bounded range reader under the same lease. |
| [`capture.rs`](../../crates/layrs-store-local/src/projection/capture.rs), [`capture/storage.rs`](../../crates/layrs-store-local/src/projection/capture/storage.rs): `persist_in_transaction` | Reuse patch construction/history/content acceptance. Extract live-file reopening and ordinary rescanning behind evidence adapters. |
| [`format.rs`](../../crates/layrs-store-local/src/projection/format.rs): `access`, `verify_directories`; [`database.rs`](../../crates/layrs-store-local/src/projection/database.rs) | Add narrowly bound provider-root validation, versioned private state and backend-aware operation gates. |
| [`remote/projection_offline.rs`](../../crates/layrs-client-core/src/native_scm/remote/projection_offline.rs), [`download_refresh.rs`](../../crates/layrs-client-core/src/native_scm/remote/download_refresh.rs) | Reuse explicit exact-authority preparation and validated fetch plans. Add manifest-metadata preparation without chunk downloads. |
| [`layrs-vfs/catalog.rs`](../../crates/layrs-vfs/src/catalog.rs), [`evidence.rs`](../../crates/layrs-vfs/src/evidence.rs) | Reuse `Catalog`, `SnapshotSource` and guarded evidence. Document intentionally missing source content. |
| [`windows/checkpoint.rs`](../../crates/layrs-vfs/src/windows/checkpoint.rs), [`control.rs`](../../crates/layrs-vfs/src/windows/control.rs) | Reuse complete namespace reconciliation and retained readers; add an exact format-4 control-directory capability. |
| [`vfs_source.rs`](../../crates/layrs-client-core/src/native_scm/vfs_source.rs), [`vfs_capture.rs`](../../crates/layrs-client-core/src/native_scm/vfs_capture.rs), [`vfs_worktree.rs`](../../crates/layrs-client-core/src/native_scm/vfs_worktree.rs) | Keep complete-store adapters intact. They bind native admitted graph objects and build native snapshots; partial adapters must not pretend to satisfy those contracts. |
| [`projection/conversion.rs`](../../crates/layrs-store-local/src/projection/conversion.rs), [`scm_v1/vfs_recovery.rs`](../../crates/layrs-store-local/src/scm_v1/vfs_recovery.rs): `VfsExportWriter` | Reuse provenance/recovery principles and verified ordinary-file writing. Existing format-3-to-4 conversion is not managed VFS-to-ordinary conversion. |

## Proposed selected metadata catalog

The adapter owns an immutable catalog for a single mount epoch. Its binding
contains family incarnation, Store/Worktree identity, private database identity,
exact projection generation/digest, observed Step/root, authority observation,
selection digest, catalog digest and provider registration/root identity.

The catalog describes every selected supported file with its exact path,
`FileId`, logical byte length and mode, plus coverage for explicit absence and
protected entries. Directories derive from validated paths. It is complete for
the selected observed namespace; it makes no claim that the repository Tree is
complete locally.

Exact-path selections require exact path observations. Subtree selections
require completed bounded structural enumeration. An unfinished page sequence,
unknown size, unsupported selected target or unresolved coverage cannot silently
remove a name or become absence. Reserved/excluded observations retain their
fences and never grant capture permission.

`ProjectionEntry` currently has no byte length. Resolve length from an admitted
local manifest or an explicit metadata preparation operation. For the latter,
use a validated exact-Step/path `FetchClaim::File` plan, select its manifest
capability and download only the manifest. Validate canonical manifest identity,
declared `FileId`, selection and authority before accepting the descriptor.
Preserve existing exact-authority renewal checks. Do not use
`fetch_authorized_file` unchanged: it acquires the content closure too.

Metadata-only descriptors belong in the private database. Shared `manifests`
and `manifest_chunks` represent installed immutable content; inserting a
descriptor with missing chunks there would weaken that contract. A remote
descriptor's manifest representation may differ from the family's locally
installed representation; require matching `FileId` and logical length, not an
invented equality between those manifest IDs.

### Proposed interface sketch

These are semantic signatures for coordination, not existing Rust APIs or a
promise that their names have been finalized:

```text
selected_vfs_catalog(expected_generation) -> SelectedVfsCatalog
read_version_range(binding, file_id, offset, output, cancellation)
begin_provider_mount(expected_generation, catalog) -> MountIntent
activate_provider_mount(intent, verified_namespace) -> VersionedProjection
begin_provider_capture(binding, expected_capture_head, transaction_id) -> CaptureToken
accept_provider_capture(token, complete_evidence, staged_files, revalidate)
    -> CapturedPatch
```

Recommended placement semantics: only after durable mount intent, complete
placeholder installation and identity verification may activation advance the
projection generation and mark those installed entries `Materialized`.
`Materialized` then describes namespace installation, not resident content.
Targets, permissions, selection and exact remote base remain unchanged.
Unactivated `Unavailable` entries stay fenced. Implement this as a distinct
journaled provider installation operation; do not call the ordinary hydration
installer with fabricated bytes. This interpretation needs the L3 owner's
review before implementation; it is not a claim about current behavior.

Activation must durably record an exact mapping from the original catalog
generation/digest to the activated capture generation/digest, bound to the same
mount identity, selection and catalog digest. Commit that mapping atomically
with the placement-generation transition and ready state. The source catalog
continues naming the original immutable generation; captures bind to the
activated generation. Neither reader nor capture may guess the relationship
from the current head or `FileId` equality alone. Validate unchanged Step/root,
targets and authority across activation. Recovery reuses the same mapping;
repeated activation must not allocate another capture generation. A later
explicit selection transition creates its own binding instead of rewriting this
mount's map.

## Proposed resident reader and explicit preparation

Add `ProjectionVfsSource` as a separate `SnapshotSource` implementation. It
resolves only identities in its fixed catalog. It acquires the family lifecycle
lease before resolving the private binding/representation and keeps that lease
through manifest validation and every intersecting chunk read/hash. Never return
an unpinned content pathname for a later read.

Use the established lock order: family lifecycle lease, Worktree lock, then
short shared-catalog lock. Cancellation and lock acquisition must be bounded.
The provider transfers a buffer only after a successful complete source read;
owned verified bytes remain usable after releasing the read lease.

The first implementation keeps file-granular admission. A file is installed and
verified completely in the shared family, while CFAPI reads ranges within it.
This proposal does not require a new partially admitted manifest/chunk format.

| Condition | Required result |
| --- | --- |
| Requested immutable bytes intentionally unprepared/unretained and absent | Typed `needs_hydration` with file/range identity; provider completes the request as failure. |
| Protected representation or captured replacement missing | Integrity/recovery error; never an acquisition miss. |
| Chunk/manifest corruption or changed binding | Integrity/identity error; no successful bytes substituted. |
| Cancellation | Cancelled request with bounded source latency. |
| Fully verified requested range | Exact bytes, including valid empty-file behavior. |

No missing range may succeed with zeros, truncation or guessed content. No read,
status or capture callback may perform HTTP. Native complete-store admission
continues requiring the complete local content closure.

Reuse `prepare_projection_offline(..., Read)` for explicit acquisition: validate
current exact-path authority, download outside acceptance locks, verify content,
install shared protection, and commit the private ready receipt. Subsequent
reads work offline. Preparation changes residency, not the selected namespace
or the mount's exact source version. Releasing a preparation removes private
claims before shared reclamation; it cannot invalidate an active read lease or
accepted capture protection.

### Read preparation for a Capture version

A `Read` request whose version is `Capture` must resolve dependency provenance
per path before deciding whether acquisition is allowed. This is a request to
read a captured version, not an `OfflineOperation::Capture` preparation receipt.

| Captured-version path | Read/preparation dependency |
| --- | --- |
| Unchanged path inherited from the capture's exact base | Resolve the patch-bound `Generation` and path/target as a base-content dependency. If its bytes were never prepared, explicit authorized `Read` preparation may acquire them. |
| Captured replacement | Resolve the accepted local replacement and its durable protection. Missing replacement bytes are corruption; never download a substitute, even if a remote/base file has an equal `FileId`. |
| Captured deletion or explicit base absence | Preserve absence; there is no content dependency to fetch. |

Bind resolution to capture sequence/digest, its recorded projection generation/
digest and import provenance where applicable. Verify the durable activation map
when relating that capture base to the fixed source catalog. Do not resolve
against the latest generation or silently reinterpret a replacement as inherited
base content. Preparation retains the original Capture request identity and
records its exact base dependencies; it does not rewrite the capture, selection
or mount catalog. Apply current exact-path authorization to any acquired base
bytes, and keep callbacks offline-only.

The independent review reports that the current online adapter rejects all
missing content for Capture operands. The dependency-aware correction is
assigned to L2 in `remote/projection_offline.rs`; it remains an implementation
dependency, not an already available behavior. Local resolution/retention must
continue rejecting missing captured replacements as corruption.

Update the `SnapshotSource` documentation to allow a fixed catalog with missing
bytes and pin available content through its actual read. Its current all-session
content-pinning wording describes the complete-store adapter.

The Windows provider currently uses ALWAYS_FULL namespace population and
PROGRESSIVE hydration. Windows can request remaining content in the background,
so no page-level residency saving is promised. Microsoft's
[hydration policy documentation](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ne-cfapi-cf_hydration_policy_primary)
also marks PARTIAL unsupported. Do not change OS policy or enable a feature as
part of this convergence work without its separate qualification.

## Proposed provider-aware capture transaction

Extract common reconciliation/persistence from `capture_patch_with_faults` and
`persist_in_transaction`. The ordinary adapter retains its scanner checks. The
provider adapter supplies a complete selected image and staged replacement
files; it must not re-enter the ordinary scanner or reopen live paths inside
the acceptance transaction. Availability preflight must use the same adapter,
including quota for the whole resulting selected capture.

Complete evidence covers every installed catalog leaf, selected explicit
absence observation, actual selected entry and required ancestor. Bind it to
root/catalog/selection identities and retain file/directory guards through
acceptance. A caller-provided list of changed paths is insufficient. Build this
evidence through provider checkpoint validation, not an unchecked serialized
`complete=true` assertion.

| Guarded evidence | Reconciliation |
| --- | --- |
| Partial, in-sync placeholder; exact root/file/catalog/path identity; unchanged size; zero modified bytes | Reuse exact immutable base `FileId` without reading its bytes. |
| Fully resident file, dirty file, ordinary replacement or insufficient clean evidence | Hash the entire effective file, even if size/time match. |
| Missing installed catalog leaf under a complete verified namespace | Deletion candidate, subject to observed write authority. |
| Existing placeholder whose source content is unavailable | Availability gap, never deletion. |
| Selected explicit absence plus new regular file | Creation candidate only with the existing exact observed destination authority. |
| Unknown creation, protected/excluded change, invalid identity, unsupported link or incomplete coverage | Refuse capture and preserve real files. |

Capture availability and acceptance must reconcile the complete selected image
first, then apply write-permission blockers only to actual proposed changes.
An unchanged read-only cold placeholder is allowed without content hydration;
an unchanged read-only fully resident file is allowed after complete hashing
proves it unchanged. Neither is blocked merely because `observed_writable` is
false. Modified read-only files remain blocked and preserved. Keep complete
namespace, integrity and quota checks even when there are no writable changes.
The ordinary and provider adapters must agree on this rule; the availability
correction is assigned to L3 and is not claimed implemented here. Watcher events
and timestamps are hints, not clean-content evidence.

`Checkpoint::with_reader` currently explicitly hydrates `NeedsHash` placeholders.
Use it only for that class. If a dirty partial file needs unavailable baseline
ranges, fail with `needs_hydration`, preserve its writes, and require explicit
preparation followed by a new checkpoint. A fully ordinary replacement can be
captured without downloading its old base. Never hydrate untouched clean files
merely to scan them.

Stage and fully hash changed effective bytes before acquiring the private
acceptance write lock. Hydration can call into the source reader; doing it under
that lock would risk deadlock. A short capture intent may precede staging, but
its pending-operation gate must still allow exact immutable reads needed by the
provider while fencing competing checkout mutations.

The acceptance token binds transaction ID, the durable source-to-capture
generation mapping, exact capture generation/digest, selection and catalog
identity, expected capture head and staged-image digest. Acceptance:

1. Revalidate the token, staged files and complete guarded namespace.
2. Reserve capacity for the entire resulting capture; install verified
   replacements and durable protective roots using existing retention code.
3. Commit the existing cumulative `CapturedPatch`, private claims and
   transaction-to-capture receipt together.
4. Return/acknowledge the durable receipt. Retain guards through the acceptance
   window; report a post-commit revalidation failure without losing the receipt.

Preserve patch sequence/history and the exact remote base. Do not retarget the
provider source or mark modified files clean after capture: their immutable
placeholder identity still names the original mount base. An accepted patch may
reference unchanged unavailable base content; expose its reconstruction gaps
instead of claiming a completely retained selected snapshot.

The caller must quiesce namespace writers and close/unmap writable mappings.
Directory handles and revalidation are not an atomic snapshot against arbitrary
uncooperative writers. Flush/close notifications and CFAPI in-sync state do not
constitute capture acceptance. Durable shared bytes/protection and the private
receipt are the acceptance boundary. Windows power-loss guarantees require
separate evidence from process-interruption recovery.

## Proposed persistence, roots and restart

Reuse the format-4 private database and shared content family. No parallel VFS
database or second content format is proposed. The inspected private schema is
version 7; version 8 is a candidate for this extension after the backend R2 fixes
and correction freeze, not an already reserved or implemented version.

Add bounded, versioned private records for catalog/descriptors and mount binding;
registration/population/activation phases and the original-to-capture generation
mapping; capture transaction/staging/receipt; and export/conversion/selection-
transition intents. Reuse existing projection generations, selection history,
captures, claims, reservations and staging mechanisms rather than duplicating
them.

Migrate explicitly while unmounted, with no unresolved conflicting operation.
Update schema validation, operation gates, fsck and family reconciliation
together. Old readers must reject the extended schema. Reclamation must fail
closed on unsupported or unreadable owners. Protect shared content before the
private transaction accepts references; release in the reverse dependency order.
Ambiguous crashes may retain excess content, never accepted dangling references.

Two root-validation changes are prerequisites:

- `format::verify_directories` currently uses ordinary `CheckoutReader`, which
  rejects reparse roots. Add an exact pinned provider-root capability, verified
  against the private registration, root identity and catalog on access. Do not
  add a generic allow-reparse switch. Keep absolute, nonnested roots and reject
  symlink/junction ancestors, root swaps and unsupported path aliases.
- Format 4 has a real `.layrs/FORMAT` directory; the provider currently attaches
  a plain `.layrs` control file. Add a capability for the exact pinned control
  directory and FORMAT identity. Creation may admit only this expected control
  entry beside the new namespace; checkpoint enumeration excludes only this
  verified capability, not arbitrary `.layrs` contents.

Route projection VFS discovery before ordinary `NativeWorkspace::open` or any
ordinary scan/materialization path. A distinct host workspace variant can carry
`ProjectionVfsWorktree`; it must not manufacture native Worktree/Operation/Tree
objects. Status reads bound metadata and provider evidence without implicit
hydration. All unadapted checkout mutators return an explicit unsupported/pending
result before touching a mounted root.

Restart reconstructs the original catalog, validates its durable mapping to the
capture generation and verifies the original registration ticket. Never
repopulate a ready root: missing leaves may be user deletions.
Persist registration intent/ticket before OS registration. Incomplete population
remains unready/needs-repair unless recovery can prove safe continuation;
unexpected existing files are never overwritten to finish creation.

Replay an accepted capture by transaction ID and return the same sequence/digest
after a lost acknowledgement. An unaccepted staged attempt requires fresh
namespace evidence before acceptance. Preserve ambiguous stages and quota
accounting. Disconnect cancels/drains callbacks before releasing source state;
stopping the host never deletes private writes, unregisters a useful root or
releases accepted capture protection.

## Required selection additions

The current placeholder identity embeds the whole catalog digest and entry
index. Changing the catalog invalidates existing identities. The first fixed
mount must reject ordinary expansion, receive/rebase, restore and publication
paths that would mutate its bound generation or checkout. Those initial guards
are not a permanent substitute for L5 selection additions.

Expansion must produce one coherent authority observation for the entire
expanded scope. It must not combine original-path permissions from an old epoch
with added-path permissions from a new epoch. Use either of these explicit
strategies:

- Preserve the original policy/authorization-epoch guard. Verify it still
  matches before and after acquiring the added scope; any change aborts expansion
  without mutating the source.
- Reobserve the **entire expanded scope**, including every original path,
  explicit absence and protected frontier, at one current policy/authorization
  epoch against the same exact Step/root. Reject an epoch change during that
  observation/acceptance. Keep the original observation as history; do not splice
  old grants into the new destination snapshot.

Reobservation applies even to original paths whose content is already in the
shared cache. Reconcile existing edits against the resulting complete authority
observation. Edits to newly revoked original paths are recovery-only: preserve
them in the companion recovery artifact or leave the transition blocked. They
cannot become authorized destination changes or publishable captures through a
cache hit or import provenance. Reobservation must not manufacture an absence
grant for an unreadable path.

The recommended first complete expansion path uses a fresh root/member:

1. Explicitly request a monotonic selection addition, choose one of the two
   authority strategies above, and obtain its complete guarded metadata. Check
   the expected source generation, selection and capture head. A cache hit does
   not replace path authorization, including for original paths.
2. Quiesce/checkpoint the source and durably preserve its effective edits,
   creations and deletions. Classify recovery-only work separately from changes
   authorized by the coherent destination observation. If required dirty bytes
   cannot be reconstructed, report recovery needs and keep the source unchanged;
   do not reacquire revoked content under an old grant.
3. Journal a fresh destination Worktree/root in the same family. Build its fixed
   expanded catalog against the same exact Step/root and one complete authority
   observation, with retained provenance and its own activation-generation map.
4. Populate metadata and replay only the authorized working overlay through
   guarded, no-clobber staging. Preserve recovery-only work outside the managed
   namespace or leave the transition blocked. Authorized existing deletions must
   remain deletions; do not resurrect them merely because the expanded base
   catalog contains those paths. Added files may remain unprepared placeholders
   with known metadata.
5. Verify the resulting namespace, content and source witness; publish destination
   readiness last only after authorized state and any companion preservation are
   verified. Record unresolved recovery-only work explicitly. Select the new root
   explicitly and retain the source.

The destination has a fresh Worktree identity and private journal/capture chain
with import provenance. An in-place remount may replace this approach only with
equivalent crash safety and a versioned identity transition. Do not silently swap
the source `Arc`, rewrite all identities or shrink selection. Metadata observation
and byte preparation alone never expand the visible namespace.

## Required ordinary managed recovery

Provide both a recovery export and managed conversion. Export is a prerequisite
and fallback, not the final L5 outcome.

Use the verified ordinary-file writer to create a fresh recovery container. Copy
effective changed bytes and unchanged resident content; record exact projection,
capture, selection, verified files and unresolved ranges. Only issue a
complete-selected-export receipt when all required files are verified. Missing
content never becomes an empty file. Preserve recoverable real files that cannot
be accepted as authorized changes, reporting them separately. Unsupported links
or unresolved dirty ranges leave the source intact and block a false completion
claim; never follow links to obtain export content.

Managed recovery must distinguish these classes before constructing the
destination:

| Source working content | Managed recovery treatment |
| --- | --- |
| Authorized but uncaptured edit/deletion at an observed writable path, or creation at an explicitly observed writable absence | Preserve in the managed working overlay and reconcile/capture using the applicable observation and existing acceptance checks. Uncaptured does not mean unauthorized. |
| Unknown/unobserved creation, modified read-only file, or original edit revoked by destination reobservation | Preserve as recovery-only content in a verified companion artifact outside the managed checkout, or leave the conversion blocked/unready. Never invent a path/absence observation or write grant. |
| Incomplete, unsupported or unverifiable effective content | Preserve the source and report the blocked recovery state; do not claim successful preservation/readiness. |

A companion artifact must have a durable receipt identifying original path,
content identity, reason it was not incorporated and its relation to the source
checkpoint/conversion. Retain its bytes under the recovery ownership mechanism;
do not expose them as accepted captures or silently omit them from the recovery
report. An ordinary managed destination may become ready only when its authorized
namespace is valid and the companion preservation is verified. Report outstanding
manual reconciliation separately; never claim all work was incorporated. If
those conditions cannot be met, keep the conversion blocked rather than marking
an invalid managed checkout ready. The source remains intact in either case.

Managed recovery is an explicit side-by-side conversion:

1. Require a stable source checkpoint and resolve conflicting source journals.
   Preserve pending publication requests at the source; do not transplant them.
2. Explicitly prepare any missing unchanged content required for a complete
   selected ordinary checkout. A failed preparation leaves recovery incomplete.
3. Create a fresh ordinary format-4 member/root in an initializing state, using
   the same shared family. Copy/hash the authorized effective working namespace
   with no writable hardlinks, preserving authorized uncaptured edits, creations
   and deletions. Preserve unknown, modified-read-only or revoked work through
   the companion/blocked-state rule above; do not place it in the managed
   namespace by inventing observations.
4. Extend the existing conversion/import-provenance mechanism to retain original
   snapshot/capture payloads and identities. Acquire destination-owned retention
   claims before readiness; do not rewrite source history to satisfy ordinary
   bootstrap checks. New captures use the destination identity. Preserve the
   existing requirement for a new local capture before publishing imported work.
5. Verify the authorized namespace, shared references, provenance, source witness,
   durable destination bytes and any companion recovery receipt. Publish
   readiness and a conversion receipt last only when those checks pass. Otherwise
   report a blocked state. Any interruption leaves the source intact and
   destination explicitly unready or resumable; source cleanup is not implied.
6. Prove that normal `ProjectionStore::open` recognizes the destination as an
   ordinary managed Worktree and supports offline status/read/capture and the
   supported prepared restore workflow. No running CFAPI provider may be needed.

The result remains a partial format-4 Worktree. Its exact remote Tree reference
does not justify admitting an incomplete native SCM graph. Selection/history and
recovery gaps stay explicit. Returning a plain directory plus manifest does not
satisfy managed recovery.

## Sequential dependencies and ownership

| Stage | Dependency and owner boundary | Completion evidence |
| --- | --- | --- |
| 0. Finish backend R2 fixes, freeze L3 corrections and qualify complete-store Windows VFS | Existing L3 correction owner and VFS integrator retain their files; parent coordinates. Schema 8/provider-capture extension remains pending; no concurrent extension. | Frozen interface revision, relevant passing checks and outstanding limitations recorded by owners. |
| 1. Versioned Store/evidence extension | Next sequential task after stage 0's relevant freeze; L3 owner controls core projection, private schema/migration, catalog/range APIs, capture persistence and retention/reconciliation. | Compatibility, deterministic evidence, lease, quota and crash tests; independent review. |
| 2. Narrow provider support | VFS owner controls control-directory/root proof, checkpoint changes, source contract and missing-range diagnostics. | Native root/control identity and no-hydration checkpoint tests. |
| 3. Partial adapter first delivery | New adapter owner controls new `projection_vfs_source` / `projection_vfs_worktree` modules and focused tests. Parent handles module registration and daemon/CLI/Desktop routing. | Fixed-selection mount, explicit preparation, capture/restart and recovery export tests. L5 remains open. |
| 4. Managed recovery and explicit selection additions | Scheduled after the shared seams stabilize; parent assigns conversion and transition ownership without overlapping active edits. | Ordinary managed destination and preserved-work expansion tests, including interruption replay. |
| 5. Platform and integrated qualification | Parent coordinates independent reviewers and test operator. Linux/FUSE is conditional after Windows qualification. | Dated matrix of passed/failed/skipped cases; no unsupported platform claims. |

Existing complete-VFS source/capture/worktree files stay with their integrator
until freeze. Share proposed signatures first; do not independently refactor
those files or the L3 acceptance engine. The parent owns decisions when the
frozen interfaces differ from this proposal.

Independent-review corrections remain assigned as follows; updating this document
does not complete or reassign their implementation:

| Priority | Contract correction | Implementation coordination |
| --- | --- | --- |
| P1 | Expansion preserves the existing epoch guard or reobserves the entire expanded scope at the same Step/root; revoked original edits are recovery-only, including on cache hits. | Parent schedules the expansion owner after shared-interface freeze. |
| P2 | Capture-version `Read` preparation resolves unchanged cold paths to exact base-Generation dependencies; missing captured replacements remain non-downloadable corruption. | L2 owns the online adapter correction and coordinates its dependency resolution with L3. |
| P2 | Capture availability applies write permission only to actual reconciled changes; unchanged read-only cold/full files are allowed. | L3 owns the common availability/evidence correction. |
| P2 | Managed recovery separates authorized uncaptured work from unknown/modified-read-only/revoked work through verified companion preservation or a blocked state. | Parent schedules the conversion/adapter owner; no invented observations or readiness. |

## Concrete acceptance matrix — all proposed, not run here

Use disposable roots and deterministic faults. Keep the existing catalogue,
path, file/capture-byte and change-count bounds; exercise boundary rejection
without broadening them implicitly.

| ID | Scenario | Required observation |
| --- | --- | --- |
| C1 | Mount a selected set containing a resident file, an unprepared file and an explicit absence; reconnect and inspect status. | Known names/sizes are complete; no content hydration for status/reconnect; missing size or incomplete metadata refuses activation. |
| C2 | Read missing, random, boundary, EOF and cross-chunk ranges. | Exact `needs_hydration` for missing bytes; no successful zeros/truncation. Valid resident ranges match expected content. |
| C3 | Explicitly prepare a missing file and an unchanged cold path addressed through an exact Capture version; disconnect the network and read/capture. Repeat with changed authority and a missing captured replacement. | Capture inheritance resolves to the recorded base-Generation dependency and activation map, never the latest head; the original Capture request remains bound. Authorized base preparation enables offline reads. Missing replacements are corruption with zero download attempts, even if equivalent remote bytes exist. Changed authority prevents acquisition acceptance. Callbacks issue no HTTP. |
| C4 | Pause a range reader while attempting eviction; inject corrupt/missing protected content. | Lease prevents reclamation during verification. Corruption/protected loss is not reported as an acquisition miss. |
| C5 | Capture untouched read-only cold and fully resident files alongside writable dirty files, ordinary replacements, deletion and authorized creation; then modify the read-only files. | Availability and acceptance allow both unchanged read-only classes without a write-permission blocker. Clean trusted cold placeholders cause zero content reads; fully resident/dirty files are entirely hashed. Only actual reconciled changes require write permission. Modified read-only/unknown/protected work is preserved and refused; missing backing content is never deletion. |
| C6 | Two Worktrees modify the same path differently, capture, stop and restart. | Both variants and private histories survive; immutable content is shared without writable aliasing. |
| C7 | Crash before/after registration, population and activation; replace the root/control capability. | Incomplete state remains unready or safely resumable; original-to-capture generation mapping and readiness commit together and replay without another generation. Source catalog stays fixed; no overwrite/adoption of foreign files or resurrection of ready-root deletions. |
| C8 | Crash after staging, shared installation/protection, private commit and before acknowledgement. | No dangling accepted reference; acknowledged or replayed capture has exactly one transaction receipt/sequence. Ambiguous stages remain protected/accounted. |
| C9 | Leave writers/mappings open, race namespace changes, inject disk-full/flush failures and cancel/stop the host during reads. | No false successful capture; real work preserved; bounded callback shutdown where the source honors cancellation. Record mmap/power-loss limits separately. |
| C10 | Trap ordinary scanner/materializer entry points; invoke status, capture, availability and Capture-version dependency resolution with unchanged read-only cold/full files. Restart, tamper with the activation map, and invoke unsupported mutations. | Provider paths never enter ordinary fallback. Availability agrees with reconciled capture permission decisions; base dependencies use the durable original-to-capture generation mapping and fixed catalog. A missing/mismatched map fails closed. Unsupported operations fail before mutation; immutable callback reads remain possible during staging. |
| C11 | Open the extended private schema with an old reader/reconciler; corrupt an owner record. | Reader refuses explicitly; reclamation fails closed. New fsck validates binding, journals, descriptors and private/shared ownership. |
| C12 | Export with missing unchanged content and with recoverable unaccepted work; interrupt export. | Source remains intact; no complete receipt for holes; recoverable real work is preserved and identified. |
| C13 | Convert with authorized uncaptured edits/deletions/creations plus an unknown creation and modified read-only file. Stop CFAPI, reopen offline, capture another authorized edit and use supported prepared restore. Inject faults, including companion preservation failure. | Authorized uncaptured work survives in a genuine ordinary managed format-4 Worktree with history/provenance. Unknown/modified-read-only/revoked work is preserved in a verified companion artifact with an explicit unresolved-work receipt, or conversion stays blocked/unready. No fabricated absence/write observations, captures or readiness. Source retained; ready-last replay is idempotent. Export-only success is insufficient. |
| C14 | Add selection through fresh-root/remount with existing edits, creations and deletions. Exercise unchanged-epoch guard success/failure and whole-expanded-scope reobservation at the same Step/root. Revoke an edited original path while its bytes remain cached; interrupt each phase. | No mixed-epoch destination is accepted. Guard mismatch aborts; reobservation covers every original and added path despite cache hits. Revoked original edits become verified recovery-only content or block transition, never authorized destination changes/captures. Authorized work/deletions and exact Step/root survive; added metadata appears only by explicit action, with its own activation map and no implicit byte downloads. Source retained. |
| C15 | Exercise rename/delete, open handles, mmap, watchers, flush, callback cancellation and restart on the supported Windows recipe. | Supported behavior is documented and verified; unsupported rename/dehydration paths refuse safely. Watchers/flush are never substituted for acceptance evidence. |

The first adapter delivery must pass C1–C12 within its stated support bounds.
L5 completion additionally requires C13–C15 and independent specification and
technical review. Failed or skipped native cases remain visible limitations,
not inferred successes from unit tests or the complete-store prototype.

## Limits

- This document implements and validates no new API, schema or OS behavior.
- Start with fresh local Windows roots, the provider's current ASCII/regular-file
  path restrictions, fixed mount identity and file-granular content admission.
  Symlink, hardlink, executable-mode, Unicode/alias and volume behavior outside
  the qualified subset must be refused or explicitly left unqualified.
- Keep separate nonnested family/checkout roots, private writable files, exact
  observed bases and current authorization checks for explicit network actions.
- No automatic dehydration, selection shrink, in-place root rewriting, native
  incomplete-graph admission, commits, feature installation or production-data
  conversion is authorized by this document.
- Process-crash recovery is not proof of Windows power-loss durability. Report
  provider-stopped and network-unavailable behavior separately.
- Linux/FUSE work starts only after Windows qualification and available
  prerequisites; do not implement around an unavailable OS feature. macOS and
  untested Windows versions/volumes remain unqualified.
