# Ordinary managed export owner

27 October 2026. This increment supplies the physical source of an ordinary
schema9 re-export. It does not allocate, publish or admit destination C. Runtime
dispositions are recorded in the main Worktrees checkpoint. All 17 released r27
recipes pass, including the genuine native source owner (285.171 s outer).
r26 was audited but not executed; its test assertion was corrected before r27.
This evidence does not qualify destination allocation, publication or admission.

## Ownership and transfer contract

`ProjectionStore::begin_managed_export(expected_generation, destination, cancel)`
requires a genuine admitted ordinary source, a distinct destination in the same
store/space/origin/principal context, and the exact current generation. It retains
exclusive `format::Access` and immutable Windows file handles. Every operation
recreates admission/history evidence inside its actual private SQL snapshot;
neither a `ContentScope` nor a transaction escapes the callback.

The selected image records exact file manifests, executable modes, expected bases
and deletions. Compatible readonly rescans verify the namespace, root, selection
and snapshot around transfers; retained handles verify identities, single-link
files and complete bytes. Windows sharing excludes ordinary write/delete opens
against pinned leaves. This does not freeze absent names or arbitrary namespace
mutations: external writers must remain quiescent.

Two independent sets contribute retained transfer requirements:

- Exact historical mappings authenticated by the outgoing prefix, including
  restore-only preimages absent from captures and current checkout bytes.
- Selected snapshot bases resolved through exact private `projection_content`
  mappings under the retained content scope. An unchanged base need not occur in
  historical captures. Adding these requirements does not alter the prefix's
  historical mapping commitment.

All retained metadata is bounded before byte verification. Full shared-file
verification polls cancellation through its sink at each bounded chunk (at most
1 MiB), with checks around the stream. Readonly checkout scans poll per directory,
entry and 64 KiB read, including EOF. Existing ordinary capture callers retain
their original API and no-token behavior. This is cooperative cancellation, not
forced interruption of a blocked synchronous OS read or SQLite operation.

Current-image and retained copies support full files and exact chunks. Failure
may leave a prefix in the caller's sink; the caller must keep it unpublished.
Only successful whole-file verification establishes the assembled FileId.
`close(self)` positively finalizes the private database while pins remain alive;
dropping the owner does not acknowledge successful closure. SQL rollback and
combined transfer/postcheck errors remain observable.

## Planning boundary

The internal ordinary adapter derives a DATA plan from the current snapshot,
selection, protected image, independent bases and exact historical mappings.
The Provider adapter retains its fixed observation, activation and selection
checks. No Provider activation proof is manufactured for an ordinary source.

The plan deliberately carries no validated reservation metadata floor. The
future writer must derive the complete envelope from its actual source and
destination intents before reserving space. B's admitted lease authorizes B;
it cannot substitute for C's family import authority.

## Required next integration

1. Preallocation source intent and cancellation are now qualified in r28-r29,
   with incoming admission anchored to its original Ready transaction/receipt.
   See the [journal contract](worktrees-partial-vfs-ordinary-source-intents.md).
   Extend that journal to Copying and later resource-bearing phases.
2. Bind allocation to the retained parent identity and ordinary source owner;
   obtain dedicated destination import authority and exact reservations.
3. Connect content production, physical copy, staging, Ready and publication.
4. Independently reopen and authenticate the actual published source during C's
   admission; support ordinary-source reconstruction without Provider handles.
5. Reopen C with A and B unavailable, exercise inherited restore inverses and
   positive publications from multiple committed segments.

Companions, selection expansion, remaining recovery/lifecycle/GC, performance,
CLI/Desktop and final programme acceptance remain in the full active goal.
