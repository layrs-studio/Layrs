# ADR 0001: Local-first shared store

Date: 2026-06-29

Status: accepted

## Context

Layrs must remain useful without a central server. Users and agents must be
able to create, inspect, switch and protect work locally before any sync.

## Decision

Layrs V1 uses a local-first shared store as the authority for local client
behavior. Studio CLI and Studio Desktop access that store through
`layrs-client-core`.

The store represents Layrs concepts directly: Spaces, Layers, Steps, artifacts,
object trees, access registries and future Weave inputs. Sync, collaboration
and external bridges are built around this model.

## Consequences

- Local operations must be deterministic and inspectable.
- CLI and Desktop must not duplicate local source-control logic.
- Objects must carry enough metadata to sync later.
- Conflicts must be expressed in Layrs terms, not hidden in temp files.
- Tests must prioritize durability and recovery before visual polish.
