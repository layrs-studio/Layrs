# Layrs Product Glossary

Terms in this file are canonical for documentation, code reviews and agent
work.

## Workspace

Server-side organization boundary. A Workspace owns members, Teams, Spaces,
devices, audit events and governance defaults.

## Team

Group of Workspace members used for permissions and ownership.

## Space

Repo-like project in Layrs. A Space contains Layers, artifacts, access policy
registries, Steps and future Weaves.

## Local Space

Machine-local copy of a Space. It is a user folder plus `.layrs/` metadata and
can work offline.

## Draft Local Space

Local Space that has not been sent to Studio Server yet. It can have files,
Layers and Steps locally before it receives server ids.

## Layer

Switchable line of work inside a Space. A Layer behaves like a branch in the
user mental model, but it is represented as a Layrs Layer, not a Git branch.

## Step

Anonymous snapshot of a Layer state. A Step is not a commit and does not need a
name or message. Steps protect local work, power diffs/timeline and form the
local pending-publish queue.

## Pending Publish

Set of local Steps not yet confirmed by the server. Publishing must send every
pending Step in order, not only the latest one.

## Client Core

The shared Rust engine used by Studio CLI and Studio Desktop for local
behavior. New local functionality should be implemented here first.

## Artifact

Stored object visible to users through a path or product reference. Artifacts
can be code, text, images, textures, metadata, generated outputs or future Proof
material.

## Lens

Adapter for a file/artifact type. A Lens owns preview, diff and future
reconcile behavior. UI surfaces render Lens outputs.

## View

Readable projection of a Space, Layer, Step, artifact or future Weave.

## Weave

Future review and reconciliation flow between Layers. A Weave will connect
intent, changed artifacts, Steps, Lens diffs, Proofs, Gates, comments and final
decisions.

## Proof

Evidence attached to a Step, Gate, Weave or artifact. Proofs can be automatic
or human-provided, but must remain inspectable.

## Gate

Control point that allows, blocks or asks for Proofs before a Weave or publish
can proceed.

## Policy

Declarative rule for permissions, access registries, Gates and allowed actions.

## Graph

Relationships between Layrs objects: Workspaces, Teams, Spaces, Layers,
Artifacts, Steps, Lenses, Weaves, Proofs, Gates and Policies.
