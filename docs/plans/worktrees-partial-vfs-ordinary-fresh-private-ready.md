# Fresh Ordinary private Ready finalization

Source integration, 28 October 2026, after the r82 family-publication review.
The three bounded implementation tasks and independent critical source review
r85 are complete. No concrete actionable defect was established. The integrated
r85/r86 candidate compiled and passed workspace/all-targets in r87b after ROOT
corrected one test pattern. The four short r88 SQL recipes passed; the first
native failed in its setup receipt witness before the fresh driver. Four later
natives were not reached. The failed run is archived. ROOT corrected the witness
and split allocation timings; build/check r90 PASS, integrated review r89 found
no concrete defect. ROOT75810/r91 now runs the retained corrected candidate:
two affected SQL recipes PASS, fresh private success/replay PASS585.368s and
fresh private ACK/replay PASS571.530s. Retained private ACK PASS559.386s;
family Updated PASS573.955s and family ACK PASS570.455s. ROOT75810/r91 is terminal
exit0 with all seven recipes passed and archived. These outcomes do not establish
actual native close failure, crash recovery or untested checkout shapes.
Source review and build base: `f01f6b9518b71a3cfc309be3a2911c51599f2e79`.

Review manifest: `D:/Layrs/tmp/provider-ordinary-fresh-private-ready-review-r85-inputs.json`,
50 sources, 13 changed paths, SHA256
`ed6167d858eec42dda2038e75ed821d43a62071af4266114d82dee5e6a0b3c1f`.
Diff SHA256 `9f9d8b873a8274c6486f266c15331f3e3b95791cbb2a88d66ec62aa1a5f8f00e`.
Baseline is the retained r84 source tree; unchanged context matched after newline
normalization. ROOT73332/r84 finished with seven PASS results. ROOT38516/r88
is terminal exit1; ROOT75810/r91 is terminal exit0 with seven PASS. The separate
checkout runner ROOT86893/r94 is now active. Never reuse r88
fixtures or rerun its scripts. A fresh retained revision is required.

Implemented interfaces: `read_ordinary_checkout_finalizable` returns a boxed
complete journal after actual Staging, full family and future C10 budget checks;
`read_manifest_exact` returns exact-ID SQL DATA under a retained transaction;
`OrdinaryPrivateReadyFinalization` owns fresh B Exclusive/C Exclusive resources
and physical proofs. Actual Staging and actual Ready use separate complete
readers. The Ready branch explicitly requires the family member Initializing.

The two proposed native recipes use distinct genuine checkout-complete setups,
explicit old B close and all-holder teardown, then cover success/replay and a
withheld ACK after SQLite connection-close success. They also inspect real B/C
byte drift, C Exclusive contention and pinned bytes. Incomplete journal and
exact-manifest/corrupt family refusals are covered by the SQL recipe that passed
in r88, not by the native recipes. The r88 native failure is a setup witness
error: C correctly inherits B ingress receipt. No fresh-driver success or actual
xClose/OS-close failure behavior is established at this checkpoint.

The follow-up retained ReadyContext correction and test-only phase timings are
integrated as r86 passed independent critical source review. Six paths differ
from r85; the fresh private product driver itself is unchanged. The r86 review
selects 60 sources, manifest SHA256
`d9684350c299384cc3006d42c1a60efa45e58671d5a4a996de32a86ccce6275d`.
The corrected candidate requires new runtime qualification. Timing events describe
scope duration, including unwind; nested scopes overlap and END is not success.

## Exact scope

Actual B SourceStaging, C8 DestinationStaging, family Initializing, complete
protected CAS and a complete published checkout journal become private C Ready.
B and the family member do not advance. Reopen only after clean teardown of all
old owners, image, allocation, CAS and checkout holders. No allocation adoption,
lock upgrade, native-close quarantine repair or crash recovery is inferred.

The existing `OrdinaryFamilyReadyPublication` begins one phase later, when C is
already private Ready. Its provisional private reader must not be generalized
to pretend that Staging has a Ready suffix or receipt.

## Existing kernels and missing boundary

Paths below are relative to `crates/layrs-store-local/src/projection/`.

- `provider/ordinary_destination/checkout.rs::read_checkout_staging` authenticates
  actual B SourceStaging, derives ExpectedCheckout and checks the copied prefix,
  operational tables and typed journal. It permits legal intermediate journal
  progress: a finalization entry must explicitly require `is_complete()`.
- `provider/destination_ready/ordinary.rs::finalize_ordinary_checkout_or_replay`
  already performs the exact Staging→Ready suffix mutation and pre/post SQL
  validation. Ready replay returns before mutation. Do not call this writer as
  a read-only probe: the Staging branch really invokes append_suffix.
- Extract a separate read-only finalizable entry using existing
  `prepare(PrivateReady, Staging, …)`, the actual checkout-Staging reader and
  journal completion. Preserve exact union, floor, protection and full family
  accounting. Its result is detached DATA, never a fabricated terminal holder.

## Exact manifest reconstruction before Ready exists

Use actual B's verified historical mappings and `include_selected_bases` after
fresh B image authentication and `verify_source`. Fetch each canonical manifest
by its exact ManifestId, check FileId/length/body, bound individual and aggregate
bytes and observe cancellation. These bodies are provisional SQL DATA until
the complete family and physical checks finish.

`shared/content.rs::read_manifest_optional` already preflights payload bounds,
decodes canonical bodies, matches the exact ID/index and validates chunk rows.
It is private; a narrow internal extraction can reuse it. Do not use
`manifest_for(FileId)`: that selects the first representation in ID order and
can substitute another canonical representation of the same file.

Derive the original fixed plan with `reserve_plan::derive`, then
`content::expected_protection`, `OwnedOrdinaryReadyData::new` and
`content::expected_checkout`. The full
`AdmittedFamilyContext::validate_export_protection` must report protection
present, with C Initializing and complete retained groups/debits/reservations.
Keep the future C10 admission budget checks, not only current Ready metadata.

## Fresh physical owner and writer sequence

Suggested closed API: `OrdinaryPrivateReadyFinalization::open(B,C,tx,cancel)`
then consuming `finalize_ready(close,cancel)`, returning ReadyEffect and exact
Ready/receipt DATA or an error retaining step/cause/held.

1. Acquire B Exclusive and authenticate its unchanged SourceStaging/current
   frontier. Independently scan and pin current B bytes after SQL closes.
2. Use a distinct C resource factory from the B owner, checking aliases and
   family before acquiring C **Exclusive from the start**. The r82 Shared
   resource holder cannot be upgraded. Never manufacture
   ManagedDestinationAllocation from paths; its contract forbids adoption.
3. Complete all actual B/C/family SQL checks, positively close the scopes, then
   verify/pin C with `verify_published_cas` and `verify_managed_ready_checkout`.
   The latter consumes ExpectedCheckout plus a complete journal; its name does
   not itself require SQL Ready. Repeat the complete read after physical checks.
4. Retain B/family strict RO transactions, close the C preflight before opening
   C's FK/Immediate writer, and repeat full pre/post SQL checks around the
   existing finalizer. No independent C reader survives to C COMMIT.
5. Explicit rollback on active failed transactions, observable SQLite close,
   enclosing positive closes, gate release, then independent full readback.
   Any earlier error stops further probes except mandatory cleanup. Keep the
   original cause and all owners/image/CAS/checkout pins on failure.

For actual private Ready replay, the r82 complete family-publication reader can
be reused with an explicit `member == Initializing` requirement. Staging and
Ready remain separate closed branches, each fully validating its actual phase.
No terminal/admission gate needs weakening.

## Qualification fixture and retained-path follow-up

Add a genuine setup stop after `copy_checkout` and before
`finalize_destination_ready` in the source-publication test helper. Use a
checkout-specific fixture without fields claiming an observed Ready. Explicitly
close old B and drop every other holder before returning owned DATA/paths.

Cover normal finalization plus no-writer replay, withheld ACK after SQLite close
success, real B drift, incomplete journal, exact-manifest substitution, missing
or corrupt family group, and changed C bytes despite a complete SQL journal.
Only separately executed cases establish runtime qualification.

Do not copy the old retained `ReadyContext` as a fresh capability: it still owns
Access/allocation. Source inspection also shows unconditional post-result
`boundary` calls in `read_current` and `finalize`, non-strict readers, and checkout
namespace validation under the catalog gate. These are an explicit follow-up
audit/fix item for retained-path failure ordering; the new fresh driver must use
strict error propagation. No runtime counterexample was executed by this
investigation, and no broad retained-path correctness claim follows from r82.

The original investigator was read-only. Implementation followed in three
separate bounded source-only tasks; ROOT integrated the contributions and froze
the review inputs. ROOT's current native source10 run is independent of r85.

### Retained ReadyContext follow-up, source-confirmed ordering

ROOT traced the old path without executing it. In `managed_export/ready.rs`,
`finalize` propagates a failed `finish_writer` into the enclosing non-strict
`read_cancellable` callback. After the mandatory reader close,
`database_io::read_inner` in Interpret mode still calls `database.snapshot()`
before returning/classifying that callback failure. A failing concluding
snapshot can replace the original callback cause with Unavailable. The same
pattern surrounds the retained family writer. This is source control-flow
evidence, not an injected runtime failure.

In addition, `read_current`, `finalize` and `publish_family` evaluate their
concluding `boundary` even when their recorded result is Err. That boundary
calls checkout `require_parts`, which performs physical namespace validation;
the catalog gate is still retained. The correction should use strict nested
readers, propagate prior errors before optional concluding checks, and split
identity-only checks from full namespace/byte validation outside catalog gates.
Preserve existing full SQL validation, explicit rollback/close and holder
lifetimes. Add focused evidence that an ACK error preserves its primary cause
and prevents later observed filesystem/SQL probes. Do not modify these frozen
r85 context files while its independent review is running.
