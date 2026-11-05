# ADR 0010: Weaves produce Steps and Lens reconciliation is deterministic

Date: 2026-07-19

Status: accepted

## Context

The development workflow models a Weave as a mutable request and session. It
copies source Steps into a target Layer, pauses on content conflicts, and only
updates the target after every conflict is resolved. Client and server also
contain separate Lens dispatch and reconciliation implementations.

The target graph has global immutable Steps and Layers that reference one or
more heads. Reconciliation must therefore create graph history directly and
must behave identically offline, on another device, and on a self-hosted
server.

## Decision

### Accepted Weaves always produce a Step

A Weave reconciles explicitly selected source and target heads. Once request
validation, authorization, policy, and Gates accept the operation, it produces
or reuses exactly one content-addressed result Step and creates exactly one
Operation that installs its resulting Repository View. The result Step may be
clean or conflicted. CAS reuse is not a second result and does not skip the
Operation.

Content conflicts are successful reconciliation output. They do not change the
Weave into a failed or paused operation.

The following conditions reject the command before a result Step is created:

- invalid or ambiguous selectors;
- missing or unavailable objects;
- failed authorization;
- a blocking Policy or Gate;
- malformed canonical data;
- an unrecoverable store or durability failure.

These rejections are not ConflictObjects.

No-op detection happens before acceptance. If reconciliation would add no new
parent relation, tree state, head state, or other repository-visible result,
the command is rejected with `NoChanges`. It creates no Step, Operation,
Repository View, pending sync item, or Weave receipt. A distinct graph join is
not a no-op even when its tree is byte-identical to a parent.

### A Weave joins heads; replay is a different operation

A Weave creates a result Step whose parents record the reconciled heads. It
does not copy source Steps into the target Layer and does not use
`origin_layer_id` or `origin_step_id` to simulate shared ancestry.

Replay, reparent, split, fold, and other history transformations belong to the
graph-rewrite API. They may reuse the reconciliation engine but produce their
own rewrite maps and Operations.

### Merge bases are graph-derived

Merge bases are computed from the Step DAG, never from Layer lineage or copied
Step provenance. The plan records every minimal common ancestor selected as a
merge base.

If there is no merge base, reconciliation uses explicit absent bases and
records that fact. If several minimal merge bases exist, the engine must either:

- recursively reconcile them into a deterministic virtual base; or
- retain all base terms in the resulting ConflictObjects.

It must not choose one base using iteration, timestamp, Layer name, or database
row order. Ambiguity that the configured algorithm cannot represent rejects the
Weave before it mutates repository references.

### Preview is pure

Weave preview reads immutable Steps and CAS objects and returns a proposed
result model. It does not:

- create an active session;
- move a Layer head;
- write Worktree files;
- create pending sync work;
- reserve a mutable conflict identifier;
- become required input to the subsequent apply.

Apply recomputes or validates the preview against exact input Step IDs and Lens
pins. A stale preview is harmless.

### There is no modal source of truth

The authoritative Weave result is its result Step and immutable Weave receipt.
There is no repository-wide `active_weave`, and no `continue` is required.

The receipt records:

- source and target heads before the operation;
- selected merge bases;
- result Step;
- clean or conflicted derived outcome;
- exact Lens executions;
- the Operation and optional Gate snapshot;
- initial conflict IDs reachable from the result.

`source`, `target`, `existing`, and `incoming` are execution-local roles in the
receipt. Each role maps to an explicit canonical `termId`; role names never
determine term order, side identity, resolution choice, or content hashes.

The receipt's set-like collections — source heads, target heads, merge bases,
conflict IDs, Lens executions, and role mappings — are duplicate-free and
sorted by canonical identifier bytes. Only arrays documented as execution
sequences may preserve order.

The Weave receipt ID is lowercase BLAKE3 over the ASCII domain separator
`layrs-weave-receipt-v1`, one NUL byte, and RFC 8785 canonical JSON bytes of the
receipt with `weaveId` omitted. Lens execution IDs use domain
`layrs-lens-execution-v1` over their complete identity projection — path, exact
pin and implementation digest, explicit term-role set, deterministic profile
ID, result, and optional conflict ID — with `executionId` omitted.

A real merge base has `baseId == stepId`. A virtual base ID is BLAKE3 over
`layrs-virtual-base-v1`, one NUL byte, and RFC 8785 canonical bytes of its
ASCII-sorted source Step IDs, exact algorithm ID/version, and result Tree or
object. The deterministic profile ID uses the same construction with domain
`layrs-determinism-profile-v1` and the profile object without `profileId`.

A resolution UI may persist drafts, cursor positions, or preferred tools, but
those are disposable Worktree state rather than repository history.

### Lens selection and execution are exact

The shared reconciliation engine selects a Lens from canonical path, media type,
and configured policy. Every semantic execution records an exact `LensPin`:

- Lens ID;
- semantic version;
- manifest digest;
- reconcile implementation digest.

The implementation digest must match the validated manifest entrypoint. A Lens
family name or mutable `latest` reference is insufficient.

Built-in and external Lenses use the same result contract. Client and server
must call the shared engine rather than maintain separate match statements with
different behavior.

### Lens output is deterministic and bounded

For identical canonical input, exact Lens pin, resolution policy, and resource
profile, a Lens must produce byte-identical canonical output.

The host provides no ambient network, wall clock, randomness, locale, current
directory, host path, or undeclared environment input. Ordered output is
canonicalized before hashing. The host enforces:

- maximum input bytes;
- maximum output bytes;
- maximum memory;
- execution deadline;
- fuel or equivalent instruction budget when supported;
- maximum conflict blocks, segments, paths, and recursion depth.

The immutable receipt captures the complete deterministic profile actually
used: canonicalization and hash algorithms, merge algorithm and version, base
selection/virtual-base algorithm and version, host policy ID/version/digest,
ambient-capability policy, complete resolution policy, and every
resource/count/depth limit. Changing any profile field creates a distinct
`profileId` and therefore a distinct Lens execution identity.

The resolution policy is not an implicit host default. It records its exact
ID, version, digest, duplicate-free ASCII-sorted allowed method set, prohibition
of automatic side selection, `Absent` semantics, combine ordering, manual-input
policy, rename policy, and an explicit action for each unsupported Lens,
unavailable Lens, timeout, quota violation, trap, invalid output, invalid
signature, and digest mismatch. Every one of these fields participates in the
`layrs-determinism-profile-v1` identity projection. The recorded failure action
is either structural fallback or rejection; execution may not silently choose
a side or substitute a machine-local default.

A timeout, quota violation, unavailable implementation, invalid signature,
digest mismatch, trap, or invalid output does not lose either side. It produces
a structurally resolvable conflict when safe to do so. Security, authorization,
or canonical-data violations reject the command instead of being hidden as a
content conflict.

### Structural fallback remains resolvable

Text and Raw Lenses may auto-merge content or expose semantic blocks. Code and
Image Lenses may later add richer behavior. Regardless of Lens capability, the
core can persist the bases and sides as a ConflictObject and offers applicable
whole-value choices.

Fallback never means interpreting unknown bytes as text, injecting markers
into binary data, converting `Absent` to an empty file, or silently selecting a
preferred side.

### Atomic application

The result Step, trees, ConflictObjects, Repository View, Operation, Weave
receipt, and conditional Layer-head update are committed as one logical
operation protected by the recovery journal. Worktree materialization occurs
through the materialization lease protocol.

If the expected heads changed concurrently, Layrs preserves both outcomes as
heads or retries from the new explicit inputs. It never overwrites another
device's accepted head.

### Receipt consistency is exact

The receipt outcome is derived from the result Step closure. `clean` requires
an empty `conflictIds` set; `conflicted` requires the duplicate-free, sorted,
complete set of every ConflictObject transitively reachable from the result
Tree. Missing or extra IDs reject the receipt.

A Lens execution with result `auto_resolved` has no conflict ID. Results
`conflicted` and `structural_fallback` identify a reachable ConflictObject.
Every recorded Lens-derived ConflictObject has exactly one matching execution
with the same pin and implementation digest. Structural core conflicts that did
not execute a Lens have no fabricated Lens execution. Every Lens execution's
`determinismProfileId` equals the enclosing receipt profile's `profileId`, and
both IDs must verify against the profile's canonical derivation.

## Consequences

- The current `layrs-weave` append planner is replaced by a pure graph and
  reconciliation engine.
- Client, CLI, Desktop, server, and Web consume one canonical result contract.
- Server Weave sessions, replay-copy tables, and mutable conflict-resolution
  status are not part of the new baseline schema.
- Gates run before the accepted result is installed; content conflicts are not
  Gate blockers unless a later operation, Marker, or Release policy says so.
- Weave receipt data is audit metadata, not the identity of the result Step.
- Conflict resolution is described by ADR 0009 and creates descendant Steps.

## Rejected alternatives

- **Pause before creating a Step:** prevents durable and synchronizable
  conflicted history.
- **Copy source Steps into the target Layer:** duplicates identities and hides
  the real DAG.
- **Use Layer ancestry as merge ancestry:** Layer workflow relationships are not
  Step graph relationships.
- **Persist preview sessions:** introduces stale mutable plans and global modal
  state.
- **Allow best-effort unpinned Lens execution:** cannot be reproduced or audited.
- **Let unsupported Lenses make conflicts unresolvable:** violates source-control
  safety.
