# Import metadata and provider status checkpoint — 26 October 2026

## Full SQL source closure and transition intent

The bounded import reader now checks the exact schema and FORMAT ingress,
reconstructs the receipt ancestry, verifies each historical terminal version,
and rehashes twelve explicitly scoped immutable rowsets. Capture origins and
destination boundaries are resolved from that ancestry. Historical activation
maps are checked against reconstructed catalogues and their source/placement
snapshots. Captured content mappings bind lengths; companion preimages bind the
effective file at the exact path. This is SQL metadata evidence only.

The schema-8 transition intent codec binds a compact source prefix and full
immutable plan before destination creation. The final receipt separately binds
the full plan and the actual postcreation destination proof. The owner digest
is computed after creation; reservation operations include destination, prefix
and transaction. Cancellation retains acquired facts and grants no cleanup.

`provider-import-reader-check-r7.log` records a successful workspace/all-targets
check. After the final negative-fixture correction,
`provider-import-reader-pure-r3.log` records **117 passed, 292 ignored**.
`provider-import-reader-20261026-r1` retains 430 source inputs and EXE/PDB:
SHA-256 `434091c70dd544c111152bdf72391d4ebabae80a98ce32cb408ca32ba4a68e4e`,
PDB age 34. Three independently source-reviewed, bounded in-memory SQLite child
recipes pass: scoped high-water/hash checks, exact Active source/map validation,
and two successive imports with historical closure, FORMAT, terminal version,
receipt boundary and map falsifications. Each negative asserts its intended
refusal. Their database handles close observably; the fixture parent is fresh.

The SQL recipes do not yet exercise nonempty content captures or companion SQL
rows. The companion exact-path/preimage behavior has a focused pure test.
Family protection, content bytes, destination creation/writer/restart, ordinary
admission, expansion and product-host qualification remain separate work.

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
