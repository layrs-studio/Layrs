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

Run the main safety checks:

```powershell
cargo fmt --check
cargo test -p layrs-cli --test local_data_safety -- --test-threads=1
cargo test -p layrs-cli
$env:RUST_TEST_THREADS='1'; cargo test -p layrs-client-core
cargo test -p layrs-server
pnpm --filter @layrs/studio-web check
pnpm --filter @layrs/studio-desktop check
```

Future UI safety coverage is under Playwright:

```powershell
pnpm test:e2e
```

If a test fails while creating a temporary Local Space, the failing directory is
expected to remain on disk so the lost-data path can be inspected.
