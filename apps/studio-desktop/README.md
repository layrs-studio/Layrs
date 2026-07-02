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

## Expected Local Features

- Initialize an existing folder as a Draft Local Space.
- Create empty local Spaces.
- Show working-tree changes through Lenses.
- Save local Steps.
- Show timeline and Step diffs.
- Create and switch Layers without losing local work.
- Receive, publish and compact through the shared core.
- Use secure desktop auth for server sync.

## Testing

The first UI tests run the React renderer with a controlled fake Tauri bridge.
That validates user flow and presentation without depending on a native window.
Native Tauri/WebDriver automation can be added later.
