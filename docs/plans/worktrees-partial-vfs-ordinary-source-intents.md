# Ordinary outgoing source intents

27 October 2026. This increment connects the protected ordinary export owner to
a durable source journal. It covers Planned and cancellation before destination
allocation. Copying, Staging, Ready, Published and resource-bearing cancellation
remain separate driver integration work. Runtime evidence belongs in the main
checkpoint; the source contract below does not itself establish qualification.

r28-r29 qualification is now recorded in the
[checkpoint](worktrees-partial-vfs-checkpoint.md): ten pure/SQL passes on r28,
then the corrected complete native lifecycle on r29 (341.302 s outer). r28's
native witness failure and its independent diagnosis are preserved. Only the
test changed for r29. This proves orderly close/reopen and preallocation cancel,
not process-crash recovery or a resource-bearing destination lifecycle.

## Incoming and outgoing domains

Private9 resolves its immutable incoming Ready by the actual admission record's
transaction ID, Ready digest, receipt ID and prefix ID. Prefix reuse re-reads the
bounded actual admission metadata in the same snapshot; it never recursively
calls the full admission reader. Fresh private8 destinations keep their original
whole-table singleton rule.

All remaining transition rows are then interpreted as outgoing ordinary source
plans. Before broader historical and foreign-key interpretation, an envelope
bounds the complete inventory to 4,096 rows and 16 MiB of normalized columns.
The complete reader authenticates every canonical row and SQL projection,
source FORMAT/database/admission binding, snapshot/selection and capture frontier.
There can be only one live Planned intent. Cancelled rows may retain an older
valid frontier after ordinary generations and captures advance.

At this boundary, outgoing rows must have no destination proof, reservation or
receipt. Other phases are refused until their actual resource-aware driver exists.
These metadata checks grant neither bytes nor destination/family authority.

## Actual writer

The owner derives a plan using its protected image and independently pinned
parent. It then revalidates the same image, admission, source prefix and exact
retained contents under an Immediate transaction. The SQL primitive performs
full source-closure verification and exact insertion/replay; a different plan
under the same ID, the incoming ID or a second live intent refuses.

Initial insertion also refuses competing ordinary budgets, prejournal staging
and acquiring preparations. It reserves enough metadata capacity for the actual
encoded Cancelled candidate, whose payload and state projection are larger than
Planned. This preserves cancellation at the per-record and aggregate limits.
The stored row remains the exact Planned body.

Commit is observed explicitly. An error also observes rollback and preserves
both errors when necessary. Normal source-copy callbacks remain rollback-only.
The owner keeps its pins until positive close; no destination is created here.

## Reopening and ordinary mutation barriers

Ordinary idle/mutation gates refuse a live outgoing intent. Admitted restore
recovery checks the same barrier during clean read-only preflight and fresh
writer validation. Ordinary Exclusive access skips implicit committed-budget
settlement while a valid live intent exists, so a refused mutation does not
first change unrelated source/family metadata.

`ProjectionStore::open_for_managed_export` supplies a separate genuine private9
Shared/read-only opening route, avoiding the ordinary `state()` idle gate.
It revalidates the actual admission and current platform snapshot; it does not
migrate legacy formats, settle budgets or replay SQLite/filesystem recovery.

`cancel_managed_export_plan` uses the same dedicated access purpose in Exclusive
mode. It validates and compare-and-swaps only a no-resource Planned record to
Cancelled, positively closes the database, preserves external checkout edits,
and retains terminal history. Retrying Cancelled performs no operation against
later ordinary work, including a still-unsettled committed capture budget.
This requires skipping implicit settlement even when no live export remains.

## Remaining driver work

Attach destination allocation and dedicated C family authority to the durable
plan, then add source phase writers with actual destination/reservation proofs.
Reconstruct ordinary-source Ready, perform the independently opened Published
handoff and admit C. Existing Provider proof paths remain strict and separate.
The [driver plan](worktrees-partial-vfs-ordinary-export-driver.md) records these
dependencies and end-to-end acceptance. No process-crash or post-allocation
cancellation qualification is inferred from orderly close/reopen.
