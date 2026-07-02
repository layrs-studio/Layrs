# ADR 0004: Store durability target

Date: 2026-06-29

Status: accepted

## Context

Source control is only useful if users trust it. Once Layrs accepts local work,
that work must not disappear silently after a process crash, UI interruption,
Layer switch, compaction or local resume.

## Decision

The V1 store target is local durability after write acknowledgement, within the
limits of the host file system.

The design must prioritize:

- atomic or transactional writes for critical metadata;
- content-addressed objects for file bytes and trees;
- stable identifiers for Layers, Steps and artifacts;
- recovery paths that detect partial state;
- safety tests that exercise real local workflows through the CLI and core.

## Consequences

- Caches cannot be the only source of accepted objects.
- Switching Layer must preserve local work before materializing another Layer.
- Publish must send every pending Step in order.
- Compaction must preserve object readability.
- Anti-loss tests are a release gate, not optional coverage.
