# layrs-store-local

Durable local storage primitives for a Layrs store root (normally `.layrs`).
This crate owns the content-addressed object directory, transactional metadata,
recovery journal, locking, and conservative consistency checks. Native client
flows use the typed CAS and single-writer transaction boundary; format conversion
is deliberately separate from opening or initializing a store.

The native SCM API in `scm_v1` now initializes **FORMAT 2 / SQLite baseline 2**,
including canonical Product CAS. `tests/fixtures/scm-v1.sql` is preserved;
`scm-v2.sql` is the new checksummed baseline. Previous native versions are refused
without mutation, with no automatic conversion. See
[ADR 0014](../../docs/decisions/0014-native-product-cas-and-structural-host.md).

## SQLite pin

`rusqlite` is pinned to `0.32.1` with `bundled`. It shares
`libsqlite3-sys 0.30.1` with SQLx 0.8.6, avoiding two workspace crates with
`links = "sqlite3"`. The bundled runtime is checked at startup and in tests to
be SQLite 3.46.0 or newer. Changing this pin must be coordinated with SQLx.

Every writable connection verifies WAL mode, `synchronous=FULL`, foreign keys,
a non-zero busy timeout, `trusted_schema=OFF`, and the configured WAL automatic
checkpoint. Schema migrations are ordered, checksummed, and committed with
`BEGIN IMMEDIATE`.

Object installation uses file `sync_all`, a post-sync hash check, atomic rename,
and parent-directory sync on platforms that support it. Windows directory
handles cannot be flushed with `FlushFileBuffers`, so Windows publication uses
`MoveFileExW` with `MOVEFILE_WRITE_THROUGH` instead.

## Sessions and recovery

`LocalStoreSession` first claims a process-local lock, then a real shared or
exclusive OS lock at `.layrs/metadata/layrs.lock`. Shared sessions are SQLite
read-only; writes and migrations require an exclusive session. Incompatible
re-entry fails instead of silently reusing a lock.

Journal recovery exposes durable state and paths to a caller-provided handler;
it does not guess whether external work happened. `fsck_existing` opens only an
existing database and lock file, never creates or migrates them, and uses an
immutable SQLite URI so no WAL/SHM sidecar is created. It refuses a non-empty,
uncheckpointed WAL rather than reading a stale main database.

Before repair mutates metadata, it creates a consistent `VACUUM INTO` backup
under `metadata/repair-backups`, syncs the backup, and validates its integrity,
migration checksums, and store signature. Backup failure aborts repair. Repair
itself is limited to metadata quarantine actions and never creates, rewrites,
moves, or deletes object bytes.
