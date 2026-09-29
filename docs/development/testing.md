# Development And Testing

The easiest development path is:

```powershell
pnpm install
pnpm run dev
```

`pnpm run dev` starts version-pinned Docker services, runs server migrations,
then starts Layrs Server and Studio Web. Stop the services with
`pnpm run dev:down`.

Default endpoints are Studio Web on `127.0.0.1:5173`, Layrs Server on
`127.0.0.1:8787`, PostgreSQL on `127.0.0.1:15432`, and MinIO on
`127.0.0.1:19000`.

## Canonical Trust Gate

On Windows, run every required test with:

```powershell
pnpm test
```

This deliberately runs all of the following:

1. migration checksums, source-size limits, Rust format and Clippy with
   warnings denied;
2. every TypeScript workspace check, both Studio production builds and the
   client SDK unit tests;
3. the whole Rust workspace against mandatory disposable PostgreSQL;
4. Studio Desktop Tauri shell tests;
5. both renderer projects, real native Desktop, real Studio Web, and the real
   Desktop-to-server-to-Web flow.

`pnpm test` does not silently skip the Windows-native suite on another OS. CI
uses explicit platform jobs: Linux proves static/PostgreSQL/Web behavior and
Windows proves the native Tauri app.

## Static Gates

```powershell
pnpm check
pnpm format:check
pnpm lint:rust
pnpm check:typescript
pnpm build:apps
pnpm check:file-size
```

`pnpm check` runs every command above. Clippy warnings are failures, not
background noise.

## Local Durability And CLI

These are the highest-priority focused tests:

```powershell
pnpm test:core
cargo test -p layrs-cli --test local_data_safety -- --test-threads=1
```

They cover init, diff, Step creation, pending publish, Layer switching,
compaction and no-loss behavior. A failing temporary Local Space should remain
on disk for inspection. The feature matrix lives in
[`test-matrix.md`](test-matrix.md).

## Rust And PostgreSQL

```powershell
pnpm test:rust
pnpm test:postgres
pnpm test:tauri
```

`test:rust` starts an isolated `postgres:16.14-alpine3.23` container when no
database URL is supplied, then runs the whole Rust workspace with database
tests required. `test:postgres` runs only `layrs-server` against the same kind
of disposable database.

To provide a database instead, it must be disposable and explicit:

```powershell
$env:LAYRS_TEST_DATABASE_URL = "postgres://layrs:layrs@127.0.0.1:55432/layrs_test"
$env:LAYRS_TEST_DATABASE_ISOLATED = "1"
pnpm test:postgres
```

Without the isolation acknowledgement, the command fails before mutation.
Use `RUST_TEST_THREADS=1` until all temp-path-sensitive tests are parallel-safe.

## TypeScript And Builds

```powershell
pnpm check:typescript
pnpm test:sdk
pnpm build:apps
```

SDK unit tests are separate because TypeScript compilation does not execute
normalization and Lens diff assertions.

## Migration Integrity

Published migrations are append-only and protected by
`scripts/migrations.lock.json`.

```powershell
pnpm check:migrations
pnpm migrations:append
```

`check:migrations` fails if a known migration changed or disappeared, or if a
new migration is not recorded. `migrations:append` adds only higher sequence
numbers and refuses to replace an existing checksum. This accepts a legitimate
new migration without permitting a historical rewrite.

CI also creates an empty upgrade database, applies the PR base migrations with
`sqlx-cli 0.8.6`, then applies current migrations. This complements the fresh
database server tests.

## Playwright UI Tests

```powershell
pnpm test:desktop
pnpm test:desktop:ci
pnpm test:web:ci
pnpm test:e2e:renderer
```

Desktop-native tests open the real Tauri app and drive the visible UI through
WebView2 CDP. Renderer mocks are allowed only for non-critical presentation;
source-control workflows require the native app.

CI-mode Playwright has zero retries. Each target writes to its own
`test-results/<target>` and `playwright-report/<target>` directory. Failed
traces, screenshots and videos are retained as CI artifacts for 14 days.

## CI Jobs

- `Linux / static, PostgreSQL, Web`: static checks, SDK, builds, all workspace
  Rust tests on PostgreSQL, migration upgrade, renderer and Studio Web tests.
- `Windows / Tauri native Desktop`: Desktop typecheck, Tauri shell tests,
  package build and real native Desktop UI tests.

Actions are pinned by commit SHA. The test PostgreSQL image is pinned in
`scripts/test-postgres.mjs`; override it with `LAYRS_TEST_POSTGRES_IMAGE`.

## Testing Priorities

1. No local data loss.
2. Correct shared behavior through `layrs-client-core`.
3. CLI black-box parity.
4. Server permissions and sync on real PostgreSQL.
5. UI behavior and accessibility.
