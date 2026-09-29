# Layrs

Layrs is a local-first source control system for code and assets. Its first
priority is simple: accepted local work must not disappear silently.

Layrs keeps product concepts separate from Git concepts:

- **Workspace**: server-side organization boundary.
- **Team**: group used for permissions and ownership.
- **Space**: repo-like project.
- **Layer**: switchable line of work inside a Space.
- **Step**: anonymous snapshot of a Layer state, used for safety and review.
- **Lens**: preview, diff and reconcile adapter for a file type.
- **Weave**: review and reconciliation flow between Layers.

## Architecture

Local behavior is implemented once in `layrs-client-core`. Studio Desktop and
Studio CLI call that same Rust core; they must not reimplement local source
control behavior independently.

The local store uses content-addressed Merkle objects and chunks. Steps keep
tree/object references, not copies of the whole project. Chunk compression and
compaction are part of the durability story, but performance must never be
optimized by allowing silent data loss.

Studio Web is the server interface for Workspaces, Teams, Spaces, Layers,
access rules, devices, reviews and Weaves. Studio Desktop and Studio CLI
are the local-first client surfaces.

## Common Commands

Start the current server and Studio Web with Node.js 22+ and Docker Desktop
running Linux containers (Docker Compose 2.24.4+ and Buildx required):

```powershell
npm run dev
```

Open the HTTPS URL printed by the command (normally `https://localhost:19443`).
It also prints the path to the generated administrator credentials. The first
build takes several minutes; dependencies compile inside Docker, so no local
Rust or pnpm installation is needed for this command. The browser requires
explicit acceptance of the local certificate.

`npm run dev:info` shows the connection details; `npm run dev:down` stops the
services while preserving projects. This runs built images, without hot reload;
rerun `npm run dev` after source changes. See the [local server guide](docs/development/local-server.md)
for credentials, certificates, Desktop connection and troubleshooting.

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
Desktop UI, Studio Web, and the Desktop and Studio Web renderer suites.

Use targeted checks while iterating:

```powershell
pnpm run test:core
pnpm run test:desktop:ci
pnpm run test:web:ci
pnpm run test:e2e:renderer
```

If a test fails while creating a temporary Local Space, the failing directory is
expected to remain on disk so the lost-data path can be inspected.
