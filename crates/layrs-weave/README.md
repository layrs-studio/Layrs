# Native reconciliation and supervised WASM

`reconcile_file` is the pure text/code engine. Accepted semantic Weaves use
`WasmHost`: the exact bundled module runs without imports, with fuel and linear
memory limits inside a separately supervised, explicitly pinned worker.
`reconcile_in_worker` alone is resource supervision and **cannot** attest a
semantic receipt: its native process is not an ambient-capability sandbox.

`WasmHost::open(path, expected_sha256, limits)` supports Windows and Linux x86_64.
The executable path and expected digest are trusted deployment inputs, never
repository content. A mismatching pin, malformed protocol or changed request
rejects execution; no integrity failure silently selects structural fallback.

| Bound | Windows | Linux x86_64 |
|---|---|---|
| Whole worker memory | Job committed-memory limit | `RLIMIT_AS` hard and soft address-space ceiling |
| Process count | Job active-process limit 1 | seccomp denies fork/vfork/process clone before exec |
| Worker lifetime | Job closes/kills | parent-death SIGKILL, kill/wait on completion/error |
| Runtime | Parent deadline, bounded stdout/stderr and stdin | Same |
| Guest | No imports; wasmi fuel, memory/table bounds | Same bundled module and bounds |
| Executable identity | Handles pin file and ancestors | No-follow component opening, hash, immutable sealed memfd snapshot |

Linux permits threads sharing the sole process address space; they cannot create
another process. `clone3` returns ENOSYS so pthreads can use checked `clone`.
The filter rejects alternate syscall ABIs and modifications of resource limits
or parent-death policy; querying limits remains permitted. RLIMIT_NPROC is not
used as a process-count guarantee because its behavior depends on UID/privilege.

Linux requires seccomp filters/no-new-privileges, executable sealed memfd and
`/proc/self/fd`. Missing/forbidden kernel facilities or impractically small memory
limits produce explicit execution failure, without relaxing limits or reopening
an unpinned path. Only native ELF worker images are accepted. Paths containing
symlinks, including parent components, are refused; replacing the source file
after host creation cannot alter the sealed image. The platform's dynamic loader
and system libraries remain part of the trusted installed host environment.

The process memory bound is not an RSS reading and does not cover the parent
application or executable-snapshot preparation. Linux address space also counts
mapped images/libraries and reserved regions, so the same numeric ceiling can
reject workloads accepted by Windows committed-memory accounting. Resource
failures are refusals, not successful receipts. Fuel and guest growth failures
have bounded typed replies; an OS-level termination can still be `ProcessExit`.

Default fuel is 100 million, maximum configurable fuel one billion. Windows and
Linux real-worker fixtures both accept 1 KiB lines at the default and 16/64 KiB
lines with explicit one-billion fuel/30-second deadline; the larger fixtures
refuse the default budget. These are bounded examples, not arbitrary-input
performance guarantees. macOS and other Linux architectures remain unsupported
for supervised semantic execution; the pure/structural engine remains separate.

Tests cover memory exhaustion, exact/unraisable limits, process-creation refusal,
deadline/output/input, parent death, image seals/replacement/symlinks, exact guest
output, pins and long lines. Linux recipes run in a network-disabled Docker
container with read-only sources; see [T2 journal](../../docs/plans/pme/journal/T2.md).

Kernel contracts: [resource limits](https://man7.org/linux/man-pages/man2/getrlimit.2.html),
[seccomp](https://man7.org/linux/man-pages/man2/seccomp.2.html),
[memfd](https://man7.org/linux/man-pages/man2/memfd_create.2.html),
[parent-death signal](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html).
