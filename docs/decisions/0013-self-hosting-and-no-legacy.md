# ADR 0013: Complete self-hosting, secure bootstrap and no legacy

Date: 2026-07-19

Status: accepted

## Context

Layrs is an open-source complete source-control stack with native storage,
synchronization and collaboration surfaces. It is still in development, so
preserving V1/V2 database or wire layouts would constrain the new graph model
without protecting production users.

The former development Compose stack starts PostgreSQL and MinIO but does not
constitute an operable deployment: object storage selection, server/Web images,
identity bootstrap, durable volumes, health, backup and restore must all be real.

## Decision

The graph redesign starts from one new PostgreSQL baseline and one V3 wire
protocol. There is no automatic upgrade from development V1/V2 data, no V2
endpoint compatibility and no Git storage or bridge in the core. Unsupported
stores and clients fail closed with an explicit version error and are not
mutated.

## Minimum supported self-hosted deployment

The supported deployment contains:

- an OCI image for Layrs Server;
- an OCI image or server-packaged static Studio Web build;
- PostgreSQL on a documented persistent volume for mutable metadata,
  transactions, receipts and event logs;
- one actually used durable CAS backend: a documented persistent filesystem
  volume or S3-compatible storage such as MinIO;
- local account authentication with password hashing, session/token revocation
  and recovery as the mandatory baseline; OIDC is optional;
- a one-shot first-administrator bootstrap that consumes a single-use secret,
  refuses to rerun after initialization and never ships a default credential;
- explicit ordered/checksummed schema initialization and forward migration
  tooling;
- readiness and liveness endpoints, including PostgreSQL and selected object
  backend health;
- documented quotas, consistency checks, backup, restore and disaster drills.

Production terminates TLS in Layrs Server or a documented reverse proxy. Secrets
come from files/secret stores or environment injection outside image defaults,
are rotatable, and are never returned by health/version endpoints. Object
transfers stream with encoded/decoded size and quota limits. PostgreSQL `BYTEA`
is not the production chunk backend.

The server publishes wire protocol version, database baseline version,
deployment identifier, authentication modes and object-store health without
credentials.

## Database/CAS backup protocol

Layrs uses CAS-before-reference admission: immutable objects are fully installed,
flushed and verified before PostgreSQL may commit a reachable Step, product or
reference. A supported backup therefore:

1. records a PostgreSQL/event watermark and deployment/baseline identifiers;
2. captures a transaction-consistent PostgreSQL backup at that watermark;
3. captures or retains a CAS snapshot whose completion point is at or after the
   database watermark;
4. writes a manifest of every object reachable from database roots at the
   watermark, including RepositoryViews, FileManifests and Chunks, PolicySets,
   AuthorizationSnapshots, receipts, kind, canonical length and digest;
5. retains leased uploads needed by transactions represented in the database
   backup.

Restore loads the CAS and database into an isolated, non-serving instance, checks
baseline/deployment compatibility, and runs closure fsck against the manifest.
Traffic is enabled only after every reachable object's kind, length and digest
verifies and every retained FileManifest reconstructs its FileId. A database
referencing an older/missing CAS snapshot fails closed; repair never invents
bytes.

## Consequences

- Existing development databases are disposable and intentionally rejected.
- Migration code moves forward only from the new baseline.
- Tests create fresh V3 fixtures rather than upgrade V2 data.
- Self-hosting needs no proprietary service and no Git forge.
- Filesystem CAS supports one durable node; S3/MinIO is the scalable backend.
- A deployer can create the first administrator without a vendor control plane or
  permanent bootstrap secret.
- Release gates include restore drills, manifest closure and DB/CAS integrity
  verification.
