# Layrs Studio Desktop

This package is a minimal Tauri-ready placeholder for the desktop shell. Network
installation is intentionally not performed in this worker run, so the app is
scaffolded but not built locally.

Expected local flow after dependencies are available:

```bash
pnpm install
pnpm --filter @layrs/studio-desktop tauri:dev
```

The desktop UI reuses the shared `@layrs/ui` components and the typed
`@layrs/client-sdk` fixtures so the Web and Desktop shells expose the same
Workspace, Team, Space, Layer, Artifact, Weave, Proof, Gate and Policy concepts.
