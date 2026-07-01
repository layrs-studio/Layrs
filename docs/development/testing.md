# Development Startup

The root development command is intentionally the easiest path:

```powershell
pnpm install
pnpm run dev
```

`pnpm run dev` starts the shared development services through Docker Compose, runs the Postgres migrations from the server, then starts Layrs Server and Layrs Studio Web. It writes the selected local ports to `.layrs-local/dev.env` so repeated launches keep the same addresses.

- Studio Web: http://127.0.0.1:5173 by default
- Layrs Server: http://127.0.0.1:8787 by default
- Layrs Server health: http://127.0.0.1:8787/healthz by default
- Layrs Server routes: http://127.0.0.1:8787/v1/routes by default
- PostgreSQL: `127.0.0.1:15432` by default
- MinIO API: http://127.0.0.1:19000 by default
- MinIO console: http://127.0.0.1:19001 by default

The dev runner probes ports before starting services. If a default port is blocked or reserved by Windows, it chooses the next candidate and records it in `.layrs-local/dev.env`.

The backend runtime is the Axum/Tokio/sqlx server. It persists accounts, sessions, Workspaces, Teams, Spaces, Layers, Desktop device tokens, audit events and Layer access policies in Postgres. The full product interface remains Layrs Studio Web.

Stop the Vite process with `Ctrl+C`. The Docker services are left running so local state stays warm between sessions. Stop them with:

```powershell
pnpm run dev:down
```

Useful validation commands:

```powershell
cargo fmt --check
cargo test -p layrs-server -p layrs-api -p layrs-store-server
cargo check --workspace
pnpm run check:workspaces
```
