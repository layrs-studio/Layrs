# Layrs Studio Desktop

Studio Desktop is the local-first graphical client for Layrs. It is a Tauri
shell around the shared Rust `layrs-client-core` engine.

Desktop must not reimplement local source-control logic. Features such as init,
scan, diff, save Step, timeline, Layer create/switch, pending publish, receive,
publish and compact belong in `layrs-client-core` and are exposed through Tauri
commands.

## Development

```powershell
pnpm install
pnpm --filter @layrs/studio-desktop tauri:dev
```

Renderer-only development:

```powershell
pnpm --filter @layrs/studio-desktop dev
```

## Current Local Features

- Initialize an existing folder as a local Space.
- Create empty local Spaces.
- Show working-tree changes through Lenses.
- Save local Steps.
- Show local history, changes, merge comparisons and restore previews.
- Create and switch Layers without losing local work.
- Receive, publish and compact through the shared core.
- Use secure desktop auth for server sync.
- Import a local Git repository and export to a new bare repository through a guarded preview.
- Read published files/history, discuss exact review versions, and publish or revoke releases.
- Manage teams, invitations, file access, server settings and authorized instance administration through shared Web/Desktop components.

The entrypoint is `src/app/DesktopApp.tsx`; project routes live in
`src/app/ProjectFrame.tsx`. Complete projects and limited-access projections
share the shell while preserving their distinct native commands and capabilities.
See [current status](../../docs/STATUS.md) for delivery coverage and limits.

## Testing

Renderer tests use a controlled Tauri bridge for presentation and error states:
`pnpm test:e2e:renderer`. Real Windows tests drive the Tauri WebView2 through
the native fixture: `pnpm test:desktop:ci`. The limited-access recipe is separate:
`node scripts/test-native-ui-ci.mjs desktop-partial`. These service recipes require
disposable PostgreSQL; credentials must stay outside screenshots and traces.

`pnpm test:tauri` runs shell and adapter tests. A renderer build alone does not
qualify native operations or produce an installed/signed Desktop package.

The [L6 qualification](../../docs/design/redesign/04-qualification-l6.md) records
the current local test results and identifies the rebuilt test executable.
