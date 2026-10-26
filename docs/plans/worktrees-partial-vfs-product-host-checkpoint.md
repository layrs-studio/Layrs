# Provider status product host — 26 October 2026

The Windows `layrs partial provider status --ticket FILE` executable route now
declares the audited SQLite process profile before its first database access.
The declaration is in binary `main`, restricted to Status. Client and Store
libraries do not qualify an embedding host. Profile rejection is propagated.

## Exact artifact and audit

`provider-product-cli-20261026-r1` retains EXE/PDB and 908 source/input files.
EXE SHA-256: `5c74323998ac0d47fe794d052df2d45a16eb830191f6c61e0a7d0b26cb46702b`.
PDB SHA-256: `57a64f0b12dadc1df44a3cf404b3cff4e8a58458fb0bbe4d140b8087d3ec7c56`.
RSDS/PDB GUID `bb9c22e0-3676-4b62-9f84-7be418ea27b1`, age 1.

The independent read-only audit is retained in
`provider-product-cli-static-r1/REVIEW.md` and `final-r1/`. It enumerates 2,640
PDB modules, the actual native objects and startup targets. All 62 third-party
native objects match their linked archives: SQLite 1, ring 24, zstd 33, Blake3
assembly 4. SQLite code/data, relocations and symbols match the previously
reviewed bundled object. Its debug/checksum sections differ as documented.
The three CRT initializers are stock MSVC startup functions; the sole TLS
callback is the checked Rust 1.85 thread-local guard. There are no delay imports;
normal imports are Windows/UCRT/VCRUNTIME only. No production global SQLite
configuration or automatic-extension mutation was found on the route.

2,702 of 2,922 unique source/checksum tuples match available bytes. 217 stock
precompiled-source paths are unavailable. Three older checksums were resolved
to a cfg(test) visibility change, line endings and a doc-comment-only change;
no production semantic difference was found. The audit records these limits.
Authentic stock Rust/MSVC/Windows and absence of native injection remain trust
assumptions; resolved OS DLL identities and the complete historical preprocessing
environment are not asserted. Other CLI commands and other hosts are not
qualified by this evidence.

## Runtime evidence

`provider-product-cli-build-r3.log` records the build. The reviewed exact recipe
`product_status::audited_product_status_reopens_and_refuses_foreign_ticket_without_writes`
passes on fresh owned C: fixtures, using retained libtest EXE
`1b3a9c04c01618c222017cf7aa11724f9d46d398793780756a20ed508054bc3b`
(PDB age 37). Five fresh product processes verify:

- exact Active status, catalogue, generation, digest and root in JSON and text;
- rejection of an inconsistent ticket and a canonical but foreign expected binding;
- rejection of a Prepared provider with the intended private-metadata cause;
- unchanged family and private database witnesses after every invocation.

Each directly owned CLI child has a 30-second local deadline. The libtest child
has a 180-second bound. Normal completion is verified. Forced termination of
the outer process is not claimed to reap its descendants; fixtures are retained
on such failure. This is not crash-recovery qualification.

Creation, activation, capture, managed conversion and selection expansion still
need their product routes and separate qualification. A changed executable or
native/startup closure requires a new artifact disposition.
