# Provider recovery export checkpoint

This checkpoint describes the provider-only export implementation in the Windows
Store. It does not describe managed import, source repair, or SCM history
creation. Native qualification and a retained source/binary fingerprint are
pending the root's serial test run.

## Entry point and retained proof

`ProviderActiveSession::export_quiesced(&mut self, parent, cancel)` requires a
connected, Active/Ready provider session. The caller must keep editors, agents,
build processes, and every other checkout writer quiescent until the response.
The method acquires the authenticated allocation receipt's capture lane for
the whole operation. It validates the full private Active/capture state and
family registration before and after content reads, without holding owner or
catalog locks across a source callback or native file read. A pending Hashing
or Staging capture is refused before output creation.

The VFS `ExportCheckpoint` retains namespace and native file proofs. Its
`original_path` accessor resolves a CFAPI identity against the fixed catalog;
this distinguishes a moved placeholder even when two catalog paths share the
same FileId. An ordinary replacement has no CFAPI origin and is accepted only
when its path-derived base matches the fixed descriptor at that path. The
checkpoint and the destination ancestor guard are revalidated through export.
The destination must be outside the provider root, family root, private
metadata and provider control, including native path aliases. Export creates
ordinary files under a fresh `layrs-vfs-export-<id>` container; it does not
write the source root, A/P receipts, provider SQL or family catalog.

## Files, gaps, and completion

The full checkpoint file entry set is preflighted, and the native namespace
guard is revalidated, before content reads. Limits are
100,000 paths, 1 GiB per file and 4 GiB aggregate, with bounded path and
manifest metadata. Immutable files are read from the fixed local source,
never from a guessed native path. `NeedsHash` files, including relocated
placeholders and ordinary additions, are read through retained
`ExportCheckpoint::with_reader`. Each available file is hashed before output
creation, copied into a create-new file, and independently verified by
`VfsExportWriter`. The writer checks cancellation during verification rereads
and immediately before publishing the manifest.

Selected known files with a verified same-path origin are marked
`managedCandidate`; that label is descriptive and grants no capture authority.
Unknown paths, excluded paths, relocated placeholders and changed protected
files are marked `recoveryOnly*`. Unknown ordinary file mode is `null`, because
no fixed provider entry establishes it. Deletions remain explicit records.

An exact typed `MissingBytes` from the local source becomes a gap. Successfully
verified other files are still copied and a provider-specific manifest records
`completeSelected: false`; the API returns `Incomplete(report)`. Other native
read failures, including HRESULT failures, return `Failure` with their original
cause and leave any incomplete container intact. A failure before manifest
rename leaves no completion manifest. A failure during final publication may
have occurred after rename and requires inspection of the retained artifact;
the error response does not claim completion. `Failure.attempted_container_root` is only a proposed create-new path
when creation itself fails; it does not prove that a directory exists or grant
cleanup authority. `Failure.container_root` is set after successful creation.

`Complete(report)` requires every selected and otherwise recoverable observed
file to be verified and no gap. It means only that this provider recovery
export completed. It is not a managed Worktree receipt or a claim that
unknown/protected content was admitted into SCM history.

## Manifest

The last published `manifest.json` uses
`layrs.provider-recovery-export.v1`, separate from the legacy VFS export
schema. It binds:

- the exact allocation and prepared receipt references;
- the active mount and activation digests;
- the fixed source commitment, including catalog and selection digests;
- the validated capture sequence and digest at export time;
- a domain-separated canonical observed-image digest;
- sorted verified files with FileId, byte length, optional executable mode and
  classification; deletions with their base and expected mode; and typed gaps;
- `completeSelected`, which is false whenever gaps remain.

The digest covers observed path, content target or absence, length and mode,
without granting authority from a classification. The manifest contains no
fabricated TreeId, Step, Operation or server observation. A partial manifest
explicitly describes gaps and is never a complete-selected receipt.

## Retained qualification

`provider-recovery-export-pure-r2.log`: **109 provider pure tests pass**, with
284 native recipes intentionally ignored and 227 unrelated tests filtered.
This includes the initial import/companion DATA codecs; no managed writer or
SQL import interpreter is qualified by those codec tests. The VFS pure suite
passes **45 tests**, with 31 native recipes intentionally ignored. The exact
writer test verifies cancellation between bounded 64KiB verification blocks.
`provider-recovery-export-workspace-r1.log` records a passing
`cargo check --locked --offline --workspace --all-targets` on the same compiled
source set. All 417 retained inputs still matched their hashes before commit.

`provider-recovery-export-20261026-r1` retains EXE/PDB and all 417 source inputs.
EXE SHA-256: `cbcd5e4effc87353d5c649008da6ee3cb7503f7d0ee36a7d25c1ee22dda4c39b`;
PDB age 29. The nine retained Rust startup source checksums and current native
toolchain bindings are recorded in `host-disposition.json`. The five separately
reviewed native recipes all pass, serially in a fresh owned C: parent:

- ordinary added file plus a typed missing cold source gap;
- protected edit exported with actual verified bytes;
- ordinary replacement at a known writable path;
- family destination refused before output creation;
- cancellation after `manifest.pending` flush, preserving file bytes and
  withholding `manifest.json`.

Independent critical review covered origin identity, including equal-FileId
relocation, destination exclusions, retained guards, hashes, cancellation,
typed causes and manifest-last publication. Native relocation, concurrent
external namespace mutation, forced cancellation of blocked hydration and a
product host remain unqualified. The static host audit retains the earlier
stock Rust/MSVC/Windows loader assumptions; it does not establish resolved DLL
identity or a universal absence of process-global SQLite extensions.

The compact import/companion codecs preserve original lineage across repeated
imports. Each immutable transition receipt embeds its compact source prefix,
including the exact parent reference, so replacing the destination singleton
cannot erase earlier ancestry. These are DATA contracts only: SQL closure,
family protection, managed transition writers, selection expansion and product
integration remain later work. Reclamation keeps a separate strict evidence
fence even when ordinary imported-history admission is added later.
