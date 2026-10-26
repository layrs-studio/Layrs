# Import metadata and provider status checkpoint — 26 October 2026

Capture and completed-publication readers now accept an origin supplied by a
separately verified lineage index. They reuse the existing canonical SQL,
history, snapshot, selection, permission and size checks without recursively
calling conversion provenance. The ordinary paths still verify their original
provenance and content bytes. These new readers establish metadata consistency,
not content protection, native authority or destination readiness.

The client and CLI have a separate read-only entry point:

```text
layrs partial provider status --ticket FILE
```

The canonical `layrs.partial-provider-status-ticket.v1` document binds family,
Worktree, transaction and expected bootstrap binding. Its size is limited to
128 KiB. A Store helper reads it with directory-relative no-follow handles,
checks the actual file length and revalidates file/parent identity. It needs no
DELETE right on the ticket. The result is untrusted input until Store admission.

The status route reopens and revalidates Active, without connecting, activating,
recovering or scanning an ordinary checkout. Its DTO reports `active`, root,
catalogue, generation and digest. It does not grant ordinary mutation capabilities.
The CLI does **not** declare its own process-wide SQLite profile. Until a product
host is audited and explicitly declares that profile, this route returns
`process_profile_unavailable`; product-host qualification remains open.

## Retained evidence

`provider-import-seams-pure-r1.log`: **110 provider pure tests pass**, with 287
native recipes ignored. `provider-status-pure-r1.log`: the client ticket codec
and CLI routing tests both pass. The compiled SQL import reader in this snapshot
is work in progress and is not qualified by these results.

`provider-import-seams-20261026-r1` retains EXE/PDB and 424 source inputs. EXE
SHA-256 `7f1ba694c20b93f4898c4f394ffd57820e4312437f8d6198a63c786e21100723`,
PDB age 30. Five exact, independently source-reviewed recipes pass in fresh
owned C: fixtures with bounded child processes:

- imported capture/publication metadata accepts the correct origin and refuses
  a foreign origin, invalid generation, forged expected base and altered receipt;
- the ticket reader accepts a readonly regular file and refuses oversize input,
  a directory and an existing ancestor junction;
- legacy conversion preserves source/history and requires a fresh destination capture;
- imported publication receipts retain their original identity;
- completed publication replay refuses an altered transition.

These results do not qualify concurrent ticket mutation, a production executable,
the full imported SQL closure, managed transition writers, companions, selection
expansion or capture process-crash recovery. Those remain separate work. The
earlier native-host trust assumptions and audit limits still apply.
