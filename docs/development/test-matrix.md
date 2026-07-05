# Layrs Local Reliability Test Matrix

Layrs is source control. The first product invariant is that local user data is
never silently lost. Every local feature must be proven at the lowest reliable
layer, then exercised through the CLI and, for critical user workflows, through
the real Studio Desktop UI.

## Proof Levels

- `core`: direct `layrs-client-core` invariant test.
- `cli`: black-box `layrs --json` test from a real temporary folder.
- `desktop-native`: visible Studio Desktop test through the real Tauri app.
- `server-sync`: publish/receive test against Layrs Server, without relying on
  Studio Web for local correctness.

## Canonical Matrix

| Feature | Required proof | Current coverage |
| --- | --- | --- |
| Init empty Local Space | core, cli, desktop-native | core + cli + desktop-native |
| Init existing folder | core, cli, desktop-native | core + cli + desktop-native |
| Discover `.layrs` from child directory | cli | cli |
| Refuse init in existing Layrs folder | cli | cli |
| Refuse init on file path | cli | cli |
| Preserve nested project files byte-for-byte | cli | cli |
| `.layrsignore` exclusions | cli | cli |
| Scan additions/modifications/deletions | core, cli, desktop-native | core + cli + desktop-native |
| Detect same-size edits after cache warmup | core | core |
| Step creation and clean repeat step | core, cli, desktop-native | core + cli + desktop-native |
| Latest pending Step diff when working tree is clean | cli, desktop-native | cli |
| Timeline ordering and limits | cli | cli |
| Diff by Step id | cli, desktop-native | cli |
| Diff options: stat/name/window/wrap | cli | cli |
| Long text lines are never truncated | core, cli, desktop-native | core + cli |
| Binary/image assets preserve exact bytes | cli, desktop-native | cli |
| Rename/move safety | cli | cli |
| Layer create/use round trip | core, cli, desktop-native | core + cli + desktop-native |
| Switch with unstepped work auto-saves before switch | cli, desktop-native | cli |
| Switch with auto local steps disabled still preserves work | core | core |
| Failed switch with missing object is non-destructive | core, cli | core + cli |
| Delete non-active layer | core, cli, desktop-native | core + cli |
| Refuse delete active layer | cli, desktop-native | cli |
| Refuse delete layer with children | cli | cli |
| Forget local archives `.layrs` and keeps project files | core, desktop-native | core |
| Pending publish is per-layer and ordered | core, cli | core + cli |
| Publish sends every pending Step | core, server-sync | core |
| Receive materializes V2 chunks byte-for-byte | core, server-sync | core |
| Compact keeps data readable and reduces storage | core, cli | core + cli |
| Store corruption/hash mismatch is loud and non-destructive | core, cli | core + cli |
| Desktop shortcuts and settings validation | core, desktop-native | core |

## Gaps To Close Next

- `desktop-native` for latest pending Step diff after clean working tree.
- `desktop-native` for long text line diff and binary/image fallback.
- `desktop-native` for layer delete and Forget local confirmations.
- `server-sync` for CLI draft publish with multiple pending Steps, then receive
  into a second Local Space.
- `server-sync` for Desktop draft publish, then CLI receive into a second Local
  Space.

## Rules For New Features

- A local Desktop feature must call `layrs-client-core`; it must not invent a
  second local implementation.
- A local Desktop feature needs at least `core` and `cli` proof. Critical flows
  also need `desktop-native`.
- Server synchronization tests can use Layrs Server, Docker and Postgres, but
  they should not use Studio Web as the proof of local data correctness.
- Failed local durability tests must keep their temporary folder and print its
  path for inspection.
