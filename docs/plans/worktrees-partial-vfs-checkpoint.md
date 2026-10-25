# Worktrees / partial VFS checkpoint — 2026-10-25

This branch is a work-in-progress checkpoint, not whole-programme or product
acceptance. It saves the existing source, tests, scripts and programme documents.
No new full-suite run was performed for the checkpoint commit.

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
  holder. It still needs independent design review and source authorization;
  no implementation was started.
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
