# ADR 0006: Steps are immutable content-addressed snapshots

Date: 2026-07-19

Status: accepted

## Context

Layrs must preserve work continuously without introducing a public commit
ceremony. A Step is therefore a source-control snapshot, not an automation
task, a release, a review decision, or a mutable record of execution.

The previous model attached Steps to Layers, assigned them a position in a
Layer timeline, allowed only one parent, and mixed snapshot identity with
capture metadata. That model cannot represent a repository-wide DAG, replay,
multi-head Layers, persistent conflicts, or deterministic deduplication.

## Decision

A Step is a globally addressable, immutable Merkle object with exactly these
identity fields:

- an encoding version;
- a set of zero or more parent Step ids;
- one root Tree id.

The Step does not contain a Layer id, name, message, timestamp, actor, device,
capture reason, sequence number, mutable status, or rewrite information.
Those facts are observations or relationships around the Step and do not alter
the snapshot's identity.

The version 1 identity domain is the ASCII string `layrs-step-v1`. The
canonical payload is UTF-8 and uses LF only:

```text
parent-count\t<N>\n
parent\t<PARENT_STEP_ID_1>\n
...
root-tree\t<ROOT_TREE_ID>\n
```

Before encoding, parent ids are validated for uniqueness and then sorted by
their ASCII byte representation. A duplicate parent rejects the Step; it is
never silently deduplicated. Parent order has no semantic meaning. A parent
equal to the resulting Step id is invalid. Cycles are rejected when the Step is
admitted into a repository graph.

The Step id is domain-separated from every other object type:

```text
blake3:<lowercase hex BLAKE3("layrs-step-v1" || NUL || canonical payload)>
```

The NUL is exactly one zero byte. The identity domain is not repeated in the
payload. A different domain or version always produces a different id even if
another object type has identical payload bytes.

The root Tree recursively identifies files, subtrees, symlinks, tombstones and
ConflictObjects. A Step that contains conflicts is still a valid Step and can
be used as a parent, stored, inspected and synchronized.

Capture metadata is represented by immutable `StepObservation` records. More
than one observation may refer to the same Step. An observation may record:

- Space, Worktree and originating local Operation;
- actor and device;
- observation timestamp;
- capture reason;
- non-identity metadata.

Rewrites do not mutate a Step and do not add `supersedes` fields to it. An
Operation owns the rewrite map from old Step ids to replacement Step ids.

An automatic capture that produces the same root and parent set resolves to the
same Step id. The capture workflow must avoid recording a new RepositoryView or
Operation when it would produce no observable state change.

`StepObservation.spaceId` and `StepObservation.operationId` are independently
optional so an observation can describe an object received or inspected before
it is admitted into local Space history. If `spaceId` is present, the Step must
be admitted and reachable in that Space. If `operationId` is present, the
Operation must exist and its resulting state must introduce or reach the Step.
If both are present, the Operation must belong to the same Space. Their absence
never grants access to the Step.

## Ownership

The Step, Tree, TreeEntry target and identifier contracts belong to
`layrs-core`. Canonical hashing and durable object installation belong to the
core/store boundary. `layrs-sync` transports those objects but does not define
their identity or semantics.

The existing automation-oriented type named `Step` must be renamed before the
snapshot type becomes public. Automation state is not source-control state.

## Consequences

- Steps are shared by all Layers and are never owned by one Layer.
- A Layer timeline is a projection of graph reachability and observations, not
  a sequence stored on the Step.
- Descriptions, milestones and releases are separate Marker objects.
- Actor, device and time do not prevent content deduplication.
- Equal Step ids guarantee equal canonical parent sets and root Trees.
- A changed parent set produces a different Step even when the root Tree is
  byte-identical.
- Server authorization is based on Space reachability and references, not on a
  Space id embedded in the globally addressable Step object.
- Garbage collection must follow Step parents and root Tree closure.

## Rejected alternatives

- **Mutable Step ids:** they make sync, undo and audit ambiguous.
- **Layer-owned Steps:** they require copying or artificial propagation between
  Layers.
- **Timestamp or actor in the Step hash:** they prevent deterministic reuse of
  identical snapshots.
- **A required message:** it recreates commit ceremony and conflates snapshots
  with important points.
- **First-parent semantics:** reconciliation provenance belongs to Operations
  and Weaves, not to an arbitrary parent ordering.
