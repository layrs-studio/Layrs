# Ordinary re-export driver integration

27 October 2026. Integration plan after the qualified ordinary physical owner
and implementation of the preallocation source journal. This document separates
implemented boundaries from the remaining driver work; it is not runtime proof.
The target is a real B9 to C9 transfer with inherited history usable after A and
B become unavailable. Do not substitute source DATA validation for that outcome.

## Incoming admission and outgoing operations

The [ordinary source journal](worktrees-partial-vfs-ordinary-source-intents.md)
now implements schema9 incoming Ready selection by actual admission transaction,
digest, receipt and prefix, followed by bounded outgoing interpretation. Fresh
Ready8 retains its singleton rule. Metadata reads are nonrecursive and actual
admission anchors are reread in the retained snapshot.

Implemented outgoing states are Planned and resource-free Cancelled only, with
at most one live plan, ordinary mutation barriers and dedicated managed
reopen/cancellation. Historical Cancelled rows may outlive later captures.
Copying and later phases must extend every reader and gate together, with real
destination and reservation evidence. Published remains unimplemented here.

## Concrete adapters and shared mechanics

Keep the existing Provider proof paths strict:

- `transition_sql::{apply_source_intent, publication::apply_source_publication}`
  and `validation/transition::validate_view` bind Provider A/P, activation,
  capture/publication and its source root.
- `managed_journal::{private,family,observe_unregistered_source,begin,apply}`
  depends on the Active owner, capture-attempt lock and Provider mount/checkpoint.
- `managed_allocation::{allocate,boundary,marker}` and destination/content drivers
  carry the same Provider witnesses.

Add a concrete ordinary source adapter using actual Exclusive Access, incoming
admission, private database identity and the protected ordinary image. Reuse
destination filesystem mechanisms and common DATA computations only after
separating their source adapter. `OrdinaryManagedExport::with_source` rolls back
read snapshots; `with_source_transaction` now supplies the explicit Immediate
writer/commit path used by `begin_plan`. Terminal success still observes positive
database closure.

## Family authority and durable transfer

The admitted family lease authorizes B only. It cannot register, reserve for or
protect C. Introduce dedicated ordinary import access binding B's actual Ready
membership/admission and exact source intent to C's fresh root, control, member,
metadata, database and owner identities. Preserve exact transaction-specific
reservations and the complete representation union.

Recompute reservation metadata from the actual source/destination intents;
`validated_metadata_floor == None` is not permission to reserve. Preserve
ordered attempted/confirmed allocation effects and their pins on failure. No
automatic adoption or cleanup of orphan resources.

### Source proof and allocation interfaces

Use a closed import-source input enum with two concrete validators. The Provider
variant retains allocation/preparation/activation/capture checks unchanged. The
ordinary variant exists only inside the genuine owner's source transaction and
borrows its connection, marker, `VerifiedAdmittedImport`, verified import closure
and `AdmittedFamilyContext`. Require the outgoing intent from that exact snapshot,
the actual incoming admission and family binding, and source-connection equality.
No detached DATA or conversion into Provider evidence grants physical authority.

Ordinary family validation authenticates B's actual Ready registry membership
through `AdmittedFamilyContext::validate_catalog`, then the common C registry
kernel checks C independently. Do not widen B's ordinary lease to authorize C.
Once C has an active transition reservation, the ordinary catalog validator
currently rejects that unretained group. A dedicated export interpretation must
recognize exactly that target before reservation and historical streaming are
connected; generic admitted access remains strict.

Destination allocation must own independent family/lifecycle pins. Borrowing
`FamilyAccess` from `OrdinaryManagedExport` would prevent the mutable owner
callback needed to commit Planned-to-Copying. Keep source SQL proofs out of the
long-lived destination holder. Its common storage owns parent/root/control/
member/metadata/database/FORMAT/owner pins and exact plan/destination commitments.

The existing `ExportParentGuard` borrows a native Provider Session and cannot
serve an ordinary source. Use a distinct `OrdinaryRootGuard` parent adapter with
actual NTFS/outside-sync-root revalidation, independently pinned directory
identity and exclusions for B's checkout/control/metadata and family paths.
Never fabricate a Session. Common filesystem effects and exact FORMAT-inode
publication remain mechanical; source ownership checks remain concrete.

The next executable slice is real C allocation, source Copying CAS, exact FORMAT
publication and C Initializing registration. Every attempted/confirmed effect
and retained failure pin must remain observable. Check the actual encoded
Copying capacity before allocation: current Planned capacity reserves only its
resource-free Cancelled successor. `cancel_planned` stays Planned-only until
resource-bearing cancellation has its own recovery protocol.

## Destination and handoff fences

Ordinary inputs must reach all of the following, rather than just allocation:

- `managed_content/ready_data::OwnedReadyData`, `destination_ready::ManagedReadyInputs`
  and suffix preparation.
- `import_validation/destination_prefix::read_prefix_rows`, autonomous Ready
  reconstruction, and frozen historical Ready interpretation.
- `published_handoff::PublicationSource` and `read_published_for_ready`.
- Physical admission commit, `managed_admission::admit_ready` and admission
  record linkage.

Provider reconstruction retains catalog/activation evidence. The ordinary view
uses committed snapshots, selection, selected base mappings and the complete
protected representations. Observation and placement bodies derive from B's
current snapshot, but C's FORMAT initial commitments must describe the copied
generation-zero snapshot and selection, not B's current frontier.

The ordinary publication handoff independently opens the actual B database,
authenticates durable Published and its incoming admission, and retains source
and family snapshots through C's admission commit. Preserve positive closes and
fresh comparisons. Historical Published handoff retries must tolerate a later
valid ordinary suffix on B.

## Acceptance sequence

1. Genuine admitted B; original A holders closed; ordinary capture and restore
   on B with a restore-only preimage.
2. Production ordinary holder allocates C, copies protected current/base/history
   content, stages checkout and reaches source Published plus C9 admission.
3. Close and reopen C with A and B unavailable; read inherited history, apply an
   inherited inverse and make a new ordinary capture on C.
4. Prove B's original admission remains unchanged; then qualify positive
   publications across multiple committed segments and repeated exports.

Refusal coverage includes substituted admission/database/checkout, outgoing
receipt nominated as incoming, malformed additional rows, a second live intent,
stale image/head, incomplete representations, distinct manifests sharing one
FileId, insufficient metadata budget, changed parent/member/database, wrong C
reservation owner, uncertain commit/close and historical handoff after B advances.

The next programme stages remain companions, expansion, remaining recovery and
lifecycle/GC, performance, CLI/Desktop and full end-to-end acceptance.
