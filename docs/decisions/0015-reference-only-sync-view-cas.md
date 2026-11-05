# ADR 0015: Atomic reference-only Sync V3 View changes

Date: 2026-09-20

Status: accepted

Creating a Marker or Release on an existing Step must not require another Step
or a fictitious Layer head change. The original minimum of one Layer intent
prevented this basic operation.

An empty `layerMutations` array now explicitly means a **reference-only** View
mutation. The proposal must be inline and its View ID must differ from the
expected current View ID. The server verifies the exact current View ID and
generation under its Space lock. No stale integration exception applies.

Every Layer head set must equal the expected View; the existing exact-intent
check rejects hidden Layer additions, removals or head changes. Existing Marker
and Release IDs cannot be removed. Added references must resolve to immutable
Products admitted in this Space, in this transaction or previously. Release
protection checks still apply before commit. View, Products, events and receipt
commit atomically. No Worktree or Operation is sent.

An unchanged View is omitted by native clients. A stored View proposal with no
Layer intents is invalid. A malformed or stale reference-only mutation changes
neither heads nor references and produces a terminal rejected receipt. The
idempotency key and request digest retain their existing semantics.

This extends the V3 development contract; older validators reject the new
zero-intent form. The JSON schema, Rust validator and admission planner must be
deployed together. No existing database layout or native Store version changes.
