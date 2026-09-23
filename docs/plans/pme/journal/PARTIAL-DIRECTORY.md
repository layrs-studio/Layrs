# Store3 directory moves

`preview_directory_move(generation, source, destination)` requires an actual
pinned source directory and an absent destination whose parent already exists.
Every present source file must have a writable materialized-file or explicit
absence observation. Every mapped destination file must have a separate
writable absence observation. Reserved subtrees, aliases, overlaps, unknown
paths and destination collisions are refused. The preview records the source
directory identity, complete observed source file descriptors, and checkout
digests before and after the move. The full resulting capture must fit the
1024-change, 1 GiB/file and 4 GiB limits.

`apply_directory_move` recomputes the preview under the exclusive local lock,
then durably writes a canonical version 1 directory move journal before the
physical rename. The pinned directory move never replaces the destination.
Recovery accepts a moved directory only if its identity and the complete
checkout scan match the journal. Changed or replaced source/destination paths
leave the journal pending for explicit repair. Capture, history and pending
journal removal commit in one SQL transaction. Completed replay checks the
immutable capture and history without inspecting the live checkout.

Directory moves share the namespace pending/history tables with file namespace
operations. Schema dispatch preserves the older 64 KiB journal bound and uses
the widened 8 MiB bound only for directory moves. `resume_namespace` resumes
either operation; `fsck` verifies both history forms.

Independent critical verification completed. Fixed final-commit races by
rechecking the full checkout, directory identity and source absence after the
last fault boundary; completed history binds the exact capture digest.
Windows directory filter: 15 passed; Linux: 16 passed. Additional Windows
long-path preflight regression passed. Source/mapped paths above4096 bytes or
mapped depth above128 are rejected before journaling or physical relocation.

External Unix namespace mutation can cause a physical move before rejection;
the pending journal and all bytes are retained. This is not a namespace lock
against arbitrary external programs. No macOS/other-machine qualification.
Native client/CLI/Desktop wiring is a subsequent lot.
