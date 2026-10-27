# Admitted restore: cancellation before the durable journal

27 October 2026. The implementation described below is integrated and source
reviewed; its ten-case native recipe passes on r22. This complements the restore-specific durable-journal route in
[the admission contract](worktrees-partial-vfs-managed-admission-contract.md).
The target covers the whole window before `JournalDurable`, including partial
preservation of checkout bytes into CAS; passing only `RestorePoint::Staged`
does not complete this requirement.

## Starting behavior at 5e3b8cd

`restore/engine.rs::apply` reserves `restore:<operation_id>`, creates an owned
checkout quarantine, records `OwnedStage`, writes/syncs its `new-NNNN` files,
then reaches `Staged`. Only afterward does a private transaction preserve old
checkout files, accept the restore journal, delete the prejournal and accept the
restore protection. Family content installation can survive a failure before
that private transaction commits.

The common recovery opener refuses schema9 cancellation before obtaining a
writer. The new restore-specific opener requires an accepted pending journal,
so it cannot cancel prejournal work. Admitted inspection currently ignores
budgets/prejournal and can report an empty reason list despite staging blocking
a fresh restore attempt.

Simply enabling the legacy cancellation is insufficient:

- `retention::cancel_with_access` deletes the private budget before releasing
  its family reservation, potentially losing the retry reference on failure.
- It then calls generic `settle`, which cannot rediscover the typed admitted
  family context. `settle_with_lease` already preserves the retained context and
  orders family release before private deletion.
- `retention/staging.rs::cancel` materializes rows before bounded validation,
  does not first bind all staging to the active operation, and reopens ordinary
  children without revalidating private permissions.
- `shared/staging.rs::cancel_installations` validates and acts one installation
  at a time. A corrupt later Intent can be found after earlier effects.

## Implemented boundary

The implementation adds a dedicated cancellation mode behind both existing
`cancel_shared_reservation` entry points. Legacy behavior remains available and
other schema9 recovery modes remain fenced until their own implementation.
It reuses the same-owner `open_access<T>` preflight/family/fresh-validation pattern.

1. In the genuine clean private RO transaction, validate schema9 import and
   bounded cancellation DATA: unique restore budget, canonical operation,
   absence of accepted filesystem/publication journals and conflicting
   preparation, and zero or one canonical `OwnedStage`. Bind path, operation,
   digest, identity and allowed `new-NNNN` names. Zero staging records must be
   permitted for a cleanup whose last row was already removed.
2. Positively close RO before admitting the family. Validate exact ready
   registration, reservation owner/operation/budget and ordinary reservation
   authorization. Missing family reservation is acceptable only after the
   private budget is durably `committed` (release may already have succeeded).
3. Prevalidate **all** relevant family installation Intent records and physical
   staging before obtaining the private writer or deleting anything. The
   exclusive owner, root authorization and family GC exclusion remain held.
4. Revalidate private DATA and physical pins under the admitted handle. Cancel
   installations using the retained family context, preserving verified CAS
   content and existing protection roots. Then clean only recorded checkout
   staging. Both paths must tolerate their own previously completed steps.
5. Once installations and prejournal are absent, durably mark the private
   budget `committed`, then call `settle_with_lease`: release family reservation,
   then delete the private reference. Never erase the only retry reference
   before release has succeeded.
6. Add an inspection reason for pending reservation/prejournal work. It is
   discovery DATA, not authority to cancel. Malformed evidence must not be
   presented as successfully cleaned or safely cancellable.

Use pinned private-child openers without creation or permission repair. The
checkout record identifies the directory and allowed child names rather than
each child's original inode; retaining and rechecking its private permissions
is part of the deletion authority. Unrelated checkout files, imported roots and
retained exact-installation artifacts must remain untouched.

## Helper boundaries

| Existing code | Reusable evidence / required work |
| --- | --- |
| `shared/staging.rs::load` | Canonical Intent and payload at most 8 KiB; SQL bounds precede loading FileId, manifest and scalar fields. Oversized existing rows cannot appear absent. |
| `Stage::validate_identity` | Pure comparison extracted from begin: incarnation, reservation, FileId, byte count, owner, operation and database identity. The writer/creation path is not called during preflight. |
| `Lease::authorize_content_reservation` | Refuses retained exact-import reservations and wrong admitted ownership. |
| `Stage::directory` / `authorize_content_directory` | Validate recorded staging name/identity and reject retained-import aliases by name or native identity; use private reopening. |
| `Stage::inventory` | Nonmutating validation precedes the deletion loop: known manifest/chunk names, regular files, count, size and single-link bounds for admitted worktrees. |
| `decode_manifest`, manifest/chunk verification | Validate every available piece in staging or CAS before recovering partially published content. `publish_installation` is not a read-only validator. |
| `cancel_restore_installations` | Dedicated admitted route recovers partial CAS installations after complete bounded validation of all matching records (maximum 1024); it uses the exact manifest rather than arbitrary FileId lookup. |
| `settle_with_lease` | Reuse release-before-delete ordering for the terminal committed budget phase. |

`installation_dependencies` omits Intent records without a manifest and is not
sufficient preflight. Exact-import validators concern a different journal and
must not be substituted for ordinary Intent validation.

The bounded global ordinary-installation inventory additionally rejects aliasing
by staging directory name **or inode** across different `(reservation, FileId)`
keys. It validates scalar/payload bounds before streaming canonical Intents.
This check is also used by ordinary admitted `Stage::directory` retries, before
resetting unverified content. The retained exact-import alias guard is separate.

Admitted staging reads, deletion handles and CAS publication handles reject
hardlinks. A checked directory alone cannot prevent an existing outside hardlink
from remaining an alias to content subsequently renamed into immutable CAS.

An already empty private cancellation retry returns without claiming or touching
orphan family reservations. Accepted restore journals still refuse cancellation.
Inspection reports `Reservation` only when no filesystem/publication journal
owns the recovery action; the legacy shared inspection reports it too.

## Qualification and remaining evidence

The r22 exact native recipe passes in 928.34 s child / 928.37 s parent
(child1200/outer1260). It constructs a genuine admitted destination, then exercises
ten single-checkout-file cycles: Staged without an installation; all six
production installation fault points under the actual restore reservation; and
the checkout cleaning/file-deletion/directory-deletion interruption cases.
The installation helper is called after the real Staged stop to establish the
prejournal preservation states; no claim of process-crash injection is made.

Seven cancellation error-return points are exercised across those cases.
Both public cancellation APIs, a clean terminal retry, unchanged dirty checkout,
removed budget/installation rows, original-preview retry, inverse, retained
capture bytes, same imported admission and final retention fsck succeed.
The malformed prejournal, wrong reservation, unexpected child, invalid later
installation, real hardlink and ordinary reservation alias cases refuse without
changing byte witnesses. The alias fixture also exercises ordinary install retry.
See the checkpoint for binary, review and portable proof hashes.

The acceptance requirements below retain the broader matrix. Additional
ACL/identity/reparse/retained-alias negatives, actual I/O/close failures,
multi-file partial cleanup and process/hot-journal recovery are not all
established by this bounded recipe.

- Native cancellation via bound and direct APIs after `Staged`, plus failures
  during preservation before journal acceptance. Retry the unchanged preview,
  restore and inverse; preserve imported provenance and existing captures.
- Read-only inspection and refusals with unchanged byte witnesses: wrong
  binding, foreign budget/reservation, oversized/noncanonical evidence,
  path/operation mismatch, replaced directory, broad ACL, unexpected child,
  link/reparse, retained-import alias and already accepted journal.
- Resume partial cleanup, `cleaning`, already removed directories/prejournal,
  installations at their supported partial publication steps, committed budget,
  reservation already released and release/close failures.
- A malformed later installation must be refused before effects on earlier
  ones. Verify protections and quota accounting after successful cancellation.
- Preserve the four durable-journal recovery scenarios and their rejection of
  cancellation, and preserve applicable legacy recovery behavior.

Hot SQLite journals remain refused by clean admission. A process crash during
private preservation cannot be treated as permission for implicit SQLite
rollback; that recovery boundary requires separate design and qualification.
