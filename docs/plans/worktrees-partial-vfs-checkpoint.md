# Worktrees / partial VFS checkpoint — 2026-10-25

**27 October, r23 imported restore DATA and append-stable history:** prefix v2
adds a committed restore rowset and ordinary checkout-origin evidence while
preserving v1 canonical bytes/domain. Source, scoped/ancestor and copied readers
carry exact `operation_id,capture_sequence,payload` rows; before/after content
requirements participate in exact mappings. Schema9 inverse consumption now has
a typed incoming-admission origin route. Independent review corrected dispatch
to use the imported capture boundary, not logical identity inequality: an old
checkout can have the same logical identity as a later destination.

New restore operations always append a fresh capture, so a later restore cannot
add a receipt below an already committed capture frontier. Ordinary capture and
content deduplication remain; existing-operation replay adds no history. Seven
codec tests and three SQL-memory DATA recipes pass on r23. The SQL fixtures
prove canonical metadata/mapping commitments and refusals, not content residence
or a genuine physical multi-segment import.

The released native recipe
`managed_restore_advances_frontier_without_disabling_capture_deduplication`
passes on a real admitted destination: A capture/dedup, dirty B restore A, dirty C
restore A, distinct fresh captures with identical changes, unchanged earlier
history, ordinary dedup, old-operation replay, reopen, inverse to C and retained
content fsck. Runtime 260.31 s child / 260.38 s parent / 260.394 s outer,
child480/outer540. No inherited inverse or new source export is exercised.

r23 EXE SHA-256
`a63195260d1059f44f381845771c190d597cc075f7d12d39919c770a4877024f`,
CodeView age101, 583 retained inputs and nine startup checksums. Independent
source release covers 65 files and exactly 11 recipes; hashes matched current
sources before execution. All-targets checks and final test build pass. The
portable `imported-history-evidence-20261027-r23.tar.gz` contains 607 verified
files, 1,723,345 bytes, SHA-256
`e5be607df8aacf7b66e407843dc311bd6e806dd183b2878835072c2b2ceb29d0`.

Next is genuine admitted ordinary export, not just allowing schema9 in the
source enum. Current planner/allocation/Ready/publication/admission inputs still
depend on provider activation; the [updated implementation plan](worktrees-partial-vfs-imported-restore-history.md)
records the ordinary owner, source admission/database evidence and public-driver
work. Acceptance must reach C9 reopened without A/B, then inherited inverses and
positive multi-segment publications. Remaining recovery, companions, expansion,
lifecycle/GC and CLI/Desktop acceptance remain in the active programme. Older
writers against exported sources and process crashes/hot journals are unqualified.

**27 October, r22 prejournal cancellation:** admitted schema9 now has a dedicated
bound/direct restore-cancellation route, with clean private RO preflight,
positive close, complete family/physical validation, fresh writer comparison,
and consuming finalization. Inspection reports `Reservation` for journal-free
private budgets/staging. The cancellation covers partial CAS preservation as
well as the Staged callback, preserves verified content and checkout edits,
and releases the family reservation before deleting its durable private retry
reference. A clean terminal retry is a no-op, not authority over orphan family
reservations. Other operation types and SQLite rollback remain fenced.

Independent critical review closed two concrete integrity findings: hardlinked
staging could leave an outside alias after CAS publication, and different
ordinary reservations could claim the same staging directory/inode. The
implemented checks cover read/prepublication/unlink handles and a bounded
global ordinary Intent inventory, alongside the retained exact-import guard.
Ordinary admitted staging retries also reject the cross-reservation alias.

The released native recipe
`managed_admitted_restore_cancels_prejournal_and_partial_installations_then_retries`
passes all ten cases on r22: real restore Staged interruption, six production
installation-helper fault states under its restore budget, and seven
cancellation interruption points. These are installations cleared, staging
marked cleaning, file unlinked, directory removed before prejournal deletion,
staging cleared, private budget committed, and family reservation released.
Bound/direct cancellation, terminal replay, retry of the original preview,
restore/inverse, retained capture bytes, original admission and final fsck pass.
Malformed staging, unexpected checkout entry, wrong reservation, invalid later
installation, cross-reservation alias (also ordinary retry), and real hardlink
refuse with unchanged checkout/family byte witnesses.

Exact r22: EXE SHA-256
`053aa9eb2d43778b8be8aa22a668a54acb916753efec8a8051c633bc3639e6b3`,
CodeView age 99, 581 retained inputs, child1200/outer1260 on a fresh C: fixture.
Runtime: 928.34 s child / 928.37 s libtest parent, 928.696 s outer runner.
All-targets check and test build pass. The verified portable
`restore-cancel-evidence-20261027-r22.tar.gz` contains 596 files, 1,706,121 bytes,
SHA-256 `4f78afd2f5c63c867f070a2ff3e10994b78ec30c72f5710ea7dfcbd18f2a7801`.
Sources, review, scripts and logs are included; binaries/databases/fixtures stay
local. No full-suite, crash, hot-journal, all-ACL/identity corruption matrix,
actual I/O/close-failure or product-host acceptance is inferred.

Next: inherited restore history needs authenticated transport, not just a
schema9 reader branch. Current import closures forbid and omit restore history;
ordinary origins also lack checkout identity evidence. The
[imported history plan](worktrees-partial-vfs-imported-restore-history.md) records
the versioned closure/content/origin work and the independent positive
multi-segment publication qualification. AfterCommit fault replay, additional
recovery refusals, other operation-specific recovery, re-export9, companions,
expansion, lifecycle/GC and CLI/Desktop qualification remain open.

**27 October, r20-r21 admitted restore recovery:** the clean schema9 inspection
and dedicated restore opener are implemented and independently reviewed. They
validate the bounded pending journal in the same genuine private RO snapshot
as immutable admission, positively close it, admit/validate the exact family
reservation and protection, then open the bound private writer and compare
fresh detached evidence. Other recovery operations and SQLite rollback remain
fenced. Review fixes cover digest representation, protection-operation binding,
current selection, and bounded protection-index loading.

The SQL-memory protection-index refusal recipe passes on r20. Its native attempt
stopped at secure parent-directory opening (sandbox access denied), before any
checkout was created; that failure is retained. The identical source and binary
then ran with the required host access in a fresh C: parent on r21 and passed:
`managed_admitted_restore_resumes_four_durable_interruptions_with_readonly_inspection`
in 393.88 s child / 393.95 s libtest parent (394.492 s outer runner). It covers
JournalDurable, PathQuarantined, PathInstalled and BeforeCommit, alternating the
bound/direct resume APIs; read-only inspection; wrong binding and other-route
refusals; malformed journal refusal without byte changes; DATA refusal of an
unrelated protection operation, excluded selection and wrong budget; receipt
replay, destination-owned inverse, historical content and immutable import.

r21 EXE SHA-256 `ed2bb01c15d5276bf38dfae63df69d70a8bbe09dd31fd3a113078c2d83b17643`,
CodeView age 97, 577 retained inputs, child720/outer780. r19 was superseded
without execution after a comment correction about retained owner/GC scopes.
The verified portable `restore-recovery-evidence-20261027-r19-r21.tar.gz`
contains 1,758 files, 4,848,141 bytes, SHA-256
`ad224df8beaf7c83c64c51f0f1595a2649cef23f53a6996973879b36e1a89ec7`.

This is explicit error-return recovery, not process-crash, hot-journal or
whole-programme qualification. Prejournal cancellation/inspection is qualified
separately on r22 above. Remaining restore work includes AfterCommit fault replay, further binding/geometry and
close-failure negatives, and inherited inverse history. The next implementation
plan covers the **whole** prejournal preservation window, including partially
installed CAS content: [restore staging recovery](worktrees-partial-vfs-restore-staging-recovery.md).
Transitions, companions, expansion, lifecycle/GC and product-host integration
remain required.

**27 October, r16 ordinary reopening:** the public schema9 route now uses a
physically bound, independently closed RO preflight before admitting its family
and opening the requested private handle. Owned ordinary operations explicitly
finalize SQLite on success and error, while owner/lifecycle scopes remain held.
The independently reviewed native recipe
`managed_admitted_ordinary_reopen_finalizes_success_errors_and_unfinished_transactions`
passes (162.18 s child / 162.23 s parent): true admission, source helper returned,
ordinary open/state, typed stale-capture refusal, unfinished transaction refusal
and rollback on both success/error exits, reopening, unchanged admission digest
and byte witnesses. The recipe does not perform a valid capture or restore.

Exact r16 evidence: `.layrs-local/validation/provider-family-publication-20261027-r16`;
EXE `ead816cc5e06d0159600c518aaaf2bf5c291e7fdb1bce5336decc6f76c0fb703`,
CodeView age 91, 573 source inputs. Two runner prechecks caught transcription
errors in expected source hashes before creating any native fixture or child;
the corrected recipe then ran once. Historical r14-r15 statements below describe
their own earlier code, not the current opener.

**Current integration:** explicit owner-borrowed `ContentScope` carries durable
admission through capture, immutable file/chunk reads, offline availability and
preparation, and ordinary restore. Metadata-only provenance readers remain
independent; legacy unscoped helpers refuse private9. Independent review found
and ROOT fixed preparation's selection of an arbitrary family representation:
both begin/resume now prefer the exact private mapping. Integrated all-targets
checks and test builds pass. The independently reviewed source-unavailable
mutable-cycle recipe passes on r18 in 272.92 s child / 272.96 s parent: the fresh
fixture source checkout is moved aside after all original holders are gone;
availability and preparation retain the exact private representation without
acquisition; two edits are captured, deduplicated, read in full and by verified
chunks, compared and reopened; capture restore, destination-owned inverse and
generation restore succeed; immutable version bytes, retained-content fsck and
the original admission survive the complete cycle. No process crash or deletion
of source bytes is claimed. Interrupted schema9 restore recovery is qualified
separately in r21 above; inherited inverse history remains unqualified and fenced. Other transitions, companions,
expansion, lifecycle/GC and product-host qualification are still outstanding.

r18: EXE `1ae6a72b052be6ee9bd070985e6e1d1af6187422c98880c734a04ef8f7c578b6`,
CodeView age 94, 574 source inputs, child480/outer540 on a fresh C: parent. r17
was superseded without execution after the test oracle was corrected to use
the actual private representation rather than the physical checkout manifest.
The portable `ordinary-evidence-20261027-r16-r18.tar.gz` supplement contains
1,744 verified files (4,768,023 bytes), SHA-256
`e3ec0c58cc3375bdc6bf8b4df9ffc28e715e8d5ba2757c3b0754f1a94b07298e`.
It includes exact source snapshots, independent review receipts, runners and
build/runtime logs; binaries, SQLite databases and native fixtures stay local.

**Latest continuation: 27 October 2026.** The user explicitly resumed after the
[night handoff](worktrees-partial-vfs-night-handoff-2026-10-27.md). The
[managed checkpoint](worktrees-partial-vfs-managed-source-checkpoint.md) records
passing exact installation/protection, physical checkout and retained-provenance
private/family Ready and source Published recipes, including live reads under
immutable CAS handles. The r6 continuation additionally qualifies all three
autonomous historical-prefix SQL recipes and the publication-aware capture
mutation gate in SQL. A real post-Published capture also passes the r6 native
recipe. The complete autonomous Ready SQL DATA reader passes its two r7
recipes, including source removal and corruption refusals. Read-only physical
reopening without original holders/source connection passes the r8 native
recipe. Ordinary mutable admission remains open; its reviewed handoff contract
and r8 evidence are in the [admission contract](worktrees-partial-vfs-managed-admission-contract.md).
See the managed checkpoint for preserved failures and earlier evidence.

**27 October, r11 continuation:** the remaining three pure observation recipes
now pass after two fixture-only corrections retained in the r9/r10 evidence.
Together with the first six unchanged r9 recipes, this qualifies the bounded
observation DATA slice; live authority, expansion persistence and content import
remain open. The shared SQL/DATA content-mapping commitment recipe and both
private schema9 historical-reader recipes pass on r11. The latter preserve
import provenance while permitting later local capture/publication/claim and
operational-journal DATA, and reject ACK/archive/schema corruption without
repair. They do not qualify an admission writer or ordinary mutable opening.
Independent review identified and ROOT corrected missing checkpoint-owner and
effective-image bindings in the reconcile kernel. Eight original pure recipes
pass on r12; the two new production-helper recipes pass on r13 after correcting
fixture enum-field names. This qualifies the DATA slice, not a real checkpoint
constructor, physical byte hashing/quiescence, full SQL closure or durable
companions. Seven admission-sizing recipes also pass on r12. The transactional
admission writer passes its independent review, exact-union pure recipe and all
four SQL-memory recipes on r13: exact archive/ACK/replay, rollback to private8,
changed journal/historical ACK refusal, and fixed legacy-budget refusal.
Physical durable admission and ordinary mutable access remain open.

**27 October, r14-r15 continuation:** the private admitted-origin adapter and
retained family catalog policy are integrated and compile. The three private
reader and four family-reader SQL-memory recipes pass on r14; the staging
directory alias regression passes on r15. Independent review closed findings
on aggregate charged-chunk memory and on preventing ordinary staging from
aliasing an exact retained directory before physical effects. The separate
content policy, catalog I/O adapter, and exact private-representation reuse are
implemented and reviewed, but the public ordinary opener is not yet connected.
The physical admission driver and source-independent resolution of a retained
failure holder are implemented and reviewed. The first r14 native attempt
failed in a fixture byte witness of the exclusively held owner lock (Windows
error 33); it is preserved. The r15 corrected native run is recorded separately
in the [admission contract](worktrees-partial-vfs-managed-admission-contract.md).
It passed in 138.74 seconds: real admission, refused close ACK, destination-only
resolution after source disconnect, and real private9-to-family context linkage.
No source deletion, process restart, ordinary edit/capture/restore, or GC
qualification is inferred from the SQL reader results.

r11 exact host evidence: `.layrs-local/validation/provider-family-publication-20261027-r11`;
EXE SHA-256 `989dde773830ba69ab6ac880f54c6df9c3fd6d0a5b7bbd97469cac6c944f2ec0`,
CodeView age 85, 551 retained source inputs. Results are in
`transition-observation-pure-results.json` and `managed-admission-sql-results.json`.
This newer evidence is in the separate portable
`admission-evidence-20261027-r11-r13.tar.gz` supplement; the r1–r10 archive is unchanged.

r12: EXE `9bee68b21aeef48bbb7a26ceafb0a26dc1d89db62f2af8a59a9ae0e73f932cdb`,
CodeView age 86, 555 inputs. r13: EXE
`16f86c9e9f577a296c193b42e05746d4383887e77d4a85e0bbbdda19b26ecac7`,
age 88, 557 inputs. The failed r12 fixture recipe is preserved; the eight
unchanged passing bodies were not rerun merely to obtain one green revision.

This branch is a work-in-progress checkpoint, not whole-programme or product
acceptance. It saves the existing source, tests, scripts and programme documents.
No new full-suite run was performed for the checkpoint commit.

**26 October continuation:** the portable packet has been verified and restored
on the second workstation. The [catalog checkpoint](worktrees-partial-vfs-catalog-checkpoint.md)
records the following metadata-only slice and its separate local qualification.
The evidence below retains its original writer R7 scope.

## Accepted bounded writer milestone

The recorded writer R7 run passed all 156 ordered selections (137 owned-process
recipes and 19 interruption recipes), with no retry. Independent source reviews
and the subsequent runtime review found no new supported P0–P3 findings in their
reviewed scope. ROOT accepted those source-defined assertions.

The endpoint is `control_published`, with the family member still `initializing`,
publication-only resume and strict no-claims abandon. It does not qualify product
hosts, activation, source serving, cleanup/GC, power-loss recovery or hostile-writer
atomicity. The native rename-error branch remains source-established without a
demonstrated native-error recipe. R5's nine pure passes retain their historical
unchanged-body provenance; they are not fresh R7 executions. Earlier failures,
including the R6 receipt-polarity failure, remain failures.

Local evidence packet:
`.layrs-local/validation/l5-store-stage1b-writer-r7-bdbc821b61a64108b8355ee9771f6df1`

- Source manifest SHA-256:
  `3915A6FC143A73177C59B08321B06408F574C7A5F9B212414D976052937271D5`
- Runtime subdirectory: `owned-runtime-dea358a7a7a049b588aa5d23291eeb11`.
- Runtime manifest SHA-256:
  `B5BA7D4490B051B5595D6BD69870F7FBD0733CA8557C675287461F5F2988DC48`
- Accepted `review-disposition-r1.json` SHA-256:
  `F5C83B5DFE4F33D07681D1A31FF31D92B674F55CD4F6984DC518705D00D24FBF`

The first checkpoint omitted the ignored `.layrs-local` evidence. The follow-up
[portable continuation packet](../continuation/worktrees-partial-vfs/README.md)
now carries the contracts, review records, logs and frozen source copies in an
archive that restores their original relative paths. Binaries, build outputs and
native fixtures remain local. Preserve them; do not clean or retry fixtures.

## Remaining work

- The [next read-only design proposal](worktrees-partial-vfs-next-slice-proposal.md)
  is a bound bootstrap catalog/descriptor handoff from the retained published
  holder. This was pending at the 25 October checkpoint; its reviewed
  implementation and qualification are recorded in the continuation above.
- Durable post-bootstrap registration, provider preparation/population, serving,
  capture, managed-member/companion integration and expansion remain separate
  obligations. CLI/Desktop activation needs its own host qualification.
- Race/performance optimization remains HOLD. The namespace-race investigation
  observed `STATUS_ACCESS_DENIED`; it did not establish the denied capability or
  final physical namespace state. The R6 queue-order incident remains preserved.
- Prior first-live inspection/recovery coverage spans multiple binaries; it is
  not an all-38 run on one revision. Earlier programme documents retain their
  historical scope and should be read with this later checkpoint.

No new runtime, product activation, cleanup or automatic retry is authorized by
this checkpoint.
