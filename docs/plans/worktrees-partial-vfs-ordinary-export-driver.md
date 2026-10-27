# Ordinary re-export driver integration

27 October 2026. Read-only source investigation after the ordinary physical
owner implementation. This is the remaining integration plan, not runtime proof.
The target is a real B9 to C9 transfer with inherited history usable after A and
B become unavailable. Do not substitute source DATA validation for that outcome.

## Incoming admission and outgoing operations

`managed_admission/read.rs::read_admitted_import` reads the admission record
before asking `import_validation/destination_prefix.rs` to reconstruct it.
`read_ready_link` currently counts every `projection_provider_transitions` row
and requires a singleton. Inserting B's first outgoing source intent would
therefore invalidate B's original admission, including its content scope.

For schema9, select the incoming Ready by the immutable admission record's
transaction, Ready digest, receipt and prefix commitments. Preserve the fresh
Ready8 singleton rule. Separately bound and authenticate all remaining rows as
ordinary outgoing source intents; ignoring them is not validation. Never choose
an outgoing receipt as incoming ancestry, or recursively call the admission
reader to validate outgoing rows.

At most one outgoing transfer may be live. Historical Published/Cancelled rows
must remain interpretable after later ordinary captures. Existing ordinary
mutation gates primarily inspect ordinary pending-operation tables, not outbound
managed intents: define an explicit refusal/resume policy after interruption
before removing the singleton rule. The exclusive live owner alone does not
cover later reopening.

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
separating their source adapter. `OrdinaryManagedExport::with_source` always rolls
back its SQL snapshot: the writer needs its own explicit transaction/commit path.
Terminal success must still observe positive database closure.

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
