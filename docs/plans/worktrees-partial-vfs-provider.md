# Windows CFAPI provider prototype

Implementation handoff, 24 October 2026. This is the bounded provider portion of
the approved L4/L5 programme, not completion of native Worktree integration.
Changes owned here are `crates/layrs-vfs/**` and this document. The parent owns
workspace membership, Cargo.lock, CAS adaptation, capture wiring and independent
review. No commits, pushes, deployment, production conversion, feature enabling
or system installation were performed by this implementation agent.

## Decision and operating limits

The inspected machine has CldApi.dll and a running CldFlt, while ProjFS is absent
and enabling it requires elevation. The prototype uses actual CFAPI registration,
placeholders and hydration. It never falls back to eager file copies.

Registration now requests ALWAYS_FULL namespace population and PROGRESSIVE hydration,
without automatic dehydration, unrestricted placeholder management or hardlinks.
Connections block self implicit hydration. All file names are created as metadata
before the root becomes reconnectable. Only regular files and derived native
directories are supported. The initial namespace deliberately accepts ASCII
Windows-safe RepoPaths only: case aliases, DOS devices, ADS, tilde aliases,
trailing dots/spaces, duplicate paths and file/directory collisions are refused.
Catalog limits are 100,000 namespace entries, 32 MiB encoded metadata, 1,024-byte
paths, and depth 64. Unicode namespace support needs an NTFS-aware alias policy.

**PARTIAL was used only in the historical smoke run below.** The official hydration-policy
enumeration page explicitly warns that PARTIAL is not currently supported, even
though the registration page describes it. This host accepted that policy, but
the measured small reads caused whole-file requests. The parent approved a narrow
switch to the documented PROGRESSIVE policy before provider integration.
Registration and reconnect validation share the same policy constant. Historical
PARTIAL registrations are rejected on reconnect rather than silently migrated;
their retained directories and tickets remain intact.

PROGRESSIVE can continue requesting data after a foreground read completes until
the file is resident or the last user handle closes. Requests need not start at
zero or arrive sequentially. It does not promise page-range residency savings.
New-policy execution evidence is separate below; the historical success is not a
claim about the changed policy. See [Microsoft's hydration policies](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ne-cfapi-cf_hydration_policy_primary).

## Public contract and module ownership

- `Catalog::new(version, entries)` freezes validated path, immutable FileId and
  logical size metadata. A deterministic FileId digest binds the catalog and
  version. Identical FileIds must have identical sizes.
- `SnapshotSource` exposes that catalog and cancellable `read_exact_at` by FileId.
  The source must retain verified immutable bytes, fill the complete requested
  buffer or fail, and honor cancellation with bounded latency. A local CAS adapter
  can implement this without depending on client-core. The provider does not
  re-hash a whole file on each range request: adapter verification is mandatory.
- `Session::create(explicit_root, root_id, source)` accepts an existing empty
  disposable local NTFS directory. Root and ancestor handles deny directory
  replacement while in use. Reparse, UNC, device, relative, short-name and dot
  aliases are refused. Existing/nested registrations are not adopted.
- `registration().encode()` produces a bounded ownership ticket to persist
  **outside** the root. `Registration::decode` plus `Session::reconnect` compares
  the catalog, CFAPI root blob, volume serial and native directory file ID.
  Two roots can use the same `Arc<dyn SnapshotSource>` and keep private NTFS bytes.
- `disconnect` and `connect` retain the registration and all user files. Drop
  attempts disconnect, never unregisters or deletes. A failed disconnect reports
  its HRESULT to explicit callers; Drop conservatively leaks the callback context
  and DLL reference until process exit if it cannot establish disconnection.
- `namespace` enumerates current files plus original catalog paths, preserving
  deletion candidates. `inspect` returns guarded `Inspection` evidence without
  reading contents. `hydrate_for_hash` explicitly hydrates for a hashing consumer
  in the provider process, whose implicit hydration is blocked.
- `capability` dynamically loads System32 CldApi and queries platform version;
  this alone is not proof that registration will succeed. Non-Windows callers get
  an explicit unsupported capability result. The Windows Session API is cfg-gated.
- `hydration_requests` retains at most 64 completed request traces;
  `callback_failures` retains at most 32 bounded diagnostics. Traces record kernel
  required/optional ranges, process ID, transfer boundaries, source/transfer bytes
  and completion status. These are diagnostic evidence, not a durable audit log.

Code is split into catalog/range/evidence/registration contracts and Windows
`api` (dynamic bindings), `fs` (root/handle checks), `callback` (hydration and
lifetime), and `mod` (Session lifecycle/inspection). Dependencies are layrs-core
and existing windows-sys 0.61.2 bindings; no third-party provider framework.

## Hydration, lifetime and failure handling

The callback copies no borrowed pointer into asynchronous work. It synchronously
serves at most eight concurrent requests, each using a 256 KiB read buffer.
Checked arithmetic validates bounds, including CF_EOF. Transfers align starts to
4 KiB and lengths to 4 KiB except at exact EOF. Required ranges determine reads;
optional prefetch hints are recorded but not served. TRANSFER_DATA carries the
original connection, transfer and request keys, and correct union-member ParamSize.

Missing content fails with a cloud-file NTSTATUS and a precise source diagnostic;
it never becomes a zero-filled successful file. The public error keeps exact
HRESULTs for Win32/CFAPI failures. Whole-request cancellation signals the source;
subset cancellation leaves still-required bytes serviceable. Shutdown signals all
pending source reads, disconnects, and retains the callback context through all
in-flight callbacks and their failure completions. Rust source panics are caught
at the fetch boundary. Bounded shutdown latency still depends on the source
honoring its cancellation contract.

Windows documents protection against overwriting modified/unsynced file bytes
during TRANSFER_DATA. This provider also refuses hydration after a local size
change rather than guessing a new immutable base. Rename and dehydration requests
receive explicit NOT_SUPPORTED acknowledgements. See [callback range semantics](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ns-cfapi-cf_callback_parameters),
[transfer requirements](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ns-cfapi-cf_operation_parameters),
and [disconnect lifetime](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfdisconnectsyncroot).

## Capture evidence boundary

`CfGetPlaceholderInfo` supplies identity, in-sync state, modified byte count and
residency while a read-access handle is used only for metadata queries. Attribute-
only handles were found not to enforce the intended sharing restrictions; see
the independent-review correction below. An unchanged partial
placeholder with the expected immutable identity and logical size can retain its
FileId without hydration. Dirty files, ordinary files and fully resident files
require exact hashing by the upper layer. Timestamps never prove unchanged bytes.
The immutable per-entry blob identifies the original catalog entry, even when the
current file name differs; a detected rename is explicitly unsupported instead
of substituting the current path's base identity.

Absence in a completed ALWAYS_FULL namespace is a deletion candidate. A present
placeholder with zero resident bytes is not deleted. A root still marked as being
populated is not reconnectable, so an interrupted initial creation cannot turn
uncreated entries into apparent deletions. No failure path erases the incomplete
root; recovery/cleanup requires explicit future tooling.

Inspection is per-file evidence, **not an atomic capture**. The caller must
quiesce/reconcile the complete namespace and writers before publication, retain
guards while consuming unchanged evidence, and hash dirty/full files exactly.
Deletion observations can race with concurrent namespace writers. The crate does
not publish snapshots, admit CAS data, or integrate native Worktree capture.
See [read-only placeholder information](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetplaceholderinfo)
and [registration/tracking policies](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfregistersyncroot).

## Historical PARTIAL execution evidence

The parent owns the Cargo slot and executed these checks using native Rust 1.85,
VS2019 and the installed Windows SDK. This implementation agent did not run Cargo
or modify the shared lockfile.

1. Windows provider compiled. The first pure run passed 6/7: the alias fixture
   incorrectly unwrapped a RepoPath already rejected by core. The fixture was
   corrected to require rejection at either validation boundary. The current
   rerun passed **7/7** (catalog, identity, range, state, cancellation, missing
   bytes and ticket tests); the implementation agent inspected
   `.layrs-local/validation/l4-provider-instrumented.log`.
2. The first real two-root run registered successfully and served native reads,
   then failed its page-residency assumption. Its log is
   `.layrs-local/validation/l4-native.log`. No data-integrity failure was hidden.
3. After request instrumentation, the parent ran
   `cargo test --locked --target-dir .layrs-local/target -p layrs-vfs --test windows_native -- --ignored --exact two_roots_real_cfapi --nocapture`.
   It passed **1/1** in **0.72 s**. The implementation agent read the full raw log
   `.layrs-local/validation/l4-native-instrumented.log`.

The successful run reported CFAPI build **26100**, revision **9278**, integration
**1576 (0x628)**; this is the API platform report, distinct from the host OS build.
Two application reads of 71 bytes each caused two kernel requests for **4,194,341
bytes each**, starting at zero. Source and successful transfer totals were both
**8,388,682 bytes**, exactly the aligned kernel requirements, not optional hints.
Both accessed files became fully resident. No within-file space saving is proven.

The same real run verified:

- Zero source reads during initial namespace enumeration and clean metadata
  inspection; untouched files remained nonresident after other files were read.
- Correct file bytes returned through actual kernel callbacks to child processes.
- Two roots held different native writes to the same path; shared immutable
  source bytes remained unchanged and dirty evidence required hashing.
- Missing source bytes failed with Win32 **389**; provider-down reads failed with
  Win32 **362**; rename refusal returned Win32 **379**.
- Both private variants remained readable while disconnected and after reconnect.
- A fresh provider process reconnected from the persisted ticket, hydrated from
  the immutable source, and exited without Session::drop; the parent reconnected
  afterward and recovered the files.
- Namespace deletion in one root did not delete the other root's file; a tampered
  root ownership ticket was rejected.

Retained successful fixture:
`.layrs-local/vfs-smoke/layrs-vfs-23872-1792805717587165900`, including `a.ticket`,
`b.ticket` and both disconnected registered roots. No automatic cleanup was run.
The final test summary string was corrected after this run to say exact
kernel-requested hydration instead of partial hydration; that text-only change
does not imply another native execution.

To repeat, supply an explicitly disposable absolute directory through
`LAYRS_VFS_TEST_PARENT`. The test creates a unique run directory beneath it and
retains all roots, even on failure. Select the exact test above: the separately
ignored `native_child_io` test is an internal child-process helper and must not be
invoked without its helper environment.

## PROGRESSIVE policy and P1 guard execution evidence

The approved change updates only the provider's registration/reconnect policy,
cancellation coverage validation/tests, native smoke assertions and this document.
ALWAYS_FULL population, no automatic dehydration/hardlinks, and
BLOCK_SELF_IMPLICIT_HYDRATION remain enforced. No existing registration is updated.

Cancellation review: a cancellation covering a complete fetch signals that fetch's
source token; a subset does not fail bytes still needed outside it. Transfer and
request keys isolate concurrent/random-access fetches. Subset cancellation may
still perform extra source IO in the bounded chunks, rather than incorrectly
failing the entire request. Malformed/overflowing cancellation ranges do not
cancel another valid request. Added tests cover overlapping/disjoint ranges,
CF_EOF, nonzero offsets and invalid arithmetic. Real cancellation/close races
under PROGRESSIVE remain a native qualification item.

The parent ran the PROGRESSIVE provider after correcting the P1 guards. The
implementation agent inspected `.layrs-local/validation/l4-p1-guards-unit.log`
(**9/9 unit tests passed**) and
`.layrs-local/validation/l4-p1-guards-native.log` (**1/1 exact native test passed**,
0.98 s). Retained fixture:
`.layrs-local/vfs-smoke/layrs-vfs-51176-1792838427874134300`.

Before any hydration, the native child attempted five prohibited operations:
file WRITE_DATA, file DELETE, parent DELETE, root DELETE and parent rename. All
five returned sharing violation **32**. Assertions verified **0 source reads**
and **0 resident bytes** before and after these probes. A provider-process
implicit content read separately failed with Win32 **395**, with source reads
and residency still zero. This proves effective write/delete exclusion for the
tested guards; it does not prove that directory handles freeze child creation.

The same run retained two-root divergent writes and service/process restart
checks. Its two 71-byte application reads again produced full-file kernel
requests: **8,388,682 required bytes**, exactly **8,388,682 source/transfer bytes**.
Both accessed files became fully resident. No page-range saving is claimed.
The callback-prefix and control/checkpoint changes described below were made
after this run and require a new serialized compiler/native run.

## Remaining qualification and handoff

Independent technical/specification review is required and owned by the parent.
The native smoke uses verified immutable in-memory fixtures behind the real CFAPI
provider. A verified local CAS adapter, admission/pinning, persisted Worktree
registration and atomic mixed native/immutable capture remain integration work.
No mock provider success was used as native evidence.

Not yet qualified: pending-I/O cancellation races under the real driver, crashes
during initial population, simultaneous read/write stress, mmap, watchers, flush
and durability fault injection, disk-full recovery, rename reconciliation,
dehydration, Unicode/case-sensitive namespaces, Windows versions/volumes beyond
this machine, non-Windows builds, and session teardown under an uncooperative
source. A successful process restart is not a power-loss durability proof. The
provider is not a filesystem security sandbox; ownership blobs prevent accidental
foreign adoption, not a malicious same-user process forging metadata.

## Independent-review corrections and integration contracts

The critical review reproduced a P1: FILE_READ_ATTRIBUTES-only opens did not
exclude another handle requesting WRITE_DATA or DELETE. Therefore historical
smoke success did not qualify the previous Inspection as frozen evidence. The
correction uses FILE_READ_DATA for files and FILE_LIST_DIRECTORY for directory
identity guards, with OPEN_REPARSE_POINT and self implicit hydration blocked.
Inspection denies write/delete sharing; parent/root identity guards deny delete
sharing. The new native proof requires actual Win32 sharing violations for file
writes/deletes and parent/root deletion/rename, alongside zero source reads and
zero residency. The five-operation native proof above passed before capture
checkpoints were added.
Directory identity guards alone do not claim to freeze creation of child names.

Accepted integration API:

- `Session::create_with_registration_hook(root, root_id, source, persist)` invokes
  `persist(&Registration)` after validation and **before the first native
  registration**. The caller durably records ProviderPrepared there.
- `Registration::filesystem_identity()` returns the existing `fs:v1` format.
  `Registration::verify(&Catalog)` is read-only and never connects another
  provider. A matching registration with incomplete population returns NeedsRepair.
- `publish_control_file(bytes)` creates only `.layrs`, after population;
  `attach_control_file(&ControlFileIdentity)` validates it on reconnect.
  The bounded encoded capability binds native file identity and content FileId.
  Namespace exclusion applies only while that exact capability is attached.
- `read_control_file(root)` is a bounded, fixed-name, non-reparse/single-link
  discovery read returning bytes and identity. These bytes are locator hints
  until the client validates the private journal, registration and capability.
- `checkpoint()` now retains file/directory handles, exposes entries and guarded
  `with_reader`, and allows complete namespace
  revalidation. The client retains it through Store CAS and acknowledgement.
  It does not obtain an atomic namespace merely by opening directory handles.

The parent owns Store locking and journal phases. Incomplete population is retained
as NeedsRepair/export-only; it is never automatically repopulated or deleted.
P2 now parses raw size prefixes before individually reading the exact fields
used. It never creates a reference to a potentially short external callback or
parameter struct. Fetch validates through OptionalLength, cancellation validates
both range fields, and process info validates its own prefix. Three tests cover
every shorter prefix, exact required lengths and an unaligned buffer.

Control discovery reads at most 64 KiB from a plain, single-link, fixed `.layrs`
file while retaining effective read guards. Its encoded capability binds volume,
root/file native identities, byte length and content FileId. Publication uses
CREATE_NEW and flushes bytes before reading back that exact identity and content;
failure never deletes a replacement. Namespace/checkpoint traversal excludes
only the attached capability, verified before reserved-name RepoPath validation.
Other control-looking paths receive ordinary validation.

Checkpoint readers use retained handles only for NeedsHash entries and require
the consumer to read the exact observed length. Clean immutable entries cannot
be read through that API. Namespace revalidation compares names, types and native
identities, including empty directories; the caller must still quiesce namespace
writers and retain the checkpoint through CAS and acknowledgement. A final scan
is not an atomic filesystem snapshot or protection against later child creation.

The expanded native test covers durable hook ordering, read-only verification
of a live registration, control-byte identity and write/delete refusal, zero-read
clean checkpoints, exact guarded dirty reads, observed namespace changes, control
reattachment and retained incomplete-population NeedsRepair. These additions,
the P2 short-buffer tests and fresh independent review are pending at handoff;
the earlier 9+1 result must not be represented as their validation.

## Step1 control-directory metadata foundation: qualified checkpoint

This additive Step1 checkpoint is limited to an ordinary `.layrs` directory with
exactly one immutable, single-link `FORMAT` file. All mutable state and receipts
remain external. It does not change Session control attachment, checkpoint
exclusion, source access, registration/population or activation behavior. The
legacy complete-Store control-file APIs remain available; file and directory
layouts refuse one another with typed errors. Earlier L4/R9 evidence remains
separate: [26 R9 unit tests](../../.layrs-local/validation/l4-provider-r9-lib.log),
[retained export-parent guards](../../.layrs-local/validation/l4-provider-r9-parent-guard.log)
and [cloud visibility](../../.layrs-local/validation/l4-provider-r9-visibility.log)
are not reclassified or broadened by this Step1 result.

### Callable API and authority boundary

The public comparison types and codecs are in
[control_directory.rs](../../crates/layrs-vfs/src/control_directory.rs); Windows
proofs are in
[windows/control_directory.rs](../../crates/layrs-vfs/src/windows/control_directory.rs),
with the one-attempt publisher in
[control_directory_publish.rs](../../crates/layrs-vfs/src/windows/control_directory_publish.rs).

- `ControlRootPreflight::open(root)` retains a canonical, initially empty,
  ordinary local NTFS root and its ancestors. `root()` supplies the canonical
  path for the caller's FORMAT bytes. `revalidate()` checks native/path bindings,
  attributes, streams and exact absence of sync-root registration, not perpetual
  emptiness after successful publication.
- `observe_control_directory_stage(&preflight, staged_control)` observes an
  **empty FORMAT** allocation and returns `ControlDirectoryStageIdentity` data.
  Its getters expose canonical root/parent/stage paths and native root, parent,
  directory and FORMAT identities for comparison with the caller's independently
  owned allocations. Observation is not an ownership capability. Recovery uses
  the recorded allocation data; it does not rerun empty-FORMAT observation.
- `publish_control_directory_recoverable(&preflight, allocated.stage(),
  &allocated, format_bytes, prepared, persist, faults)` requires allocation data
  on **every call**, including `prepared: None`. It compares that allocation
  before persistence or rename. `persist(&ControlDirectoryIdentity)` runs while
  the strong FORMAT pin is held. `Some(&prepared)` never invokes the callback.
  The provider neither creates/writes FORMAT nor authenticates Store receipts.
- `ControlDirectoryGuard::open(root, &expected)` returns a retained,
  non-serializable layout proof; `try_clone()` duplicates the owned directory
  pin and gives FORMAT an independent reader. `identity()`, `format_bytes()` and
  native identity getters expose the bound values. Generic `revalidate()` permits
  a populated root and performs no registration/readiness check.
  `revalidate_bootstrap_root()` separately requires only exact `.layrs` in the
  root and exact NOT_UNDER checks on held/reopened ancestry, control and FORMAT.
  Fresh final recovery needs no empty-root preflight and no surviving historical
  stage path. Publisher final replay additionally enforces the bootstrap layout.
- `discover_control_directory(root)` returns distinct `ObservedControlDirectory`
  hints. Its borrowed `identity().matches_expected(&expected)` can compare but
  cannot serialize, promote or convert the observation into a guard. Public
  `decode` on either expected identity type also yields comparison data only.
  Store must authenticate its durable allocation/prepared receipt and bind its
  private state independently before any authority or effects.
  Discovery can return an observation whose derived control path exceeds the
  expected codec's path bound. This remains harmless, unmatched comparison data:
  it cannot match a valid expected identity or be promoted into a guard.

The `LYRCSTG1` codec binds root/parent/stage paths, volume and all four native
object indices; `LYRCDIR1` adds FORMAT byte length and content FileId. Directory
identity strings use hexadecimal `fs:v1`; FORMAT's compatibility accessor uses
decimal `volume:high:low`. Exported allocation/receipt budgeting limits are:

| Constant | Maximum |
| --- | ---: |
| `CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES` | 4,096 UTF-8 bytes per canonical path |
| `CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES` | 12,344 encoded bytes |
| `CONTROL_DIRECTORY_MAX_ENCODED_BYTES` | 12,431 encoded bytes |
| `CONTROL_DIRECTORY_MAX_FORMAT_BYTES` | 65,536 FORMAT bytes |

Derived `root/.layrs`, `root/.layrs/FORMAT` and `stage/FORMAT` must fit the same
path limit before allocation/persistence effects. These are byte bounds, not
character or UTF-16-unit counts. The caller owns FORMAT's canonical schema; the
provider verifies bounded exact bytes, length, real EOF and FileId only.

### Native publication and retained historical failures

Root/ancestor pins exclude deletion. The control guard owns the sole
DELETE-capable directory pin; additional proofs use `DuplicateHandle`, while
read/list observers deliberately share READ, WRITE and DELETE without requesting
DELETE. An old share-READ/WRITE observer is incompatible. FORMAT uses a strong
read pin. Stage and FORMAT require protected current-process-owner-only DACLs,
verified by handle with READ_CONTROL; the stage parent may be an ordinary
inherited directory. No existing ACL is repaired. Token impersonation, other
owners and case-sensitive user aliases are not broadened by this contract.

Two earlier recipes remain terminal failures, with source and raw logs preserved:
[strong FORMAT pin](../../crates/layrs-vfs/tests/support/control_directory.rs)
returned Win32 **5 / 0x80070005** on directory rename
([log](../../.layrs-local/validation/l5-control-directory-feasibility-r1-fixed.log));
[retained FORMAT with DELETE sharing](../../crates/layrs-vfs/tests/support/control_handoff.rs)
failed the same way in all four arms
([log](../../.layrs-local/validation/l5-control-retained-handoff-r1.log)). The later
[close-window R1](../../.layrs-local/validation/l5-control-close-window-r1.log)
and [close-window R2](../../.layrs-local/validation/l5-control-close-window-r2.log)
feasibility passes do not turn those failures into successes.

The new-publication path closes **only FORMAT** after the durable callback and
attempts a no-replace rename using the retained directory handle, then acquires a
fresh strong FORMAT pin and compares native identity, links, full bytes/EOF, privacy,
attributes, streams and closed layout before issuing the final proof. Each call
makes **at most one no-replace rename attempt; no internal retry**. Validation
refusal and exact-final replay can make zero attempts. Absolute destination
resolution retains ancestor pins and checks resulting identities; it is not a
relative-rename or atomic-namespace
guarantee. A raw Win32 5 remains `RenameRefused`, with operation, point and original
cause, not an inferred reader conflict or generic Busy classification. Historical
feasibility log labels do not define the current error contract. A persistence
error means `ReceiptDurabilityUnknown`: retain files and reread the durable
receipt. `NeedsRepair` grants no deletion, adoption, rewrite or cleanup authority.
Nested errors can carry the generic `BeforeProof` point during a later layout or
bootstrap revalidation. That diagnostic label does not assert that publication
is occurring or grant publication authority.

### Corrected R1 findings and fresh execution evidence

The Step1 SPEC R1 path-bound finding was corrected before this checkpoint:
preflight checks derived control paths before filesystem effects, and allocation
validation checks the stage/FORMAT suffix. ASCII and multibyte boundary tests
cover both. Critical R1 corrections add exact NOT_UNDER checks on held/reopened
stage and FORMAT before persistence/close/rename, and final control/FORMAT before
and after bootstrap validation. Only **0x80070186** proves NOT_UNDER; other results,
including success for an actual registration, refuse the bootstrap proof. The
last root/ancestor check runs after final opens and checks, immediately before
proof issuance. These pins do not prevent concurrent native registration.

Generic layout open/revalidate/discovery no longer uses root `FindFirstFileW` or
enumerates user names. Canonical child spelling/native IDs remain required;
closed FORMAT-only control enumeration remains. Stage/control attributes use a
closed DIRECTORY plus READONLY/HIDDEN/SYSTEM/ARCHIVE/NOT_CONTENT_INDEXED policy.
FORMAT permits NORMAL plus those benign flags, rejecting reparse, directory,
OFFLINE, SPARSE and unknown flags. Links, streams and attributes are revalidated;
share locks do not make those metadata immutable.

The root executed the corrected source, and the implementation agent inspected
the following raw results. No author Cargo/native execution was used:

| Root-run target | Result and raw evidence |
| --- | --- |
| `cargo test -p layrs-vfs --lib` | **33/33 passed**, 1.32 s; [lib-r2](../../.layrs-local/validation/l5-control-step1-lib-r2.log) |
| Exact ignored `control_directory_step1_native` | **1/1 passed**, eight gates, 8.66 s; [native-r1](../../.layrs-local/validation/l5-control-step1-native-r1.log) |
| Exact ignored `control_directory_step1_process_recovery` | **1/1 passed**, 4.42 s; [process-r1](../../.layrs-local/validation/l5-control-step1-process-r1.log) |

The native run proved actual sharing violations **32**, compatible share-ALL
observation, drop-all-handles final reopening without the historical stage path,
populated-layout versus bootstrap refusal, immutable allocation/foreign-collision
checks, explicit retry after the leaked-reader rename failure, and six actual
close-window mutations refused before final proof. Native attribute, stream and
hardlink mutations succeeded and were detected on revalidation. The bounded
non-Microsoft/GUID reparse mutation also **succeeded and was detected** in this
new run; it does not retroactively qualify the older denied reparse attempts.
Native publication-time registration injections ran at **AfterPersist** and
**BeforeProof** and were refused by the production bootstrap boundaries. Other
publication registration-boundary placements are source-inspection evidence,
not separately injected native races. Prepared-identity mismatch, a final
directory present without prepared identity, and foreign same-byte final replay
are likewise source-proven branches; the no-replace foreign collision was tested
natively. The privacy negative covers an unprotected FORMAT DACL, not extra ACEs,
wrong masks/owners or security-query IO faults.

Real production discovery/open/revalidation on a connected cloud root reported
`FETCH_DATA=0`, `FETCH_PLACEHOLDERS=0`, notifications=0 and malformed callbacks=0.
Fetch and malformed counts were asserted zero; notification zero was only logged.
The fixture uses **ALWAYS_FULL/PROGRESSIVE**, disables on-demand population on the
root and keeps the control objects ordinary. With no positive control establishing
that enumeration could trigger a callback, the zero-counter result is
non-discriminating: it proves these APIs work while connected under this policy,
not arbitrary callback avoidance or behavior under other providers/population
policies. It does not claim within-file residency savings.

The process recipe exited **117** at each of `StageValidated`, `AfterPersist`,
`PreClose`, `PostClose`, `PreRename`, `PostRename`, `PreStrongPin`, `BeforeProof`;
the eight fault labels cover **six distinct handle/disk states**, not eight
independent configurations. For each fault the parent launched a publisher child
and then a fresh resumer child, which exited **0**. The original parent performed
the final fresh expected-open/native identity/byte/bootstrap checks after both
children exited; there was **no third verifier child**. Prepared receipts were
unchanged and `Some` never repersisted. Both recipes require an explicit
`LAYRS_VFS_TEST_PARENT`; fixtures, registrations, receipts and child logs remain
retained. The internal `control_directory_step1_child` is not a standalone target.

[The source/log manifest](../../.layrs-local/validation/l5-control-step1-r2-source.json)
records SHA256 for 14 selected source/log entries at the tested checkpoint and
remains immutable. The subsequent approved publisher change is comment-only:
"exactly one" was corrected to "at most one"; the root owns any refreshed
manifest, and the original runtime logs are unchanged. The recipe source is
[control_directory_step1.rs](../../crates/layrs-vfs/tests/support/control_directory_step1.rs),
with minimal [native-test wiring](../../crates/layrs-vfs/tests/windows_native.rs).
Native fixture: `.layrs-local/vfs-smoke/layrs-vfs-control-step1-native-51340-1792869112220148800`.
Process fixture: `.layrs-local/vfs-smoke/layrs-vfs-control-step1-process-31276-1792869203548386400`.

**Step1 SPEC R2 passed without a blocking finding. Critical R2 qualified the
bounded Step1 scope with no Critical, High or Medium findings.** The accepted
evidence limits and the SPEC P3 wording correction are recorded above. Executable
production and test contents stay frozen; these verdicts grant no Store work.
This is a metadata foundation, not production Store bootstrap, a complete
population/ProviderRootGuard capability, Session/checkpoint directory integration,
source/readiness authority or L5 completion. A populated layout guard alone grants
none of those. Successful process recovery is not power-loss durability, atomic
namespace publication, arbitrary-provider compatibility or a registration lock.

Registrations are intentionally retained with their fixtures. Any later manual
cleanup requires an explicit operator action on the **exact recorded test paths**,
after verifying the recorded native ownership identities and registration. Only
that separately authorized operation may call CFAPI unregister for those roots;
this document performs or authorizes no cleanup now. A name prefix, matching
bytes or an arbitrary registration is never sufficient ownership evidence.
Unregistration must not silently adopt or delete files, and failed verification
must leave the fixture and evidence intact. There is no automatic cleanup path.
