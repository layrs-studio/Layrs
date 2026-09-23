# Store3 resolved regular-file rebase, 23 September 2026

Schema 5 adds an atomic, exact migration from schemas 3 and 4. It preserves prior snapshots, captures and namespace history, adds a 64 MiB rebase journal/history, and a separate 8 MiB opaque client draft table. A schema-4 reader rejects schema 5. The draft table grants no authority and does not block local edits; the client must revalidate its draft against current generation, capture and authorized remote projection.

`preview_rebase(generation, capture_sequence, target, resolutions)` requires the latest exact capture of the current full pinned checkout. The target is an authorized `ProjectionSnapshot` supplied by the caller; this layer checks its schema, non-regressing authorization observation, unchanged path semantics and unchanged write/protection classification. Each writable regular target leaf needs an explicit resolved `Some(CapturedFile)` or `None`. Target paths may be newly observed writable regular leaves, and an existing leaf can change between materialized file and explicit absence. Existing observed paths cannot disappear. Permission/visibility changes and directory or symlink transformations require a separate operation before this rebase.

`apply_rebase` recomputes the preview under exclusive access, stages changed resolved bytes through a bounded writer, and journals source identities and staged identities before moving any checkout file. The callback reads prepared local spools only; it must not access the network while the store lock is held. Old files move into private quarantine with no-overwrite pinned renames; new files install with the same collision rule. Full checkout scans, source/stage identity and hash checks, and a final transactional scan precede the atomic new projection, base-relative capture, immutable history and journal-clear commit. Clean old base bytes removed by the rebase are persisted from the verified quarantine pin in the same transaction, so later history verification does not depend on live paths. A durable ready journal is explicitly resumed; external collisions or changed bytes leave the journal and all external bytes untouched. Incomplete staging can be discarded and retried under the same stable operation ID; incomplete private staging directories remain in quarantine for later controlled cleanup.

The resulting projection records the exact remote Step and root-tree IDs. Resolved checkout bytes may differ and are represented solely by the new base-relative local capture. `completed_rebase(operation_id)` validates immutable history and returns the capture without consulting the current checkout, allowing a client to resolve a lost response. `fsck` verifies the rebase chain, both projection generations, source/result captures, file witnesses and stored old bytes and the bounded durable client draft. Capture/file bounds remain 4 GiB total, 1 GiB per file, 1024 changes; file IO uses bounded streaming buffers.

Focused checks: Store3 projection tests (73 passed), including fault recovery, no-overwrite collision, corrupt stage, stale checkout, protected path refusal, schema-3 rollback/retry and schema-4 migration. Linux full projection suite: 74 passed,20.58s. This layer does not synthesize native SCM merge Steps or change remote authority.

Root self-review found a late filesystem race after the BeforeCommit callback.
The final identity/hash/absence and metadata-boundary checks now run after that
callback. Regression replaces an installed file with equal external bytes and
recreates a deleted path: both refuse commit, preserve external bytes and retain
the recovery journal. Windows rebase suite11PASS,3.55s. Independent re-review
unavailable because the agent runtime refuses more threads; this is self-review.


