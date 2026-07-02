# ADR 0002: No Git data model in the V1 core

Date: 2026-06-29

Status: accepted

## Context

Layrs is an alternative source-control system and platform. Reusing Git as the
core would impose commits, branches, index and refs as product primitives,
while Layrs needs Layers, Steps, Lenses, Weaves, Proofs, Gates and Policies.

## Decision

The Layrs core does not store state as a Git repository and does not expose Git
objects as the product model.

Git-like storage techniques are allowed when they serve Layrs goals: content
addressing, Merkle trees, chunks, compression and packs. The storage technique
is not the product model.

## Consequences

- Product names are Layrs names first.
- APIs expose Layrs concepts, not disguised Git abstractions.
- Future Git import/export must translate concepts explicitly.
- V1 tests validate Layrs behavior independently of Git.
