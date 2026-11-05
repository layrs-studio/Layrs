# Prepared reopen and resident reads — 26 October 2026

This follows the [native preparation checkpoint](worktrees-partial-vfs-preparation-checkpoint.md).
`PartialStoreFamily::open_provider_prepared` reconstructs a distinct owner from
exact durable A/P, complete private/family SQL validation and native READY.
It makes no write report for a past invocation. It never registers or repopulates
the root, preserves user deletions, and refuses pending even when native READY
exists. Root-ID discovery happens after the complete schema and payload fences.

Both prepared holders expose `read_resident_range`. Shared lifecycle, owner and
catalog locks cover validation, content verification, observable SQLite close
and final copy. Reads use only the restricted readonly database route; missing
content never invokes recovery or a download. The unclaimed initializing owner
and full family checks distinguish an absent manifest from missing/corrupt
accepted content. No caller buffer is modified on failure.

The range is bounded to 256 KiB. A local manifest matching the authenticated
source descriptor allows full verification of the intersecting chunks. A
different local representation is supported by hashing the entire file and
checking its FileId before returning bytes. Chunk hashes alone cannot establish
this relationship. The fallback may read up to 1 GiB per range; its scratch
buffer remains 64 KiB. A retained verified-representation cache is a possible
later optimization, not an existing guarantee.

Cancellation is checked between file reads, descriptor comparisons and major
validation operations. A scoped SQLite progress callback checks every 128 VM
operations and is removed before observable close, including on unwind. Cleanup
failure remains a cleanup failure even if cancellation also occurred. Bounded
Core/VFS constructors and synchronous Windows filesystem calls are not
preempted; no strict worst-case cancellation latency or callback-drain guarantee
is inferred from these tests. No `SnapshotSource` or connected product host is
issued by this increment.

## Evidence

- `preparation-reopen-20261026-r1`: four native reopen recipes and ten SQL
  recipes pass, including rejection of a substituted mount view before decode.
- `preparation-ranges-20261026-r1`: an additional canonical alternate ticket
  passes SQL validation and is refused by the actual native READY proof; three
  initial range recipes pass.
- `preparation-cancellation-20261026-r3`: 82 pure provider tests, ten exact SQL
  recipes and eight isolated recipes pass. The latter comprise cancellation SQL,
  five range cases and two writer regressions after the readonly refactor.
  The workspace check with all targets also passes.

All packets live under `.layrs-local/validation/`; native fixtures use fresh
owned C: temporary roots and existing bounded children. The final retained
executable SHA-256 is
`148ee0e68d1b61672347a2e057e2af9c958258d5add448c624511c9fb5cf9f2c`.
The snapshot contains 381 source files, EXE/PDB binding, startup and native-input
pins. Independent review closed source-identity, SQL-query bounds, callback
lifetime and descriptor-loop cancellation findings. The same explicit stock
toolchain/loader trust assumptions as the earlier checkpoint apply.

The initial cancellation packet retains a failed pure fixture test: it excluded
an explicitly observed absence. Only that test selection was corrected. A
same-process lock recipe was corrected before execution to expect the existing
reentrant-lock refusal. Its passing result establishes local lease retention
and later release; it does not qualify cross-process contention.

## Activation remains in progress

Pure codecs now define the exact source-generation-0 to activated-generation-1
mapping and a `mount.v3` tied to the complete prepared v2 predecessor. Only
selected file placements change from unavailable to materialized; observations,
rights, selection, targets, absences and exclusions stay fixed. Materialized in
this provider transition describes placeholder installation, not resident bytes.
Three pure tests pass and independent source review found no blocker in this
data contract.

The full Active SQL interpreter, private mapping commit, ready-last family
transition, closed-source host integration and activation recovery are still to
be implemented. Capture, managed/companion conversion, selection expansion and
CLI/Desktop qualification remain open. These data codecs do not grant activation.
