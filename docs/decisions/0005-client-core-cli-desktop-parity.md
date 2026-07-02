# ADR 0005: Client Core parity for CLI and Desktop

Date: 2026-07-02

Status: accepted

## Context

Layrs has two local client surfaces: Studio CLI and Studio Desktop. If they
implement local behavior separately, they will drift and one surface can lose
work that the other would preserve.

## Decision

Every local source-control capability must be implemented in
`layrs-client-core` first. Studio CLI and Studio Desktop are wrappers around
that core.

Examples include init, scan, diff, save Step, timeline, Layer create/switch,
pending publish, receive, publish and compact.

## Consequences

- A new Desktop local feature must have a CLI equivalent unless it is purely UI.
- Local behavior needs core tests and black-box CLI tests.
- UI tests verify presentation and user flow; they do not replace core/CLI
  durability tests.
- Weaves should follow the same rule: shared engine first, UI surfaces second.
