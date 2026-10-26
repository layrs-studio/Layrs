# Partial VFS activation checkpoint — 26 October 2026

Implementation and qualification are separate below. This checkpoint does not
complete the [Worktrees programme](worktrees-partial-vfs.md).

## Implemented activation

The consuming activation writer accepts a fully validated prepared owner. Under
the family lifecycle and exclusive owner/catalog gates it commits the exact
generation-zero to generation-one placement transition, activation map and
mount.v3 record in the private database. Only after observable close and fresh
private validation does it commit the family's exact Initializing-to-Ready CAS.
The immutable source, selected targets and authority remain unchanged.

Every refusal returns the original Store pins and native READY proof. Reports
distinguish confirmed commits from uncertain effects and failed observable
closes. A fresh explicit retry reobserves both databases; it cannot infer state
from an earlier report or allocate generation two. A real SQLite BUSY close
quarantines the process profile; a successful retry is tested separately using
a synthetic interruption followed by a successful close.

Active SQL has its own complete interpreter, cardinality/bounds/schema checks
and exact generation/map validation. The bootstrap and prepared interpreters
continue refusing Active state.

## Implemented follow-up, qualification in progress

`open_provider_active` reconstructs a distinct read-only owner from the complete
durable binding and native READY proof, without inventing a historical writer
report or repopulating deleted placeholders. `resume_provider_activation`
explicitly handles private Active with family Initializing or Ready, with empty
historical effects. Resident reads validate that exact activation map before
and after verified immutable content access, retaining operation gates through
the final output copy. Missing bytes never cause a network fetch.

Maintenance reads the catalog and owner databases without implicit rollback.
Explicit shared-database recovery first requires a fully readable ordinary
private backend before permitting family rollback, and its final shared private
read also stays read-only. This deliberately restricts the earlier recovery
contract: a hot shared private database cannot currently prove that it is
ordinary and is refused unchanged. Nonshared ordinary recovery and family
recovery with a clean ordinary private owner remain supported. A future broader
shared-private recovery capability needs independent backend evidence; missing
provider receipts are not such evidence.

## Retained evidence

`.layrs-local/validation/provider-activation-20261026-r1` retains the executable,
matching PDB, 388 source files and host disposition. EXE SHA-256:
`0e265570af817e3d8efd09f6d4789d07425b65d3ddfc4c58e2d8a0b39f1d9d03`.
Independent review verified these bindings before native execution.

- 82 focused provider pure tests passed.
- 13 isolated SQL tests passed, including three Active interpreter cases.
- Eight isolated activation recipes and one real hot-journal maintenance
  regression passed on fresh owned C: roots. The known-invalid old family
  BUSY recipe was explicitly excluded; its corrected version awaits later
  qualification.
- An accidentally unfiltered libtest run under the filesystem sandbox is also
  retained: 116 passed, 186 failed, 255 ignored. Failures include denied pinned
  access to `C:\Users\ALEXA`; this is not a successful full-suite validation.

The later r2 executable and 392-source snapshot are retained separately. EXE
SHA-256 `3f680f3bc121a00126e8bb5d0d1148c3e5723ba113fc051e0cd33311c01988af`:
82 provider pure tests and all 13 newly reviewed native recipes pass (corrected
family BUSY, five reopen, five active ranges, two ControlPublished recovery
refusals). Independent review verified 421 retained source/binary/toolchain
entries. The additional final shared-read guard in `format/recovery.rs` follows
this binary and awaits its own regression run. Stock Rust/MSVC/Windows host trust assumptions
and their limits remain those recorded in each host disposition. These results
do not qualify a product process or arbitrary SQLite embedding.

## Remaining programme work

The local SnapshotSource/session adapter is implemented and r3 passes two actual
Windows callback recipes with an external reader (cold refusal and resident
content), plus two provider recovery refusals and five ordinary recovery
regressions. Its 83 pure provider tests pass. EXE SHA-256:
`e4fa771f7535e5d08abeffbbdfefe64f4ea50f81543b371d8b87c759a90fd319`.
The shared lifecycle lease survives normal disconnect until the wrapper/source
is released. These tests do not force disconnect failure or disconnect during
an in-flight callback; native lifetime review covers the retained Arc ownership.
The final shared-private read-only guard is included in r3.

The new nonshared recovery fixture failed in r4 before its helper acknowledged:
legacy schema6 has no `projection_observations`. The failure is retained; the
helper now spills through existing `projection_capture_files`. The fresh r5
run passes, proving the preserved explicit nonshared recovery path. EXE SHA-256
`10832652c1e84db4575d2425d6f4f46e736dd44f4e514c048048152b08b19763`.
This binary also contains the next capture slice under development: five pure
capture-codec tests pass, but a new capture-authority fixture fails because its
chosen entry is read-only. That fixture is not part of activation qualification;
its failure is retained and being corrected. No capture writer is qualified.

Capture,
managed ordinary recovery with companions, selection additions, authorized
content preparation and CLI/Desktop integration still require implementation
and qualification. Linux/FUSE evaluation remains conditional on Windows
completion and available prerequisites; macOS is not qualified.
