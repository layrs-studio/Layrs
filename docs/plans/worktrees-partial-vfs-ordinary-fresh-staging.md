# Fresh Ordinary Staging to Ready: bounded implementation proposal

Source investigation, 28 October 2026. Not implemented or runtime-qualified.
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
