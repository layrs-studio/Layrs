# Native client directory moves

`preview_projection_directory_move` finds physically present source candidates
only among writable, observed snapshot file leaves. It rejects reserved or
overlapping roots before network I/O and maps at most 1024 files to distinct
destination leaves. Every source and mapped destination receives a fresh exact
path classification in sorted batches of at most 200. Each batch is bound to
the principal, Space, Step, root Tree, policy set and authorization epoch.
Source targets and executable bits must match the snapshot; destinations must
be writable, readable absences. A fresh bootstrap and local generation/digest
check follow the batches.

Only after those checks does the client atomically observe destination
absences and request the Store3 preview. Store3 scans the complete pinned
checkout and rejects unknown source files, reparse paths, collisions and
unobserved destination subtrees. The native preview contains the local preview
and the exact classifications used to prepare it.

`apply_projection_directory_move` recomputes the candidate paths, checks the
local generation/digest and original classifications, then repeats all remote
classifications and bootstrap before entering Store3's exclusive journaled
application. The local engine rechecks the full directory, source identity and
destination absence before the physical move. A preview conveys no lasting
write authority; revocation between the final remote check and the local move
cannot retract already moved bytes, and publication still requires its own
fresh server guard.

Focused Windows client tests cover reserved parent rejection, exact rights
and absent targets across batches, epoch changes, and Store3 rejection of an
unobserved source file.
