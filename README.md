# Layrs

Layrs is a local-first source control system for code and assets. Its first
priority is simple: accepted local work must not disappear silently.

Layrs keeps product concepts separate from Git concepts:

- **Workspace**: server-side organization boundary.
- **Team**: group used for permissions and ownership.
- **Space**: repo-like project.
- **Layer**: switchable line of work inside a Space.
- **Step**: anonymous snapshot of a Layer state, used for safety and review.
- **Lens**: preview, diff and future reconcile adapter for a file type.
- **Weave**: future review/reconciliation flow between Layers.

## Architecture

Local behavior is implemented once in `layrs-client-core`. Studio Desktop and
Studio CLI call that same Rust core; they must not reimplement local source
control behavior independently.

The local store uses content-addressed Merkle objects and chunks. Steps keep
tree/object references, not copies of the whole project. Chunk compression and
compaction are part of the durability story, but performance must never be
optimized by allowing silent data loss.

Studio Web is the server interface for Workspaces, Teams, Spaces, Layers,
access rules, devices and future Weaves/Gates. Studio Desktop and Studio CLI
are the local-first client surfaces.

## Common Commands

Start the server and Studio Web:

```powershell
pnpm install
pnpm run dev
```

Use the CLI locally:

```powershell
cargo build -p layrs-cli
layrs init "My Space"
layrs diff
layrs step
layrs timeline
layrs layer create Feature
layrs layer use Main
layrs compact
```

Run the full test suite:

```powershell
pnpm run test
```

This includes the Rust workspace, the Studio Desktop Tauri shell, native
Desktop UI, Studio Web, and the Desktop-to-server-to-Web E2E suite.

Use targeted checks while iterating:

```powershell
pnpm run test:core
pnpm run test:desktop:ci
pnpm run test:web:ci
pnpm run test:e2e:ci
pnpm test:e2e
```

If a test fails while creating a temporary Local Space, the failing directory is
expected to remain on disk so the lost-data path can be inspected.
