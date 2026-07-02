# Layrs System Overview

This document describes the current intended V1 architecture. It is written for
both human developers and coding agents.

## Goals

- Never lose accepted local work silently.
- Keep Layrs concepts explicit instead of hiding Git concepts underneath.
- Share all local source-control behavior between Studio CLI and Studio
  Desktop.
- Support code and non-code assets through Lenses.
- Prepare Weaves as the future review and reconciliation flow between Layers.

## Current Layers

```text
Studio Web
  Server management surface for accounts, Workspaces, Teams, Spaces, Layers,
  access rules, devices and future Weaves/Gates.

Studio Desktop
  Tauri UI for Local Spaces. Calls layrs-client-core for local behavior.

Studio CLI
  Command-line surface for the same Local Space operations as Desktop.

layrs-client-core
  Shared Rust engine for local config, Local Spaces, Layers, scans, Steps,
  diffs, object store, compact, publish and receive.

layrs-server
  Axum/sqlx server for Studio Web, auth, Workspace/Team/Space/Layer metadata,
  access policies, chunks and published state.

packages/*
  TypeScript SDK, Lens contracts and shared UI components.
```

## Local Space Model

A Local Space is a directory containing user files plus `.layrs/` metadata. The
active Layer is materialized into the user-visible folder. Switching Layer must
preserve current work first, then replace only the files needed for the target
Layer.

Steps are anonymous Layer snapshots. They are not commits. They exist so local
work can be reviewed, restored, published later, and protected during Layer
switches.

## Store And Durability

The local store is content-addressed:

- file contents become chunks;
- file objects reference ordered chunks;
- tree objects reference files and subtrees;
- Layer states and Steps reference root tree ids.

Chunk ids are based on raw content hashes. Compression and packs reduce storage
cost, but the raw content hash remains the integrity boundary.

Critical invariants:

- no accepted Step stores full project copies;
- switching Layer cannot overwrite unsaved work without first creating a Step;
- caches are never the only source of accepted objects;
- failed tests preserve temp folders for inspection.

## Lenses

Lenses own preview, diff and future reconcile behavior. Text/code/image/raw
Lenses are built in first, and external Lenses can be added later. UI surfaces
render Lens outputs; they should not implement file-specific diff logic.

## Server And Access

The server is authoritative for accounts, Workspace/Team/Space membership,
Layer access policies and published Layer state. Local clients cache access
registries per Layer and prevalidate redacted/reserved paths, but the server
revalidates publish/receive.

## Weaves

Weaves are not implemented yet. The target is a review/reconciliation object
that connects:

- source Layer;
- target Layer;
- Steps and changed artifacts;
- Lens diffs;
- Proofs and Gates;
- decisions and comments.

Weaves should explain why a Layer state should be accepted, not just what bytes
changed.
