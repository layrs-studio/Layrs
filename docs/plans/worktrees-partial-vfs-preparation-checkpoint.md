# Provider preparation checkpoint — 26 October 2026

The post-bootstrap writer now consumes `ProviderControlPublished`, persists a
bound `mount.v2` pending ticket before the native factory, creates the selected
namespace without a content source, and commits the exact prepared successor.
This implements the preparation portion of the
[contract](worktrees-partial-vfs-preparation-contract.md). It does not activate
content callbacks or mark the family member ready.

The complete schema-8 SQL interpreter validates both v2 states, including all
table inventories, bounds, source/descriptors, A/P binding, ticket, projections
and foreign keys. Bootstrap v1 readers continue refusing the successor. Each
writer transition validates its exact predecessor under an immediate transaction
and uses an exact row CAS; success requires readback and observable close.

The writer holds the lifecycle, owner and catalog gates throughout SQL and the
native operation. These gates serialize cooperating Store operations; callers
must separately quiesce namespace, metadata and registration changes as required
by the VFS factory. No claim about external-writer exclusion follows from Store
locks alone.

Every result retains the original Store pins. A withheld result also retains
any native READY proof already obtained, pending/prepared candidates and typed
causes. The report distinguishes confirmed commits from uncertain close/native
effects. Revalidation keeps this report as historical issuance and freshly
validates both family and private metadata. Diagnostic classification follows
the primary refusal while preserving original rollback error identities and
historical triggers.

## Qualification

Evidence: `.layrs-local/validation/preparation-writer-20261026-r1`.
The retained Rust 1.85 libtest executable SHA-256 is
`dfc1fcb5967033790f1e8705056f9796e549fa8a4eae8e6dea0df0796dcfdaf7`.
Its PDB identity, nine startup-source checksums, current native inputs and 373
source files are recorded with a source snapshot. This uses the same explicit
stock-toolchain and Windows-loader trust assumptions as the preceding catalog
qualification; it does not qualify a product executable.

All seven exact owned native recipes pass on fresh C: temporary roots:

- Zero root ID and changed P refuse before mutation.
- A nonempty cold file becomes a correctly sized placeholder, with no Store
  content installed and no content source supplied.
- A real BUSY close after the pending commit preserves the confirmed commit,
  withholds the owner and never registers the native root.
- A fault after the prepared commit retains native READY and both known commits.
- A changed pending row refuses the prepared CAS while retaining native proof.
- Changed family registration refuses revalidation with its scope and historical
  write report intact.

The integrated executable also passes 79 pure provider tests and nine exact
in-memory SQL recipes (three new v2 cases and six bootstrap regressions).
Unselected native tests remain ignored. Independent critical source review
closed the diagnostic and retained-report findings. No old native fixture was
reopened and no historical recipe was replayed. Cross-process contention across
the whole native interval has not yet been separately qualified.

## Remaining work

Durable prepared reopen, pending-state recovery, local resident ranges and their
leases, exact activation mapping, callback lifetime/cancellation, provider-aware
capture, managed/companion recovery, selection additions, and CLI/Desktop host
qualification remain separate stages. READY is an initial native audit; it does
not prove a later user namespace unchanged. Reopen must never repopulate READY
or resurrect user deletions. The overall programme remains open.
