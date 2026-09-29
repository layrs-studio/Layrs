# ADR 0014: Native Product CAS and the zero-Lens merge host

Date: 2026-09-20

Status: accepted for the bounded native subset below

## Context

ADR 0010 requires a canonical Weave receipt to accompany acceptance. Operations
are local history and do not carry remote Product receipts in Sync V3. Storing
the receipt only in Operation metadata would break independent publication,
admission and retention of the accepted result.

## Decision

The native CAS has a typed Product object containing the exact canonical V3
envelope. Its object ID, receipt/profile/execution identities, direct Step and
Conflict dependencies and Space admission are validated. A native Weave commits
its Step, Product, View and Operation in the same single-writer transaction.
CAS bytes are installed first; failed transactions may leave unreachable bytes,
never published metadata pointing to missing bytes. All admitted Products are
retained as GC roots independently of Operations.

This requires native FORMAT version 2, minimum reader version 2 and SQLite
baseline `native_scm_v2_products_baseline` (schema version 2). The published
`scm-v1.sql` fixture stays unchanged. New stores use `scm-v2.sql`; older native
stores are explicitly refused without modification. There is no automatic
upgrade, no legacy compatibility and no converter in this change. Opening an
existing user project must never initialize or replace it after this refusal.
The Rust module name `scm_v1` remains an API name, not a format-version claim.

The admitted Product subset is root Layer, Marker, Release and ungated Weave
with no Product dependencies. Logical/versioned Product dependencies and gated
receipts are explicitly rejected until their complete closure is implemented.

## Actual execution guarantees

The initial accepting host is `layrs.no-lens-execution`, version 1. Structural
preview disables all Lens calls. A usual preview is also acceptable if it made
zero Lens calls. Any actual Lens execution or inherited Lens conflict is refused
by this host. The deterministic planner enforces byte/count/depth/work budgets.

`maxMemoryBytes=1` and `maxExecutionMs=1` in this profile are unused budgets for
Lens invocations, of which there are exactly zero; they are **not** a statement
that the graph planner runs within one byte or one millisecond. No whole-process
memory/deadline attestation is manufactured. Semantic acceptance needs a separate
host that really constrains its worker and records the execution proof.

The additional `layrs.wasmi-no-imports-process` v1 host executes an actual
`wasm32-unknown-unknown` code/text module with no imports or start function.
Wasmi fuel and linear-memory bounds sit inside a Windows Job with global
committed-memory, process-count and deadline enforcement. A trusted deployment
supplies the worker SHA256; image and ancestor handles prevent replacement.
The Lens pin is the exact module SHA256. The private completed planner profile
commits worker/module digests and all host budgets, and supplies actual
memory/deadline/fuel to the canonical receipt. The graph planner itself is still
governed by its deterministic byte/count/depth budgets, not by the Lens process
memory claim. See `crates/layrs-weave/WORKER.md` for ABI/build/limits.

Native admission accepts only this supported semantic profile and exact module
pins. A Lens-derived structural Conflict must have exactly one matching receipt
execution and pin. Pure in-process semantic preview cannot produce the private
completed host evidence used by native acceptance. This is local enforcement;
remote receipt validation checks declared identities and policy, not a hardware
proof of execution. Authenticated remote author admission remains necessary.

## Worktree projection and recovery

Acceptance compares the exact View, Operation frontier and Worktree active
Operation from the preview. The caller captures local edits before creating the
preview. Clean active results use the recoverable checkout materializer. An
unmaterializable clean tree is refused before publication.

An accepted conflicted result advances the Layer head and persists every
canonical conflict, but keeps the old Worktree base and materialized Tree and
marks the Worktree Conflicted. No conflict markers or invented projection are
written. Resolution acceptance is a subsequent descendant `ResolveConflict`
operation; it creates no second Weave receipt. Explicit manual replacements
provide exact bytes and an executable bit, create File manifests/chunks and a
descendant Step, and preserve the original conflict and receipt in history.
They are later user edits, not a retroactive change to the Lens execution's
recorded resolution policy. Input and encoded-output budgets are checked before
copying replacement bytes. Partial whole-term selection or file replacement
retains the old projection and Conflicted status; complete selection materializes
the exact resolved Tree and marks the Worktree Ready. Preview alone never claims
that resolution has been installed.

Preview owns a stable mutation identity: a lost commit response returns the
same accepted operation and receipt on retry. Checkout crash recovery and CAS
admission remain distinct: recovery must complete before further checkout work.

## Remaining boundaries

Rich and multi-placement conflicts, block-level resolutions, Product
dependency closure and gated receipts remain unsupported. Semantic process
supervision on Linux/macOS and trusted worker packaging/distribution are pending.
Sync transports structural conflicted Weaves with their complete supported
closure; semantic receipt transport requires matching server admission support.
