# Worktree benchmark protocol (L0/L6)

`scripts/measure-worktrees.mjs` measures the existing native CLI on fresh,
retained synthetic checkouts. It does not build the CLI or qualify native VFS.
The root agent must finish CLI validation and coordinate a quiet measurement
slot before running it. This document records a protocol and the limited pilot
cells below, not full-matrix performance results. No speedup or memory claim
follows from implementing the runner or these smoke runs.

**Complete-audit gate:** default `doctor` cannot completely audit the unique
10k/99k fixtures: its 1,024-finding option limits work to 65,536 metadata rows.
Every case now requires an explicit current read-only auditor, used identically
for baseline and candidate format-2 Stores. Root compiled the auditor, passed
its 12 Windows tests and completed the two independent-copy pilot cells below;
R5 independent review accepted the physical-accounting fixes. The R5 and R6
two-agent shared pilots remain failed; the fresh R7 code, assets and mixed
32-file/two-agent shared cells below passed the unchanged strict contract.
Larger fixtures and the full matrix
remain unqualified. The fixed
1,000,000-row budget is checked against actual Store
rows/history, followed by mandatory `clean: true` **and** `complete: true`.
Over-budget or truncated cases fail; the default matrix is never reduced.

## Invocation

Use Node with its built-in APIs; Windows also uses the installed Windows
PowerShell and .NET through `scripts/measure-worktree-process.ps1`. The helper
loads a small in-process Win32 interop type with `Add-Type`; it installs no
dependency or service. Supply the exact binary, its actual build
profile, an explicit storage model, and `--audit-binary`/`--audit-profile`/`--audit-sha256`.
Profiles are operator declarations, not inferences from directory names.
The runner records the resolved binary
path, SHA-256 before and after the run, size, modification time, runner hash,
machine/OS/CPU/RAM/Node information, helper path/hash, launcher path, options,
and an optional provenance note. Environment variables are never serialized.
A changed CLI or auditor binary invalidates the run. Do not compile into either
binary path while measuring.

Historical R5 smoke command template, retained for provenance. **Fresh cohorts
must use the corrected retained candidate and auditor with the R7 pins below,
after root validation and quiet-slot coordination.**
The old paths/hash below are not the next-cohort selection:

```powershell
$auditBinary = (Resolve-Path .layrs-local/validation/benchmark-auditor-v4-9efbf58305f74c03a6813cf89776b290/measure_worktree_audit.exe).Path
$auditHash = 'af2bf4207ffa7062d90d3be3913d67448ff8ea8f24a69e31b41c16205b4924f9'
$candidateBinary = (Resolve-Path .layrs-local/validation/benchmark-candidate-r5-12a33a8d81b448a4bc3adcc4958bed0b/layrs.exe).Path
node scripts/measure-worktrees.mjs --binary .layrs-local/target-baseline/debug/layrs.exe --profile debug --mode independent-copies --audit-binary $auditBinary --audit-profile release --audit-sha256 $auditHash --counts 32 --agents 1,2 --run-note "Baseline provenance-r2 verified by root; quiet slot"
node scripts/measure-worktrees.mjs --binary $candidateBinary --profile debug --mode independent-copies --audit-binary $auditBinary --audit-profile release --audit-sha256 $auditHash --counts 32 --agents 1,2 --run-note "Retained R5 candidate; protocol smoke only; independent-copy control; root quiet slot"
node scripts/measure-worktrees.mjs --binary $candidateBinary --profile debug --mode shared-worktrees --audit-binary $auditBinary --audit-profile release --audit-sha256 $auditHash --counts 32 --agents 1,2 --run-note "Retained R5 candidate; protocol smoke only; root quiet slot"
```

These were proposed broader smoke commands, not the exact executed cells below.
The retained R5 candidate is for protocol smoke only and predates later
integration fixes. Root must select and validate a new candidate before making
programme performance claims. Replace its path/profile/note with verified
provenance when authorized; the runner never substitutes another binary or
falls back to independent copies. The baseline executable lacks worktree
commands and must use `independent-copies`.

Root's [provenance-r2](../../.layrs-local/validation/baseline-provenance-r2.json)
records 1,202 archive matches and the following SHA-256
values (root-supplied evidence, not re-derived by this implementation agent):

- Baseline `.layrs-local/target-baseline/debug/layrs.exe`:
  `235653bfe69610ab6f03470bf810842469f077408b87ab814573b20ef9842ed2`.
- Worker: `9eeb615583e92959937ac3979148caf13b18ed84c01988e45a9face054a72df2`.
- Previous release auditor:
  `48cd4034721130336717d5802a893b613adec158351ce0353d952742cb942fda`.
  Root recorded its 90-second build and the prior 9 Rust/108 Node passes.
  This is historical build/validation provenance, not a benchmark timing. That
  auditor predates the backup-classification schema and is superseded by the
  retained auditor below; do not reuse its old hash with this runner.

The retained release auditor used for the R5/R6 reports has SHA-256
`af2bf4207ffa7062d90d3be3913d67448ff8ea8f24a69e31b41c16205b4924f9`.
Its validation copy remains immutable for those old reports while Cargo can
rebuild its target output independently. Root verified the hash
before/after copying; this agent also read and hashed the retained copy.
Its [provenance.json](../../.layrs-local/validation/benchmark-auditor-v4-9efbf58305f74c03a6813cf89776b290/provenance.json)
explicitly records that **no complete original build-time source inventory
exists**. Concurrent working-tree edits mean neither that inventory nor a
reproducible build is claimed. Matching immutable executable bytes establishes
the common auditor across those cohorts, not complete source/build reproducibility.
After the R6 disposition below, root rebuilt both candidate and auditor from
the same corrected source and retained/pinned the R7 binaries recorded below.
Every new baseline/candidate cohort must use that same new auditor SHA-256.
Old and new auditor cohorts must not be combined as a matched comparison.

For the full matrix, omit `--counts` (defaults to `1000,10000,99000`). To stage a
smaller measurement, use `--counts 32,1000`. Payloads `code,assets,mixed` and agent counts
`1,2` are defaults. Run each binary/model combination separately, with matching
options and hardware. A candidate `independent-copies` run is also useful to
separate binary changes from storage-model changes; it is not a third model.
The three 32-file commands above preserve the earlier proposed comparison;
their candidate and auditor selections are superseded for future runs. They have not been run
by this implementation agent.

### Executed pilot cells, 2026-10-24

Root executed exactly these three cells with debug CLI profiles, release auditor
SHA `af2bf4207ffa7062d90d3be3913d67448ff8ea8f24a69e31b41c16205b4924f9`,
`--counts 32 --payloads code --repeats 1 --cas-retries 2`, a 900,000 ms command
budget and 600,000 ms run budget. Reports retain the exact invocation, binary
hashes before/after, options and run notes. The pilots used the then-current
target release-auditor path with these same bytes, subsequently retained in the
immutable copy above. Caches were unpurged. Root's notes record no concurrent
Cargo/native tests; the shared pilot allowed author edits/read-only work.

- **Baseline, independent-copies, one agent: PASS.** Cell `code-32-1`,
  run `QrdRGJ`, 16 successful CLI commands and separate auditor command 17,
  with clean/complete integrity and validated accounting.
  [Report](../../.layrs-local/validation/layrs-worktrees-QrdRGJ/report.json),
  [run log](../../.layrs-local/validation/benchmark-baseline-smoke-r1.log),
  [raw audit envelope](../../.layrs-local/validation/layrs-worktrees-QrdRGJ/logs/17.stdout.txt).
- **Retained R5 candidate, independent-copies, one agent: PASS.** Cell
  `code-32-1`, run `ZLjalc`, 16 successful CLI commands and separate auditor
  command 17, with clean/complete integrity and validated accounting.
  [Report](../../.layrs-local/validation/layrs-worktrees-ZLjalc/report.json),
  [run log](../../.layrs-local/validation/benchmark-candidate-independent-smoke-r1.log),
  [raw audit envelope](../../.layrs-local/validation/layrs-worktrees-ZLjalc/logs/17.stdout.txt).
- **Retained R5 candidate, shared-worktrees, two agents: FAILED.** Cell
  `code-32-2`, run `z6rF0T`, terminal `success: false`. Initial capture commands
  17/18 succeeded. During the repeat capture, command 25 (agent 0) exited 1
  with `store lock is busy` (`LockBusy`); command 26 (agent 1) exited 0 and
  captured a Step. Command 25 has `retryScheduled: false`; the failed batch has
  zero external retries. The run stopped before final history/payload/audit
  qualification, with zero audit commands. One successful peer does not make
  the batch or shared case pass.
  [Report](../../.layrs-local/validation/layrs-worktrees-z6rF0T/report.json),
  [run log](../../.layrs-local/validation/benchmark-candidate-shared-smoke-r1.log),
  [command 25 raw failure](../../.layrs-local/validation/layrs-worktrees-z6rF0T/logs/25.stdout.txt),
  [command 26 raw success](../../.layrs-local/validation/layrs-worktrees-z6rF0T/logs/26.stdout.txt).

Both candidate reports use retained CLI SHA-256
`1c23a1d6d2a8806e6edae769de26651c142448000b286a8181979fd03d488ee9`;
its [R5 provenance](../../.layrs-local/validation/benchmark-candidate-r5-12a33a8d81b448a4bc3adcc4958bed0b/provenance.json)
limits it to protocol smoke. Root assigned production contention diagnosis to
L1. This runner's retry/failure policy remains unchanged: no `LockBusy` string
retry, repair, or conversion of the failed report into a pass. Raw reports,
logs and fixtures remain retained. Assets, mixed payloads, larger counts and
the remaining agent/model cells have no real-run qualification from these
pilots. These results support no performance or full-matrix claim.

### R6 shared pilot and root disposition

The quiet R6 shared-worktree cell `code-32-2`, run `GI8ZzO`, is terminal
**FAILED**. All **32 CLI commands succeeded**; exact 3/5-Operation histories,
private-byte checks and final payload checks passed. The retained unique
payload remains 36 manifests/chunks and 18,432 bytes. **Auditor command 33
failed**: 100 indexed objects / **79,378 bytes** versus 102 physical CAS files /
**81,854 bytes**, a difference of two files / **2,476 bytes**. The indexed audit
reported clean/complete with no findings, but physical accounting was not
reconciled; 99 objects were reachable and **one indexed object was unreachable**.
Repair backups contained two directories and zero files; staging contained only
the empty lease. These facts do not make the overall audit or cohort pass.
[Report](../../.layrs-local/validation/layrs-worktrees-GI8ZzO/report.json),
[raw command 33 audit failure](../../.layrs-local/validation/layrs-worktrees-GI8ZzO/logs/33.stdout.txt),
[root run log](../../.layrs-local/validation/benchmark-candidate-shared-smoke-r2.log).

The retained R6 candidate is in
`.layrs-local/validation/benchmark-candidate-r6-beeefa28e458429a945ecf30f97f344e`.
Its [provenance](../../.layrs-local/validation/benchmark-candidate-r6-beeefa28e458429a945ecf30f97f344e/provenance.json)
records 775 archived sources, zero source drift during the build, and these
SHA-256 values:

- CLI: `9bbcff914bee9e50c0a2630a64e8e4006a27ed8e10df9d0eb138ecdba503c0ab`.
- Worker: `cc119e9cb86cd20ad16a854b171ec150c1d976f3362780fc761547ba7bbfa57c`.
- Source archive: `07a0d566350395034654da571aa89ed3a0d6850909678d96a2f33a000f16a177`.

This pilot used the unchanged common auditor `af2bf420…`. Its candidate snapshot
and source provenance are evidence for this failed protocol cell, not final
programme performance qualification.

Root reports that L1 independently traced the two unindexed RepositoryViews to
failed internal capture attempts, with no SQL references:

- `34c0d31eec7e4fe220e626570e38fba1b7f47688c1d737d108ac2573c97cd798`,
  1,201 bytes, command 18.
- `479518e63895678eac0cea52ff608c87e97b1da316cab05be205ce1a22d73634`,
  1,275 bytes, command 26.

The historical admitted empty Tree
`186bcc84729af1f82339e14aff729fe09b9c0d0f8706f3d8668aed591f60949d`
is a canonical historical View dependency omitted by reachability. Root
classifies this as a **retention defect**, not accepted lifecycle garbage.
Successful captured logical content therefore remains distinct from the failed
attempt metadata artifacts and the missing historical retention dependency.

Root approved two bounded production corrections owned by L1: an exact stale
check before CAS installation for **new transactions only**, preserving existing
receipt replay/collision behavior; and reachability for canonical historical
View Worktree dependencies. At this R6 disposition, targeted runtime validation
and independent review were pending; the subsequent R7 evidence is recorded
below. No cleanup, auditor/runner relaxation, report reclassification
or rerun against the retained fixture is authorized by this disposition.
Root required rebuilding and pinning both candidate and auditor from the
corrected source before a fresh cohort; the old auditor and failed reports remain intact.
Matching the new auditor SHA across all new cohorts is mandatory. No performance
claim follows from this failed pilot or the corrections.

### R7 strict shared code pilot: one passing cell

Fresh run `UFmI6b` is terminal **PASSED**, `success: true`, for exactly one
`shared-worktrees` cell: **32 code files, two agents**, one repeat and the
unchanged two-retry ceiling. All **32 CLI commands succeeded** and separate
**audit command 33 passed**, with `contractValidated: true`. Exact 3/5-Operation
histories, all four private-byte checks and both final 32-file/16,384-byte private
payload checks passed. All recorded external retry counts are zero; internal
retry counts remain unmeasured.
[Report](../../.layrs-local/validation/layrs-worktrees-UFmI6b/report.json),
[raw command 33 audit](../../.layrs-local/validation/layrs-worktrees-UFmI6b/logs/33.stdout.txt),
[root run log](../../.layrs-local/validation/benchmark-candidate-shared-smoke-r3.log).

The audit is clean and complete, with no findings or truncation. Indexed and
physical CAS agree exactly at **100 objects / 79,378 bytes**; all **100 are
reachable**, with **zero unreachable objects**. Physical accounting is complete
and reconciled. Repair backups contain two directories and zero files, and
staging contains only the empty lease. The strict zero-unreachable and exact
physical/indexed checks were retained, with no runner or validator relaxation.

Both binaries come from the retained same-source folder
`.layrs-local/validation/benchmark-candidate-r7-b757cc1adcf64618b281d60dbd2c33ab`.
Its [provenance](../../.layrs-local/validation/benchmark-candidate-r7-b757cc1adcf64618b281d60dbd2c33ab/provenance.json)
records a complete 784-source archive and zero source drift during the builds:

- Debug CLI `layrs.exe`:
  `f000d4c4267f3ec169df8a1a63d18001d279e42a13c5d93a7a33ccae7210dab4`.
- Release auditor `measure_worktree_audit.exe`:
  `f1f047819fad7d6b601ce4769552f134fb9911584896c284762549b0c39db650`.
- Debug worker `layrs-lens-worker.exe`:
  `cc119e9cb86cd20ad16a854b171ec150c1d976f3362780fc761547ba7bbfa57c`.
- [Complete source archive](../../.layrs-local/validation/benchmark-candidate-r7-b757cc1adcf64618b281d60dbd2c33ab/source-complete.tar):
  `253f61cb978132955351f7d410d92145cf072d92ef5bfffdc121b4dd0eaf69c0`.

The report's CLI and auditor hashes match those pins before and after the run.
All new baseline/candidate cohorts must use this same retained release-auditor
path/hash; the older `af2bf420…` auditor remains immutable for its old reports.
This pass does not change `GI8ZzO` or `z6rF0T`: both remain terminal **FAILED**
with their original logs and fixtures retained.

The [R7 auditor test log](../../.layrs-local/validation/benchmark-auditor-r7.log)
records **12 passed, zero failed**. Root additionally reports 57 substantive
Linux passes across Docker and Ubuntu, release of that resource slot, and final
L1 Opus R2 review with no P0/P1/P2 findings; those are root-reported validation
and review evidence, not additional benchmark cells.

The report records **146,295.46 ms total run wall time**. This includes setup,
verification and audit; it is protocol-run evidence, not a performance result.
Its run note records a quiet slot after Linux resource release, without
concurrent Cargo/native/Linux tests, and uncontrolled caches. Shared capture
wall times still include lock wait/serialization without separate counters.
The quiet code pilot has ended. It qualified only `code-32-2`; the subsequent
assets/mixed cells are recorded separately below. This pass does not qualify
the full matrix, native VFS or programme performance.

### R7 shared assets/mixed follow-up: two passing cells

Fresh run `peAfGZ` is terminal **PASSED**, `success: true`, with exactly two
shared cells: `assets-32-2` and `mixed-32-2`. The recorded UTC interval is
**2026-10-24T18:26:46.198Z to 2026-10-24T18:31:25.172Z**. Each cell completed
**32 successful CLI commands**, plus its separate successful audit. Both used
one repeat, a two-retry ceiling and 900,000 ms command/run budgets. All recorded
external retry counts are zero; internal retry counts remain unmeasured.
[Report](../../.layrs-local/validation/layrs-worktrees-peAfGZ/report.json),
[root run log](../../.layrs-local/validation/benchmark-candidate-shared-assets-mixed-r7.log).

- **Assets:** 32 asset files totaling **131,072 bytes per checkout**. All four
  private-byte checks passed. [Audit command 33](../../.layrs-local/validation/layrs-worktrees-peAfGZ/logs/33.stdout.txt)
  reconciles indexed and physical CAS exactly at **100 objects / 208,484 bytes**,
  with 100 reachable and zero unreachable objects. The retained payload is
  exactly **36 manifests/unique chunks / 147,456 bytes**. Total Store accounting
  is **1,027,864 bytes**, plus a separately counted 1,532-byte secondary locator.
- **Mixed:** 16 code and 16 asset files totaling **73,728 bytes per checkout**.
  All eight private-byte checks passed, covering both edited paths in each
  agent/round. [Audit command 66](../../.layrs-local/validation/layrs-worktrees-peAfGZ/logs/66.stdout.txt)
  reconciles indexed and physical CAS exactly at **108 objects / 155,515 bytes**,
  with 108 reachable and zero unreachable objects. The retained payload is
  exactly **40 manifests/unique chunks / 92,160 bytes**. Total Store accounting
  is **995,375 bytes**, plus a separately counted 1,529-byte secondary locator.

Both raw audit envelopes agree with their retained report results and are
clean/complete, with zero findings or truncation and `contractValidated: true`.
Physical accounting is complete and reconciled; every Store category sums
exactly to the total. Each Store has two repair-backup directories, zero backup
files and only the empty staging lease. Both cells passed exact 3/5-Operation
histories, expected Step/Operation/parent identities, and final payload checks
for both private 32-file checkouts. No validator or retry policy was relaxed.

CLI `f000d4c4…` and auditor `f1f04781…` match the full R7 pins and same-source
provenance above, unchanged before/after this run. Runner SHA-256
`b41530ebc29337236034718d68da9e159aa8654b909648bad89b61cfc2643d98`
matches both the earlier R7 code report and the unchanged runner file. The run
note records a serialized native slot with authors allowed to patch/read only.
Root reports the process finished and the quiet slot was released.

Recorded elapsed wall times are **278,974.87 ms for the run**, **143,275.51 ms
for assets**, and **135,578.47 ms for mixed**. These include setup/verification
and audit and are descriptive protocol timings only. Author activity was
permitted, caches were not purged, and no comparative performance claim follows.
Capture wall times include lock wait/serialization without separate measurement.

Together with the separate `UFmI6b` code pass, all three payload shapes now have
**32-file/two-agent shared functional passes**. `GI8ZzO` and `z6rF0T` remain
terminal **FAILED** with all original evidence retained. Single-agent cells,
baseline and candidate independent-copy controls with the common R7 auditor,
the full 1k/10k/99k matrices and repeated release-profile comparisons remain
pending. Earlier independent-copy pilots retain their historical scope and old
auditor pin; they do not supply those new matched controls. Native VFS and
programme performance remain outside these functional passes.

### Release artifacts and baseline code-1k bounded run

Root built and retained release-profile CLI/worker artifacts for both cohorts:

- Baseline folder
  `.layrs-local/validation/benchmark-baseline-release-r1-0761d97b4edd450aaa006cf5d28f5e00`:
  [provenance](../../.layrs-local/validation/benchmark-baseline-release-r1-0761d97b4edd450aaa006cf5d28f5e00/provenance.json),
  [source inventory](../../.layrs-local/validation/benchmark-baseline-release-r1-0761d97b4edd450aaa006cf5d28f5e00/source-manifest.json),
  [build log](../../.layrs-local/validation/benchmark-baseline-release-build-r1.log).
  Provenance records 1,202 exact archive-source files and no drift, baseline
  commit `4601b1694a4737183049e3c12124141b24860b92`, archive SHA-256
  `8708a0c8521c4a9549b7903c7d550c36d5a34ffe7542434a9dded99ec5268d02`,
  and release CLI SHA-256
  `5508198aa3827e12042cf6078e70becd887aee4984b88a537c4c23724a3e0ffa`.
- R7 candidate folder
  `.layrs-local/validation/benchmark-release-r7-a61dcd2852304b2b930883ece51f33ed`:
  [provenance](../../.layrs-local/validation/benchmark-release-r7-a61dcd2852304b2b930883ece51f33ed/provenance.json),
  [source inventory](../../.layrs-local/validation/benchmark-release-r7-a61dcd2852304b2b930883ece51f33ed/source-manifest.json),
  [build log](../../.layrs-local/validation/benchmark-release-r7-a61dcd2852304b2b930883ece51f33ed/build.log).
  Provenance records 784 exact archive-source files and no drift, the same
  corrected R7 archive `253f61cb978132955351f7d410d92145cf072d92ef5bfffdc121b4dd0eaf69c0`,
  and release CLI SHA-256
  `66e95351a3e63a3f940f3ad6b017842086978ce684c0272b9f7b935bceb4b3c4`.

The common release auditor remains the retained R7 `f1f04781…650` executable
with the full pin above. These release CLI builds are distinct from the debug
CLI snapshots used in the earlier functional pilots; build completion is not
candidate benchmark qualification.

Fresh baseline run `jwXIsr`, **release independent-copies**, requested code /
1,000 files / agents 1 and 2 / one repeat. It is terminal **FAILED** because
the **1,200,000 ms whole-run budget** expired during the second cell. The
per-command budget was 900,000 ms and was not exhausted by either failing
command. UTC start/end are **2026-10-24T18:48:54.306Z to
2026-10-24T19:08:54.407Z**; recorded run wall time is **1,200,101.35 ms**,
including termination/report overhead.
[Report](../../.layrs-local/validation/layrs-worktrees-jwXIsr/report.json),
[root run log](../../.layrs-local/validation/benchmark-release-baseline-code-1k-r1.log).

- **One-agent cell PASSED:** 16 successful CLI commands plus separate
  [audit command 17](../../.layrs-local/validation/layrs-worktrees-jwXIsr/logs/17.stdout.txt),
  **605,039.13 ms** cell wall time. Audit clean/complete with no findings or
  truncation; indexed/physical CAS agree exactly at **2,019 objects /
  1,562,450 bytes**, all reachable and zero unreachable. The two private-byte
  checks, exact three-Operation histories and final private payload of 1,000
  files / 512,000 bytes passed. Retained payload is exactly 1,002 manifests /
  unique chunks / 513,024 bytes.
- **Two-agent cell is incomplete and failed:** commands 18–31 succeeded.
  Commands **32/33**, `dirty-diff`, invocation 0, both timed out with exit -1
  after **51,610.21 / 51,604.60 ms**. Their starts left only approximately
  51,578 / 51,573 ms of the global budget. Both raw stdout/stderr files are
  empty, both have `retryScheduled: false`, and no retries occurred. There
  were no capture/private-byte/history/final-payload qualification results
  and **no audits** for this cell.
  [Command 32 helper evidence](../../.layrs-local/validation/layrs-worktrees-jwXIsr/logs/32.launcher.stdout.txt),
  [command 33 helper evidence](../../.layrs-local/validation/layrs-worktrees-jwXIsr/logs/33.launcher.stdout.txt).

The report's baseline CLI and auditor hashes match before/after and were
checked against their retained copies. The run note records a root-owned quiet
slot, no concurrent builds/native tests, authors patch-only, and unpurged
caches. Root reports the process terminal/reaped and the quiet slot released.
These descriptive timings and the global-budget failure establish neither a
source defect/data corruption nor performance superiority. The passed first
cell does not turn the overall run or second cell into a pass.

A fresh two-agent baseline cell with an adequately sized whole-run budget
remains pending; the retained incomplete fixture is not a retry target.
Candidate release controls, the complete 1k/10k/99k matrices, repeated
release-profile comparisons and performance claims remain pending. All earlier
reports retain their original outcomes, including `GI8ZzO` FAILED and the
separate R7 code/assets/mixed 32-file/two-agent shared functional passes.

### Additional options

- `--audit-sha256 HEX`: **required** expected 64-digit SHA-256 pin for the identical
  auditor across all cohorts. Missing/malformed pins and mismatches fail before
  fixtures or CLI launch. Comparison also requires matching CLI `profileDeclared`,
  auditor `profileDeclared`, and auditor SHA-256; root verifies the baseline's
  actual build provenance before any real run. A matching label alone is not provenance.
- `--payloads code,assets,mixed`, `--agents 1,2`: select subsets explicitly.
- `--repeats 1`: one additional invocation after the first, bounded to 1–5.
- `--cas-retries 2`: at most two additional external capture attempts, bounded
  to 0–5; internal CLI retry counts remain unknown.
- `--timeout-ms 900000`: command budget, including launcher startup, at most
  15 minutes. The helper terminates its owned CLI when the remaining budget
  expires, with a 10 ms polling interval plus OS scheduling latency.
- `--run-timeout-ms 14400000`: four-hour whole-run budget (maximum 12 hours),
  checked during fixture generation/verification and applied to active commands.
- `--output-parent PATH`: an **existing** directory; default is the OS temporary
  directory. Every invocation allocates a fresh `layrs-worktrees-*` child.
- `--help`: prints the complete interface without creating files or processes.

CLI calls are shell-free with `--json`. Report paths and raw stdout/stderr paths
are printed or recorded. No dependency installation, Cargo, cleanup, reset,
recovery, GC, publication, or modification of an existing checkout occurs.

## Deterministic fixtures and storage models

Each case is `(payload, logical file count, agent count)`. Counts are files per
checkout, not the sum across agents. There are at most 1,000 files per shard
directory, one payload directory, and no extra tracked metadata files. At 99,000
files this is 100 directories, or **99,101 entries including the checkout root**,
below the 100,000-entry materialization budget. `.layrs` is CLI metadata, not
tracked fixture payload. The runner rejects counts above 99,000.

- Code: unique, valid 512-byte JavaScript modules with an index, variant, and
  SHA-256-derived comment. These are deliberately compressible source files.
- Assets: unique 4,096-byte deterministic SHAKE256 output, with a four-byte
  file index. This approximates poorly compressible assets, not large media.
- Mixed: alternate code and assets by index within the same shard directories.
  Even indices are code; odd indices are assets. Counts must be at least two.
  For odd counts, code has one extra file. The report records `codeFiles`,
  `assetFiles`, and their weighted byte total. At 99,000 files the mix is
  49,500 of each, totaling 228,096,000 payload bytes per checkout, with the same
  99,101-entry budget as the pure cases. Payload bytes reuse the corresponding
  pure-profile generator; initial and final verification covers both types.
- Both models use identical paths and bytes for each payload/count, and identical
  agent/round edit variants. These are not the old 200-payload duplicate fixture.
  Generation is outside measured CLI durations. Initial and final verification
  reads every payload, checks its expected bytes, regular-file type and link
  count, counts logical files/bytes, and records an ordered content digest.

In `independent-copies`, agent 0 is initialized in a new payload checkout.
Agent 1 gets an ordinary payload-directory copy and a separate `init`, creating
another complete Store. Copy time and each initialization are recorded
separately. A `.layrs` directory is never copied because it contains checkout
identity. This represents independent complete copies with duplicated Store
cost, not shared immutable storage and not a history-preserving clone.

In `shared-worktrees`, agent 0 owns a complete Store. Its JSON status identifies
the current Worktree entry in `state.view.worktrees`; the runner uses that
entry's **explicit `activeLayerId` and exact `baseStepId`** in
`worktree create EMPTY_DIR --layer ID --step ID`. The second checkout must have
a distinct Worktree identity, the same Store identity, and the requested Layer
and Step. One-agent cases use only the primary checkout.

## Measured sequence and correctness

After setup and full payload verification, the runner measures coordinated
clean `status` and `diff --stat` calls, first invocation then repeats. For each
edit round it writes distinct agent bytes into the **same relative paths** in
each private checkout: one file for pure profiles, one code file and one asset
for mixed. It then coordinates `status`, `diff --stat`, and `step` processes
across one or two agents. Launch requests for a batch start in one event-loop
turn; Windows helper startup can skew actual native process starts. Every
attempt records launcher/native PID, wall duration, exit/signal, JSON success,
timeout, output caps, counters, error and raw log paths. Batches distinguish
launcher overlap from `observedNativeOverlapMs`, computed from Windows process
creation/exit timestamps. Zero means no observed native overlap; unavailable
timestamps produce `null`. A simultaneous launch request or overlapping
launchers is not proof that CLI captures overlapped or forced a CAS collision.
Shared capture wall durations include any lock wait and serialization within
the CLI. Neither is separately measured, and observed native process overlap
does not establish parallel Store writes or quantify time waiting for a lock.

Diff output is correctness evidence: a clean response must contain the CLI
`changes` array and nothing else, with zero entries. A dirty response must
contain exactly the edited paths, each `modified`, with exact before/after byte
counts (512 or 4,096), unchanged executable mode, and different nonempty digests.
Missing/extra/duplicate paths, missing arrays, wrong byte counts, and
`fixtureOnly` placeholders fail the case. Validated changes are retained in
`diffChecks`; raw envelopes remain in command logs.

Only the CLI's known compare-and-swap mismatch or stale-capture message permits
a bounded external retry of `step`, with a fresh process reading a new base.
Failed attempts, retry decisions, and retry counts remain visible in the JSON
report. Unknown errors (including `LockBusy`), malformed output, timeout and
output-limit failures are fatal; there is no generic retry, repair, or silent
success. Native capture can perform internal retries without exposing a count; `internalRetryCount`
is `null`, never fabricated as zero. A recovered external CAS attempt can yield
a successful batch, while its failed process remains recorded.

Each capture must produce a distinct accepted Step. Fresh `status` processes
must reopen each checkout cleanly at its own captured Step, and the edited bytes
must still match that agent's variant. After all rounds, full-payload checks
verify untouched bytes too. The separate current auditor invokes native fsck;
both `clean` and `complete` must be true for each complete Store. This checks live retained
variants, not crash recovery, restore, or every historical payload under eviction.
After capture rounds, first/repeat `timeline --limit 10` calls measure bounded
local Operation history. The runner validates Operation schema, Space, unique
IDs, kinds, observed capture/setup IDs, and the complete parent chain ending
at the observed initialization Operation. The fixture's natural lifecycle
requires `repeats + 2` Operations for independent copies and the shared primary;
the shared secondary has two additional `restore` Operations for registration
and materialization. With the default two captures these counts are **3 versus
5**. The maximum configured history is nine Operations, below the display cap.
`histories` retains actual/expected cardinalities, kinds, IDs and parent edges.

No lifecycle is altered to manufacture a match. All history commands/batches
are explicitly marked with `comparisonExclusion`; history summaries carry
`equalWorkComparisonEligible: false`. These timings are retained as lifecycle
diagnostics and excluded from equal-work performance comparison. A later
analysis may only establish a separate matched cohort using actual cardinality,
ancestry, output size, binary profile and Store workload; matching a `--limit`
argument alone does not establish equal work.

After fsck, the auditor enumerates each Store under its shared lock, rejecting
symlinks, Windows reparse points and special filesystem entries. Enumeration is
bounded to 2,000,000 entries and depth 16. CAS files use
`objects/<kind>/<digest[0..2]>/<digest[2..4]>/<digest>`; `cas::encode_object` writes
raw chunks or canonical envelopes, admission records the exact encoded
`bytes_len` in `objects.byte_length`, and fsck verifies every indexed path and
actual file length. Thus `.layrs/objects` must contain exactly the indexed
`objects` row count in regular files, with total length exactly
`canonicalObjectBytes + chunkBytes`. Extra unindexed files fail even if empty;
the complete fsck check separately catches missing/replaced indexed content.

`physicalStore` retains complete/reconciled flags, indexed expectations, actual
category counts, bounded-enumeration evidence and any error. Staging is counted
separately and must contain only the mandatory zero-byte `.lease` control file.
Every other staging entry, including an empty file/directory, fails completion.
Partial counts and up to eight staging paths survive in the failed audit result;
the runner preserves its report and fixtures. Nothing is deleted or recovered.

`storeBytes` uses those reconciled regular-file lengths, counting the shared
Store once and independent Stores separately. Each `stores` entry and the
case's `storeByteBreakdown` separate `indexedCas`, `databaseAndMetadata`,
`repairBackups`, `other` and `staging`; their sum must equal total Store bytes.
`metadata/repair-backups` contains displaced checkout payloads from materialize.
It is classified before general metadata and counted only in `repairBackups`,
which records files, bytes, directories (including the backup subtree root)
and up to eight sample paths. It is retained evidence, not database overhead.
Metadata includes the SQLite database, sidecars and remaining files under
`metadata/`; its bounded samples exclude the database, WAL and SHM paths and
exclude the entire backup subtree. `other` includes FORMAT and entries outside
the named subtrees, with up to eight sample paths. Samples include files and
directories, follow filesystem enumeration order and are not exhaustive inventories.
Staging's permitted
control file contributes one file and zero bytes. These are enumerated file
lengths, **not allocated disk blocks**. `privateCheckoutBytes` covers verified editable payload
bytes across agents; it excludes locator files and is **not process private
memory**. File sizes are not allocated blocks or
physical disk usage, especially on compressed/reflink-capable filesystems.
`sharedLocators` records each secondary `.layrs` locator's path and regular-file
length, `sharedLocatorBytes` sums them (zero for independent copies), and
`storeAndLocatorBytes` includes them alongside the complete Store bytes.

## Complete audit contract and root qualification

`crates/layrs-store-local/examples/measure_worktree_audit.rs` is a standalone
read-only measurement example using existing dependencies. It does not change
production defaults, schema versions or APIs. Root compiles one current auditor
and supplies that same binary/hash for every cohort, including the older baseline.
The auditor is qualification evidence, not part of measured CLI work. Use a
**release auditor with its SHA-256 pinned identically for both cohorts** to keep
qualification cost practical at 99k files. Its profile is independent of the
baseline/candidate CLI profiles, which must still be matched for comparison.

Exact build and example-test commands for root's serialized slot (not executed
by this implementation agent):

```powershell
cargo build --locked --release --target-dir .layrs-local/target -p layrs-store-local --example measure_worktree_audit
cargo test --locked --target-dir .layrs-local/target -p layrs-store-local --test measure_worktree_audit
```

The build output is `.layrs-local/target/release/examples/measure_worktree_audit.exe`.
Root retains each qualified build separately. Following the R6 disposition,
fresh cohorts use the immutable R7 auditor copy/pin recorded above from the
corrected source; the old validation copy remains tied to the old reports.
Its interface is `measure_worktree_audit --json --store ABSOLUTE_STORE_ROOT
[--checkout ABSOLUTE_CHECKOUT_ROOT] [--checkout ABSOLUTE_SECOND_CHECKOUT_ROOT]`;
checkout arguments are optional for standalone Store inspection, limited to two
distinct absolute paths, and mandatory for every checkout when called by the runner.
there is no repair, tuning, migration, cache-purge or incomplete-audit option.
For example, after an authorized smoke run has created a Store:

```powershell
& $auditBinary --json --store "C:\absolute\retained-fixture\agent-0\.layrs" --checkout "C:\absolute\retained-fixture\agent-0"
```

One invocation audits one complete format-2 Store; the shared Store is audited
once, independent Stores individually. The caller's verified payload/file counts
and complete captured Operation histories remain separate evidence and must
agree with the audit's observed Store/worktree/Operation/Step counts.

The example implements this fixed contract:

1. Validate the exact Store root/format and acquire the existing read-only
   `NativeStoreAudit` capability. It must not migrate, recover, checkpoint,
   clean, or write into the Store. Any supplemental metadata preflight must use
   a correctly encoded immutable read-only SQLite URI under the held shared
   lock, reject links/nonempty WAL, and create no sidecars. The library's audit
   connection accessor remains crate-private; the example opens its own immutable
   read-only connection without changing production APIs.
2. Count the actual `Context::check_work_budget` table union with an aggregate
   bound of 1,000,001 rows, retaining per-table counts and actual file-manifest,
   Step, Operation, parent-edge and Worktree cardinalities. This includes
   retained fixture/history and any additional persisted objects; it must not
   assume that file count equals audit work. Refuse over-budget/incomplete
   preflight. `spaces` is counted separately because fsck's work-row union does
   not include it. Spaces, Steps, Operations, Worktrees and Step/Operation parent
   edges each have an additional 256-row ceiling. Identity strings are bounded
   to 160 bytes. The runner checks exact Step/Operation/Worktree/Space ID sets
   and Operation parent edges against its observed state and full timeline.
   File-manifest cardinality and retained file bytes must equal the initial
   unique files plus every accepted distinct edit, including both mixed edits.
   The existing capture path uses 1 MiB fixed chunks; these unique 512/4,096-byte
   versions each occupy one distinct chunk. Manifest-chunk and unique-chunk
   counts must therefore equal manifest counts, and unique chunk bytes must
   equal retained file bytes. The runner also requires zero unreachable objects.
3. Use the current `NativeFsck::run` with exactly
   `FsckOptions { max_findings: 15_625 }`. Existing derived bounds are 1,000,000
   work rows, 256 MiB canonical metadata, 4 GiB chunk reads, 4 GiB file reads,
   and 4,000,000 traversal work units. The preflight reports these budgets and
   measured counts/available byte totals; it does not prove traversal or byte
   budgets suffice. `preflight.observedCountsWithinBudgets` replaces the ambiguous
   `withinBounds` name: observed bytes are lower bounds on total audit work.
   The auditor's final completion result remains authoritative.
4. Validate every supplied checkout through the existing read-only
   `worktree_audit::resolve_store_for_audit` API, before and after fsck under the
   held shared Store lock. This checks the registered filesystem/locator binding
   and requires the exact Store root. The report records `checkoutBindings`; the
   runner rejects missing, duplicated, unexpected or only partially checked roots.
   No production API is exposed or changed for this measurement.
5. Enumerate and reconcile physical Store files as described above, after the
   clean/complete native audit, with no filesystem mutation. Over-budget walks,
   unsafe entries, count/byte mismatches and staged residue fail qualification
   even when the separate native fsck result is clean and complete.
6. Return a JSON envelope (`data.schemaVersion: "layrs.worktree-audit.v4"`)
   with `ok`, `data.storeId`, `data.storeRoot`,
   `data.formatVersion`, `data.preflight`, `data.budgets`, `data.clean`,
   `data.complete`, `data.checkedObjects`, `data.checkedRows`,
   `data.truncatedCount`, findings and retention/reachability counts. Exit 0
   requires `clean === true`, `complete === true`, `truncatedCount === 0` and
   successful preflight. Budget exhaustion, unsupported format, missing rows,
   timeout, output truncation or I/O failure must retain a failed/incomplete
   report and exit nonzero. Never relabel a truncated audit as passed.

The runner requires explicit `--audit-binary PATH`, `--audit-profile LABEL` and
`--audit-sha256 HEX`, records the auditor SHA-256 before/after, and verifies the
mandatory pin before creating any case. Comparison requires identical auditor
hashes and matching CLI/auditor declared profiles across cohorts.
After a case's timed CLI commands have exited and payloads are verified, each
Store is audited with the existing owned-process supervision and output caps.
Report schema `layrs.worktrees-benchmark.v5` places those commands, timings,
counters, expectations and raw `auditResult` in `case.auditValidation.commands`,
separate from `case.commands`/`batches`. Failed/incomplete audit data is retained
before the case fails. Only exit 0, `ok: true`, the full contract checks, and
matching identities/bytes qualify the case. There is no doctor fallback.
Audit timing never enters an equal-work CLI comparison; whole-case/run wall
times still include validation and are not CLI operation timings.

Root's required runtime fixture cases, on disposable retained fixtures only:

- Run the three 32-file matrix commands above: baseline independent copies,
  candidate independent copies, candidate shared worktrees; all three payload
  shapes and one/two agents. Verify three/five natural Operation histories,
  exact dirty/clean diffs, retained private bytes and one audit per complete Store.
- Once those pass, stage 1,000, 10,000 and 99,000 files with the same matrix and
  auditor hash in a quiet slot. Require actual row counts within 1,000,000 and
  clean, complete audits. Larger counts have not been qualified by static checks.
- Check the example's four original unit cases (table union drift/default unchanged,
  URI quoting, bounded row count, bounded checkout arguments) plus the physical
  accounting cases. These create fresh retained directories to verify category
  totals, extra unindexed objects, byte mismatches, staged files/empty directories,
  nonempty leases, and Unix symlinks or Windows junctions; Unix also checks a
  special socket entry. They exercise the actual enumerator without a real Store.
  Additional fixtures verify displaced backup payloads are excluded from metadata
  and included once in totals, samples stop at eight, and a similar filename
  prefix is not mistaken for the backup subtree. Small parameterized tests run
  the production accounting traversal at exactly 14 entries/depth 5, above those
  limits, and one below each limit. The public CLI retains fixed 2M/16 defaults;
  no large synthetic traversal is needed for boundary checks.
  Exercise disposable Stores just within/above
  the row ceiling and above the 256 history-row ceiling; overflow must fail with
  incomplete preflight, retained evidence and nonzero exit. No ceiling tuning.
- Exercise unsupported FORMAT, missing/corrupt metadata, nonempty WAL, path
  links and truncated/unclean results. Hash Store files and list sidecars before
  and after an isolated audit to verify read-only behavior. Use both cohorts'
  format-2 Stores; never modify an existing user Store to manufacture failures.
- Check missing auditor, hash mismatch, timeout and malformed/mismatched audit
  output. Each must retain a failed runner report; no successful case may have
  skipped audit, incomplete preflight or nonzero truncation.

Compilation, these real Store cases and independent review belong to root;
the implementation agent has not executed Cargo, a real CLI or a load benchmark.

## Windows process counters and overhead

On Windows, each command uses a hidden `powershell.exe -NoProfile -NonInteractive`
helper. Node sends a JSON request containing only the binary, working directory,
argument array, output paths/caps and deadline. After interop loading, the helper
announces readiness and requires an explicit start acknowledgment before it can
launch a native child. The helper uses
`System.Diagnostics.Process` with `UseShellExecute=false` and `CreateNoWindow=true`,
and retains the actual child's handle through exit. It does not look up or kill
processes by name or reopen a PID. Arguments follow the
[Microsoft CRT quote/backslash rules](https://learn.microsoft.com/en-us/cpp/c-language/parsing-c-command-line-arguments?view=vs-2019),
including empty strings and trailing backslashes; no argument is shell code.

Each command's `resourceMetrics` contains:

- `cpu`: user/kernel/total milliseconds from the exited child's
  [GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes).
  This is accumulated CPU execution time across its threads, never inferred
  from wall duration. A real OS zero is possible for very short commands.
- `memory`: OS peak working set and peak private committed bytes, from
  `PeakWorkingSetSize` and `PeakPagefileUsage` in
  [PROCESS_MEMORY_COUNTERS_EX](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex).
  These are resident working set and private commitment respectively, not two
  names for RSS. Reads occur approximately every 10 ms and once after exit.
  `complete: true` means the final handle read succeeded; otherwise the retained
  maximum is labeled `last-observed-lifetime-peak`, a potentially incomplete
  lower bound. No successful read means `null`, with the Win32 error and sample
  count retained. An unavailable final read is explicitly recorded.
- `io`: read/write/other operation counts and transferred bytes from
  [GetProcessIoCounters](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getprocessiocounters)
  on the held handle after exit. These include process I/O such as files and
  output pipes; they do not establish physical disk traffic, allocated bytes,
  filesystem cache misses, or network traffic. No derived network counter exists.

Failed API reads produce `null` values with a concrete `unavailableReason`.
Non-Windows platforms use direct CLI launch and explicitly report the absence
of a counter adapter. All counters cover **only the CLI process**, excluding
Node, PowerShell, descendants, other agents, and system cache. A helper failure
fails the command; it does not silently switch launch methods or invent metrics.

`elapsedWallMs` remains Node-observed command wall duration, including the
launcher and collection. `nativeElapsedWallMs` is OS-reported native lifetime
(exit timestamp minus creation timestamp), including CLI startup but excluding
the wrapper. `launcherOverheadMs` is their difference, covering PowerShell/
interop startup, scheduling, pipe drain, accounting and summary transmission.
It is `null` if native timestamps are unavailable. Raw launcher summary and
stderr have separate log paths. Resource counters belong to the native PID.

The helper's startup and sampling can perturb short or concurrent calls.
Subtracting launcher overhead does not remove this observer effect. Compare
the same instrumentation across models and retain all native/launcher timings;
do not treat these measurements as an uninstrumented performance result.

## L0 operation coverage

The JSON `operationCoverage` records what this bounded runner covers:

- Creation: initialization and, for two agents, exact-Step worktree creation
  or payload copy plus independent initialization. Copy time is separate and
  is not instrumented as native CLI CPU/I/O.
- Capture, comparison and history: `step`, clean/dirty `diff --stat`, and
  `timeline --limit 10`, with first/repeat phases where applicable. History is
  excluded from equal-work timing comparison. Integrity uses the separate current
  read-only auditor and is mandatory validation, not a timed CLI comparison.
- First usable file: unmeasured. Completion of materialization or subsequent
  payload verification is not an instrumented first-file timestamp.
- Lock wait: unavailable; no CLI lock-wait counter is exposed. External CAS
  retry counts and total process duration do not establish lock-wait time.
- Reception, publication and network bytes: require real server fixtures and
  remain outside this local runner. No fake offline measurement substitutes
  for these operations.
- Storage: logical retained Store and private payload bytes; Windows process
  I/O is additional evidence, not physical allocation or disk utilization.

## Cache policy, reports, and limits

Cache policy is always **unpurged**. Every command starts a new CLI process,
but fixture generation, copying, initialization, hash checks and prior commands
can already warm the OS cache. Labels are `first-invocation` and `repeat` within
each measurement phase. Repeated captures also grow retained history and use new
edit bytes. Neither label establishes physically cold or isolated warm caches.
There is no cache-purge command or claim. A future physical-cold protocol needs
an actual coordinated purge immediately before its measurement, with evidence.

The fresh run directory contains `report.json`, `logs/`, and all case roots.
The JSON report is checkpointed after commands/batches and case completion via
serialized temporary writes and atomic replacement of **only this run's own
report**. Failed runs retain their report, logs and fixtures and exit nonzero;
the matrix stops at the first unrecovered failure. Missing/unusable binaries
also leave a failure report. Invalid CLI syntax or an inaccessible output parent
can fail before report allocation; this is printed to stderr with nonzero exit.
An abrupt OS/process termination can leave the last checkpoint marked running
and an unfinished temporary report; no completion is inferred from it.

SIGINT/SIGTERM close each Windows helper's stdin; parent termination also closes
the pipe. The helper checks that lifetime signal and its deadline, then kills
only the child for which it holds the process handle. It waits up to five
seconds for exit and bounds post-exit pipe drain to two seconds plus one second
after closing streams. This is cooperative launcher supervision, not a Job
Object/process-tree sandbox: forcibly killing the helper itself can orphan a
child, and spawned descendants are not terminated or measured. The runner
therefore does not kill the wrapper after granting native launch. Before that
grant, Node can safely kill its owned wrapper if startup times out, because the
protocol guarantees that no CLI child exists yet.
Non-Windows cancellation directly kills Node's owned CLI child. Timeout budgets
cannot interrupt a stuck OS call. Filesystem copies
finish their current call before the next budget check. Native output is capped
at 16 MiB stdout and 1 MiB stderr per command; helper summary/stderr are capped
at 64 KiB each. Exceeding a cap fails the command. Raw native bytes are drained
concurrently to exclusive new files. No
existing report or checkout is accepted as a target, and nothing is deleted.

Native VFS remains **unqualified and unmeasured** until root integration connects
and validates the actual native provider. This runner rejects a VFS mode; an
ordinary directory or mock must not be labeled a VFS result. Likewise it does
not complete all L0/L6 coverage: first-file latency, network transfer,
publication, lock-wait instrumentation, non-Windows counters, and physical cold
caches need separate evidence. Large assets, storage allocation and benchmark
dispersion are also outside this bounded utility.

Measurable follow-ups for the remaining L0 gaps, proposed for separate approval:

- Reception/publication/network: use actual server fixtures with identical
  exact Step/Layer selections, data and authorization context; record native
  operation begin/end, accepted target IDs and payload verification. Attach
  per-request client/server transport byte counts and retries for those requests,
  distinguishing wire bytes, payload bytes and cache reuse. Local process I/O
  and offline mocks cannot supply this evidence.
- First usable file: start an observer at the materialization launch barrier,
  repeatedly open/read/hash a named expected file and record the interval between
  the last failed and first verified successful read, with polling frequency and
  observer overhead. CLI completion is a separate endpoint. Native VFS needs the
  qualified provider and a real first-read/hydration path, not directory simulation.
- Lock wait: instrument Store-lock acquire start/success/failure using a
  monotonic clock and command/attempt identity, or an equivalent existing native
  trace. Record contended wait separately from CPU, CAS retries and total duration.
  Adding instrumentation outside the owned scripts needs its own approved scope.
- Dispersion: root schedules at least five fresh retained trials per selected
  comparable cell, alternates baseline/candidate order with a recorded seed, and
  verifies matching profiles, payloads, cache policy and quiet conditions. Report
  all observations, sample count, median, quartiles and range; do not claim stable
  tail latency or a speedup from one run. Preflight disk/runtime cost before the
  large matrix; no automatic fixture deletion is authorized.

## Implementation validation

Root's R5 validation, before this retry-test follow-up, is recorded in
[the 118-pass Node log](../../.layrs-local/validation/benchmark-node-r5.log),
[the 12-pass Windows Rust log](../../.layrs-local/validation/benchmark-auditor-r5.log),
and [the successful release build log](../../.layrs-local/validation/benchmark-auditor-release-r5.log).
The release log reports 29.84 seconds of build time; this is not a CLI or audit
measurement. The resulting auditor is the retained `af2bf420…` executable above.
R5 independent review accepted the physical-accounting fixes. Root owns review
of the new pure retry tests and production contention validation.

Root's subsequent [full R6 Node suite](../../.layrs-local/validation/benchmark-node-r6.log)
passed **143/143 tests**, with zero failures, cancellations or skips, in
**12.46 seconds**. This includes the 25 retry-policy tests and the Windows
owned-process tests; this implementation agent verified the retained log without
rerunning the suite. Existing failed pilot reports remain unchanged, and the
runner's retry policy has not broadened. At that checkpoint root separately
reported the acquisition correction frozen with four helper tests passing,
while the two-process native test, new candidate and shared pilot were pending.
The subsequent R6 candidate and failed shared pilot are recorded above, followed
by the R7 rebuilt candidate/auditor and fresh strict passing shared cell. The R7
section records the subsequent validation and review disposition; wider cohort
qualification remains pending.

Repeatable repository checks:

```powershell
node --check scripts/measure-worktrees.mjs
node --test scripts/measure-worktrees.test.mjs
```

When new fixtures and process launches are prohibited, run only the pure retry
policy tests, or the broader pure/static contracts, using explicit selection:

```powershell
node --test --test-name-pattern '^capture retry policy' scripts/measure-worktrees.test.mjs
node --test --test-name-pattern '^(capture retry policy|payloads |diff |history |three/five-Operation |audit |runner, example |repair-backup )' scripts/measure-worktrees.test.mjs
```

The retry predicate is extracted unchanged from the failed-command branch.
Repository tests cover both CAS/stale-capture messages as strings and JSON
errors, non-retryable errors including `LockBusy`, every existing process/error
guard, disabled retries, stop requests, and exact exhaustion for limits 0–5.
They create no fixtures and launch no CLI/helper. The full command above also
runs the existing disposable-process tests and therefore needs authorization
when a quiet slot or no-fixture restriction is in effect.

This R5 follow-up ran both selected commands: **25 retry-policy tests passed**
and **128 pure/static tests passed** (including those 25), with zero failures
or skips among selected tests. Node syntax checks passed for runner and tests.
Fixture/process tests were excluded by the name filters, not rerun or counted
as passing. No new fixtures, helper/native launches, Cargo, benchmarks or child
agents were used. Only the unchanged policy extraction, tests and this protocol
changed; the auditor binary/schema and process helper were not modified.

The Node suite imports exported pure functions behind the runner's guarded
`main`; it retains diff/history/audit failure cases and tests deterministic
payloads, hash/input rejection reports, and actual Windows helper behavior using
a disposable Node fake CLI. Windows tests cover argument quoting, measured
CPU/memory/I/O or concrete unavailable reasons, bounded stdout/stderr, deadline,
parent EOF, launch acknowledgment, nonzero exit and missing executable. They
are explicitly skipped with a reason on other operating systems. No test
launches Layrs or a real Store audit; all created fixtures remain in fresh temp
directories printed by the test runner. Old OS-temp scripts are not required.

`crates/layrs-store-local/tests/measure_worktree_audit.rs` includes the actual
example as a module. Plain `cargo test -p layrs-store-local` therefore discovers
its critical drift/default, URI, row-bound, argument and physical-accounting tests and type-checks
the example's audit implementation. Root may use the targeted command above
for serialized validation. These Rust checks have not been run by this agent.

Review follow-up validation on 2026-10-24: the repository Node command above
passed **89 tests, zero failures/skips**, including the Windows fake-process
cases. Node syntax and `rustfmt --edition 2024 --check` for the example and its
integration-test entry point also passed. Fresh evidence directories printed by
the suite were `layrs-worktrees-test-failures-HvtXq2` and
`layrs-worktrees-test-process-Q9O12d` under OS temp. No OS-temp source script is
needed to repeat the checks. Cargo, real Store/checkout-binding execution and
independent review of this follow-up remain with root. The static estimate of
roughly 693k work rows at 99k files is not a runtime audit-completeness result.

Accounting follow-up validation on 2026-10-24: the same Node suite passed
**108 tests, zero failures/skips** after adding physical-accounting contract
rejections and the mandatory audit pin. Fresh evidence is retained in OS temp
`layrs-worktrees-test-failures-00KbrO` and
`layrs-worktrees-test-process-BwUTU4`. Syntax and Rust formatting checks passed.
The new actual-filesystem accounting tests are authored in the included Rust
example and await root's Cargo run: nine tests on Windows, ten on Unix including
the platform-specific special-file test. They retain all fresh fixtures and
never repair/delete evidence. No real Store audit or benchmark was executed.

R4 classification follow-up: root reported the preceding 108 Node/9 Windows
Rust checks passed and closed the CAS/staging finding. After separating repair
backups, this agent ran the repository Node suite: **118 passed, zero failures
or skips**. Evidence remains in OS temp `layrs-worktrees-test-failures-zVyrPS`
and `layrs-worktrees-test-process-euiyZ7`. Syntax and Rust formatting checks also
passed. Three additional Rust tests cover backup classification, bounded samples
and parameterized traversal boundaries; the new total is 12 tests on Windows
(13 on Unix). At that freeze they awaited root's serialized Cargo run,
release-auditor rebuild, new pinned SHA and real smoke; the subsequent R5
results are linked above. Earlier failed artifacts and provenance remain
intact; no Cargo, real Store audit, benchmark or child agent ran in this follow-up.

Author validation on 2026-10-24: `node --check scripts/measure-worktrees.mjs`
and `--help` passed. An intentionally missing binary produced exit 1 and a
retained JSON report with `status: "failed"`, `success: false`, and zero cases;
no Layrs process or payload fixture was created. No Cargo or actual benchmark
was run during implementation.

Follow-up validation, also 2026-10-24:

- Node syntax/help and PowerShell parser checks passed.
- A disposable Node fake CLI verified exact empty/space/quote/backslash/newline/
  Unicode argument round trips, positive measured CPU/memory/I/O after bounded
  CPU and file activity, stdout/stderr caps, timeout, parent-pipe cancellation,
  nonzero native exit, and missing-executable reporting.
- The then-current runner passed 51 instrumented calls against a disposable fake
  CLI in **independent-copies mode only**, with three mixed files and one/two
  processes. This did not exercise shared worktree creation or history.
  Assertions covered two
  code files plus one asset, 5,120 bytes per checkout, both edited paths,
  per-command native counters and separate launcher overhead. That fixture's
  placeholder diff/history responses are rejected by the stricter review fixes;
  those 51 calls do not validate the revised contracts.
- A disposable helper copy with five seconds of injected startup delay hit
  the runner's one-second budget in about 1.14 seconds, retained a failed
  report, and launched no native CLI. This validates the readiness protocol,
  not a performance result.

Artifacts remain under the OS temporary directory: `layrs-process-smoke-v2-efda6127-afb5-4292-a8a1-a6073fa6fc46`
(helper assertions and summaries), `layrs-mixed-fake-3afc2d26-334a-47d6-b02e-62e714301f65/layrs-worktrees-poNtIa/report.json`
(final full-runner smoke), and `layrs-launch-timeout-e9158c97-c70a-4eaa-8a6a-50e3ec6e75ef/layrs-worktrees-AFgdSc/report.json`
(startup failure). Fake `doctor` and history responses prove runner handling
only; they are not evidence of native Layrs integrity, compatibility, or speed.
Neither the baseline nor candidate Layrs binary was executed in this follow-up.

Independent-review fixes, 2026-10-24:

- `node --check scripts/measure-worktrees.mjs` passed.
- 28 pure contract checks passed against the runner's actual validator functions:
  valid clean/exact dirty changes; rejected fixture-only/missing/extra/wrong-byte
  diffs; valid three/five-Operation schemas; rejected wrong counts, fields,
  parents, kinds, Spaces, capture IDs and root IDs. The five-Operation check is
  schema-only and **does not exercise shared CLI execution**. Evidence is under
  OS temp `layrs-benchmark-contracts-ca11b30b-26e3-4382-8135-17f4cde39075`.
- Before audit wiring was authorized, the 10k preflight produced a retained failure report and exit 1, with a
  70,000-row lower bound and zero generated cases/CLI processes. Evidence is
  `layrs-audit-gate-159fd676-b747-432d-880b-059ca2dd3ce4/layrs-worktrees-bjzSYZ/report.json`
  under OS temp. The default matrix was not clamped or partially run.
- P2 diff/history fixes are implemented. Root subsequently authorized the P1
  standalone example and explicit wiring, now implemented with fixed budgets
  and mandatory clean/complete results. Compilation, real fixture validation
  and independent review remain outstanding. No production default or schema
  migration changed; the old default-doctor lower-bound gate is superseded by
  the actual Store preflight and complete auditor.

Approved audit implementation checks, 2026-10-24:

- Node syntax/help, PowerShell helper parsing and standalone `rustfmt --check`
  passed. Formatting parses the example but does not type-check or compile it.
- 38 pure/static checks passed for audit expectations and validation, including
  agreement of the Rust/JS/current-fsck work-table sets, independent/shared
  history unions and mixed retained-byte counts. Rejection cases include
  incomplete/truncated/unclean results, wrong identities/parents/cardinalities,
  missing counters, and row/byte overflows. Evidence: OS temp
  `layrs-audit-contracts-ogVAOC/results.json`. These are synthetic contract
  checks, not shared CLI execution or a real Store audit.
- The existing 28 diff/history contract checks passed again against the current
  runner; fresh evidence: `layrs-diff-history-recheck-RW3hTf/results.json`.
- Three actual runner invocations rejected a missing auditor option, absent
  auditor file and mismatched pinned auditor hash, each with exit 1, a retained
  failed report and zero cases/fixture CLI launches. Node was supplied only as
  a file/hash input, never launched as a fixture CLI. Evidence:
  `layrs-audit-failures-4ITOQj/results.json` and its retained run directories.
- The process helper was unchanged in this audit follow-up. No Cargo command,
  real Layrs invocation, native audit, load benchmark or child agent ran.

Root owns independent review and real CLI smoke/benchmark execution after CLI
tests pass and a quiet slot is available. A successful syntax check validates
neither CLI compatibility nor performance. Compare multiple retained reports
with identical payload/configuration and verified binary profiles before making
claims.
