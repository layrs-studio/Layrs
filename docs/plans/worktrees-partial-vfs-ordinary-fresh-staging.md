# Fresh Ordinary Staging to Ready: bounded implementation proposal

Source investigation, 28 October 2026. Implementation candidate r70 now exists
as `OrdinaryStagingPromotion`, with extracted physical image kernels and two
fresh-fixture recipes. Independent source review r70 found no actionable defect;
build and workspace/all-targets check r71 pass. ROOT released seven exact r72
recipes after matching all 661 retained build inputs. The first five regressions
pass; the first native fails with stack overflow0xc00000fd after642.474s; the
second is not run. Evidence is archived. r77 reproduces overflow at the nested
validated read after setup and the real image-drift refusal. A private
Box<Validated> return representation passes build/check r78 and reduces measured
compiled nested frame storage. Both r79 native recipes pass (599.491s/598.976s)
with unchanged stack limits; evidence is archived. This qualifies those exact
clean-teardown and withheld-ACK paths, not actual OS-close failure or crash recovery.
This follows the separate fresh Ready-to-Published adapter. It does not change
that adapter's explicit refusal of SourceStaging. Paths below are relative to
`crates/layrs-store-local/src/projection/`.

## Authority to reconstruct

Fresh Staging promotion requires B's actual unchanged current selected image,
fresh exclusive B ownership, independently authenticated C8/family Ready and
complete original fixed funding. It does not require reconstruction or adoption
of `ManagedDestinationAllocation`.

`provider/managed_export/image.rs::OrdinaryExportImage::build` already scans the
selected namespace, rejects protected/aliased paths, pins single-link leaves,
hashes exact deterministic manifests and retains explicit deletions. Its durable
image digest commits path, FileId, length, executable flag and deletion base
FileId, but not old per-file inode identities. New physical pins can therefore
re-establish the same image commitment.

The current API requires `ProjectionStore` and genuine `format::ContentScope`.
Passing no scope to `capture/scan.rs::scan_readonly_with_scope` routes through
`require_ordinary`, which correctly refuses admitted private layouts. Do not
fabricate Access, ContentScope, Store or a caller-supplied validation callback
to bypass this boundary.

## Smallest source change

Extract a filesystem-only selected read-only scan core taking root, authenticated
snapshot and selection. Extract image `build_parts` / `revalidate_parts` taking
root, actual marker, authenticated snapshot/selection and cancellation. These
produce physical image proof only. Existing retained wrappers must preserve
their real scope validation before calling the extracted cores.

Preserve all selected-path bounds, metadata limits, ASCII-case alias rejection,
reserved/unavailable-path rules, ancestor/namespace checks, reparse refusal,
single-link and immutable-leaf checks, deterministic hashes and deletions.

A new `provider/managed_export/staging_reopen.rs` can compose these private
kernels with the exclusive source capability and shared C inspection. Suggested
operations are `open(B_root, C_root, transaction_id, cancel)` and consuming
`promote_ready(close_observer, cancel)`; failure retains holders and step.
The transaction ID selects stored evidence, never grants authority.

## Validation sequence

1. Acquire shared family lifecycle, fresh B Exclusive, then fresh C Shared.
   Authenticate actual B SourceStaging without an outgoing receipt, its full
   admission/live frontier and actual current snapshot/selection in a clean RO
   preflight. Positively close before filesystem hashing.
2. Build fresh B image pins and require their digest to equal the stored source
   version's effective-image commitment. Changed bytes, deletions, mode or
   selected namespace refuse. Retain pins until all mandatory closes finish.
3. Use `import_validation::verify_source` with AdmittedOrdinary and this newly
   witnessed digest to authenticate history/ancestry, current frontier and
   closure. Never use the stored digest as its own independent witness.
4. Derive exact copied-content mappings and selected-base mappings through the
   existing `include_selected_bases`. Select matching canonical manifests from
   C inspection's fully interpreted and CAS-pinned `required()` union by exact
   `(FileId, ManifestId, length)`. Missing representation refuses; another
   manifest for the same FileId is not a substitute. No generic lease required.
5. Reuse `reserve_plan::derive`, `OwnedOrdinaryReadyData::new` and
   `content::expected_checkout`. A canonical destination-Staging projection is
   calculation DATA only; actual C is observed as Ready. Compare the complete
   immutable plan, original quota, exact union, metadata envelope, Ready/receipt
   and checkout inventory/deletions/journal with genuine C/family interpretations.
6. Under catalog gate and independent C/family RO scopes, close the B clean
   preflight before opening its FK-enabled writer. Do not hold an independent B
   RO lock through B COMMIT. Reconstruct transaction-bound source proofs inside
   the writer, call existing `ordinary_transition::apply_ready`, validate the
   prospective state, COMMIT and observe positive B close. Enclosing readers use
   strict failure propagation, then close before gate release and fresh readback.
7. Full source namespace/hash checks stay outside catalog/SQL scopes. Use retained
   image and C identity checks inside them. Source image and C checkout/CAS pins
   span mutation and closes. Ready and Published remain separate transactions;
   no chaining after an uncertain boundary.

## Qualification to add

The fixture must stop after C8/family Ready but before B Ready, positively close
the old B owner, drop all image/CAS/checkout/allocation/store holders and return
only DATA. Reopen only after that teardown. Cover unchanged-image success;
changed/missing/aliased/protected B paths; exact-manifest substitution; altered
plan/history/selection/receipt/family; and the exact receipt insertion/source-row
effect. Preserve B ingress and C/family evidence.

C's verified copied image cannot impersonate a B-bound `OrdinaryExportImage`.
Allowing changed B to publish using only C would require a separate reviewed
contract. Prefer fresh actual B verification and refuse changes for this unit.

External namespace quiescence still applies to absent names and pre-existing
writable mappings. Clean teardown is not crash recovery. Hot/cold journals,
interrupted copying, failed native close quarantine and actual owner loss remain
separate work. This note records source reasoning, not executable proof.

## Next fresh entry before this adapter

Read-only investigation after r72 launch identifies the closest missing step:
actual B SourceStaging, C8 private Ready, family member Initializing. Publish
only the family member Ready, then the existing fresh Staging adapter can run.
The r82 source candidate now implements this clean-holder unit; compilation,
critical review and runtime qualification are still pending at this checkpoint.

`managed_ready_reopen/ordinary_source.rs::OrdinaryExclusiveSource` already gives
fresh B ownership. `ManagedReadyPins::open` supplies existing-file identity
mechanics, but the current `OrdinaryReadyInspection` requires family Ready.
A separate C inspection must authenticate the actual stored destination proof,
FORMAT/database/control/member identities and Initializing state after B ownership;
it must not weaken the existing terminal Ready constructor.

`destination_ready/ordinary.rs::read_ordinary_checkout_ready_for_publication`
already accepts Initializing or Ready with actual B Staging, full copied/private
suffix, journal and protected family accounting. The exact member-state CAS is
`shared/admitted_catalog/export_publication.rs::publish_export_ready`. Reuse
these contracts after independent B image/source-closure reconstruction, exact
plan/funding/union derivation and fresh C CAS/checkout verification. Retain all
owners and physical proofs through family COMMIT and positive close; perform
fresh readback afterward. Actual family Ready replay should need no writer.

Earlier Copying/Staging/install/checkout phases still lack their own fresh
physical entries even where SQL phase readers exist. Never reconstruct
`ManagedDestinationAllocation` from paths: its contract forbids adoption after
holder loss. Resource-free Planned cancellation exists; Planned with partial
allocation artifacts lacks a durable destination proof and cannot use it.
Hot/cold rollback journals are refused by clean_admission. Observable native
close failure may retain process-lifetime quarantine; dropping a holder does not
establish that it cleared. Those are separate prerequisites for crash recovery.

The narrower decomposition investigation recommends an internal
`PrivateReadyParts` DATA kernel (actual prefix/suffix/Ready/receipt, exact union,
checkout/journal, canonical family/protection fields and true metadata floor),
then a distinct `VerifiedOrdinaryFamilyPublication` reader/physical holder.
Existing `VerifiedManagedReady` and `OrdinaryReadyInspection` remain fixed to
family Ready; `terminal_ready.rs` and generic membership checks stay unchanged.
The new entry observes the exact closed ExportPublicationMember phase, never a
general permissive boolean on terminal constructors.

Provisional parts cannot expose terminal capacity or physical authority. The
new entry must finish through the existing actual-B-backed
`read_ordinary_checkout_ready_for_publication` / `read_export_publication` full
catalog gate. This preserves known B empty groups, exact C-root deferral, shared
debit sets and all-reservation classification. An empty C union still has its
authenticated root/reservation; unknown foreign empty groups remain refused.
CAS/checkout verification then occurs outside SQL, followed by a full reread.
Family Exclusive/Immediate repeats the actual B/C/full-family interpretation
before and after the exact member CAS; successful closes precede fresh readback.
Keep summaries boxed through nested readers. Suggested separate ownership:
SQL/private DATA extraction in autonomous.rs/family_read.rs and a new reader;
physical driver in `managed_export/family_reopen.rs` with preliminary C resources
in `managed_ready_reopen/ordinary_family.rs`. The r82 candidate follows this
decomposition, adds exact-set comparison independent of input manifest order,
and extends the dedicated SQL publication recipe plus two new native recipes.
Independent critical review r82 is complete, with no concrete protocol defect
identified and one SQLite/OS-close evidence wording issue corrected by ROOT.
These source additions pass build/check r83 and four SQL regressions in r84.
The r84 runner executes the corrected source10 recipe before the two fresh
family natives; no family native result is claimed yet. The next private Ready
predecessor is described in `worktrees-partial-vfs-ordinary-fresh-private-ready.md`
and implemented separately from the retained r84 executable.
