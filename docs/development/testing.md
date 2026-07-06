# Development And Testing

The easiest development path is:

```powershell
pnpm install
pnpm run dev
```

`pnpm run dev` starts Docker services, runs server migrations, then starts
Layrs Server and Studio Web.

Default local endpoints:

- Studio Web: http://127.0.0.1:5173
- Layrs Server: http://127.0.0.1:8787
- Health: http://127.0.0.1:8787/healthz
- Routes: http://127.0.0.1:8787/v1/routes
- PostgreSQL: `127.0.0.1:15432`
- MinIO API: http://127.0.0.1:19000

Stop Docker services with:

```powershell
pnpm run dev:down
```

## Test Suite

Run the full canonical suite with:

```powershell
pnpm run test
```

This runs the Rust workspace tests, Studio Desktop Tauri shell tests, native
Desktop UI tests, Studio Web tests, and the Desktop-to-server-to-Web E2E flow.
The targeted commands below are useful while iterating on one layer.

## Local Durability Tests

These are the highest-priority tests.

```powershell
cargo test -p layrs-cli --test local_data_safety -- --test-threads=1
$env:RUST_TEST_THREADS='1'; cargo test -p layrs-client-core
```

They cover init, diff, Step creation, pending publish, Layer switching,
compaction and no-loss behavior. If a temporary Local Space fails, it should
remain on disk for inspection.

The canonical feature matrix lives in
[`docs/development/test-matrix.md`](test-matrix.md). New local behavior should
state whether it is covered at `core`, `cli`, `desktop-native`, and
`server-sync` level.

## CLI And Server Tests

```powershell
cargo test --workspace -- --test-threads=1
cargo test --manifest-path apps/studio-desktop/src-tauri/Cargo.toml -- --test-threads=1
cargo fmt --check
cargo check --workspace
```

`pnpm run test:core` runs the two Rust commands above. Use
`RUST_TEST_THREADS=1` until all temp-path-sensitive tests are parallel-safe.

## Frontend Checks

```powershell
pnpm --filter @layrs/studio-web check
pnpm --filter @layrs/studio-desktop check
pnpm run check:workspaces
```

## Playwright UI Tests

Playwright tests cover user-visible flows. They do not replace core or CLI
durability tests.

```powershell
pnpm test:desktop
pnpm test:e2e
pnpm test:desktop:ci
pnpm test:e2e:ci
pnpm test:e2e:ui
pnpm test:e2e:trace
```

Desktop-native tests open the real Tauri app and drive the visible UI through
Playwright over WebView2 CDP. The fake Tauri bridge tests are allowed only for
non-critical renderer behavior; source-control workflows should be covered by
the native app.

## Testing Priorities

1. No local data loss.
2. Correct shared behavior through `layrs-client-core`.
3. CLI black-box parity.
4. Server permissions and sync.
5. UI presentation and accessibility.
