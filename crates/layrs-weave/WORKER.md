# Native Lens worker supervision

Build `cargo build -p layrs-weave --bin layrs-lens-worker`. A trusted caller supplies
the explicit worker executable path to `worker::reconcile_in_worker` with a
canonical Lens request, file policy and resource limits. No PATH lookup or shell
is involved. Only the exact built-in code/text pins are accepted.

The Windows 10+ implementation applies a Job **at process creation**, using
`PROC_THREAD_ATTRIBUTE_JOB_LIST`, so no worker code executes before assignment.
It limits committed virtual memory (not RSS), permits one process, and kills the
Job when its last handle closes. It inherits only three pipe handles and an empty
environment. The parent enforces an execution deadline and bounded stdout/stderr,
terminates failed workers and drains/joins all pipe tasks. Input and output have
separate bounds, and the canonical response is checked against the exact request.

These mechanisms follow Microsoft's
[Job object documentation](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
and [limit semantics](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information).
The memory limit covers the entire worker including runtime/deserialization.
The default is 128 MiB, 5 seconds and 16 MiB per protocol direction. There is no
instruction-fuel limit. Deadline interruption is a rejection, never a selected
side. Launch errors also reject; there is no in-process execution fallback.

**This native-only entry point is not the ADR0010 semantic host.** Job resource limits do not remove
filesystem/network/wall-clock APIs, and the worker binary path is a trusted
deployment input, not a cryptographically verified installation. The working
directory is the worker executable's directory, never the repository. The API
returns a resource-supervised result with `eligible_for_semantic_receipt=false`;
it has no conversion to a receipt attestation. Non-Windows process hosts explicitly
return UnsupportedPlatform.

The separate accepting host below adds the required capability sandbox.

Tests launch real processes and prove deadline/output termination, memory-cap
failure, child-process refusal, environment clearing, deterministic code merge
and exact-pin refusal. The ignored unit-test entry is a subprocess fixture invoked
explicitly by those tests, not an omitted resource-bound check.

## Accepting WASM host

`wasm_host::WasmHost::open(path, expected_sha256, SandboxLimits)` verifies and
pins the deployment worker image and its ancestors against replacement for the
host lifetime. The path and expected digest must come from trusted deployment
configuration, never repository files or remote metadata. This is binary pinning,
not a signature-distribution or hardware-attestation system.

`layrs-lens-wasm` compiles the real built-in code/text reconciler to
`wasm32-unknown-unknown`. Rebuild the committed module and checksum using
`node crates/layrs-lens-wasm/build-module.mjs` after installing that Rust target.
Then rebuild the worker. Wasmi 0.40 rejects every import and any start function;
no WASI, clock, randomness, environment, network or filesystem API is supplied.
Only input bytes, output bytes and bounded linear memory cross the ABI.

Each invocation uses a fresh process and instance. Windows Job supervision caps
the complete runtime process, while Wasmi additionally bounds fuel, linear memory,
instances, tables and table elements. Defaults: 256 MiB global committed memory,
32 MiB linear memory, 100 million fuel, 10 seconds, 16 MiB protocol input/output.
Errors, traps and quota violations reject without native fallback. The parent
matches the canonical reply against the exact input, policy and module digest.

`plan_sandboxed_merge` records its private completed host configuration;
`NativeWorkspace::preview_sandboxed_merge` exposes it to the native client.
`accept_merge` persists its Step/View/Operation/Product in the same transaction.
Receipts pin the actual WASM module and record enforced memory/deadline/fuel.
The policy digest also commits the worker digest, linear-memory and wire limits.
Pure native semantic previews remain ineligible for native acceptance.

Build the worker before running client acceptance tests:
`cargo build -p layrs-weave --bin layrs-lens-worker`, then
`cargo test -p layrs-client-core --test native_merge_accept`.
Runtime tests run on any native platform; complete process supervision and
semantic acceptance currently support Windows 10+ only. A receipt validates
declared identity and policy; remote trust still depends on authenticated
publication/admission, not a claim of remotely proving an execution happened.
