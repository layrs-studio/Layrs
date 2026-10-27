# Imported restore history and multi-segment publications

27 October 2026. Read-only source investigation; **not implemented or qualified**.
This remains part of the Worktrees / partial VFS programme, alongside re-export
of admitted schema9 destinations. A local inverse on schema9 is already a
different, qualified path; it does not prove an imported ancestor inverse.

## Current transport gap

`restore::targets(Inverse)` loads `engine::history_with_scope` and takes the
historical preview's `before`. If the associated capture belongs to another
identity, history currently uses `conversion::provenance` to recover the source
Marker and compare the receipt's checkout identity. That legacy interpreter
explicitly refuses schema9.

Replacing only that call would not establish legitimate imported history:
`provider/import_validation/source.rs::require_quiescent_history` currently
forbids `projection_restore_history`. The twelve committed rowsets in
`ImportClosure`, `rows::Kind`, scoped readers and copy layouts do not represent
or copy restore receipts. No existing legitimate managed ingress can deliver
the ancestor receipt required by a successful inverse test.

Provider origins carry `ImportSource::Provider.binding.recovery`, including the
original identity and checkout identity, with activation bindings checked by
the scoped readers. Ordinary origins carry identity and `format_digest` but no
recoverable Marker or checkout identity. Both the receipt rowset and the missing
ordinary checkout proof need authenticated transport, rather than destination
SQL injection or a lookup by matching identity alone.

## Required proof chain

1. Resolve the genuine **incoming** admitted prefix and receipt through
   `read_admitted_import`; an arbitrary outgoing transition is not authority.
2. Use the associated capture sequence to select its provenance segment and
   enforce the segment's lower and upper generation bounds.
3. Authenticate the canonical restore row (`operation_id`, capture sequence and
   payload) as part of that segment's committed historical closure.
4. Match receipt destination identity and checkout identity to the authenticated
   origin of that segment. Verify an ordinary origin's new checkout commitment
   against its actual source Marker when exporting it.
5. Preserve capture/snapshot/digest/permission and `after` consistency checks.
   Exact mappings and protections must cover **both `before` and `after`**;
   restore-only preimages may be absent from transported captures/companions.
6. Build a new inverse preview bound to the current checkout. Preserve the
   ancestor receipt unchanged and continue refusing its direct replay there.

## Implementation ownership and dependencies

- **Versioned transport:** `provider/records/import.rs` and
  `import_validation/{rows,source,scoped,ancestry,copy}` need a committed restore
  rowset and origin checkout evidence. Retain decoding/validation of old
  prefixes, which cannot authorize history they never committed. Copy exact
  canonical rows with collision checks and validate capture/snapshot references
  and every ancestor closure.
- **Content closure:** source mapping derivation and the protected union must
  include all exact restore preimage/result representations. Preserve identities
  when the same FileId has more than one representation.
- **Typed resolution:** extend `ProvenanceIndex` and the admitted-origin adapter
  with detached `RestoreHistoryOrigin` evidence identifying the interval,
  source identity/checkout and the committing segment/receipt. Use the existing
  snapshot discipline and admission reader; no fallback to legacy conversion.
- **Consumption:** `restore/engine.rs` separates receipt interpretation from
  origin/content validation and routes schema9 through that typed evidence.
  Preserve legacy inverse behavior and destination validation at application.
- **Re-export9:** accepting a schema number alone is insufficient. Source
  references, schema checks, ancestry, copy and incoming-admission anchoring all
  need the real schema9 contract. Do not relabel it as schema8.

## Independent publication qualification

The positive multi-segment publication reader can be qualified separately from
restore transport. Existing flow is `conversion::imported_publication_record`
through `admitted_ordinary::imported_publication`, segment resolution and
`completed_record_for_origin`. Generation lower bounds are already checked.
Current admitted tests mainly cover missing records/refusals; `two_imports.rs`
does not establish nonempty inherited publications in several segments.

Build authentic nonempty publications in at least two committed segments and
read them with their original identities after admission and reopening.
Imported captures remain non-publishable through `begin_publication`; ready
destination captures and the later local suffix use their ordinary path.

## Required acceptance evidence

- Authentic multi-segment chain with sources unavailable: inverse of an older
  and a newer imported receipt, restore-only preimage retained, new receipt
  bound to the current destination, ancestor replay refused.
- Positive publications from several segments and correct origin after reopen;
  imported captures cannot be republished, local suffix remains usable.
- Refusals without effects for a receipt outside its committed closure, correct
  identity with wrong checkout, wrong segment or interval, substituted capture,
  absent preimage/mapping, operation ID collision, unrelated ancestor or outgoing
  receipt used as incoming authority.
- Preserve old prefix codecs/closures, legacy inverses and local schema9
  inverses. Keep unsupported re-export paths explicitly refused until integrated.

No native or SQL execution was performed for this investigation. These are
requirements for the next implementation, not accepted behavior.
