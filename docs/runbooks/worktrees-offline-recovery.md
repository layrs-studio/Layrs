# Worktrees, offline availability and recovery

Source-checked runbook for the `feature/worktrees-partial-vfs` programme,
24 October 2026. Commands below describe the current parser and implementation;
they are not a report of successful execution. Root must verify these recipes
against fresh CLI and native test results before declaring them qualified.

Read this alongside the [programme and qualification journal](../plans/worktrees-partial-vfs.md),
[shared partial Store contract](../plans/worktrees-partial-vfs-storage.md),
[Windows provider contract](../plans/worktrees-partial-vfs-provider.md), and
[partial/VFS convergence design](../plans/worktrees-partial-vfs-convergence.md).

| Surface | Current boundary |
| --- | --- |
| Ordinary native Worktrees | Shared immutable Store, separate checkout roots and private active Operations/Views. Recovery commands exist; use current root validation, not historical suite results. |
| Partial selection and format-4 offline storage | Implemented commands described below. Format-3 conversion is explicit and source-preserving. Recent recovery/retention corrections still require the root's corresponding fresh evidence. |
| Windows CFAPI provider | R9: **26 unit tests**, retained export-parent guards and disconnected visibility passed. The twelve-case renamed-export matrix and control-publication regressions passed on the unchanged R8 visibility/export paths. Dirty/nonresident recovery remains unqualified (0/2 seeded arms). |
| Complete-Store native VFS | R9: **13 integrated native recipes**, two retained-parent unit recipes and both CLI lifecycle/resume recipes passed. Evidence is recorded in `l4-client-r9-results.json`, `l4-client-r9-parent-guard.log`, `l8-cli-vfs-lifecycle-r9.log` and `l8-cli-vfs-resume-r9.log`. These bounded results do not qualify partial VFS, power loss or general rename capture. |
| Partial VFS / L5 | **Unimplemented / pending.** The convergence document is a design. There is no supported partial-family VFS launch or automatic partial/VFS conversion command. |

## Example inputs and credential entry

Examples use PowerShell 7 syntax. Paths, server address, names and IDs are examples,
not production targets or secrets. Replace every `<...>` value with an exact ID
from the relevant successful command. Do not invent a digest or reuse a receipt
from another Worktree. Destination directories must already exist, be empty where
specified, and be separate, nonnested roots. Use fresh receipt/output filenames
outside the checkouts. Check each command's exit status and JSON `ok` before using
its `data`; stop the recipe on a refusal.

```powershell
$Source = 'C:\Layrs\project'
$A = 'C:\Layrs\project-a'
$B = 'C:\Layrs\project-b'
$Endpoint = 'https://layrs.example.invalid'
$BaseStep = 'blake3:<64-hex exact admitted Step ID>'
$SpaceId = 'space:<actual Space ID>'
```

Online commands use the existing device-login flow:

```powershell
layrs remote login --endpoint "$Endpoint"
layrs remote whoami --endpoint "$Endpoint"
```

Open the verification URL and approve the displayed short-lived code. The CLI
stores the device credential and bearer token in the OS credential store; it does
not require a token argument or print the bearer token. For a deployment using a
private CA, add `--ca 'C:\Layrs\trust\server-ca.pem'` consistently to its remote
commands. Keep OS credentials separate from diagnostic bundles and backups.
See [credential handling](../../crates/layrs-client-core/src/native_scm/credentials.rs)
and [CLI login](../../crates/layrs-cli/src/native_remote.rs).

## Two ordinary Worktrees at an exact base

Start from a complete native checkout containing the chosen Step. Inspect its
state and Layer names, then choose one immutable `$BaseStep` for both creations:

```powershell
layrs --space "$Source" --json status
layrs --space "$Source" layers
layrs --space "$Source" worktree create "$A" --layer main --step "$BaseStep"
layrs --space "$Source" worktree create "$B" --layer main --step "$BaseStep"
layrs --space "$Source" --json worktree list
layrs worktree open "$A"
layrs worktree open "$B"
```

`main` must already exist in the source View. An exact existing Layer ID is also
accepted; creation does not create a Layer implicitly. A reference such as
`@layer:main` resolves when each command executes, so use the recorded Step ID
when both Worktrees must have the same base. In each returned `state`, check the
entry in `view.worktrees` whose `worktreeId` matches the returned Worktree: its
`baseStepId` must equal `$BaseStep`. The Store IDs match and the Worktree IDs differ.
`worktree open` validates/describes a root; it does not switch the shell's cwd.
Continue with an explicit `--space`.

Create and select separate local Layers before editing:

```powershell
layrs --space "$A" layer create task-a
layrs --space "$A" layer use task-a
layrs --space "$B" layer create task-b
layrs --space "$B" layer use task-b
```

Edit A only in `$A` and B only in `$B`. Capture each independently:

```powershell
layrs --space "$A" --json step
layrs --space "$B" --json step
layrs --space "$A" --json status
layrs --space "$B" --json status
```

Record each returned `stepId`. Shared immutable objects do not make writable files,
active Operations, or private Layer-head choices follow the other Worktree.
Capturing B does not rewrite A's parent/base or working files. Even two Worktrees
on the same Layer retain private state; a Layer name is not an interprocess lock.

To integrate B explicitly into A, use B's exact captured Step. Its Layer name may
not exist in A's private View:

```powershell
$BStep = 'blake3:<exact Step ID returned by B step>'
layrs --space "$A" weave "$BStep" --target task-a --preview
layrs --space "$A" weave "$BStep" --target task-a
layrs --space "$A" weave status
layrs --space "$A" --json status
```

Inspect the preview and stop for conflicts or unexpected heads. Keep writers
stopped between preview and apply: the CLI apply replans and performs checked
acceptance; the preview output is not itself an authorization token. Test the
resulting combined Step, including unchanged dependencies. A local `step` or
`weave` does not publish remotely. Remote review/proof gates still apply to the
exact combined result; a parent or sibling Proof is not transferable. See the
[L7 review contract](../plans/worktrees-partial-vfs-reviews.md).

Worktree isolation covers files and native metadata, not application ports,
databases, processes, environment variables or external services. Configure
distinct service ports and external test resources explicitly.

## Partial selection and source-preserving shared-family conversion

Selection controls which authorized paths are materialized. It does not grant
access, infer that unselected paths are absent, or delete remote paths.

```powershell
$PartialSource = 'C:\Layrs\partial-original'
$Family = 'C:\Layrs\partial-family'
$Partial = 'C:\Layrs\partial-shared'
layrs remote clone-partial "$SpaceId" "$BaseStep" "$PartialSource" --include README.md --include-tree src --endpoint "$Endpoint"
layrs --space "$PartialSource" --json partial status
layrs --space "$PartialSource" partial offline family create "$Family" --quota-bytes 1073741824
layrs --space "$PartialSource" --json partial offline family convert --family "$Family" --destination "$Partial"
layrs --space "$Partial" partial fsck
layrs --space "$Partial" --json partial offline status
```

Omitting both selection flags on clone retains the existing full-authorized-clone
behavior. `--include` names exact leaves; `--include-tree` names subtrees. Conversion
currently accepts format-3 sources, creates a fresh destination Worktree identity
in format 4, and retains the source files and history. The family shares verified
immutable content and quota/retention bookkeeping; snapshots, captures, selections,
publication state and filesystem journals remain private per Worktree. This family
is not a complete native SCM Store with missing objects.

Conversion refuses pending publication/local acceptance. Imported captures and
receipts remain provenance under their original identity: make a fresh destination
capture before publishing, even when the bytes match. Do not copy pending requests
or edit marker identities. Keep the source until the destination and its recovery
have been verified. A complete-to-partial remote migration is a different command,
not a substitute for this format-3-to-format-4 history conversion.

Read `projection.generation` from successful `partial status`. Use that integer as
`$Generation`, and refresh it after mutations. Expansion is explicit and additive:

```powershell
layrs --space "$Partial" --json partial status
layrs --space "$Partial" remote partial-select --expected-generation "$Generation" --include docs/help.md --include-tree tests --endpoint "$Endpoint"
```

`partial-hydrate` is an alias. Existing selection and local edits are retained;
this is not a selection-withdrawal or eviction command.

## Exact-version offline checks, reads and preparation

All `partial offline` commands operate locally. A successful availability query
is a statement about the requested operation, exact versions and paths; it does
not mean that the whole repository or all history is available offline.

```powershell
layrs --space "$Partial" --json partial offline status
layrs --space "$Partial" partial offline check read README.md --version current
layrs --space "$Partial" partial offline check edit src/main.rs --version current
layrs --space "$Partial" partial offline read README.md --version current --output 'C:\Layrs\receipts\readme-exact.bin' --max-bytes 1048576
layrs --space "$Partial" partial history
```

`current` means the recorded current projection generation, not arbitrary unsaved
editor bytes. `generation:N` and `capture:N` resolve identities/digests from stored
history; capture sequences start at 1. For example, use the actual retained
generation and capture sequence in these selectors:

```powershell
$BeforeVersion = 'generation:<retained generation number>'
$AfterVersion = 'capture:<accepted capture sequence>'
layrs --space "$Partial" partial offline check compare-content README.md --version "$BeforeVersion" --other "$AfterVersion"
layrs --space "$Partial" partial offline compare "$BeforeVersion" "$AfterVersion" README.md --content
```

Without `--content`, comparison reports file-identity/mode differences from
metadata; it is not a text diff or proof that both byte streams are retained.
`check` accepts `read`, `edit`, `capture`, `restore`, `compare-metadata` and
`compare-content`; only comparison accepts and requires `--other`. Paths are
repository-relative, unique and bounded to 1..1024. `read` requires a new output
file, never stdout or overwrite. Its default limit is 65,536 bytes and maximum
1,048,576; exceeding the limit is a refusal, not truncated success. An output I/O
failure can leave a partial file, identified by the error, which is not a valid
read receipt.

Online observation can record an exact historical Step without changing checkout
selection. Save the successful JSON response as UTF-8 in a new file outside the
checkout, then use it directly as a version descriptor:

```powershell
$HistoricalStep = 'blake3:<exact historical Step ID>'
layrs --space "$Partial" --json remote partial-observe "$HistoricalStep" README.md --expected-generation "$Generation" --endpoint "$Endpoint" | Out-File -Encoding utf8 -NoClobber 'C:\Layrs\receipts\observed-version.json'
layrs --space "$Partial" remote partial-prepare read README.md --version 'version-file:C:\Layrs\receipts\observed-version.json' --expected-generation "$Generation" --endpoint "$Endpoint"
layrs --space "$Partial" partial offline preparations
layrs --space "$Partial" partial offline check read README.md --version 'version-file:C:\Layrs\receipts\observed-version.json'
```

Use PowerShell 7 UTF-8 output (no BOM) for machine-readable receipts; do not use
Windows PowerShell's default UTF-16 redirection. Version files are regular files
bounded to 64 KiB and accept an exact version or successful `{ok,data.version}`
receipt. They supply neither credentials nor remote authority.

Preparation requires a shared family, the current expected generation and fresh
authenticated remote authority. If permissions/epoch change, re-observe the exact
version rather than fabricating a descriptor. Existing local cache membership
does not authorize remote acquisition. Missing protected capture bytes are
corruption, not a reason to fetch unrelated/current remote content.

Preparation is operation-specific. **Capture/Restore preparation receipts are
currently unsupported**: those engines need whole-checkout preflight. Prepare
immutable restore target bytes with `read`; an availability check alone neither
reserves capacity nor applies a restore. Capture scans the selected working set,
including dependencies outside a narrow check request. Inspect `effectivePaths`
and blockers in JSON, then capture with the current generation:

```powershell
layrs --space "$Partial" --json partial offline check capture src/main.rs --version current
layrs --space "$Partial" partial capture "$Generation"
```

## Retention, quota and explicit cancellation

```powershell
layrs --space "$Partial" --json partial offline quota
layrs --space "$Partial" partial offline preparations
```

Quota accounts for shared content and outstanding reservations. A ready
preparation pins its exact dependencies; other Worktrees' claims, active bases,
captures, preimages and recovery operations also protect content. For a preparation
that is no longer needed, use its exact returned ID:

```powershell
$PreparationId = '<exact preparation ID from preparations>'
layrs --space "$Partial" partial offline release "$PreparationId"
layrs --space "$Partial" partial offline evict
layrs --space "$Partial" partial offline quota --bytes 2147483648
```

These are explicit state changes, not generic repair steps. `release` relinquishes
that preparation's retention; it does not delete checkout edits or accepted
history. `evict` reconciles registered owners and removes only unretained shared
objects. It may reclaim zero bytes. Ambiguous/unreadable owners prevent unsafe
reclamation; do not remove their metadata to force progress. Quota is a configured
storage budget, not a promise of available physical disk space.

`partial offline release-generation N` can release a superseded base generation
only when active bases, retained captures and preparations no longer depend on it.
This stage has no history-pruning command. If retained history fills the quota,
increase capacity or start a separately planned family while preserving the old
one; deleting CAS files is not an eviction strategy.

Choose recovery by the recorded operation, keeping writers stopped during it:

| Recorded condition | Exact CLI path | Preservation and refusal boundary |
| --- | --- | --- |
| Format-4 preparation is `acquiring`; acquisition should continue | `layrs --space "$Partial" remote partial-resume-preparation "$PreparationId" --expected-generation "$Generation" --endpoint "$Endpoint"` | Revalidates current credentials, authority and generation; does not silently select a different version. |
| Acquiring or ready preparation should be cancelled/released | `layrs --space "$Partial" partial offline release "$PreparationId"` | Local operation; reconciles only owned installation staging and releases this claim/reservation. Accepted content/history and checkout files survive. |
| An interrupted operation left an active budget, with no private journal/publication pending | `layrs --space "$Partial" partial offline cancel-reservation` | Cancels the reservation and only its durably recorded, identity-verified staging. Preserves checkout edits and accepted history. Unclassified staging refuses cleanup; then resume the recorded operation or retain it for diagnosis. |
| Installation, namespace, refresh, restore or rebase journal is pending | `partial resume-installation`, `partial resume-namespace`, `partial resume-refresh`, `partial resume-restore`, or `partial resume-rebase`, each with `--space "$Partial"` | Select the matching operation. Reservation cancellation refuses accepted application journals. Do not run all recovery commands blindly. |
| Publication has an unresolved outcome or pending local acceptance | Inspect `partial status`; retry the original publication action/capture sequence when connected | Preserve the durable request and receipt. Do not create a substitute request, convert or cancel around an unknown remote outcome. |

The Desktop Diagnostic uses nonmutating, identity-validated inspection. A hot
SQLite rollback journal is reported separately from a pending application
operation. Its explicit database recovery action repairs only that database
condition; any remaining installation, restore or publication still needs its
own action. An interrupted ordinary shared bootstrap can be resumed explicitly
from its recorded initial snapshot and selection. Unsupported legacy schemas
are refused without automatic migration by this diagnostic path.

Recovery requests carry the inspected checkout, database and marker bindings.
If the path now names another valid checkout, native recovery refuses before
changing it; inspect the intended checkout again before choosing an action.
The dedicated database rollback and ordinary-bootstrap actions currently exist
in Desktop only. CLI application-resume commands inspect read-only immediately
before their explicit action; they do not offer separate database/bootstrap
recovery commands. Ordinary Files browsing still uses the ordinary Store opener
and its supported migration behavior.

For an interrupted conversion, retain both roots and choose one of these paths:

```powershell
layrs --space "$PartialSource" --json partial offline family resume --destination "$Partial"
```

Resume checks the same source and retained destination and reuses the recorded
identity. If the source changed or disappeared, exact resume may refuse. To give
up only an **unready** conversion, explicitly name its family and destination:

```powershell
layrs partial offline family abandon --family "$Family" --destination "$Partial"
```

Abandonment does not require the old source to open. It releases validated
unready registration/claims but **preserves the physical source, destination files
and private metadata**. It refuses ready destinations or new accepted local
captures. An abandoned locator cannot resume or register itself again. Preserve
those retained artifacts for inspection; abandonment is not directory cleanup.

## Ordinary Worktree creation and materialization recovery

Stop writers and keep the original shared Store and checkout directories in place.
Creation can stop before registration, with only a staged `.layrs.reservation`, or
after publishing the `.layrs` locator. A list of registered Worktrees may therefore
omit a reserved root. Resume from the retained target directory, not the source:

```powershell
layrs --space "$A" --json worktree resume
layrs worktree open "$A"
layrs --space "$A" --json status
```

The implementation discovers either marker, validates Store/root/metadata
identities and the durable creation intent, and resumes the original exact Step
and Worktree ID. Do not rerun `create` with a newer Layer head or manually rename
the reservation. If a registered Worktree reports `NeedsRepair` from an interrupted
materialization, diagnose and complete that journal before retrying creation:

```powershell
layrs --space "$A" --json doctor
layrs --space "$A" recover
layrs --space "$A" worktree resume
```

Use this sequence only for that interrupted-creation state. `doctor` is read-only;
`recover` and `resume` can mutate recovery state. Identity mismatch, replaced roots,
foreign files or unrecoverable journals require inspection; do not delete controls
or overwrite files to make a check pass. Ordinary creation does not expose a
generic abandon/delete command.

## Complete-Store VFS host lifecycle — bounded R9 qualification

These commands require supported Windows/local NTFS and a complete native source
Store. They neither enable OS features nor install a system service. Do not point
them at a format-3/4 partial checkout. A ready host is not evidence that all L4/L5
capture and recovery guarantees have been qualified.

Named empty directories, executable directory entries and every Tombstone-bearing
target Tree, including root and mixed live/Tombstone Trees, are unsupported by this VFS catalog. Source
preparation refuses them before creating a journal or registering a provider.
A genuinely empty root and nonempty directory aliases
remain supported. Existing development mounts based on an unsupported snapshot
retain their checkout bytes and private journals, but normal reopen, start, serve
and VFS export refuse. Do not flatten the snapshot, retarget its source, remove
metadata or invoke an ordinary scanner/materializer to force recovery. No
migration for such development mounts has been implemented.

```powershell
$Virtual = 'C:\Layrs\project-virtual'
$Ticket = 'C:\Layrs\receipts\vfs-launch.json'
layrs --space "$Source" --json vfs create "$Virtual" --layer main --step "$BaseStep" --background --ready-timeout-seconds 60 | Out-File -Encoding utf8 -NoClobber "$Ticket"
layrs vfs status --ticket "$Ticket"
```

Background launch starts a hidden user process and waits for readiness, normally
30 seconds; the explicit range is 1..300 seconds. `--ready-timeout-seconds` on
`create`/`resume` requires `--background`. A failed/timed-out launch retains
recoverable work, so inspect it instead of retrying creation at a guessed root.

For an existing complete root, choose either foreground serve or background start:

```powershell
layrs --json vfs serve "$Virtual"
```

The foreground command emits one ready response and stays running; control it from
a second terminal. Save that successful JSON line unchanged as a new UTF-8 ticket
file and select it with `--ticket`; do not wait for the host to exit to obtain its
ticket. Alternatively, use the finite background-launch command:

```powershell
$Ticket = 'C:\Layrs\receipts\vfs-restarted.json'
layrs --json vfs start "$Virtual" --ready-timeout-seconds 60 | Out-File -Encoding utf8 -NoClobber "$Ticket"
```

Retain the **new** launch ticket. It contains Store/Worktree/endpoint identities,
not the private control token. The token stays in the owner-only Store recovery
directory. Control clients validate those bindings; a ticket is not a portable
replacement for that private state. The CLI accepts the complete successful JSON
launch response as `--ticket FILE`; do not print or copy the private endpoint file.

Interrupted creation, including before root registration or `.layrs` publication,
has a distinct resume command:

```powershell
$VfsWorktreeId = 'worktree:<exact retained Worktree ID>'
$Ticket = 'C:\Layrs\receipts\vfs-resumed.json'
layrs --space "$Source" --json vfs resume "$VfsWorktreeId" --background --ready-timeout-seconds 60 | Out-File -Encoding utf8 -NoClobber "$Ticket"
```

Use the ID from retained creation/diagnostic evidence, not a directory name or new
UUID. The command opens the original source Store's journal and recorded root/base;
it accepts no replacement `--step`, `--layer` or destination. If no ID was surfaced
before failure, preserve the Store recovery records for bounded inspection; there
is no CLI promise that `worktree list` enumerates preregistration VFS reservations.
Incomplete provider population can remain `NeedsRepair`; do not repopulate or
delete it automatically. Export is usable only if the provider/checkpoint can be
opened successfully, not a guaranteed bypass for every damaged root.

Before capture or export, stop **all** Worktree writers, including editors,
formatters, build/watch processes and agents, and keep them stopped until the
response. The flag acknowledges this condition; it does not suspend applications:

```powershell
layrs vfs capture --ticket "$Ticket" --writers-quiesced
layrs vfs export 'C:\Layrs\exports' --ticket "$Ticket" --writers-quiesced
layrs vfs status --ticket "$Ticket"
layrs vfs stop --ticket "$Ticket"
```

Use the ticket for the currently running host in these commands. Capture reports
the accepted Step/Operation; status includes `sourceStepId`, `acceptedStepId` and
`captureAckPending`. Reconnect resolves durable acceptance before acknowledgment;
do not assume a lost response means no Step was accepted. Export writes a fresh
container/tree and manifest under the existing parent and reports `treeRoot` and
`manifestPath`; it preserves the source and does not recreate a native Store or
all history. `stop` disconnects the host and retains the root/work. Restart with
`serve`/`start`, or use exact-ID `resume` for retained creation.

`captureBlockedReason` distinguishes an acknowledgment still pending from an
unproven historical activation or a Worktree advanced outside provider-owned
acceptance. An external advance is never adopted by a later capture or restart.
Keep the accepted history and export unaccepted work; a legacy root without a
proven activation or accepted provider receipt requires export and recreation.
Status and export remain available for these lineage refusals. An interrupted
registration without its ticket, incomplete population or a missing registered
root can still conservatively block other creation in that Store until repaired;
identity-only overlap validation does not waive those refusals.

Renaming a placeholder after stopping its provider does not retarget its immutable
catalog identity. Normal capture continues to refuse that rename. Explicit export
uses the current exact name and hashes bytes from the retained actual file,
including private edits. The R6 native matrix and integrated recipe cover clean,
resident and dirty renamed files, case-only changes, and missing-source refusal
followed by verified source restoration. Export never fills missing bytes with
zeros or substitutes the immutable source for private work. If hydration fails,
retain the root, ticket, journal and incomplete output; the diagnostic includes
the actual path and native failure. Retry only after the verified backing content
is available. Partial files whose size differs from their backing remain refused.
The streaming export has explicit logical limits of 1 GiB per file, 4 GiB total,
100,000 checkpoint paths and the existing snapshot depth bound. It preflights
these limits before creating output; it does not retain whole-file payloads in
memory. Capture keeps its separate 64 MiB per-file and 256 MiB dirty-byte limits.
Export destinations must have ordinary directories throughout their ancestor chain.
OneDrive and other cloud/reparse destinations are refused. A process already holding
DELETE access on an ancestor can cause a sharing-violation refusal (Windows error
32). Export retains guards that prevent directory replacement, compares their
native identities with the ordinary writer pins, and revalidates before publication.
These guards do not make child-name or attribute changes atomic; writers must stay
quiesced. Native destination tests exercised Unicode spelling and an available 8.3
alias with the same directory identity; this does not broaden the provider's user
file namespace contract.
The R7 native streaming recipe passed with a 70 MiB + 17 byte file in both
hydrated-clean and relocated states, checking exported bytes and unchanged history.
Mixed dirty/nonresident recovery is not yet qualified: the first native fixture
could not establish that state because Windows refused its preparatory write.
A later seeded fixture established partial residency but exposed a metadata
classification disagreement after disconnect. Native probing traced it to Windows
placeholder compatibility masking. The scoped exposure correction now passes its
26 unit tests and the native disconnected forced-DISGUISE regression, preserving
the caller's mode and performing no metadata hydration. The unchanged seeded
fixture now agrees with native identity and residency, but both predetermined
source-window arms still fail at the private write with Windows error 389 while
requesting an unavailable range. Its result is **0/2 mixed-state arms qualified**;
the harness's successful exit is not a dirty/nonresident recovery pass. Raw logs
are `l4-provider-r8-lib.log`, `l4-provider-r8-visibility.log`, and
`l4-provider-r8-seeded-dirty-partial.log` under `.layrs-local/validation`. Subsequent
R9 retained-parent and visibility tests passed, as did all thirteen integrated
native recipes listed in `l4-client-r9-results.json`, including large streaming
exports and a genuinely available 8.3 destination alias. These results do not
change the mixed-state qualification count.

Ordinary `step`/`status`, Layer switching and restore are refused inside VFS roots;
the supported control commands above do not provide rename/restore/Layer-switch
operations. Keep ordinary Worktrees available for those workflows.

## Coherent backup and restore boundaries

There is no verified one-command local backup/relocation recipe for this combined
topology. Before copying anything, inventory every root and its Store/family,
Worktree ID, exact accepted Step/version, pending operation and private database.
Use ordinary `worktree list/open`, partial `status/offline quota`, and VFS ticket
and `status` output. Failed opens and unregistered reservations belong in the
inventory too; do not infer completeness from the registered list alone.

1. Stop all checkout writers and all clients that can capture, publish, receive,
   prepare, evict, compact or convert. While a healthy VFS host is still running,
   capture or export needed dirty work with writers quiesced; retain the receipts.
2. Stop every VFS host with its current ticket and confirm it is disconnected.
   Keep all writers/hosts stopped for the whole backup interval. An unavailable
   host, pending operation or uncertain owner must be recorded, not treated as a
   successful shutdown or discarded to simplify the backup.
3. Preserve the complete coherent set below with a qualified backup method. Do
   not describe sequential copies of live SQLite files, roots and CAS as atomic.
   A snapshot of one volume does not coordinate additional volumes automatically.
4. Record the software revision, format versions, paths/identities and diagnostics
   alongside the backup. Verify the resulting artifact and a restore procedure
   independently before relying on it. Leave the original set intact meanwhile.

| Topology | Files/state that must remain together |
| --- | --- |
| Ordinary complete Store with linked Worktrees | Entire source Store (`.layrs`, including `FORMAT`, objects, metadata/`layrs.sqlite3`, journals, staging and recovery/maintenance backups), every primary/secondary checkout's writable files, and each `.layrs` locator or `.layrs.reservation`. Private Worktree state resides in the shared metadata; a secondary root alone is not a backup. |
| Format-3 partial source | Entire checkout and its `.layrs` metadata/content/journals, including `projection.sqlite3`. Keep preserved conversion sources as independent recovery assets. |
| Format-4 family | Entire family `FORMAT`, objects/chunks/manifests, staging, metadata/`catalog.sqlite3`, and **all** per-Worktree private databases under `worktrees/<internal-key>/metadata/projection.sqlite3`; every checkout root and its `.layrs/FORMAT`, private journals/claims, retained conversion data and dirty files. Do not reconstruct the internal key from a guessed ID. |
| Complete-Store VFS | Complete Store, private recovery journals and provider/control identity records, every virtual root including dirty native files and provider metadata, plus any successful export/manifest. A generic file copy of offline placeholders is not evidence that their bytes were backed up. |

Keep SQLite sidecars and pending-journal artifacts with their owning stopped
database; do not remove them by hand. The Store CAS alone omits dirty files and
private intent. A checkout copy alone omits immutable dependencies and history.
An export is useful portable file evidence, not a replacement for the coherent set.

Restoring bytes under another path or newly created filesystem objects can change
native directory/file/database identities even when paths and hashes look equal.
Locators bind those identities, Store incarnation and Worktree membership; CFAPI
registration/control files add provider bindings. **Do not blindly copy locators,
edit `FORMAT`, transplant databases or reuse host tickets to bypass a refusal.**
No relocation/rebind CLI is promised here. Prefer a qualified identity-preserving
restore, or an explicitly reviewed import/recreation into fresh identities while
retaining the old set. Check ordinary `doctor`, partial `fsck`, and the appropriate
VFS reconnect/status before allowing writes. A refused validation is evidence to
investigate, not authorization to weaken identity checks.

## Diagnostics and remaining limits

Use the command for the root's actual type:

```powershell
layrs --space "$A" --json doctor
layrs --space "$A" --json operation-history --limit 20
layrs --space "$Partial" --json partial status
layrs --space "$Partial" partial fsck
layrs --space "$Partial" --json partial offline preparations
layrs --space "$Partial" --json partial offline quota
layrs vfs status --ticket "$Ticket"
```

An ordinary `doctor` report must be both `clean` and `complete`; a truncated scan
is not full verification. Partial status can itself refuse when recovery is
needed; use the recorded operation's exact resume path. Availability reports name
blockers such as `missingContent`, `protectedPath`, `excludedPath`, `notMaterialized`,
`pendingOperation`, `quota` and `changedCheckout`. Preserve the exact command,
exit status, IDs, generation, error and matching nonsecret receipt. Redact local
paths/account details when sharing diagnostics and never attach credential-store
contents or private endpoint tokens.

Limits remain explicit:

- Offline operation requires locally verified bytes for the selected exact
  versions. It does not make remote collaboration, hosted CI or cloud AI available
  without a network. A local model/tool has its own installation and dependency
  requirements.
- VFS is not an application or same-user security sandbox. Service ports and
  external state remain shared unless the operator isolates them separately.
- The CFAPI prototype supports the documented bounded ASCII Windows-safe regular
  file namespace on supported local NTFS. Symlinks, hardlinks, unqualified Unicode/
  case semantics and other volumes/platforms are not general support claims.
- CFAPI uses PROGRESSIVE hydration. Historical native measurements requested
  8,388,682 source bytes for 142 application bytes; small reads can hydrate whole
  files. Do not claim page-level residency savings or automatic dehydration.
- mmap, watchers, simultaneous writer stress, disk-full and driver cancellation
  races need their own qualification. A namespace recheck and open-handle guards
  are not an atomic filesystem snapshot; writers must remain quiesced.
- Process-interruption tests do not prove Windows power-loss durability. Backup
  restore, multi-volume behavior and full native VFS remain unqualified where
  fresh root logs are absent. No automatic cleanup is a substitute for that proof.

The command contracts were checked against [help](../../crates/layrs-cli/src/args/help.rs),
[Worktree parser/adapter](../../crates/layrs-cli/src/native_worktree.rs),
[offline parser](../../crates/layrs-cli/src/args/offline.rs),
[offline adapter](../../crates/layrs-cli/src/native_offline.rs), and
[VFS parser](../../crates/layrs-cli/src/args/vfs.rs)/[Windows host adapter](../../crates/layrs-cli/src/native_vfs_windows.rs).
Recovery evidence should reference fresh results for the existing
[Worktree CLI tests](../../crates/layrs-cli/tests/native_worktrees.rs),
[offline CLI tests](../../crates/layrs-cli/tests/native_offline.rs),
[VFS CLI tests](../../crates/layrs-cli/tests/native_vfs.rs),
[shared recovery tests](../../crates/layrs-store-local/src/projection/tests/shared/recovery.rs),
[complete VFS recipes](../../crates/layrs-client-core/tests/native_vfs_worktrees.rs),
and [native provider recipes](../../crates/layrs-vfs/tests/windows_native.rs).
Their existence or historical results do not mark this runbook's current recipes
as passed. Root owns execution, independent review and the final qualification record.
