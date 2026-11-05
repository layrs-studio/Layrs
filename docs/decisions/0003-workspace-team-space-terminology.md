# ADR 0003: Workspace, Team and Space terminology

Date: 2026-06-29

Status: accepted

Updated 2026-10-05: the [project context contract](../contracts/project-contexts.md)
refines Workspace ownership: company by default, optional organisations and
independently optional personal contexts. A Workspace is a technical ownership
boundary, not a mandatory organisation-creation step for users.

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
