# Layrs Tests

This folder holds cross-surface tests and fixtures.

## Safety Invariants

- Accepted local work is never lost silently.
- A Layer switch preserves current work before materializing another Layer.
- Steps are per Layer.
- `layrs diff` shows working-tree changes, or clearly shows the latest pending
  Step when the working tree is clean.
- Publish queues must include every pending Step in order.
- Compaction must not make file objects unreadable.

## Strategy

- `layrs-client-core`: engine invariants and low-level local behavior.
- `layrs-cli`: black-box local workflows through the real `layrs` binary.
- `layrs-server`: server sync, access and persistence.
- Playwright: user-visible Studio Web and Studio Desktop renderer flows.

When a local safety test fails, the temporary Space should be preserved so the
exact data-loss path can be inspected.
