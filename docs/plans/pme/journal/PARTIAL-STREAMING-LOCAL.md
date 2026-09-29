# Local partial capture streaming

Store3 captures retain the `layrs.projection-capture.v1` document, digest,
sequence, and projection identity. Changed files may now be up to 1 GiB each
and 4 GiB per capture. The previous 16 MiB `MAX_PATCH_BYTES` limit remains
only for legacy buffered namespace operations, pending their separate extension.

New capture bytes are stored as immutable 1 MiB chunk rows in the projection
database. Capture scans and SQL persistence read one chunk at a time, rehash
the reopened pinned file, and check the checkout again before commit. An
interrupted or changed capture rolls the transaction back. Legacy blob rows
remain readable. Opening an old projection store performs an exclusive,
schema-checked migration from SQLite schema version 3 to 4. Old binaries
reject the new schema during their exact schema check.
The same migration widens namespace journal payload bounds to 8 MiB while
copying existing pending and completed rows in the transaction.

`captured_file_manifest(sequence, path)` returns the standard SCM
`FileManifest` with deterministic fixed 1 MiB chunking.
`read_captured_chunk(sequence, path, ordinal, out)` validates the capture
document, path binding, and requested chunk digest. The manifest call and
`write_captured_file(sequence, path, out)` verify the full file digest; the
latter streams through bounded buffers. The buffered `file_bytes` helper was
removed after the restore integration moved to streaming.

Restoration now stages selected files in streams and preserves original bytes
as chunks inside the same SQL transaction as its durable recovery journal.
Inverse operations and history verification also stream. Windows restore tests:
7 passed, including 17 MiB restore/recovery/inverse at JournalDurable,
PathQuarantined and BeforeCommit. Evidence:
`D:/Layrs/tmp/restore-streaming-20260923.log`.

Independent critical review of capture/migration/restore completed: six added
regressions pass. Conflicting blob/chunk representations, coherent wrong-file
chunk replacement, oversized corrupt SQL rows, replaced restore destinations
and recreated deleted paths all fail closed. Migration failure rolls all DDL
back and permits a repaired retry. The opaque `VerifiedCapturedFile` witness
binds chunk reads to a previously verified complete file without holding a
database lock over network I/O.

Memory qualification: new chunk payloads are bounded to 1 MiB. Legacy SQLite
`substr` may internally materialize the historical blob, bounded to 16 MiB;
Rust output buffers remain chunked. No strict 1 MiB process-memory claim.
Integrated Windows projection suite: 62 passed, 0 failed, 0 ignored (23.28s),
before the additional Windows-only long-path test which also passed separately.
1 GiB/file and 4 GiB aggregate stress qualification is still pending.
