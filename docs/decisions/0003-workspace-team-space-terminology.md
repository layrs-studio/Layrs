# ADR 0003: Workspace, Team and Space terminology

Date: 2026-06-29

Status: accepted

## Context

Layrs needs stable terms for organization, permissions, repo-like projects and
local work. GitHub/Git terms can help explain the product, but must not control
the data model.

## Decision

V1 uses:

- Workspace for the organization boundary.
- Team for member groups and permissions.
- Space for the repo-like project.
- Local Space for a machine-local copy of a Space.

These are canonical names in docs, code review and APIs.

## Consequences

- "Organization" and "repo" can be explanatory words, not core names.
- Policies and access rules can target Workspaces, Teams, Spaces and Layers.
- Studio Web manages Workspace/Team/Space state.
- Studio CLI and Desktop manage Local Spaces through the shared Client Core.
