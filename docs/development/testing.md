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

## Local Durability Tests

These are the highest-priority tests.

```powershell
cargo test -p layrs-cli --test local_data_safety -- --test-threads=1
$env:RUST_TEST_THREADS='1'; cargo test -p layrs-client-core
```

They cover init, diff, Step creation, pending publish, Layer switching,
compaction and no-loss behavior. If a temporary Local Space fails, it should
remain on disk for inspection.

## CLI And Server Tests

```powershell
cargo test -p layrs-cli
cargo test -p layrs-server
cargo fmt --check
cargo check --workspace
```

Use `RUST_TEST_THREADS=1` for client-core until all temp-path-sensitive tests
are parallel-safe.

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
pnpm test:e2e
pnpm test:e2e:ui
pnpm test:e2e:trace
```

Desktop renderer tests run the React app in a browser with a controlled fake
Tauri bridge. Native Tauri window automation is a future layer.

## Testing Priorities

1. No local data loss.
2. Correct shared behavior through `layrs-client-core`.
3. CLI black-box parity.
4. Server permissions and sync.
5. UI presentation and accessibility.
