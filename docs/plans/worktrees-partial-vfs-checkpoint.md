# Worktrees / partial VFS checkpoint — 2026-10-25

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
